//! Engine: the BLOB primitive. One peer serves a file; another fetches it by hash.
//!
//! This is the SECOND primitive — proving the engine is a real substrate (mutable
//! state + immutable content), not a tab library in disguise. Dropwire-on-mesh, the
//! mesh-browser, and file-sharing in chat all stand on exactly this.

mod common;
use common::local_mesh_with_blobs;

#[tokio::test(flavor = "multi_thread")]
async fn serve_a_file_fetch_a_file() {
    let work = tempfile::tempdir().unwrap();
    let da = tempfile::tempdir().unwrap();
    let db = tempfile::tempdir().unwrap();

    let src = work.path().join("hello.txt");
    let payload = b"hello from the mesh blob primitive";
    std::fs::write(&src, payload).unwrap();

    let a = local_mesh_with_blobs(da.path()).await;
    let b = local_mesh_with_blobs(db.path()).await;

    // A imports + serves the file -> content hash.
    let hash = a.share_file(&src).await.unwrap();

    // B fetches it by hash from A, writing to dest.
    let dest = work.path().join("out.txt");
    b.fetch_file(a.endpoint_addr(), hash, &dest).await.unwrap();

    assert_eq!(
        std::fs::read(&dest).unwrap(),
        payload,
        "B fetched A's file byte-for-byte over the mesh"
    );

    a.shutdown().await.unwrap();
    b.shutdown().await.unwrap();
}
