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
