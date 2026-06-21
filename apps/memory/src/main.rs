//! agent-memory binary: run the MCP server an AI agent connects to (the headline
//! use), or quickly remember/recall from the command line.
//!
//! `serve` writes ONLY JSON-RPC to stdout (logs go to stderr) so it can be wired
//! straight into an MCP client like Claude Desktop.

use std::path::PathBuf;

use agent_memory::{Memory, MeshConfig};

/// Where this device stores its identity + replica. Override with
/// `CENTRALTABS_MEMORY_DIR`; defaults to `~/.centraltabs/memory`.
fn data_dir() -> PathBuf {
    if let Ok(d) = std::env::var("CENTRALTABS_MEMORY_DIR") {
        return PathBuf::from(d);
    }
    let home = std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".centraltabs").join("memory")
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let cmd = args.get(1).map(String::as_str).unwrap_or("serve");
    let dir = data_dir();

    match cmd {
        // The headline use: an MCP server over stdio that Claude Desktop / Cursor /
        // any MCP client connect to. Logs go to STDERR only.
        "serve" => {
            let mem = Memory::start(MeshConfig::serverless(&dir)).await?;
            eprintln!(
                "agent-memory MCP server ready (device {}, data {})",
                mem.endpoint_id(),
                dir.display()
            );
            mesh_mcp::serve_stdio(mem).await?;
        }
        // Print this device's identity (for pairing / debugging).
        "id" => {
            let mem = Memory::start(MeshConfig::serverless(&dir)).await?;
            println!("{}", mem.endpoint_id());
            mem.shutdown().await?;
        }
        "remember" => {
            let text = args[2..].join(" ");
            if text.is_empty() {
                eprintln!("usage: agent-memory remember <text>");
                return Ok(());
            }
            let mem = Memory::start(MeshConfig::serverless(&dir)).await?;
            let id = mem.remember(&text, "fact").await?;
            println!("remembered: {id}");
            mem.shutdown().await?;
        }
        "recall" => {
            let query = args[2..].join(" ");
            let mem = Memory::start(MeshConfig::serverless(&dir)).await?;
            for e in mem.search(&query).await {
                println!("- {}  [{}]", e.text, e.id);
            }
            mem.shutdown().await?;
        }
        _ => {
            eprintln!("usage: agent-memory [serve | id | remember <text> | recall <query>]");
        }
    }
    Ok(())
}
