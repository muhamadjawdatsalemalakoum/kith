# Security Policy

Kith handles people's private data, so we take security reports seriously. Please
read this before reporting.

## Status: alpha — not independently audited

Kith is **alpha** software. It is built on well-known building blocks (iroh
QUIC/TLS 1.3 transport, SPAKE2 pairing, HMAC-SHA256 group-key authentication, and
XChaCha20-Poly1305 at-rest encryption) and is covered by tests, but it has **not**
had an independent security review, and `spake2 0.4` is itself unreviewed.

What's enforced (with tests):

- **Spaces.** A device runs N independent encrypted Spaces over one endpoint; edits in
  one Space never reach another, and a member of one Space cannot sync or fetch blobs
  from another (`two_spaces_isolated`, `cross_space_blob_isolation`).
- Both **document sync and file transfer** require a mutual group-key proof — a peer
  that can't prove the shared key is refused before any byte flows
  (`wrong_group_key_cannot_sync`, `stranger_cannot_fetch_blob`).
- **Membership & roles (role-enforced Spaces).** Membership is rooted in **device
  identity (EndpointId), not mere possession of the group key**: a signed, hash-chained
  membership log whose root **Admin** is cryptographically bound to the SpaceId. A
  non-member is refused at connect even if it holds the group key
  (`non_member_endpointid_rejected_even_with_group_key`). Roles
  (`Admin`/`Writer`/`Reader`) are enforced **cryptographically against honest peers**:
  every Automerge change carries its author's Ed25519 signature, and an honest peer
  applies a change only if its author is a `Writer`/`Admin`, so a `Reader`'s (or a
  leaked-key non-member's) writes are rejected (`reader_write_rejected`). Membership
  changes must be signed by an Admin (`membership_change_requires_admin`,
  `forged_membership_entry_is_rejected_on_replay`); blob serving is gated to
  `Writer`/`Admin` (`writer_can_write_reader_cannot_share_blob`). The hash-chained log
  is tamper-evident (`tampering_breaks_the_hash_chain`).
- Pairing derives the group key from a short code via SPAKE2 with a mandatory,
  constant-time key-confirmation round; a wrong code hands out nothing.
- The replica is encrypted at rest (XChaCha20-Poly1305).

Current limitations:

- The at-rest and group keys are stored in files in the data directory (`0600` on
  Unix; an ACL restricted to the current user on Windows). This guards a stray copy,
  but not someone who already has full read access to the directory. Moving the keys
  into an OS keychain / a passphrase is planned.
- **Role enforcement** holds against *honest* peers (it relies on honest peers to drop
  unauthorized changes; an honest majority is assumed for liveness). It does not stop a
  Reader from forking its own private copy locally — only from getting its writes
  accepted by others.
- **Concurrent membership edits** are reconciled by longest-valid-chain, not a full
  group-key-agreement (CGKA) merge; Admin actions should be serialized. Full
  forward-secret group agreement over unreliable P2P is out of scope.
- **Revocation = epoch rekey → post-removal confidentiality for *future* data.** Removing
  a member appends a signed removal to the log (so honest peers refuse the device at the
  connect gate) **and** rotates the Space's epoch key: a fresh, Admin-signed key is minted,
  distributed to the remaining members over the authenticated, membership-gated channel
  (the removed device fails the gate and never receives it), and the at-rest snapshot is
  re-encrypted under the new epoch (`revoked_device_cannot_sync_new_epoch`,
  `remaining_members_get_new_key_and_converge`, `at_rest_reencrypted_under_new_epoch`).
  This is **not** forward secrecy and **not** a retroactive wipe: a removed device keeps
  whatever it already synced, and the guarantee is post-removal confidentiality of future
  data in the honest-peer model. (The legacy default-Space "Reset & re-key" still rotates
  the group key for single-group installs.)
- **Audit log.** Each role-enforced Space's signed, hash-chained membership log doubles as
  a tamper-evident audit log (space-created, member added/removed/role-changed, key
  rotations, pairings). Because a tampered log fails to replay, it cannot be silently
  altered (`audit_log_hash_chain_detects_tampering`). Honest limit: it covers
  membership/admin actions, not every local read, and a peer can still withhold its own
  local view.
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
