//! # kith-files — file sharing across your own devices, on [mesh_engine]
//!
//! A thin app on the engine's BLOB primitive. To share a file, it imports the bytes
//! into the content store and advertises a small *offer* (name, size, content hash,
//! which device has it) in the replicated document. Any linked device sees the offer
//! and pulls the bytes directly, end-to-end encrypted and BLAKE3-verified, with live
//! progress. No tickets and no central server — your circle already shares the group.
//!
//! [mesh_engine]: mesh_engine

mod model;

use std::path::{Path, PathBuf};

use mesh_engine::{endpoint_addr_from_id, hash_from_str, CoreConfig, CoreError, Mesh, Result};
pub use model::FileEntry;

// Apps re-export the engine config so callers don't need a direct engine dependency.
pub use mesh_engine::CoreConfig as MeshConfig;

/// A running file-sharing peer (an engine [`Mesh`] with the file-offer schema on top).
pub struct Files {
    mesh: Mesh,
}

impl Files {
    /// Start a files peer on the given engine configuration. (Enable blobs on the
    /// config — `CoreConfig::with_blobs(true)` — so this device can serve content.)
    pub async fn start(config: CoreConfig) -> Result<Files> {
        Ok(Files {
            mesh: Mesh::start(config).await?,
        })
    }

    /// Wrap an already-running engine handle so this app shares ONE mesh with the rest
    /// of the family (one identity, one pairing, one replica).
    pub fn from_mesh(mesh: Mesh) -> Files {
        Files { mesh }
    }

    /// This device's stable public identity.
    pub fn endpoint_id(&self) -> String {
        self.mesh.endpoint_id()
    }

    /// Offer a local file to the circle: import its bytes and advertise the offer.
    /// The bytes stay served while this peer runs. Returns the created offer.
    pub async fn offer(&self, path: &Path) -> Result<FileEntry> {
        let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        let name = path
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "file".to_string());
        let hash = self.mesh.share_file(path).await?;
        let entry = FileEntry {
            id: uuid::Uuid::new_v4().to_string(),
            name,
            size,
            hash: hash.to_string(),
            from_id: self.mesh.endpoint_id(),
            from_name: device_name(),
        };
        model::add_file(&self.mesh.doc(), &entry).await?;
        self.mesh.save().await?;
        self.mesh.announce_change();
        Ok(entry)
    }

    /// Every current offer across the circle (this device's and linked devices').
    pub async fn all(&self) -> Vec<FileEntry> {
        model::all_files(&self.mesh.doc()).await
    }

    /// Withdraw an offer by id. Returns whether it was found.
    pub async fn forget(&self, id: &str) -> Result<bool> {
        let found = model::remove_file(&self.mesh.doc(), id).await?;
        if found {
            self.mesh.save().await?;
            self.mesh.announce_change();
        }
        Ok(found)
    }

    /// Rename an offer's display name by id. Returns whether it was found.
    pub async fn rename(&self, id: &str, name: &str) -> Result<bool> {
        let found = model::rename_file(&self.mesh.doc(), id, name).await?;
        if found {
            self.mesh.save().await?;
            self.mesh.announce_change();
        }
        Ok(found)
    }

    /// Download `entry` into `dest_dir`, reporting `on_progress(bytes, relayed)`.
    /// Returns the written path.
    pub async fn fetch(
        &self,
        entry: &FileEntry,
        dest_dir: &Path,
        on_progress: impl FnMut(u64, bool),
    ) -> Result<PathBuf> {
        if entry.from_id == self.mesh.endpoint_id() {
            // The bytes are already on this device; no need to fetch from ourselves.
            return Err(CoreError::Other(anyhow::anyhow!(
                "this file is already on this device"
            )));
        }
        let addr = endpoint_addr_from_id(&entry.from_id)?;
        let hash = hash_from_str(&entry.hash)?;
        // Don't clobber an existing file of the same name — disambiguate like a browser.
        let target = unique_path(dest_dir, &sanitize(&entry.name));
        self.mesh
            .fetch_file_with_progress(addr, hash, &target, on_progress)
            .await?;
        Ok(target)
    }

    /// Look up an offer by id and download it into `dest_dir`.
    pub async fn fetch_by_id(
        &self,
        id: &str,
        dest_dir: &Path,
        on_progress: impl FnMut(u64, bool),
    ) -> Result<PathBuf> {
        let entry = self
            .all()
            .await
            .into_iter()
            .find(|e| e.id == id)
            .ok_or_else(|| CoreError::Other(anyhow::anyhow!("file offer not found")))?;
        self.fetch(&entry, dest_dir, on_progress).await
    }
}

fn device_name() -> String {
    std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "a device".to_string())
}

/// A non-colliding path in `dir` for `name`: appends " (1)", " (2)", … before the
/// extension if the file already exists, so a download never overwrites another file.
fn unique_path(dir: &Path, name: &str) -> PathBuf {
    let base = dir.join(name);
    if !base.exists() {
        return base;
    }
    let p = Path::new(name);
    let stem = p
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| name.to_string());
    let ext = p
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();
    for i in 1..1000 {
        let cand = dir.join(format!("{stem} ({i}){ext}"));
        if !cand.exists() {
            return cand;
        }
    }
    base
}

/// Strip path separators from an offered name so a download can't escape `dest_dir`.
fn sanitize(name: &str) -> String {
    let n: String = name
        .chars()
        .filter(|c| !matches!(c, '/' | '\\' | ':'))
        .collect();
    if n.trim().is_empty() {
        "file".to_string()
    } else {
        n
    }
}

/// MCP surface: an agent can offer, list, and download files over the mesh.
impl mesh_mcp::McpApp for Files {
    fn server_name(&self) -> String {
        "kith-files".to_string()
    }

    fn tools(&self) -> Vec<mesh_mcp::ToolDef> {
        use serde_json::json;
        vec![
            mesh_mcp::ToolDef::new(
                "files.share",
                "Offer a local file to the user's other devices (returns its id).",
                json!({ "type": "object", "properties": { "path": { "type": "string" } }, "required": ["path"] }),
            ),
            mesh_mcp::ToolDef::new(
                "files.list",
                "List files offered across the user's devices.",
                json!({ "type": "object" }),
            ),
            mesh_mcp::ToolDef::new(
                "files.fetch",
                "Download an offered file (by id) into a destination folder.",
                json!({ "type": "object", "properties": { "id": { "type": "string" }, "dest": { "type": "string" } }, "required": ["id", "dest"] }),
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
            "files.share" => {
                let path = args
                    .get("path")
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.trim().is_empty())
                    .ok_or("missing or empty 'path'")?;
                let e = self
                    .offer(Path::new(path))
                    .await
                    .map_err(|e| e.to_string())?;
                Ok(json!({ "id": e.id, "name": e.name, "size": e.size, "hash": e.hash }))
            }
            "files.list" => {
                let files: Vec<_> = self
                    .all()
                    .await
                    .into_iter()
                    .map(|e| json!({ "id": e.id, "name": e.name, "size": e.size, "from": e.from_name, "fromId": e.from_id }))
                    .collect();
                Ok(json!({ "files": files }))
            }
            "files.fetch" => {
                let id = args
                    .get("id")
                    .and_then(|v| v.as_str())
                    .ok_or("missing 'id'")?;
                let dest = args
                    .get("dest")
                    .and_then(|v| v.as_str())
                    .ok_or("missing 'dest'")?;
                let p = self
                    .fetch_by_id(id, Path::new(dest), |_, _| {})
                    .await
                    .map_err(|e| e.to_string())?;
                Ok(json!({ "path": p.to_string_lossy() }))
            }
            other => Err(format!("unknown tool: {other}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::sanitize;

    #[test]
    fn sanitize_strips_path_separators() {
        // A malicious offered name can't escape the destination directory.
        assert_eq!(sanitize("../../etc/passwd"), "....etcpasswd");
        assert_eq!(sanitize("a/b\\c:d"), "abcd");
        assert_eq!(sanitize("ok-file.txt"), "ok-file.txt");
        assert_eq!(sanitize(""), "file");
        assert_eq!(sanitize("///"), "file");
    }
}
