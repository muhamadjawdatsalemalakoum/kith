//! App + MCP: an agent drives the tabs app over the MCP protocol. This proves the
//! full chain — MCP request -> McpApp trait -> the app's mesh replica — works, i.e.
//! implementing one trait made the app agent-accessible.

use centraltabs::{MeshConfig, Tabs};
use serde_json::json;

#[tokio::test]
async fn an_agent_adds_and_reads_tabs_over_mcp() {
    let dir = tempfile::tempdir().unwrap();
    let tabs = Tabs::start(MeshConfig::local_only(dir.path()))
        .await
        .unwrap();

    // tools/list advertises the tabs tools.
    let listed = mesh_mcp::handle_line(
        &tabs,
        &json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" }).to_string(),
    )
    .await
    .unwrap();
    assert!(
        listed.contains("tabs.add"),
        "tools/list should advertise tabs.add"
    );

    // The agent saves a tab.
    let add = json!({
        "jsonrpc": "2.0", "id": 2, "method": "tools/call",
        "params": { "name": "tabs.add", "arguments": { "url": "https://example.com", "title": "Example" } }
    });
    let added = mesh_mcp::handle_line(&tabs, &add.to_string())
        .await
        .unwrap();
    assert!(
        added.contains("\"isError\":false"),
        "tabs.add should succeed"
    );

    // The agent reads it back — proving the call mutated the real mesh replica.
    let count = json!({ "jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": { "name": "tabs.count" } });
    let counted = mesh_mcp::handle_line(&tabs, &count.to_string())
        .await
        .unwrap();
    assert!(
        counted.contains("count") && counted.contains('1'),
        "tabs.count should report 1"
    );

    tabs.shutdown().await.unwrap();
}
