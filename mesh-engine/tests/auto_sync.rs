//! Engine: the mesh layer auto-syncs. After peers are added, a change on one peer
//! propagates to the other WITHOUT a manual sync call — the background loop does it.

mod common;
use std::time::Duration;

use common::{get_probe, local_mesh, put_probe};

#[tokio::test(flavor = "multi_thread")]
async fn peers_auto_converge_on_change() {
    let da = tempfile::tempdir().unwrap();
    let db = tempfile::tempdir().unwrap();
    let a = local_mesh(da.path()).await;
    let b = local_mesh(db.path()).await;

    // Form the mesh (each device knows the other), then write — no manual sync.
    a.add_peer(b.endpoint_addr()).await;
    b.add_peer(a.endpoint_addr()).await;
    put_probe(&a, "auto").await;
    a.announce_change();

    // The background loop should carry it to B within a couple of intervals.
    let mut converged = false;
    for _ in 0..100 {
        if get_probe(&b).await.as_deref() == Some("auto") {
            converged = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(
        converged,
        "B auto-synced A's change with no manual sync call"
    );

    a.shutdown().await.unwrap();
    b.shutdown().await.unwrap();
}
