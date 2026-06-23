# Blob transfer throughput

Kith's goal for file transfer is **"good enough, resumable, and automatic"** — direct
(hole-punched) when possible, relay fallback when not, resuming after interruption — not
beating FASP/Aspera. This note records what we tune, why, and the measured numbers.

## What we tune

### 1. QUIC receive/send windows (the dominant fix)

The default QUIC `stream_receive_window` is far below the bandwidth-delay product (BDP) of a
real link, so a single stream stalls waiting for ACKs — the cause of the ~1%-of-LAN
throughput reported in [n0-computer/iroh#4286](https://github.com/n0-computer/iroh/issues/4286)
(~1.3 MB/s default vs ~42–50 MB/s tuned on a LAN). `mesh-engine`'s endpoint sets
(`endpoint.rs::tuned_transport`):

- `stream_receive_window = 16 MiB`
- `receive_window = 64 MiB` (per connection)
- `send_window = 64 MiB`

These cover the BDP of, e.g., a 100 ms × 500 Mbit link (~6 MiB) with headroom, so a single
stream is no longer flow-control-limited. This is the change that matters most on real,
latent links.

### 2. Multi-stream range fetch (parallelism on top)

`blobs::ensure_local_multi` fetches a large blob over **N concurrent range requests**
(separate QUIC streams on one connection), each an independent BLAKE3-verified GET of a byte
segment. `Mesh::fetch_file_multi` uses it (4 streams) for large blobs. Benefits:

- On a **latent** link, parallel streams keep more data in flight than one stream's window.
- Even on **loopback** it's faster, because it parallelizes the CPU-bound bao/BLAKE3
  verification across cores.

### 3. Congestion controller — BBR not applied (documented blocker)

The default controller is CUBIC. BBR usually beats CUBIC on lossy/long links. iroh re-exports
the `ControllerFactory` *trait* but **not** a concrete BBR config, so selecting BBR would
require depending on iroh's internal `noq_proto` fork **pinned to its exact version** — fragile
and not a stable public API. We therefore ship CUBIC + the window tuning above (which captures
the bulk of the single-stream win) and leave BBR for when iroh exposes it publicly. This is the
honest "ship the best available tuning + document why" outcome.

## Measured numbers

From `cargo test -p mesh-engine --test throughput -- --nocapture` (dev machine, **loopback**,
64 MiB blob — a *local high-throughput path*, not a real network):

| Fetch | Time | Sustained |
|---|---|---|
| single-stream | ~831 ms | ~77 MB/s |
| multi-stream (4) | ~685 ms | ~93 MB/s |

The benchmark (`benchmark_records_throughput`) writes a results file to the temp dir on each
run. Loopback is CPU-bound, so these reflect verification/copy speed, not network behavior —
the window-tuning win shows on real latent links, which we measure by hand (point the
`SelfHosted`/`Decentralized` config at two machines on different networks and watch the
direct/relayed badge).

## Resume

Interrupted transfers resume: partial data persists in the content store, and the next fetch
requests only what's missing (`local.missing()`), BLAKE3-verified end to end. Covered by
`large_blob_resumes_after_interrupt`.

## NAT / path notes

- **Direct first, relay fallback.** iroh hole-punches a direct path when possible and falls
  back to a relay otherwise. The UI shows a direct/relayed badge per transfer.
- **CGNAT / symmetric NAT.** When both peers are behind symmetric NAT or CGNAT, hole-punching
  typically fails and the transfer goes via the relay — correct but bounded by the relay's
  per-client bandwidth cap (`infra/relay/relay.toml [limits]`). This is the minority of links
  and the only real recurring cost of self-hosting a relay.
- **LAN.** On the same LAN, peers connect directly at local-link speed with no relay.
