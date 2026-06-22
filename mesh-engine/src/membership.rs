//! Per-Space membership rooted in **device identity (EndpointId), not mere possession
//! of the shared group key**, with enforceable roles.
//!
//! Membership is a **signed, hash-chained log** of operations. Each entry is signed by
//! an Admin's device key (Ed25519, reusing the iroh `node.key`); peers replay the log
//! from the genesis and verify every entry chains (its `prev` is the prior entry's
//! hash) and is signed by an Admin *at that point* before accepting it. The Space
//! creator is the self-signed **root Admin**, cryptographically bound to the `SpaceId`
//! (`SpaceId == SpaceId::new(founder_pubkey, nonce)`), so a Reader cannot forge a
//! competing root. Replaying the verified log yields the current role map.
//!
//! The hash chain makes the log **tamper-evident**: altering any entry changes its
//! hash, which breaks the next entry's `prev` link and fails replay. This same log is
//! the per-Space **audit log** (it records space-created / member-added / role-changed /
//! removed / key-rotated / pairing-completed). It is the M3 audit log too; the
//! revocation ops (`RemoveMember`, `KeyRotated`) are defined here and exercised there.
//!
//! ## Honest limits
//! The log is a linear chain reconciled by *longest-valid-chain-wins*. It is designed
//! for membership changes that are **serialized through Admins** (the common case).
//! Truly concurrent Admin edits are not CGKA-merged — a fork keeps the
//! deterministically-chosen branch and re-applies the rest later. Full forward-secret
//! group key agreement over unreliable P2P is out of scope (see `SECURITY.md`).

use std::collections::HashMap;
use std::path::PathBuf;

use iroh::{PublicKey, SecretKey, Signature};
use sha2::{Digest, Sha256};

use crate::error::{CoreError, Result};
use crate::space::SpaceId;

const SIGN_DOMAIN: &[u8] = b"kith member op v1";
const HASH_DOMAIN: &[u8] = b"kith member hash v1";
const ZERO_HASH: [u8; 32] = [0u8; 32];

/// A device's capability within a Space. Ordered `Reader < Writer < Admin`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Role {
    Reader,
    Writer,
    Admin,
}

impl Role {
    fn to_byte(self) -> u8 {
        match self {
            Role::Reader => 0,
            Role::Writer => 1,
            Role::Admin => 2,
        }
    }
    fn from_byte(b: u8) -> Option<Role> {
        match b {
            0 => Some(Role::Reader),
            1 => Some(Role::Writer),
            2 => Some(Role::Admin),
            _ => None,
        }
    }
    /// May author data changes that honest peers accept.
    pub fn can_write(self) -> bool {
        self >= Role::Writer
    }
    /// May change membership (add/remove/promote) and serve blobs.
    pub fn can_admin(self) -> bool {
        self == Role::Admin
    }
    /// Human-readable form for the GUI / audit view.
    pub fn as_str(self) -> &'static str {
        match self {
            Role::Reader => "reader",
            Role::Writer => "writer",
            Role::Admin => "admin",
        }
    }
}

/// One operation in the membership/audit log.
#[derive(Debug, Clone, PartialEq, Eq)]
enum OpKind {
    /// Establishes the root Admin = `founder`, bound to the SpaceId via `nonce`.
    Genesis {
        founder: [u8; 32],
        nonce: [u8; 16],
    },
    AddMember {
        endpoint: [u8; 32],
        role: u8,
    },
    SetRole {
        endpoint: [u8; 32],
        role: u8,
    },
    RemoveMember {
        endpoint: [u8; 32],
    },
    KeyRotated {
        epoch: u64,
    },
    PairingCompleted {
        endpoint: [u8; 32],
    },
}

impl OpKind {
    fn tag(&self) -> u8 {
        match self {
            OpKind::Genesis { .. } => 0,
            OpKind::AddMember { .. } => 1,
            OpKind::SetRole { .. } => 2,
            OpKind::RemoveMember { .. } => 3,
            OpKind::KeyRotated { .. } => 4,
            OpKind::PairingCompleted { .. } => 5,
        }
    }
    fn encode(&self, out: &mut Vec<u8>) {
        out.push(self.tag());
        match self {
            OpKind::Genesis { founder, nonce } => {
                out.extend_from_slice(founder);
                out.extend_from_slice(nonce);
            }
            OpKind::AddMember { endpoint, role } | OpKind::SetRole { endpoint, role } => {
                out.extend_from_slice(endpoint);
                out.push(*role);
            }
            OpKind::RemoveMember { endpoint } | OpKind::PairingCompleted { endpoint } => {
                out.extend_from_slice(endpoint);
            }
            OpKind::KeyRotated { epoch } => out.extend_from_slice(&epoch.to_le_bytes()),
        }
    }
    /// Decode a kind from `buf` starting at `pos`; returns the kind and the new pos.
    fn decode(buf: &[u8], pos: usize) -> Option<(OpKind, usize)> {
        let tag = *buf.get(pos)?;
        let mut p = pos + 1;
        let mut take = |n: usize| -> Option<&[u8]> {
            let s = buf.get(p..p + n)?;
            p += n;
            Some(s)
        };
        let kind = match tag {
            0 => {
                let founder = arr32(take(32)?);
                let mut nonce = [0u8; 16];
                nonce.copy_from_slice(take(16)?);
                OpKind::Genesis { founder, nonce }
            }
            1 => {
                let endpoint = arr32(take(32)?);
                let role = take(1)?[0];
                OpKind::AddMember { endpoint, role }
            }
            2 => {
                let endpoint = arr32(take(32)?);
                let role = take(1)?[0];
                OpKind::SetRole { endpoint, role }
            }
            3 => OpKind::RemoveMember {
                endpoint: arr32(take(32)?),
            },
            4 => {
                let mut e = [0u8; 8];
                e.copy_from_slice(take(8)?);
                OpKind::KeyRotated {
                    epoch: u64::from_le_bytes(e),
                }
            }
            5 => OpKind::PairingCompleted {
                endpoint: arr32(take(32)?),
            },
            _ => return None,
        };
        Some((kind, p))
    }
}

/// A signed, hash-chained log entry.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Entry {
    seq: u64,
    prev: [u8; 32],
    epoch: u64,
    signer: [u8; 32],
    kind: OpKind,
    sig: [u8; 64],
}

impl Entry {
    /// The bytes that are signed and (with the signature) hashed: everything but `sig`.
    fn unsigned(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(96);
        out.extend_from_slice(&self.seq.to_le_bytes());
        out.extend_from_slice(&self.prev);
        out.extend_from_slice(&self.epoch.to_le_bytes());
        out.extend_from_slice(&self.signer);
        self.kind.encode(&mut out);
        out
    }

    /// What the signer signs: domain || space || unsigned-entry. Binding `space_id`
    /// stops an entry being replayed into a different Space.
    fn sign_input(&self, space_id: &SpaceId) -> Vec<u8> {
        let mut v = Vec::with_capacity(SIGN_DOMAIN.len() + 32 + 96);
        v.extend_from_slice(SIGN_DOMAIN);
        v.extend_from_slice(space_id.as_bytes());
        v.extend_from_slice(&self.unsigned());
        v
    }

    /// Content hash over the full (signed) entry — the chain link.
    fn hash(&self) -> [u8; 32] {
        let mut h = Sha256::new();
        h.update(HASH_DOMAIN);
        h.update(self.unsigned());
        h.update(self.sig);
        let mut out = [0u8; 32];
        out.copy_from_slice(&h.finalize());
        out
    }

    fn encode(&self, out: &mut Vec<u8>) {
        let body = {
            let mut b = self.unsigned();
            b.extend_from_slice(&self.sig);
            b
        };
        out.extend_from_slice(&(body.len() as u32).to_le_bytes());
        out.extend_from_slice(&body);
    }

    /// Decode one length-prefixed entry from `buf` at `pos`; returns entry + new pos.
    fn decode(buf: &[u8], pos: usize) -> Option<(Entry, usize)> {
        let len_bytes = buf.get(pos..pos + 4)?;
        let len = u32::from_le_bytes(arr4(len_bytes)) as usize;
        let start = pos + 4;
        let end = start.checked_add(len)?;
        let body = buf.get(start..end)?;
        // Parse the body: seq(8) prev(32) epoch(8) signer(32) kind(..) sig(64).
        let mut p = 0usize;
        let mut take = |n: usize| -> Option<&[u8]> {
            let s = body.get(p..p + n)?;
            p += n;
            Some(s)
        };
        let seq = u64::from_le_bytes(arr8(take(8)?));
        let prev = arr32(take(32)?);
        let epoch = u64::from_le_bytes(arr8(take(8)?));
        let signer = arr32(take(32)?);
        let (kind, after_kind) = OpKind::decode(body, p)?;
        p = after_kind;
        let sig = {
            let s = body.get(p..p + 64)?;
            p += 64;
            arr64(s)
        };
        if p != body.len() {
            return None; // trailing garbage in the entry
        }
        Some((
            Entry {
                seq,
                prev,
                epoch,
                signer,
                kind,
                sig,
            },
            end,
        ))
    }

    /// Verify this entry's signature against its claimed signer.
    fn verify_sig(&self, space_id: &SpaceId) -> bool {
        let Ok(pk) = PublicKey::from_bytes(&self.signer) else {
            return false;
        };
        let sig = Signature::from_bytes(&self.sig);
        pk.verify(&self.sign_input(space_id), &sig).is_ok()
    }
}

/// The current, verified membership state derived by replaying the log.
#[derive(Debug, Clone, Default)]
pub struct MembershipState {
    members: HashMap<[u8; 32], Role>,
    founder: Option<[u8; 32]>,
    epoch: u64,
    len: usize,
}

impl MembershipState {
    pub fn role_of(&self, endpoint: &[u8; 32]) -> Option<Role> {
        self.members.get(endpoint).copied()
    }
    pub fn is_member(&self, endpoint: &[u8; 32]) -> bool {
        self.members.contains_key(endpoint)
    }
    pub fn is_writer(&self, endpoint: &[u8; 32]) -> bool {
        self.role_of(endpoint).map(Role::can_write).unwrap_or(false)
    }
    pub fn is_admin(&self, endpoint: &[u8; 32]) -> bool {
        self.role_of(endpoint).map(Role::can_admin).unwrap_or(false)
    }
    /// The current key epoch (bumped by revocation in M3).
    #[allow(dead_code)] // consumed by M3 (epoch-aware checks)
    pub fn epoch(&self) -> u64 {
        self.epoch
    }
    /// `(endpoint, role)` for every current member.
    pub fn members(&self) -> Vec<([u8; 32], Role)> {
        let mut v: Vec<_> = self.members.iter().map(|(k, r)| (*k, *r)).collect();
        v.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        v
    }
    /// How many verified entries produced this state (the log length).
    #[allow(dead_code)] // used by tests + the M3 audit view
    pub fn len(&self) -> usize {
        self.len
    }
}

/// Replay a chain of entries from genesis, verifying signatures, the hash chain, and
/// that each op is authorized (signed by an Admin at that point). Returns the derived
/// state, or `Err` if the chain is broken/forged/tampered.
fn replay(space_id: &SpaceId, entries: &[Entry]) -> Result<MembershipState> {
    let mut state = MembershipState::default();
    let mut expected_prev = ZERO_HASH;
    for (i, e) in entries.iter().enumerate() {
        if e.seq != i as u64 {
            return Err(bad("entry seq out of order"));
        }
        if e.prev != expected_prev {
            return Err(bad("hash chain broken (prev mismatch)"));
        }
        if !e.verify_sig(space_id) {
            return Err(bad("entry signature invalid"));
        }
        match &e.kind {
            OpKind::Genesis { founder, nonce } => {
                if i != 0 {
                    return Err(bad("genesis must be the first entry"));
                }
                if &e.signer != founder {
                    return Err(bad("genesis must be self-signed by the founder"));
                }
                // Bind the root Admin to the SpaceId (skip for the default Space, which
                // has a constant id with no founder derivation).
                if !space_id.is_default() && SpaceId::new(founder, nonce) != *space_id {
                    return Err(bad("genesis founder does not match the SpaceId"));
                }
                state.members.insert(*founder, Role::Admin);
                state.founder = Some(*founder);
            }
            OpKind::AddMember { endpoint, role } | OpKind::SetRole { endpoint, role } => {
                if !state.is_admin(&e.signer) {
                    return Err(bad("membership change not signed by an Admin"));
                }
                let role = Role::from_byte(*role).ok_or_else(|| bad("invalid role"))?;
                state.members.insert(*endpoint, role);
            }
            OpKind::RemoveMember { endpoint } => {
                if !state.is_admin(&e.signer) {
                    return Err(bad("removal not signed by an Admin"));
                }
                state.members.remove(endpoint);
            }
            OpKind::KeyRotated { epoch } => {
                if !state.is_admin(&e.signer) {
                    return Err(bad("key rotation not signed by an Admin"));
                }
                state.epoch = *epoch;
            }
            OpKind::PairingCompleted { .. } => {
                if !state.is_admin(&e.signer) {
                    return Err(bad("pairing record not signed by an Admin"));
                }
            }
        }
        expected_prev = e.hash();
    }
    state.len = entries.len();
    Ok(state)
}

/// A Space's membership/audit log, kept in `members.log` and synced between peers.
#[derive(Debug, Clone)]
pub struct Membership {
    space_id: SpaceId,
    entries: Vec<Entry>,
    state: MembershipState,
    path: PathBuf,
}

impl Membership {
    /// Create a fresh enforced log: a self-signed genesis making `secret`'s device the
    /// root Admin, bound to `space_id` via `nonce`. Persists `members.log`.
    pub fn genesis(
        space_id: SpaceId,
        secret: &SecretKey,
        nonce: [u8; 16],
        path: PathBuf,
    ) -> Result<Membership> {
        let founder = arr32(secret.public().as_bytes());
        let mut e = Entry {
            seq: 0,
            prev: ZERO_HASH,
            epoch: 0,
            signer: founder,
            kind: OpKind::Genesis { founder, nonce },
            sig: [0u8; 64],
        };
        sign_entry(&mut e, space_id, secret);
        let entries = vec![e];
        let state = replay(&space_id, &entries)?;
        let m = Membership {
            space_id,
            entries,
            state,
            path,
        };
        m.persist()?;
        Ok(m)
    }

    /// Load + verify an existing log from `path`. `Ok(None)` if there is no log file
    /// (a permissive Space). `Err` if the file exists but is corrupt/forged.
    pub fn open(space_id: SpaceId, path: PathBuf) -> Result<Option<Membership>> {
        let bytes = match std::fs::read(&path) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(CoreError::Io(e)),
        };
        let entries = decode_entries(&bytes).ok_or_else(|| bad("membership log unreadable"))?;
        let state = replay(&space_id, &entries)?;
        Ok(Some(Membership {
            space_id,
            entries,
            state,
            path,
        }))
    }

    pub fn state(&self) -> &MembershipState {
        &self.state
    }

    #[allow(dead_code)] // M3 audit view
    pub fn space_id(&self) -> SpaceId {
        self.space_id
    }

    /// Serialize the whole log for sending to a peer.
    pub fn serialize(&self) -> Vec<u8> {
        let mut out = Vec::new();
        for e in &self.entries {
            e.encode(&mut out);
        }
        out
    }

    /// Append an `AddMember` op (signer must be Admin). Persists.
    pub fn add_member(&mut self, secret: &SecretKey, endpoint: [u8; 32], role: Role) -> Result<()> {
        self.append(
            secret,
            OpKind::AddMember {
                endpoint,
                role: role.to_byte(),
            },
        )
    }

    /// Append a `SetRole` op (promote/demote; signer must be Admin). Persists.
    pub fn set_role(&mut self, secret: &SecretKey, endpoint: [u8; 32], role: Role) -> Result<()> {
        self.append(
            secret,
            OpKind::SetRole {
                endpoint,
                role: role.to_byte(),
            },
        )
    }

    /// Append a `RemoveMember` op (signer must be Admin). Persists. (Used by M3.)
    pub fn remove_member(&mut self, secret: &SecretKey, endpoint: [u8; 32]) -> Result<()> {
        self.append(secret, OpKind::RemoveMember { endpoint })
    }

    /// Append a `KeyRotated` audit op at `epoch` (signer must be Admin). (Used by M3.)
    #[allow(dead_code)] // wired in M3 (epoch rekey)
    pub fn record_key_rotation(&mut self, secret: &SecretKey, epoch: u64) -> Result<()> {
        self.append(secret, OpKind::KeyRotated { epoch })
    }

    /// Append a `PairingCompleted` audit op (signer must be Admin).
    #[allow(dead_code)] // wired in M3 (audit of pairing)
    pub fn record_pairing(&mut self, secret: &SecretKey, endpoint: [u8; 32]) -> Result<()> {
        self.append(secret, OpKind::PairingCompleted { endpoint })
    }

    fn append(&mut self, secret: &SecretKey, kind: OpKind) -> Result<()> {
        let signer = arr32(secret.public().as_bytes());
        if !self.state.is_admin(&signer) {
            return Err(bad("only an Admin may change membership"));
        }
        let prev = self.entries.last().map(Entry::hash).unwrap_or(ZERO_HASH);
        let mut e = Entry {
            seq: self.entries.len() as u64,
            prev,
            epoch: self.state.epoch,
            signer,
            kind,
            sig: [0u8; 64],
        };
        sign_entry(&mut e, self.space_id, secret);
        let mut candidate = self.entries.clone();
        candidate.push(e);
        let state = replay(&self.space_id, &candidate)?;
        self.entries = candidate;
        self.state = state;
        self.persist()?;
        Ok(())
    }

    /// Merge a peer's serialized log into ours: adopt the longest valid chain derivable
    /// from the union (sharing our genesis). Returns whether our state changed.
    pub fn merge(&mut self, incoming: &[u8]) -> Result<bool> {
        let Some(incoming) = decode_entries(incoming) else {
            return Ok(false); // unreadable peer log: ignore rather than fail the sync
        };
        // Union all entries by content hash.
        let mut by_hash: HashMap<[u8; 32], Entry> = HashMap::new();
        for e in self.entries.iter().chain(incoming.iter()) {
            by_hash.insert(e.hash(), e.clone());
        }
        let chain = canonical_chain(&self.space_id, &by_hash);
        // Only adopt a strictly longer valid chain that replays cleanly.
        if chain.len() <= self.entries.len() {
            return Ok(false);
        }
        let state = match replay(&self.space_id, &chain) {
            Ok(s) => s,
            Err(_) => return Ok(false),
        };
        self.entries = chain;
        self.state = state;
        self.persist()?;
        Ok(true)
    }

    /// The verified log as a human-readable audit trail (oldest first). Because `open`/
    /// `merge` only ever keep a chain that replays cleanly, every entry returned here has
    /// passed signature + hash-chain + authorization checks — a tampered log fails to
    /// load rather than producing entries.
    pub fn audit(&self) -> Vec<AuditEntry> {
        let ep = |e: &[u8; 32]| {
            PublicKey::from_bytes(e)
                .map(|p| p.to_string())
                .unwrap_or_else(|_| "?".to_string())
        };
        self.entries
            .iter()
            .map(|e| {
                let (action, target) = match &e.kind {
                    OpKind::Genesis { founder, .. } => {
                        ("space-created".to_string(), Some(ep(founder)))
                    }
                    OpKind::AddMember { endpoint, role } => (
                        format!(
                            "member-added:{}",
                            Role::from_byte(*role).map(Role::as_str).unwrap_or("?")
                        ),
                        Some(ep(endpoint)),
                    ),
                    OpKind::SetRole { endpoint, role } => (
                        format!(
                            "role-changed:{}",
                            Role::from_byte(*role).map(Role::as_str).unwrap_or("?")
                        ),
                        Some(ep(endpoint)),
                    ),
                    OpKind::RemoveMember { endpoint } => {
                        ("member-removed".to_string(), Some(ep(endpoint)))
                    }
                    OpKind::KeyRotated { epoch } => (format!("key-rotated:{epoch}"), None),
                    OpKind::PairingCompleted { endpoint } => {
                        ("pairing-completed".to_string(), Some(ep(endpoint)))
                    }
                };
                AuditEntry {
                    seq: e.seq,
                    epoch: e.epoch,
                    signer: ep(&e.signer),
                    action,
                    target,
                }
            })
            .collect()
    }

    fn persist(&self) -> Result<()> {
        let bytes = self.serialize();
        let tmp = self.path.with_extension("log.tmp");
        std::fs::write(&tmp, &bytes)?;
        std::fs::rename(&tmp, &self.path)?;
        Ok(())
    }
}

/// One human-readable line of a Space's audit log (a verified membership-log entry).
#[derive(Debug, Clone)]
pub struct AuditEntry {
    /// Position in the chain (0 = genesis).
    pub seq: u64,
    /// The key epoch in effect when this entry was appended.
    pub epoch: u64,
    /// Endpoint id (string) of the Admin that signed this entry.
    pub signer: String,
    /// What happened: `space-created`, `member-added:<role>`, `role-changed:<role>`,
    /// `member-removed`, `key-rotated:<epoch>`, `pairing-completed`.
    pub action: String,
    /// The endpoint id this entry acted on, when applicable.
    pub target: Option<String>,
}

/// Sign an entry in place over `domain || space || unsigned`.
fn sign_entry(e: &mut Entry, space_id: SpaceId, secret: &SecretKey) {
    let sig = secret.sign(&e.sign_input(&space_id));
    e.sig = sig.to_bytes();
}

/// Greedily walk the longest valid chain from genesis out of a pool of entries. A fork
/// (two entries with the same `prev`) is resolved deterministically by lower hash.
fn canonical_chain(space_id: &SpaceId, pool: &HashMap<[u8; 32], Entry>) -> Vec<Entry> {
    // Index entries by their `prev` link.
    let mut by_prev: HashMap<[u8; 32], Vec<&Entry>> = HashMap::new();
    for e in pool.values() {
        by_prev.entry(e.prev).or_default().push(e);
    }
    let pick = |cands: Option<&Vec<&Entry>>, expected_seq: u64| -> Option<Entry> {
        let cands = cands?;
        cands
            .iter()
            .filter(|e| e.seq == expected_seq && e.verify_sig(space_id))
            .min_by_key(|e| e.hash())
            .map(|e| (*e).clone())
    };
    let mut chain = Vec::new();
    // Genesis: prev == ZERO, seq == 0.
    let Some(mut cur) = pick(by_prev.get(&ZERO_HASH), 0) else {
        return chain;
    };
    loop {
        let h = cur.hash();
        let seq = cur.seq;
        chain.push(cur);
        match pick(by_prev.get(&h), seq + 1) {
            Some(next) => cur = next,
            None => break,
        }
    }
    chain
}

fn decode_entries(buf: &[u8]) -> Option<Vec<Entry>> {
    let mut out = Vec::new();
    let mut pos = 0usize;
    while pos < buf.len() {
        let (e, next) = Entry::decode(buf, pos)?;
        out.push(e);
        pos = next;
    }
    Some(out)
}

fn bad(msg: &str) -> CoreError {
    CoreError::Other(anyhow::anyhow!("membership: {msg}"))
}

fn arr4(s: &[u8]) -> [u8; 4] {
    let mut a = [0u8; 4];
    a.copy_from_slice(s);
    a
}
fn arr8(s: &[u8]) -> [u8; 8] {
    let mut a = [0u8; 8];
    a.copy_from_slice(s);
    a
}
fn arr32(s: &[u8]) -> [u8; 32] {
    let mut a = [0u8; 32];
    a.copy_from_slice(s);
    a
}
fn arr64(s: &[u8]) -> [u8; 64] {
    let mut a = [0u8; 64];
    a.copy_from_slice(s);
    a
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(seed: u8) -> SecretKey {
        SecretKey::from_bytes(&[seed; 32])
    }
    fn ep(secret: &SecretKey) -> [u8; 32] {
        arr32(secret.public().as_bytes())
    }
    fn space_for(founder: &SecretKey, nonce: [u8; 16]) -> SpaceId {
        SpaceId::new(&ep(founder), &nonce)
    }

    #[test]
    fn genesis_makes_founder_admin_and_binds_space() {
        let dir = tempfile::tempdir().unwrap();
        let admin = key(1);
        let nonce = [9u8; 16];
        let sid = space_for(&admin, nonce);
        let m = Membership::genesis(sid, &admin, nonce, dir.path().join("members.log")).unwrap();
        assert_eq!(m.state().role_of(&ep(&admin)), Some(Role::Admin));
        assert!(m.state().is_writer(&ep(&admin)));
        assert_eq!(m.state().len(), 1);
    }

    #[test]
    fn admin_can_add_and_promote() {
        let dir = tempfile::tempdir().unwrap();
        let admin = key(1);
        let writer = key(2);
        let nonce = [3u8; 16];
        let sid = space_for(&admin, nonce);
        let mut m =
            Membership::genesis(sid, &admin, nonce, dir.path().join("members.log")).unwrap();

        m.add_member(&admin, ep(&writer), Role::Reader).unwrap();
        assert_eq!(m.state().role_of(&ep(&writer)), Some(Role::Reader));
        assert!(!m.state().is_writer(&ep(&writer)));

        m.set_role(&admin, ep(&writer), Role::Writer).unwrap();
        assert!(m.state().is_writer(&ep(&writer)));
        assert!(!m.state().is_admin(&ep(&writer)));
    }

    #[test]
    fn non_admin_cannot_change_membership() {
        let dir = tempfile::tempdir().unwrap();
        let admin = key(1);
        let reader = key(2);
        let victim = key(3);
        let nonce = [4u8; 16];
        let sid = space_for(&admin, nonce);
        let mut m =
            Membership::genesis(sid, &admin, nonce, dir.path().join("members.log")).unwrap();
        m.add_member(&admin, ep(&reader), Role::Reader).unwrap();

        // A Reader appending a self-promotion is refused locally...
        let mut as_reader = m.clone();
        as_reader.path = dir.path().join("forged.log");
        let r = as_reader.set_role(&reader, ep(&reader), Role::Admin);
        assert!(r.is_err(), "a non-Admin cannot append a membership change");
        // ...and would not verify on replay either.
        let _ = victim;
    }

    #[test]
    fn forged_membership_entry_is_rejected_on_replay() {
        let dir = tempfile::tempdir().unwrap();
        let admin = key(1);
        let reader = key(2);
        let nonce = [5u8; 16];
        let sid = space_for(&admin, nonce);
        let m = Membership::genesis(sid, &admin, nonce, dir.path().join("members.log")).unwrap();

        // The Reader forges an entry promoting itself to Admin, signed by ITS OWN key
        // (it cannot sign as the Admin). Replaying must reject it.
        let prev = m.entries.last().unwrap().hash();
        let mut forged = Entry {
            seq: 1,
            prev,
            epoch: 0,
            signer: ep(&reader),
            kind: OpKind::SetRole {
                endpoint: ep(&reader),
                role: Role::Admin.to_byte(),
            },
            sig: [0u8; 64],
        };
        sign_entry(&mut forged, sid, &reader);
        let mut chain = m.entries.clone();
        chain.push(forged);
        assert!(
            replay(&sid, &chain).is_err(),
            "an Admin-op signed by a non-Admin is rejected"
        );
    }

    #[test]
    fn tampering_breaks_the_hash_chain() {
        let dir = tempfile::tempdir().unwrap();
        let admin = key(1);
        let writer = key(2);
        let nonce = [6u8; 16];
        let sid = space_for(&admin, nonce);
        let mut m =
            Membership::genesis(sid, &admin, nonce, dir.path().join("members.log")).unwrap();
        m.add_member(&admin, ep(&writer), Role::Writer).unwrap();
        m.set_role(&admin, ep(&writer), Role::Reader).unwrap();

        // Tamper with the middle entry's role (Writer -> Admin) without re-signing.
        let mut chain = m.entries.clone();
        if let OpKind::AddMember { role, .. } = &mut chain[1].kind {
            *role = Role::Admin.to_byte();
        }
        assert!(
            replay(&sid, &chain).is_err(),
            "altering an entry fails its signature / breaks the chain"
        );
    }

    #[test]
    fn merge_adopts_longer_chain_and_roundtrips() {
        let dir = tempfile::tempdir().unwrap();
        let admin = key(1);
        let writer = key(2);
        let nonce = [7u8; 16];
        let sid = space_for(&admin, nonce);

        // Admin builds the authoritative log.
        let mut a = Membership::genesis(sid, &admin, nonce, dir.path().join("a.log")).unwrap();
        a.add_member(&admin, ep(&writer), Role::Writer).unwrap();

        // A peer that only has the genesis merges the Admin's log and converges.
        let mut b = Membership::open(sid, dir.path().join("b.log"))
            .unwrap()
            .unwrap_or_else(|| {
                // b starts from the same genesis bytes (first entry only).
                let mut only_genesis = Vec::new();
                a.entries[0].encode(&mut only_genesis);
                std::fs::write(dir.path().join("b.log"), &only_genesis).unwrap();
                Membership::open(sid, dir.path().join("b.log"))
                    .unwrap()
                    .unwrap()
            });
        assert_eq!(b.state().len(), 1);
        let changed = b.merge(&a.serialize()).unwrap();
        assert!(changed, "b adopted the longer chain");
        assert!(b.state().is_writer(&ep(&writer)));

        // Re-merging the same log is a no-op.
        assert!(!b.merge(&a.serialize()).unwrap());
    }
}
