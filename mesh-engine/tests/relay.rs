//! Engine: state sync over the REAL relay transport (in-process relay, no internet).
//!
//! Every other test uses direct loopback. This proves the PRODUCTION relay-fallback
//! path: both peers are relay-only (all direct IP transports stripped), so the only
//! way between them is through the relay. If they converge here, relaying works.
//! Gated behind the `test-utils` feature.

mod common;
use common::{get_probe, put_probe};

use mesh_engine::{CoreConfig, Infra, Mesh};

async fn relay_mesh(dir: &std::path::Path, relay_map: iroh::RelayMap) -> Mesh {
    Mesh::start(CoreConfig {
        data_dir: dir.to_path_buf(),
        infra: Infra::LocalRelay { relay_map },
        group_key: Some([5u8; 32]), // both relay peers share a group
        enable_blobs: false,
    })
    .await
    .unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn state_syncs_over_relay() {
    let da = tempfile::tempdir().unwrap();
    let db = tempfile::tempdir().unwrap();

    // One in-process relay; both peers can ONLY reach each other through it.
    // `_relay` is the server drop guard — it must stay alive for the test.
    let (relay_map, _url, _relay) = iroh::test_utils::run_relay_server().await.unwrap();

    let a = relay_mesh(da.path(), relay_map.clone()).await;
    let b = relay_mesh(db.path(), relay_map.clone()).await;

    // Make sure both have a relay-reachable address before dialing.
    a.online().await;
    b.online().await;

    put_probe(&a, "via-relay").await;
    a.sync_with(b.endpoint_addr()).await.unwrap();

    assert_eq!(
        get_probe(&b).await.as_deref(),
        Some("via-relay"),
        "state converged over the relay-only transport"
    );

    a.shutdown().await.unwrap();
    b.shutdown().await.unwrap();
}

/// The `Infra::SelfHosted` path builds an endpoint from a relay URL (+ optional token) and
/// pkarr/DNS wiring, and two such peers converge over that self-hosted relay. The relay is
/// the in-process test relay (so this runs offline); under `test-utils` the SelfHosted arm
/// trusts its cert and forces the relay path, exercising the real self-hosted code end to
/// end. (pkarr/DNS discovery needs real servers; peers dial by address here.)
#[tokio::test(flavor = "multi_thread")]
async fn selfhosted_endpoint_builds_and_syncs() {
    let da = tempfile::tempdir().unwrap();
    let db = tempfile::tempdir().unwrap();

    let (_relay_map, url, _relay) = iroh::test_utils::run_relay_server().await.unwrap();

    let cfg = |dir: &std::path::Path| CoreConfig {
        data_dir: dir.to_path_buf(),
        infra: Infra::SelfHosted {
            relay_url: url.to_string(),
            relay_token: String::new(), // open in-process test relay
            pkarr_relay: "https://localhost/pkarr".to_string(),
            origin_domain: "kith.invalid".to_string(),
        },
        group_key: Some([6u8; 32]),
        enable_blobs: false,
    };

    let a = Mesh::start(cfg(da.path())).await.unwrap();
    let b = Mesh::start(cfg(db.path())).await.unwrap();
    a.online().await;
    b.online().await;

    put_probe(&a, "via-selfhosted").await;
    a.sync_with(b.endpoint_addr()).await.unwrap();
    assert_eq!(
        get_probe(&b).await.as_deref(),
        Some("via-selfhosted"),
        "state converged over a self-hosted relay"
    );

    a.shutdown().await.unwrap();
    b.shutdown().await.unwrap();
}
