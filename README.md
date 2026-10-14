# sama

[![release](https://img.shields.io/github/v/release/swastikgn/sama)](https://github.com/swastikgn/sama/releases)
[![ci](https://img.shields.io/github/actions/workflow/status/swastikgn/sama/release.yml?label=release%20build)](https://github.com/swastikgn/sama/actions)

A TCP (L4) load balancer. It forwards raw TCP connections to configured
backends using one of six algorithms — round robin, weighted round robin,
least connections, power of two choices, random, consistent hash — switchable
at runtime via the built-in dashboard. Ships as a single binary.

Built with Rust (tokio + axum). The dashboard is Vue + Vite, compiled into the
binary and served by it.

<img width="3420" height="1894" alt="image" src="https://github.com/user-attachments/assets/481fa908-0faa-4671-a81c-618425138e89" />

## Features

- **6 balancing algorithms** — round robin, weighted round robin,
  least connections, power of two choices, random, consistent hash
  (same client IP sticks to the same backend). Switchable at runtime
  from the dashboard.
- **Health checking** — TCP-connect probe every 2 s (600 ms timeout);
  3 consecutive failures take a backend out, 2 consecutive successes
  bring it back.
- **Failover** — if dialing a backend fails, the next one is tried,
  up to 3 backends per client connection.
- **Change backends without restarting** — add, remove, change weights,
  drain, all from the dashboard (drain a backend before deleting it).
  Changes are saved to `config.toml`, so they survive restarts.
- **Dashboard** — backend health, active/total connections, bytes in/out,
  weights, draining, algorithm picker, dark/light mode.
- **Connection cap** — `max_connections` limits concurrent proxied
  connections; rejected ones are counted.
- **Graceful shutdown** — SIGTERM/Ctrl-C stops accepting new connections
  and waits for in-flight ones (up to 10 s) before exiting.

## Quickstart

1. Download the binary for your platform from
   [Releases](https://github.com/swastikgn/sama/releases) and make it executable
   (not needed on Windows):
   ```sh
   chmod +x sama-*-x86_64-unknown-linux-gnu
   ```
2. Copy the example config and point it at your backends:
   ```sh
   # grab config.example.toml from the repo, then:
   cp config.example.toml config.toml
   # edit config.toml: listen_address, admin_listen_address, servers
   ```
3. Run it:
   ```sh
   ./sama-*-x86_64-unknown-linux-gnu --config config.toml
   ```
4. Open the dashboard at `http://127.0.0.1:8001/` and send traffic to
   `127.0.0.1:8000`.

## Installation

**Option 1 — release binary (recommended).** One file per platform, published
on every `v*` tag:

| File                                        | Platform            |
| ------------------------------------------- | ------------------- |
| `sama-<version>-x86_64-unknown-linux-gnu`   | Linux x86_64        |
| `sama-<version>-aarch64-unknown-linux-gnu`  | Linux ARM64         |
| `sama-<version>-aarch64-apple-darwin`       | macOS Apple Silicon |
| `sama-<version>-x86_64-apple-darwin`        | macOS Intel         |
| `sama-<version>-x86_64-pc-windows-msvc.exe` | Windows x86_64      |

**Option 2 — from source.** Requires Rust (stable) and Node.js 22+:

```sh
cp config.example.toml config.toml
./scripts/package-release.sh
./dist/sama-* --config config.toml
```

The script builds the UI first (required — it is embedded into the binary
at compile time), then the release binary, and stages it in `dist/`.

## Configuration

`config.toml` (see `config.example.toml`, documented inline):

| Key                    | Default          | Description                                                                                                          |
| ---------------------- | ---------------- | -------------------------------------------------------------------------------------------------------------------- |
| `listen_address`       | —                | TCP proxy listener, e.g. `127.0.0.1:8000`                                                                            |
| `admin_listen_address` | `127.0.0.1:8001` | Dashboard listener                                                                                                   |
| `algorithm`            | —                | `round_robin` · `weighted_round_robin` · `least_connections` · `power_of_two_choices` · `random` · `consistent_hash` |
| `max_connections`      | `1024`           | Concurrent proxy connection cap (excess is rejected + counted)                                                       |
| `listen_backlog`       | `512`            | TCP listen backlog                                                                                                   |
| `servers[].address`    | —                | Backend `ip:port`                                                                                                    |
| `servers[].weight`     | —                | `1`–`1000`, used by weighted algorithms                                                                              |
| `servers[].enabled`    | `true`           | `false` removes it from rotation                                                                                     |
| `servers[].draining`   | `false`          | `true` stops new connections, keeps existing ones                                                                    |

Validation: dashboard and proxy addresses must differ from each other and from
every backend; no duplicates; weights ≥ 1. Defaults follow the Kong
convention — proxy `8000`, dashboard `8001`, backends `8002+`.

## How it works

```
clients ──TCP──▶ :8000 (proxy) ──TCP──▶ healthy backends
                    │
                    ▼
              :8001 dashboard
```

- The proxy is a semaphore-gated TCP relay: pick a backend from a lock-free
  snapshot, dial (2 s timeout), then splice bytes both ways (8 KiB buffers,
  60 s idle timeout) while counting bytes per backend.
- A background prober dials every backend every 2 s and flips health flags
  (fall 3 / rise 2). The picker only sees healthy, enabled, non-draining
  backends; dead ones get up to 3 failover attempts per connection.
- Dashboard changes go through a single config-writer task: validate → persist TOML
  → publish a new snapshot. Readers never block.

## Benchmarks

Measured over loopback against 3 local backends, one HTTP request per TCP
connection, zero errors across all runs. All six algorithms perform the same
within run-to-run variance (expected — picking a backend costs nanoseconds
against millisecond connections).

| Concurrent connections | Throughput | p50     | p99      |
| ---------------------- | ---------- | ------- | -------- |
| 10                     | ~14k req/s | ~0.5 ms | ~0.7 ms  |
| 50                     | ~14k req/s | ~2.8 ms | ~3.7 ms  |
| 200                    | ~14k req/s | ~12 ms  | ~120 ms+ |

This is a connection-setup benchmark: every request pays two full TCP
lifecycles, so throughput is capped by connection churn, not by forwarding
speed. With keep-alive or real network round-trips in play, expect
different (better) numbers.

## Development

```sh
cargo test
cd ui && npm run build && cd ..
cargo run -- --config config.toml -v
cd ui && npm run dev
```

`RUST_LOG` overrides log levels, e.g. `RUST_LOG=sama=debug`.
