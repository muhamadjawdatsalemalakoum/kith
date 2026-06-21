//! Engine configuration: where relay + discovery come from.

use std::path::PathBuf;

/// How a mesh peer is configured at startup.
#[derive(Debug, Clone)]
pub struct CoreConfig {
    /// Application data directory. Holds the node identity (`node.key`), the group
    /// key, the at-rest key, and the persisted replica.
    pub data_dir: PathBuf,
    /// Relay + discovery selection.
    pub infra: Infra,
    /// Optional explicit group key — the shared secret that gates who may sync.
    /// `None` loads/generates one in the data dir; devices in the same group hold the
    /// same key (established by pairing, or set explicitly here for tests).
    pub group_key: Option<[u8; 32]>,
    /// Serve the content/blob primitive. OFF by default — blob serving is not yet
    /// access-gated, so only enable it among trusted peers / on trusted networks.
    pub enable_blobs: bool,
}

/// Relay + discovery infrastructure selection.
///
/// The family ships serverless by default: peers find each other over the public
/// Mainline DHT (no server we run) and fall back to n0's free relays only when a
/// direct, end-to-end-encrypted connection can't be made.
#[derive(Debug, Clone)]
pub enum Infra {
    /// **Default.** Discovery via the public Mainline BitTorrent DHT (pkarr) +
    /// n0's free public relays as connection fallback. No servers we run; works
    /// across the internet; depends on n0 only for the minority of links that
    /// can't go direct.
    Decentralized,

    /// n0's public relays **and** n0's DNS discovery. Simplest free path; handy
    /// for development.
    N0Default,

    /// No relay, no discovery — direct connections only, using addresses exchanged
    /// out of band. Used for LAN mode and hermetic loopback tests.
    LocalOnly,

    /// **Test-only.** Relay-only transport against an *in-process* relay, with all
    /// direct IP paths removed — so single-machine tests exercise the real relay
    /// path. Gated behind `test-utils`; never in shipped builds.
    #[cfg(feature = "test-utils")]
    LocalRelay {
        /// Relay map from `iroh::test_utils::run_relay_server`.
        relay_map: iroh::RelayMap,
    },
}

impl CoreConfig {
    /// Recommended default: serverless — DHT discovery + n0 free relay fallback.
    pub fn serverless(data_dir: impl Into<PathBuf>) -> Self {
        Self {
            data_dir: data_dir.into(),
            infra: Infra::Decentralized,
            group_key: None,
            enable_blobs: false,
        }
    }

    /// Development configuration on n0's public infra (relays + DNS discovery).
    pub fn dev(data_dir: impl Into<PathBuf>) -> Self {
        Self {
            data_dir: data_dir.into(),
            infra: Infra::N0Default,
            group_key: None,
            enable_blobs: false,
        }
    }

    /// Local-only configuration (no relay/discovery) — used by tests and LAN mode.
    pub fn local_only(data_dir: impl Into<PathBuf>) -> Self {
        Self {
            data_dir: data_dir.into(),
            infra: Infra::LocalOnly,
            group_key: None,
            enable_blobs: false,
        }
    }

    /// Set the group key explicitly (e.g. the two ends of a pairing, or test peers
    /// that should be in the same group).
    pub fn with_group_key(mut self, key: [u8; 32]) -> Self {
        self.group_key = Some(key);
        self
    }

    /// Enable serving the content/blob primitive (off by default).
    pub fn with_blobs(mut self, enable: bool) -> Self {
        self.enable_blobs = enable;
        self
    }
}
