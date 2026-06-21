//! Engine: the replica survives a restart.
//!
//! Write state, shut the peer down (which persists a compacted snapshot), then
//! start a fresh peer on the SAME data dir and confirm the state is still there.

mod common;
use common::{get_probe, local_mesh, put_probe};

#[tokio::test(flavor = "multi_thread")]
async fn replica_survives_restart() {
    let dir = tempfile::tempdir().unwrap();

    {
        let a = local_mesh(dir.path()).await;
        put_probe(&a, "persisted").await;
        a.shutdown().await.unwrap(); // persists doc.automerge
    }

    {
        let a2 = local_mesh(dir.path()).await; // loads doc.automerge
        assert_eq!(
            get_probe(&a2).await.as_deref(),
            Some("persisted"),
            "state written before shutdown is restored on restart"
        );
        a2.shutdown().await.unwrap();
    }
}
