<p align="center">
  <img src="assets/brand/kith-readme-banner-1280x640.png" alt="Kith — your circle of devices, in sync" width="820">
</p>

<p align="center">
  <a href="https://github.com/muhamadjawdatsalemalakoum/kith/actions/workflows/ci.yml"><img src="https://github.com/muhamadjawdatsalemalakoum/kith/actions/workflows/ci.yml/badge.svg?branch=main" alt="CI"></a>
  <a href="https://github.com/muhamadjawdatsalemalakoum/kith/actions/workflows/release.yml"><img src="https://github.com/muhamadjawdatsalemalakoum/kith/actions/workflows/release.yml/badge.svg" alt="Release"></a>
  <img src="https://img.shields.io/badge/status-alpha-orange" alt="Status: alpha">
  <img src="https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue" alt="License: MIT OR Apache-2.0">
</p>

> *kith* — one's own trusted circle. **Kith** keeps your circle of devices in sync.

A serverless, end-to-end-encrypted, **no-account** peer-to-peer engine — and a family
of thin apps that run on it. Your data and files sync directly across **your own
devices** (and small trusted groups), with **no server that can read them**, because
there isn't one. Joining is a short pairing code, not an account.

The engine is the product. Apps are thin. (Crates keep the `mesh-*` names internally;
the product is Kith — the protocol already speaks it: `kith/pair/1`.)

## What's here

```
mesh-engine/    the substrate: identity, mainline-DHT discovery, relay fallback,
                SPAKE2 pairing, CRDT state sync, content-addressed blob transfer.
                Auto-syncing, offline-tolerant, persisted. The ONLY crate that
                touches iroh / automerge.
mesh-mcp/       a tiny MCP (Model Context Protocol) host — implement one trait and
                an app's data + actions become readable/writable by ANY AI agent,
                with no server in the loop.
apps/memory/    ★ agent-memory (the flagship): portable, vendor-neutral AI memory
                that syncs across your machines and is agent-accessible over MCP.
apps/tabs/      centralTabs: cross-device tab sync (the engine's proof app).
```

## How it works

- **Flat mesh, no hub.** Every device is an equal peer holding a full encrypted
  replica. Changes propagate directly between your devices over [iroh] QUIC.
- **CRDT state + blobs.** Mutable state is an [Automerge] document (conflict-free,
  offline-tolerant); files ride content-addressed, BLAKE3-verified blob transfer.
- **No account.** Devices link with a short pairing code (SPAKE2 group key).
- **Honest serverless.** "No server that can read your data" — not zero-infra. If
  all your devices are off, updates are *pending, not lost*.

Scope is **desktop** (Windows 10+, macOS 10.13+ on Intel & Apple Silicon, Linux).
See [ROADMAP.md](ROADMAP.md) for the North Star, invariants, and what's next.

## Try the flagship

```
cargo build --release -p agent-memory
```

Point Claude Desktop (or any MCP client) at it — see [apps/memory/README.md](apps/memory/README.md):

```json
{ "mcpServers": { "agent-memory": { "command": "/path/to/agent-memory", "args": ["serve"] } } }
```

Then any chat can `memory.append` / `memory.search` / `memory.read` / `memory.forget`,
and your memory syncs across every device running it — no account, nothing in the cloud.

## Status

`cargo test --workspace` + the relay-path test are green (~30 tests), clippy-clean,
rustfmt-clean. Built and verified primitive-by-primitive:

| | |
|---|---|
| state sync | convergence, store-carry-forward (A→C→B), auto-sync, persistence |
| blobs | serve-a-file / fetch-a-file (byte-perfect), off by default |
| access control | wrong group key rejected; same key syncs; rotation evicts |
| pairing | join from a short code (SPAKE2 over the wire); wrong code fails |
| at rest | replica encrypted on disk (XChaCha20-Poly1305) |
| transport | direct loopback **and** real relay path |
| reliability | offline-peer errors bounded; one dead peer can't block others; frame caps + timeouts |
| integrity | corrupt-replica recovery; hardened identity; durable atomic saves |
| MCP | agent drives tabs **and** memory over the protocol; malformed-input handled |

## Security status — alpha (locally tested; not independently audited)

What's now enforced, each with local tests:

- ✅ **Transport** is end-to-end encrypted (iroh QUIC / TLS 1.3).
- ✅ **Access control** — only peers holding the shared group key may sync. A peer with
  the wrong key is rejected *before any data is exchanged* (mutual HMAC challenge over
  the group key). Tests: `wrong_group_key_cannot_sync`, `same_group_key_syncs`.
- ✅ **Account-free pairing** — a new device joins from a short code via SPAKE2; a wrong
  code hands out nothing. Tests: `pair_a_new_device_then_sync`, `wrong_pairing_code_fails`.
- ✅ **Encrypted at rest** — the replica is XChaCha20-Poly1305 encrypted on disk. Test:
  `replica_is_encrypted_at_rest`.
- ✅ **Revocation** — rotate the group key to evict a device. Test:
  `rotating_group_key_evicts_old_devices`.
- ✅ **Blobs off by default** — content serving is opt-in (`with_blobs(true)`).

Honest caveats to weigh before flipping the switch to public:

- The at-rest key currently lives in a `0600` file in the data dir — it guards a stray
  copy of the data file, **not** someone who already has full access to the whole
  directory. Moving it to an OS keychain / passphrase is the planned upgrade.
- The crypto has **not** had an independent security audit (and `spake2 0.4` is itself
  unaudited). Treat as alpha.
- Real-NAT hole-punching and live-DHT/relay behavior are verified by hand, not in CI.
- Revocation is manual (rotate, then re-pair the devices you keep); there's no forward
  secrecy for data a device already synced.

It is built to be private-data-capable; the remaining items above are the path from
"alpha" to "trust it broadly." See [ROADMAP.md](ROADMAP.md).

## License

MIT OR Apache-2.0.

[iroh]: https://iroh.computer
[Automerge]: https://automerge.org
