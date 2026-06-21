//! Public error type for the engine.
//!
//! Internals use `anyhow` freely; everything crossing the public API is normalized
//! into this enum so apps never see an `iroh` or `automerge` error type directly.

use thiserror::Error;

/// Errors surfaced across the engine boundary.
#[derive(Debug, Error)]
pub enum CoreError {
    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),

    /// An Automerge document operation failed (transaction, load, merge).
    #[error("crdt error: {0}")]
    Crdt(#[from] automerge::AutomergeError),

    /// A peer could not be reached (offline, unreachable, or never overlapped).
    /// The update is not lost — it stays pending and is retried / carried forward.
    #[error("peer unreachable: {0}")]
    Unreachable(String),

    /// Device pairing failed (wrong code, or the PAKE exchange did not complete).
    #[error("pairing failed: {0}")]
    Pairing(String),

    /// The replication / sync exchange failed.
    #[error("sync error: {0}")]
    Sync(String),

    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

/// Convenience alias used throughout the public API (and by apps on the engine).
pub type Result<T> = std::result::Result<T, CoreError>;
