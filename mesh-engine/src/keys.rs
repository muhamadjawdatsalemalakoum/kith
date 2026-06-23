//! 32-byte secret key files (the group key for auth, the at-rest key for disk
//! encryption). Same hardening as the identity key: corruption refuses rather than
//! silently overwriting, and creation is atomic.

use std::path::Path;

use crate::config::KeyStore;
use crate::error::{CoreError, Result};
use crate::keychain;

/// Generate 32 fresh random bytes (a new group key, at-rest key, or Space nonce).
/// Reuses iroh's CSPRNG so no extra RNG dependency is pulled in.
pub fn generate() -> [u8; 32] {
    iroh::SecretKey::generate().to_bytes()
}

/// Persist a 32-byte secret to `path` atomically (temp file + rename) and harden its
/// permissions. Used to seed/rotate a Space's `group.key`.
pub fn write_key(path: &Path, key: &[u8; 32]) -> Result<()> {
    let tmp = path.with_extension("key.tmp");
    std::fs::write(&tmp, key)?;
    std::fs::rename(&tmp, path)?;
    harden_permissions(path);
    Ok(())
}

/// Load (or create) a 32-byte secret, honoring the [`KeyStore`] policy.
///
/// - [`KeyStore::File`] — the hardened file at `path` (the default; unchanged behavior).
/// - [`KeyStore::Keychain`] — prefer the OS keychain under `account`: load it if present;
///   otherwise migrate an existing file into the keychain (deleting the file once it's
///   safely stored); otherwise generate a fresh key and store it in the keychain. If there
///   is no keychain backend (e.g. headless Linux), transparently fall back to the file.
pub fn secured_load_or_create(account: &str, path: &Path, store: KeyStore) -> Result<[u8; 32]> {
    if store == KeyStore::File {
        return load_or_create(path);
    }
    // 1. Already in the keychain.
    if let Some(key) = keychain::load(account) {
        return Ok(key);
    }
    // 2. Migrate an existing key file into the keychain (only delete it once stored).
    match std::fs::read(path) {
        Ok(bytes) if bytes.len() == 32 => {
            let key: [u8; 32] = bytes.try_into().expect("len checked");
            if keychain::store(account, &key) {
                let _ = std::fs::remove_file(path);
            }
            return Ok(key);
        }
        Ok(bytes) if !bytes.is_empty() => {
            return Err(CoreError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("{} is {} bytes (expected 32)", path.display(), bytes.len()),
            )));
        }
        _ => {}
    }
    // 3. Generate fresh: keychain when available, else the hardened file fallback.
    let key = generate();
    if !keychain::store(account, &key) {
        write_key(path, &key)?;
    }
    Ok(key)
}

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
