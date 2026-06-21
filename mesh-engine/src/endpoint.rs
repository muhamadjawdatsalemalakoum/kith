//! Endpoint construction — the one place that wires up relay + discovery.
//!
//! iroh-1.0 naming is current as of the pin: `EndpointId` (was `NodeId`),
//! `EndpointAddr` (was `NodeAddr`), and `address_lookup` (was `discovery`).
//! Reused from Dropwire, with the engine's mesh-sync ALPN in place of the blobs ALPN.

use anyhow::Context;
use iroh::{Endpoint, SecretKey};

use crate::config::Infra;
use crate::error::Result;
use crate::MESH_ALPN;

/// Build and bind the single long-lived endpoint for the given infra.
///
/// The endpoint serves and dials [`MESH_ALPN`]. We intentionally do not block on
/// `endpoint.online()` here so startup is instant; callers await it (time-boxed)
/// only right before they need a relay-reachable [`iroh::EndpointAddr`].
pub async fn build(secret_key: SecretKey, infra: &Infra, enable_blobs: bool) -> Result<Endpoint> {
    use iroh::endpoint::presets;

    // Advertise mesh sync + pairing always; blobs only when enabled (off by default).
    let mut alpns = vec![MESH_ALPN.to_vec(), crate::pair::PAIR_ALPN.to_vec()];
    if enable_blobs {
        alpns.push(iroh_blobs::ALPN.to_vec());
    }

    let endpoint = match infra {
        // DEFAULT: Mainline-DHT (pkarr) discovery + n0's free relay fallback. No
        // server we run; works across the internet; depends on n0 only for the
        // minority of links that can't go direct.
        Infra::Decentralized => {
            use iroh::endpoint::RelayMode;
            use iroh_mainline_address_lookup::DhtAddressLookup;
            // Publishes + resolves our address via the public BitTorrent DHT. Must be
            // built inside a Tokio runtime (this fn is async). By default it publishes
            // only relay addresses, so the endpoint must stay online to republish.
            let dht = DhtAddressLookup::builder()
                .build()
                .context("build DHT address lookup")?;
            Endpoint::builder(presets::Minimal)
                .secret_key(secret_key)
                .alpns(alpns)
                .relay_mode(RelayMode::Default)
                .address_lookup(dht)
                .bind()
                .await
                .context("bind endpoint (decentralized: DHT + n0 relay)")?
        }

        Infra::N0Default => Endpoint::builder(presets::N0)
            .secret_key(secret_key)
            .alpns(alpns)
            .bind()
            .await
            .context("bind endpoint (n0 default)")?,

        Infra::LocalOnly => {
            use iroh::endpoint::RelayMode;
            Endpoint::builder(presets::Minimal)
                .secret_key(secret_key)
                .alpns(alpns)
                .relay_mode(RelayMode::Disabled)
                .bind()
                .await
                .context("bind endpoint (local only)")?
        }

        // TEST-ONLY: relay-only against an in-process relay. Add the custom relay
        // transport, trust its self-signed test cert, and strip every direct IP
        // transport — so the only path to a peer is through the relay.
        #[cfg(feature = "test-utils")]
        Infra::LocalRelay { relay_map } => {
            use iroh::endpoint::RelayMode;
            use iroh::tls::CaTlsConfig;
            Endpoint::builder(presets::Minimal)
                .secret_key(secret_key)
                .alpns(alpns)
                .relay_mode(RelayMode::Custom(relay_map.clone()))
                .ca_tls_config(CaTlsConfig::insecure_skip_verify())
                .clear_ip_transports()
                .bind()
                .await
                .context("bind endpoint (local relay, test-only)")?
        }
    };

    Ok(endpoint)
}
