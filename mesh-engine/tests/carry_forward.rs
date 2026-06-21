//! Engine HEADLINE: store-carry-forward.
//!
//! Peers A and B are NEVER online together. A's value still reaches B, carried by
//! intermediary C. This is the load-bearing bet behind the whole serverless
//! design: if an update can hop A -> C -> B, the mesh tolerates offline gaps and
//! the "pending, not lost" promise holds.

mod common;
use common::{get_probe, local_mesh, put_probe};

#[tokio::test(flavor = "multi_thread")]
async fn store_carry_forward_a_to_c_to_b() {
    let da = tempfile::tempdir().unwrap();
    let db = tempfile::tempdir().unwrap();
    let dc = tempfile::tempdir().unwrap();
    let a = local_mesh(da.path()).await;
    let b = local_mesh(db.path()).await;
    let c = local_mesh(dc.path()).await;

    // A writes a value. B and C have never seen it.
    put_probe(&a, "from-A").await;

    // A and C sync directly. B is not involved at all.
    a.sync_with(c.endpoint_addr()).await.unwrap();
    assert_eq!(
        get_probe(&c).await.as_deref(),
        Some("from-A"),
        "C got A's value directly"
    );

    // A goes offline for good — proving B never talks to A.
    a.shutdown().await.unwrap();

    // Later, C and B sync. A's change must reach B THROUGH C.
    c.sync_with(b.endpoint_addr()).await.unwrap();

    assert_eq!(
        get_probe(&b).await.as_deref(),
        Some("from-A"),
        "B received A's value via C, though A and B were never online together"
    );

    b.shutdown().await.unwrap();
    c.shutdown().await.unwrap();
}
