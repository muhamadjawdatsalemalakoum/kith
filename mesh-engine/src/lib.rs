//! # mesh-engine — the serverless P2P substrate
//!
//! The shared engine the whole family runs on: a flat peer-to-peer mesh where every
//! device is an equal peer holding a full, end-to-end-encrypted replica of a small
//! mutable [Automerge] document, synced directly between a user's own devices over
//! [iroh] QUIC (mainline-DHT discovery, relays only as fallback). There is no hub and
//! no account.
//!
//! ## Spaces
//! A device runs **N independent encrypted Spaces** concurrently ([`SpaceId`]). Each
//! Space is its own private network — its own group key, at-rest key, replica, blob
//! store, peers, and data subdir — but they all share the one [iroh] endpoint and the
//! one device identity. Edits in Space A converge only to A's members and never leak
//! into B. Every device has a **default Space** (a well-known constant id) so the
//! single-group behaviour the engine had before Spaces still holds; the public methods
//! on [`Mesh`] (e.g. [`Mesh::doc`], [`Mesh::add_peer`]) operate on the **active Space**
//! (the default until changed), while [`Mesh::space`] / [`Mesh::create_space`] /
//! [`Mesh::list_spaces`] address Spaces explicitly.
//!
//! Apps are thin: centralTabs (tabs), agent-memory, Dropwire-on-mesh, an MCP app —
//! each brings its own data model + UX and runs on this one substrate. This crate is
//! the *only* place that depends on `iroh` / `automerge`; apps speak its types — and
//! the re-exported [`automerge`] — so the whole family shares exactly one CRDT version.
//!
//! [Automerge]: https://automerge.org
//! [iroh]: https://iroh.computer

mod atrest;
mod auth;
mod blobs;
mod config;
mod doc;
mod endpoint;
mod epoch;
mod error;
mod export;
mod identity;
mod keychain;
mod keys;
mod membership;
mod pair;
pub mod pairing;
mod space;
mod sync;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use automerge::ActorId;
use iroh::protocol::Router;
use iroh::EndpointId;
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

// The engine's public vocabulary. Apps build their data models on the re-exported
// `automerge` (one CRDT version family-wide), move files via the blob primitive keyed
// by `Hash`, and address peers with `EndpointAddr`.
pub use automerge;
pub use automerge::Automerge;
pub use iroh::EndpointAddr;
pub use iroh_blobs::Hash;

pub use config::{CoreConfig, Infra, KeyStore};
pub use doc::SharedDoc;
pub use error::{CoreError, Result};
pub use membership::{AuditEntry, Role};
pub use space::{SpaceId, SpaceInfo};
pub use sync::MeshSync;

use pair::ArmedPairing;
use space::{MemberOp, SpaceRegistry, SpaceState};

/// ALPN for the engine's mesh sync protocol.
pub const MESH_ALPN: &[u8] = b"mesh-engine/sync/1";

/// How often the background loop re-syncs with known peers (also wakes early on a
/// local change or a newly added peer).
const SYNC_INTERVAL: Duration = Duration::from_millis(1500);
/// Bound on dialing a peer before treating it as unreachable (so a dead peer can't
/// hang the sync loop or a `fetch`).
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// Bound on a single sync round once connected.
const SYNC_ROUND_TIMEOUT: Duration = Duration::from_secs(30);

/// Shared engine state, held behind an `Arc` so the background sync loop, the accept
/// dispatchers, and the public handle share one registry of Spaces and one router.
struct Inner {
    router: Router,
    /// Every Space this device runs, looked up by id on inbound connections and by the
    /// public API. Shared with the sync/blob/pairing dispatchers.
    registry: Arc<SpaceRegistry>,
    /// Root application data directory (`node.key` + `spaces/<id>/…`).
    data_dir: PathBuf,
    /// The global device CRDT actor (one identity per device across all Spaces).
    actor: ActorId,
    /// This device's public key bytes — the founding key for [`SpaceId::new`].
    device_pubkey: [u8; 32],
    /// This device's Ed25519 secret bytes (`node.key`), for signing changes and
    /// membership ops in enforced Spaces.
    device_secret: [u8; 32],
    /// The Space the bare [`Mesh`] methods operate on (default until changed).
    active: StdMutex<SpaceId>,
    /// Where new Spaces store their at-rest / group keys (file or OS keychain).
    key_store: KeyStore,
    /// Pinged on a local change, a new peer, or a new Space to wake the sync loop.
    changed: Arc<Notify>,
    /// The currently-armed pairing (Some while in "add a device" mode), naming a Space.
    pair_armed: Arc<StdMutex<Option<ArmedPairing>>>,
    /// Set to the endpoint id of a device that just paired in (host side). Drained by
    /// [`Mesh::take_joined`].
    pair_joined: Arc<StdMutex<Option<String>>>,
    /// Stops the background loop on shutdown.
    cancel: CancellationToken,
}

/// A running mesh peer. Cheap to clone (internally `Arc`). Apps hold this.
#[derive(Clone)]
pub struct Mesh {
    inner: Arc<Inner>,
    /// When `Some`, this handle's active-Space methods address exactly this Space and
    /// ignore [`Mesh::set_active_space`] — the structural binding behind an MCP server
    /// pinned to one Space ([`Mesh::bound_to`]). `None` follows the shared active Space.
    active_override: Option<SpaceId>,
}

impl Mesh {
    /// Start a peer: load/derive identity, restore every Space's replica, bind the one
    /// endpoint, spin up the always-on sync router (whose accept handlers route by
    /// [`SpaceId`]), and start the background loop that keeps known peers converged and
    /// persists each Space.
    pub async fn start(config: CoreConfig) -> Result<Mesh> {
        std::fs::create_dir_all(&config.data_dir)?;

        let secret = identity::load_or_create(&config.data_dir.join("node.key"))?;
        let actor = identity::actor_id(&secret);
        let device_pubkey = {
            let pk = secret.public();
            let mut a = [0u8; 32];
            a.copy_from_slice(pk.as_bytes());
            a
        };
        let device_secret = secret.to_bytes();

        // Spaces live under `data_dir/spaces/<id>/`. Migrate any pre-Spaces flat layout
        // (a single-group install) into the default Space the first time.
        let spaces_root = config.data_dir.join("spaces");
        migrate_flat_layout(&config.data_dir, &spaces_root)?;
        std::fs::create_dir_all(&spaces_root)?;

        let registry = SpaceRegistry::new();
        let default_id = SpaceId::default_space();
        let default_dir = spaces_root.join(default_id.to_hex());
        // The default Space adopts the config's explicit key (tests / a just-paired key
        // on restart) or loads/generates its own. Other Spaces load from disk.
        let default_space = SpaceState::open(
            default_dir,
            default_id,
            actor.clone(),
            device_secret,
            config.group_key,
            config.key_store,
        )
        .await?;
        registry.insert(default_space);

        if let Ok(rd) = std::fs::read_dir(&spaces_root) {
            for entry in rd.flatten() {
                if !entry.path().is_dir() {
                    continue;
                }
                let Some(name) = entry.file_name().to_str().map(str::to_string) else {
                    continue;
                };
                let Some(id) = SpaceId::parse(&name) else {
                    continue;
                };
                if id == default_id {
                    continue;
                }
                let state = SpaceState::open(
                    entry.path(),
                    id,
                    actor.clone(),
                    device_secret,
                    None,
                    config.key_store,
                )
                .await?;
                registry.insert(state);
            }
        }
        let registry = Arc::new(registry);

        let endpoint = endpoint::build(secret, &config.infra, config.enable_blobs).await?;
        let pair_armed = Arc::new(StdMutex::new(None));
        let pair_joined = Arc::new(StdMutex::new(None));
        let changed = Arc::new(Notify::new());

        // One router; its accept handlers dispatch by SpaceId. Mesh sync + pairing
        // always; blobs only when explicitly enabled.
        let mut builder = Router::builder(endpoint)
            .accept(MESH_ALPN, sync::SyncDispatcher::new(registry.clone()))
            .accept(
                pair::PAIR_ALPN,
                pair::PairingDispatcher {
                    armed: pair_armed.clone(),
                    registry: registry.clone(),
                    joined: pair_joined.clone(),
                    changed: changed.clone(),
                },
            );
        if config.enable_blobs {
            builder = builder.accept(
                iroh_blobs::ALPN,
                blobs::BlobDispatcher::new(registry.clone()),
            );
        }
        let router = builder.spawn();

        let inner = Arc::new(Inner {
            router,
            registry,
            data_dir: config.data_dir.clone(),
            actor,
            device_pubkey,
            device_secret,
            active: StdMutex::new(default_id),
            key_store: config.key_store,
            changed,
            pair_armed,
            pair_joined,
            cancel: CancellationToken::new(),
        });

        tokio::spawn(sync_loop(inner.clone()));
        Ok(Mesh {
            inner,
            active_override: None,
        })
    }

    // ---- device-global handles (one endpoint / identity across all Spaces) ----

    /// This device's stable public identity (`EndpointId`), as a string.
    pub fn endpoint_id(&self) -> String {
        self.inner.router.endpoint().id().to_string()
    }

    /// This device's connectable address (id + direct/relay transports).
    pub fn endpoint_addr(&self) -> EndpointAddr {
        self.inner.router.endpoint().addr()
    }

    /// Wait (time-boxed) for the relay handshake so [`Mesh::endpoint_addr`] includes a
    /// relay-reachable address. A near-instant no-op on direct/local-only links.
    pub async fn online(&self) {
        let _ = tokio::time::timeout(
            Duration::from_secs(10),
            self.inner.router.endpoint().online(),
        )
        .await;
    }

    // ---- Space management ----

    /// Create a brand-new, empty Space with a random public id and a fresh random group
    /// key. It is registered live and persisted under `data_dir/spaces/<id>/`. Returns
    /// the new [`SpaceId`]; pair another device into it (or [`Mesh::join_space`]) to add
    /// members.
    pub async fn create_space(&self, name: &str) -> Result<SpaceId> {
        let nonce = keys::generate();
        let id = SpaceId::new(&self.inner.device_pubkey, &nonce[..16]);
        if self.inner.registry.contains(&id) {
            return Ok(id);
        }
        let group_key = keys::generate();
        let dir = self.space_dir(id);
        std::fs::create_dir_all(&dir)?;
        keys::write_key(&dir.join("group.key"), &group_key)?;
        let _ = space::write_name(&dir, name);
        let state = SpaceState::open(
            dir,
            id,
            self.inner.actor.clone(),
            self.inner.device_secret,
            None,
            self.inner.key_store,
        )
        .await?;
        self.inner.registry.insert(state);
        self.inner.changed.notify_waiters();
        Ok(id)
    }

    /// Join an existing Space whose `(id, group_key)` you already hold (from pairing, an
    /// export, or [`Mesh::create_space`] + [`Mesh::group_key_of`] on another device).
    /// Registers it live and persists it. Idempotent if already joined.
    pub async fn join_space(&self, id: SpaceId, group_key: [u8; 32], name: &str) -> Result<()> {
        if self.inner.registry.contains(&id) {
            return Ok(());
        }
        let dir = self.space_dir(id);
        std::fs::create_dir_all(&dir)?;
        keys::write_key(&dir.join("group.key"), &group_key)?;
        let _ = space::write_name(&dir, name);
        let state = SpaceState::open(
            dir,
            id,
            self.inner.actor.clone(),
            self.inner.device_secret,
            None,
            self.inner.key_store,
        )
        .await?;
        self.inner.registry.insert(state);
        self.inner.changed.notify_waiters();
        Ok(())
    }

    /// Leave a Space: stop syncing it, close its store, and delete its local data. The
    /// default Space cannot be left (it is the always-present base). Returns whether the
    /// Space was present.
    pub async fn leave_space(&self, id: SpaceId) -> Result<bool> {
        if id.is_default() {
            return Err(CoreError::Other(anyhow::anyhow!(
                "the default space cannot be left"
            )));
        }
        let Some(state) = self.inner.registry.remove(&id) else {
            return Ok(false);
        };
        {
            let mut a = self.inner.active.lock().expect("active lock");
            if *a == id {
                *a = SpaceId::default_space();
            }
        }
        state.shutdown().await;
        let _ = std::fs::remove_dir_all(self.space_dir(id));
        // Also remove any keychain-stored keys for this Space (best-effort).
        keychain::delete(&format!("space:{}:group", id.to_hex()));
        keychain::delete(&format!("space:{}:atrest", id.to_hex()));
        // Wake the loop so it drops this Space's per-peer sync tasks.
        self.inner.changed.notify_waiters();
        Ok(true)
    }

    /// Every Space this device runs (default first).
    pub fn list_spaces(&self) -> Vec<SpaceInfo> {
        self.inner.registry.infos()
    }

    /// The Space the bare [`Mesh`] methods currently operate on (the bound Space if this
    /// handle is [`Mesh::bound_to`] one, else the shared active Space).
    pub fn active_space(&self) -> SpaceId {
        self.active_override
            .unwrap_or_else(|| *self.inner.active.lock().expect("active lock"))
    }

    /// Point the bare [`Mesh`] methods at a different Space. Returns whether the Space
    /// exists (no-op if it doesn't). No effect on a [`Mesh::bound_to`] handle, which is
    /// pinned for an MCP server's lifetime.
    pub fn set_active_space(&self, id: SpaceId) -> bool {
        if self.active_override.is_some() || !self.inner.registry.contains(&id) {
            return false;
        }
        *self.inner.active.lock().expect("active lock") = id;
        true
    }

    /// A handle **bound** to one Space for an MCP server's lifetime: its active-Space
    /// methods always address `id` and ignore [`Mesh::set_active_space`]. Combined with
    /// the fact that no MCP tool accepts a Space argument, a prompt-injected agent is
    /// structurally unable to reach another Space (the confused-deputy / wrong-tenant
    /// failure mode). `None` if `id` isn't a joined Space.
    pub fn bound_to(&self, id: SpaceId) -> Option<Mesh> {
        if !self.inner.registry.contains(&id) {
            return None;
        }
        Some(Mesh {
            inner: self.inner.clone(),
            active_override: Some(id),
        })
    }

    /// Whether this handle is bound to a single Space (an MCP server handle).
    pub fn is_bound(&self) -> bool {
        self.active_override.is_some()
    }

    /// A scoped handle to one Space, for code that addresses a Space explicitly (the
    /// GUI's Space picker, an MCP server bound to one Space). `None` if not joined.
    pub fn space(&self, id: SpaceId) -> Option<SpaceHandle> {
        self.inner.registry.get(&id).map(|space| SpaceHandle {
            inner: self.inner.clone(),
            space,
        })
    }

    /// A Space's current group key (your own secret; handed to a new device by pairing
    /// or used by [`Mesh::join_space`] on another of your devices). `None` if not joined.
    pub fn group_key_of(&self, id: SpaceId) -> Option<[u8; 32]> {
        self.inner.registry.get(&id).map(|s| s.group_key())
    }

    /// Export a Space to an **encrypted, passphrase-protected** bundle — the no-account
    /// recovery path. Archives the whole Space (replica + blobs + membership + epoch keys),
    /// injecting the group/at-rest keys even when they live in the OS keychain, then seals
    /// it with an Argon2id-stretched passphrase. Restores on a fresh device via
    /// [`Mesh::import_space`]. Losing every device without an export means the data is gone.
    pub async fn export_space(&self, id: SpaceId, passphrase: &str) -> Result<Vec<u8>> {
        let state = self
            .inner
            .registry
            .get(&id)
            .ok_or_else(|| CoreError::Other(anyhow::anyhow!("not a member of that space")))?;
        state.save().await?; // freshest snapshot on disk before archiving
        let dir = self.space_dir(id);
        let mut files = export::collect_dir(&dir)?;
        // The group key (and, for a permissive Space, the at-rest key) may live in the
        // keychain rather than the dir — inject the in-memory copies so the bundle stands
        // alone on a fresh device.
        export::upsert(&mut files, "group.key", state.group_key().to_vec());
        if !state.enforced() {
            export::upsert(&mut files, "atrest.key", state.atrest_key_bytes().to_vec());
        }
        // Blob contents are exported via the store API (the raw store dir is locked +
        // non-portable) and re-import to identical content-addressed hashes.
        let blobs = blobs::export_all(state.store()).await?;
        export::seal(&id, &files, &blobs, passphrase)
    }

    /// Import an encrypted Space bundle onto this device (recovery / migration). Returns
    /// the restored [`SpaceId`]. `Err` on a wrong passphrase, a corrupt bundle, or if the
    /// Space is already present here.
    pub async fn import_space(&self, bundle: &[u8], passphrase: &str) -> Result<SpaceId> {
        let opened = export::open(bundle, passphrase)?;
        let id = opened.id;
        if self.inner.registry.contains(&id) {
            return Err(CoreError::Other(anyhow::anyhow!(
                "that space is already on this device"
            )));
        }
        let dir = self.space_dir(id);
        if dir.exists() {
            return Err(CoreError::Other(anyhow::anyhow!(
                "space data already exists on disk"
            )));
        }
        export::extract_to(&dir, &opened.files)?;
        let state = SpaceState::open(
            dir,
            id,
            self.inner.actor.clone(),
            self.inner.device_secret,
            None,
            self.inner.key_store,
        )
        .await?;
        // Re-add the exported blob contents into this Space's fresh store.
        state.import_blobs(&opened.blobs).await?;
        self.inner.registry.insert(state);
        self.inner.changed.notify_waiters();
        Ok(id)
    }

    // ---- role-enforced Spaces (membership rooted in EndpointId) ----

    /// Create a **role-enforced** Space: like [`Mesh::create_space`] but with a signed
    /// membership log whose root **Admin** is this device, cryptographically bound to the
    /// SpaceId. Add devices with [`Mesh::add_member`]; their `Admin`/`Writer`/`Reader`
    /// roles are then enforced against honest peers (a Reader's writes are rejected).
    pub async fn create_space_with_roles(&self, name: &str) -> Result<SpaceId> {
        let nonce_full = keys::generate();
        let mut nonce = [0u8; 16];
        nonce.copy_from_slice(&nonce_full[..16]);
        let id = SpaceId::new(&self.inner.device_pubkey, &nonce);
        if self.inner.registry.contains(&id) {
            return Ok(id);
        }
        let group_key = keys::generate();
        let dir = self.space_dir(id);
        std::fs::create_dir_all(&dir)?;
        keys::write_key(&dir.join("group.key"), &group_key)?;
        let _ = space::write_name(&dir, name);
        // Genesis: this device is the self-signed root Admin, bound to the SpaceId.
        let secret = iroh::SecretKey::from_bytes(&self.inner.device_secret);
        membership::Membership::genesis(id, &secret, nonce, dir.join("members.log"))?;
        let state = SpaceState::open(
            dir,
            id,
            self.inner.actor.clone(),
            self.inner.device_secret,
            None,
            self.inner.key_store,
        )
        .await?;
        // Mint epoch 0's Admin-signed key (the revocation substrate + at-rest key).
        state.seed_genesis_epoch()?;
        self.inner.registry.insert(state);
        self.inner.changed.notify_waiters();
        Ok(id)
    }

    /// A bundle that seeds another device joining with roles: the verified membership log
    /// PLUS the Space's current epoch keys (so the joiner can derive its at-rest key and
    /// verify future rotations). Hand it alongside the group key, e.g. during pairing.
    pub fn space_join_bundle(&self, id: SpaceId) -> Option<Vec<u8>> {
        self.inner.registry.get(&id).and_then(|s| s.join_bundle())
    }

    /// Join a role-enforced Space you've been added to: persist its key, the verified
    /// membership log (which roots trust in the founder Admin), and the current epoch
    /// keys, then register it.
    pub async fn join_space_with_roles(
        &self,
        id: SpaceId,
        group_key: [u8; 32],
        join_bundle: &[u8],
        name: &str,
    ) -> Result<()> {
        if self.inner.registry.contains(&id) {
            return Ok(());
        }
        let (mem, epochs) = SpaceState::split_join_bundle(join_bundle)
            .ok_or_else(|| CoreError::Other(anyhow::anyhow!("malformed join bundle")))?;
        let dir = self.space_dir(id);
        std::fs::create_dir_all(&dir)?;
        keys::write_key(&dir.join("group.key"), &group_key)?;
        let _ = space::write_name(&dir, name);
        let tmp = dir.join("members.log.tmp");
        std::fs::write(&tmp, mem)?;
        std::fs::rename(&tmp, dir.join("members.log"))?;
        std::fs::write(dir.join("epochs.bin"), epochs)?;
        // SpaceState::open replays + verifies the seeded log; a forged/mismatched log
        // (not bound to this SpaceId) fails here rather than being trusted.
        let state = SpaceState::open(
            dir,
            id,
            self.inner.actor.clone(),
            self.inner.device_secret,
            None,
            self.inner.key_store,
        )
        .await?;
        self.inner.registry.insert(state);
        self.inner.changed.notify_waiters();
        Ok(())
    }

    /// Add a device (by endpoint-id string) to a role-enforced Space. This device must
    /// be an Admin of that Space.
    pub fn add_member(&self, id: SpaceId, endpoint: &str, role: Role) -> Result<()> {
        let ep = endpoint_bytes(endpoint)?;
        self.space_state(id)?.membership_op(MemberOp::Add(ep, role))
    }

    /// Promote/demote a member of a role-enforced Space (Admin only).
    pub fn set_member_role(&self, id: SpaceId, endpoint: &str, role: Role) -> Result<()> {
        let ep = endpoint_bytes(endpoint)?;
        self.space_state(id)?
            .membership_op(MemberOp::SetRole(ep, role))
    }

    /// Remove a device from a role-enforced Space (Admin only). This revokes it: the
    /// signed log records the removal (so honest peers refuse it at the gate) AND the
    /// epoch key is rotated, so remaining members re-key future at-rest data under a key
    /// the removed device can't obtain. Post-removal confidentiality for *future* data —
    /// not a retroactive wipe (see `SECURITY.md`).
    pub async fn remove_member(&self, id: SpaceId, endpoint: &str) -> Result<()> {
        let ep = endpoint_bytes(endpoint)?;
        let state = self.space_state(id)?;
        state.membership_op(MemberOp::Remove(ep))?;
        state.rotate_epoch().await?;
        Ok(())
    }

    /// Proactively rotate a role-enforced Space's epoch key (Admin only) without removing
    /// anyone — e.g. periodic rekeying. Remaining members converge on the new key.
    pub async fn rotate_epoch(&self, id: SpaceId) -> Result<()> {
        self.space_state(id)?.rotate_epoch().await
    }

    /// The current key epoch of a role-enforced Space (0 for a permissive Space).
    pub fn space_epoch(&self, id: SpaceId) -> u64 {
        self.inner
            .registry
            .get(&id)
            .map(|s| s.current_epoch())
            .unwrap_or(0)
    }

    /// The tamper-evident audit log of a role-enforced Space (space-created, members
    /// added/removed/role-changed, key rotations, pairings), oldest first. Empty for a
    /// permissive Space. A tampered log fails to load, so every entry here is verified.
    pub fn audit_log(&self, id: SpaceId) -> Vec<AuditEntry> {
        self.inner
            .registry
            .get(&id)
            .map(|s| s.audit_log())
            .unwrap_or_default()
    }

    /// Current members of a role-enforced Space as `(endpoint-id, role)` (empty for a
    /// permissive Space).
    pub fn members(&self, id: SpaceId) -> Vec<(String, Role)> {
        self.inner
            .registry
            .get(&id)
            .map(|s| s.members())
            .unwrap_or_default()
    }

    /// This device's role in a Space (`None` if permissive or not a member).
    pub fn my_role(&self, id: SpaceId) -> Option<Role> {
        self.inner.registry.get(&id).and_then(|s| s.my_role())
    }

    fn space_state(&self, id: SpaceId) -> Result<Arc<SpaceState>> {
        self.inner
            .registry
            .get(&id)
            .ok_or_else(|| CoreError::Other(anyhow::anyhow!("not a member of that space")))
    }

    fn space_dir(&self, id: SpaceId) -> PathBuf {
        self.inner.data_dir.join("spaces").join(id.to_hex())
    }

    /// The active Space's state — the bound Space for a [`Mesh::bound_to`] handle, else
    /// the shared active Space (the default Space is always present as a fallback).
    fn active_state(&self) -> Arc<SpaceState> {
        let id = self.active_space();
        self.inner
            .registry
            .get(&id)
            .or_else(|| self.inner.registry.get(&SpaceId::default_space()))
            .expect("a space is always present")
    }

    // ---- active-Space delegating API (unchanged surface for apps) ----

    /// This device's group key for the active Space.
    pub fn group_key(&self) -> [u8; 32] {
        self.active_state().group_key()
    }

    /// Enter "add a device" mode for the active Space: answer ONE pairing attempt
    /// presenting `code`, then disarm. Run this on the device the user is joining FROM.
    pub fn arm_pairing(&self, code: &[u8]) {
        *self.inner.pair_joined.lock().expect("joined lock") = None;
        *self.inner.pair_armed.lock().expect("armed lock") = Some(ArmedPairing {
            code: code.to_vec(),
            space_id: self.active_space(),
        });
    }

    /// Leave "add a device" mode without completing a pairing. Safe when not armed.
    pub fn disarm_pairing(&self) {
        *self.inner.pair_armed.lock().expect("armed lock") = None;
    }

    /// Take the endpoint id of a device that just paired in (host side), clearing it.
    /// The engine has already added it as a sync peer; this is for the app to persist.
    pub fn take_joined(&self) -> Option<String> {
        self.inner.pair_joined.lock().expect("joined lock").take()
    }

    /// For each peer of the active Space that has ever synced, the seconds since that
    /// last successful round (keyed by endpoint-id string).
    pub fn last_sync_ages(&self) -> HashMap<String, u64> {
        self.active_state().last_sync_ages()
    }

    /// Join the Space hosted by `host` using the shared short `code`: run the SPAKE2
    /// pairing handshake, receive the `(SpaceId, group key)`, and persist it under
    /// `data_dir/spaces/<id>/`. **Restart this peer** afterwards to adopt the Space and
    /// begin syncing.
    pub async fn pair_with(&self, host: EndpointAddr, code: &[u8]) -> Result<()> {
        let (id, gk) = pair::join(self.inner.router.endpoint(), host, code)
            .await
            .map_err(|e| CoreError::Pairing(e.to_string()))?;
        let dir = self.space_dir(id);
        std::fs::create_dir_all(&dir)?;
        keys::write_key(&dir.join("group.key"), &gk)?;
        // Give a freshly-joined non-default Space a placeholder name (don't clobber the
        // default Space's name or an existing one).
        if !id.is_default() && !dir.join("name").exists() {
            let _ = space::write_name(&dir, "Linked space");
        }
        Ok(())
    }

    /// Revoke access to the active Space by rotating to a NEW group key (persisted for
    /// next start). The evicted device's old key stops authenticating. **Restart this
    /// device, then re-pair the devices you keep.** (Live epoch rotation is M3.)
    pub fn rotate_group_key(&self) -> Result<[u8; 32]> {
        let new = keys::generate();
        let dir = self.space_dir(self.active_space());
        keys::write_key(&dir.join("group.key"), &new)?;
        Ok(new)
    }

    /// Add a peer to keep the active Space converged with. The background loop syncs
    /// with it continuously (and immediately). Idempotent by endpoint id.
    pub async fn add_peer(&self, peer: EndpointAddr) {
        if self.active_state().add_peer(peer).await {
            self.inner.changed.notify_waiters();
        }
    }

    /// Stop keeping a peer converged in the active Space. Returns whether it was present.
    pub async fn remove_peer(&self, id: &str) -> bool {
        let Ok(addr) = endpoint_addr_from_id(id) else {
            return false;
        };
        let removed = self.active_state().remove_peer(addr.id).await;
        if removed {
            self.inner.changed.notify_waiters();
        }
        removed
    }

    /// Tell the engine the local replica just changed, so it syncs + persists now.
    pub fn announce_change(&self) {
        self.inner.changed.notify_waiters();
    }

    /// Dial one peer and run a single active-Space sync round now (initiator).
    pub async fn sync_with(&self, peer: impl Into<EndpointAddr>) -> Result<()> {
        sync_peer(&self.inner, &self.active_state(), peer.into()).await
    }

    /// Sync the active Space once with every known peer (best-effort).
    pub async fn sync_all(&self) -> Result<()> {
        let space = self.active_state();
        for p in space.peers().await {
            let _ = sync_peer(&self.inner, &space, p).await;
        }
        Ok(())
    }

    /// The active Space's live replica handle. Apps define their own schema on this.
    pub fn doc(&self) -> SharedDoc {
        self.active_state().doc()
    }

    /// BLOB PRIMITIVE — serve a file from the active Space. Imports `path` into that
    /// Space's content-addressed store and returns its [`Hash`].
    pub async fn share_file(&self, path: &Path) -> Result<Hash> {
        self.active_state().share_file(path).await
    }

    /// BLOB PRIMITIVE — fetch a file by `hash` from `peer` in the active Space, writing
    /// it to `dest`. Resumes from partial data; BLAKE3-verified end to end.
    pub async fn fetch_file(
        &self,
        peer: impl Into<EndpointAddr>,
        hash: Hash,
        dest: &Path,
    ) -> Result<()> {
        fetch_for_space(
            &self.inner,
            &self.active_state(),
            peer.into(),
            hash,
            dest,
            |_, _| {},
        )
        .await
    }

    /// Like [`Mesh::fetch_file`], reporting live progress via `on_progress(bytes, relayed)`.
    pub async fn fetch_file_with_progress(
        &self,
        peer: impl Into<EndpointAddr>,
        hash: Hash,
        dest: &Path,
        on_progress: impl FnMut(u64, bool),
    ) -> Result<()> {
        fetch_for_space(
            &self.inner,
            &self.active_state(),
            peer.into(),
            hash,
            dest,
            on_progress,
        )
        .await
    }

    /// BLOB PRIMITIVE — read `[start, end)` bytes of `hash` from the active Space,
    /// fetching the blob from `peer` first if it isn't already local. Backs `files.read`
    /// so an agent reads file contents across devices without writing to a user path.
    pub async fn read_file(
        &self,
        peer: impl Into<EndpointAddr>,
        hash: Hash,
        start: u64,
        end: u64,
    ) -> Result<Vec<u8>> {
        read_for_space(
            &self.inner,
            &self.active_state(),
            peer.into(),
            hash,
            start,
            end,
        )
        .await
    }

    /// Persist the active Space's replica now (a compacted snapshot).
    pub async fn save(&self) -> Result<()> {
        self.active_state().save().await
    }

    /// Gracefully stop: halt the background loop, persist every Space, and shut down the
    /// router + every Space's blob store.
    pub async fn shutdown(self) -> Result<()> {
        self.inner.cancel.cancel();
        // Router shutdown drives each Space's blobs *provider* shutdown via the
        // BlobDispatcher; then persist + close each Space's store.
        let _ = self.inner.router.shutdown().await;
        for space in self.inner.registry.all() {
            space.shutdown().await;
        }
        Ok(())
    }
}

/// A scoped handle to one Space. Mirrors the active-Space [`Mesh`] methods but always
/// addresses this specific Space — for the GUI's Space picker and an MCP server bound
/// to exactly one Space (M6).
#[derive(Clone)]
pub struct SpaceHandle {
    inner: Arc<Inner>,
    space: Arc<SpaceState>,
}

impl SpaceHandle {
    /// This Space's id.
    pub fn id(&self) -> SpaceId {
        self.space.id()
    }

    /// This Space's friendly name.
    pub fn name(&self) -> String {
        self.space.name().to_string()
    }

    /// Whether this is the device's default Space.
    pub fn is_default(&self) -> bool {
        self.space.is_default()
    }

    /// This Space's live replica handle.
    pub fn doc(&self) -> SharedDoc {
        self.space.doc()
    }

    /// This Space's group key.
    pub fn group_key(&self) -> [u8; 32] {
        self.space.group_key()
    }

    /// Add a peer to keep this Space converged with.
    pub async fn add_peer(&self, peer: EndpointAddr) {
        if self.space.add_peer(peer).await {
            self.inner.changed.notify_waiters();
        }
    }

    /// Stop keeping a peer converged in this Space. Returns whether it was present.
    pub async fn remove_peer(&self, id: &str) -> bool {
        let Ok(addr) = endpoint_addr_from_id(id) else {
            return false;
        };
        let removed = self.space.remove_peer(addr.id).await;
        if removed {
            self.inner.changed.notify_waiters();
        }
        removed
    }

    /// Dial one peer and run a single sync round for this Space now (initiator).
    pub async fn sync_with(&self, peer: impl Into<EndpointAddr>) -> Result<()> {
        sync_peer(&self.inner, &self.space, peer.into()).await
    }

    /// Sync this Space once with every known peer (best-effort).
    pub async fn sync_all(&self) -> Result<()> {
        for p in self.space.peers().await {
            let _ = sync_peer(&self.inner, &self.space, p).await;
        }
        Ok(())
    }

    /// Serve a file from this Space.
    pub async fn share_file(&self, path: &Path) -> Result<Hash> {
        self.space.share_file(path).await
    }

    /// Fetch a file by `hash` from `peer` in this Space.
    pub async fn fetch_file(
        &self,
        peer: impl Into<EndpointAddr>,
        hash: Hash,
        dest: &Path,
    ) -> Result<()> {
        fetch_for_space(&self.inner, &self.space, peer.into(), hash, dest, |_, _| {}).await
    }

    /// Like [`SpaceHandle::fetch_file`], reporting live progress.
    pub async fn fetch_file_with_progress(
        &self,
        peer: impl Into<EndpointAddr>,
        hash: Hash,
        dest: &Path,
        on_progress: impl FnMut(u64, bool),
    ) -> Result<()> {
        fetch_for_space(
            &self.inner,
            &self.space,
            peer.into(),
            hash,
            dest,
            on_progress,
        )
        .await
    }

    /// Read `[start, end)` bytes of `hash` from this Space, fetching it from `peer` first
    /// if it isn't already local. Backs `files.read` for an MCP server bound to this Space.
    pub async fn read_file(
        &self,
        peer: impl Into<EndpointAddr>,
        hash: Hash,
        start: u64,
        end: u64,
    ) -> Result<Vec<u8>> {
        read_for_space(&self.inner, &self.space, peer.into(), hash, start, end).await
    }

    /// Persist this Space's replica now.
    pub async fn save(&self) -> Result<()> {
        self.space.save().await
    }

    /// Tell the engine this Space changed, so it syncs + persists now.
    pub fn announce_change(&self) {
        self.inner.changed.notify_waiters();
    }
}

/// Build a connectable [`EndpointAddr`] from a peer's endpoint-id string. Discovery
/// resolves the actual paths, so an id alone is enough to dial a peer that is online.
pub fn endpoint_addr_from_id(id: &str) -> Result<EndpointAddr> {
    let id: iroh::EndpointId = id
        .trim()
        .parse()
        .map_err(|e| CoreError::Pairing(format!("invalid device id: {e}")))?;
    Ok(EndpointAddr::from(id))
}

/// Parse an endpoint-id string into its 32 raw public-key bytes (for membership ops).
fn endpoint_bytes(s: &str) -> Result<[u8; 32]> {
    let id: iroh::EndpointId = s
        .trim()
        .parse()
        .map_err(|e| CoreError::Pairing(format!("invalid device id: {e}")))?;
    let mut a = [0u8; 32];
    a.copy_from_slice(id.as_bytes());
    Ok(a)
}

/// Parse a content [`Hash`] from its string form (as stored in a file offer).
pub fn hash_from_str(s: &str) -> Result<Hash> {
    s.trim()
        .parse::<Hash>()
        .map_err(|e| CoreError::Other(anyhow::anyhow!("invalid file hash: {e}")))
}

/// Background loop: on a change/new-peer/new-Space ping or every [`SYNC_INTERVAL`], sync
/// every Space with each of its peers and persist each Space. One independent,
/// cancellable task per `(Space, peer)`, so a slow/dead peer in one Space can't stall
/// any other peer or Space. Exits when the engine shuts down.
async fn sync_loop(inner: Arc<Inner>) {
    let mut tasks: HashMap<(SpaceId, EndpointId), CancellationToken> = HashMap::new();
    loop {
        tokio::select! {
            _ = inner.cancel.cancelled() => break,
            _ = inner.changed.notified() => {}
            _ = tokio::time::sleep(SYNC_INTERVAL) => {}
        }
        let spaces = inner.registry.all();
        let mut current: HashSet<(SpaceId, EndpointId)> = HashSet::new();
        for space in &spaces {
            for p in space.peers().await {
                let key = (space.id(), p.id);
                current.insert(key);
                tasks.entry(key).or_insert_with(|| {
                    let token = CancellationToken::new();
                    tokio::spawn(peer_sync_task(
                        inner.clone(),
                        space.clone(),
                        p.clone(),
                        token.clone(),
                    ));
                    token
                });
            }
            let _ = space.save().await;
        }
        // Cancel + forget tasks for (Space, peer) pairs no longer present (peer removed
        // or Space left).
        tasks.retain(|key, token| {
            if current.contains(key) {
                true
            } else {
                token.cancel();
                false
            }
        });
    }
    for (_, token) in tasks {
        token.cancel();
    }
}

/// One independent task per `(Space, peer)`: re-sync on a ping or every interval, each
/// round bounded by timeouts. Exits on shutdown or when this pair is removed.
async fn peer_sync_task(
    inner: Arc<Inner>,
    space: Arc<SpaceState>,
    peer: EndpointAddr,
    token: CancellationToken,
) {
    loop {
        tokio::select! {
            _ = inner.cancel.cancelled() => break,
            _ = token.cancelled() => break,
            _ = inner.changed.notified() => {}
            _ = tokio::time::sleep(SYNC_INTERVAL) => {}
        }
        let _ = sync_peer(&inner, &space, peer.clone()).await;
    }
}

/// Dial a peer and drive one initiator-side sync round for `space`, bounded by timeouts
/// so an unreachable/slow peer fails cleanly instead of hanging.
async fn sync_peer(inner: &Arc<Inner>, space: &Arc<SpaceState>, peer: EndpointAddr) -> Result<()> {
    let peer_id = peer.id;
    let connect = inner.router.endpoint().connect(peer, MESH_ALPN);
    let conn = tokio::time::timeout(CONNECT_TIMEOUT, connect)
        .await
        .map_err(|_| CoreError::Unreachable("connect timed out".into()))?
        .map_err(|e| CoreError::Unreachable(e.to_string()))?;
    // Role-enforced Spaces run the verified protocol (membership gate + per-change
    // signature verification); permissive Spaces run plain Automerge sync.
    if space.enforced() {
        tokio::time::timeout(SYNC_ROUND_TIMEOUT, space.initiate_verified(conn))
            .await
            .map_err(|_| CoreError::Sync("sync round timed out".into()))?
            .map_err(|e| CoreError::Sync(e.to_string()))?;
    } else {
        let round = space.sync().clone().initiate_sync(conn);
        tokio::time::timeout(SYNC_ROUND_TIMEOUT, round)
            .await
            .map_err(|_| CoreError::Sync("sync round timed out".into()))?
            .map_err(|e| CoreError::Sync(e.to_string()))?;
    }
    space.record_sync(peer_id);
    Ok(())
}

/// Dial a peer on the blobs ALPN and fetch `hash` for `space` (its store, key, and id).
async fn fetch_for_space(
    inner: &Arc<Inner>,
    space: &Arc<SpaceState>,
    peer: EndpointAddr,
    hash: Hash,
    dest: &Path,
    on_progress: impl FnMut(u64, bool),
) -> Result<()> {
    let connect = inner.router.endpoint().connect(peer, iroh_blobs::ALPN);
    let conn = tokio::time::timeout(CONNECT_TIMEOUT, connect)
        .await
        .map_err(|_| CoreError::Unreachable("connect timed out".into()))?
        .map_err(|e| CoreError::Unreachable(e.to_string()))?;
    let group_key = space.group_key();
    let space_id = space.id();
    blobs::fetch_to_with_progress(
        space.store(),
        conn,
        hash,
        dest,
        &group_key,
        &space_id,
        on_progress,
    )
    .await
}

/// Read `[start, end)` bytes of `hash` in `space` into memory, fetching the blob from
/// `peer` first if it isn't already local (membership/role-gated like any fetch). Backs
/// `files.read`: an agent reads file contents across devices without a user-path write.
async fn read_for_space(
    inner: &Arc<Inner>,
    space: &Arc<SpaceState>,
    peer: EndpointAddr,
    hash: Hash,
    start: u64,
    end: u64,
) -> Result<Vec<u8>> {
    if !blobs::is_local_complete(space.store(), hash).await {
        let connect = inner.router.endpoint().connect(peer, iroh_blobs::ALPN);
        let conn = tokio::time::timeout(CONNECT_TIMEOUT, connect)
            .await
            .map_err(|_| CoreError::Unreachable("connect timed out".into()))?
            .map_err(|e| CoreError::Unreachable(e.to_string()))?;
        blobs::ensure_local(
            space.store(),
            conn,
            hash,
            &space.group_key(),
            &space.id(),
            |_, _| {},
        )
        .await?;
    }
    blobs::read_range(space.store(), hash, start, end).await
}

/// Migrate a pre-Spaces flat data directory (`data_dir/{doc.automerge,group.key,
/// atrest.key,blobs/}`) into the default Space's subdir, the first time a device with an
/// old single-group install starts under the Spaces layout. A no-op on fresh installs
/// (nothing to move) and on already-migrated dirs (`spaces/` already exists).
fn migrate_flat_layout(data_dir: &Path, spaces_root: &Path) -> Result<()> {
    if spaces_root.exists() {
        return Ok(());
    }
    let flat_doc = data_dir.join("doc.automerge");
    let flat_group = data_dir.join("group.key");
    let flat_atrest = data_dir.join("atrest.key");
    let flat_blobs = data_dir.join("blobs");
    let has_flat =
        flat_doc.exists() || flat_group.exists() || flat_atrest.exists() || flat_blobs.exists();
    if !has_flat {
        return Ok(());
    }
    let default_dir = spaces_root.join(SpaceId::default_space().to_hex());
    std::fs::create_dir_all(&default_dir)?;
    for (src, name) in [
        (flat_doc, "doc.automerge"),
        (flat_group, "group.key"),
        (flat_atrest, "atrest.key"),
    ] {
        if src.exists() {
            let _ = std::fs::rename(&src, default_dir.join(name));
        }
    }
    if flat_blobs.is_dir() {
        let _ = std::fs::rename(&flat_blobs, default_dir.join("blobs"));
    }
    Ok(())
}
