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

use automerge::ActorId;
use iroh::endpoint::Connection;
use iroh::protocol::AcceptError;
use iroh::{EndpointAddr, EndpointId};
use iroh_blobs::api::TempTag;
use iroh_blobs::store::fs::FsStore;
use iroh_blobs::{BlobsProtocol, Hash};
use sha2::{Digest, Sha256};
use tokio::sync::{mpsc, Mutex};

use crate::doc::SharedDoc;
use crate::error::Result;
use crate::sync::MeshSync;
use crate::{atrest, blobs, doc, keys};

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
    /// Current-epoch group key (gates who may sync; derives nothing here — the at-rest
    /// key is separate). Rotation persists a new key, adopted on restart (M1); live
    /// epoch rotation is M3.
    group_key: [u8; 32],
    /// Per-device key encrypting this Space's replica snapshot on disk.
    atrest_key: [u8; 32],
    doc: SharedDoc,
    sync: Arc<MeshSync>,
    store: FsStore,
    blobs_protocol: BlobsProtocol,
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
        configured_key: Option<[u8; 32]>,
    ) -> Result<SpaceState> {
        std::fs::create_dir_all(&dir)?;
        let name = read_name(&dir).unwrap_or_else(|| default_name(&id));
        let atrest_key = keys::load_or_create(&dir.join("atrest.key"))?;
        let group_key = match configured_key {
            Some(k) => k,
            None => keys::load_or_create(&dir.join("group.key"))?,
        };
        let doc_path = dir.join("doc.automerge");
        let doc = load_or_recover(&doc_path, actor, &atrest_key);

        let store = blobs::open(&dir.join("blobs")).await?;
        let blobs_protocol = BlobsProtocol::new(&store, None);

        // The merged-doc out-channel is unused today (the receiver is dropped); a sync
        // round still merges into the shared `doc` directly. Kept for a future
        // change-notification consumer.
        let (merged_tx, _merged_rx) = mpsc::channel::<automerge::Automerge>(16);
        let sync = MeshSync::with_shared(doc.clone(), group_key, id, merged_tx);

        Ok(SpaceState {
            id,
            name,
            doc_path,
            group_key,
            atrest_key,
            doc,
            sync,
            store,
            blobs_protocol,
            kept_tags: Mutex::new(Vec::new()),
            peers: Mutex::new(Vec::new()),
            last_sync: StdMutex::new(HashMap::new()),
        })
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

    /// Import a file into this Space's content store and keep it served.
    pub(crate) async fn share_file(&self, path: &Path) -> Result<Hash> {
        let tt = blobs::add_file(&self.store, path).await?;
        let hash = tt.hash();
        self.kept_tags.lock().await.push(tt);
        Ok(hash)
    }

    /// Persist this Space's replica (compacted, encrypted at rest, crash-safe).
    pub(crate) async fn save(&self) -> Result<()> {
        let plain = doc::save(&self.doc).await;
        let bytes = atrest::encrypt(&self.atrest_key, &plain);
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
