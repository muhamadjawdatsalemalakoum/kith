//! agent-memory: memory taught on one device is present on another after sync —
//! the core promise ("your memory follows you across your machines, no cloud").

use agent_memory::{Memory, MeshConfig};

#[tokio::test(flavor = "multi_thread")]
async fn memory_follows_you_across_devices() {
    let da = tempfile::tempdir().unwrap();
    let db = tempfile::tempdir().unwrap();
    let laptop = Memory::start(MeshConfig::local_only(da.path()).with_group_key([7u8; 32]))
        .await
        .unwrap();
    let desktop = Memory::start(MeshConfig::local_only(db.path()).with_group_key([7u8; 32]))
        .await
        .unwrap();

    // Taught on the laptop...
    let id = laptop
        .remember("the user prefers Rust and dark mode", "preference")
        .await
        .unwrap();
    assert!(
        desktop.search("dark mode").await.is_empty(),
        "desktop hasn't synced yet"
    );

    // ...synced to the desktop.
    laptop.sync_with(desktop.endpoint_addr()).await.unwrap();
    let hits = desktop.search("dark mode").await;
    assert_eq!(hits.len(), 1, "desktop now recalls the laptop's memory");
    assert_eq!(hits[0].kind, "preference");

    // Forgetting on the desktop propagates back to the laptop.
    assert!(desktop.forget(&id).await.unwrap());
    desktop.sync_with(laptop.endpoint_addr()).await.unwrap();
    assert!(
        laptop.search("dark mode").await.is_empty(),
        "forget propagated as a tombstone"
    );

    laptop.shutdown().await.unwrap();
    desktop.shutdown().await.unwrap();
}
