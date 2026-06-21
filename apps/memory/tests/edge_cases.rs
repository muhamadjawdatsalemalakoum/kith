//! agent-memory edge cases: an empty search must not dump everything (privacy), and
//! memories persist + accumulate across restarts (durable saves, not just the loop).

use agent_memory::{Memory, MeshConfig};

#[tokio::test(flavor = "multi_thread")]
async fn empty_query_returns_nothing_not_everything() {
    let dir = tempfile::tempdir().unwrap();
    let m = Memory::start(MeshConfig::local_only(dir.path()))
        .await
        .unwrap();

    m.remember("alpha", "fact").await.unwrap();
    m.remember("beta", "fact").await.unwrap();

    assert_eq!(
        m.search("").await.len(),
        0,
        "empty query must not dump all memory"
    );
    assert_eq!(
        m.search("   ").await.len(),
        0,
        "whitespace query must not dump all memory"
    );
    assert_eq!(m.search("alpha").await.len(), 1, "a real query still works");

    m.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn memory_persists_and_accumulates_across_restarts() {
    let dir = tempfile::tempdir().unwrap();

    {
        let m = Memory::start(MeshConfig::local_only(dir.path()))
            .await
            .unwrap();
        m.remember("first", "fact").await.unwrap(); // durable via save()
        m.shutdown().await.unwrap();
    }
    {
        let m = Memory::start(MeshConfig::local_only(dir.path()))
            .await
            .unwrap();
        m.remember("second", "fact").await.unwrap();
        m.shutdown().await.unwrap();
    }
    {
        let m = Memory::start(MeshConfig::local_only(dir.path()))
            .await
            .unwrap();
        assert_eq!(
            m.all().await.len(),
            2,
            "both memories survived two restarts"
        );
        m.shutdown().await.unwrap();
    }
}
