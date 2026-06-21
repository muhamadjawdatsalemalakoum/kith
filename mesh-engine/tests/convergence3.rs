//! Three peers, each writing a DISTINCT key concurrently, all converge to the same
//! merged document — the multi-device circle, not just pairwise sync.

mod common;
use common::local_mesh;

use mesh_engine::automerge::transaction::Transactable;
use mesh_engine::automerge::{ReadDoc, ScalarValue, Value, ROOT};
use mesh_engine::Mesh;

async fn put(mesh: &Mesh, key: &str, val: &str) {
    let doc = mesh.doc();
    let mut g = doc.lock().await;
    let mut tx = g.transaction();
    tx.put(ROOT, key, val).unwrap();
    tx.commit();
}

async fn get(mesh: &Mesh, key: &str) -> Option<String> {
    let doc = mesh.doc();
    let g = doc.lock().await;
    match g.get(ROOT, key).ok().flatten() {
        Some((Value::Scalar(s), _)) => match s.as_ref() {
            ScalarValue::Str(t) => Some(t.to_string()),
            _ => None,
        },
        _ => None,
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn three_peers_converge() {
    let da = tempfile::tempdir().unwrap();
    let db = tempfile::tempdir().unwrap();
    let dc = tempfile::tempdir().unwrap();

    let a = local_mesh(da.path()).await;
    let b = local_mesh(db.path()).await;
    let c = local_mesh(dc.path()).await;

    // Each peer writes its own key concurrently (distinct keys → no conflict).
    put(&a, "a", "1").await;
    put(&b, "b", "2").await;
    put(&c, "c", "3").await;

    // A few full-mesh rounds so every write reaches every peer (transitively).
    for _ in 0..3 {
        a.sync_with(b.endpoint_addr()).await.unwrap();
        b.sync_with(c.endpoint_addr()).await.unwrap();
        c.sync_with(a.endpoint_addr()).await.unwrap();
    }

    for m in [&a, &b, &c] {
        assert_eq!(get(m, "a").await.as_deref(), Some("1"));
        assert_eq!(get(m, "b").await.as_deref(), Some("2"));
        assert_eq!(get(m, "c").await.as_deref(), Some("3"));
    }

    a.shutdown().await.unwrap();
    b.shutdown().await.unwrap();
    c.shutdown().await.unwrap();
}
