# agent-memory

The memory app of [Kith](../../README.md): portable, vendor-neutral memory that syncs
across **your** devices over the mesh — no account, no cloud, end-to-end encrypted — and
is readable + writable by **any** AI agent over MCP. Your model becomes swappable; the
memory stays yours.

Built on [`mesh-engine`](../../mesh-engine) (the serverless P2P substrate) and
[`mesh-mcp`](../../mesh-mcp) (the local MCP host). This crate is just the memory schema +
a thin facade — the hard parts (sync, pairing, offline tolerance, encryption, MCP) come
from the engine.

> **Most people should use the Kith desktop app** ([`apps/desktop`](../desktop)), which
> bundles this memory app with tabs, files, a device-pairing UI, and a unified MCP server
> (`kith serve`) — see the top-level [README](../../README.md). This crate is the memory
> component and a small standalone CLI.

## Use it with Claude Desktop (or any MCP client)

The Kith desktop app exposes all apps (memory, tabs, files) through one MCP server — open
its **Agents** tab to copy a `kith serve` config. To use *just* this memory crate as a
standalone server, build it:

```
cargo build --release -p agent-memory
# -> target/release/agent-memory  (.exe on Windows)
```

Add it to your MCP client. For Claude Desktop (`claude_desktop_config.json`):

```json
{
  "mcpServers": {
    "agent-memory": {
      "command": "/absolute/path/to/agent-memory",
      "args": ["serve"]
    }
  }
}
```

Now any chat can call:

- `memory.append { text, kind }` — remember a fact/preference
- `memory.search { query }` — recall relevant memories
- `memory.read` — list everything remembered
- `memory.forget { id }` — forget an entry

Run the same binary on your other computers and the memory **syncs across all of
them** automatically — no account, nothing in anyone's cloud.

## CLI

```
agent-memory remember "the user prefers Rust and dark mode"
agent-memory recall "rust"
agent-memory id
```

State lives in `~/.kith/memory` (override with `KITH_MEMORY_DIR`). A pre-existing
`~/.centraltabs/memory` directory and the legacy `CENTRALTABS_MEMORY_DIR` variable are
still honored, so upgrading keeps your synced data.

## Status / honesty

- ⚠️ **ALPHA (locally tested; not independently audited).** Now enforced: account-free
  SPAKE2 pairing from a short code, group-key access control, encrypted-at-rest, multiple
  isolated **Spaces**, and — in Team spaces — EndpointId membership with `Admin`/`Writer`/
  `Reader` roles plus epoch-key revocation. Keys can live in the OS keychain, and a Space
  exports to an encrypted, passphrase-protected file (the no-account recovery path).
  Caveats before relying on it: the crypto has no independent audit, role enforcement holds
  against honest peers, revocation protects future (not already-synced) data, and real-NAT /
  DHT behavior is hand-tested. See the workspace `ROADMAP.md` and `SECURITY.md`.
- Cross-device sync, persistence, and the MCP surface are implemented and tested.
- **Device pairing has a UI** in the Kith desktop app (Devices → Link a device /
  Enter a code). This crate exposes pairing via the library API; the desktop app drives it.
- **The desktop GUI exists** ([`apps/desktop`](../desktop)) and is the recommended way to
  run Kith; this standalone crate remains for a memory-only MCP server + CLI.
