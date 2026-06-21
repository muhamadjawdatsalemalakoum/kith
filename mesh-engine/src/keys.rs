//! 32-byte secret key files (the group key for auth, the at-rest key for disk
//! encryption). Same hardening as the identity key: corruption refuses rather than
//! silently overwriting, and creation is atomic.

use std::path::Path;

use crate::error::{CoreError, Result};

/// Load a 32-byte secret from `path`, or generate + persist one if absent.
pub fn load_or_create(path: &Path) -> Result<[u8; 32]> {
    match std::fs::read(path) {
        Ok(bytes) if bytes.len() == 32 => Ok(bytes.try_into().expect("len checked")),
        // Present-but-wrong-length = corruption: never overwrite.
        Ok(bytes) => Err(CoreError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("{} is {} bytes (expected 32)", path.display(), bytes.len()),
        ))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            // Reuse iroh's CSPRNG for 32 random bytes (no extra RNG dependency).
            let key = iroh::SecretKey::generate().to_bytes();
            let tmp = path.with_extension("key.tmp");
            std::fs::write(&tmp, key)?;
            std::fs::rename(&tmp, path)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
            }
            Ok(key)
        }
        Err(e) => Err(CoreError::Io(e)),
    }
}
