//! agent-memory over MCP: an agent appends and recalls memory through the protocol —
//! the same memory any model on any of the user's devices would read.

use agent_memory::{Memory, MeshConfig};
use serde_json::json;

#[tokio::test]
async fn agent_remembers_and_recalls_over_mcp() {
    let dir = tempfile::tempdir().unwrap();
    let mem = Memory::start(MeshConfig::local_only(dir.path()))
        .await
        .unwrap();

    // tools/list advertises the memory tools.
    let listed = mesh_mcp::handle_line(
        &mem,
        &json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" }).to_string(),
    )
    .await
    .unwrap();
    assert!(listed.contains("memory.append") && listed.contains("memory.search"));

    // The agent remembers something.
    let append = json!({
        "jsonrpc": "2.0", "id": 2, "method": "tools/call",
        "params": { "name": "memory.append", "arguments": { "text": "user is allergic to peanuts", "kind": "fact" } }
    });
    let added = mesh_mcp::handle_line(&mem, &append.to_string())
        .await
        .unwrap();
    assert!(added.contains("\"isError\":false"));

    // The agent recalls it.
    let search = json!({
        "jsonrpc": "2.0", "id": 3, "method": "tools/call",
        "params": { "name": "memory.search", "arguments": { "query": "peanuts" } }
    });
    let found = mesh_mcp::handle_line(&mem, &search.to_string())
        .await
        .unwrap();
    assert!(
        found.contains("allergic to peanuts"),
        "agent recalled the memory it stored"
    );

    mem.shutdown().await.unwrap();
}
