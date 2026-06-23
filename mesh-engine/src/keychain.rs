//! Best-effort OS keychain storage for 32-byte keys.
//!
//! Native backends on Windows (Credential Manager) and macOS (Keychain) — no C deps. On
//! Linux there is no backend compiled in (to avoid a libdbus/secret-service build
//! requirement), so every call here fails and the caller keeps the hardened key *file*.
//! Used by [`crate::keys::secured_load_or_create`] when [`crate::KeyStore::Keychain`] is
//! selected. Every function is infallible at the type level (returns `bool`/`Option`) so a
//! missing or locked keychain degrades to the file fallback instead of bricking startup.

/// Service name under which Kith stores its keys.
const SERVICE: &str = "kith";

/// Store a 32-byte `key` under `account`. Returns whether it was actually stored (false if
/// there is no keychain backend or the write failed — the caller then keeps the file).
pub fn store(account: &str, key: &[u8; 32]) -> bool {
    match keyring::Entry::new(SERVICE, account) {
        Ok(entry) => entry.set_secret(key).is_ok(),
        Err(_) => false,
    }
}

/// Load a 32-byte key for `account`. `None` if absent, no backend, or the wrong length.
pub fn load(account: &str) -> Option<[u8; 32]> {
    let entry = keyring::Entry::new(SERVICE, account).ok()?;
    let secret = entry.get_secret().ok()?;
    secret.try_into().ok()
}

/// Remove `account`'s key from the keychain (best-effort; ignores "not found").
pub fn delete(account: &str) {
    if let Ok(entry) = keyring::Entry::new(SERVICE, account) {
        let _ = entry.delete_credential();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Round-trips a key through the REAL OS keychain on platforms that have one
    /// (Windows/macOS); on a backend-less host (headless Linux) `store` returns false and
    /// the test documents that the file fallback is the path there.
    #[test]
    fn keychain_roundtrip() {
        let account = "kith-test-roundtrip-v1";
        let key = [9u8; 32];
        if !store(account, &key) {
            eprintln!("no OS keychain backend on this host; file fallback is used instead");
            return;
        }
        assert_eq!(load(account), Some(key), "stored key round-trips");
        delete(account);
        assert_eq!(load(account), None, "deleted key is gone");
    }
}
