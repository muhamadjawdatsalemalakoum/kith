//! # centralTabs — the first app on [mesh-engine]
//!
//! Tabs, end-to-end encrypted across a user's own desktops, no account, no server.
//! All the hard parts — P2P transport, identity, discovery, offline-tolerant sync,
//! encryption, pairing — live in [`mesh_engine`]. This crate is *just the app*: a
//! tab data model ([`model`]) on the engine's replicated document, plus a thin
//! [`Tabs`] facade. That thinness is the whole point of the family architecture.
//!
//! [mesh-engine]: mesh_engine

mod model;

use mesh_engine::{CoreConfig, EndpointAddr, Mesh, Result};

// Apps re-export the engine config so callers don't need a direct engine dependency
// just to start one.
pub use mesh_engine::CoreConfig as MeshConfig;

/// A running centralTabs peer: an engine [`Mesh`] with the tab schema on top.
pub struct Tabs {
    mesh: Mesh,
}

impl Tabs {
    /// Start a tabs peer on the given engine configuration.
    pub async fn start(config: CoreConfig) -> Result<Tabs> {
        Ok(Tabs {
            mesh: Mesh::start(config).await?,
        })
    }

    /// This device's stable public identity.
    pub fn endpoint_id(&self) -> String {
        self.mesh.endpoint_id()
    }

    /// This device's connectable address (hand to a peer's [`Tabs::sync_with`]).
    pub fn endpoint_addr(&self) -> EndpointAddr {
        self.mesh.endpoint_addr()
    }

    /// Sync this device's tabs with a peer (one round). Pending, not lost, if the
    /// peer is unreachable.
    pub async fn sync_with(&self, peer: impl Into<EndpointAddr>) -> Result<()> {
        self.mesh.sync_with(peer).await
    }

    /// Seed an example space/group/tab (Phase-0 convenience; real edits land next).
    pub async fn seed_example(&self) -> Result<()> {
        model::seed_example(&self.mesh.doc()).await
    }

    /// The url of the first tab in the replica, if any.
    pub async fn first_tab_url(&self) -> Option<String> {
        model::first_tab_url(&self.mesh.doc()).await
    }

    /// Gracefully stop serving.
    pub async fn shutdown(self) -> Result<()> {
        self.mesh.shutdown().await
    }
}

/// MCP surface: implementing this one trait makes centralTabs' data + actions
/// available to ANY AI agent (Claude Desktop, Cursor, …) over a local MCP server —
/// reading/writing the SAME mesh replica, with no server in the loop. Run it with
/// `mesh_mcp::serve_stdio(tabs).await`.
impl mesh_mcp::McpApp for Tabs {
    fn server_name(&self) -> String {
        "centraltabs".to_string()
    }

    fn tools(&self) -> Vec<mesh_mcp::ToolDef> {
        use serde_json::json;
        vec![
            mesh_mcp::ToolDef::new(
                "tabs.add",
                "Save a tab to the mesh (syncs to all your devices).",
                json!({
                    "type": "object",
                    "properties": { "url": { "type": "string" }, "title": { "type": "string" } },
                    "required": ["url"]
                }),
            ),
            mesh_mcp::ToolDef::new(
                "tabs.count",
                "How many tabs are saved.",
                json!({ "type": "object" }),
            ),
            mesh_mcp::ToolDef::new(
                "tabs.first_url",
                "URL of the first saved tab.",
                json!({ "type": "object" }),
            ),
        ]
    }

    async fn call_tool(
        &self,
        name: &str,
        args: serde_json::Value,
    ) -> std::result::Result<serde_json::Value, String> {
        use serde_json::json;
        match name {
            "tabs.add" => {
                let url = args
                    .get("url")
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.trim().is_empty())
                    .ok_or("missing or empty 'url'")?;
                let title = args.get("title").and_then(|v| v.as_str()).unwrap_or("");
                model::add_tab(&self.mesh.doc(), url, title)
                    .await
                    .map_err(|e| e.to_string())?;
                self.mesh.save().await.map_err(|e| e.to_string())?; // durable now
                self.mesh.announce_change(); // and sync to peers
                Ok(json!({ "ok": true }))
            }
            "tabs.count" => Ok(json!({ "count": model::count_tabs(&self.mesh.doc()).await })),
            "tabs.first_url" => Ok(json!({ "url": model::first_tab_url(&self.mesh.doc()).await })),
            other => Err(format!("unknown tool: {other}")),
        }
    }
}
