//! # agent-memory — the flagship app on [mesh-engine] + [mesh-mcp]
//!
//! Portable, vendor-neutral AI memory that lives on YOUR machines. Facts and
//! preferences are an Automerge replica that syncs conflict-free across your own
//! devices over the mesh (no account, no cloud, end-to-end encrypted), and any AI
//! agent reads/writes the SAME memory through a local MCP server — so your model is
//! swappable and your memory is yours.
//!
//! All the hard parts (P2P sync, identity, pairing, offline tolerance, encryption,
//! the MCP host) come from the engine + mesh-mcp. This crate is just the memory
//! schema ([`model`]) + a thin [`Memory`] facade + the [`mesh_mcp::McpApp`] surface.
//!
//! [mesh-engine]: mesh_engine
//! [mesh-mcp]: mesh_mcp

mod model;

use mesh_engine::{endpoint_addr_from_id, CoreConfig, CoreError, EndpointAddr, Mesh, Result};
pub use model::Entry;

/// Re-exported so callers can configure the engine without a direct dependency.
pub use mesh_engine::CoreConfig as MeshConfig;

/// A running agent-memory peer.
pub struct Memory {
    mesh: Mesh,
}

impl Memory {
    /// Start a memory peer on the given engine configuration.
    pub async fn start(config: CoreConfig) -> Result<Memory> {
        Ok(Memory {
            mesh: Mesh::start(config).await?,
        })
    }

    /// Wrap an already-running engine handle so several apps can share ONE mesh —
    /// one identity, one pairing, one replica. `Mesh` is cheap to clone (`Arc`).
    pub fn from_mesh(mesh: Mesh) -> Memory {
        Memory { mesh }
    }

    /// This device's stable identity / connectable address.
    pub fn endpoint_id(&self) -> String {
        self.mesh.endpoint_id()
    }
    pub fn endpoint_addr(&self) -> EndpointAddr {
        self.mesh.endpoint_addr()
    }

    /// Add a device to keep this memory converged with (continuously, in the background).
    pub async fn add_device(&self, peer: EndpointAddr) {
        self.mesh.add_peer(peer).await;
    }
    /// One-shot sync with a peer now (used by tests / manual flows).
    pub async fn sync_with(&self, peer: impl Into<EndpointAddr>) -> Result<()> {
        self.mesh.sync_with(peer).await
    }
    /// Wait for a relay-reachable address (relay-backed modes).
    pub async fn online(&self) {
        self.mesh.online().await
    }

    // ---- device linking (pairing) ----

    /// Enter "add a device" mode on THIS device: answer one pairing attempt that
    /// presents `code` and hand over the group key, then disarm. Show `code` (or an
    /// invite carrying it) to the user. Pair with [`Memory::pair_with_host`].
    pub fn arm_pairing(&self, code: &str) {
        self.mesh.arm_pairing(code.as_bytes());
    }

    /// Leave "add a device" mode without completing a pairing (e.g. cancelled).
    pub fn disarm_pairing(&self) {
        self.mesh.disarm_pairing();
    }

    /// Join the group hosted by `host_id` using the shared `code`. Persists the group
    /// key; the caller should restart this peer (drop + [`Memory::start`]) to adopt it.
    /// Time-boxed so an offline host fails cleanly instead of hanging.
    pub async fn pair_with_host(&self, host_id: &str, code: &str) -> Result<()> {
        let host = endpoint_addr_from_id(host_id)?;
        match tokio::time::timeout(
            std::time::Duration::from_secs(45),
            self.mesh.pair_with(host, code.as_bytes()),
        )
        .await
        {
            Ok(r) => r,
            Err(_) => Err(CoreError::Pairing(
                "couldn't reach the other device — make sure it's showing a code and online".into(),
            )),
        }
    }

    /// Keep this memory converged with a device addressed by its id (discovery
    /// resolves the paths). Call for each linked device on startup.
    pub async fn add_device_by_id(&self, id: &str) -> Result<()> {
        self.add_device(endpoint_addr_from_id(id)?).await;
        Ok(())
    }

    /// Rotate the group key (revoke access). Restart, then re-pair the devices you keep.
    pub fn rotate_group_key(&self) -> Result<[u8; 32]> {
        self.mesh.rotate_group_key()
    }

    /// Remember something. Returns the entry id.
    pub async fn remember(&self, text: &str, kind: &str) -> Result<String> {
        let id = model::append(&self.mesh.doc(), text, kind).await?;
        self.mesh.save().await?; // durable immediately (not just on the 1.5s loop)
        self.mesh.announce_change();
        Ok(id)
    }
    /// All remembered entries.
    pub async fn all(&self) -> Vec<Entry> {
        model::all(&self.mesh.doc()).await
    }
    /// Search memory (case-insensitive substring).
    pub async fn search(&self, query: &str) -> Vec<Entry> {
        model::search(&self.mesh.doc(), query).await
    }
    /// Forget an entry by id.
    pub async fn forget(&self, id: &str) -> Result<bool> {
        let found = model::forget(&self.mesh.doc(), id).await?;
        if found {
            self.mesh.save().await?;
            self.mesh.announce_change();
        }
        Ok(found)
    }

    pub async fn shutdown(self) -> Result<()> {
        self.mesh.shutdown().await
    }
}

/// MCP surface: any agent (Claude Desktop, Cursor, …) reads + writes the same memory
/// over a local MCP server. Run with `mesh_mcp::serve_stdio(memory).await`.
impl mesh_mcp::McpApp for Memory {
    fn server_name(&self) -> String {
        "agent-memory".to_string()
    }

    fn tools(&self) -> Vec<mesh_mcp::ToolDef> {
        use serde_json::json;
        vec![
            mesh_mcp::ToolDef::new(
                "memory.append",
                "Remember a fact or preference about the user (syncs to all their devices).",
                json!({
                    "type": "object",
                    "properties": { "text": { "type": "string" }, "kind": { "type": "string" } },
                    "required": ["text"]
                }),
            ),
            mesh_mcp::ToolDef::new(
                "memory.search",
                "Search the user's memory for relevant entries.",
                json!({ "type": "object", "properties": { "query": { "type": "string" } }, "required": ["query"] }),
            ),
            mesh_mcp::ToolDef::new(
                "memory.read",
                "List everything the user has remembered.",
                json!({ "type": "object" }),
            ),
            mesh_mcp::ToolDef::new(
                "memory.forget",
                "Forget a memory entry by id.",
                json!({ "type": "object", "properties": { "id": { "type": "string" } }, "required": ["id"] }),
            ),
        ]
    }

    async fn call_tool(
        &self,
        name: &str,
        args: serde_json::Value,
    ) -> std::result::Result<serde_json::Value, String> {
        use serde_json::json;
        let entries_json = |entries: Vec<Entry>| {
            json!(entries
                .into_iter()
                .map(|e| json!({ "id": e.id, "text": e.text, "kind": e.kind }))
                .collect::<Vec<_>>())
        };
        match name {
            "memory.append" => {
                let text = args
                    .get("text")
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.trim().is_empty())
                    .ok_or("missing or empty 'text'")?;
                let kind = args.get("kind").and_then(|v| v.as_str()).unwrap_or("fact");
                let id = self.remember(text, kind).await.map_err(|e| e.to_string())?;
                Ok(json!({ "id": id }))
            }
            "memory.search" => {
                let query = args
                    .get("query")
                    .and_then(|v| v.as_str())
                    .ok_or("missing 'query'")?;
                Ok(json!({ "results": entries_json(self.search(query).await) }))
            }
            "memory.read" => Ok(json!({ "entries": entries_json(self.all().await) })),
            "memory.forget" => {
                let id = args
                    .get("id")
                    .and_then(|v| v.as_str())
                    .ok_or("missing 'id'")?;
                Ok(json!({ "forgotten": self.forget(id).await.map_err(|e| e.to_string())? }))
            }
            other => Err(format!("unknown tool: {other}")),
        }
    }
}
