/* Kith desktop — UI logic.
 * Commands: app_version, my_endpoint_id, this_device, list_notes, add_note,
 * search_notes, forget_note, start_link, cancel_link, join_link, list_devices,
 * rename_device, unlink_device, open_external.
 */

const TAURI = window.__TAURI__;
const HAS_TAURI = !!(TAURI && TAURI.core);
const invoke = HAS_TAURI ? TAURI.core.invoke : mockInvoke;

/* ---- demo data: used ONLY outside the Kith app (e.g. a browser preview), so the
   UI is fully navigable for screenshots. The real app sets HAS_TAURI and never
   touches any of this. ---- */
const DEMO_ID = "k7d3p2qmf8x4a1c9n6b5v0w2z7y3t8r4e5u1i6o9s2d4f7g0";
let demoNotes = [
  { id: "n1", text: "Mum's birthday is March 3rd 🌷", kind: "fact" },
  { id: "n2", text: "Home Wi-Fi password: sunflower42", kind: "fact" },
  { id: "n3", text: "Pick up the prescription on Thursday", kind: "note" },
  { id: "n4", text: "Gift idea for Sam — the blue scarf she liked", kind: "idea" },
  { id: "n5", text: "Dr. Reyes (clinic): 0412 555 209", kind: "fact" },
];
let demoDevices = [{ id: "a2f9c7e1b4d8", name: "Home desktop", synced_ago: 14 }];
let demoTabs = [
  { id: "t1", url: "https://github.com/muhamadjawdatsalemalakoum/kith", title: "Kith on GitHub" },
  { id: "t2", url: "https://automerge.org", title: "Automerge — the CRDT behind Kith" },
  { id: "t3", url: "https://iroh.computer", title: "iroh — the P2P transport" },
];
let demoFiles = [
  { id: "f1", name: "Family photos.zip", size: 248 * 1024 * 1024, from: "Home desktop", mine: false, local_path: null },
  { id: "f2", name: "Resume.pdf", size: 182 * 1024, from: "This laptop", mine: true, local_path: "C:\\Users\\You\\Documents\\Resume.pdf" },
];
let demoLinkPolls = 0;
let demoCancelled = new Set();
let demoSpaces = [
  { id: "0".repeat(64), name: "Personal", is_default: true, is_active: true, enforced: false, role: null, epoch: 0, members: 0 },
  { id: "a".repeat(64), name: "Design team", is_default: false, is_active: false, enforced: true, role: "admin", epoch: 1, members: 3 },
];
let demoMembers = [
  { id: DEMO_ID, role: "admin", is_me: true },
  { id: "b3c1f7a9d2e4" + "b".repeat(40), role: "writer", is_me: false },
  { id: "c5e2a8f1b6d3" + "c".repeat(40), role: "reader", is_me: false },
];
let demoAudit = [
  { seq: 0, epoch: 0, signer: DEMO_ID, action: "space-created", target: DEMO_ID },
  { seq: 1, epoch: 0, signer: DEMO_ID, action: "member-added:writer", target: "b3c1f7a9d2e4" + "b".repeat(40) },
  { seq: 2, epoch: 0, signer: DEMO_ID, action: "member-added:reader", target: "c5e2a8f1b6d3" + "c".repeat(40) },
  { seq: 3, epoch: 1, signer: DEMO_ID, action: "key-rotated:1", target: "" },
];
let demoNetwork = { mode: "decentralized", relay_url: "", relay_token: "", pkarr_relay: "", origin_domain: "" };
async function mockInvoke(cmd, args = {}) {
  await new Promise((r) => setTimeout(r, 120));
  switch (cmd) {
    case "app_version": return "0.0.1";
    case "my_endpoint_id": return DEMO_ID;
    case "this_device": return { id: DEMO_ID, name: "This laptop" };
    case "list_notes": return demoNotes.slice();
    case "search_notes": return demoNotes.filter((n) => n.text.toLowerCase().includes((args.query || "").toLowerCase()));
    case "add_note": { const n = { id: "n" + Date.now(), text: args.text, kind: args.kind || "note" }; demoNotes.unshift(n); return n; }
    case "forget_note": { demoNotes = demoNotes.filter((n) => n.id !== args.id); return true; }
    case "start_link": return { invite: DEMO_ID + ":K7P29QXM" };
    case "cancel_link": return null;
    case "poll_pairing": { demoLinkPolls++; if (demoLinkPolls >= 3) { demoLinkPolls = 0; const d = { id: "c9f1a2b3d4e5", name: "Linked device", synced_ago: 1 }; demoDevices.push(d); return d; } return null; }
    case "join_link": { const d = { id: "b5e1d3a7f902", name: (args.name && args.name.trim()) || "Linked device" }; demoDevices.push(d); return d; }
    case "list_devices": return { me: { id: DEMO_ID, name: "This laptop" }, linked: demoDevices.slice() };
    case "rename_device": { const d = demoDevices.find((x) => x.id === args.id); if (d) d.name = args.name; return null; }
    case "unlink_device": { demoDevices = demoDevices.filter((x) => x.id !== args.id); return null; }
    case "list_tabs": return demoTabs.slice();
    case "add_tab": { const t = { id: "t" + Date.now(), url: args.url, title: args.title || "" }; demoTabs.unshift(t); return t; }
    case "forget_tab": { demoTabs = demoTabs.filter((t) => t.id !== args.id); return true; }
    case "agent_info": return {
      config: JSON.stringify({ mcpServers: { kith: { command: "C:\\Program Files\\Kith\\kith.exe", args: ["serve"] } } }, null, 2),
      binary: "C:\\Program Files\\Kith\\kith.exe",
      tools: [
        { name: "memory.append", description: "Remember a fact or preference about the user (syncs to all their devices)." },
        { name: "memory.search", description: "Search the user's memory for relevant entries." },
        { name: "memory.read", description: "List everything the user has remembered." },
        { name: "memory.forget", description: "Forget a memory entry by id." },
        { name: "tabs.add", description: "Save a tab to the mesh (syncs to all your devices)." },
        { name: "tabs.list", description: "List saved tabs (id, url, title)." },
        { name: "tabs.forget", description: "Remove a saved tab by id." },
        { name: "files.share", description: "Offer a local file to the user's other devices (returns its id)." },
        { name: "files.list", description: "List files offered across the user's devices." },
        { name: "files.fetch", description: "Download an offered file (by id) into a destination folder." },
        { name: "files.read", description: "Read the CONTENTS of an offered file by id (fetching it across your devices if needed)." },
        { name: "files.search", description: "Search offered files by name across your devices." },
      ],
    };
    case "list_files": return demoFiles.slice();
    case "offer_file": { const f = { id: "f" + Date.now(), name: "New upload.bin", size: 12 * 1024 * 1024, from: "This laptop", mine: true, local_path: "C:\\Users\\You\\Documents\\New upload.bin" }; demoFiles.unshift(f); return f; }
    case "forget_file": { demoFiles = demoFiles.filter((x) => x.id !== args.id); return true; }
    case "rename_file": { const f = demoFiles.find((x) => x.id === args.id); if (f) f.name = args.name; return true; }
    case "download_file": {
      const ev = args.onEvent;
      const total = (demoFiles.find((x) => x.id === args.id) || {}).size || 1000000;
      let sent = 0;
      const step = () => {
        if (demoCancelled.has(args.id)) { demoCancelled.delete(args.id); ev && ev.onmessage && ev.onmessage({ kind: "cancelled" }); return; }
        sent += total / 8;
        if (sent < total) { ev && ev.onmessage && ev.onmessage({ kind: "transferring", offset: sent, total, relayed: false }); setTimeout(step, 300); }
        else { const fe = demoFiles.find((x) => x.id === args.id); const pth = "C:\\Users\\You\\Downloads\\Kith\\" + ((fe || {}).name || "file"); if (fe) fe.local_path = pth; ev && ev.onmessage && ev.onmessage({ kind: "done", path: pth }); }
      };
      setTimeout(step, 300);
      return null;
    }
    case "cancel_download": { demoCancelled.add(args.id); return null; }
    case "list_history": { const now = Math.floor(Date.now() / 1000); return [
      { id: "h1", name: "Resume.pdf", size: 182 * 1024, direction: "sent", peer: "Your devices", ts: now - 120, path: "C:\\Users\\You\\Documents\\Resume.pdf" },
      { id: "h2", name: "Family photos.zip", size: 248 * 1024 * 1024, direction: "received", peer: "Home desktop", ts: now - 5400, path: "C:\\Users\\You\\Downloads\\Kith\\Family photos.zip" },
    ]; }
    case "clear_history": return null;
    case "get_settings": return { data_dir: "C:\\Users\\You\\.kith\\memory", download_dir: "C:\\Users\\You\\Downloads\\Kith", key_storage: "Windows Credential Manager" };
    case "list_spaces": return demoSpaces.map((s) => ({ ...s }));
    case "switch_space": demoSpaces.forEach((s) => { s.is_active = s.id === args.id; }); return null;
    case "create_space": { const s = { id: ((args.team ? "a1" : "b2") + Date.now() + "0".repeat(64)).slice(0, 64), name: args.name, is_default: false, is_active: false, enforced: !!args.team, role: args.team ? "admin" : null, epoch: 0, members: args.team ? 1 : 0 }; demoSpaces.push(s); return { ...s }; }
    case "leave_space": demoSpaces = demoSpaces.filter((s) => s.id !== args.id); return true;
    case "space_members": return demoMembers.map((m) => ({ ...m }));
    case "space_add_member": demoMembers.push({ id: args.endpoint, role: args.role, is_me: false }); return null;
    case "space_set_role": { const m = demoMembers.find((x) => x.id === args.endpoint); if (m) m.role = args.role; return null; }
    case "space_remove_member": demoMembers = demoMembers.filter((x) => x.id !== args.endpoint); return null;
    case "space_audit": return demoAudit.map((a) => ({ ...a }));
    case "space_export": return "C:\\Users\\You\\Documents\\kith-space.kithspace";
    case "space_import": { const s = { id: ("99" + Date.now() + "0".repeat(64)).slice(0, 64), name: "Imported space", is_default: false, is_active: false, enforced: false, role: null, epoch: 0, members: 0 }; demoSpaces.push(s); return { ...s }; }
    case "get_network": return { ...demoNetwork };
    case "set_network": demoNetwork = { ...args.settings }; return null;
    case "set_download_dir": return "C:\\Users\\You\\Downloads";
    case "reveal_path": return null;
    case "open_external": window.open(args.url, "_blank"); return null;
    default: throw new Error("unknown command " + cmd);
  }
}

const $ = (s) => document.querySelector(s);
const $$ = (s) => [...document.querySelectorAll(s)];
const RM = matchMedia("(prefers-reduced-motion: reduce)").matches;
const canAnim = !RM && typeof Element.prototype.animate === "function";
const makeChannel = () => (HAS_TAURI ? new TAURI.core.Channel() : { onmessage: null });
function fmtBytes(n) {
  if (n == null) return "";
  const u = ["B", "KB", "MB", "GB", "TB"];
  let i = 0, v = n;
  while (v >= 1024 && i < u.length - 1) { v /= 1024; i++; }
  return `${v.toFixed(v < 10 && i > 0 ? 1 : 0)} ${u[i]}`;
}

/* -------------------------------- toasts -------------------------------- */
const ICON_OK = '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"><path d="m5 13 4 4L19 7"/></svg>';
const ICON_ERR = '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round"><path d="M12 8v5M12 16h0"/><circle cx="12" cy="12" r="9"/></svg>';
function toast(msg, kind) {
  const el = document.createElement("div");
  el.className = "toast" + (kind ? " " + kind : "");
  el.innerHTML = (kind === "ok" ? ICON_OK : kind === "err" ? ICON_ERR : "") + "<span></span>";
  el.querySelector("span").textContent = msg;
  $("#toasts").appendChild(el);
  if (canAnim) el.animate([{ opacity: 0, transform: "translateY(8px)" }, { opacity: 1, transform: "none" }], { duration: 220, easing: "cubic-bezier(.34,1.3,.5,1)" });
  setTimeout(() => {
    if (canAnim) { const a = el.animate([{ opacity: 1 }, { opacity: 0, transform: "translateY(6px)" }], { duration: 220 }); a.onfinish = () => el.remove(); }
    else el.remove();
  }, 2600);
}

/* ----------------------------- navigation ------------------------------- */
let curView = "notes";
function moveIndicator(btn) {
  const ind = $(".nav-indicator");
  if (!ind || !btn) return;
  ind.style.transform = `translateY(${btn.offsetTop + btn.offsetHeight / 2 - 12}px)`;
}
function switchView(view) {
  if (view === curView) {
    if (view === "devices") loadDevices();
    return;
  }
  curView = view;
  $$(".nav-item").forEach((b) => {
    const on = b.dataset.view === view;
    b.classList.toggle("is-active", on);
    if (on) { b.setAttribute("aria-current", "page"); moveIndicator(b); }
    else b.removeAttribute("aria-current");
  });
  $$(".view").forEach((v) => v.classList.toggle("is-active", v.id === "view-" + view));
  if (view === "devices") loadDevices();
  if (view === "notes") refreshNotes();
  if (view === "tabs") loadTabs();
  if (view === "files") loadFiles();
  if (view === "spaces") loadSpaces();
  if (view === "agents") loadAgents();
  if (view === "about") loadSettings();
}
$$(".nav-item").forEach((b) => b.addEventListener("click", () => switchView(b.dataset.view)));

/* -------------------------------- theme --------------------------------- */
function applyTheme(mode) {
  document.documentElement.setAttribute("data-theme", mode);
}
(function initTheme() {
  applyTheme(localStorage.getItem("kith-theme") || "dark");
  $("#theme-toggle").addEventListener("click", () => {
    const next = document.documentElement.getAttribute("data-theme") === "light" ? "dark" : "light";
    localStorage.setItem("kith-theme", next);
    applyTheme(next);
  });
})();

/* -------------------------- external links ------------------------------ */
document.addEventListener("click", (e) => {
  const a = e.target.closest(".js-ext");
  if (!a) return;
  e.preventDefault();
  invoke("open_external", { url: a.dataset.url }).catch(() => {});
});

/* --------------------------------- notes -------------------------------- */
let curKind = "note";
$$("#note-kinds .chip").forEach((c) => c.addEventListener("click", () => {
  $$("#note-kinds .chip").forEach((x) => x.classList.remove("on"));
  c.classList.add("on");
  curKind = c.dataset.kind;
}));

function renderNotes(notes) {
  const list = $("#notes-list"), empty = $("#notes-empty");
  list.innerHTML = "";
  if (!notes || !notes.length) { empty.classList.remove("hidden"); return; }
  empty.classList.add("hidden");
  notes.forEach((n, i) => list.appendChild(noteEl(n, i)));
}
function noteEl(n, i) {
  const el = document.createElement("div");
  el.className = "note";
  el.dataset.id = n.id;
  const body = document.createElement("div");
  body.className = "note-body";
  const text = document.createElement("div");
  text.className = "note-text";
  text.textContent = n.text;
  body.appendChild(text);
  if (n.kind && n.kind !== "note") {
    const tag = document.createElement("span");
    tag.className = "note-tag";
    tag.textContent = n.kind;
    body.appendChild(tag);
  }
  const del = document.createElement("button");
  del.className = "note-del";
  del.title = "Forget this";
  del.setAttribute("aria-label", "Forget this note");
  del.innerHTML = '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="M4 7h16M9 7V5a1 1 0 0 1 1-1h4a1 1 0 0 1 1 1v2m2 0v12a2 2 0 0 1-2 2H8a2 2 0 0 1-2-2V7"/></svg>';
  del.addEventListener("click", () => forgetNote(n.id, el));
  el.append(body, del);
  if (canAnim) el.animate([{ opacity: 0, transform: "translateY(8px)" }, { opacity: 1, transform: "none" }], { duration: 220, delay: Math.min(i, 8) * 28, easing: "cubic-bezier(.22,1,.36,1)" });
  return el;
}
async function refreshNotes() {
  const q = $("#note-search").value.trim();
  try {
    const notes = q ? await invoke("search_notes", { query: q }) : await invoke("list_notes");
    renderNotes(notes);
  } catch (e) { /* engine starting; ignore transient */ }
}
async function addNote() {
  const ta = $("#note-input");
  const text = ta.value.trim();
  if (!text) { ta.focus(); return; }
  const btn = $("#note-add"); btn.disabled = true;
  try {
    await invoke("add_note", { text, kind: curKind });
    ta.value = "";
    $("#note-search").value = "";
    await refreshNotes();
    toast("Remembered — synced to your devices", "ok");
  } catch (e) {
    toast(String(e), "err");
  } finally { btn.disabled = false; ta.focus(); }
}
async function forgetNote(id, el) {
  try {
    await invoke("forget_note", { id });
    if (canAnim) { const a = el.animate([{ opacity: 1 }, { opacity: 0, transform: "translateX(8px)" }], { duration: 180 }); a.onfinish = () => { el.remove(); maybeEmpty(); }; }
    else { el.remove(); maybeEmpty(); }
  } catch (e) { toast(String(e), "err"); }
}
function maybeEmpty() {
  if (!$("#notes-list").children.length) $("#notes-empty").classList.remove("hidden");
}
$("#note-add").addEventListener("click", addNote);
$("#note-input").addEventListener("keydown", (e) => {
  if ((e.ctrlKey || e.metaKey) && e.key === "Enter") { e.preventDefault(); addNote(); }
});
let searchTimer = null;
$("#note-search").addEventListener("input", () => {
  clearTimeout(searchTimer);
  searchTimer = setTimeout(refreshNotes, 160);
});

/* --------------------------------- tabs --------------------------------- */
const GLOBE = '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="9"/><path d="M3 12h18M12 3c2.6 2.7 2.6 15.3 0 18M12 3c-2.6 2.7-2.6 15.3 0 18"/></svg>';
function renderTabs(tabs) {
  const list = $("#tabs-list"), empty = $("#tabs-empty");
  list.innerHTML = "";
  if (!tabs || !tabs.length) { empty.classList.remove("hidden"); return; }
  empty.classList.add("hidden");
  tabs.forEach((t, i) => list.appendChild(tabEl(t, i)));
}
function tabEl(t, i) {
  const el = document.createElement("div");
  el.className = "note";
  el.dataset.id = t.id;
  const av = document.createElement("div");
  av.className = "dev-avatar";
  av.innerHTML = GLOBE;
  const body = document.createElement("div");
  body.className = "note-body";
  body.style.cursor = "pointer";
  body.title = "Open in your browser";
  const title = document.createElement("div");
  title.className = "note-text";
  title.textContent = t.title && t.title.trim() ? t.title : t.url;
  const meta = document.createElement("div");
  meta.className = "dev-meta";
  meta.textContent = t.url;
  body.append(title, meta);
  body.addEventListener("click", () => invoke("open_external", { url: t.url }).catch(() => {}));
  const del = document.createElement("button");
  del.className = "note-del";
  del.title = "Remove";
  del.innerHTML = '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="M4 7h16M9 7V5a1 1 0 0 1 1-1h4a1 1 0 0 1 1 1v2m2 0v12a2 2 0 0 1-2 2H8a2 2 0 0 1-2-2V7"/></svg>';
  del.addEventListener("click", () => forgetTab(t.id, el));
  el.append(av, body, del);
  if (canAnim) el.animate([{ opacity: 0, transform: "translateY(8px)" }, { opacity: 1, transform: "none" }], { duration: 220, delay: Math.min(i, 8) * 28, easing: "cubic-bezier(.22,1,.36,1)" });
  return el;
}
async function loadTabs() {
  try { renderTabs(await invoke("list_tabs")); } catch (e) { /* starting */ }
}
async function addTab() {
  const u = $("#tab-url"), ti = $("#tab-title");
  const url = u.value.trim();
  if (!url) { u.focus(); return; }
  const btn = $("#tab-add"); btn.disabled = true;
  try {
    await invoke("add_tab", { url, title: ti.value.trim() });
    u.value = ""; ti.value = "";
    await loadTabs();
    toast("Tab saved — synced to your devices", "ok");
  } catch (e) { toast(String(e), "err"); }
  finally { btn.disabled = false; u.focus(); }
}
async function forgetTab(id, el) {
  try {
    await invoke("forget_tab", { id });
    if (canAnim) { const a = el.animate([{ opacity: 1 }, { opacity: 0, transform: "translateX(8px)" }], { duration: 180 }); a.onfinish = () => { el.remove(); if (!$("#tabs-list").children.length) $("#tabs-empty").classList.remove("hidden"); }; }
    else { el.remove(); }
  } catch (e) { toast(String(e), "err"); }
}
$("#tab-add").addEventListener("click", addTab);
$("#tab-url").addEventListener("keydown", (e) => { if (e.key === "Enter") { e.preventDefault(); addTab(); } });
$("#tab-title").addEventListener("keydown", (e) => { if (e.key === "Enter") { e.preventDefault(); addTab(); } });

/* -------------------------------- agents -------------------------------- */
let agentBinary = null;
async function loadAgents() {
  try {
    const info = await invoke("agent_info");
    $("#agent-config").textContent = info.config;
    const tl = $("#agent-tools"); tl.innerHTML = "";
    (info.tools || []).forEach((t) => {
      const el = document.createElement("div");
      el.className = "note";
      el.innerHTML = '<div class="note-body"><div class="note-text" style="font-family:var(--mono);font-size:14px;color:var(--teal)"></div><div class="dev-meta" style="font-family:var(--font);color:var(--muted)"></div></div>';
      el.querySelector(".note-text").textContent = t.name;
      el.querySelector(".dev-meta").textContent = t.description;
      tl.appendChild(el);
    });
    agentBinary = info.binary || null;
    if (agentBinary) {
      $("#agent-reveal").classList.remove("hidden");
      $("#agent-binary-warn").textContent = "";
    } else {
      $("#agent-reveal").classList.add("hidden");
      $("#agent-binary-warn").textContent = "Tip: the agent-memory server wasn't found next to Kith yet. The config above assumes it's on your PATH — or build it with  cargo build --release -p agent-memory";
    }
  } catch (e) { /* starting */ }
}
$("#agent-copy").addEventListener("click", () => {
  const c = $("#agent-config").textContent;
  if (c && navigator.clipboard) {
    navigator.clipboard.writeText(c);
    const b = $("#agent-copy"); b.textContent = "Copied ✓";
    setTimeout(() => { b.textContent = "Copy config"; }, 1400);
  }
});
$("#agent-reveal").addEventListener("click", () => { if (agentBinary) invoke("reveal_path", { path: agentBinary }).catch(() => {}); });

/* --------------------------------- files -------------------------------- */
const FILE_ICON = '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="M14 3v5h5"/><path d="M6 3h8l5 5v11a1 1 0 0 1-1 1H6a1 1 0 0 1-1-1V4a1 1 0 0 1 1-1Z"/></svg>';
const FILE_ACT_ICN = {
  open: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="M3 7a2 2 0 0 1 2-2h3.5l2 2H19a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2Z"/></svg>',
  rename: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="M12 20h9"/><path d="M16.5 3.5a2.1 2.1 0 0 1 3 3L7 19l-4 1 1-4Z"/></svg>',
  remove: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="M4 7h16M9 7V5a1 1 0 0 1 1-1h4a1 1 0 0 1 1 1v2m2 0v12a2 2 0 0 1-2 2H8a2 2 0 0 1-2-2V7"/></svg>',
};
function fileBtn(kind, title, onClick) {
  const b = document.createElement("button");
  b.className = "note-del";
  b.title = title;
  b.setAttribute("aria-label", title);
  b.innerHTML = FILE_ACT_ICN[kind];
  b.addEventListener("click", onClick);
  return b;
}
const DOWNLOAD_SVG = '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" style="width:16px;height:16px"><path d="M12 4v10M8 11l4 4 4-4"/><path d="M5 19h14"/></svg>';
const CANCEL_SVG = '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><path d="M6 6l12 12M18 6 6 18"/></svg>';
// Fill a remote file's action area with Download + Remove (also used to restore after
// a cancel/error).
function restoreDownload(f, el, action) {
  action.innerHTML = "";
  const dl = document.createElement("button");
  dl.className = "btn-ghost";
  dl.innerHTML = DOWNLOAD_SVG + " Download";
  dl.addEventListener("click", () => startDownload(f, el, action));
  action.appendChild(dl);
  action.appendChild(fileBtn("remove", "Remove from your files", () => forgetFile(f.id, el)));
}
function renderFiles(files) {
  const list = $("#files-list"), empty = $("#files-empty");
  list.innerHTML = "";
  if (!files || !files.length) { empty.classList.remove("hidden"); return; }
  empty.classList.add("hidden");
  files.forEach((f, i) => list.appendChild(fileEl(f, i)));
}
function fileEl(f, i) {
  const el = document.createElement("div");
  el.className = "note";
  el.dataset.id = f.id;
  const av = document.createElement("div");
  av.className = "dev-avatar";
  av.innerHTML = FILE_ICON;
  const body = document.createElement("div");
  body.className = "note-body";
  const name = document.createElement("div");
  name.className = "note-text";
  name.textContent = f.name;
  const meta = document.createElement("div");
  meta.className = "dev-meta";
  meta.style.fontFamily = "var(--font)";
  const where = f.mine ? "on this device" : f.local_path ? "downloaded · from " + f.from : "from " + f.from;
  meta.textContent = fmtBytes(f.size) + " · " + where;
  body.append(name, meta);
  const action = document.createElement("div");
  action.className = "file-action";
  // Remote file not yet here → Download (+ Remove). Otherwise it's local → manage it.
  if (!f.mine && !f.local_path) {
    restoreDownload(f, el, action);
  } else {
    if (f.local_path) action.appendChild(fileBtn("open", "Open file location", () => invoke("reveal_path", { path: f.local_path }).catch(() => {})));
    if (f.mine) action.appendChild(fileBtn("rename", "Rename", () => renameFile(f)));
    action.appendChild(fileBtn("remove", "Remove from your files", () => forgetFile(f.id, el)));
  }
  el.append(av, body, action);
  if (canAnim) el.animate([{ opacity: 0, transform: "translateY(8px)" }, { opacity: 1, transform: "none" }], { duration: 220, delay: Math.min(i, 8) * 28, easing: "cubic-bezier(.22,1,.36,1)" });
  return el;
}
async function renameFile(f) {
  const name = prompt("Rename this file", f.name);
  if (name && name.trim() && name.trim() !== f.name) {
    try { await invoke("rename_file", { id: f.id, name: name.trim() }); await loadFiles(); }
    catch (e) { toast(String(e), "err"); }
  }
}
async function loadFiles() {
  try { renderFiles(await invoke("list_files")); } catch (e) { /* starting */ }
  loadHistory();
}
async function offerFile() {
  try {
    const f = await invoke("offer_file");
    if (f) { await loadFiles(); toast("Sharing “" + f.name + "” — keep Kith open", "ok"); }
  } catch (e) { toast(String(e), "err"); }
}
async function forgetFile(id, el) {
  try {
    await invoke("forget_file", { id });
    if (canAnim) { const a = el.animate([{ opacity: 1 }, { opacity: 0, transform: "translateX(8px)" }], { duration: 180 }); a.onfinish = () => { el.remove(); if (!$("#files-list").children.length) $("#files-empty").classList.remove("hidden"); }; }
    else el.remove();
  } catch (e) { toast(String(e), "err"); }
}
function startDownload(f, el, action) {
  action.innerHTML =
    '<div class="dl"><div class="dl-bar"><span class="dl-fill"></span></div><div class="dl-row"><span class="dl-pct">0%</span><span class="route-badge connecting">connecting</span><button class="note-del js-cancel" title="Cancel download">' +
    CANCEL_SVG +
    "</button></div></div>";
  const fill = action.querySelector(".dl-fill");
  const pct = action.querySelector(".dl-pct");
  const badge = action.querySelector(".route-badge");
  action.querySelector(".js-cancel").addEventListener("click", () => invoke("cancel_download", { id: f.id }).catch(() => {}));
  const ch = makeChannel();
  ch.onmessage = (m) => {
    if (m.kind === "transferring") {
      const p = f.size > 0 ? Math.min(1, m.offset / f.size) : 0;
      fill.style.width = Math.round(p * 100) + "%";
      pct.textContent = Math.round(p * 100) + "%";
      badge.className = "route-badge " + (m.relayed ? "relayed" : "direct");
      badge.textContent = m.relayed ? "relayed" : "direct";
    } else if (m.kind === "done") {
      action.innerHTML = "";
      const open = document.createElement("button");
      open.className = "btn-ghost";
      open.textContent = "Open location";
      open.addEventListener("click", () => invoke("reveal_path", { path: m.path }).catch(() => {}));
      action.appendChild(open);
      action.appendChild(fileBtn("remove", "Remove from your files", () => forgetFile(f.id, el)));
      toast("Downloaded “" + f.name + "”", "ok");
    } else if (m.kind === "cancelled") {
      restoreDownload(f, el, action);
      toast("Download cancelled");
    } else if (m.kind === "error") {
      restoreDownload(f, el, action);
      toast(m.message || "Download failed", "err");
    }
  };
  invoke("download_file", { id: f.id, onEvent: ch }).catch((e) => {
    restoreDownload(f, el, action);
    toast(String(e), "err");
  });
}
$("#file-share").addEventListener("click", offerFile);

/* recent transfers (local history) */
const HIST_UP = '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="M12 20V6M6 12l6-6 6 6"/></svg>';
const HIST_DOWN = '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="M12 4v14M6 12l6 6 6-6"/></svg>';
function renderHistory(items) {
  const list = $("#history-list"), head = $("#history-head");
  list.innerHTML = "";
  if (!items || !items.length) { head.classList.add("hidden"); return; }
  head.classList.remove("hidden");
  const now = Math.floor(Date.now() / 1000);
  items.forEach((h) => {
    const el = document.createElement("div");
    el.className = "note";
    const av = document.createElement("div");
    av.className = "dev-avatar";
    av.innerHTML = h.direction === "sent" ? HIST_UP : HIST_DOWN;
    const body = document.createElement("div");
    body.className = "note-body";
    const name = document.createElement("div");
    name.className = "note-text";
    name.textContent = h.name;
    const meta = document.createElement("div");
    meta.className = "dev-meta";
    meta.style.fontFamily = "var(--font)";
    const dir = h.direction === "sent" ? "Sent · to " + h.peer : "Received · from " + h.peer;
    meta.textContent = dir + " · " + fmtBytes(h.size) + " · " + fmtAgo(Math.max(0, now - h.ts));
    body.append(name, meta);
    el.append(av, body);
    if (h.path) el.append(fileBtn("open", "Open location", () => invoke("reveal_path", { path: h.path }).catch(() => {})));
    list.appendChild(el);
  });
}
async function loadHistory() {
  try { renderHistory(await invoke("list_history")); } catch (_) {}
}
if ($("#history-clear")) $("#history-clear").addEventListener("click", async () => {
  try { await invoke("clear_history"); loadHistory(); } catch (_) {}
});

/* -------------------------------- devices ------------------------------- */
const DEV_ICON ='<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><rect x="3" y="4" width="18" height="12" rx="2"/><path d="M8 20h8M12 16v4"/></svg>';
function shortId(id) { return id ? id.slice(0, 10) + "…" + id.slice(-6) : ""; }

async function loadDevices() {
  try {
    const d = await invoke("list_devices");
    renderMe(d.me);
    renderLinked(d.linked || []);
    setNetDot(d.linked || []);
  } catch (e) { /* starting */ }
}
function fmtAgo(secs) {
  if (secs == null) return "";
  if (secs < 10) return "just now";
  if (secs < 60) return secs + "s ago";
  if (secs < 3600) return Math.floor(secs / 60) + "m ago";
  if (secs < 86400) return Math.floor(secs / 3600) + "h ago";
  return Math.floor(secs / 86400) + "d ago";
}
// Honest status light: lit (live) only when a device actually synced recently.
function setNetDot(linked) {
  const dot = $(".net-dot");
  if (!dot) return;
  const arr = linked || [];
  const recent = arr.some((d) => d.synced_ago != null && d.synced_ago < 90);
  if (recent) {
    dot.classList.remove("off");
    dot.title = "In sync with your devices";
  } else if (arr.length > 0) {
    dot.classList.add("off");
    dot.title = "Linked devices, but not synced recently";
  } else {
    dot.classList.add("off");
    dot.title = "No other devices linked yet";
  }
}
async function leaveGroup() {
  if (!confirm("Reset & re-key Kith?\n\nThis disconnects ALL linked devices and changes your security key, so a removed device can no longer sync. Your data on this device stays. You'll re-link any devices you want to keep.")) return;
  try {
    await invoke("leave_group");
    loadDevices();
    toast("Reset — re-link a device to sync again", "ok");
  } catch (e) { toast(String(e), "err"); }
}
function renderMe(me) {
  const wrap = $("#me-card");
  wrap.innerHTML = "";
  const el = document.createElement("div");
  el.className = "dev me";
  el.innerHTML = `<div class="dev-avatar">${DEV_ICON}</div>
    <div class="dev-body"><div class="dev-name"></div><div class="dev-meta"></div></div>
    <span class="badge-you">This one</span>`;
  el.querySelector(".dev-name").textContent = me.name || "This device";
  el.querySelector(".dev-meta").textContent = shortId(me.id);
  el.querySelector(".dev-meta").title = me.id;
  wrap.appendChild(el);
}
function renderLinked(list) {
  const wrap = $("#linked-list"), empty = $("#linked-empty");
  wrap.innerHTML = "";
  if (!list.length) { empty.classList.remove("hidden"); return; }
  empty.classList.add("hidden");
  list.forEach((dev) => {
    const el = document.createElement("div");
    el.className = "dev";
    el.innerHTML = `<div class="dev-avatar">${DEV_ICON}</div>
      <div class="dev-body"><div class="dev-name"></div><div class="dev-meta"></div></div>
      <div class="dev-actions">
        <button class="btn-quiet js-rename">Rename</button>
        <button class="btn-quiet js-unlink">Unlink</button>
      </div>`;
    el.querySelector(".dev-name").textContent = dev.name;
    const sync = dev.synced_ago != null ? "synced " + fmtAgo(dev.synced_ago) : "not synced yet";
    el.querySelector(".dev-meta").textContent = sync + " · " + shortId(dev.id);
    el.querySelector(".dev-meta").title = dev.id;
    el.querySelector(".js-rename").addEventListener("click", async () => {
      const name = prompt("Rename this device", dev.name);
      if (name && name.trim()) {
        try { await invoke("rename_device", { id: dev.id, name: name.trim() }); loadDevices(); }
        catch (e) { toast(String(e), "err"); }
      }
    });
    el.querySelector(".js-unlink").addEventListener("click", async () => {
      if (!confirm(`Unlink "${dev.name}"? This stops syncing with it on this device.`)) return;
      try { await invoke("unlink_device", { id: dev.id }); loadDevices(); toast("Device unlinked", "ok"); }
      catch (e) { toast(String(e), "err"); }
    });
    wrap.appendChild(el);
  });
}

/* ------------------------------ link (host) ----------------------------- */
function openModal(id) {
  const scrim = $(id);
  scrim.classList.remove("hidden");
  const sheet = scrim.querySelector(".sheet");
  if (canAnim) {
    scrim.animate([{ opacity: 0 }, { opacity: 1 }], { duration: 160 });
    sheet.animate([{ opacity: 0, transform: "translateY(12px) scale(.97)" }, { opacity: 1, transform: "none" }], { duration: 240, easing: "cubic-bezier(.34,1.3,.5,1)" });
  }
}
function closeModal(id) { $(id).classList.add("hidden"); }

let linkPollTimer = null;
function stopLinkPoll() {
  if (linkPollTimer) { clearInterval(linkPollTimer); linkPollTimer = null; }
}
$("#open-link").addEventListener("click", async () => {
  openModal("#link-modal");
  $("#link-code").textContent = "…";
  const w = $("#link-modal .waiting");
  if (w) { w.innerHTML = '<span class="spinner"></span> Waiting for the other computer to connect…'; w.style.color = ""; }
  stopLinkPoll();
  try {
    const info = await invoke("start_link");
    $("#link-code").textContent = info.invite;
    // Poll for the other device completing the link, so THIS (host) side updates too.
    linkPollTimer = setInterval(async () => {
      let dev = null;
      try { dev = await invoke("poll_pairing"); } catch (_) {}
      if (dev) {
        stopLinkPoll();
        const wd = $("#link-modal .waiting");
        if (wd) { wd.innerHTML = ""; wd.textContent = "Linked ✓ — " + dev.name + " is now in sync."; wd.style.color = "var(--success)"; }
        toast("Device linked", "ok");
        loadDevices();
        setTimeout(() => closeModal("#link-modal"), 1700);
      }
    }, 1500);
  } catch (e) {
    $("#link-code").textContent = "Couldn't start linking.";
    toast(String(e), "err");
  }
});
$("#link-copy").addEventListener("click", () => {
  const c = $("#link-code").textContent;
  if (c && navigator.clipboard) {
    navigator.clipboard.writeText(c);
    const b = $("#link-copy"); b.textContent = "Copied ✓";
    setTimeout(() => { b.textContent = "Copy code"; }, 1400);
  }
});
async function closeLink() {
  stopLinkPoll();
  await invoke("cancel_link").catch(() => {});
  closeModal("#link-modal");
  loadDevices();
}
$("#link-cancel").addEventListener("click", closeLink);
$("#link-modal").addEventListener("click", (e) => { if (e.target.id === "link-modal") closeLink(); });

/* ------------------------------ join (this) ----------------------------- */
$("#open-join").addEventListener("click", () => {
  $("#join-code").value = "";
  $("#join-name").value = "";
  $("#join-error").textContent = "";
  $("#join-ok").classList.add("hidden");
  $("#join-go").disabled = false;
  $("#join-go").querySelector(".js-label").textContent = "Link devices";
  openModal("#join-modal");
  setTimeout(() => $("#join-code").focus(), 60);
});
function closeJoin() { closeModal("#join-modal"); }
$("#join-cancel").addEventListener("click", closeJoin);
$("#join-modal").addEventListener("click", (e) => { if (e.target.id === "join-modal") closeJoin(); });
$("#join-go").addEventListener("click", async () => {
  const invite = $("#join-code").value.trim();
  const name = $("#join-name").value.trim();
  $("#join-error").textContent = "";
  if (!invite) {
    $("#join-error").textContent = "Paste the code from your other device first.";
    $("#join-code").classList.add("shake"); setTimeout(() => $("#join-code").classList.remove("shake"), 420);
    return;
  }
  const go = $("#join-go"); go.disabled = true;
  go.querySelector(".js-label").textContent = "Linking…";
  try {
    const dev = await invoke("join_link", { invite, name });
    $("#join-ok").classList.remove("hidden");
    $("#join-ok").innerHTML = ICON_OK + "<span></span>";
    $("#join-ok").querySelector("span").textContent = `Linked to ${dev.name}. Your memory will sync.`;
    toast("Devices linked", "ok");
    setTimeout(() => { closeJoin(); switchView("devices"); refreshNotes(); }, 1100);
  } catch (e) {
    $("#join-error").textContent = String(e);
    $("#join-code").classList.add("shake"); setTimeout(() => $("#join-code").classList.remove("shake"), 420);
    go.disabled = false;
    go.querySelector(".js-label").textContent = "Link devices";
  }
});

const leaveBtn = $("#leave-group");
if (leaveBtn) leaveBtn.addEventListener("click", leaveGroup);

/* -------------------------------- settings ------------------------------ */
async function loadSettings() {
  try {
    const s = await invoke("get_settings");
    const dl = $("#set-download"); if (dl) { dl.textContent = s.download_dir; dl.title = s.download_dir; }
    const dd = $("#set-datadir"); if (dd) { dd.textContent = s.data_dir; dd.title = s.data_dir; }
    const ks = $("#set-keys"); if (ks && s.key_storage) { ks.textContent = s.key_storage; ks.title = s.key_storage; }
  } catch (_) {}
  try {
    const n = await invoke("get_network");
    const ne = $("#set-network");
    if (ne) ne.textContent = n.mode === "self_hosted" ? "Self-hosted (" + (n.relay_url || "—") + ")" : "Default (serverless)";
  } catch (_) {}
}
if ($("#set-download-change")) $("#set-download-change").addEventListener("click", async () => {
  try {
    const p = await invoke("set_download_dir");
    if (p) { $("#set-download").textContent = p; $("#set-download").title = p; toast("Download folder updated", "ok"); }
  } catch (e) { toast(String(e), "err"); }
});
if ($("#set-datadir-open")) $("#set-datadir-open").addEventListener("click", () => {
  const p = $("#set-datadir").textContent;
  if (p && p !== "—") invoke("reveal_path", { path: p }).catch(() => {});
});

/* -------------------------------- spaces -------------------------------- */
const SPACE_PERSONAL_ICON = '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="8" r="4"/><path d="M5 20a7 7 0 0 1 14 0"/></svg>';
const SPACE_TEAM_ICON = '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><circle cx="9" cy="9" r="3"/><path d="M3.5 19a5.5 5.5 0 0 1 11 0"/><path d="M16 7a3 3 0 0 1 0 5.5M17 19a5.5 5.5 0 0 0-2-4.3"/></svg>';
let manageSpaceId = null;
let passMode = null; // "export" | "import"
let passSpaceId = null;
let createType = "personal";

function btnQuiet(label, onClick) {
  const b = document.createElement("button");
  b.className = "btn-quiet";
  b.textContent = label;
  b.addEventListener("click", onClick);
  return b;
}

async function loadSpaces() {
  try { renderSpaces(await invoke("list_spaces")); } catch (_) { /* starting */ }
}
function renderSpaces(spaces) {
  const list = $("#spaces-list");
  list.innerHTML = "";
  (spaces || []).forEach((s, i) => list.appendChild(spaceEl(s, i)));
}
function spaceEl(s, i) {
  const el = document.createElement("div");
  el.className = "dev space" + (s.is_active ? " is-active" : "");
  const av = document.createElement("div");
  av.className = "dev-avatar";
  av.innerHTML = s.enforced ? SPACE_TEAM_ICON : SPACE_PERSONAL_ICON;
  const body = document.createElement("div");
  body.className = "dev-body";
  const name = document.createElement("div");
  name.className = "dev-name";
  name.textContent = s.name;
  const meta = document.createElement("div");
  meta.className = "dev-meta";
  meta.style.fontFamily = "var(--font)";
  let bits = [s.enforced ? "Team" : "Personal"];
  if (s.is_default) bits.push("your default");
  if (s.enforced) bits.push(s.members + (s.members === 1 ? " member" : " members"), "you're " + (s.role || "reader"));
  meta.textContent = bits.join(" · ");
  body.append(name, meta);
  const actions = document.createElement("div");
  actions.className = "dev-actions";
  if (s.is_active) {
    const badge = document.createElement("span");
    badge.className = "badge-you";
    badge.textContent = "Active";
    actions.appendChild(badge);
  } else {
    actions.appendChild(btnQuiet("Use", () => useSpace(s)));
  }
  if (s.enforced) actions.appendChild(btnQuiet("Manage", () => openManage(s)));
  actions.appendChild(btnQuiet("Export", () => openPass("export", s)));
  if (!s.is_default) actions.appendChild(btnQuiet("Leave", () => leaveSpace(s)));
  el.append(av, body, actions);
  if (canAnim) el.animate([{ opacity: 0, transform: "translateY(8px)" }, { opacity: 1, transform: "none" }], { duration: 220, delay: Math.min(i, 8) * 28, easing: "cubic-bezier(.22,1,.36,1)" });
  return el;
}
async function useSpace(s) {
  try {
    await invoke("switch_space", { id: s.id });
    toast("Now working in “" + s.name + "”", "ok");
    await loadSpaces();
    refreshNotes();
  } catch (e) { toast(String(e), "err"); }
}
async function leaveSpace(s) {
  if (!confirm(`Leave “${s.name}”?\n\nIts data is deleted from THIS device. ${s.enforced ? "Other members keep their copies." : "Export it first if you want to keep it."}`)) return;
  try { await invoke("leave_space", { id: s.id }); toast("Left the space", "ok"); await loadSpaces(); refreshNotes(); }
  catch (e) { toast(String(e), "err"); }
}

/* create space */
$$("#space-type .chip").forEach((c) => c.addEventListener("click", () => {
  $$("#space-type .chip").forEach((x) => x.classList.remove("on"));
  c.classList.add("on");
  createType = c.dataset.type;
  $("#space-type-hint").textContent = createType === "team"
    ? "For a trusted circle. You're the admin; add devices with Reader / Writer / Admin roles."
    : "Just for you and your own devices — everyone is a full writer.";
}));
$("#space-new").addEventListener("click", () => {
  $("#space-name").value = "";
  $("#space-create-error").textContent = "";
  $$("#space-type .chip").forEach((x) => x.classList.toggle("on", x.dataset.type === "personal"));
  createType = "personal";
  $("#space-type-hint").textContent = "Just for you and your own devices — everyone is a full writer.";
  openModal("#space-create-modal");
  setTimeout(() => $("#space-name").focus(), 60);
});
$("#space-create-cancel").addEventListener("click", () => closeModal("#space-create-modal"));
$("#space-create-modal").addEventListener("click", (e) => { if (e.target.id === "space-create-modal") closeModal("#space-create-modal"); });
$("#space-create-go").addEventListener("click", async () => {
  const name = $("#space-name").value.trim();
  if (!name) { $("#space-create-error").textContent = "Give the space a name."; return; }
  const btn = $("#space-create-go"); btn.disabled = true;
  try {
    await invoke("create_space", { name, team: createType === "team" });
    closeModal("#space-create-modal");
    toast("Space created", "ok");
    await loadSpaces();
  } catch (e) { $("#space-create-error").textContent = String(e); }
  finally { btn.disabled = false; }
});

/* manage team space */
async function openManage(s) {
  manageSpaceId = s.id;
  $("#space-manage-title").textContent = "Manage “" + s.name + "”";
  $("#space-manage-id").textContent = s.id;
  $("#member-id").value = "";
  $("#member-error").textContent = "";
  openModal("#space-manage-modal");
  await refreshMembers();
  await refreshAudit();
}
async function refreshMembers() {
  const wrap = $("#space-members");
  wrap.innerHTML = "";
  try {
    const members = await invoke("space_members", { id: manageSpaceId });
    members.forEach((m) => wrap.appendChild(memberEl(m)));
  } catch (e) { toast(String(e), "err"); }
}
function memberEl(m) {
  const el = document.createElement("div");
  el.className = "dev";
  el.innerHTML = `<div class="dev-avatar">${DEV_ICON}</div><div class="dev-body"><div class="dev-name"></div><div class="dev-meta"></div></div><div class="dev-actions"></div>`;
  el.querySelector(".dev-name").textContent = m.is_me ? "This device" : shortId(m.id);
  el.querySelector(".dev-meta").textContent = m.role + " · " + shortId(m.id);
  el.querySelector(".dev-meta").title = m.id;
  const acts = el.querySelector(".dev-actions");
  if (m.is_me) {
    const b = document.createElement("span"); b.className = "badge-you"; b.textContent = m.role; acts.appendChild(b);
  } else {
    const sel = document.createElement("select");
    sel.className = "input select sm";
    ["reader", "writer", "admin"].forEach((r) => {
      const o = document.createElement("option");
      o.value = r; o.textContent = r[0].toUpperCase() + r.slice(1);
      if (r === m.role) o.selected = true;
      sel.appendChild(o);
    });
    sel.addEventListener("change", async () => {
      try { await invoke("space_set_role", { id: manageSpaceId, endpoint: m.id, role: sel.value }); toast("Role updated", "ok"); refreshMembers(); refreshAudit(); }
      catch (e) { toast(String(e), "err"); refreshMembers(); }
    });
    acts.appendChild(sel);
    acts.appendChild(btnQuiet("Remove", async () => {
      if (!confirm("Remove this device? It's revoked and the space key is rotated, so it can't follow new changes.")) return;
      try { await invoke("space_remove_member", { id: manageSpaceId, endpoint: m.id }); toast("Removed & re-keyed", "ok"); refreshMembers(); refreshAudit(); loadSpaces(); }
      catch (e) { toast(String(e), "err"); }
    }));
  }
  return el;
}
$("#member-add").addEventListener("click", async () => {
  const endpoint = $("#member-id").value.trim();
  const role = $("#member-role").value;
  $("#member-error").textContent = "";
  if (!endpoint) { $("#member-error").textContent = "Paste the device's id."; return; }
  try {
    await invoke("space_add_member", { id: manageSpaceId, endpoint, role });
    $("#member-id").value = "";
    toast("Member added", "ok");
    refreshMembers(); refreshAudit(); loadSpaces();
  } catch (e) { $("#member-error").textContent = String(e); }
});
async function refreshAudit() {
  const wrap = $("#space-audit");
  wrap.innerHTML = "";
  try {
    const entries = await invoke("space_audit", { id: manageSpaceId });
    entries.forEach((e) => {
      const row = document.createElement("div");
      row.className = "audit-row";
      const act = document.createElement("span");
      act.className = "audit-act";
      act.textContent = e.action.replace(/:/g, " ");
      const who = document.createElement("span");
      who.className = "audit-who";
      who.textContent = e.target ? shortId(e.target) : "by " + shortId(e.signer);
      who.title = e.target || e.signer;
      row.append(act, who);
      wrap.appendChild(row);
    });
  } catch (_) {}
}
$("#space-manage-id").addEventListener("click", () => {
  const id = $("#space-manage-id").textContent;
  if (id && navigator.clipboard) { navigator.clipboard.writeText(id); toast("Space id copied", "ok"); }
});
$("#space-manage-close").addEventListener("click", () => closeModal("#space-manage-modal"));
$("#space-manage-modal").addEventListener("click", (e) => { if (e.target.id === "space-manage-modal") closeModal("#space-manage-modal"); });

/* export / import via passphrase */
function openPass(mode, s) {
  passMode = mode;
  passSpaceId = s ? s.id : null;
  $("#space-pass").value = "";
  $("#space-pass-error").textContent = "";
  const exporting = mode === "export";
  $("#space-pass-title").textContent = exporting ? "Export “" + s.name + "”" : "Import a space";
  $("#space-pass-sub").textContent = exporting
    ? "Choose a passphrase. You'll need it to import this backup on another device — there's no way to recover it if you forget it."
    : "Enter the passphrase the backup was exported with, then choose the .kithspace file.";
  $("#space-pass-go").querySelector(".js-label").textContent = exporting ? "Export" : "Choose file & import";
  openModal("#space-pass-modal");
  setTimeout(() => $("#space-pass").focus(), 60);
}
$("#space-import").addEventListener("click", () => openPass("import", null));
$("#space-pass-cancel").addEventListener("click", () => closeModal("#space-pass-modal"));
$("#space-pass-modal").addEventListener("click", (e) => { if (e.target.id === "space-pass-modal") closeModal("#space-pass-modal"); });
$("#space-pass-go").addEventListener("click", async () => {
  const passphrase = $("#space-pass").value;
  if (!passphrase || passphrase.length < 6) { $("#space-pass-error").textContent = "Use at least 6 characters."; return; }
  const btn = $("#space-pass-go"); btn.disabled = true;
  try {
    if (passMode === "export") {
      const path = await invoke("space_export", { id: passSpaceId, passphrase });
      closeModal("#space-pass-modal");
      if (path) toast("Exported to " + path, "ok");
    } else {
      await invoke("space_import", { passphrase });
      closeModal("#space-pass-modal");
      toast("Space imported", "ok");
      await loadSpaces();
    }
  } catch (e) { $("#space-pass-error").textContent = String(e); }
  finally { btn.disabled = false; }
});

/* -------------------------------- network ------------------------------- */
let netMode = "decentralized";
$$("#network-mode .chip").forEach((c) => c.addEventListener("click", () => {
  $$("#network-mode .chip").forEach((x) => x.classList.remove("on"));
  c.classList.add("on");
  netMode = c.dataset.mode;
  $("#network-fields").classList.toggle("hidden", netMode !== "self_hosted");
}));
if ($("#set-network-change")) $("#set-network-change").addEventListener("click", async () => {
  $("#network-error").textContent = "";
  let n = { mode: "decentralized", relay_url: "", relay_token: "", pkarr_relay: "", origin_domain: "" };
  try { n = await invoke("get_network"); } catch (_) {}
  netMode = n.mode === "self_hosted" ? "self_hosted" : "decentralized";
  $$("#network-mode .chip").forEach((x) => x.classList.toggle("on", x.dataset.mode === netMode));
  $("#network-fields").classList.toggle("hidden", netMode !== "self_hosted");
  $("#net-relay").value = n.relay_url || "";
  $("#net-token").value = n.relay_token || "";
  $("#net-pkarr").value = n.pkarr_relay || "";
  $("#net-origin").value = n.origin_domain || "";
  openModal("#network-modal");
});
$("#network-cancel").addEventListener("click", () => closeModal("#network-modal"));
$("#network-modal").addEventListener("click", (e) => { if (e.target.id === "network-modal") closeModal("#network-modal"); });
$("#network-go").addEventListener("click", async () => {
  $("#network-error").textContent = "";
  const settings = {
    mode: netMode,
    relay_url: $("#net-relay").value.trim(),
    relay_token: $("#net-token").value.trim(),
    pkarr_relay: $("#net-pkarr").value.trim(),
    origin_domain: $("#net-origin").value.trim(),
  };
  if (netMode === "self_hosted" && !settings.relay_url) { $("#network-error").textContent = "Enter your relay URL."; return; }
  const btn = $("#network-go"); btn.disabled = true;
  btn.querySelector(".js-label").textContent = "Applying…";
  try {
    await invoke("set_network", { settings });
    closeModal("#network-modal");
    toast("Network updated — reconnecting", "ok");
    loadSettings();
  } catch (e) { $("#network-error").textContent = String(e); }
  finally { btn.disabled = false; btn.querySelector(".js-label").textContent = "Apply"; }
});

/* ------------------------------ onboarding ------------------------------ */
function dismissWelcome() {
  localStorage.setItem("kith-welcomed", "1");
  closeModal("#welcome-modal");
}
if ($("#welcome-skip")) $("#welcome-skip").addEventListener("click", dismissWelcome);
if ($("#welcome-link")) $("#welcome-link").addEventListener("click", () => {
  dismissWelcome();
  switchView("devices");
  setTimeout(() => $("#open-link").click(), 220);
});
async function maybeOnboard() {
  if (localStorage.getItem("kith-welcomed")) return;
  try {
    const notes = await invoke("list_notes");
    const d = await invoke("list_devices");
    const fresh = (!notes || !notes.length) && (!d.linked || !d.linked.length);
    if (fresh) openModal("#welcome-modal");
    else localStorage.setItem("kith-welcomed", "1");
  } catch (_) {}
}

/* ------------------------------ global keys ----------------------------- */
document.addEventListener("keydown", (e) => {
  if (e.key === "Escape") {
    const dismiss = ["#space-pass-modal", "#space-manage-modal", "#space-create-modal", "#network-modal"]
      .find((id) => !$(id).classList.contains("hidden"));
    if (dismiss) closeModal(dismiss);
    else if (!$("#link-modal").classList.contains("hidden")) closeLink();
    else if (!$("#join-modal").classList.contains("hidden")) closeJoin();
    else if (!$("#welcome-modal").classList.contains("hidden")) dismissWelcome();
  }
});
$("#welcome-modal").addEventListener("click", (e) => { if (e.target.id === "welcome-modal") dismissWelcome(); });

/* --------------------------------- init --------------------------------- */
(async function init() {
  requestAnimationFrame(() => moveIndicator($(".nav-item.is-active")));
  if (document.fonts && document.fonts.ready) document.fonts.ready.then(() => moveIndicator($(".nav-item.is-active")));
  refreshNotes();
  try { const v = await invoke("app_version"); $("#app-version").textContent = "v" + v; } catch (_) {}
  try { const id = await invoke("my_endpoint_id"); $("#about-id").textContent = id; $("#about-id").title = id; } catch (_) {}
  try { const d = await invoke("list_devices"); setNetDot(d.linked || []); } catch (_) {}
  maybeOnboard();
  // Light periodic refresh so notes synced from other devices appear.
  setInterval(() => { if (curView === "notes" && document.hasFocus() && $("#link-modal").classList.contains("hidden") && $("#join-modal").classList.contains("hidden")) refreshNotes(); }, 3500);
})();
