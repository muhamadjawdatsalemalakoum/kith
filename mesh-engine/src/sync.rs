//! Automerge-over-iroh sync seam: the [`ProtocolHandler`] (inbound / responder)
//! plus the per-peer sync loop driven over a QUIC bi-stream in both initiator and
//! responder roles.
//!
//! Ported from n0-computer/iroh-examples/iroh-automerge (`src/protocol.rs`),
//! re-targeted to automerge 0.10 (the `sync` API is unchanged from 0.7) and iroh
//! 1.0. The error glue follows Dropwire's `control.rs` (`AcceptError::from_err`).
//!
//! This is a per-CONNECTION seam. The flat-mesh layer on top (peer set,
//! re-sync-on-change, membership) drives [`MeshSync::initiate_sync`] against each
//! known peer; see `lib.rs::Mesh`.

use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use automerge::sync::{self, SyncDoc};
use automerge::Automerge;
use hmac::{Hmac, Mac};
use iroh::endpoint::{Connection, RecvStream, SendStream};
use iroh::protocol::{AcceptError, ProtocolHandler};
use sha2::Sha256;
use subtle::ConstantTimeEq;
use tokio::sync::{mpsc, Mutex};

use crate::space::{SpaceId, SpaceRegistry};

type HmacSha256 = Hmac<Sha256>;

/// Bytes of the [`SpaceId`] that prefix every inbound sync stream, so the dispatcher
/// can route a connection to the right Space before any auth or merge.
const SPACE_ID_LEN: usize = 32;

/// Hard cap on a single sync frame — defends against a malicious/huge length prefix
/// (an attacker-controlled `u64` length would otherwise allocate unboundedly = OOM).
const MAX_FRAME: usize = 8 * 1024 * 1024;
/// Per-read timeout so a stalled / half-open stream errors instead of hanging the
/// doc-merge path forever.
const READ_TIMEOUT: Duration = Duration::from_secs(30);

// --- Group-key auth handshake (proves both peers hold the shared group key before
// any document data is exchanged; neither side reveals the key) ---
const AUTH_LABEL: &[u8] = b"kith mesh auth v1";
const NONCE_LEN: usize = 16;
const PROOF_LEN: usize = 32;

/// HMAC-SHA256(group_key, label || nonce) — the proof that one holds the group key.
fn compute_proof(group_key: &[u8; 32], nonce: &[u8]) -> [u8; PROOF_LEN] {
    let mut mac = <HmacSha256 as Mac>::new_from_slice(group_key).expect("hmac accepts any key");
    mac.update(AUTH_LABEL);
    mac.update(nonce);
    let tag = mac.finalize().into_bytes();
    let mut out = [0u8; PROOF_LEN];
    out.copy_from_slice(&tag);
    out
}

/// A fresh random nonce (from iroh's CSPRNG — no extra RNG dependency).
fn random_nonce() -> [u8; NONCE_LEN] {
    let bytes = iroh::SecretKey::generate().to_bytes();
    let mut n = [0u8; NONCE_LEN];
    n.copy_from_slice(&bytes[..NONCE_LEN]);
    n
}

/// Read exactly `buf.len()` bytes, time-boxed.
async fn read_exact_timed(recv: &mut RecvStream, buf: &mut [u8]) -> Result<()> {
    match tokio::time::timeout(READ_TIMEOUT, recv.read_exact(buf)).await {
        Err(_) => anyhow::bail!("auth handshake stalled"),
        Ok(r) => {
            r?;
            Ok(())
        }
    }
}

/// Holds the shared live doc and an out-channel that republishes the merged doc to
/// the rest of the app after each completed sync.
#[derive(Debug, Clone)]
pub struct MeshSync {
    inner: Arc<Mutex<Automerge>>,
    sync_finished: mpsc::Sender<Automerge>,
    /// The shared group secret. Both peers must prove possession before any sync.
    group_key: [u8; 32],
    /// Which Space this handler syncs. The initiator writes it first on the wire; the
    /// dispatcher reads it to select this handler in the first place.
    space_id: SpaceId,
}

impl MeshSync {
    /// Build a handler that shares an EXISTING doc handle, so inbound sync and the
    /// device's primary doc are the same live document, bound to one Space.
    pub fn with_shared(
        inner: Arc<Mutex<Automerge>>,
        group_key: [u8; 32],
        space_id: SpaceId,
        sync_finished: mpsc::Sender<Automerge>,
    ) -> Arc<Self> {
        Arc::new(Self {
            inner,
            sync_finished,
            group_key,
            space_id,
        })
    }

    /// Initiator side of the auth handshake. `Err` if the peer can't prove the group key.
    async fn auth_initiator(&self, send: &mut SendStream, recv: &mut RecvStream) -> Result<()> {
        let nonce_i = random_nonce();
        send.write_all(&nonce_i).await?;
        let mut nonce_r = [0u8; NONCE_LEN];
        read_exact_timed(recv, &mut nonce_r).await?;
        send.write_all(&compute_proof(&self.group_key, &nonce_r))
            .await?;
        let mut proof_r = [0u8; PROOF_LEN];
        read_exact_timed(recv, &mut proof_r).await?;
        let expected = compute_proof(&self.group_key, &nonce_i);
        if bool::from(proof_r.as_slice().ct_eq(expected.as_slice())) {
            Ok(())
        } else {
            anyhow::bail!("peer failed group authentication")
        }
    }

    /// Responder side of the auth handshake. `Err` (no merge) if the peer fails.
    async fn auth_responder(&self, send: &mut SendStream, recv: &mut RecvStream) -> Result<()> {
        let mut nonce_i = [0u8; NONCE_LEN];
        read_exact_timed(recv, &mut nonce_i).await?;
        let nonce_r = random_nonce();
        send.write_all(&nonce_r).await?;
        let mut proof_i = [0u8; PROOF_LEN];
        read_exact_timed(recv, &mut proof_i).await?;
        let expected = compute_proof(&self.group_key, &nonce_r);
        if !bool::from(proof_i.as_slice().ct_eq(expected.as_slice())) {
            anyhow::bail!("peer failed group authentication");
        }
        send.write_all(&compute_proof(&self.group_key, &nonce_i))
            .await?;
        Ok(())
    }

    /// Snapshot the shared doc for a sync session. `fork()` mints a NEW random
    /// actor — fine, this copy is transport-only and merged back in.
    pub async fn fork_doc(&self) -> Automerge {
        self.inner.lock().await.fork()
    }

    /// Fold received changes back into the shared doc.
    pub async fn merge_doc(&self, doc: &mut Automerge) -> Result<()> {
        let mut automerge = self.inner.lock().await;
        // 0.10: merge() -> Result<Vec<ChangeHash>>; `?` discards the applied hashes.
        automerge.merge(doc)?;
        Ok(())
    }

    /// Length-prefixed write of one sync message (or a 0-length "nothing" marker).
    async fn send_msg(msg: Option<sync::Message>, send: &mut SendStream) -> Result<()> {
        if let Some(msg) = msg {
            let encoded = msg.encode();
            send.write_all(&(encoded.len() as u64).to_le_bytes())
                .await?;
            send.write_all(&encoded).await?;
        } else {
            send.write_all(&0u64.to_le_bytes()).await?;
        }
        Ok(())
    }

    /// Read one length-prefixed sync message; a 0-length frame means "nothing".
    /// Frames are size-capped and reads are time-boxed so a malicious or stalled
    /// peer can't OOM us or hang the merge path.
    async fn recv_msg(recv: &mut RecvStream) -> Result<Option<sync::Message>> {
        let mut incoming_len = [0u8; 8];
        match tokio::time::timeout(READ_TIMEOUT, recv.read_exact(&mut incoming_len)).await {
            Err(_) => anyhow::bail!("sync stream stalled reading length"),
            Ok(r) => r?,
        };
        let len = u64::from_le_bytes(incoming_len);
        if len == 0 {
            return Ok(None);
        }
        if len as usize > MAX_FRAME {
            anyhow::bail!("sync frame too large: {len} bytes (max {MAX_FRAME})");
        }
        let mut buffer = vec![0u8; len as usize];
        match tokio::time::timeout(READ_TIMEOUT, recv.read_exact(&mut buffer)).await {
            Err(_) => anyhow::bail!("sync stream stalled reading body"),
            Ok(r) => r?,
        };
        Ok(Some(sync::Message::decode(&buffer)?))
    }

    /// Initiator (dialer): opens the bi-stream, names the Space, then authenticates.
    /// Order: send SpaceId -> auth -> generate -> send -> recv.
    pub async fn initiate_sync(self: Arc<Self>, conn: Connection) -> Result<()> {
        let (mut send, mut recv) = conn.open_bi().await?;
        // Name the Space first so the responder routes to the right replica.
        send.write_all(self.space_id.as_bytes()).await?;
        self.auth_initiator(&mut send, &mut recv).await?; // prove group membership first
        let mut doc = self.fork_doc().await;
        let mut state = sync::State::new();
        loop {
            let our_msg = doc.generate_sync_message(&mut state);
            let is_local_done = our_msg.is_none();
            Self::send_msg(our_msg, &mut send).await?;

            let their_msg = Self::recv_msg(&mut recv).await?;
            let is_remote_done = their_msg.is_none();
            if let Some(m) = their_msg {
                doc.receive_sync_message(&mut state, m)?;
                self.merge_doc(&mut doc).await?;
            }
            if is_remote_done && is_local_done {
                break;
            }
        }
        conn.close(0u32.into(), b"bye");
        let _ = self.sync_finished.send(self.fork_doc().await).await;
        Ok(())
    }

    /// Responder (accepter): the dispatcher has already accepted the bi-stream and
    /// consumed the SpaceId prefix to select this handler; we take the streams from here.
    /// Order: recv -> generate -> send (mirror of the initiator — that asymmetry is what
    /// makes both-done converge).
    pub async fn respond_streams(
        &self,
        conn: Connection,
        mut send: SendStream,
        mut recv: RecvStream,
    ) -> Result<()> {
        self.auth_responder(&mut send, &mut recv).await?; // reject non-members (no merge)
        let mut doc = self.fork_doc().await;
        let mut state = sync::State::new();
        loop {
            let their_msg = Self::recv_msg(&mut recv).await?;
            let is_remote_done = their_msg.is_none();
            if let Some(m) = their_msg {
                doc.receive_sync_message(&mut state, m)?;
                self.merge_doc(&mut doc).await?;
            }
            let our_msg = doc.generate_sync_message(&mut state);
            let is_local_done = our_msg.is_none();
            Self::send_msg(our_msg, &mut send).await?;
            if is_remote_done && is_local_done {
                break;
            }
        }
        let _ = self.sync_finished.send(self.fork_doc().await).await;
        // Responder lets the initiator close the connection.
        conn.closed().await;
        Ok(())
    }
}

/// Read the [`SpaceId`] that prefixes an inbound sync stream, time-boxed.
async fn read_space_id(recv: &mut RecvStream) -> Result<SpaceId> {
    let mut buf = [0u8; SPACE_ID_LEN];
    read_exact_timed(recv, &mut buf).await?;
    Ok(SpaceId::from_bytes(buf))
}

/// The single MESH_ALPN accept handler. It reads the SpaceId off the first stream,
/// looks up the matching Space, and runs that Space's auth + sync against that Space's
/// replica — so one endpoint multiplexes every Space the device is in. A connection
/// naming a Space this device isn't in is refused before any auth or merge.
#[derive(Clone)]
pub struct SyncDispatcher {
    registry: Arc<SpaceRegistry>,
}

impl SyncDispatcher {
    pub fn new(registry: Arc<SpaceRegistry>) -> Self {
        Self { registry }
    }
}

// Redacted Debug (ProtocolHandler requires Debug) — never expose internals.
impl std::fmt::Debug for SyncDispatcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SyncDispatcher { .. }")
    }
}

/// `AcceptError::from_err` wants a sized `Error`; iroh's connection errors and
/// `anyhow::Error` aren't directly convertible, so carry the message through a std
/// `io::Error` (sized, `Error + Send + Sync`).
pub(crate) fn acc_err(e: impl std::fmt::Display) -> AcceptError {
    AcceptError::from_err(std::io::Error::other(e.to_string()))
}

impl ProtocolHandler for SyncDispatcher {
    async fn accept(&self, conn: Connection) -> std::result::Result<(), AcceptError> {
        let (send, mut recv) = conn.accept_bi().await.map_err(acc_err)?;
        let space_id = read_space_id(&mut recv).await.map_err(acc_err)?;
        let Some(space) = self.registry.get(&space_id) else {
            // Not a Space this device is in — refuse before any auth/merge.
            return Err(acc_err("unknown space"));
        };
        // Role-enforced Spaces use the verified path (EndpointId gate + per-change
        // signature verification); permissive Spaces use the plain Automerge sync.
        if space.enforced() {
            space
                .respond_verified(conn, send, recv)
                .await
                .map_err(acc_err)?;
        } else {
            space
                .sync()
                .respond_streams(conn, send, recv)
                .await
                .map_err(acc_err)?;
        }
        Ok(())
    }
}
