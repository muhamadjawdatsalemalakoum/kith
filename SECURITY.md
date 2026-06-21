# Security Policy

Kith handles people's private data, so we take security reports seriously. Please
read this before reporting.

## Status: alpha — not independently audited

Kith is **alpha** software. It is built on well-known building blocks (iroh
QUIC/TLS 1.3 transport, SPAKE2 pairing, HMAC-SHA256 authentication, and
XChaCha20-Poly1305 at-rest encryption) and is covered by local tests, but it has
**not** had an independent security review, and `spake2 0.4` is itself unreviewed.

Current limitations (also noted in the [README](README.md#security-status)):

- The at-rest encryption key is stored in a `0600`-permissioned file in the data
  directory. This guards against a stray copy of the data file, but not against
  someone who already has full read access to the directory. Moving the key into an
  OS keychain / passphrase is planned.
- Removing a device is a manual step (rotate the group key, then re-pair). Data a
  device synced before removal is not retroactively protected.
- Real-network NAT traversal and live DHT/relay behavior are checked by hand rather
  than in CI.

Because of the above, please treat Kith as alpha and don't rely on it yet for data
you couldn't afford to expose.

## Supported versions

This is pre-`0.1`; only the latest `main` / latest release receives fixes.

| Version | Supported |
| ------- | --------- |
| latest `main` | ✅ |
| older         | ❌ |

## Reporting an issue

**Please don't open a public issue for security problems.** Report privately via one
of:

1. **GitHub Security Advisories** — the "Report a vulnerability" button under the
   repository's *Security* tab (preferred; keeps the report private and tracked).
2. **Email** — `keon.me@gmail.com` with `[Kith security]` in the subject.

Helpful details to include:

- a description of the issue and its impact,
- steps to reproduce, or a small example that demonstrates it,
- the affected component (`mesh-engine`, `mesh-mcp`, an app) and version/commit.

You can expect an acknowledgement within a few days. We'll work with you on a fix and
a sensible disclosure timeline, and credit you in the release notes unless you prefer
to stay anonymous.
