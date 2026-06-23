//! The blob / content primitive: content-addressed, immutable, BLAKE3-verified
//! file transfer over the mesh.
//!
//! Where [`crate::sync`] handles small MUTABLE state (CRDT), this handles large
//! IMMUTABLE bytes. Both run on the same endpoint, each on its own ALPN — so one
//! engine powers Dropwire-on-mesh, a mesh-browser's static sites, and file-sharing
//! inside chat. The calls here are lifted from the Dropwire engine (iroh-blobs
//! 0.103), trimmed to a single raw blob (no Collection) for the minimal primitive.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use iroh::endpoint::Connection;
use iroh::protocol::{AcceptError, ProtocolHandler};
use iroh_blobs::api::blobs::{AddPathOptions, ExportMode, ExportOptions, ImportMode};
use iroh_blobs::api::remote::GetProgressItem;
use iroh_blobs::api::TempTag;
use iroh_blobs::store::fs::FsStore;
use iroh_blobs::{BlobFormat, Hash, HashAndFormat};

use n0_future::StreamExt;

use crate::error::{CoreError, Result};
use crate::space::{SpaceId, SpaceRegistry};

/// SpaceId bytes that prefix the blob auth stream (mirrors the sync dispatcher).
const SPACE_ID_LEN: usize = 32;

/// The single blobs-ALPN accept handler. Like [`crate::sync::SyncDispatcher`], it reads
/// the SpaceId off the first (auth) stream, looks up the Space, runs THAT Space's
/// group-key gate, then hands the connection to that Space's stock blobs provider — so
/// a member of Space A can never fetch Space B's content, and a non-member is refused
/// before any byte is served. Without the gate the blobs ALPN would hand any hash to
/// any caller (the content hash would be the only capability).
#[derive(Clone)]
pub struct BlobDispatcher {
    registry: Arc<SpaceRegistry>,
}

impl BlobDispatcher {
    pub fn new(registry: Arc<SpaceRegistry>) -> Self {
        Self { registry }
    }
}

// Redacted Debug (ProtocolHandler requires Debug) — never log key material.
impl std::fmt::Debug for BlobDispatcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("BlobDispatcher { .. }")
    }
}

impl ProtocolHandler for BlobDispatcher {
    async fn accept(&self, conn: Connection) -> std::result::Result<(), AcceptError> {
        use crate::sync::acc_err;
        // The first bi-stream is `SpaceId || group-key handshake`; reject a non-member
        // (or an unknown Space) before the stock provider ever sees the connection.
        let (mut send, mut recv) = conn.accept_bi().await.map_err(acc_err)?;
        let mut id = [0u8; SPACE_ID_LEN];
        crate::auth::read_exact_timed(&mut recv, &mut id)
            .await
            .map_err(acc_err)?;
        let space_id = SpaceId::from_bytes(id);
        let Some(space) = self.registry.get(&space_id) else {
            return Err(acc_err("unknown space"));
        };
        let group_key = space.group_key();
        crate::auth::responder(&group_key, &mut send, &mut recv)
            .await
            .map_err(acc_err)?;
        // Membership gate (enforced Spaces): root content access in EndpointId, not mere
        // group-key possession — so a removed device (it keeps the stable group key) and
        // any non-member are refused before a byte is served. No-op for permissive Spaces.
        space.gate(&conn).map_err(acc_err)?;
        let _ = send.finish();
        // Authenticated + authorized for this Space: hand the rest of the connection to
        // that Space's stock blobs provider, which loops accepting the request stream(s).
        space.blobs_accept(conn).await
    }

    async fn shutdown(&self) {
        for space in self.registry.all() {
            space.blobs_shutdown().await;
        }
    }
}

/// If no download progress arrives within this window, treat the transfer as stalled
/// (e.g. a dead peer mid-fetch) instead of hanging forever. It resets on each
/// progress item, so legitimately large/slow transfers are unaffected.
const STALL_TIMEOUT: Duration = Duration::from_secs(30);

/// Open (or create) the persistent on-disk content store.
///
/// Persistence is what makes resume work across restarts — interrupted downloads
/// keep their partial data here.
pub async fn open(dir: &Path) -> Result<FsStore> {
    std::fs::create_dir_all(dir)?;
    let store = FsStore::load(dir).await.context("open blob store")?;
    Ok(store)
}

/// Import a file into the store as a single raw blob, returning a [`TempTag`] that
/// keeps it alive (drop the tag and it may be garbage-collected). The caller holds
/// the tag for as long as it wants to serve the file; the hash is `tag.hash()`.
pub async fn add_file(store: &FsStore, path: &Path) -> Result<TempTag> {
    let tt = store
        .add_path_with_opts(AddPathOptions {
            path: path.to_path_buf(),
            mode: ImportMode::TryReference,
            format: BlobFormat::Raw,
        })
        .temp_tag()
        .await
        .with_context(|| format!("import {}", path.display()))?;
    Ok(tt)
}

/// Download the raw blob named by `hash` over `conn` (resuming from any partial data
/// already local), then export it to `dest`. BLAKE3-verified end to end. Calls
/// `on_progress(bytes_downloaded, relayed)` as data arrives — the live byte count plus
/// whether the current path is via the relay (for a direct-vs-relayed badge). The total
/// size is known by the caller (it rides in the file offer), so it isn't reported here.
pub async fn fetch_to_with_progress(
    store: &FsStore,
    conn: Connection,
    hash: Hash,
    dest: &Path,
    group_key: &[u8; 32],
    space_id: &SpaceId,
    on_progress: impl FnMut(u64, bool),
) -> Result<()> {
    ensure_local(store, conn, hash, group_key, space_id, on_progress).await?;
    // Write the verified bytes out to `dest`.
    store
        .export_with_opts(ExportOptions {
            hash,
            target: dest.to_path_buf(),
            mode: ExportMode::Copy,
        })
        .await
        .with_context(|| format!("export to {}", dest.display()))?;
    Ok(())
}

/// Ensure the blob named by `hash` is fully present locally, fetching the missing parts
/// over `conn` (resuming from any partial data) if it isn't. No-op if already complete.
/// The transfer is SpaceId-routed + group-key gated exactly like [`fetch_to_with_progress`]
/// — it is the shared download core behind both download-to-path and read-into-memory.
pub async fn ensure_local(
    store: &FsStore,
    conn: Connection,
    hash: Hash,
    group_key: &[u8; 32],
    space_id: &SpaceId,
    mut on_progress: impl FnMut(u64, bool),
) -> Result<()> {
    let hf = HashAndFormat::raw(hash);
    // Resume: only request what's missing (everything, on a first fetch).
    let local = store
        .remote()
        .local(hf)
        .await
        .context("inspect local store")?;
    if local.is_complete() {
        return Ok(());
    }
    // Name the Space, then prove we hold its group key, before requesting any bytes
    // (the dispatcher routes on the SpaceId and gates on the key; an unauthenticated
    // peer — or one naming the wrong Space — is served nothing).
    {
        let (mut a_send, mut a_recv) = conn.open_bi().await.context("open blob auth stream")?;
        a_send
            .write_all(space_id.as_bytes())
            .await
            .context("send space id")?;
        crate::auth::initiator(group_key, &mut a_send, &mut a_recv)
            .await
            .context("blob group authentication")?;
        let _ = a_send.finish();
    }
    let route_conn = conn.clone(); // route can upgrade relay→direct mid-transfer
    let get = store.remote().execute_get(conn, local.missing());
    let mut stream = get.stream();
    loop {
        match tokio::time::timeout(STALL_TIMEOUT, stream.next()).await {
            Err(_) => return Err(CoreError::Sync("download stalled".into())),
            Ok(None) => break,
            Ok(Some(GetProgressItem::Done(_))) => break,
            Ok(Some(GetProgressItem::Error(e))) => {
                return Err(CoreError::Sync(format!("download failed: {e}")))
            }
            Ok(Some(GetProgressItem::Progress(offset))) => {
                on_progress(offset, conn_is_relayed(&route_conn));
            }
        }
    }
    Ok(())
}

/// Whether the blob is already fully present in the local store (no network).
pub async fn is_local_complete(store: &FsStore, hash: Hash) -> bool {
    store
        .remote()
        .local(HashAndFormat::raw(hash))
        .await
        .map(|l| l.is_complete())
        .unwrap_or(false)
}

/// Read `[start, end)` bytes of a LOCALLY-complete blob into memory (BLAKE3-verified).
/// The caller bounds the window, so this never loads an unbounded blob; ensure the blob
/// is local first (e.g. via [`ensure_local`]).
pub async fn read_range(store: &FsStore, hash: Hash, start: u64, end: u64) -> Result<Vec<u8>> {
    if end <= start {
        return Ok(Vec::new());
    }
    store
        .export_ranges(hash, start..end)
        .concatenate()
        .await
        .map_err(|e| CoreError::Sync(format!("read blob range: {e}")))
}

/// Whether the connection's active path currently runs through the relay (vs. a
/// direct hole-punched path). Used for the transfer's direct/relayed badge.
fn conn_is_relayed(conn: &Connection) -> bool {
    let paths = conn.paths();
    if let Some(p) = paths.iter().find(|p| p.is_selected()) {
        return p.is_relay();
    }
    // No explicitly-selected path: relayed only if there's no direct IP path at all.
    !paths.iter().any(|p| p.is_ip())
}
