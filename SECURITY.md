# Security Policy

Kith handles people's private data, so we take security reports seriously. Please
read this before reporting.

## Status: alpha — not independently audited

Kith is **alpha** software. It is built on well-known building blocks (iroh
QUIC/TLS 1.3 transport, SPAKE2 pairing, HMAC-SHA256 group-key authentication, and
XChaCha20-Poly1305 at-rest encryption) and is covered by tests, but it has **not**
had an independent security review, and `spake2 0.4` is itself unreviewed.

What's enforced (with tests):

- Both **document sync and file transfer** require a mutual group-key proof — a peer
  that can't prove the shared key is refused before any byte flows
  (`wrong_group_key_cannot_sync`, `stranger_cannot_fetch_blob`).
- Pairing derives the group key from a short code via SPAKE2 with a mandatory,
  constant-time key-confirmation round; a wrong code hands out nothing.
- The replica is encrypted at rest (XChaCha20-Poly1305).

Current limitations:

- The at-rest and group keys are stored in files in the data directory (`0600` on
  Unix; an ACL restricted to the current user on Windows). This guards a stray copy,
  but not someone who already has full read access to the directory. Moving the keys
  into an OS keychain / a passphrase is planned.
- **Revocation:** "Reset & re-key" rotates the group key so removed devices can no
  longer authenticate, and you re-pair the ones you keep. There is no forward secrecy
  for data a device already synced before removal.
- `spake2 0.4` is unaudited and not constant-time; pairing depends on it (mitigated by
  the mandatory confirmation round, but it's a residual risk).
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
