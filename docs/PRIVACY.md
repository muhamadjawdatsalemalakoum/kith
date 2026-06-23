# Privacy

Kith is built so that **no company — including us — can see your data**, because there
is no server in the middle. This page states plainly what is and isn't stored, and what
crosses the network.

## No accounts, no tracking

- **No account, no sign-in, no email.** Linking devices uses a one-time code, not an account.
- **No analytics, no telemetry, no phone-home.** Kith makes no requests to any Kith server
  (there isn't one).

## What lives on your machine

Everything is under your data directory (`~/.kith/memory` by default; override with
`KITH_MEMORY_DIR`):

| Path | What it is |
|---|---|
| `node.key` | this device's cryptographic identity (one per device, across all spaces) |
| `spaces/<id>/` | one folder per **space** you're in (the default space, plus any you create/join) |
| `spaces/<id>/doc.automerge` | that space's encrypted replica — memory, tabs, and file *offers* (not file bytes) |
| `spaces/<id>/group.key`, `epochs.bin` | that space's keys (the rotating epoch key for team spaces) |
| `spaces/<id>/members.log` | a team space's signed, hash-chained membership + audit log |
| `spaces/<id>/blobs/` | content store for files shared or downloaded in that space |
| `devices.json` | the devices you've linked (id + a friendly name you choose) |
| `network.json`, `settings.json` | your network (relay) choice and app settings |
| `offered_paths.json`, `downloads.json`, `history.json` | **local-only** file id → path maps + transfer history, so the app can "open location". Never synced. |

Your notes, tabs, and the file index are **encrypted at rest** (XChaCha20-Poly1305). The
encryption keys are kept in your **OS keychain** (Windows Credential Manager / macOS
Keychain) where available, falling back to a hardened key file on platforms without one
(e.g. headless Linux). Because there's no account, an **encrypted, passphrase-protected
space export** is your only recovery path — see [SECURITY.md](../SECURITY.md).

## What crosses the network

- **Sync + file transfer** flow **directly between your linked devices**, end-to-end
  encrypted (iroh QUIC / TLS 1.3), and are **gated by your group key** — a device that
  can't prove the shared key is refused before any byte is exchanged.
- **Discovery** uses the public mainline DHT: to be reachable, your device publishes its
  endpoint id and a relay address to the DHT. This is connection *metadata*, not your data.
- **Relays** (n0's free relays) are used only as a fallback when a direct connection can't
  be made. A relay only ever forwards **ciphertext** it cannot read.

If all your devices are offline, changes are **pending, not lost** — they sync when a
device comes back online.

## Your AI assistant (MCP)

When you connect an MCP client (e.g. Claude Desktop), it talks to a **local** `kith serve`
process on your machine, which reads/writes the same local replica. The server is bound to
a single space (the active one) — no tool can address another space, so a prompt-injected
agent can't cross spaces. Whatever your AI does with your memory/tabs/files is governed by
that client's own data handling — Kith itself sends nothing to a Kith server.
