//! Kith desktop shell (Tauri v2).
//!
//! A friendly window over the whole Kith platform. It owns the Tauri app, the window,
//! and the command surface, and forwards everything to the verified engine + thin
//! apps. Crucially it runs ONE `mesh_engine::Mesh` (one identity, one pairing, one
//! encrypted replica) and layers all three apps on it via `Memory::from_mesh` /
//! `Tabs::from_mesh` / `Files::from_mesh` — so memory, tabs, and files sync together
//! with a single device link, and `kith serve` exposes them all over MCP. No `iroh` /
//! `automerge` types appear here — only stable app types and strings.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::time::Duration;

use agent_memory::{Entry, Memory};
use centraltabs::{Tab, Tabs};
use kith_files::Files;
use mesh_engine::{endpoint_addr_from_id, CoreConfig, Mesh, Role, SpaceId};
use mesh_mcp::McpApp;
use serde::{Deserialize, Serialize};
use tauri::ipc::Channel;
use tauri::{AppHandle, Manager, State};
use tauri_plugin_dialog::DialogExt;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

/// The single shared engine handle. `Arc` so the MCP bridge always reads the CURRENT
/// engine even after a restart (pairing / leave-group swap it in place).
type MeshHolder = Arc<Mutex<Option<Mesh>>>;

/// Long-lived app state.
struct AppState {
    /// `None` only briefly while the engine restarts (after joining/leaving a group).
    mesh: MeshHolder,
    data_dir: PathBuf,
    pairing: StdMutex<Option<String>>,
    /// In-flight downloads, so they can be cancelled by id.
    downloads: StdMutex<HashMap<String, CancellationToken>>,
}

#[derive(Serialize)]
struct NoteDto {
    id: String,
    text: String,
    kind: String,
}
impl From<Entry> for NoteDto {
    fn from(e: Entry) -> Self {
        Self {
            id: e.id,
            text: e.text,
            kind: e.kind,
        }
    }
}

#[derive(Serialize)]
struct TabDto {
    id: String,
    url: String,
    title: String,
}
impl From<Tab> for TabDto {
    fn from(t: Tab) -> Self {
        Self {
            id: t.id,
            url: t.url,
            title: t.title,
        }
    }
}

/// A file offer, as the UI sees it (`mine` = offered by this device;
/// `local_path` set when the bytes are on THIS machine — the original you offered,
/// or a copy you already downloaded — so the UI can "open location").
#[derive(Serialize)]
struct FileDto {
    id: String,
    name: String,
    size: u64,
    from: String,
    mine: bool,
    local_path: Option<String>,
}

/// Live download events streamed to the UI over a Tauri channel.
#[derive(Serialize, Clone)]
#[serde(tag = "kind", rename_all = "camelCase")]
enum DlEvent {
    Transferring {
        offset: u64,
        total: u64,
        relayed: bool,
    },
    Done {
        path: String,
    },
    Error {
        message: String,
    },
    Cancelled,
}

/// A device in the user's circle (this one, or a linked one).
#[derive(Serialize, Deserialize, Clone)]
struct Device {
    id: String,
    name: String,
}

/// One completed transfer, recorded locally for the Files "Recent" list.
#[derive(Serialize, Deserialize, Clone)]
struct HistoryEntry {
    id: String,
    name: String,
    size: u64,
    /// "sent" (you offered it) or "received" (you downloaded it).
    direction: String,
    /// "Your devices" for a send, or the offering device's name for a receive.
    peer: String,
    /// Unix seconds when it was recorded.
    ts: u64,
    /// Local path, if the bytes are on this machine.
    path: Option<String>,
}

/// The "add a device" payload shown on the host: a copyable invite the other
/// computer pastes into "Enter a code".
#[derive(Serialize)]
struct LinkInfo {
    invite: String,
}

/// A linked device for the UI, with sync recency (`synced_ago` = seconds since the
/// last successful sync, `None` = not yet synced this session).
#[derive(Serialize)]
struct LinkedDevice {
    id: String,
    name: String,
    synced_ago: Option<u64>,
}

#[derive(Serialize)]
struct DevicesDto {
    me: Device,
    linked: Vec<LinkedDevice>,
}

/// What an AI assistant can do with Kith over MCP (shown on the Agents screen).
#[derive(Serialize)]
struct AgentInfo {
    /// Ready-to-paste MCP client config (Claude Desktop / Cursor).
    config: String,
    /// Resolved path to the `agent-memory` MCP server binary, if found next to us.
    binary: Option<String>,
    tools: Vec<ToolInfo>,
}

#[derive(Serialize)]
struct ToolInfo {
    name: String,
    description: String,
}

// --------------------------------------------------------------------------- helpers

/// Kith's data directory — shared with the `agent-memory` CLI and any MCP agents, so
/// the GUI shows the same memory. Mirrors the CLI's resolution (incl. legacy paths).
fn kith_data_dir() -> PathBuf {
    if let Ok(d) = std::env::var("KITH_MEMORY_DIR") {
        return PathBuf::from(d);
    }
    if let Ok(d) = std::env::var("CENTRALTABS_MEMORY_DIR") {
        return PathBuf::from(d);
    }
    let home = PathBuf::from(
        std::env::var("USERPROFILE")
            .or_else(|_| std::env::var("HOME"))
            .unwrap_or_else(|_| ".".to_string()),
    );
    let current = home.join(".kith").join("memory");
    let legacy = home.join(".centraltabs").join("memory");
    if !current.exists() && legacy.exists() {
        legacy
    } else {
        current
    }
}

fn this_device_name() -> String {
    std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "This device".to_string())
}

/// A short, human-typeable link code from the friendly alphabet (no look-alikes).
fn gen_code() -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
    let uuid = uuid::Uuid::new_v4();
    let b = uuid.as_bytes();
    (0..8)
        .map(|i| ALPHABET[(b[i] as usize) % ALPHABET.len()] as char)
        .collect()
}

fn devices_path(dir: &Path) -> PathBuf {
    dir.join("devices.json")
}
fn load_devices(dir: &Path) -> Vec<Device> {
    std::fs::read(devices_path(dir))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}
fn save_devices(dir: &Path, devices: &[Device]) {
    if let Ok(json) = serde_json::to_vec_pretty(devices) {
        let _ = std::fs::write(devices_path(dir), json);
    }
}
fn upsert_device(dir: &Path, dev: &Device) {
    let mut list = load_devices(dir);
    if let Some(existing) = list.iter_mut().find(|d| d.id == dev.id) {
        existing.name = dev.name.clone();
    } else {
        list.push(dev.clone());
    }
    save_devices(dir, &list);
}

/// Where received files land by default: ~/Downloads/Kith.
fn downloads_dir() -> PathBuf {
    let home = std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join("Downloads").join("Kith")
}

/// The user's chosen download folder (Settings), or the default.
fn effective_download_dir(data_dir: &Path) -> PathBuf {
    load_map(data_dir, "settings.json")
        .get("download_dir")
        .map(PathBuf::from)
        .unwrap_or_else(downloads_dir)
}

/// Append crashes to `kith.log` in the data dir (with the default hook still firing),
/// so a field crash leaves a trace even with `panic = "abort"` and no console.
fn install_panic_logger(data_dir: &Path) {
    let log = data_dir.join("kith.log");
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log)
        {
            use std::io::Write;
            let secs = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            let _ = writeln!(f, "[{secs}] panic: {info}");
        }
        prev(info);
    }));
}

// Local-only id→path maps so the UI can reveal a file's location. Kept on disk (not
// in the synced doc) so a device's local paths never leave the machine.
fn load_map(dir: &Path, file: &str) -> HashMap<String, String> {
    std::fs::read(dir.join(file))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}
fn map_insert(dir: &Path, file: &str, key: &str, value: &str) {
    let mut m = load_map(dir, file);
    m.insert(key.to_string(), value.to_string());
    if let Ok(j) = serde_json::to_vec_pretty(&m) {
        let _ = std::fs::write(dir.join(file), j);
    }
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn load_history(dir: &Path) -> Vec<HistoryEntry> {
    std::fs::read(dir.join("history.json"))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}
/// Append a completed transfer to the local history log (capped, oldest dropped).
fn append_history(dir: &Path, entry: HistoryEntry) {
    let mut h = load_history(dir);
    h.push(entry);
    let n = h.len();
    if n > 200 {
        h.drain(0..n - 200);
    }
    if let Ok(j) = serde_json::to_vec_pretty(&h) {
        let _ = std::fs::write(dir.join("history.json"), j);
    }
}

// -------------------------------------------------------------------------- commands

#[tauri::command]
fn app_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

#[tauri::command]
async fn my_endpoint_id(state: State<'_, AppState>) -> Result<String, String> {
    let guard = state.mesh.lock().await;
    let mesh = guard.as_ref().ok_or("Kith is still starting…")?;
    Ok(mesh.endpoint_id())
}

#[tauri::command]
async fn this_device(state: State<'_, AppState>) -> Result<Device, String> {
    let guard = state.mesh.lock().await;
    let mesh = guard.as_ref().ok_or("Kith is still starting…")?;
    Ok(Device {
        id: mesh.endpoint_id(),
        name: this_device_name(),
    })
}

// ---- Memory app ----

#[tauri::command]
async fn list_notes(state: State<'_, AppState>) -> Result<Vec<NoteDto>, String> {
    let guard = state.mesh.lock().await;
    let mesh = guard.as_ref().ok_or("Kith is still starting…")?;
    let mem = Memory::from_mesh(mesh.clone());
    Ok(mem.all().await.into_iter().map(NoteDto::from).collect())
}

#[tauri::command]
async fn add_note(
    text: String,
    kind: Option<String>,
    state: State<'_, AppState>,
) -> Result<NoteDto, String> {
    let text = text.trim().to_string();
    if text.is_empty() {
        return Err("Type something to remember first.".into());
    }
    let kind = kind
        .map(|k| k.trim().to_string())
        .filter(|k| !k.is_empty())
        .unwrap_or_else(|| "note".to_string());
    let guard = state.mesh.lock().await;
    let mesh = guard.as_ref().ok_or("Kith is still starting…")?;
    let mem = Memory::from_mesh(mesh.clone());
    let id = mem
        .remember(&text, &kind)
        .await
        .map_err(|e| e.to_string())?;
    Ok(NoteDto { id, text, kind })
}

#[tauri::command]
async fn search_notes(query: String, state: State<'_, AppState>) -> Result<Vec<NoteDto>, String> {
    let guard = state.mesh.lock().await;
    let mesh = guard.as_ref().ok_or("Kith is still starting…")?;
    let mem = Memory::from_mesh(mesh.clone());
    Ok(mem
        .search(&query)
        .await
        .into_iter()
        .map(NoteDto::from)
        .collect())
}

#[tauri::command]
async fn forget_note(id: String, state: State<'_, AppState>) -> Result<bool, String> {
    let guard = state.mesh.lock().await;
    let mesh = guard.as_ref().ok_or("Kith is still starting…")?;
    let mem = Memory::from_mesh(mesh.clone());
    mem.forget(&id).await.map_err(|e| e.to_string())
}

// ---- Tabs app ----

#[tauri::command]
async fn list_tabs(state: State<'_, AppState>) -> Result<Vec<TabDto>, String> {
    let guard = state.mesh.lock().await;
    let mesh = guard.as_ref().ok_or("Kith is still starting…")?;
    let tabs = Tabs::from_mesh(mesh.clone());
    Ok(tabs.all().await.into_iter().map(TabDto::from).collect())
}

#[tauri::command]
async fn add_tab(
    url: String,
    title: Option<String>,
    state: State<'_, AppState>,
) -> Result<TabDto, String> {
    let url = url.trim().to_string();
    if url.is_empty() {
        return Err("Paste a link to save first.".into());
    }
    let title = title.map(|t| t.trim().to_string()).unwrap_or_default();
    let guard = state.mesh.lock().await;
    let mesh = guard.as_ref().ok_or("Kith is still starting…")?;
    let tabs = Tabs::from_mesh(mesh.clone());
    let id = tabs.add(&url, &title).await.map_err(|e| e.to_string())?;
    Ok(TabDto { id, url, title })
}

#[tauri::command]
async fn forget_tab(id: String, state: State<'_, AppState>) -> Result<bool, String> {
    let guard = state.mesh.lock().await;
    let mesh = guard.as_ref().ok_or("Kith is still starting…")?;
    let tabs = Tabs::from_mesh(mesh.clone());
    tabs.forget(&id).await.map_err(|e| e.to_string())
}

// ---- Files app ----

/// Grab a clone of the running engine and release the state lock, so long file
/// transfers run concurrently instead of serializing on the lock.
async fn mesh_handle(state: &State<'_, AppState>) -> Result<Mesh, String> {
    let guard = state.mesh.lock().await;
    Ok(guard.as_ref().ok_or("Kith is still starting…")?.clone())
}

#[tauri::command]
async fn list_files(state: State<'_, AppState>) -> Result<Vec<FileDto>, String> {
    let mesh = mesh_handle(&state).await?;
    let me = mesh.endpoint_id();
    let offered = load_map(&state.data_dir, "offered_paths.json");
    let downloads = load_map(&state.data_dir, "downloads.json");
    let files = Files::from_mesh(mesh);
    Ok(files
        .all()
        .await
        .into_iter()
        .map(|e| {
            let mine = e.from_id == me;
            // The original you offered (mine) or the copy you downloaded (remote).
            let local_path = if mine {
                offered.get(&e.id).cloned()
            } else {
                downloads.get(&e.id).cloned()
            }
            .filter(|p| Path::new(p).exists());
            FileDto {
                mine,
                local_path,
                id: e.id,
                name: e.name,
                size: e.size,
                from: e.from_name,
            }
        })
        .collect())
}

#[tauri::command]
async fn offer_file(app: AppHandle, state: State<'_, AppState>) -> Result<Option<FileDto>, String> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog().file().pick_file(move |f| {
        let _ = tx.send(f);
    });
    let picked = rx.await.map_err(|e| e.to_string())?;
    let Some(fp) = picked else {
        return Ok(None); // user cancelled
    };
    let path = fp.into_path().map_err(|e| e.to_string())?;
    let mesh = mesh_handle(&state).await?;
    let me = mesh.endpoint_id();
    let files = Files::from_mesh(mesh);
    let e = files.offer(&path).await.map_err(|e| e.to_string())?;
    let local_path = path.to_string_lossy().into_owned();
    map_insert(&state.data_dir, "offered_paths.json", &e.id, &local_path);
    append_history(
        &state.data_dir,
        HistoryEntry {
            id: uuid::Uuid::new_v4().to_string(),
            name: e.name.clone(),
            size: e.size,
            direction: "sent".into(),
            peer: "Your devices".into(),
            ts: now_secs(),
            path: Some(local_path.clone()),
        },
    );
    Ok(Some(FileDto {
        mine: e.from_id == me,
        local_path: Some(local_path),
        id: e.id,
        name: e.name,
        size: e.size,
        from: e.from_name,
    }))
}

#[tauri::command]
async fn rename_file(id: String, name: String, state: State<'_, AppState>) -> Result<bool, String> {
    let name = name.trim().to_string();
    if name.is_empty() {
        return Err("Give the file a name.".into());
    }
    let mesh = mesh_handle(&state).await?;
    Files::from_mesh(mesh)
        .rename(&id, &name)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn forget_file(id: String, state: State<'_, AppState>) -> Result<bool, String> {
    let mesh = mesh_handle(&state).await?;
    Files::from_mesh(mesh)
        .forget(&id)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn download_file(
    id: String,
    on_event: Channel<DlEvent>,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let mesh = mesh_handle(&state).await?;
    let files = Files::from_mesh(mesh);
    let entry = files
        .all()
        .await
        .into_iter()
        .find(|e| e.id == id)
        .ok_or("That file is no longer offered.")?;
    let total = entry.size;
    let dest_dir = effective_download_dir(&state.data_dir);
    std::fs::create_dir_all(&dest_dir).map_err(|e| e.to_string())?;

    // Register a cancellation token so the UI can stop this transfer by id.
    let token = CancellationToken::new();
    state
        .downloads
        .lock()
        .unwrap()
        .insert(id.clone(), token.clone());

    let fetch = files.fetch(&entry, &dest_dir, |offset, relayed| {
        let _ = on_event.send(DlEvent::Transferring {
            offset,
            total,
            relayed,
        });
    });
    // Race the transfer against cancellation; cancelling drops the fetch future, which
    // aborts the in-flight get (partial data stays for resume; no dest file is written).
    let result = tokio::select! {
        r = fetch => Some(r),
        _ = token.cancelled() => None,
    };
    state.downloads.lock().unwrap().remove(&id);

    match result {
        None => {
            let _ = on_event.send(DlEvent::Cancelled);
            Ok(())
        }
        Some(Ok(path)) => {
            let ps = path.to_string_lossy().into_owned();
            map_insert(&state.data_dir, "downloads.json", &id, &ps);
            append_history(
                &state.data_dir,
                HistoryEntry {
                    id: uuid::Uuid::new_v4().to_string(),
                    name: entry.name.clone(),
                    size: entry.size,
                    direction: "received".into(),
                    peer: entry.from_name.clone(),
                    ts: now_secs(),
                    path: Some(ps.clone()),
                },
            );
            let _ = on_event.send(DlEvent::Done { path: ps });
            Ok(())
        }
        Some(Err(e)) => {
            let _ = on_event.send(DlEvent::Error {
                message: e.to_string(),
            });
            Err(e.to_string())
        }
    }
}

/// Cancel an in-flight download by file id.
#[tauri::command]
fn cancel_download(id: String, state: State<'_, AppState>) {
    if let Some(token) = state.downloads.lock().unwrap().get(&id) {
        token.cancel();
    }
}

// ---- Agents (MCP) ----

#[tauri::command]
async fn agent_info(state: State<'_, AppState>) -> Result<AgentInfo, String> {
    // The unified MCP server IS this binary, run as `kith serve`.
    let binary = std::env::current_exe()
        .ok()
        .map(|p| p.to_string_lossy().into_owned());
    let command = binary.clone().unwrap_or_else(|| "kith".to_string());
    let config = serde_json::to_string_pretty(&serde_json::json!({
        "mcpServers": {
            "kith": { "command": command, "args": ["serve"] }
        }
    }))
    .unwrap_or_default();
    // Generate the tool list from the ACTUAL server surface so it can never drift.
    let mesh = mesh_handle(&state).await?;
    let tools = kith_app(&mesh)
        .tools()
        .into_iter()
        .map(|t| ToolInfo {
            name: t.name,
            description: t.description,
        })
        .collect();
    Ok(AgentInfo {
        config,
        binary,
        tools,
    })
}

// ---- linking / devices ----

#[tauri::command]
async fn start_link(state: State<'_, AppState>) -> Result<LinkInfo, String> {
    let guard = state.mesh.lock().await;
    let mesh = guard.as_ref().ok_or("Kith is still starting…")?;
    mesh.online().await; // become relay-reachable so the other computer can find us
    let id = mesh.endpoint_id();
    let code = gen_code();
    mesh.arm_pairing(code.as_bytes());
    let invite = format!("{id}:{code}");
    *state.pairing.lock().unwrap() = Some(code);
    Ok(LinkInfo { invite })
}

#[tauri::command]
async fn cancel_link(state: State<'_, AppState>) -> Result<(), String> {
    let guard = state.mesh.lock().await;
    if let Some(mesh) = guard.as_ref() {
        mesh.disarm_pairing();
    }
    *state.pairing.lock().unwrap() = None;
    Ok(())
}

/// Host side: poll for a device that just linked in. When a device completes pairing,
/// the engine has already peered with it; here we persist it locally and hand it to
/// the UI so the host's Devices list reflects the link (not just the joiner's).
#[tauri::command]
async fn poll_pairing(state: State<'_, AppState>) -> Result<Option<Device>, String> {
    let mesh = mesh_handle(&state).await?;
    match mesh.take_joined() {
        Some(id) => {
            let dev = Device {
                id,
                name: "Linked device".to_string(),
            };
            upsert_device(&state.data_dir, &dev);
            *state.pairing.lock().unwrap() = None;
            Ok(Some(dev))
        }
        None => Ok(None),
    }
}

#[tauri::command]
async fn join_link(
    invite: String,
    name: Option<String>,
    state: State<'_, AppState>,
) -> Result<Device, String> {
    let (host_id, code) = invite
        .trim()
        .rsplit_once(':')
        .ok_or("That code doesn't look right. Copy the whole code from the other computer.")?;
    let host_id = host_id.trim().to_string();
    let code = code.trim().to_string();
    if host_id.is_empty() || code.is_empty() {
        return Err(
            "That code doesn't look right. Copy the whole code from the other computer.".into(),
        );
    }
    let host = endpoint_addr_from_id(&host_id).map_err(|e| e.to_string())?;

    let mut guard = state.mesh.lock().await;
    {
        let mesh = guard.as_ref().ok_or("Kith is still starting…")?;
        match tokio::time::timeout(
            Duration::from_secs(45),
            mesh.pair_with(host, code.as_bytes()),
        )
        .await
        {
            Ok(r) => r.map_err(|e| e.to_string())?,
            Err(_) => {
                return Err(
                    "Couldn't reach the other computer — make sure it's showing a code and online."
                        .into(),
                )
            }
        }
    }

    // Adopt the received group key by restarting the engine.
    if let Some(old) = guard.take() {
        let _ = old.shutdown().await;
    }
    let new = Mesh::start(effective_core_config(&state.data_dir))
        .await
        .map_err(|e| e.to_string())?;

    let dev = Device {
        id: host_id.clone(),
        name: name
            .map(|n| n.trim().to_string())
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| "Linked device".to_string()),
    };
    upsert_device(&state.data_dir, &dev);
    for d in load_devices(&state.data_dir) {
        if let Ok(addr) = endpoint_addr_from_id(&d.id) {
            new.add_peer(addr).await;
        }
    }
    *guard = Some(new);
    Ok(dev)
}

#[tauri::command]
async fn list_devices(state: State<'_, AppState>) -> Result<DevicesDto, String> {
    let guard = state.mesh.lock().await;
    let mesh = guard.as_ref().ok_or("Kith is still starting…")?;
    let me = Device {
        id: mesh.endpoint_id(),
        name: this_device_name(),
    };
    let ages = mesh.last_sync_ages();
    let linked = load_devices(&state.data_dir)
        .into_iter()
        .map(|d| LinkedDevice {
            synced_ago: ages.get(&d.id).copied(),
            id: d.id,
            name: d.name,
        })
        .collect();
    Ok(DevicesDto { me, linked })
}

#[tauri::command]
fn rename_device(id: String, name: String, state: State<'_, AppState>) -> Result<(), String> {
    let name = name.trim().to_string();
    if name.is_empty() {
        return Err("Give the device a name.".into());
    }
    upsert_device(&state.data_dir, &Device { id, name });
    Ok(())
}

#[tauri::command]
async fn unlink_device(id: String, state: State<'_, AppState>) -> Result<(), String> {
    let mut list = load_devices(&state.data_dir);
    list.retain(|d| d.id != id);
    save_devices(&state.data_dir, &list);
    // Actually stop syncing with it now (not just on next restart).
    if let Ok(mesh) = mesh_handle(&state).await {
        mesh.remove_peer(&id).await;
    }
    Ok(())
}

/// Leave the group and re-key: rotates the group key (so removed devices can no longer
/// authenticate), forgets all linked devices, and restarts the engine to adopt the new
/// key. This is the real "disconnect everything" — you re-pair the devices you keep.
#[tauri::command]
async fn leave_group(state: State<'_, AppState>) -> Result<(), String> {
    let mut guard = state.mesh.lock().await;
    {
        let mesh = guard.as_ref().ok_or("Kith is still starting…")?;
        mesh.rotate_group_key().map_err(|e| e.to_string())?;
    }
    save_devices(&state.data_dir, &[]);
    if let Some(old) = guard.take() {
        let _ = old.shutdown().await;
    }
    let new = Mesh::start(effective_core_config(&state.data_dir))
        .await
        .map_err(|e| e.to_string())?;
    *guard = Some(new);
    Ok(())
}

/// Open a folder/file location in the OS file manager. Only reveals a path that
/// actually exists (the path is supplied by our own UI from the local id→path maps,
/// but the filename component can be remote-influenced, so validate before spawning).
#[tauri::command]
fn reveal_path(path: String) {
    let p = Path::new(&path);
    if !p.exists() {
        return;
    }
    #[cfg(target_os = "windows")]
    {
        // No shell is involved (args are passed directly), and on a non-existent path
        // we already returned — so reveal the file, falling back to its folder.
        let _ = std::process::Command::new("explorer")
            .arg(format!("/select,{path}"))
            .spawn();
    }
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open")
        .arg("-R")
        .arg(&path)
        .spawn();
    #[cfg(target_os = "linux")]
    let _ = std::process::Command::new("xdg-open").arg(&path).spawn();
}

/// Open a web link in the user's real browser (never inside the app webview).
#[tauri::command]
fn open_external(url: String) {
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return;
    }
    #[cfg(target_os = "windows")]
    let _ = std::process::Command::new("explorer").arg(&url).spawn();
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open").arg(&url).spawn();
    #[cfg(target_os = "linux")]
    let _ = std::process::Command::new("xdg-open").arg(&url).spawn();
}

// ----------------------------------------------------------------------- settings

#[derive(Serialize)]
struct Settings {
    data_dir: String,
    download_dir: String,
    /// How at-rest / group keys are stored, for display.
    key_storage: String,
}

#[tauri::command]
fn get_settings(state: State<'_, AppState>) -> Settings {
    // The desktop always opts into the keychain; on Windows/macOS keys live in the OS
    // keychain, on Linux (no backend compiled) they fall back to the hardened key file.
    let key_storage = if cfg!(target_os = "windows") {
        "Windows Credential Manager".to_string()
    } else if cfg!(target_os = "macos") {
        "macOS Keychain".to_string()
    } else {
        "Hardened key file (no OS keychain on this platform)".to_string()
    };
    Settings {
        data_dir: state.data_dir.to_string_lossy().into_owned(),
        download_dir: effective_download_dir(&state.data_dir)
            .to_string_lossy()
            .into_owned(),
        key_storage,
    }
}

/// Pick + persist the folder downloads are saved to. Returns the chosen path, or
/// `None` if cancelled.
#[tauri::command]
async fn set_download_dir(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<Option<String>, String> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog().file().pick_folder(move |f| {
        let _ = tx.send(f);
    });
    let picked = rx.await.map_err(|e| e.to_string())?;
    let Some(fp) = picked else {
        return Ok(None);
    };
    let path = fp.into_path().map_err(|e| e.to_string())?;
    let s = path.to_string_lossy().into_owned();
    map_insert(&state.data_dir, "settings.json", "download_dir", &s);
    Ok(Some(s))
}

// ----------------------------------------------------------------- transfer history

#[tauri::command]
fn list_history(state: State<'_, AppState>) -> Vec<HistoryEntry> {
    let mut h = load_history(&state.data_dir);
    h.reverse(); // newest first
    h
}

#[tauri::command]
fn clear_history(state: State<'_, AppState>) {
    let _ = std::fs::write(state.data_dir.join("history.json"), b"[]");
}

// --------------------------------------------------------------------- Spaces

/// A Space as the UI sees it.
#[derive(Serialize)]
struct SpaceDto {
    id: String,
    name: String,
    is_default: bool,
    is_active: bool,
    /// Role-enforced ("team") Space vs a personal/permissive one.
    enforced: bool,
    /// This device's role in a team Space ("admin"/"writer"/"reader"), else null.
    role: Option<String>,
    epoch: u64,
    members: u32,
}

#[derive(Serialize)]
struct MemberDto {
    id: String,
    role: String,
    is_me: bool,
}

#[derive(Serialize)]
struct AuditDto {
    seq: u64,
    epoch: u64,
    signer: String,
    action: String,
    target: String,
}

fn parse_space(id: &str) -> Result<SpaceId, String> {
    SpaceId::parse(id.trim()).ok_or_else(|| "That space id isn't valid.".to_string())
}

fn role_from_str(s: &str) -> Result<Role, String> {
    match s.trim().to_lowercase().as_str() {
        "admin" => Ok(Role::Admin),
        "writer" => Ok(Role::Writer),
        "reader" => Ok(Role::Reader),
        other => Err(format!("unknown role: {other}")),
    }
}

fn space_dto(mesh: &Mesh, id: SpaceId) -> Option<SpaceDto> {
    let info = mesh.list_spaces().into_iter().find(|i| i.id == id)?;
    let role = mesh.my_role(id);
    let members = mesh.members(id);
    Some(SpaceDto {
        id: id.to_string(),
        name: info.name,
        is_default: info.is_default,
        is_active: id == mesh.active_space(),
        enforced: role.is_some() || !members.is_empty(),
        role: role.map(|r| r.as_str().to_string()),
        epoch: mesh.space_epoch(id),
        members: members.len() as u32,
    })
}

#[tauri::command]
async fn list_spaces(state: State<'_, AppState>) -> Result<Vec<SpaceDto>, String> {
    let mesh = mesh_handle(&state).await?;
    let active = mesh.active_space();
    Ok(mesh
        .list_spaces()
        .into_iter()
        .map(|info| {
            let role = mesh.my_role(info.id);
            let members = mesh.members(info.id);
            SpaceDto {
                is_active: info.id == active,
                enforced: role.is_some() || !members.is_empty(),
                role: role.map(|r| r.as_str().to_string()),
                epoch: mesh.space_epoch(info.id),
                members: members.len() as u32,
                id: info.id.to_string(),
                name: info.name,
                is_default: info.is_default,
            }
        })
        .collect())
}

#[tauri::command]
async fn switch_space(id: String, state: State<'_, AppState>) -> Result<(), String> {
    let mesh = mesh_handle(&state).await?;
    let sid = parse_space(&id)?;
    if mesh.set_active_space(sid) {
        Ok(())
    } else {
        Err("That space isn't on this device.".into())
    }
}

#[tauri::command]
async fn create_space(
    name: String,
    team: bool,
    state: State<'_, AppState>,
) -> Result<SpaceDto, String> {
    let name = name.trim().to_string();
    if name.is_empty() {
        return Err("Give the space a name.".into());
    }
    let mesh = mesh_handle(&state).await?;
    let id = if team {
        mesh.create_space_with_roles(&name)
            .await
            .map_err(|e| e.to_string())?
    } else {
        mesh.create_space(&name).await.map_err(|e| e.to_string())?
    };
    space_dto(&mesh, id).ok_or_else(|| "couldn't read the new space".into())
}

#[tauri::command]
async fn leave_space(id: String, state: State<'_, AppState>) -> Result<bool, String> {
    let mesh = mesh_handle(&state).await?;
    let sid = parse_space(&id)?;
    mesh.leave_space(sid).await.map_err(|e| e.to_string())
}

#[tauri::command]
async fn space_members(id: String, state: State<'_, AppState>) -> Result<Vec<MemberDto>, String> {
    let mesh = mesh_handle(&state).await?;
    let sid = parse_space(&id)?;
    let me = mesh.endpoint_id();
    Ok(mesh
        .members(sid)
        .into_iter()
        .map(|(endpoint, role)| MemberDto {
            is_me: endpoint == me,
            role: role.as_str().to_string(),
            id: endpoint,
        })
        .collect())
}

#[tauri::command]
async fn space_add_member(
    id: String,
    endpoint: String,
    role: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let mesh = mesh_handle(&state).await?;
    let sid = parse_space(&id)?;
    let role = role_from_str(&role)?;
    mesh.add_member(sid, endpoint.trim(), role)
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn space_set_role(
    id: String,
    endpoint: String,
    role: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let mesh = mesh_handle(&state).await?;
    let sid = parse_space(&id)?;
    let role = role_from_str(&role)?;
    mesh.set_member_role(sid, endpoint.trim(), role)
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn space_remove_member(
    id: String,
    endpoint: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let mesh = mesh_handle(&state).await?;
    let sid = parse_space(&id)?;
    mesh.remove_member(sid, endpoint.trim())
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn space_audit(id: String, state: State<'_, AppState>) -> Result<Vec<AuditDto>, String> {
    let mesh = mesh_handle(&state).await?;
    let sid = parse_space(&id)?;
    Ok(mesh
        .audit_log(sid)
        .into_iter()
        .map(|e| AuditDto {
            seq: e.seq,
            epoch: e.epoch,
            signer: e.signer,
            action: e.action,
            target: e.target.unwrap_or_default(),
        })
        .collect())
}

/// Export a Space to an encrypted `.kithspace` bundle (passphrase-protected). Opens a save
/// dialog; returns the chosen path, or `None` if cancelled.
#[tauri::command]
async fn space_export(
    app: AppHandle,
    id: String,
    passphrase: String,
    state: State<'_, AppState>,
) -> Result<Option<String>, String> {
    if passphrase.trim().len() < 6 {
        return Err("Choose a passphrase of at least 6 characters.".into());
    }
    let mesh = mesh_handle(&state).await?;
    let sid = parse_space(&id)?;
    let bundle = mesh
        .export_space(sid, &passphrase)
        .await
        .map_err(|e| e.to_string())?;

    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog()
        .file()
        .add_filter("Kith space", &["kithspace"])
        .set_file_name("kith-space.kithspace")
        .save_file(move |f| {
            let _ = tx.send(f);
        });
    let Some(fp) = rx.await.map_err(|e| e.to_string())? else {
        return Ok(None);
    };
    let path = fp.into_path().map_err(|e| e.to_string())?;
    std::fs::write(&path, &bundle).map_err(|e| e.to_string())?;
    Ok(Some(path.to_string_lossy().into_owned()))
}

/// Import an encrypted `.kithspace` bundle (recovery / move to this device). Opens a file
/// dialog; returns the restored Space.
#[tauri::command]
async fn space_import(
    app: AppHandle,
    passphrase: String,
    state: State<'_, AppState>,
) -> Result<SpaceDto, String> {
    let mesh = mesh_handle(&state).await?;
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog()
        .file()
        .add_filter("Kith space", &["kithspace"])
        .pick_file(move |f| {
            let _ = tx.send(f);
        });
    let Some(fp) = rx.await.map_err(|e| e.to_string())? else {
        return Err("No file chosen.".into());
    };
    let path = fp.into_path().map_err(|e| e.to_string())?;
    let bytes = std::fs::read(&path).map_err(|e| e.to_string())?;
    let sid = mesh
        .import_space(&bytes, &passphrase)
        .await
        .map_err(|e| e.to_string())?;
    space_dto(&mesh, sid).ok_or_else(|| "couldn't read the imported space".into())
}

// --------------------------------------------------------------- network (infra)

#[derive(Serialize, Deserialize, Default, Clone)]
struct NetworkSettings {
    /// "decentralized" (default) or "self_hosted".
    #[serde(default)]
    mode: String,
    #[serde(default)]
    relay_url: String,
    #[serde(default)]
    relay_token: String,
    #[serde(default)]
    pkarr_relay: String,
    #[serde(default)]
    origin_domain: String,
}

fn load_network(dir: &Path) -> NetworkSettings {
    std::fs::read(dir.join("network.json"))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

/// The engine config from the persisted network setting, with blobs + keychain enabled.
fn effective_core_config(data_dir: &Path) -> CoreConfig {
    let n = load_network(data_dir);
    let base = if n.mode == "self_hosted" && !n.relay_url.trim().is_empty() {
        CoreConfig::self_hosted(
            data_dir,
            n.relay_url,
            n.relay_token,
            n.pkarr_relay,
            n.origin_domain,
        )
    } else {
        CoreConfig::serverless(data_dir)
    };
    base.with_blobs(true).with_keychain(true)
}

#[tauri::command]
fn get_network(state: State<'_, AppState>) -> NetworkSettings {
    let mut n = load_network(&state.data_dir);
    if n.mode.is_empty() {
        n.mode = "decentralized".into();
    }
    n
}

/// Persist a network choice and restart the engine on it. Switching to self-hosted requires
/// a relay URL.
#[tauri::command]
async fn set_network(settings: NetworkSettings, state: State<'_, AppState>) -> Result<(), String> {
    if settings.mode == "self_hosted" && settings.relay_url.trim().is_empty() {
        return Err("Enter your relay URL (e.g. https://relay.example.org/).".into());
    }
    let json = serde_json::to_vec_pretty(&settings).map_err(|e| e.to_string())?;
    std::fs::write(state.data_dir.join("network.json"), json).map_err(|e| e.to_string())?;

    // Restart the engine on the new infra (mirrors leave_group's swap).
    let mut guard = state.mesh.lock().await;
    if let Some(old) = guard.take() {
        let _ = old.shutdown().await;
    }
    let new = Mesh::start(effective_core_config(&state.data_dir))
        .await
        .map_err(|e| e.to_string())?;
    for d in load_devices(&state.data_dir) {
        if let Ok(addr) = endpoint_addr_from_id(&d.id) {
            new.add_peer(addr).await;
        }
    }
    *guard = Some(new);
    Ok(())
}

// ----------------------------------------------------------------- MCP serve mode

/// The whole platform behind one MCP server: memory + tabs + files, all on one mesh.
/// Routes each tool call to the owning app by name prefix.
struct KithApp {
    mem: Memory,
    tabs: Tabs,
    files: Files,
}

impl McpApp for KithApp {
    fn server_name(&self) -> String {
        "kith".to_string()
    }

    fn tools(&self) -> Vec<mesh_mcp::ToolDef> {
        let mut t = self.mem.tools();
        t.extend(self.tabs.tools());
        t.extend(self.files.tools());
        t
    }

    async fn call_tool(
        &self,
        name: &str,
        args: serde_json::Value,
    ) -> std::result::Result<serde_json::Value, String> {
        if let Some(rest) = name.strip_prefix("memory.") {
            self.mem.call_tool(&format!("memory.{rest}"), args).await
        } else if let Some(rest) = name.strip_prefix("tabs.") {
            self.tabs.call_tool(&format!("tabs.{rest}"), args).await
        } else if let Some(rest) = name.strip_prefix("files.") {
            self.files.call_tool(&format!("files.{rest}"), args).await
        } else {
            Err(format!("unknown tool: {name}"))
        }
    }
}

/// Build the unified MCP app over a shared engine handle.
fn kith_app(mesh: &Mesh) -> KithApp {
    KithApp {
        mem: Memory::from_mesh(mesh.clone()),
        tabs: Tabs::from_mesh(mesh.clone()),
        files: Files::from_mesh(mesh.clone()),
    }
}

/// Host a localhost MCP bridge over the GUI's engine: a `127.0.0.1` TCP server whose
/// port + a random token are written to `mcp.port`. When the GUI owns the engine,
/// `kith serve` connects here instead of opening the data dir a second time — so the AI
/// and the app share ONE engine. Localhost-only and token-gated.
fn start_mcp_bridge(holder: MeshHolder, data_dir: PathBuf) {
    tauri::async_runtime::spawn(async move {
        let listener = match tokio::net::TcpListener::bind("127.0.0.1:0").await {
            Ok(l) => l,
            Err(e) => {
                eprintln!("kith: MCP bridge bind failed: {e}");
                return;
            }
        };
        let Ok(addr) = listener.local_addr() else {
            return;
        };
        let token = uuid::Uuid::new_v4().to_string();
        let _ = std::fs::write(
            data_dir.join("mcp.port"),
            format!("{}\n{}\n", addr.port(), token),
        );
        loop {
            let Ok((sock, _)) = listener.accept().await else {
                continue;
            };
            let holder = holder.clone();
            let token = token.clone();
            tauri::async_runtime::spawn(async move {
                use tokio::io::AsyncBufReadExt;
                let (r, w) = sock.into_split();
                let mut reader = tokio::io::BufReader::new(r);
                let mut first = String::new();
                // First line must be the shared token (gates the local bridge).
                if reader.read_line(&mut first).await.is_err() || first.trim() != token {
                    return;
                }
                // Read the CURRENT engine (a pairing/leave-group restart swaps it).
                let mesh = {
                    let guard = holder.lock().await;
                    match guard.as_ref() {
                        Some(m) => m.clone(),
                        None => return,
                    }
                };
                // Bind this MCP connection to the GUI's active Space (the human's
                // selection) for its lifetime — no tool can address another Space.
                let active = mesh.active_space();
                let bound = mesh.bound_to(active).unwrap_or(mesh);
                let _ = mesh_mcp::serve_stream(kith_app(&bound), reader, w).await;
            });
        }
    });
}

/// Bridge stdio MCP to a running Kith GUI's localhost MCP server.
async fn proxy_to_running(data_dir: &Path) -> anyhow::Result<()> {
    use tokio::io::AsyncWriteExt;
    let contents = std::fs::read_to_string(data_dir.join("mcp.port")).map_err(|_| {
        anyhow::anyhow!(
            "Kith doesn't seem to be running. Open the Kith app, then reconnect your MCP client."
        )
    })?;
    let mut lines = contents.lines();
    let port: u16 = lines.next().unwrap_or("").trim().parse().map_err(|_| {
        anyhow::anyhow!("Kith's MCP bridge info is unreadable; restart the Kith app.")
    })?;
    let token = lines.next().unwrap_or("").trim().to_string();
    let stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .map_err(|_| {
            anyhow::anyhow!("Kith is starting or not running; couldn't reach its local MCP bridge.")
        })?;
    let (mut tcp_r, mut tcp_w) = stream.into_split();
    tcp_w.write_all(format!("{token}\n").as_bytes()).await?;
    let mut stdin = tokio::io::stdin();
    let mut stdout = tokio::io::stdout();
    tokio::select! {
        _ = tokio::io::copy(&mut stdin, &mut tcp_w) => {}
        _ = tokio::io::copy(&mut tcp_r, &mut stdout) => {}
    }
    Ok(())
}

/// The Space id the human asked this MCP server to bind to, from `kith serve --space <id>`
/// or the `KITH_SPACE` env var. `None` means bind to the default Space.
fn requested_space() -> Option<String> {
    let args: Vec<String> = std::env::args().collect();
    if let Some(p) = args.iter().position(|a| a == "--space") {
        if let Some(v) = args.get(p + 1) {
            let v = v.trim();
            if !v.is_empty() {
                return Some(v.to_string());
            }
        }
    }
    std::env::var("KITH_SPACE")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Resolve the Space an MCP server binds to and return a handle PINNED to it for the
/// process's lifetime. Falls back to the default Space (logging) if none/unknown is given.
fn bind_app_mesh(mesh: &Mesh) -> Mesh {
    match requested_space() {
        Some(s) => match SpaceId::parse(&s).and_then(|id| mesh.bound_to(id)) {
            Some(m) => m,
            None => {
                eprintln!("kith serve: unknown --space '{s}', serving the default space");
                mesh.clone()
            }
        },
        None => mesh.clone(),
    }
}

/// Headless mode (`kith serve`): expose the unified MCP server over stdio for an MCP
/// client like Claude Desktop. If this process can own the engine, it serves directly;
/// if the GUI already owns it, it transparently bridges to the GUI's engine so the two
/// never collide. ONLY JSON-RPC goes to stdout; logs go to stderr.
pub fn serve() {
    let rt = match tokio::runtime::Runtime::new() {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("kith serve: cannot start runtime: {e}");
            std::process::exit(1);
        }
    };
    rt.block_on(async {
        let data_dir = kith_data_dir();
        std::fs::create_dir_all(&data_dir).ok();
        install_panic_logger(&data_dir);
        // Try to own the engine. If another Kith (usually the GUI) already owns the data
        // dir, the content store is locked and start fails — then we bridge to it.
        match Mesh::start(effective_core_config(&data_dir)).await {
            Ok(mesh) => {
                for d in load_devices(&data_dir) {
                    if let Ok(addr) = endpoint_addr_from_id(&d.id) {
                        mesh.add_peer(addr).await;
                    }
                }
                // Bind this MCP server to exactly ONE Space, chosen by the human out of
                // band (`--space <id>` / `KITH_SPACE`), else the default Space. The bound
                // handle ignores active-Space changes and no tool takes a Space argument,
                // so a prompt-injected agent cannot reach another Space (confused-deputy).
                let app_mesh = bind_app_mesh(&mesh);
                eprintln!(
                    "kith MCP server ready (device {}, space {}, data {})",
                    mesh.endpoint_id(),
                    app_mesh.active_space().to_hex(),
                    data_dir.display()
                );
                if let Err(e) = mesh_mcp::serve_stdio(kith_app(&app_mesh)).await {
                    eprintln!("kith serve: {e}");
                }
            }
            Err(_) => {
                eprintln!("kith: another instance owns the engine — bridging to it…");
                if let Err(e) = proxy_to_running(&data_dir).await {
                    eprintln!("kith serve: {e}");
                    std::process::exit(1);
                }
            }
        }
    });
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_window_state::Builder::default().build())
        .setup(|app| {
            let data_dir = kith_data_dir();
            std::fs::create_dir_all(&data_dir).ok();
            install_panic_logger(&data_dir);
            // One engine for the whole app, serverless by default (DHT + n0 relay).
            // Blobs enabled so files can be served to / fetched from your circle.
            let mesh = match tauri::async_runtime::block_on(Mesh::start(effective_core_config(
                &data_dir,
            ))) {
                Ok(m) => m,
                Err(e) => {
                    use tauri_plugin_dialog::MessageDialogKind;
                    app.dialog()
                        .message(format!(
                            "Kith couldn't start its engine.\n\n{e}\n\nIs Kith already running (for example a background MCP server)? Close the other instance and try again."
                        ))
                        .kind(MessageDialogKind::Error)
                        .title("Kith")
                        .blocking_show();
                    std::process::exit(1);
                }
            };
            let devices = load_devices(&data_dir);
            let offered = load_map(&data_dir, "offered_paths.json");
            tauri::async_runtime::block_on(async {
                for d in &devices {
                    if let Ok(addr) = endpoint_addr_from_id(&d.id) {
                        mesh.add_peer(addr).await;
                    }
                }
                // Re-serve files we still offer, so offers survive a restart (the
                // content tag is in-memory; re-importing the unchanged file restores it).
                let files = Files::from_mesh(mesh.clone());
                for e in files.all().await {
                    if e.from_id == mesh.endpoint_id() {
                        if let Some(p) = offered.get(&e.id) {
                            if Path::new(p).exists() {
                                let _ = mesh.share_file(Path::new(p)).await;
                            }
                        }
                    }
                }
            });
            // Host the localhost MCP bridge so `kith serve` (the AI) can share THIS
            // engine instead of opening the data dir a second time.
            let holder: MeshHolder = Arc::new(Mutex::new(Some(mesh)));
            start_mcp_bridge(holder.clone(), data_dir.clone());
            app.manage(AppState {
                mesh: holder,
                data_dir,
                pairing: StdMutex::new(None),
                downloads: StdMutex::new(HashMap::new()),
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            app_version,
            my_endpoint_id,
            this_device,
            list_notes,
            add_note,
            search_notes,
            forget_note,
            list_tabs,
            add_tab,
            forget_tab,
            list_files,
            offer_file,
            forget_file,
            rename_file,
            download_file,
            cancel_download,
            agent_info,
            start_link,
            cancel_link,
            poll_pairing,
            join_link,
            list_devices,
            rename_device,
            unlink_device,
            leave_group,
            reveal_path,
            open_external,
            get_settings,
            set_download_dir,
            list_history,
            clear_history,
            list_spaces,
            switch_space,
            create_space,
            leave_space,
            space_members,
            space_add_member,
            space_set_role,
            space_remove_member,
            space_audit,
            space_export,
            space_import,
            get_network,
            set_network
        ])
        .run(tauri::generate_context!())
        .expect("error while running Kith");
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The unified MCP server lists every app's tools and routes calls by prefix.
    #[tokio::test(flavor = "multi_thread")]
    async fn unified_mcp_lists_and_routes() {
        let dir = tempfile::tempdir().unwrap();
        let mesh = Mesh::start(CoreConfig::local_only(dir.path()).with_blobs(true))
            .await
            .unwrap();
        let app = kith_app(&mesh);

        let names: Vec<String> = app.tools().into_iter().map(|t| t.name).collect();
        for expected in ["memory.append", "memory.read", "tabs.add", "files.share"] {
            assert!(
                names.iter().any(|n| n == expected),
                "missing tool {expected}"
            );
        }

        // Route a memory write then read it back (proves prefix routing + shared mesh).
        app.call_tool(
            "memory.append",
            serde_json::json!({ "text": "remember me", "kind": "fact" }),
        )
        .await
        .unwrap();
        let read = app
            .call_tool("memory.read", serde_json::json!({}))
            .await
            .unwrap();
        assert!(read.to_string().contains("remember me"));

        // A tabs write is routed to the tabs app (not memory).
        app.call_tool(
            "tabs.add",
            serde_json::json!({ "url": "https://example.com", "title": "Example" }),
        )
        .await
        .unwrap();

        // An unknown prefix is an error.
        assert!(app
            .call_tool("bogus.thing", serde_json::json!({}))
            .await
            .is_err());
    }

    /// An MCP server built over a bound handle addresses exactly one Space and ignores
    /// active-Space changes (M6 — the structural binding).
    #[tokio::test(flavor = "multi_thread")]
    async fn mcp_server_bound_to_one_space() {
        let dir = tempfile::tempdir().unwrap();
        let mesh = Mesh::start(CoreConfig::local_only(dir.path()).with_blobs(true))
            .await
            .unwrap();
        let other = mesh.create_space("client-b").await.unwrap();
        let bound = mesh.bound_to(other).expect("space exists");
        assert!(bound.is_bound());
        assert_eq!(bound.active_space(), other);

        // Changing the shared active Space does NOT move the bound handle.
        assert!(mesh.set_active_space(SpaceId::default_space()));
        assert_eq!(
            bound.active_space(),
            other,
            "a bound MCP handle ignores active-Space changes"
        );
        // The bound handle itself refuses to switch.
        assert!(!bound.set_active_space(SpaceId::default_space()));
        assert_eq!(bound.active_space(), other);
    }

    /// No MCP tool exposes a Space argument — an agent has no way to name another Space.
    #[tokio::test(flavor = "multi_thread")]
    async fn mcp_has_no_cross_space_argument() {
        let dir = tempfile::tempdir().unwrap();
        let mesh = Mesh::start(CoreConfig::local_only(dir.path()).with_blobs(true))
            .await
            .unwrap();
        for t in kith_app(&mesh).tools() {
            if let Some(props) = t.input_schema.get("properties").and_then(|p| p.as_object()) {
                for key in props.keys() {
                    assert!(
                        !key.to_lowercase().contains("space"),
                        "tool {} exposes a Space argument '{}'",
                        t.name,
                        key
                    );
                }
            }
        }
    }

    /// An agent's writes land ONLY in the bound Space, never the default Space.
    #[tokio::test(flavor = "multi_thread")]
    async fn agent_writes_land_only_in_bound_space() {
        let dir = tempfile::tempdir().unwrap();
        let mesh = Mesh::start(CoreConfig::local_only(dir.path()).with_blobs(true))
            .await
            .unwrap();
        let client = mesh.create_space("client-a").await.unwrap();

        // The MCP app is bound to `client`; an agent appends a memory through it.
        let app = kith_app(&mesh.bound_to(client).unwrap());
        app.call_tool(
            "memory.append",
            serde_json::json!({ "text": "client-a secret", "kind": "fact" }),
        )
        .await
        .unwrap();

        // It is present in the bound Space…
        let in_bound = Memory::from_mesh(mesh.bound_to(client).unwrap())
            .all()
            .await;
        assert!(
            in_bound.iter().any(|e| e.text == "client-a secret"),
            "write landed in the bound Space"
        );
        // …and absent from the default Space (no cross-space leak).
        let in_default = Memory::from_mesh(mesh.clone()).all().await;
        assert!(
            in_default.iter().all(|e| e.text != "client-a secret"),
            "write did not leak into the default Space"
        );
    }
}
