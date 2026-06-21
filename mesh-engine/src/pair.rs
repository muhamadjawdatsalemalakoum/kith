//! Device pairing over the wire: a new device joins the group from a short code.
//!
//! Both sides run symmetric SPAKE2 ([`crate::pairing`]) from the same human code to
//! derive an ephemeral key, confirm it with the HMAC tag (constant-time, mutual),
//! then the host sends the GROUP KEY encrypted under that ephemeral key. A wrong code
//! yields different keys → the confirm step fails → no key is handed out. SPAKE2 gives
//! an attacker only one online guess per attempt, and the host only answers while
//! explicitly armed (one-shot).

use std::sync::{Arc, Mutex};

use anyhow::Result;
use iroh::endpoint::{Connection, RecvStream, SendStream};
use iroh::protocol::{AcceptError, ProtocolHandler};
use subtle::ConstantTimeEq;

use crate::{atrest, pairing};

/// ALPN for the pairing protocol.
pub const PAIR_ALPN: &[u8] = b"kith/pair/1";

const MAX_FRAME: usize = 64 * 1024;

/// Host side: answers a pairing attempt while armed, handing out the group key.
#[derive(Clone)]
pub struct PairingHandler {
    /// The currently-armed short code, or `None` when not in pairing mode.
    pub armed: Arc<Mutex<Option<Vec<u8>>>>,
    /// The group key this device hands out on a successful pairing.
    pub group_key: [u8; 32],
}

// Redacted Debug (iroh's ProtocolHandler requires Debug) — never log key material.
impl std::fmt::Debug for PairingHandler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PairingHandler { .. }")
    }
}

impl ProtocolHandler for PairingHandler {
    async fn accept(&self, conn: Connection) -> std::result::Result<(), AcceptError> {
        self.run(conn)
            .await
            .map_err(|e| AcceptError::from_err(std::io::Error::other(e.to_string())))
    }
}

impl PairingHandler {
    async fn run(&self, conn: Connection) -> Result<()> {
        let code = self.armed.lock().expect("armed lock").clone();
        let Some(code) = code else {
            anyhow::bail!("not in pairing mode");
        };

        let (mut send, mut recv) = conn.accept_bi().await?;
        let p = pairing::start(&code);
        let their_msg = read_frame(&mut recv).await?;
        write_frame(&mut send, &p.outbound).await?;
        let key = pairing::finish(p, &their_msg).map_err(|_| anyhow::anyhow!("pairing finish"))?;

        // Mutual key confirmation (constant-time). Mismatch == wrong code.
        let our_tag = pairing::confirm_tag(&key);
        write_frame(&mut send, &our_tag).await?;
        let their_tag = read_frame(&mut recv).await?;
        if their_tag.len() != 32 || !bool::from(their_tag.as_slice().ct_eq(&our_tag)) {
            anyhow::bail!("pairing code mismatch");
        }

        // Hand over the group key, encrypted under the confirmed ephemeral key.
        let blob = atrest::encrypt(&key, &self.group_key);
        write_frame(&mut send, &blob).await?;
        let _ = send.finish();

        // One-shot: disarm after a successful pairing.
        *self.armed.lock().expect("armed lock") = None;
        // Keep the connection open until the joiner has read the blob + closed.
        conn.closed().await;
        Ok(())
    }
}

/// Joiner side: connect to `host`, run pairing from `code`, return the group key to
/// adopt. The caller persists it (and restarts) to join the group.
pub async fn join(
    endpoint: &iroh::Endpoint,
    host: iroh::EndpointAddr,
    code: &[u8],
) -> Result<[u8; 32]> {
    let conn = endpoint
        .connect(host, PAIR_ALPN)
        .await
        .map_err(|e| anyhow::anyhow!("pairing connect: {e}"))?;
    let (mut send, mut recv) = conn.open_bi().await?;

    let p = pairing::start(code);
    write_frame(&mut send, &p.outbound).await?;
    let their_msg = read_frame(&mut recv).await?;
    let key = pairing::finish(p, &their_msg).map_err(|_| anyhow::anyhow!("pairing finish"))?;

    let our_tag = pairing::confirm_tag(&key);
    let their_tag = read_frame(&mut recv).await?;
    if their_tag.len() != 32 || !bool::from(their_tag.as_slice().ct_eq(&our_tag)) {
        anyhow::bail!("pairing code mismatch");
    }
    write_frame(&mut send, &our_tag).await?;

    let blob = read_frame(&mut recv).await?;
    let gk = atrest::decrypt(&key, &blob)
        .ok_or_else(|| anyhow::anyhow!("could not decrypt group key"))?;
    if gk.len() != 32 {
        anyhow::bail!("bad group key length");
    }
    let mut out = [0u8; 32];
    out.copy_from_slice(&gk);
    conn.close(0u32.into(), b"paired");
    Ok(out)
}

async fn write_frame(send: &mut SendStream, data: &[u8]) -> Result<()> {
    send.write_all(&(data.len() as u32).to_le_bytes()).await?;
    send.write_all(data).await?;
    Ok(())
}

async fn read_frame(recv: &mut RecvStream) -> Result<Vec<u8>> {
    let mut len = [0u8; 4];
    recv.read_exact(&mut len).await?;
    let n = u32::from_le_bytes(len) as usize;
    if n > MAX_FRAME {
        anyhow::bail!("pairing frame too large");
    }
    let mut buf = vec![0u8; n];
    recv.read_exact(&mut buf).await?;
    Ok(buf)
}
