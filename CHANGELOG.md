# Changelog

All notable changes to this project will be documented in this file.

The format is based on Keep a Changelog, and this project follows Semantic
Versioning.

## [Unreleased]

### Added

- **Kith desktop app** (`kith`, Tauri v2) for Windows/macOS/Linux, with Memory, Tabs,
  Files, Devices (pairing), and Agents (MCP) surfaces.
- **`kith-files`** app: file sharing on the engine's blob primitive — offer/list/fetch
  with live progress, a direct-vs-relayed badge, and per-file open-location/rename/remove.
- **Unified `kith serve` MCP server** aggregating `memory.*`, `tabs.*`, and `files.*`
  tools on one shared engine, so an AI assistant can drive all of Kith locally.
- Cross-platform installer pipeline (tauri-action) and a CI job that builds + lints the
  desktop app on all three OSes.

### Changed

- **Blob transfer is now group-key gated** (mutual HMAC), matching document sync — a
  non-member can no longer fetch a blob by hash. (Regression test: `stranger_cannot_fetch_blob`.)
- **Pairing is now mutual**: the host learns the joiner's identity and peers back, so
  both sides converge (not just the joiner dialing the host).
- Engine peer set deduplicates and supports removal (cancellable per-peer sync tasks).

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

[Unreleased]: https://github.com/muhamadjawdatsalemalakoum/kith/compare/v0.0.1...HEAD
[0.0.1]: https://github.com/muhamadjawdatsalemalakoum/kith/releases/tag/v0.0.1
