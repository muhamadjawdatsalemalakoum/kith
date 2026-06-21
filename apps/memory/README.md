# agent-memory

Your AI memory, on your own machines. Portable, vendor-neutral memory that syncs
across **your** devices over the mesh — no account, no cloud, end-to-end encrypted —
and is readable + writable by **any** AI agent over MCP. Your model becomes
swappable; the memory stays yours.

Built on [`mesh-engine`](../../mesh-engine) (the serverless P2P substrate) and
[`mesh-mcp`](../../mesh-mcp) (the local MCP host). This crate is just the memory
schema + a thin facade — the hard parts (sync, pairing, offline tolerance,
encryption, MCP) come from the engine.

## Use it with Claude Desktop (or any MCP client)

Build the binary:

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

State lives in `~/.centraltabs/memory` (override with `CENTRALTABS_MEMORY_DIR`).

## Status / honesty

- ⚠️ **ALPHA (locally tested; not independently audited).** Now enforced: group-key
  access control (only your paired devices may sync — wrong key rejected), account-free
  SPAKE2 pairing from a short code, encrypted-at-rest, and key-rotation revocation.
  Caveats before relying on it: the at-rest key sits in a `0600` file (OS-keystore /
  passphrase upgrade pending), the crypto has no independent audit, and real-NAT / DHT
  behavior is hand-tested. See the workspace `ROADMAP.md` (M1.5).
- Cross-device sync, persistence, and the MCP surface are implemented and tested.
- **Device pairing CLI** (exchange a short SPAKE2 code to link a new device) is the
  next piece — today the engine pairs via the library API; a `pair` subcommand is
  on the roadmap.
- Desktop tray/GUI cockpit is a later nicety; the MCP server needs no UI.
