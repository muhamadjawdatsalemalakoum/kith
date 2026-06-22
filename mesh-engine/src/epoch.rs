//! Epoch keys for revocation: a per-Space, monotonically-increasing **epoch key** that
//! rotates when a member is removed.
//!
//! ## What an epoch key is (and isn't)
//! Enforced Spaces keep their stable group key (for the HMAC handshake) and root
//! membership in EndpointIds (the [`crate::membership`] log + the connect gate), which is
//! what already excludes a removed device from honest peers. On top of that, revocation
//! rotates a separate **epoch key** that:
//! - re-keys the Space's at-rest encryption (so a post-rotation disk needs the new key), and
//! - is the "new key" remaining members converge on.
//!
//! Each epoch key is **signed by an Admin** (`sign over space || epoch || key`), so a peer
//! adopts a pushed epoch key only if a current Admin minted it — a non-Admin (or the
//! removed device) cannot forge one. Keys are distributed peer-to-peer over the
//! authenticated, membership-gated verified-sync channel; the removed device fails the
//! gate and never receives the new epoch key.
//!
//! **Honest guarantee:** post-removal confidentiality for *future* data, in the
//! honest-peer model. This is **not** a retroactive wipe and **not** forward secrecy for
//! data a removed device already synced (see `SECURITY.md`).

use std::collections::HashMap;
use std::path::PathBuf;

use hmac::{Hmac, Mac};
use iroh::{PublicKey, SecretKey, Signature};
use sha2::Sha256;

use crate::space::SpaceId;

type HmacSha256 = Hmac<Sha256>;

const EPOCH_SIG_DOMAIN: &[u8] = b"kith epoch key v1";
const ATREST_KDF_DOMAIN: &[u8] = b"kith epoch atrest v1";

/// One epoch's key plus the Admin attestation that minted it.
#[derive(Clone, Copy)]
pub(crate) struct EpochKey {
    pub key: [u8; 32],
    pub admin: [u8; 32],
    pub sig: [u8; 64],
}

/// Per-Space store of epoch keys, persisted to `epochs.bin` as
/// `[epoch(8) || key(32) || admin(32) || sig(64)]*`.
pub(crate) struct EpochStore {
    path: PathBuf,
    map: HashMap<u64, EpochKey>,
}

impl EpochStore {
    pub(crate) fn load(path: PathBuf) -> EpochStore {
        let mut map = HashMap::new();
        if let Ok(bytes) = std::fs::read(&path) {
            for chunk in bytes.chunks_exact(8 + 32 + 32 + 64) {
                let epoch = u64::from_le_bytes(arr8(&chunk[0..8]));
                let key = arr32(&chunk[8..40]);
                let admin = arr32(&chunk[40..72]);
                let sig = arr64(&chunk[72..136]);
                map.insert(epoch, EpochKey { key, admin, sig });
            }
        }
        EpochStore { path, map }
    }

    pub(crate) fn get(&self, epoch: u64) -> Option<EpochKey> {
        self.map.get(&epoch).copied()
    }

    pub(crate) fn has(&self, epoch: u64) -> bool {
        self.map.contains_key(&epoch)
    }

    pub(crate) fn max_epoch(&self) -> Option<u64> {
        self.map.keys().copied().max()
    }

    /// Insert an epoch key (already verified) and persist.
    pub(crate) fn put(&mut self, epoch: u64, ek: EpochKey) {
        self.map.insert(epoch, ek);
        self.persist();
    }

    /// The on-disk / on-wire encoding of all epoch keys (to seed a joiner or push to a
    /// peer). Same layout as `epochs.bin`.
    pub(crate) fn serialize(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.map.len() * 136);
        let mut epochs: Vec<u64> = self.map.keys().copied().collect();
        epochs.sort_unstable();
        for e in epochs {
            let ek = &self.map[&e];
            out.extend_from_slice(&e.to_le_bytes());
            out.extend_from_slice(&ek.key);
            out.extend_from_slice(&ek.admin);
            out.extend_from_slice(&ek.sig);
        }
        out
    }

    fn persist(&self) {
        let tmp = self.path.with_extension("bin.tmp");
        if std::fs::write(&tmp, self.serialize()).is_ok() {
            let _ = std::fs::rename(&tmp, &self.path);
        }
    }
}

/// Decode `[epoch || key || admin || sig]*` (a peer's epoch keys) without a backing
/// file — the keys still need per-entry signature + Admin verification before adoption.
pub(crate) fn parse_keys(bytes: &[u8]) -> Vec<(u64, EpochKey)> {
    let mut out = Vec::new();
    for chunk in bytes.chunks_exact(8 + 32 + 32 + 64) {
        let epoch = u64::from_le_bytes(arr8(&chunk[0..8]));
        let key = arr32(&chunk[8..40]);
        let admin = arr32(&chunk[40..72]);
        let sig = arr64(&chunk[72..136]);
        out.push((epoch, EpochKey { key, admin, sig }));
    }
    out
}

/// What an Admin signs to attest an epoch key: domain || space || epoch || key.
fn sign_input(space_id: &SpaceId, epoch: u64, key: &[u8; 32]) -> Vec<u8> {
    let mut v = Vec::with_capacity(EPOCH_SIG_DOMAIN.len() + 32 + 8 + 32);
    v.extend_from_slice(EPOCH_SIG_DOMAIN);
    v.extend_from_slice(space_id.as_bytes());
    v.extend_from_slice(&epoch.to_le_bytes());
    v.extend_from_slice(key);
    v
}

/// Mint a signed epoch key as the Admin device `secret`.
pub(crate) fn mint(space_id: &SpaceId, epoch: u64, key: [u8; 32], secret: &SecretKey) -> EpochKey {
    let mut admin = [0u8; 32];
    admin.copy_from_slice(secret.public().as_bytes());
    let sig = secret.sign(&sign_input(space_id, epoch, &key)).to_bytes();
    EpochKey { key, admin, sig }
}

/// Verify an epoch key was minted by `admin` (the caller separately checks `admin` is an
/// Admin in the membership at this epoch).
pub(crate) fn verify(space_id: &SpaceId, epoch: u64, ek: &EpochKey) -> bool {
    let Ok(pk) = PublicKey::from_bytes(&ek.admin) else {
        return false;
    };
    pk.verify(
        &sign_input(space_id, epoch, &ek.key),
        &Signature::from_bytes(&ek.sig),
    )
    .is_ok()
}

/// Derive the at-rest encryption key for an epoch from its epoch key (HMAC as a PRF).
/// Tying at-rest to the epoch key is what re-keys on-disk data on rotation.
pub(crate) fn derive_atrest(epoch_key: &[u8; 32], space_id: &SpaceId, epoch: u64) -> [u8; 32] {
    let mut mac = <HmacSha256 as Mac>::new_from_slice(epoch_key).expect("hmac accepts any key");
    mac.update(ATREST_KDF_DOMAIN);
    mac.update(space_id.as_bytes());
    mac.update(&epoch.to_le_bytes());
    let tag = mac.finalize().into_bytes();
    let mut out = [0u8; 32];
    out.copy_from_slice(&tag);
    out
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
