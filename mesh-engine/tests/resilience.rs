//! Engine resilience: an unreachable peer fails cleanly (no hang), and an offline
//! peer in the set never blocks syncing with the healthy ones (per-peer isolation).
//!
//! "Offline" here is a valid EndpointId we can't reach (no address, not
//! discoverable) — what an offline peer looks like in production. (We avoid a
//! *shut-down* peer's stale direct address: in LocalOnly mode, hammering a dead
//! direct address interferes with the shared magicsock — a test-mode artifact, not a
//! production path, since Decentralized mode dials by id via DHT/relay.)

mod common;
use std::time::Duration;

use common::{get_probe, local_mesh, put_probe};
use iroh::{EndpointAddr, SecretKey};

/// A valid EndpointId we can never reach — i.e. a peer that is simply offline.
fn offline_peer() -> EndpointAddr {
    EndpointAddr::new(SecretKey::generate().public())
}

#[tokio::test(flavor = "multi_thread")]
async fn sync_to_unreachable_peer_errors() {
    let da = tempfile::tempdir().unwrap();
    let a = local_mesh(da.path()).await;

    let r = a.sync_with(offline_peer()).await;
    assert!(
        r.is_err(),
        "syncing an offline peer returns an error, not a hang"
    );

    a.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn offline_peer_does_not_block_healthy_peer() {
    let da = tempfile::tempdir().unwrap();
    let db = tempfile::tempdir().unwrap();
    let a = local_mesh(da.path()).await;
    let b = local_mesh(db.path()).await;

    // A's peer set contains an OFFLINE peer AND a healthy one. The offline peer (its
    // own isolated task, bounded by a connect timeout) must not stop B converging.
    a.add_peer(offline_peer()).await;
    a.add_peer(b.endpoint_addr()).await;
    b.add_peer(a.endpoint_addr()).await;
    put_probe(&a, "through").await;
    a.announce_change();

    let mut converged = false;
    for _ in 0..100 {
        if get_probe(&b).await.as_deref() == Some("through") {
            converged = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(
        converged,
        "healthy peer converged despite an offline peer in the set"
    );

    a.shutdown().await.unwrap();
    b.shutdown().await.unwrap();
}
