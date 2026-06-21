//! Device pairing via symmetric SPAKE2: two devices that share a short
//! out-of-band code derive an identical 32-byte group key. The flat mesh has no
//! client/server role, so the symmetric variant is exactly right. The "code" is
//! the family's account-free sign-in.
//!
//! SECURITY (read before trusting this):
//! - `spake2` 0.4.0 is UNAUDITED and not constant-time (per its own docs).
//! - [`finish`] does NOT error on a code mismatch / MITM — it silently yields
//!   DIFFERENT keys per side. So the key-confirmation round ([`confirm_tag`]) is
//!   MANDATORY before trusting the channel: each side computes the tag over its
//!   derived key and exchanges it; equal tags prove both derived the same key.

use hmac::{Hmac, Mac};
use sha2::Sha256;
use spake2::{Ed25519Group, Identity, Password, Spake2};

type HmacSha256 = Hmac<Sha256>;

/// Domain-separation label mixed into every pairing.
const PAIRING_ID: &[u8] = b"kith mesh pairing v1";

/// In-progress pairing state plus this side's outbound message.
pub struct Pairing {
    state: Spake2<Ed25519Group>,
    /// Send this to the peer over the transport (e.g. an iroh QUIC stream).
    pub outbound: Vec<u8>,
}

/// Start a symmetric pairing from the shared short code. Both peers run this
/// identical call; the outbound message is minted at construction.
pub fn start(short_code: &[u8]) -> Pairing {
    // `start_symmetric` is gated behind the default `getrandom` feature.
    let (state, outbound) = Spake2::<Ed25519Group>::start_symmetric(
        &Password::new(short_code),
        &Identity::new(PAIRING_ID),
    );
    Pairing { state, outbound }
}

/// Complete pairing with the peer's inbound message. CONSUMES the state and
/// returns the shared 32-byte key. `Err` only on a malformed inbound message —
/// NOT on a code mismatch (that yields a different key; see [`confirm_tag`]).
pub fn finish(pairing: Pairing, inbound: &[u8]) -> Result<[u8; 32], spake2::Error> {
    let key = pairing.state.finish(inbound)?;
    let mut out = [0u8; 32];
    out.copy_from_slice(&key);
    Ok(out)
}

/// Key-confirmation tag: an HMAC-SHA256 over a domain label, keyed by the derived
/// group key. Each side computes and exchanges it; equal tags prove both sides
/// derived the SAME key (codes matched, no MITM). Required because [`finish`]
/// cannot detect a mismatch.
pub fn confirm_tag(key: &[u8; 32]) -> [u8; 32] {
    let mut mac =
        <HmacSha256 as Mac>::new_from_slice(key).expect("HMAC accepts a key of any length");
    mac.update(b"kith confirm v1");
    let out = mac.finalize().into_bytes();
    let mut tag = [0u8; 32];
    tag.copy_from_slice(&out);
    tag
}
