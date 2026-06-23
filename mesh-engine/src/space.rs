//! Spaces: N independent encrypted networks multiplexed over ONE device endpoint.
//!
//! A device runs many [`SpaceState`]s concurrently. Each Space is its own private
//! network — its own group key, at-rest key, Automerge replica, blob store, peer set,
//! and data subdir — but they all share the one [`iroh::Endpoint`] and the one device
//! identity (per iroh guidance: N endpoints would be N DHT publishers + N relay
//! registrations). Routing is by [`SpaceId`], carried first on every inbound stream so
//! the accept handlers ([`crate::sync`], [`crate::blobs`], [`crate::pair`]) can look up
//! the right Space before any data flows.
//!
//! ## Why the default Space has a constant id
//! Every device has a **default Space** with a well-known, constant [`SpaceId`]
//! ([`SpaceId::default_space`]). Two devices configured with the same group key (an
//! explicit-key test, the relay test, or a freshly-paired pair adopting the host's
//! key) therefore route to the *same* default Space and converge — exactly the
//! single-group behaviour the engine had before Spaces. The id is **independent of the
//! group key**, so it is stable across the epoch rotations that revocation performs
//! (the key changes; the Space's identity does not). New Spaces created with
//! [`SpaceId::new`] get a random, public id derived from the founding device key.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::SystemTime;

use automerge::{ActorId, Change, ChangeHash};
use iroh::endpoint::{Connection, RecvStream, SendStream};
use iroh::protocol::AcceptError;
use iroh::{EndpointAddr, EndpointId, PublicKey, SecretKey, Signature};
use iroh_blobs::api::TempTag;
use iroh_blobs::store::fs::FsStore;
use iroh_blobs::{BlobsProtocol, Hash};
use sha2::{Digest, Sha256};
use tokio::sync::{mpsc, Mutex};

use crate::config::KeyStore;
use crate::doc::SharedDoc;
use crate::epoch::{self, EpochStore};
use crate::error::{CoreError, Result};
use crate::membership::{Membership, Role};
use crate::sync::MeshSync;
use crate::{atrest, blobs, doc, keys};

/// Domain label binding a per-change signature to its Space (so a signature can't be
/// replayed onto a change of the same hash in another Space).
const CHANGE_SIG_DOMAIN: &[u8] = b"kith change sig v1";
/// Hard cap on a verified-sync frame (membership log / heads / change batch).
const MAX_VERIFIED_FRAME: usize = 32 * 1024 * 1024;

/// Stable, public identifier for a Space. 32 bytes; safe to put on the wire (it is a
/// hash, never the secret group key). Displayed/parsed as lowercase hex.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct SpaceId([u8; 32]);

impl SpaceId {
    /// The well-known id every device's **default Space** shares. Domain-separated
    /// constant — not secret, and deliberately independent of any group key so it
    /// survives epoch rotation. See the module docs for why this keeps single-group
    /// installs and tests converging.
    pub fn default_space() -> SpaceId {
        let mut h = Sha256::new();
        h.update(b"kith://space/default/v1");
        let mut out = [0u8; 32];
        out.copy_from_slice(&h.finalize());
        SpaceId(out)
    }

    /// Mint a fresh Space id from the founding device's public key and a random nonce.
    /// Public and safe on the wire; NOT derived from the secret group key, so rotating
    /// the key (revocation) never changes the id.
    pub fn new(founder_pubkey: &[u8], nonce: &[u8]) -> SpaceId {
        let mut h = Sha256::new();
        h.update(b"kith://space/v1");
        h.update(founder_pubkey);
        h.update(nonce);
        let mut out = [0u8; 32];
        out.copy_from_slice(&h.finalize());
        SpaceId(out)
    }

    /// The raw 32 bytes (the on-wire form).
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// Reconstruct from raw bytes read off the wire.
    pub fn from_bytes(b: [u8; 32]) -> SpaceId {
        SpaceId(b)
    }

    /// Lowercase-hex form (the on-disk directory name and display form).
    pub fn to_hex(&self) -> String {
        data_encoding::HEXLOWER.encode(&self.0)
    }

    /// Parse the lowercase-hex form. `None` on a malformed / wrong-length string.
    pub fn parse(s: &str) -> Option<SpaceId> {
        let v = data_encoding::HEXLOWER.decode(s.trim().as_bytes()).ok()?;
        if v.len() != 32 {
            return None;
        }
        let mut o = [0u8; 32];
        o.copy_from_slice(&v);
        Some(SpaceId(o))
    }

    /// Whether this is the default Space id.
    pub fn is_default(&self) -> bool {
        *self == SpaceId::default_space()
    }
}

impl std::fmt::Display for SpaceId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.to_hex())
    }
}

impl std::fmt::Debug for SpaceId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Short prefix is enough to disambiguate in logs without being unwieldy.
        let hex = self.to_hex();
        write!(f, "SpaceId({}…)", &hex[..16.min(hex.len())])
    }
}

/// A Space's public summary, for listing in the GUI / API.
#[derive(Clone, Debug)]
pub struct SpaceInfo {
    pub id: SpaceId,
    pub name: String,
    pub is_default: bool,
}

/// Everything one Space owns. Held behind an `Arc` in the [`SpaceRegistry`] so the
/// accept dispatchers, the background sync loop, and the public API all share one
/// replica / peer set / blob store per Space.
pub(crate) struct SpaceState {
    id: SpaceId,
    name: String,
    doc_path: PathBuf,
    /// Stable group key for the HMAC handshake (defence in depth alongside the
    /// EndpointId membership gate). It does NOT rotate on revocation; the rotating
    /// secret is the epoch key (see `epoch_store`).
    group_key: [u8; 32],
    /// Key encrypting this Space's replica snapshot on disk, plus the epoch it is keyed
    /// under. Permissive Spaces use a per-device `atrest.key` at epoch 0 (no header).
    /// Enforced Spaces derive it from the current epoch key, so a revocation re-keys
    /// on-disk data; the snapshot then carries an 8-byte epoch header.
    atrest_key: StdMutex<[u8; 32]>,
    atrest_epoch: StdMutex<u64>,
    /// Signed, rotating epoch keys (enforced Spaces only) — the revocation substrate.
    epoch_store: Option<StdMutex<EpochStore>>,
    doc: SharedDoc,
    sync: Arc<MeshSync>,
    store: FsStore,
    blobs_protocol: BlobsProtocol,
    /// This device's Ed25519 secret (the global `node.key`), kept for signing changes
    /// and membership ops in enforced Spaces. Reconstructed via [`SpaceState::secret`].
    secret_bytes: [u8; 32],
    /// Present iff this Space enforces roles: a signed, hash-chained membership log
    /// rooting trust in EndpointIds. `None` ⇒ a permissive Space (the default / M1
    /// behaviour: any group-key holder is a full writer).
    membership: Option<StdMutex<Membership>>,
    /// Per-change author signatures keyed by change hash, so a Reader's (or a
    /// non-member's) writes are rejected by honest peers and authorized changes can be
    /// relayed (carry-forward). Only populated for enforced Spaces.
    sigs: StdMutex<SigStore>,
    /// Heads up to which our own local changes have been signed.
    signed_heads: Mutex<Vec<ChangeHash>>,
    /// Temp tags for files we serve in this Space, held so blobs aren't GC'd.
    kept_tags: Mutex<Vec<TempTag>>,
    /// Peers this Space keeps converged with.
    peers: Mutex<Vec<EndpointAddr>>,
    /// Last successful sync time per peer (powers per-device "synced N ago").
    last_sync: StdMutex<HashMap<EndpointId, SystemTime>>,
}

impl SpaceState {
    /// Open (or create) a Space rooted at `dir`. `configured_key`:
    /// - `Some(k)` — use `k` as the group key WITHOUT persisting it (explicit-key tests
    ///   and the relay test; matches the old `CoreConfig.group_key` semantics).
    /// - `None` — load the persisted `group.key`, generating + persisting one if absent.
    ///
    /// `actor` is the global device actor (one identity per device across all Spaces).
    pub(crate) async fn open(
        dir: PathBuf,
        id: SpaceId,
        actor: ActorId,
        secret_bytes: [u8; 32],
        configured_key: Option<[u8; 32]>,
        key_store: KeyStore,
    ) -> Result<SpaceState> {
        std::fs::create_dir_all(&dir)?;
        let name = read_name(&dir).unwrap_or_else(|| default_name(&id));
        let group_key = match configured_key {
            Some(k) => k,
            None => keys::secured_load_or_create(
                &format!("space:{}:group", id.to_hex()),
                &dir.join("group.key"),
                key_store,
            )?,
        };
        let doc_path = dir.join("doc.automerge");

        // Enforced iff a verified membership log is present (created via
        // `create_space_with_roles` / `join_space_with_roles`).
        let membership = Membership::open(id, dir.join("members.log"))?;

        // At-rest: enforced Spaces key the snapshot off the current epoch key (so a
        // revocation re-keys it); permissive Spaces use a per-device file/keychain key.
        let (atrest_key, atrest_epoch, epoch_store, doc) = if membership.is_some() {
            let store = EpochStore::load(dir.join("epochs.bin"));
            let eff = store.max_epoch().unwrap_or(0);
            let at = store
                .get(eff)
                .map(|ek| epoch::derive_atrest(&ek.key, &id, eff))
                .unwrap_or([0u8; 32]);
            let doc = load_enforced(&doc_path, actor, &store, &id);
            (at, eff, Some(StdMutex::new(store)), doc)
        } else {
            let at = keys::secured_load_or_create(
                &format!("space:{}:atrest", id.to_hex()),
                &dir.join("atrest.key"),
                key_store,
            )?;
            let doc = load_or_recover(&doc_path, actor, &at);
            (at, 0u64, None, doc)
        };

        let blobs_store = blobs::open(&dir.join("blobs")).await?;
        let blobs_protocol = BlobsProtocol::new(&blobs_store, None);

        // The merged-doc out-channel is unused today (the receiver is dropped); a sync
        // round still merges into the shared `doc` directly. Kept for a future
        // change-notification consumer.
        let (merged_tx, _merged_rx) = mpsc::channel::<automerge::Automerge>(16);
        let sync = MeshSync::with_shared(doc.clone(), group_key, id, merged_tx);

        let sigs = SigStore::load(dir.join("sigs.bin"));

        Ok(SpaceState {
            id,
            name,
            doc_path,
            group_key,
            atrest_key: StdMutex::new(atrest_key),
            atrest_epoch: StdMutex::new(atrest_epoch),
            epoch_store,
            doc,
            sync,
            store: blobs_store,
            blobs_protocol,
            secret_bytes,
            membership: membership.map(StdMutex::new),
            sigs: StdMutex::new(sigs),
            signed_heads: Mutex::new(Vec::new()),
            kept_tags: Mutex::new(Vec::new()),
            peers: Mutex::new(Vec::new()),
            last_sync: StdMutex::new(HashMap::new()),
        })
    }

    /// This device's Ed25519 secret, reconstructed for signing.
    fn secret(&self) -> SecretKey {
        SecretKey::from_bytes(&self.secret_bytes)
    }

    /// This device's EndpointId bytes (its public key).
    pub(crate) fn my_endpoint(&self) -> [u8; 32] {
        let mut a = [0u8; 32];
        a.copy_from_slice(self.secret().public().as_bytes());
        a
    }

    /// Whether this Space enforces EndpointId membership + roles.
    pub(crate) fn enforced(&self) -> bool {
        self.membership.is_some()
    }

    /// This device's role in this Space (`None` if permissive or not a member).
    pub(crate) fn my_role(&self) -> Option<Role> {
        let me = self.my_endpoint();
        self.membership
            .as_ref()?
            .lock()
            .expect("membership lock")
            .state()
            .role_of(&me)
    }

    /// Whether `endpoint` is a member at the current epoch (permissive ⇒ always true).
    pub(crate) fn is_member(&self, endpoint: &[u8; 32]) -> bool {
        match &self.membership {
            None => true,
            Some(m) => m
                .lock()
                .expect("membership lock")
                .state()
                .is_member(endpoint),
        }
    }

    /// Whether `endpoint` may author accepted writes (permissive ⇒ always true).
    fn is_writer(&self, endpoint: &[u8; 32]) -> bool {
        match &self.membership {
            None => true,
            Some(m) => m
                .lock()
                .expect("membership lock")
                .state()
                .is_writer(endpoint),
        }
    }

    /// `(endpoint-id string, role)` for every current member (enforced Spaces only).
    pub(crate) fn members(&self) -> Vec<(String, Role)> {
        let Some(m) = &self.membership else {
            return Vec::new();
        };
        m.lock()
            .expect("membership lock")
            .state()
            .members()
            .into_iter()
            .filter_map(|(ep, role)| {
                PublicKey::from_bytes(&ep)
                    .ok()
                    .map(|pk| (pk.to_string(), role))
            })
            .collect()
    }

    /// A bundle that seeds a new device with this Space's membership AND its current
    /// epoch keys: `[u32 mem_len][membership log][epoch keys]`. The epoch keys are needed
    /// so the joiner can derive its at-rest key (and verify future rotations). Parsed by
    /// [`SpaceState::install_join_bundle`].
    pub(crate) fn join_bundle(&self) -> Option<Vec<u8>> {
        let m = self.membership.as_ref()?;
        let mem = m.lock().expect("membership lock").serialize();
        let epochs = self
            .epoch_store
            .as_ref()
            .map(|s| s.lock().expect("epoch lock").serialize())
            .unwrap_or_default();
        let mut out = Vec::with_capacity(4 + mem.len() + epochs.len());
        out.extend_from_slice(&(mem.len() as u32).to_le_bytes());
        out.extend_from_slice(&mem);
        out.extend_from_slice(&epochs);
        Some(out)
    }

    /// Split a join bundle into `(membership log bytes, epoch-keys bytes)` for the joiner
    /// to write before opening the Space.
    pub(crate) fn split_join_bundle(bundle: &[u8]) -> Option<(&[u8], &[u8])> {
        if bundle.len() < 4 {
            return None;
        }
        let mut len = [0u8; 4];
        len.copy_from_slice(&bundle[0..4]);
        let mem_len = u32::from_le_bytes(len) as usize;
        let mem = bundle.get(4..4 + mem_len)?;
        let epochs = &bundle[4 + mem_len..];
        Some((mem, epochs))
    }

    /// Append an Admin-signed membership op (add / set-role / remove). Errors if this
    /// Space is permissive or this device is not an Admin. Removal's epoch rekey is driven
    /// by the caller ([`Mesh::remove_member`]) via [`SpaceState::rotate_epoch`].
    pub(crate) fn membership_op(&self, op: MemberOp) -> Result<()> {
        let Some(m) = &self.membership else {
            return Err(CoreError::Other(anyhow::anyhow!(
                "this Space does not enforce roles"
            )));
        };
        let secret = self.secret();
        let mut guard = m.lock().expect("membership lock");
        match op {
            MemberOp::Add(ep, role) => guard.add_member(&secret, ep, role),
            MemberOp::SetRole(ep, role) => guard.set_role(&secret, ep, role),
            MemberOp::Remove(ep) => guard.remove_member(&secret, ep),
        }
    }

    /// The current epoch (from the membership log; 0 for permissive Spaces).
    pub(crate) fn current_epoch(&self) -> u64 {
        self.membership
            .as_ref()
            .map(|m| m.lock().expect("membership lock").state().epoch())
            .unwrap_or(0)
    }

    /// The highest epoch for which we actually hold a key (≤ `current_epoch`).
    pub(crate) fn held_epoch(&self) -> u64 {
        self.epoch_store
            .as_ref()
            .and_then(|s| s.lock().expect("epoch lock").max_epoch())
            .unwrap_or(0)
    }

    /// Mint epoch 0's key at genesis (creator side), signed by this device (the root
    /// Admin), and persist it. Idempotent.
    pub(crate) fn seed_genesis_epoch(&self) -> Result<()> {
        let Some(store) = &self.epoch_store else {
            return Ok(());
        };
        let mut guard = store.lock().expect("epoch lock");
        if guard.has(0) {
            return Ok(());
        }
        let secret = self.secret();
        let key = keys::generate();
        let ek = epoch::mint(&self.id, 0, key, &secret);
        guard.put(0, ek);
        drop(guard);
        // Key the at-rest under epoch 0 now that we have its key.
        let at = epoch::derive_atrest(&key, &self.id, 0);
        *self.atrest_key.lock().expect("atrest lock") = at;
        *self.atrest_epoch.lock().expect("atrest epoch lock") = 0;
        Ok(())
    }

    /// Rotate to a fresh, Admin-signed epoch key (revocation substrate). Bumps the
    /// membership epoch, mints + persists the new key, re-keys the at-rest snapshot, and
    /// re-saves it under the new epoch. Admin-only.
    pub(crate) async fn rotate_epoch(&self) -> Result<()> {
        let (Some(m), Some(store)) = (&self.membership, &self.epoch_store) else {
            return Err(CoreError::Other(anyhow::anyhow!(
                "this Space does not enforce roles"
            )));
        };
        let secret = self.secret();
        let new_epoch = {
            let mut guard = m.lock().expect("membership lock");
            if !guard.state().is_admin(&self.my_endpoint()) {
                return Err(CoreError::Other(anyhow::anyhow!(
                    "only an Admin can rotate the epoch key"
                )));
            }
            let next = guard.state().epoch() + 1;
            guard.record_key_rotation(&secret, next)?;
            next
        };
        let key = keys::generate();
        let ek = epoch::mint(&self.id, new_epoch, key, &secret);
        store.lock().expect("epoch lock").put(new_epoch, ek);
        let at = epoch::derive_atrest(&key, &self.id, new_epoch);
        *self.atrest_key.lock().expect("atrest lock") = at;
        *self.atrest_epoch.lock().expect("atrest epoch lock") = new_epoch;
        self.save().await?;
        Ok(())
    }

    /// Serialize the epoch keys we hold (to push to a peer in a verified exchange).
    fn serialize_epoch_keys(&self) -> Vec<u8> {
        self.epoch_store
            .as_ref()
            .map(|s| s.lock().expect("epoch lock").serialize())
            .unwrap_or_default()
    }

    /// Adopt a peer's pushed epoch keys: take only those we lack whose signature verifies
    /// AND whose signer is an Admin in the (already-merged) membership log. If our held
    /// epoch advances, re-key at-rest and re-save under the new epoch. Returns whether we
    /// advanced.
    async fn adopt_epoch_keys(&self, bytes: &[u8]) -> Result<bool> {
        let (Some(store), Some(m)) = (&self.epoch_store, &self.membership) else {
            return Ok(false);
        };
        let before = self.held_epoch();
        for (epoch_n, ek) in epoch::parse_keys(bytes) {
            {
                let guard = store.lock().expect("epoch lock");
                if guard.has(epoch_n) {
                    continue;
                }
            }
            if !epoch::verify(&self.id, epoch_n, &ek) {
                continue;
            }
            let signer_is_admin = m
                .lock()
                .expect("membership lock")
                .state()
                .is_admin(&ek.admin);
            if !signer_is_admin {
                continue;
            }
            store.lock().expect("epoch lock").put(epoch_n, ek);
        }
        let after = self.held_epoch();
        if after > before {
            if let Some(ek) = store.lock().expect("epoch lock").get(after) {
                let at = epoch::derive_atrest(&ek.key, &self.id, after);
                *self.atrest_key.lock().expect("atrest lock") = at;
                *self.atrest_epoch.lock().expect("atrest epoch lock") = after;
            }
            self.save().await?;
            return Ok(true);
        }
        Ok(false)
    }

    /// The membership/audit log as human-readable entries (enforced Spaces only).
    pub(crate) fn audit_log(&self) -> Vec<crate::membership::AuditEntry> {
        self.membership
            .as_ref()
            .map(|m| m.lock().expect("membership lock").audit())
            .unwrap_or_default()
    }

    pub(crate) fn id(&self) -> SpaceId {
        self.id
    }

    pub(crate) fn name(&self) -> &str {
        &self.name
    }

    pub(crate) fn is_default(&self) -> bool {
        self.id.is_default()
    }

    pub(crate) fn info(&self) -> SpaceInfo {
        SpaceInfo {
            id: self.id,
            name: self.name.clone(),
            is_default: self.is_default(),
        }
    }

    pub(crate) fn doc(&self) -> SharedDoc {
        self.doc.clone()
    }

    pub(crate) fn group_key(&self) -> [u8; 32] {
        self.group_key
    }

    /// The current at-rest key bytes (for an encrypted export, where the key may live in
    /// the OS keychain rather than the Space dir).
    pub(crate) fn atrest_key_bytes(&self) -> [u8; 32] {
        *self.atrest_key.lock().expect("atrest lock")
    }

    /// Re-add exported blob contents into this Space's store (content-addressed → identical
    /// hashes), holding the temp tags so the restored blobs aren't garbage-collected.
    pub(crate) async fn import_blobs(&self, blobs: &[Vec<u8>]) -> Result<()> {
        let tags = blobs::import_all(&self.store, blobs).await?;
        self.kept_tags.lock().await.extend(tags);
        Ok(())
    }

    pub(crate) fn sync(&self) -> &Arc<MeshSync> {
        &self.sync
    }

    pub(crate) fn store(&self) -> &FsStore {
        &self.store
    }

    /// Hand an authenticated blob connection to this Space's stock provider.
    pub(crate) async fn blobs_accept(
        &self,
        conn: Connection,
    ) -> std::result::Result<(), AcceptError> {
        use iroh::protocol::ProtocolHandler;
        self.blobs_protocol.accept(conn).await
    }

    pub(crate) async fn blobs_shutdown(&self) {
        use iroh::protocol::ProtocolHandler;
        self.blobs_protocol.shutdown().await;
    }

    /// A snapshot of this Space's peer set.
    pub(crate) async fn peers(&self) -> Vec<EndpointAddr> {
        self.peers.lock().await.clone()
    }

    /// Add a peer (idempotent by endpoint id). Returns whether it was newly added.
    pub(crate) async fn add_peer(&self, peer: EndpointAddr) -> bool {
        let mut peers = self.peers.lock().await;
        if peers.iter().any(|p| p.id == peer.id) {
            return false;
        }
        peers.push(peer);
        true
    }

    /// Drop a peer by endpoint id. Returns whether it was present.
    pub(crate) async fn remove_peer(&self, id: EndpointId) -> bool {
        let mut peers = self.peers.lock().await;
        let before = peers.len();
        peers.retain(|p| p.id != id);
        peers.len() != before
    }

    pub(crate) fn record_sync(&self, peer: EndpointId) {
        if let Ok(mut m) = self.last_sync.lock() {
            m.insert(peer, SystemTime::now());
        }
    }

    /// Seconds since each peer's last successful sync, keyed by endpoint-id string.
    pub(crate) fn last_sync_ages(&self) -> HashMap<String, u64> {
        let now = SystemTime::now();
        self.last_sync
            .lock()
            .map(|m| {
                m.iter()
                    .map(|(id, t)| {
                        (
                            id.to_string(),
                            now.duration_since(*t).map(|d| d.as_secs()).unwrap_or(0),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Import a file into this Space's content store and keep it served. In an enforced
    /// Space, blob *serving* (`files.share`) is gated to `Writer`/`Admin` — a `Reader`
    /// may fetch but not offer.
    pub(crate) async fn share_file(&self, path: &Path) -> Result<Hash> {
        if self.enforced() && !self.my_role().map(Role::can_write).unwrap_or(false) {
            return Err(CoreError::Other(anyhow::anyhow!(
                "your role in this Space is read-only; you cannot share files"
            )));
        }
        let tt = blobs::add_file(&self.store, path).await?;
        let hash = tt.hash();
        self.kept_tags.lock().await.push(tt);
        Ok(hash)
    }

    /// Persist this Space's replica (compacted, encrypted at rest, crash-safe). Enforced
    /// Spaces prepend an 8-byte epoch header (the epoch the at-rest key is derived from);
    /// permissive Spaces write the bare AEAD blob (unchanged from M1).
    pub(crate) async fn save(&self) -> Result<()> {
        let plain = doc::save(&self.doc).await;
        let at = *self.atrest_key.lock().expect("atrest lock");
        let bytes = if self.epoch_store.is_some() {
            let epoch = *self.atrest_epoch.lock().expect("atrest epoch lock");
            let mut out = epoch.to_le_bytes().to_vec();
            out.extend_from_slice(&atrest::encrypt(&at, &plain));
            out
        } else {
            atrest::encrypt(&at, &plain)
        };
        let path = self.doc_path.clone();
        tokio::task::spawn_blocking(move || -> std::io::Result<()> {
            use std::io::Write;
            let seq = SAVE_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let tmp = path.with_extension(format!("automerge.tmp.{}.{}", std::process::id(), seq));
            {
                let mut f = std::fs::File::create(&tmp)?;
                f.write_all(&bytes)?;
                f.sync_all()?;
            }
            std::fs::rename(&tmp, &path)?;
            if let Some(dir) = path.parent() {
                if let Ok(d) = std::fs::File::open(dir) {
                    let _ = d.sync_all();
                }
            }
            Ok(())
        })
        .await
        .map_err(|e| anyhow::anyhow!("save task failed: {e}"))??;
        Ok(())
    }

    /// Persist + close this Space's content store. The blobs *provider* is shut by the
    /// router via [`crate::blobs::BlobDispatcher`]; here we only flush state and close
    /// the store, so neither is shut twice.
    pub(crate) async fn shutdown(&self) {
        let _ = self.save().await;
        let _ = self.store.shutdown().await;
    }

    // ---------------------------------------------------------- role-enforced sync

    /// Sign any new *local-authored* changes so we can attest them to peers. Idempotent;
    /// a no-op for permissive Spaces.
    async fn sign_local_changes(&self) {
        if !self.enforced() {
            return;
        }
        let me = self.my_endpoint();
        let secret = self.secret();
        let from = self.signed_heads.lock().await.clone();
        let (changes, heads) = {
            let doc = self.doc.lock().await;
            (doc.get_changes(&from), doc.get_heads())
        };
        {
            let mut sigs = self.sigs.lock().expect("sigs lock");
            let mut added = false;
            for c in &changes {
                let h = c.hash().0;
                if c.actor_id().to_bytes() == me.as_slice() && !sigs.has(&h) {
                    let sig = secret.sign(&change_sign_input(&self.id, &h));
                    sigs.put(h, sig.to_bytes());
                    added = true;
                }
            }
            if added {
                sigs.persist();
            }
        }
        *self.signed_heads.lock().await = heads;
    }

    /// Membership gate: refuse a peer whose TLS-authenticated EndpointId is not a member
    /// of this Space — even if it proved the group key. No-op for permissive Spaces.
    /// Applied on both the sync and blob paths so a removed device (which still holds the
    /// stable group key) can neither sync nor fetch content.
    pub(crate) fn gate(&self, conn: &Connection) -> Result<()> {
        if !self.enforced() {
            return Ok(());
        }
        let mut remote = [0u8; 32];
        remote.copy_from_slice(conn.remote_id().as_bytes());
        if !self.is_member(&remote) {
            return Err(CoreError::Sync("peer is not a member of this Space".into()));
        }
        Ok(())
    }

    /// Initiator side of role-enforced sync: name the Space, prove the group key, pass
    /// the membership gate, then run the verified exchange.
    pub(crate) async fn initiate_verified(&self, conn: Connection) -> Result<()> {
        let (mut send, mut recv) = conn.open_bi().await.map_err(sync_err)?;
        send.write_all(self.id.as_bytes()).await.map_err(sync_err)?;
        crate::auth::initiator(&self.group_key, &mut send, &mut recv).await?;
        self.gate(&conn)?;
        self.verified_exchange(&mut send, &mut recv).await?;
        conn.close(0u32.into(), b"bye");
        Ok(())
    }

    /// Responder side: the dispatcher has read the SpaceId; prove the group key, gate on
    /// membership, then run the verified exchange.
    pub(crate) async fn respond_verified(
        &self,
        conn: Connection,
        mut send: SendStream,
        mut recv: RecvStream,
    ) -> Result<()> {
        crate::auth::responder(&self.group_key, &mut send, &mut recv).await?;
        self.gate(&conn)?;
        self.verified_exchange(&mut send, &mut recv).await?;
        conn.closed().await;
        Ok(())
    }

    /// The symmetric verified exchange (both sides run the same steps): swap the signed
    /// membership log, swap heads, then push the changes the peer lacks — each carrying
    /// its author's signature — and apply only the peer's *authorized* changes.
    async fn verified_exchange(&self, send: &mut SendStream, recv: &mut RecvStream) -> Result<()> {
        self.sign_local_changes().await;

        // 1. Membership log: swap + merge so both agree on roles before judging changes.
        let our_mem = self
            .membership
            .as_ref()
            .map(|m| m.lock().expect("membership lock").serialize())
            .unwrap_or_default();
        write_frame(send, &our_mem).await?;
        let their_mem = read_frame(recv).await?;
        self.merge_membership_bytes(&their_mem)?;

        // 1b. Epoch keys: swap + adopt the Admin-signed keys the peer has that we lack, so
        // a remaining member converges onto a post-revocation epoch key. Done after the
        // membership merge so we can check the signer is a current Admin. The removed
        // device never reaches here — it fails the gate before the exchange.
        let our_epochs = self.serialize_epoch_keys();
        write_frame(send, &our_epochs).await?;
        let their_epochs = read_frame(recv).await?;
        self.adopt_epoch_keys(&their_epochs).await?;

        // 2. Heads.
        let our_heads = { self.doc.lock().await.get_heads() };
        write_frame(send, &serialize_heads(&our_heads)).await?;
        let their_heads = parse_heads(&read_frame(recv).await?);

        // 3. Signed change push (both directions, so both converge in one round).
        let push = self.build_change_push(&their_heads).await;
        write_frame(send, &push).await?;
        let their_push = read_frame(recv).await?;
        self.apply_verified_push(&their_push).await?;

        // 4. Apply barrier: each side acks only AFTER applying, so a caller that syncs
        // then immediately reads the peer's replica sees the result. (Automerge's plain
        // message loop self-synchronises; this custom protocol must do it explicitly.)
        write_frame(send, &[1u8]).await?;
        let _ = read_frame(recv).await?;
        Ok(())
    }

    /// Merge a peer's membership log into ours (enforced Spaces only).
    fn merge_membership_bytes(&self, bytes: &[u8]) -> Result<()> {
        if bytes.is_empty() {
            return Ok(());
        }
        if let Some(m) = &self.membership {
            m.lock().expect("membership lock").merge(bytes)?;
        }
        Ok(())
    }

    /// The wire batch of changes the peer lacks, each tagged with its author signature.
    async fn build_change_push(&self, their_heads: &[ChangeHash]) -> Vec<u8> {
        let changes = { self.doc.lock().await.get_changes(their_heads) };
        let sigs = self.sigs.lock().expect("sigs lock");
        let mut out = Vec::new();
        out.extend_from_slice(&(changes.len() as u32).to_le_bytes());
        for c in &changes {
            let raw = c.raw_bytes();
            let sig = sigs.get(&c.hash().0).unwrap_or([0u8; 64]);
            out.extend_from_slice(&(raw.len() as u32).to_le_bytes());
            out.extend_from_slice(raw);
            out.extend_from_slice(&sig);
        }
        out
    }

    /// Verify + apply a peer's change push: keep only changes whose author signature is
    /// valid AND whose author is a `Writer`/`Admin` at the current epoch; drop the rest.
    /// This is the cryptographic read-only enforcement — a `Reader` (or a leaked-key
    /// non-member) cannot get a write accepted by an honest peer.
    async fn apply_verified_push(&self, buf: &[u8]) -> Result<()> {
        let Some(head) = buf.get(0..4) else {
            return Ok(());
        };
        let count = u32::from_le_bytes([head[0], head[1], head[2], head[3]]) as usize;
        let mut pos = 4usize;
        let mut verified: Vec<Change> = Vec::new();
        let mut new_sigs: Vec<([u8; 32], [u8; 64])> = Vec::new();
        for _ in 0..count {
            let Some(lenb) = buf.get(pos..pos + 4) else {
                break;
            };
            let clen = u32::from_le_bytes([lenb[0], lenb[1], lenb[2], lenb[3]]) as usize;
            pos += 4;
            let Some(cb) = buf.get(pos..pos + clen) else {
                break;
            };
            pos += clen;
            let Some(sigb) = buf.get(pos..pos + 64) else {
                break;
            };
            pos += 64;
            let Ok(change) = Change::from_bytes(cb.to_vec()) else {
                continue;
            };
            let author = change.actor_id().to_bytes();
            if author.len() != 32 {
                continue;
            }
            let mut author32 = [0u8; 32];
            author32.copy_from_slice(author);
            let h = change.hash().0;
            let Ok(pk) = PublicKey::from_bytes(&author32) else {
                continue;
            };
            let mut sig = [0u8; 64];
            sig.copy_from_slice(sigb);
            if pk
                .verify(
                    &change_sign_input(&self.id, &h),
                    &Signature::from_bytes(&sig),
                )
                .is_err()
            {
                continue; // forged / unsigned change
            }
            if !self.is_writer(&author32) {
                continue; // Reader / non-member author — rejected by honest peers
            }
            verified.push(change);
            new_sigs.push((h, sig));
        }
        if verified.is_empty() {
            return Ok(());
        }
        {
            let mut doc = self.doc.lock().await;
            doc.apply_changes(verified)
                .map_err(|e| CoreError::Sync(e.to_string()))?;
        }
        {
            let mut sigs = self.sigs.lock().expect("sigs lock");
            for (h, sig) in new_sigs {
                sigs.put(h, sig);
            }
            sigs.persist();
        }
        Ok(())
    }
}

/// A membership change requested through the engine API (`Mesh::add_member`, etc.).
pub(crate) enum MemberOp {
    Add([u8; 32], Role),
    SetRole([u8; 32], Role),
    Remove([u8; 32]),
}

/// Per-change author signatures, persisted to `sigs.bin` as `[hash(32) || sig(64)]*`.
struct SigStore {
    path: PathBuf,
    map: HashMap<[u8; 32], [u8; 64]>,
}

impl SigStore {
    fn load(path: PathBuf) -> SigStore {
        let mut map = HashMap::new();
        if let Ok(bytes) = std::fs::read(&path) {
            for chunk in bytes.chunks_exact(96) {
                let mut h = [0u8; 32];
                h.copy_from_slice(&chunk[..32]);
                let mut s = [0u8; 64];
                s.copy_from_slice(&chunk[32..]);
                map.insert(h, s);
            }
        }
        SigStore { path, map }
    }
    fn has(&self, h: &[u8; 32]) -> bool {
        self.map.contains_key(h)
    }
    fn get(&self, h: &[u8; 32]) -> Option<[u8; 64]> {
        self.map.get(h).copied()
    }
    fn put(&mut self, h: [u8; 32], sig: [u8; 64]) {
        self.map.insert(h, sig);
    }
    fn persist(&self) {
        let mut out = Vec::with_capacity(self.map.len() * 96);
        for (h, s) in &self.map {
            out.extend_from_slice(h);
            out.extend_from_slice(s);
        }
        let tmp = self.path.with_extension("bin.tmp");
        if std::fs::write(&tmp, &out).is_ok() {
            let _ = std::fs::rename(&tmp, &self.path);
        }
    }
}

/// What an author signs for a change: domain || space || change-hash. Binding the Space
/// stops a valid signature being replayed onto an identical change in another Space.
fn change_sign_input(space_id: &SpaceId, change_hash: &[u8; 32]) -> Vec<u8> {
    let mut v = Vec::with_capacity(CHANGE_SIG_DOMAIN.len() + 64);
    v.extend_from_slice(CHANGE_SIG_DOMAIN);
    v.extend_from_slice(space_id.as_bytes());
    v.extend_from_slice(change_hash);
    v
}

fn serialize_heads(heads: &[ChangeHash]) -> Vec<u8> {
    let mut v = Vec::with_capacity(heads.len() * 32);
    for h in heads {
        v.extend_from_slice(&h.0);
    }
    v
}

fn parse_heads(bytes: &[u8]) -> Vec<ChangeHash> {
    bytes
        .chunks_exact(32)
        .map(|c| {
            let mut a = [0u8; 32];
            a.copy_from_slice(c);
            ChangeHash(a)
        })
        .collect()
}

async fn write_frame(send: &mut SendStream, data: &[u8]) -> Result<()> {
    send.write_all(&(data.len() as u32).to_le_bytes())
        .await
        .map_err(sync_err)?;
    send.write_all(data).await.map_err(sync_err)?;
    Ok(())
}

async fn read_frame(recv: &mut RecvStream) -> Result<Vec<u8>> {
    let mut len = [0u8; 4];
    recv.read_exact(&mut len).await.map_err(sync_err)?;
    let n = u32::from_le_bytes(len) as usize;
    if n > MAX_VERIFIED_FRAME {
        return Err(CoreError::Sync(format!("verified frame too large: {n}")));
    }
    let mut buf = vec![0u8; n];
    recv.read_exact(&mut buf).await.map_err(sync_err)?;
    Ok(buf)
}

fn sync_err(e: impl std::fmt::Display) -> CoreError {
    CoreError::Sync(e.to_string())
}

/// Monotonic counter for unique save temp-file names (avoids concurrent-save clobber).
static SAVE_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// The set of Spaces a device is currently running. Shared (`Arc`) between the accept
/// dispatchers, the sync loop, and the public API.
pub(crate) struct SpaceRegistry {
    spaces: StdMutex<HashMap<SpaceId, Arc<SpaceState>>>,
}

impl SpaceRegistry {
    pub(crate) fn new() -> SpaceRegistry {
        SpaceRegistry {
            spaces: StdMutex::new(HashMap::new()),
        }
    }

    /// Register a Space, returning the shared handle.
    pub(crate) fn insert(&self, state: SpaceState) -> Arc<SpaceState> {
        let arc = Arc::new(state);
        self.spaces
            .lock()
            .expect("registry lock")
            .insert(arc.id, arc.clone());
        arc
    }

    pub(crate) fn get(&self, id: &SpaceId) -> Option<Arc<SpaceState>> {
        self.spaces.lock().expect("registry lock").get(id).cloned()
    }

    pub(crate) fn contains(&self, id: &SpaceId) -> bool {
        self.spaces.lock().expect("registry lock").contains_key(id)
    }

    pub(crate) fn remove(&self, id: &SpaceId) -> Option<Arc<SpaceState>> {
        self.spaces.lock().expect("registry lock").remove(id)
    }

    /// A snapshot of all registered Spaces.
    pub(crate) fn all(&self) -> Vec<Arc<SpaceState>> {
        self.spaces
            .lock()
            .expect("registry lock")
            .values()
            .cloned()
            .collect()
    }

    pub(crate) fn infos(&self) -> Vec<SpaceInfo> {
        let mut v: Vec<SpaceInfo> = self
            .spaces
            .lock()
            .expect("registry lock")
            .values()
            .map(|s| s.info())
            .collect();
        // Stable ordering: default first, then by name then id.
        v.sort_by(|a, b| {
            b.is_default
                .cmp(&a.is_default)
                .then_with(|| a.name.cmp(&b.name))
                .then_with(|| a.id.to_hex().cmp(&b.id.to_hex()))
        });
        v
    }
}

/// A Space's friendly name lives in a plain UTF-8 `name` file (no serde dependency in
/// the engine). Absent ⇒ a sensible default.
fn read_name(dir: &Path) -> Option<String> {
    let s = std::fs::read_to_string(dir.join("name")).ok()?;
    let s = s.trim().to_string();
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

pub(crate) fn write_name(dir: &Path, name: &str) -> std::io::Result<()> {
    let tmp = dir.join("name.tmp");
    std::fs::write(&tmp, name.trim().as_bytes())?;
    std::fs::rename(&tmp, dir.join("name"))
}

fn default_name(id: &SpaceId) -> String {
    if id.is_default() {
        "Default".to_string()
    } else {
        "Space".to_string()
    }
}

/// Load the replica from `path`, recovering from a corrupt/torn/empty snapshot by
/// moving it aside and starting fresh (it re-converges from peers — a local-first app
/// must never brick on a damaged file). A missing file is the normal first run.
fn load_or_recover(path: &Path, actor: ActorId, key: &[u8; 32]) -> SharedDoc {
    match std::fs::read(path) {
        Ok(bytes) if !bytes.is_empty() => match atrest::decrypt(key, &bytes) {
            Some(plain) => match doc::load(&plain, actor.clone()) {
                Ok(d) => d,
                Err(_) => move_aside_and_fresh(path, actor),
            },
            None => move_aside_and_fresh(path, actor),
        },
        Ok(_) => move_aside_and_fresh(path, actor),
        Err(_) => doc::open(actor),
    }
}

fn move_aside_and_fresh(path: &Path, actor: ActorId) -> SharedDoc {
    let aside = path.with_extension(format!("automerge.corrupt.{}", std::process::id()));
    let _ = std::fs::rename(path, &aside);
    doc::open(actor)
}

/// Load an enforced Space's snapshot: `[epoch(8) || AEAD]`. The epoch header names the
/// epoch key the at-rest key was derived from; if we don't hold that epoch key yet (or
/// the blob is corrupt/torn) we recover fresh and re-converge from peers.
fn load_enforced(path: &Path, actor: ActorId, store: &EpochStore, id: &SpaceId) -> SharedDoc {
    match std::fs::read(path) {
        Ok(bytes) if bytes.len() >= 8 => {
            let mut e = [0u8; 8];
            e.copy_from_slice(&bytes[0..8]);
            let snap_epoch = u64::from_le_bytes(e);
            match store.get(snap_epoch) {
                Some(ek) => {
                    let at = epoch::derive_atrest(&ek.key, id, snap_epoch);
                    match atrest::decrypt(&at, &bytes[8..]) {
                        Some(plain) => doc::load(&plain, actor.clone())
                            .unwrap_or_else(|_| move_aside_and_fresh(path, actor)),
                        None => move_aside_and_fresh(path, actor),
                    }
                }
                None => move_aside_and_fresh(path, actor),
            }
        }
        Ok(bytes) if !bytes.is_empty() => move_aside_and_fresh(path, actor),
        _ => doc::open(actor),
    }
}
