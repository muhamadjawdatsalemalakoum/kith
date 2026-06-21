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
            harden_permissions(path);
            Ok(key)
        }
        Err(e) => Err(CoreError::Io(e)),
    }
}

/// Restrict a freshly-written key file to the current user. Best-effort: `0600` on
/// Unix, and on Windows an ACL that removes inheritance and grants only this user
/// (so the key isn't readable via default-inherited ACLs).
fn harden_permissions(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    #[cfg(windows)]
    {
        if let Ok(user) = std::env::var("USERNAME") {
            if !user.trim().is_empty() {
                use std::os::windows::process::CommandExt;
                const CREATE_NO_WINDOW: u32 = 0x0800_0000;
                let _ = std::process::Command::new("icacls")
                    .arg(path)
                    .arg("/inheritance:r")
                    .arg("/grant:r")
                    .arg(format!("{user}:F"))
                    .creation_flags(CREATE_NO_WINDOW)
                    .output();
            }
        }
    }
}
