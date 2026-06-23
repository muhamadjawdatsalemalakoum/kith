<p align="center">
  <img src="assets/brand/kith-readme-banner-1280x640.png" alt="Kith — your circle of devices, in sync" width="820">
</p>

<p align="center">
  <a href="https://github.com/muhamadjawdatsalemalakoum/kith/actions/workflows/ci.yml"><img src="https://github.com/muhamadjawdatsalemalakoum/kith/actions/workflows/ci.yml/badge.svg?branch=main" alt="CI"></a>
  <a href="https://github.com/muhamadjawdatsalemalakoum/kith/releases"><img src="https://img.shields.io/github/v/release/muhamadjawdatsalemalakoum/kith?include_prereleases&label=download" alt="Download"></a>
  <img src="https://img.shields.io/badge/status-alpha-orange" alt="Status: alpha">
  <img src="https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue" alt="License: MIT OR Apache-2.0">
</p>

> *kith* — one's own trusted circle. **Kith keeps your circle of devices in sync.**

Kith is a **serverless, end-to-end-encrypted, no-account** desktop app for keeping your
own devices in sync — your **memory/notes**, your **saved tabs**, and your **files** —
directly device to device, with **no server that can read them, because there isn't one.**
Linking a device is a short pairing code, not an account. And because it speaks **MCP**,
your AI assistant can read and add to the very same data.

Built on [iroh] (QUIC transport, mainline-DHT discovery) and [Automerge] (CRDTs).

## What you get

A single desktop app (Windows · macOS · Linux) with:

- **🧠 Memory** — notes and facts that sync across every linked device.
- **🔖 Tabs** — save links/pages and have them everywhere.
- **📁 Files** — send files straight to your own devices: end-to-end encrypted, no size
  limit, no cloud, with live progress and a direct-vs-relayed badge.
- **🌐 Spaces** — run several independent, end-to-end-encrypted worlds at once: a
  **Personal** space for yourself, or a **Team** space with per-device roles
  (Admin / Writer / Reader) for a trusted circle. Each space has its own keys, members,
  and audit log; export any space to an encrypted file for backup or to move it.
- **🔗 Devices** — link another computer with a one-time code (SPAKE2). No account.
- **🤖 Agents** — point Claude Desktop / Cursor at Kith over MCP; your AI can use your
  memory, tabs, and files locally — bound to the **active space only**, so a
  prompt-injected agent can't reach another space.

## Install

**Download** a build for your OS from the [Releases page](https://github.com/muhamadjawdatsalemalakoum/kith/releases)
(Windows `.exe`, macOS `.dmg`, Linux `.AppImage`/`.deb`/`.rpm`). Builds are currently
unsigned (alpha), so your OS may warn about an unverified developer — on Windows choose
*More info → Run anyway*; on macOS use *System Settings → Privacy & Security → Open Anyway*.

**Or build from source** (needs the [Rust toolchain](https://rustup.rs) + the
[Tauri prerequisites](https://v2.tauri.app/start/prerequisites/) for your OS):

```sh
cargo run -p kith            # build + launch the desktop app
cargo tauri build            # build installers (needs: cargo install tauri-cli)
```

## Connect your AI (MCP)

The same `kith` binary doubles as a local MCP server. In the app, open **Agents** to copy
a ready-made config, or add this to your MCP client (e.g. Claude Desktop's
`claude_desktop_config.json`) and restart it:

```json
{ "mcpServers": { "kith": { "command": "/path/to/kith", "args": ["serve"] } } }
```

Your agent then gets `memory.*`, `tabs.*`, and `files.*` tools — operating on the **same**
encrypted replica the app uses, entirely on your machine. With the Kith app open,
`kith serve` automatically bridges to it (one shared engine); with the app closed, it
runs standalone.

## How it works

- **Flat mesh, no hub.** Every device is an equal peer holding a full encrypted replica.
  Changes propagate directly between your devices over [iroh] QUIC (TLS 1.3).
- **CRDT state + gated blobs.** Mutable state is an [Automerge] document (conflict-free,
  offline-tolerant); files ride content-addressed, BLAKE3-verified blob transfer. **Both
  sync and file transfer are gated by a mutual group-key handshake** — a non-member is
  refused before any byte is exchanged.
- **No account.** Devices link with a short pairing code (SPAKE2-derived group key);
  pairing is mutual, so both sides learn each other and converge.
- **Spaces, roles & revocation.** A device runs N independent encrypted spaces over one
  connection. A team space roots membership in **device identity (EndpointId), not mere
  possession of the key**: a signed, hash-chained membership log, with `Admin`/`Writer`/
  `Reader` roles enforced cryptographically against honest peers (a Reader's writes are
  rejected; a non-member is refused even with a leaked key). Removing a device rotates an
  **epoch key** so it can't follow future changes.
- **Self-hostable.** Ships serverless by default (public DHT + n0's free relays as
  fallback); run your own relay + discovery for full control (see [`infra/`](infra/)).
- **Honest serverless.** "No server that can read your data" — not zero-infra: a relay
  only ever forwards ciphertext. If all your devices are off, updates are *pending, not lost*.

See [docs/PAIRING.md](docs/PAIRING.md) for the link flow and [docs/PRIVACY.md](docs/PRIVACY.md)
for exactly what is and isn't stored.

## Repository layout

```
mesh-engine/    the substrate: per-device identity, mainline-DHT discovery, relay
                fallback (or self-hosted), SPAKE2 pairing, N isolated encrypted Spaces,
                EndpointId membership + signed roles, epoch-key revocation + audit log,
                CRDT sync + multi-stream blob transfer, keychain keys + encrypted
                export. The ONLY crate that touches iroh / automerge.
mesh-mcp/       a tiny MCP host — implement one trait and an app's data + actions
                become readable/writable by any AI agent, with no server in the loop.
apps/memory/    agent-memory: portable, vendor-neutral memory (the memory schema).
apps/tabs/      centraltabs: cross-device tab/link sync.
apps/files/     kith-files: file sharing on the blob primitive.
apps/desktop/   ★ kith: the Tauri desktop app (GUI) + `kith serve` unified MCP server
                that runs all three apps on one shared engine.
```

The engine is the product; the apps are thin and share one `Mesh` (one identity, one
pairing, one replica). See [ROADMAP.md](ROADMAP.md) for invariants and what's next.

## Security status — alpha (locally tested; not independently audited)

Enforced today, each with tests:

- ✅ **Transport** is end-to-end encrypted (iroh QUIC / TLS 1.3).
- ✅ **Spaces are isolated** — edits in one space never reach another, and a member of one
  space cannot sync or fetch blobs from another (`two_spaces_isolated`,
  `cross_space_blob_isolation`).
- ✅ **Membership rooted in EndpointId + enforced roles** — a team space's signed,
  hash-chained membership log binds its root Admin to the space id; a non-member is refused
  even with a leaked group key, and a Reader's writes are rejected by honest peers because
  every change carries its author's Ed25519 signature (`non_member_endpointid_rejected_even_with_group_key`,
  `reader_write_rejected`, `membership_change_requires_admin`).
- ✅ **Revocation via epoch rekey** — removing a device rotates an Admin-signed epoch key
  distributed only to remaining members, and re-keys at-rest data, giving **post-removal
  confidentiality for future data** (`revoked_device_cannot_sync_new_epoch`,
  `remaining_members_get_new_key_and_converge`, `removed_device_cannot_fetch_blob`).
- ✅ **Tamper-evident audit log** — the hash-chained membership log records the lifecycle
  and fails to load if altered (`audit_log_hash_chain_detects_tampering`).
- ✅ **Keys in the OS keychain** (Windows Credential Manager / macOS Keychain; hardened-file
  fallback on Linux) and an **encrypted, passphrase-protected space export** as the
  no-account recovery path (`keychain_roundtrip`, `space_export_import_roundtrip`).
- ✅ **Account-free, mutual pairing** via SPAKE2; the replica is XChaCha20-Poly1305 encrypted
  at rest.

Honest caveats (the path from alpha to "trust it broadly"):

- The crypto has **not** had an independent audit (and `spake2 0.4` is itself unaudited).
  No coding pass substitutes for a review before trusting multi-human spaces with sensitive data.
- Revocation gives post-removal confidentiality for *future* data — it is **not** forward
  secrecy for data a removed device already synced, and **not** a retroactive wipe.
- Role enforcement holds against *honest* peers (an honest majority is assumed for liveness);
  a removed device keeps whatever it already has, and concurrent Admin edits are reconciled
  by longest-valid-chain, not full group-key agreement.
- On Linux (no keychain backend compiled) keys fall back to a hardened key file.
- Throughput is tuned for "good enough + resumable", not FASP/Aspera-class; BBR isn't applied
  (an upstream limitation — see [`docs/throughput.md`](docs/throughput.md)).
- Real-NAT hole-punching and live-DHT/relay behavior are verified by hand, not in CI.

See [SECURITY.md](SECURITY.md) to report an issue.

## License

Dual-licensed under [MIT](LICENSE-MIT) **or** [Apache-2.0](LICENSE-APACHE), at your option.

[iroh]: https://iroh.computer
[Automerge]: https://automerge.org
