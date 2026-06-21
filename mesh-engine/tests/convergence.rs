//! Engine: two peers that are online together converge.
//!
//! A value written on peer A appears on peer B after a single sync round. This is
//! the baseline substrate guarantee — schema-free, so it tests the engine itself.

mod common;
use common::{get_probe, local_mesh, put_probe};

#[tokio::test(flavor = "multi_thread")]
async fn two_peers_converge() {
    let da = tempfile::tempdir().unwrap();
    let db = tempfile::tempdir().unwrap();
    let a = local_mesh(da.path()).await;
    let b = local_mesh(db.path()).await;

    put_probe(&a, "hello-mesh").await;
    assert_eq!(get_probe(&b).await, None, "B starts empty");

    a.sync_with(b.endpoint_addr()).await.unwrap();

    assert_eq!(
        get_probe(&b).await.as_deref(),
        Some("hello-mesh"),
        "B converged to A's value after one sync round"
    );

    a.shutdown().await.unwrap();
    b.shutdown().await.unwrap();
}
