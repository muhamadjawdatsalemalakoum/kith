//! Device pairing over the wire: a new device joins a specific Space from a short code.
//!
//! Both sides run symmetric SPAKE2 ([`crate::pairing`]) from the same human code to
//! derive an ephemeral key, confirm it with the HMAC tag (constant-time, mutual), then
//! the host sends `SpaceId || group key` encrypted under that ephemeral key. A wrong
//! code yields different keys → the confirm step fails → nothing is handed out. SPAKE2
//! gives an attacker only one online guess per attempt, and the host only answers while
//! explicitly armed (one-shot) for a chosen Space.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::Result;
use iroh::endpoint::{Connection, RecvStream, SendStream};
use iroh::protocol::{AcceptError, ProtocolHandler};
use subtle::ConstantTimeEq;

use crate::space::{SpaceId, SpaceRegistry};
use crate::{atrest, pairing};

/// ALPN for the pairing protocol.
pub const PAIR_ALPN: &[u8] = b"kith/pair/1";

const MAX_FRAME: usize = 64 * 1024;
/// `SpaceId(32) || group_key(32)`, the payload handed to a joiner.
const HANDOFF_LEN: usize = 64;

/// What the host is currently offering: a short code and which Space it joins.
#[derive(Clone)]
pub struct ArmedPairing {
    pub code: Vec<u8>,
    pub space_id: SpaceId,
}

/// Host side: answers a pairing attempt while armed, handing out a Space's id + group
/// key. One global armed slot (the human arms one Space at a time).
#[derive(Clone)]
pub struct PairingDispatcher {
    /// The currently-armed pairing, or `None` when not in pairing mode.
    pub armed: Arc<Mutex<Option<ArmedPairing>>>,
    /// All Spaces this device runs — the armed Space's group key is looked up here.
    pub registry: Arc<SpaceRegistry>,
    /// Set to the joiner's endpoint id after a successful pairing (for the app to show
    /// + persist; the engine has already peered with it in the armed Space).
    pub joined: Arc<Mutex<Option<String>>>,
    /// Wakes the sync loop when the new peer is added.
    pub changed: Arc<tokio::sync::Notify>,
}

// Redacted Debug (iroh's ProtocolHandler requires Debug) — never log key material.
impl std::fmt::Debug for PairingDispatcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PairingDispatcher { .. }")
    }
}

impl ProtocolHandler for PairingDispatcher {
    async fn accept(&self, conn: Connection) -> std::result::Result<(), AcceptError> {
        self.run(conn)
            .await
            .map_err(|e| AcceptError::from_err(std::io::Error::other(e.to_string())))
    }
}

impl PairingDispatcher {
    async fn run(&self, conn: Connection) -> Result<()> {
        let armed = self.armed.lock().expect("armed lock").clone();
        let Some(armed) = armed else {
            anyhow::bail!("not in pairing mode");
        };
        let Some(space) = self.registry.get(&armed.space_id) else {
            anyhow::bail!("armed Space no longer exists");
        };

        let (mut send, mut recv) = conn.accept_bi().await?;
        let p = pairing::start(&armed.code);
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

        // Hand over `SpaceId || group key`, encrypted under the confirmed ephemeral key,
        // so the joiner adopts THIS Space (not just a bare key).
        let mut payload = Vec::with_capacity(HANDOFF_LEN);
        payload.extend_from_slice(armed.space_id.as_bytes());
        payload.extend_from_slice(&space.group_key());
        let blob = atrest::encrypt(&key, &payload);
        write_frame(&mut send, &blob).await?;

        // Learn the joiner's endpoint id (best-effort, time-boxed) and peer back with it
        // in the armed Space. Without this only the joiner would dial the host, so a
        // headless host — or one whose joiner is offline — would never converge.
        if let Ok(Ok(frame)) =
            tokio::time::timeout(Duration::from_secs(10), read_frame(&mut recv)).await
        {
            if let Ok(id) = String::from_utf8(frame) {
                let id = id.trim().to_string();
                if !id.is_empty() {
                    *self.joined.lock().expect("joined lock") = Some(id.clone());
                    if let Ok(addr) = crate::endpoint_addr_from_id(&id) {
                        if space.add_peer(addr).await {
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

/// Joiner side: connect to `host`, run pairing from `code`, return the `(SpaceId, group
/// key)` to adopt. The caller persists it (and restarts) to join that Space.
pub async fn join(
    endpoint: &iroh::Endpoint,
    host: iroh::EndpointAddr,
    code: &[u8],
) -> Result<(SpaceId, [u8; 32])> {
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
    let payload = atrest::decrypt(&key, &blob)
        .ok_or_else(|| anyhow::anyhow!("could not decrypt Space handoff"))?;
    if payload.len() != HANDOFF_LEN {
        anyhow::bail!("bad Space handoff length");
    }
    let mut id_bytes = [0u8; 32];
    id_bytes.copy_from_slice(&payload[..32]);
    let mut gk = [0u8; 32];
    gk.copy_from_slice(&payload[32..]);
    let space_id = SpaceId::from_bytes(id_bytes);

    // Tell the host our endpoint id so it can peer back with us, then let it close.
    let my_id = endpoint.id().to_string();
    let _ = write_frame(&mut send, my_id.as_bytes()).await;
    let _ = send.finish();
    conn.closed().await;
    Ok((space_id, gk))
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
