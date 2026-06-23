//! Engine: encrypted, passphrase-protected Space export/import (M7 — the recovery path).

mod common;
use common::{local_mesh, local_mesh_with_blobs};

use mesh_engine::automerge::transaction::Transactable;
use mesh_engine::automerge::{ReadDoc, ScalarValue, Value, ROOT};
use mesh_engine::SharedDoc;

async fn put_probe(doc: &SharedDoc, value: &str) {
    let mut guard = doc.lock().await;
    let mut tx = guard.transaction();
    tx.put(ROOT, "probe", value).expect("put probe");
    tx.commit();
}

async fn get_probe(doc: &SharedDoc) -> Option<String> {
    let guard = doc.lock().await;
    let (val, _) = guard.get(ROOT, "probe").ok()??;
    match val {
        Value::Scalar(s) => match s.as_ref() {
            ScalarValue::Str(t) => Some(t.to_string()),
            _ => None,
        },
        _ => None,
    }
}

/// A Space exports to an encrypted bundle and restores byte-identically on a fresh device.
#[tokio::test(flavor = "multi_thread")]
async fn space_export_import_roundtrip() {
    let da = tempfile::tempdir().unwrap();
    let db = tempfile::tempdir().unwrap();

    // Device A: create a Space, write data, export it under a passphrase.
    let a = local_mesh(da.path()).await;
    let sid = a.create_space("backup-me").await.unwrap();
    let a_s = a.space(sid).unwrap();
    put_probe(&a_s.doc(), "precious data").await;
    a_s.save().await.unwrap();
    let bundle = a
        .export_space(sid, "correct horse battery staple")
        .await
        .unwrap();
    assert!(bundle.len() > 32, "bundle has content");
    a.shutdown().await.unwrap();

    // Device B (fresh data dir): import restores the Space and its data.
    let b = local_mesh(db.path()).await;
    assert!(b.space(sid).is_none(), "B doesn't have it yet");
    let imported = b
        .import_space(&bundle, "correct horse battery staple")
        .await
        .unwrap();
    assert_eq!(imported, sid, "the same SpaceId is restored");
    let b_s = b.space(imported).unwrap();
    assert_eq!(
        get_probe(&b_s.doc()).await.as_deref(),
        Some("precious data"),
        "the replica restored byte-identically"
    );

    b.shutdown().await.unwrap();
}

/// Shared file CONTENT survives export/import: a blob shared in a Space is restored on a
/// fresh device and readable there (content-addressed, identical hash).
#[tokio::test(flavor = "multi_thread")]
async fn space_export_import_preserves_blobs() {
    let work = tempfile::tempdir().unwrap();
    let da = tempfile::tempdir().unwrap();
    let db = tempfile::tempdir().unwrap();

    let a = local_mesh_with_blobs(da.path()).await;
    let sid = a.create_space("with-files").await.unwrap();
    let a_s = a.space(sid).unwrap();
    let src = work.path().join("doc.txt");
    std::fs::write(&src, b"exported file bytes").unwrap();
    let hash = a_s.share_file(&src).await.unwrap();
    let bundle = a.export_space(sid, "pw").await.unwrap();
    a.shutdown().await.unwrap();

    let b = local_mesh_with_blobs(db.path()).await;
    let imported = b.import_space(&bundle, "pw").await.unwrap();
    let b_s = b.space(imported).unwrap();
    // The blob is local on B after import — read it without dialing anyone.
    let bytes = b_s
        .read_file(b.endpoint_addr(), hash, 0, 4096)
        .await
        .unwrap();
    assert_eq!(
        bytes, b"exported file bytes",
        "blob content restored on import"
    );

    b.shutdown().await.unwrap();
}

/// A wrong passphrase fails to import (AEAD tag mismatch), restoring nothing.
#[tokio::test(flavor = "multi_thread")]
async fn import_with_wrong_passphrase_fails() {
    let da = tempfile::tempdir().unwrap();
    let db = tempfile::tempdir().unwrap();

    let a = local_mesh(da.path()).await;
    let sid = a.create_space("secret").await.unwrap();
    let a_s = a.space(sid).unwrap();
    put_probe(&a_s.doc(), "top secret").await;
    a_s.save().await.unwrap();
    let bundle = a.export_space(sid, "the-right-passphrase").await.unwrap();
    a.shutdown().await.unwrap();

    let b = local_mesh(db.path()).await;
    let r = b.import_space(&bundle, "the-WRONG-passphrase").await;
    assert!(r.is_err(), "a wrong passphrase cannot decrypt the export");
    assert!(b.space(sid).is_none(), "nothing was restored");

    b.shutdown().await.unwrap();
}
