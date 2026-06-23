# Changelog

All notable changes to this project will be documented in this file.

The format is based on Keep a Changelog, and this project follows Semantic
Versioning.

## [Unreleased]

## [0.1.0] - 2026-06-23

### Added

- **Spaces** — run N independent, end-to-end-encrypted spaces over one connection and one
  device identity; switch the active space and Memory/Tabs/Files follow it. Edits in one
  space never reach another (`two_spaces_isolated`, `cross_space_blob_isolation`).
- **Per-device roles** — Team spaces root membership in device identity (EndpointId), not
  mere key possession, with `Admin`/`Writer`/`Reader` roles enforced cryptographically
  against honest peers: a Reader's writes are rejected and a non-member is refused even with
  a leaked group key (`reader_write_rejected`, `non_member_endpointid_rejected_even_with_group_key`).
- **Revocation** — removing a device rotates an Admin-signed **epoch key** distributed only
  to remaining members and re-keys at-rest data (post-removal confidentiality for *future*
  data — not forward secrecy, not a retroactive wipe).
- **Tamper-evident audit log** — a signed, hash-chained per-space membership log that fails
  to load if altered (`audit_log_hash_chain_detects_tampering`).
- **`files.read` / `files.search`** — agents read file *contents* across devices (chunked,
  path-traversal-safe), not just fetch a path.
- **MCP per-space binding** — each MCP server is bound to exactly one human-selected space;
  no tool accepts a space argument, so a prompt-injected agent cannot cross spaces.
- **Self-hosted relay / discovery** (`Infra::SelfHosted`) — run your own relay + pkarr/DNS
  instead of n0's; deploy configs and a guide in `infra/`.
- **OS-keychain key storage** (Windows Credential Manager / macOS Keychain; hardened-file
  fallback on Linux) and **encrypted, passphrase-protected Space export/import** (Argon2id +
  XChaCha20-Poly1305) — the no-account recovery path.
- **Throughput** — larger QUIC windows + multi-stream blob fetch + a benchmark/resume
  harness (see `docs/throughput.md`).

- **Kith desktop app** (`kith`, Tauri v2) for Windows/macOS/Linux, with Memory, Tabs,
  Files, Devices (pairing), and Agents (MCP) surfaces.
- **`kith-files`** app: file sharing on the engine's blob primitive — offer/list/fetch
  with live progress, a direct-vs-relayed badge, and per-file open-location/rename/remove.
- **Unified `kith serve` MCP server** aggregating `memory.*`, `tabs.*`, and `files.*`
  tools on one shared engine, so an AI assistant can drive all of Kith locally.
- Cross-platform installer pipeline (tauri-action) and a CI job that builds + lints the
  desktop app on all three OSes.

- **Per-device sync status** — the engine tracks each peer's last successful sync; the
  Devices view shows "synced N ago" and the status light reflects real recency.
- **Transfer history** — a local "Recent transfers" list in Files (sent/received, size,
  peer, time, open-location).
- **Settings** (download folder + data-dir reveal), first-run **onboarding**,
  **window-state** persistence, and crash logging to `kith.log`.
- Download **cancel**, and a **Reset & re-key** action that rotates the group key so
  removed devices can no longer sync.

### Changed

- **Blob transfer is now group-key gated** (mutual HMAC), matching document sync — a
  non-member can no longer fetch a blob by hash. (Regression test: `stranger_cannot_fetch_blob`.)
- **Pairing is now mutual**: the host learns the joiner's identity and peers back, so
  both sides converge (not just the joiner dialing the host).
- Engine peer set deduplicates and supports removal (cancellable per-peer sync tasks).
- Offers survive a restart (re-served from the local path); downloads never overwrite an
  existing file (auto-disambiguated); at-rest/group key files are hardened on Windows too.

## [0.0.1] - 2026-06-21

### Added

- Initial Rust workspace for Kith.
- `mesh-engine` peer-to-peer synchronization engine with identity, pairing,
  encrypted transport, CRDT state sync, blob transfer, and at-rest encryption
  modules.
- `mesh-mcp` support crate for exposing app data and actions through MCP.
- `agent-memory` flagship binary and `centraltabs` proof application.
- Project documentation, dual MIT/Apache-2.0 licensing, contribution guidance,
  security policy, CI, and release packaging workflows.

[Unreleased]: https://github.com/muhamadjawdatsalemalakoum/kith/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/muhamadjawdatsalemalakoum/kith/compare/v0.0.1...v0.1.0
[0.0.1]: https://github.com/muhamadjawdatsalemalakoum/kith/releases/tag/v0.0.1
