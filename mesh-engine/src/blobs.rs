//! The blob / content primitive: content-addressed, immutable, BLAKE3-verified
//! file transfer over the mesh.
//!
//! Where [`crate::sync`] handles small MUTABLE state (CRDT), this handles large
//! IMMUTABLE bytes. Both run on the same endpoint, each on its own ALPN — so one
//! engine powers Dropwire-on-mesh, a mesh-browser's static sites, and file-sharing
//! inside chat. The calls here are lifted from the Dropwire engine (iroh-blobs
//! 0.103), trimmed to a single raw blob (no Collection) for the minimal primitive.

use std::path::Path;
use std::time::Duration;

use anyhow::Context;
use iroh::endpoint::Connection;
use iroh_blobs::api::blobs::{AddPathOptions, ExportMode, ExportOptions, ImportMode};
use iroh_blobs::api::remote::GetProgressItem;
use iroh_blobs::api::TempTag;
use iroh_blobs::store::fs::FsStore;
use iroh_blobs::{BlobFormat, Hash, HashAndFormat};
use n0_future::StreamExt;

use crate::error::{CoreError, Result};

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

/// Download the raw blob named by `hash` over `conn` (resuming from any partial
/// data already local), then export it to `dest`. BLAKE3-verified end to end.
pub async fn fetch_to(store: &FsStore, conn: Connection, hash: Hash, dest: &Path) -> Result<()> {
    let hf = HashAndFormat::raw(hash);

    // Resume: only request what's missing (everything, on a first fetch).
    let local = store
        .remote()
        .local(hf)
        .await
        .context("inspect local store")?;
    if !local.is_complete() {
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
                Ok(Some(GetProgressItem::Progress(_))) => {}
            }
        }
    }

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
