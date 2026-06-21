//! Engine data-integrity recovery: a corrupt replica snapshot self-heals (boots
//! fresh + re-converges) instead of bricking, and a torn identity key refuses to
//! silently regenerate (which would orphan history + drop the device from peers).

mod common;
use common::{get_probe, local_mesh, put_probe};

use mesh_engine::{CoreConfig, Mesh};

#[tokio::test(flavor = "multi_thread")]
async fn corrupt_replica_recovers_fresh() {
    let dir = tempfile::tempdir().unwrap();
    {
        let a = local_mesh(dir.path()).await;
        put_probe(&a, "x").await;
        a.shutdown().await.unwrap(); // persists doc.automerge
    }

    // Corrupt the snapshot on disk (simulate a torn write / disk damage).
    std::fs::write(
        dir.path().join("doc.automerge"),
        b"not a valid automerge document",
    )
    .unwrap();

    // Restart must NOT brick — it recovers by booting a fresh replica.
    let a2 = Mesh::start(CoreConfig::local_only(dir.path()))
        .await
        .expect("must recover from a corrupt replica, not fail to start");
    assert_eq!(
        get_probe(&a2).await,
        None,
        "booted a fresh replica after corruption"
    );

    // The corrupt snapshot is preserved aside rather than silently destroyed.
    let kept_aside = std::fs::read_dir(dir.path())
        .unwrap()
        .flatten()
        .any(|e| e.file_name().to_string_lossy().contains("corrupt"));
    assert!(
        kept_aside,
        "the corrupt snapshot was moved aside for inspection"
    );

    a2.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn truncated_identity_key_errors_not_regenerates() {
    let dir = tempfile::tempdir().unwrap();
    {
        let a = local_mesh(dir.path()).await;
        a.shutdown().await.unwrap();
    }

    // Truncate node.key (simulate a torn write).
    std::fs::write(dir.path().join("node.key"), b"short").unwrap();

    // Restart must refuse rather than silently minting a NEW identity (which would
    // change the EndpointId/ActorId and orphan this device's CRDT history).
    let r = Mesh::start(CoreConfig::local_only(dir.path())).await;
    assert!(
        r.is_err(),
        "a corrupt node.key must error, not silently regenerate identity"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn replica_is_encrypted_at_rest() {
    let dir = tempfile::tempdir().unwrap();
    let a = local_mesh(dir.path()).await;
    put_probe(&a, "topsecret-value").await;
    a.save().await.unwrap();
    a.shutdown().await.unwrap();

    // The persisted snapshot must NOT contain the value in cleartext.
    let bytes = std::fs::read(dir.path().join("doc.automerge")).unwrap();
    let needle = b"topsecret-value";
    let leaked = bytes.windows(needle.len()).any(|w| w == needle);
    assert!(!leaked, "the value must not appear in cleartext on disk");

    // ...but a fresh peer on the same dir (same atrest.key) decrypts it fine.
    let a2 = local_mesh(dir.path()).await;
    assert_eq!(get_probe(&a2).await.as_deref(), Some("topsecret-value"));
    a2.shutdown().await.unwrap();
}
