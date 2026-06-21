# Contributing to Kith

Thanks for your interest. Kith is a serverless, end-to-end-encrypted P2P engine and
a family of thin apps that run on it. This guide covers how to build, test, and
propose changes.

> **Status: alpha.** APIs and on-disk formats may change without notice before `0.1`.

## Prerequisites

- Rust (stable). The toolchain is pinned in [`rust-toolchain.toml`](rust-toolchain.toml);
  `rustup` will install the right one automatically.
- Git.

## Prerequisites (desktop app)

The `kith` desktop app is built with [Tauri v2](https://v2.tauri.app). To build or run
it you also need the Tauri prerequisites for your OS (a C toolchain, and on Linux
`libwebkit2gtk-4.1-dev libappindicator3-dev librsvg2-dev patchelf`). The frontend is
plain HTML/CSS/JS in `apps/desktop/ui` — no Node build step.

## Build & test

```sh
# Build / test everything EXCEPT the GUI crate (which needs webview libs)
cargo build --workspace --exclude kith
cargo test  --workspace --exclude kith

# Real-transport (in-process relay) test
cargo test -p mesh-engine --features test-utils --test relay

# The desktop app
cargo run -p kith            # build + launch the GUI
cargo build -p kith          # compile-check it
cargo tauri build            # build installers (needs: cargo install tauri-cli)

# Lints — these are CI gates and must pass clean
cargo fmt --all --check
cargo clippy --workspace --all-targets --exclude kith -- -D warnings
cargo clippy -p kith --all-targets -- -D warnings
```

CI runs all of the above on Linux, macOS, and Windows (the GUI crate in its own job with
the webview deps installed). A PR must be green before merge.

## Workspace layout

```
mesh-engine/   the substrate (identity, discovery, pairing, group-key-gated sync + blobs)
mesh-mcp/      the local MCP host (one trait → AI-agent-accessible app)
apps/memory/   agent-memory — the memory app/schema
apps/tabs/     centraltabs — tab/link sync
apps/files/    kith-files — file sharing on the blob primitive
apps/desktop/  kith — the Tauri desktop app + `kith serve` unified MCP server
```

## Architectural rules (please respect these)

These keep the project coherent — see [`ROADMAP.md`](ROADMAP.md) for the full set of
invariants.

1. **The engine wraps the volatile dependencies.** Only `mesh-engine` touches `iroh`,
   `automerge`, `iroh-blobs`, and `spake2`. Apps speak engine types and the engine's
   re-exported `automerge` — never depend on those crates directly from an app.
2. **`unsafe` is forbidden** (`unsafe_code = "forbid"` in every crate).
3. **No secrets or per-device state in git.** Identity keys, group keys, and data
   directories are gitignored — keep it that way.
4. **Deep before wide.** Harden a primitive before adding apps that depend on it.

## Proposing changes

1. Open an issue first for anything non-trivial, so we can agree on the approach.
2. Branch from `main`, keep changes focused, add tests for new behavior.
3. Ensure `cargo test`, `fmt --check`, and `clippy -D warnings` all pass.
4. Write clear commit messages explaining the *why*, not just the *what*.
5. Open a PR; fill in the template; link the issue.

## Security

Please do **not** open public issues for security vulnerabilities. See
[`SECURITY.md`](SECURITY.md) for private reporting.

## License

By contributing, you agree that your contributions are dual-licensed under
**MIT OR Apache-2.0**, matching the project.
