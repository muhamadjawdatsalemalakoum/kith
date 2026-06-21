//! The replicated Automerge document handle the sync protocol operates on.
//!
//! The engine owns the doc *lifecycle* (create / restore / compacted snapshot);
//! each app defines its own *schema* on the returned handle — see how centraltabs'
//! `model` writes spaces -> groups -> tabs. The engine stays schema-agnostic.
//!
//! We use the plain `Automerge` (not `AutoCommit`) on purpose: the `SyncDoc` trait
//! is implemented directly on `Automerge`, so `sync` avoids the AutoCommit
//! `.sync()` borrow dance.

use std::sync::Arc;

use automerge::{ActorId, Automerge};
use tokio::sync::Mutex;

use crate::error::Result;

/// Shared, lockable handle to this device's live document.
pub type SharedDoc = Arc<Mutex<Automerge>>;

/// Create the device's primary doc bound to its stable ActorId.
pub fn open(actor: ActorId) -> SharedDoc {
    Arc::new(Mutex::new(Automerge::new().with_actor(actor)))
}

/// Restore a primary doc from a saved snapshot, re-binding the stable ActorId.
pub fn load(bytes: &[u8], actor: ActorId) -> Result<SharedDoc> {
    let mut doc = Automerge::load(bytes).map_err(|e| anyhow::anyhow!("load doc: {e}"))?;
    doc.set_actor(actor);
    Ok(Arc::new(Mutex::new(doc)))
}

/// Save a COMPACTED snapshot for persistence. In automerge 0.10 `save()` *is* the
/// compaction step (there is no separate compact API).
pub async fn save(doc: &SharedDoc) -> Vec<u8> {
    doc.lock().await.save()
}
