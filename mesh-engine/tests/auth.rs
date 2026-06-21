//! Engine access control: only peers holding the shared group key may sync. A peer
//! with the wrong key is rejected (no read, no overwrite) — the "only your devices"
//! guarantee, enforced by the group-key auth handshake.

mod common;
use common::{get_probe, local_mesh, put_probe};

use mesh_engine::{CoreConfig, Mesh};

#[tokio::test(flavor = "multi_thread")]
async fn wrong_group_key_cannot_sync() {
    let dm = tempfile::tempdir().unwrap();
    let ds = tempfile::tempdir().unwrap();

    let member = local_mesh(dm.path()).await; // in the shared test group
    let stranger = Mesh::start(CoreConfig::local_only(ds.path()).with_group_key([99u8; 32]))
        .await
        .unwrap();

    put_probe(&member, "secret").await;

    // Stranger (wrong key) tries to pull the member's state — rejected at the handshake.
    let r = stranger.sync_with(member.endpoint_addr()).await;
    assert!(r.is_err(), "a peer with the wrong group key is rejected");
    assert_eq!(get_probe(&stranger).await, None, "stranger learned nothing");

    // The reverse direction also fails — the member won't accept the stranger either.
    let r2 = member.sync_with(stranger.endpoint_addr()).await;
    assert!(r2.is_err(), "the member rejects the stranger too");
    assert_eq!(
        get_probe(&member).await.as_deref(),
        Some("secret"),
        "member's state untouched"
    );

    member.shutdown().await.unwrap();
    stranger.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn rotating_group_key_evicts_old_devices() {
    let da = tempfile::tempdir().unwrap();
    let db = tempfile::tempdir().unwrap();
    let a = local_mesh(da.path()).await; // shared test group
    let b = local_mesh(db.path()).await; // shared test group

    // Initially A and B are in the same group and sync.
    put_probe(&a, "x").await;
    a.sync_with(b.endpoint_addr()).await.unwrap();
    assert_eq!(get_probe(&b).await.as_deref(), Some("x"));

    // A revokes by rotating the group key, then restarts to adopt it.
    a.rotate_group_key().unwrap();
    a.shutdown().await.unwrap();
    let a2 = Mesh::start(CoreConfig::local_only(da.path()))
        .await
        .unwrap(); // loads NEW key

    // B (still on the OLD key) is now locked out.
    put_probe(&a2, "after-rotate").await;
    assert!(
        b.sync_with(a2.endpoint_addr()).await.is_err(),
        "a rotated-out device can no longer sync"
    );

    a2.shutdown().await.unwrap();
    b.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn same_group_key_syncs() {
    let da = tempfile::tempdir().unwrap();
    let db = tempfile::tempdir().unwrap();
    let a = local_mesh(da.path()).await;
    let b = local_mesh(db.path()).await; // same shared group key

    put_probe(&a, "shared").await;
    a.sync_with(b.endpoint_addr()).await.unwrap();
    assert_eq!(
        get_probe(&b).await.as_deref(),
        Some("shared"),
        "same-group peers converge"
    );

    a.shutdown().await.unwrap();
    b.shutdown().await.unwrap();
}
