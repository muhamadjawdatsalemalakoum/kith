//! Connection-level group-key proof handshake, used to gate the blob ALPN the same
//! way [`crate::sync`] gates document sync: both peers prove possession of the shared
//! group key (mutual HMAC over exchanged nonces) before any bytes flow, and neither
//! side ever reveals the key. A distinct domain label keeps this separate from the
//! sync handshake.

use std::time::Duration;

use anyhow::Result;
use hmac::{Hmac, Mac};
use iroh::endpoint::{RecvStream, SendStream};
use sha2::Sha256;
use subtle::ConstantTimeEq;

type HmacSha256 = Hmac<Sha256>;

const LABEL: &[u8] = b"kith blob auth v1";
const NONCE_LEN: usize = 16;
const PROOF_LEN: usize = 32;
const READ_TIMEOUT: Duration = Duration::from_secs(30);

/// HMAC-SHA256(group_key, label || nonce) — the proof that one holds the group key.
fn proof(group_key: &[u8; 32], nonce: &[u8]) -> [u8; PROOF_LEN] {
    let mut mac = <HmacSha256 as Mac>::new_from_slice(group_key).expect("hmac accepts any key");
    mac.update(LABEL);
    mac.update(nonce);
    let tag = mac.finalize().into_bytes();
    let mut out = [0u8; PROOF_LEN];
    out.copy_from_slice(&tag);
    out
}

/// A fresh random nonce from iroh's CSPRNG (no extra RNG dependency).
fn random_nonce() -> [u8; NONCE_LEN] {
    let bytes = iroh::SecretKey::generate().to_bytes();
    let mut n = [0u8; NONCE_LEN];
    n.copy_from_slice(&bytes[..NONCE_LEN]);
    n
}

/// Read exactly `buf.len()` bytes, time-boxed. Shared with the blob dispatcher so the
/// SpaceId prefix and the auth handshake use the same bounded read.
pub(crate) async fn read_exact_timed(recv: &mut RecvStream, buf: &mut [u8]) -> Result<()> {
    match tokio::time::timeout(READ_TIMEOUT, recv.read_exact(buf)).await {
        Err(_) => anyhow::bail!("blob auth handshake stalled"),
        Ok(r) => {
            r?;
            Ok(())
        }
    }
}

/// Initiator (dialer) side: prove the group key and verify the peer's proof.
/// `Err` if the peer can't prove the shared key.
pub async fn initiator(
    group_key: &[u8; 32],
    send: &mut SendStream,
    recv: &mut RecvStream,
) -> Result<()> {
    let nonce_i = random_nonce();
    send.write_all(&nonce_i).await?;
    let mut nonce_r = [0u8; NONCE_LEN];
    read_exact_timed(recv, &mut nonce_r).await?;
    send.write_all(&proof(group_key, &nonce_r)).await?;
    let mut proof_r = [0u8; PROOF_LEN];
    read_exact_timed(recv, &mut proof_r).await?;
    let expected = proof(group_key, &nonce_i);
    if bool::from(proof_r.as_slice().ct_eq(expected.as_slice())) {
        Ok(())
    } else {
        anyhow::bail!("peer failed group authentication")
    }
}

/// Responder (accepter) side: verify the peer's proof and prove the key back.
/// `Err` (serve nothing) if the peer fails.
pub async fn responder(
    group_key: &[u8; 32],
    send: &mut SendStream,
    recv: &mut RecvStream,
) -> Result<()> {
    let mut nonce_i = [0u8; NONCE_LEN];
    read_exact_timed(recv, &mut nonce_i).await?;
    let nonce_r = random_nonce();
    send.write_all(&nonce_r).await?;
    let mut proof_i = [0u8; PROOF_LEN];
    read_exact_timed(recv, &mut proof_i).await?;
    let expected = proof(group_key, &nonce_r);
    if !bool::from(proof_i.as_slice().ct_eq(expected.as_slice())) {
        anyhow::bail!("peer failed group authentication");
    }
    send.write_all(&proof(group_key, &nonce_i)).await?;
    Ok(())
}
