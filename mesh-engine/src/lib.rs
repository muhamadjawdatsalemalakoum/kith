//! # mesh-engine — the serverless P2P substrate
//!
//! The shared engine the whole family runs on: a flat peer-to-peer mesh where
//! every device is an equal peer holding a full, end-to-end-encrypted replica of
//! a small mutable [Automerge] document, synced directly between a user's own
//! devices over [iroh] QUIC (mainline-DHT discovery, relays only as fallback).
//! There is no hub and no account.
//!
//! Apps are thin: centralTabs (tabs), agent-memory, Dropwire-on-mesh, an MCP app —
//! each brings its own data model + UX and runs on this one substrate, the way
//! iroh-blobs/-docs/-gossip run on iroh. This crate is the *only* place that
//! depends on `iroh` / `automerge`; apps speak its types — and the re-exported
//! [`automerge`] — so the whole family shares exactly one CRDT version.
//!
//! ## What the engine does for you
//! - **State primitive** (this doc): a CRDT replica that auto-syncs across the
//!   peers you add, conflict-free and offline-tolerant, persisted to disk.
//! - **Blob primitive**: content-addressed file transfer ([`Mesh::share_file`] /
//!   [`Mesh::fetch_file`]).
//! - **Account-free pairing**: [`pairing`] (SPAKE2 group key from a short code).
//!
//! [Automerge]: https://automerge.org
//! [iroh]: https://iroh.computer

mod atrest;
mod blobs;
mod config;
mod doc;
mod endpoint;
mod error;
mod identity;
mod keys;
mod pair;
pub mod pairing;
mod sync;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use iroh::protocol::Router;
use iroh_blobs::api::TempTag;
use tokio::sync::{mpsc, Mutex, Notify};
use tokio_util::sync::CancellationToken;

// The engine's public vocabulary. Apps build their data models on the re-exported
// `automerge` (one CRDT version family-wide), move files via the blob primitive
// keyed by `Hash`, and address peers with `EndpointAddr`.
pub use automerge;
pub use automerge::Automerge;
pub use iroh::EndpointAddr;
pub use iroh_blobs::Hash;

pub use config::{CoreConfig, Infra};
pub use doc::SharedDoc;
pub use error::{CoreError, Result};
pub use sync::MeshSync;

/// ALPN for the engine's mesh sync protocol.
pub const MESH_ALPN: &[u8] = b"mesh-engine/sync/1";

/// How often the background loop re-syncs with known peers (also wakes early on a
/// local change or a newly added peer).
const SYNC_INTERVAL: Duration = Duration::from_millis(1500);
/// Bound on dialing a peer before treating it as unreachable (so a dead peer can't
/// hang the sync loop or a `fetch`).
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// Bound on a single sync round once connected.
const SYNC_ROUND_TIMEOUT: Duration = Duration::from_secs(30);
/// Monotonic counter for unique save temp-file names (avoids concurrent-save clobber).
static SAVE_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Shared engine state, held behind an `Arc` so the background sync loop and the
/// public handle share one set of peers, one replica, and one router.
struct Inner {
    router: Router,
    sync: Arc<MeshSync>,
    doc: SharedDoc,
    blobs_store: iroh_blobs::store::fs::FsStore,
    /// Temp tags for files we serve, held so their blobs aren't garbage-collected.
    kept_tags: Mutex<Vec<TempTag>>,
    /// Peers we keep converged with (their reachable addresses).
    peers: Mutex<Vec<EndpointAddr>>,
    /// Pinged on a local change or a new peer to wake the sync loop immediately.
    changed: Notify,
    /// Where the compacted replica snapshot is persisted (encrypted at rest).
    doc_path: PathBuf,
    /// Per-device key encrypting the snapshot on disk.
    atrest_key: [u8; 32],
    /// Shared group secret that gates who may sync (held for pairing handoff).
    group_key: [u8; 32],
    /// The currently-armed pairing code (Some while in "add a device" mode).
    pair_armed: Arc<std::sync::Mutex<Option<Vec<u8>>>>,
    /// Stops the background loop on shutdown.
    cancel: CancellationToken,
}

/// A running mesh peer. Cheap to clone (internally `Arc`). Apps hold this.
#[derive(Clone)]
pub struct Mesh {
    inner: Arc<Inner>,
}

impl Mesh {
    /// Start a peer: load/derive identity, restore-or-open the replica, bind the
    /// endpoint, spin up the always-on sync router, and start the background loop
    /// that keeps known peers converged and persists the replica.
    pub async fn start(config: CoreConfig) -> Result<Mesh> {
        std::fs::create_dir_all(&config.data_dir)?;

        let secret = identity::load_or_create(&config.data_dir.join("node.key"))?;
        let actor = identity::actor_id(&secret);
        // Per-device key that encrypts the replica snapshot at rest.
        let atrest_key = keys::load_or_create(&config.data_dir.join("atrest.key"))?;

        // Restore the replica (decrypting at rest); on a corrupt/torn/empty or
        // undecryptable snapshot, recover by booting fresh (re-converges from peers)
        // instead of bricking the app.
        let doc_path = config.data_dir.join("doc.automerge");
        let doc = load_or_recover(&doc_path, actor, &atrest_key);

        // The group key gates who may sync — provided explicitly (pairing / tests) or
        // loaded/generated in the data dir.
        let group_key = match config.group_key {
            Some(k) => k,
            None => keys::load_or_create(&config.data_dir.join("group.key"))?,
        };

        let (merged_tx, _merged_rx) = mpsc::channel::<Automerge>(16);
        let sync = MeshSync::with_shared(doc.clone(), group_key, merged_tx);

        let blobs_store = blobs::open(&config.data_dir.join("blobs")).await?;
        let endpoint = endpoint::build(secret, &config.infra, config.enable_blobs).await?;
        let pair_armed = Arc::new(std::sync::Mutex::new(None));
        // Mesh sync + pairing always; blobs only when explicitly enabled.
        let mut builder = Router::builder(endpoint)
            .accept(MESH_ALPN, sync.clone())
            .accept(
                pair::PAIR_ALPN,
                pair::PairingHandler {
                    armed: pair_armed.clone(),
                    group_key,
                },
            );
        if config.enable_blobs {
            builder = builder.accept(
                iroh_blobs::ALPN,
                iroh_blobs::BlobsProtocol::new(&blobs_store, None),
            );
        }
        let router = builder.spawn();

        let inner = Arc::new(Inner {
            router,
            sync,
            doc,
            blobs_store,
            kept_tags: Mutex::new(Vec::new()),
            peers: Mutex::new(Vec::new()),
            changed: Notify::new(),
            doc_path,
            atrest_key,
            group_key,
            pair_armed,
            cancel: CancellationToken::new(),
        });

        tokio::spawn(sync_loop(inner.clone()));
        Ok(Mesh { inner })
    }

    /// This device's stable public identity (`EndpointId`), as a string.
    pub fn endpoint_id(&self) -> String {
        self.inner.router.endpoint().id().to_string()
    }

    /// This device's group key — the shared secret that gates who may sync. Hand it to
    /// a new device during pairing so it can join the group.
    pub fn group_key(&self) -> [u8; 32] {
        self.inner.group_key
    }

    /// Enter "add a device" mode: answer ONE pairing attempt presenting `code` (a
    /// short human code shown on this device), then disarm. Run this on the device the
    /// user is joining FROM.
    pub fn arm_pairing(&self, code: &[u8]) {
        *self.inner.pair_armed.lock().expect("armed lock") = Some(code.to_vec());
    }

    /// Join the group hosted by `host` using the shared short `code`: run the SPAKE2
    /// pairing handshake, receive the group key, and persist it. **Restart this peer**
    /// afterwards to adopt the group and begin syncing. (Requires a file-based group
    /// key — i.e. `CoreConfig.group_key == None`.)
    pub async fn pair_with(&self, host: EndpointAddr, code: &[u8]) -> Result<()> {
        let gk = pair::join(self.inner.router.endpoint(), host, code)
            .await
            .map_err(|e| CoreError::Pairing(e.to_string()))?;
        let dir = self
            .inner
            .doc_path
            .parent()
            .unwrap_or_else(|| Path::new("."));
        let group_path = dir.join("group.key");
        let tmp = group_path.with_extension("key.tmp");
        std::fs::write(&tmp, gk)?;
        std::fs::rename(&tmp, &group_path)?;
        Ok(())
    }

    /// Revoke access by rotating to a NEW group key (persisted for next start). The
    /// evicted device's old key stops authenticating. **Restart this device, then
    /// re-pair the devices you keep** with the new key. (Manual revocation; automatic
    /// epoch rotation across the group is a future enhancement.)
    pub fn rotate_group_key(&self) -> Result<[u8; 32]> {
        let new = iroh::SecretKey::generate().to_bytes();
        let dir = self
            .inner
            .doc_path
            .parent()
            .unwrap_or_else(|| Path::new("."));
        let group_path = dir.join("group.key");
        let tmp = group_path.with_extension("key.tmp");
        std::fs::write(&tmp, new)?;
        std::fs::rename(&tmp, &group_path)?;
        Ok(new)
    }

    /// This device's connectable address (id + direct/relay transports).
    pub fn endpoint_addr(&self) -> EndpointAddr {
        self.inner.router.endpoint().addr()
    }

    /// Wait (time-boxed) for the relay handshake so [`Mesh::endpoint_addr`] includes
    /// a relay-reachable address. Needed before sharing an address in relay-backed
    /// modes; a near-instant no-op on direct/local-only links.
    pub async fn online(&self) {
        let _ = tokio::time::timeout(
            Duration::from_secs(10),
            self.inner.router.endpoint().online(),
        )
        .await;
    }

    /// Add a peer to keep converged with. The background loop will sync with it
    /// continuously (and immediately). Idempotent-ish; add each of your devices.
    pub async fn add_peer(&self, peer: EndpointAddr) {
        self.inner.peers.lock().await.push(peer);
        self.inner.changed.notify_waiters();
    }

    /// Tell the engine the local replica just changed, so it syncs + persists now
    /// instead of waiting for the next interval. Call after writing via [`Mesh::doc`].
    pub fn announce_change(&self) {
        self.inner.changed.notify_waiters();
    }

    /// Dial one peer and run a single sync round now (initiator). Both replicas
    /// converge. Pending, not lost, if the peer is unreachable.
    pub async fn sync_with(&self, peer: impl Into<EndpointAddr>) -> Result<()> {
        sync_peer(&self.inner, peer.into()).await
    }

    /// Sync once with every known peer (best-effort; unreachable peers are skipped).
    pub async fn sync_all(&self) -> Result<()> {
        let peers = self.inner.peers.lock().await.clone();
        for p in peers {
            let _ = sync_peer(&self.inner, p).await;
        }
        Ok(())
    }

    /// The live replica handle. Apps define their own schema on this.
    pub fn doc(&self) -> SharedDoc {
        self.inner.doc.clone()
    }

    /// BLOB PRIMITIVE — serve a file. Imports `path` into the content-addressed
    /// store and returns its [`Hash`]; served by hash to any peer that connects,
    /// kept alive for the life of this `Mesh`.
    pub async fn share_file(&self, path: &Path) -> Result<Hash> {
        let tt = blobs::add_file(&self.inner.blobs_store, path).await?;
        let hash = tt.hash();
        self.inner.kept_tags.lock().await.push(tt);
        Ok(hash)
    }

    /// BLOB PRIMITIVE — fetch a file by `hash` from `peer`, writing it to `dest`.
    /// Resumes from partial data; BLAKE3-verified end to end.
    pub async fn fetch_file(
        &self,
        peer: impl Into<EndpointAddr>,
        hash: Hash,
        dest: &Path,
    ) -> Result<()> {
        let connect = self.inner.router.endpoint().connect(peer, iroh_blobs::ALPN);
        let conn = tokio::time::timeout(CONNECT_TIMEOUT, connect)
            .await
            .map_err(|_| CoreError::Unreachable("connect timed out".into()))?
            .map_err(|e| CoreError::Unreachable(e.to_string()))?;
        blobs::fetch_to(&self.inner.blobs_store, conn, hash, dest).await
    }

    /// Persist the replica now (a compacted snapshot). Also done automatically by
    /// the background loop and on shutdown.
    pub async fn save(&self) -> Result<()> {
        save_doc(&self.inner).await
    }

    /// Gracefully stop: halt the background loop, persist the replica, and shut
    /// down the router + blob store.
    pub async fn shutdown(self) -> Result<()> {
        self.inner.cancel.cancel();
        let _ = save_doc(&self.inner).await;
        let _ = self.inner.router.shutdown().await;
        let _ = self.inner.blobs_store.shutdown().await;
        Ok(())
    }
}

/// Background loop: on a change/new-peer ping or every [`SYNC_INTERVAL`], sync with
/// every known peer and persist the replica. Exits when the engine shuts down.
async fn sync_loop(inner: Arc<Inner>) {
    let mut spawned = 0usize;
    loop {
        tokio::select! {
            _ = inner.cancel.cancelled() => break,
            _ = inner.changed.notified() => {}
            _ = tokio::time::sleep(SYNC_INTERVAL) => {}
        }
        // Give each newly-added peer its OWN long-lived sync task, so a slow/dead peer
        // (bounded by its own timeouts) is fully isolated — it can never delay syncing
        // with the others. This loop just spawns those tasks and persists.
        let peers = inner.peers.lock().await.clone();
        while spawned < peers.len() {
            tokio::spawn(peer_sync_task(inner.clone(), peers[spawned].clone()));
            spawned += 1;
        }
        let _ = save_doc(&inner).await;
    }
}

/// One independent task per peer: re-sync on a change/new-peer ping or every
/// interval, each round bounded by timeouts. Isolation is the whole point — a dead
/// peer here spins on its own without touching any other peer or persistence.
async fn peer_sync_task(inner: Arc<Inner>, peer: EndpointAddr) {
    loop {
        tokio::select! {
            _ = inner.cancel.cancelled() => break,
            _ = inner.changed.notified() => {}
            _ = tokio::time::sleep(SYNC_INTERVAL) => {}
        }
        let _ = sync_peer(&inner, peer.clone()).await;
    }
}

/// Dial a peer and drive one initiator-side sync round, bounded by timeouts so an
/// unreachable/slow peer fails cleanly instead of hanging.
async fn sync_peer(inner: &Arc<Inner>, peer: EndpointAddr) -> Result<()> {
    let connect = inner.router.endpoint().connect(peer, MESH_ALPN);
    let conn = tokio::time::timeout(CONNECT_TIMEOUT, connect)
        .await
        .map_err(|_| CoreError::Unreachable("connect timed out".into()))?
        .map_err(|e| CoreError::Unreachable(e.to_string()))?;
    let round = inner.sync.clone().initiate_sync(conn);
    tokio::time::timeout(SYNC_ROUND_TIMEOUT, round)
        .await
        .map_err(|_| CoreError::Sync("sync round timed out".into()))?
        .map_err(|e| CoreError::Sync(e.to_string()))?;
    Ok(())
}

/// Write a compacted snapshot durably: fsync the bytes, atomically rename into
/// place, then fsync the directory — so a completed save survives power loss and a
/// crash mid-write can't corrupt the replica. Blocking fs work runs off the runtime.
async fn save_doc(inner: &Arc<Inner>) -> Result<()> {
    let plain = doc::save(&inner.doc).await;
    let bytes = atrest::encrypt(&inner.atrest_key, &plain); // encrypt at rest
    let path = inner.doc_path.clone();
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
                let _ = d.sync_all(); // best-effort dir fsync (may no-op on Windows)
            }
        }
        Ok(())
    })
    .await
    .map_err(|e| anyhow::anyhow!("save task failed: {e}"))??;
    Ok(())
}

/// Load the replica from `path`, recovering from a corrupt/torn/empty snapshot by
/// moving it aside and starting fresh (it re-converges from peers — a local-first
/// app must never brick on a damaged file). A missing file is the normal first run.
fn load_or_recover(path: &Path, actor: automerge::ActorId, key: &[u8; 32]) -> SharedDoc {
    match std::fs::read(path) {
        Ok(bytes) if !bytes.is_empty() => match atrest::decrypt(key, &bytes) {
            Some(plain) => match doc::load(&plain, actor.clone()) {
                Ok(d) => d,
                Err(_) => move_aside_and_fresh(path, actor),
            },
            None => move_aside_and_fresh(path, actor), // wrong key / corrupt / tampered
        },
        Ok(_) => move_aside_and_fresh(path, actor), // present but empty (failed write)
        Err(_) => doc::open(actor),                 // missing (normal) or unreadable
    }
}

fn move_aside_and_fresh(path: &Path, actor: automerge::ActorId) -> SharedDoc {
    let aside = path.with_extension(format!("automerge.corrupt.{}", std::process::id()));
    let _ = std::fs::rename(path, &aside);
    doc::open(actor)
}
