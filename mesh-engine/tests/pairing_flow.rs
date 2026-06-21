//! Device pairing end to end: two devices in different groups pair from a short
//! code, after which the joiner can sync; a wrong code fails (no key handed out).

mod common;
use common::{get_probe, put_probe};

use mesh_engine::{CoreConfig, Mesh};

#[tokio::test(flavor = "multi_thread")]
async fn pair_a_new_device_then_sync() {
    let da = tempfile::tempdir().unwrap();
    let db = tempfile::tempdir().unwrap();

    // A and B start in DIFFERENT groups (each its own random group.key).
    let a = Mesh::start(CoreConfig::local_only(da.path()))
        .await
        .unwrap();
    let b = Mesh::start(CoreConfig::local_only(db.path()))
        .await
        .unwrap();
    put_probe(&a, "joined-data").await;

    // Before pairing, B is a stranger and cannot sync A's state.
    assert!(
        b.sync_with(a.endpoint_addr()).await.is_err(),
        "stranger rejected pre-pairing"
    );

    // Pair: A arms with a code; B joins with the same code and adopts A's group key.
    a.arm_pairing(b"hunter2");
    b.pair_with(a.endpoint_addr(), b"hunter2").await.unwrap();
    b.shutdown().await.unwrap();

    // B restarts (loading the adopted group key) and now syncs successfully.
    let b2 = Mesh::start(CoreConfig::local_only(db.path()))
        .await
        .unwrap();
    b2.sync_with(a.endpoint_addr()).await.unwrap();
    assert_eq!(
        get_probe(&b2).await.as_deref(),
        Some("joined-data"),
        "the paired device joined the group and synced"
    );

    a.shutdown().await.unwrap();
    b2.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn wrong_pairing_code_fails() {
    let da = tempfile::tempdir().unwrap();
    let db = tempfile::tempdir().unwrap();
    let a = Mesh::start(CoreConfig::local_only(da.path()))
        .await
        .unwrap();
    let b = Mesh::start(CoreConfig::local_only(db.path()))
        .await
        .unwrap();

    a.arm_pairing(b"correct-code");
    let r = b.pair_with(a.endpoint_addr(), b"wrong-code").await;
    assert!(
        r.is_err(),
        "a wrong pairing code must fail (no group key handed out)"
    );

    a.shutdown().await.unwrap();
    b.shutdown().await.unwrap();
}
