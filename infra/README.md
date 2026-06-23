# Kith self-hosted infrastructure

Two small, self-hosted services let Kith work across the internet **without depending on
n0's free public relays** (which are dev/test only — rate-limited, no SLA). Everything here is
"build up to the gate": the configs and containers are ready; you provide a server, a domain, and
run one command. Nothing here ever sees your data — the relay only forwards encrypted
packets it cannot decrypt, and discovery only maps a public key to a reachable address.

Kith ships **serverless by default** (`Infra::Decentralized`: public Mainline-DHT discovery + n0's
free relays as fallback). You only need this if you want **zero dependence on n0** — full control of
both the relay and discovery. Point the app at it with `CoreConfig::self_hosted(..)` /
`Infra::SelfHosted` (Advanced settings in the desktop app, or a release build with the values baked in).

## What runs

| Service | What it does | Why you need it |
|---|---|---|
| **iroh-relay** (`relay/`) | Forwards **encrypted** packets between two peers when a direct connection can't be hole-punched. Also helps with NAT discovery. | The fallback path. Without it, the minority of links that can't go direct (CGNAT / symmetric NAT) would fail. It carries bytes only for that minority — that's your only real recurring cost. |
| **iroh-dns-server** (`dns/`) | pkarr-based discovery: lets a device find another by its public key. | How two devices find each other with no account. |

Both are open-source binaries from the [iroh](https://github.com/n0-computer/iroh) monorepo, built
from a pinned tag in the Dockerfiles. Pin the tag to the **same iroh 1.0.x** the app ships.

## Layout

```
infra/
├─ README.md           # this file
├─ deploy.md           # step-by-step deploy + the human-gate checklist
├─ .env.example        # the relay access token (copy to relay/.env)
├─ relay/              # the relay service (own host, owns :443)
│  ├─ docker-compose.yml
│  ├─ Dockerfile
│  └─ relay.toml
└─ dns/                # the discovery service (own host, owns :443 + :53)
   ├─ docker-compose.yml
   ├─ Dockerfile
   └─ config.toml
```

## Why two hosts

Both services want port **443** (LetsEncrypt TLS), so the simplest correct layout is **one small
VPS each** (≈ $5/mo each on a flat-egress host like Hetzner — flat/unmetered egress
matters most for the relay). They can be co-located behind a reverse proxy, but
the relay's QUIC (UDP 9889) and the DNS server's UDP/TCP 53 don't proxy cleanly, so separate hosts
keep it simple and robust. Start here; scale the relay horizontally on bandwidth later.

## Lock it to your circle (no user accounts)

The relay is locked to your Kith build with a **shared token** (`access.shared_token` /
`IROH_RELAY_ACCESS_TOKEN`). The app presents the same token on connect, so third parties can't use
your relay as an open proxy — and this is **app-level** access control, not a user login, so the
"no account" promise holds. Leave the token empty for an open relay (anyone can relay through it).

➡️ Next: [`deploy.md`](deploy.md).
