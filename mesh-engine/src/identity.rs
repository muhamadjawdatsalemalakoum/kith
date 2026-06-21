//! Persistent per-device identity (Ed25519 secret key) and the stable Automerge
//! ActorId derived from it.
//!
//! A stable SecretKey means the same `EndpointId` across restarts; deriving the
//! ActorId from its public key gives each device a deterministic, distinct CRDT
//! writer id — so all of a device's edits share one actor and concurrent edits on
//! different devices touch disjoint authors. Reused from Dropwire.

use std::path::Path;

use automerge::ActorId;
use iroh::SecretKey;

use crate::error::{CoreError, Result};

/// Load the persisted identity at `key_path`, or generate and persist a new one.
///
/// The key file holds 32 raw bytes (written `0600` on Unix). Treat it as a secret:
/// it is this device's identity in the mesh.
pub fn load_or_create(key_path: &Path) -> Result<SecretKey> {
    match std::fs::read(key_path) {
        Ok(bytes) if bytes.len() == 32 => {
            let arr: [u8; 32] = bytes.try_into().expect("length checked above");
            Ok(SecretKey::from_bytes(&arr))
        }
        // A present-but-wrong-length key is corruption (torn write, truncation). NEVER
        // silently regenerate — that would change this device's EndpointId + ActorId,
        // orphan its CRDT history, and drop it from peers. Refuse instead.
        Ok(bytes) => Err(CoreError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!(
                "node.key is {} bytes (expected 32) — refusing to overwrite a corrupt identity",
                bytes.len()
            ),
        ))),
        // Only a genuinely-absent key is created fresh; a transient read error must
        // also not clobber the key.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            // iroh 1.0: SecretKey::generate() is parameterless (uses rand internally).
            let sk = SecretKey::generate();
            write_atomic(key_path, &sk.to_bytes())?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(key_path, std::fs::Permissions::from_mode(0o600));
            }
            Ok(sk)
        }
        Err(e) => Err(CoreError::Io(e)),
    }
}

/// Write bytes via a temp file + rename so a crash mid-write can't leave a torn key.
fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_extension("key.tmp");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// Derive this device's stable Automerge ActorId from its endpoint public key.
///
/// `PublicKey` == `EndpointId` in iroh 1.0; `public()` returns it and `as_bytes()`
/// yields the 32 raw bytes. `ActorId: From<Vec<u8>>`.
pub fn actor_id(secret_key: &SecretKey) -> ActorId {
    let eid = secret_key.public();
    ActorId::from(eid.as_bytes().to_vec())
}
