//! At-rest encryption for the replica snapshot (and, later, blobs).
//!
//! XChaCha20-Poly1305 AEAD. Output layout: `nonce(24) || ciphertext+tag`. The 24-byte
//! random nonce makes reuse a non-issue at our save volume.
//!
//! NOTE on the key: today the at-rest key lives in a `0600` file in the data dir
//! (`atrest.key`). That protects the data file alone (a stray copy / partial leak),
//! but not against an attacker who has the whole directory. Moving the key to an OS
//! keystore / passphrase is the documented upgrade (see ROADMAP M1.5).

use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{Key, XChaCha20Poly1305, XNonce};

const NONCE_LEN: usize = 24;

/// Encrypt `plaintext` under `key`, returning `nonce || ciphertext`.
pub fn encrypt(key: &[u8; 32], plaintext: &[u8]) -> Vec<u8> {
    let cipher = XChaCha20Poly1305::new(Key::from_slice(key));
    // 24 random nonce bytes from iroh's CSPRNG (no extra RNG dependency).
    let rand = iroh::SecretKey::generate().to_bytes();
    let nonce = XNonce::from_slice(&rand[..NONCE_LEN]);
    let ct = cipher
        .encrypt(nonce, plaintext)
        .expect("AEAD encryption cannot fail with a valid key/nonce");
    let mut out = Vec::with_capacity(NONCE_LEN + ct.len());
    out.extend_from_slice(&rand[..NONCE_LEN]);
    out.extend_from_slice(&ct);
    out
}

/// Decrypt `nonce || ciphertext`. Returns `None` if the data is malformed or the
/// authentication tag fails (wrong key / tampering / corruption).
pub fn decrypt(key: &[u8; 32], data: &[u8]) -> Option<Vec<u8>> {
    if data.len() < NONCE_LEN {
        return None;
    }
    let (nonce, ct) = data.split_at(NONCE_LEN);
    let cipher = XChaCha20Poly1305::new(Key::from_slice(key));
    cipher.decrypt(XNonce::from_slice(nonce), ct).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_tamper_detection() {
        let key = [7u8; 32];
        let msg = b"the replica snapshot bytes";
        let blob = encrypt(&key, msg);
        assert_ne!(&blob[24..], msg, "ciphertext is not the plaintext");
        assert_eq!(decrypt(&key, &blob).as_deref(), Some(&msg[..]));

        // Wrong key fails.
        assert!(decrypt(&[8u8; 32], &blob).is_none());
        // Tampered ciphertext fails the auth tag.
        let mut bad = blob.clone();
        *bad.last_mut().unwrap() ^= 0xff;
        assert!(decrypt(&key, &bad).is_none());
    }
}
