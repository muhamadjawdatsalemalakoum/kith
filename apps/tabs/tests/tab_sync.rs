//! centralTabs (app) on mesh-engine: a tab created on one device appears on
//! another after a sync. Proves the app works on the engine — the same substrate
//! that powers the generic engine tests now carries a real tab schema.

use centraltabs::{MeshConfig, Tabs};

#[tokio::test(flavor = "multi_thread")]
async fn a_tab_created_on_one_device_appears_on_another() {
    let da = tempfile::tempdir().unwrap();
    let db = tempfile::tempdir().unwrap();
    let a = Tabs::start(MeshConfig::local_only(da.path()).with_group_key([7u8; 32]))
        .await
        .unwrap();
    let b = Tabs::start(MeshConfig::local_only(db.path()).with_group_key([7u8; 32]))
        .await
        .unwrap();

    a.seed_example().await.unwrap();
    assert_eq!(b.first_tab_url().await, None, "B starts with no tabs");

    a.sync_with(b.endpoint_addr()).await.unwrap();

    assert_eq!(
        b.first_tab_url().await.as_deref(),
        Some("https://example.com"),
        "B has A's tab after one sync round"
    );

    a.shutdown().await.unwrap();
    b.shutdown().await.unwrap();
}
