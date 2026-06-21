//! centralTabs: multiple tabs added (via MCP) are counted and synced across devices,
//! and an empty url is rejected rather than persisting a blank record.

use centraltabs::{MeshConfig, Tabs};
use serde_json::json;

fn add_call(id: i64, url: &str, title: &str) -> String {
    json!({
        "jsonrpc": "2.0", "id": id, "method": "tools/call",
        "params": { "name": "tabs.add", "arguments": { "url": url, "title": title } }
    })
    .to_string()
}

#[tokio::test(flavor = "multi_thread")]
async fn multiple_tabs_counted_synced_and_empty_rejected() {
    let da = tempfile::tempdir().unwrap();
    let db = tempfile::tempdir().unwrap();
    let a = Tabs::start(MeshConfig::local_only(da.path()).with_group_key([7u8; 32]))
        .await
        .unwrap();
    let b = Tabs::start(MeshConfig::local_only(db.path()).with_group_key([7u8; 32]))
        .await
        .unwrap();

    // Add two tabs on A via MCP.
    for (i, (url, title)) in [("https://a.example", "A"), ("https://b.example", "B")]
        .iter()
        .enumerate()
    {
        let r = mesh_mcp::handle_line(&a, &add_call((i + 1) as i64, url, title))
            .await
            .unwrap();
        assert!(r.contains("\"isError\":false"), "tabs.add succeeds");
    }

    // An empty/whitespace url is rejected (no blank record gets created + synced).
    let r = mesh_mcp::handle_line(&a, &add_call(9, "   ", "blank"))
        .await
        .unwrap();
    assert!(r.contains("\"isError\":true"), "empty url is rejected");

    // After a sync, B has both tabs (count iterates all groups).
    a.sync_with(b.endpoint_addr()).await.unwrap();
    let count = json!({ "jsonrpc": "2.0", "id": 10, "method": "tools/call", "params": { "name": "tabs.count" } });
    let r = mesh_mcp::handle_line(&b, &count.to_string()).await.unwrap();
    assert!(
        r.contains("\"isError\":false") && r.contains('2'),
        "B has both synced tabs (count == 2)"
    );

    a.shutdown().await.unwrap();
    b.shutdown().await.unwrap();
}
