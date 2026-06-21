//! Device pairing over the wire: a new device joins the group from a short code.
//!
//! Both sides run symmetric SPAKE2 ([`crate::pairing`]) from the same human code to
//! derive an ephemeral key, confirm it with the HMAC tag (constant-time, mutual),
//! then the host sends the GROUP KEY encrypted under that ephemeral key. A wrong code
//! yields different keys → the confirm step fails → no key is handed out. SPAKE2 gives
//! an attacker only one online guess per attempt, and the host only answers while
//! explicitly armed (one-shot).

use std::sync::{Arc, Mutex};
use std::time::Duration;

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
    /// Set to the joiner's endpoint id after a successful pairing (for the app to
    /// show + persist; the engine has already peered with it).
    pub joined: Arc<Mutex<Option<String>>>,
    /// Shared peer set — a successful pairing peers the host back with the joiner so
    /// both sides converge (not just the joiner dialing the host).
    pub peers: Arc<tokio::sync::Mutex<Vec<iroh::EndpointAddr>>>,
    /// Wakes the sync loop when the new peer is added.
    pub changed: Arc<tokio::sync::Notify>,
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

        // Learn the joiner's endpoint id (best-effort, time-boxed) and peer back with
        // it. Without this only the joiner would dial the host, so a headless host or
        // one whose joiner is offline would never converge — the core sync promise.
        if let Ok(Ok(frame)) =
            tokio::time::timeout(Duration::from_secs(10), read_frame(&mut recv)).await
        {
            if let Ok(id) = String::from_utf8(frame) {
                let id = id.trim().to_string();
                if !id.is_empty() {
                    *self.joined.lock().expect("joined lock") = Some(id.clone());
                    if let Ok(addr) = crate::endpoint_addr_from_id(&id) {
                        let mut peers = self.peers.lock().await;
                        if !peers.iter().any(|p| p.id == addr.id) {
                            peers.push(addr);
                            self.changed.notify_waiters();
                        }
                    }
                }
            }
        }

        let _ = send.finish();
        // One-shot: disarm after a successful pairing.
        *self.armed.lock().expect("armed lock") = None;
        // The host closes once it has the joiner's id; the joiner waits on close.
        conn.close(0u32.into(), b"paired");
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
    // Tell the host our endpoint id so it can peer back with us, then let it close.
    let my_id = endpoint.id().to_string();
    let _ = write_frame(&mut send, my_id.as_bytes()).await;
    let _ = send.finish();
    conn.closed().await;
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
