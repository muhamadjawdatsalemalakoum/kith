# Roadmap

The durable plan. When a step-by-step decision feels like it might drift, check it
against the **North Star** and **Invariants** below — those don't change without a
deliberate call. Everything else is sequence and can flex.

> Naming note: the repo is `centralTabs` (the first app's name), but the *product*
> is the engine. The engine/family has **no final name yet** — see Open Decisions.

---

## North Star (the one thing not to lose)

**The engine is the product.** A serverless, end-to-end-encrypted, no-account,
agent-accessible private mesh that syncs a person's own devices (and small trusted
groups) — mutable state *and* files — with no server that can read the data. Apps
(tabs, agent-memory, vault, file-sharing, a mesh-browser, chat) are **thin things
that run on it**, the way iroh-blobs/-docs run on iroh.

The market scan confirmed this is real: the most painful 2026 problems are *literally
thin apps on this engine* = `Automerge CRDT + iroh-blobs + SPAKE2 pairing + a local
MCP server`. The eventual "post-servers SDK" is the someday-north-star — **earned by
shipping 2-3 real apps first, never led with.**

---

## Invariants (guardrails — every decision passes these or it's wrong)

1. **Serverless means "no server that can read your data"** — *not* zero-infra. One
   optional dumb, ciphertext-blind relay/mailbox is allowed; a data custodian is not.
2. **No account.** A short pairing code (SPAKE2 group key) is the sign-in.
3. **End-to-end encrypted / zero-knowledge** — the TARGET. Transport is E2E today;
   access control + at-rest encryption land in **M1.5** and are required before any
   private-data / open-internet use (see README "Security status").
4. **Desktop-only.** Windows 10+, macOS 10.13+ (Intel **and** Apple Silicon), Linux.
   Mobile is out of scope (a read-only PWA viewer is possible *only* if a server is
   ever added — a different, later path).
5. **Offline-tolerant, not always-available.** If all a user's devices are off,
   updates are *pending, not lost*. Don't promise 24/7 availability.
6. **The engine wraps the volatile deps** (iroh, automerge, iroh-blobs, spake2)
   behind one boundary. Apps speak engine types + the re-exported `automerge` only.
7. **Deep before wide.** Prove/harden a primitive before adding apps that need it.
8. **One wedge at a time.** The backlog is a *map*, not a to-do-all list.
9. **The anti-fit test** (reject on sight): anything needing 24/7 availability,
   public/anonymous fan-out, server-authoritative logic (payments/trust/moderation),
   mobile-background, anonymity, or search over data we don't hold — is NOT our fit.
10. **Free-forever, unlimited, open-source.** License: MIT OR Apache-2.0 (matches the
    Dropwire family; revisit only if a hosted tier is ever added).

---

## Status today

- ✅ **mesh-engine** — the substrate. identity (Ed25519), endpoint (mainline-DHT
  discovery + relay fallback), SPAKE2 pairing, **state-sync primitive** (Automerge
  over iroh), **blob primitive** (iroh-blobs, serve/fetch a file). Two ALPNs on one
  endpoint.
- ✅ **apps/tabs** (`centraltabs`) — app #1: tab schema + thin `Tabs` facade.
- ✅ `cargo test --workspace` = **6/6 green, 0 warnings** (convergence, carry_forward
  A→C→B, blob_transfer, tab_sync, pairing ×2).

---

## Milestones

Sizes are rough effort (S/M/L), not dates.

### M0 — Engine core ✅ DONE
State + blob primitives, pairing, first app, all tested.

### M1 — Engine hardening + the mesh layer  ·  L  ·  ✅ DONE
Make the engine production-shaped before adding apps (Invariant 7).
*Shipped: auto-syncing mesh (peer set + background loop + `add_peer`/`announce_change`),
persistence (atomic save/restore of the replica), HMAC-SHA256 pairing confirmation
(replaced the stub), relay-path test (state syncs over a real relay). 9 tests green.*
- **Flat-mesh orchestration**: a peer set; **auto-sync on local change** (today sync
  is manual per-connection); group membership/announce so peers find each other
  beyond a hand-passed address.
- **Persistence**: wire `doc::save`/`load` (save-each-change + periodic compaction);
  reload + re-sync on restart.
- **Real-transport test**: the relay-path test (`iroh::test_utils::run_relay_server`)
  + a DHT/real-network smoke test (prove it works off-loopback).
- **Security**: replace the insecure `pairing::confirm_tag` stub with HMAC-SHA256/HKDF.
- **Offline mailbox**: *decide* (build a tiny ciphertext-blind always-on peer, or
  defer). Design now, build only if a wedge needs it.
- **Exit:** N peers in a group converge automatically on edit; state survives restart;
  relay-path green; pairing confirmed cryptographically.

### M1.5 — Security: access control + at-rest encryption + pairing  ·  L  ·  ✅ DONE (alpha)
*Shipped + locally tested: group-key auth gate on sync (mutual HMAC challenge — wrong
key rejected before any data), at-rest AEAD (XChaCha20-Poly1305) for the replica,
SPAKE2 over-the-wire pairing (join from a short code), key-rotation revocation, and
blobs off-by-default. Remaining for "beta": move the at-rest key to an OS keystore /
passphrase, an independent crypto audit, and (nice) automatic epoch rotation. The
original design notes are kept below for reference.*

Designed and implemented:
- **Access control (CRITICAL):** gate inbound sync (`MeshSync::accept`) and blob GETs
  (a `BlobsProtocol` auth handler) by an authorized-EndpointId set — reject non-members
  before any merge/serve. iroh authenticates the peer's EndpointId at the QUIC layer,
  so this is an allowlist check (`conn.remote_node_id()` ∈ authorized), not new crypto.
- **Pairing handshake:** over a pairing ALPN, run the existing SPAKE2 from a short
  code, confirm with the HMAC tag (constant-time `subtle::ConstantTimeEq`), then
  exchange + persist EndpointIds into the authorized set and distribute the group key.
  (SPAKE2 derivation + the HMAC confirm tag are done + unit-tested; only the wire
  handshake remains.)
- **At-rest encryption (CRITICAL):** AEAD (XChaCha20-Poly1305) over the replica
  snapshot + blobs, keyed via HKDF from the group key — the root key must NOT sit next
  to the data (passphrase or OS keystore). Decide E2E-vs-server-search tradeoff here.
- **Revocation:** group-key epoch + rotation on device removal; `remove_peer`.
- **Tests:** unauthorized peer rejected; authorized peer syncs; at-rest encrypt/
  decrypt round-trip; pairing yields matching authorized sets. NOTE: crypto SOUNDNESS
  needs a security review, not just functional tests.

### M2 — `mesh-mcp` capability layer (the recurring moat)  ·  M  ·  ✅ DONE
The differentiator in ~4 of the top-5 wedges. Build it once, arm every app.
*Shipped: hand-rolled MCP host (JSON-RPC over stdio, no external SDK), the `McpApp`
trait (declare tools + handle calls), `serve_stdio`. Tabs wired as proof —
`tabs.add`/`count`/`first_url`; an agent drives the app over MCP in a test. 11 tests green.*
- A `mesh-mcp` crate: a trait apps implement (`mcp_tools()` / `mcp_resources()`) +
  a local MCP server host (stdio) the engine wires up automatically.
- Keep it behind its own boundary (MCP churn isolated, per Invariant 6).
- **Proof:** wire the tabs app — a local MCP client (e.g. Claude Desktop) can call
  `tabs.search` / `tabs.add` / read tabs over the mesh.
- **Exit:** an external MCP client reads+writes a mesh app's data through mesh-mcp.

### M3 — Wedge #1: agent-memory (the launch bet)  ·  M–L  ·  ✅ DONE
*Shipped: `apps/memory` (`agent-memory`) — Automerge memory schema (append/all/search/
forget with tombstones), a `Memory` facade, and the MCP surface
(memory.append/search/read/forget). Tests: memory follows you across devices
(taught on one, recalled + forgotten on another) and an agent drives it over MCP.*
"Your AI memory, on your machines, no vendor lock-in, agent-accessible."
- Automerge memory schema (facts / preferences / profile / project context).
- Local MCP: `memory.read` / `append` / `search` / `forget` — any agent on any of
  your devices reads+writes the same memory.
- One-shot importers for ChatGPT / Claude exports.
- **Exit:** memory taught to an agent on device A is readable by the agent on device
  B after sync; import works; runs as a quiet tray daemon.

### M4 — Make it real (runnable)  ·  M  ·  ✅ DONE
- ✅ **Desktop app** (`kith`, Tauri v2): Memory, Tabs, Files, Devices (pairing), and
  Agents (MCP) — see `apps/desktop`. The same binary runs `kith serve` (unified MCP
  server) for Claude Desktop / Cursor; `agent-memory` remains as a memory-only CLI.
- ✅ **Device pairing UI**: link by short code (SPAKE2 over the wire), mutual so both
  sides converge; the engine peers back with the joiner.

### M5 — Launch  ·  M  ·  ◑ PARTIAL
- ✅ **CI** (`.github/workflows/ci.yml`): cross-OS tests + the relay-path test +
  `fmt --check` + `clippy -D warnings`, plus a `desktop` job that builds, lints, and
  tests the GUI app on all three OSes (webview deps installed).
- ✅ **Release pipeline** (`.github/workflows/release.yml`): builds + bundles the Kith
  desktop **installers** (Windows `.exe`, macOS universal `.dmg`, Linux
  `.AppImage`/`.deb`/`.rpm`) on a `v*` tag, version pinned from the tag.
- ✅ **Dual license** (`LICENSE-MIT` + `LICENSE-APACHE`) + root `README` + install /
  pairing / privacy / releasing docs (`docs/`).
- ⬜ **Needs your accounts** (cannot be automated): code signing + macOS notarization,
  the auto-updater signing key (see `docs/RELEASING.md`), a landing page + demo GIF,
  and the actual Product Hunt launch.
- **Exit:** publicly installable + launched.

---

## Engine v2 — Spaces, per-device roles, revocation (in progress)

Closing the gap between the stated vision (a multi-Space, per-device-identity,
role-aware, revocable mesh) and the single-group engine that shipped. Sequenced
substrate-first; each milestone lands with green tests.

### EV2-M1 — Spaces (multi-group substrate) · ✅ DONE
A device now runs **N independent encrypted Spaces** over ONE endpoint + ONE device
identity. Each Space owns its group key, at-rest key, replica, blob store, peers, and
`data_dir/spaces/<id>/` subdir. The sync, blob, and pairing accept handlers route by a
`SpaceId` carried first on the wire, so edits in Space A converge only to A's members
and never leak into B.
- **Default Space:** every device has one with a well-known constant `SpaceId`
  (independent of the group key, so it survives epoch rotation). This preserves the
  pre-Spaces single-group behaviour — two devices with the same key still converge — and
  keeps the existing apps working on the **active Space** (`Mesh::doc` etc.).
- **API:** `create_space` / `join_space` / `list_spaces` / `leave_space` /
  `set_active_space` / `space(id)` (a scoped `SpaceHandle`); pairing hands over
  `(SpaceId, group key)`. A pre-Spaces flat install is migrated into the default Space.
- **Tests:** `two_spaces_isolated`, `three_peer_convergence_per_space`,
  `space_carry_forward`, `cross_space_blob_isolation`,
  `migrates_flat_layout_into_default_space` — plus every prior test still green, and the
  relay-path test. `clippy -D warnings` + `fmt` clean; `forbid(unsafe_code)` intact.

### EV2-M2 — Per-device identity, membership & roles · ⬜ NEXT
Membership rooted in **EndpointId** (not mere possession of the shared key), with signed
membership changes and enforceable `Admin` / `Writer` / `Reader` roles (cryptographic
read-only against honest peers via per-writer change signing).

### EV2-M3 — Revocation, key epochs & audit log · ⬜
Epoch rekey on device removal (post-removal confidentiality for future data — **not**
forward secrecy for already-synced data), re-encrypt at rest under the new epoch, and a
hash-chained per-Space audit log.

> Later (not in the current substrate pass): `files.read` (EV2-M4), self-hosted relay
> (EV2-M5), MCP per-Space binding (EV2-M6), keychain + encrypted export/recovery
> (EV2-M7), multi-stream throughput + a measured benchmark (EV2-M8), and wiring every
> capability through the desktop GUI (EV2-M10).

---

## Backlog (buckets — unscheduled, pull from the map one at a time)

**Fast-follow apps** (all thin, on engine + mesh-mcp):
- Household / trusted-group "brain" (wedge #1 by fit; scope to desktop small-teams,
  NOT literal families — they're mobile-native).
- CRDT password/secrets vault (structurally cures KeePass conflict-copies).
- Account-free shared folder ("the other person needs no account").
- Dropwire-on-mesh (file transfer, validates the blob primitive as a full app).
- Group chat (state app; introduces multi-user membership).
- Mesh-browser (webview + a `mesh://` scheme over blobs — a flagship *demo*, hard
  standalone product; Beaker/Agregore are the cautionary prior art).

**Engine extras:** self-hosted relay + DNS, optional encrypted mailbox, key
rotation / lost-device re-pair flow, E2E-mode tradeoff decisions.

**Someday north-star:** extract a narrow `mesh-sdk` for third parties; the ecosystem
/ "app store" of mesh apps. Only after 2-3 first-party apps prove the engine.

---

## Open decisions (decide deliberately, don't drift)

- **Which wedge is the launch flagship?** ✅ **DECIDED (2026-06-21): agent-memory.**
  Dead-center of the AI/MCP/engine-as-platform vision, desktop-native evangelist
  audience, weak incumbents (a race but an open early one). Tabs stays the proof-
  anchor; household/team brain is the queued fast-follow on the same engine + mesh-mcp.
- **Cockpit timing**: tray daemon vs full UI, and whether it's needed at M3 or M4.
- **Offline mailbox**: build the dumb ciphertext-blind always-on peer, or ship
  opportunistic-sync + encrypted file export first?
- **The name** of the engine/family/product.

---

## Parking lot — ANTI-FITS (do NOT build, however shiny at 2am)

All of these *feel* like a fit and fail an Invariant (availability / mobile /
server-authoritative / anonymity):
WeTransfer-to-a-stranger · whistleblower/source intake · family-photo cloud
(Google Photos replacement) · grocery / shared-lists app · Notion-style shared
multi-writer workspace · law-firm ↔ external-client exchange · cross-device
clipboard handoff · private AI notetaker.
