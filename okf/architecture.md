---
type: Architecture
title: Architecture
description: System architecture, module relationships, and data flow.
tags: [architecture, data-flow, modules]
timestamp: 2026-06-18T00:00:00Z
---

# Architecture

## Component diagram

```
┌──────────────────┐     ┌───────────────────┐
│  Ollama (remote/ │     ┌───────────────────┴───────────────────┐
│  localhost)      │     │   ollama_monitor (Rust + Axum)       │
│  :11434/api/*    │────▶│                                       │
└──────────────────┘     │  ┌────────┐ ┌────────┐ ┌──────────┐  │
                         │  │Config  │ │Ollama  │ │GPU      │  │
┌──────────────────┐     │  │Module  │ │Client  │ │Module   │  │
│  nvidia-smi CLI  │◄─────│  │(env)  │ │(HTTP)  │ │(CLI)    │  │
│  (same host GPU) │     │  └────────┘ └────────┘ └──────────┘  │
└──────────────────┘     │  ┌────────┐ ┌────────┐ ┌──────────┐  │
                         │  │System  │ │DB      │ │API/     │  │
┌──────────────────┐     │  │Metrics │ │(SQLite)│ │Dashboard│  │
│  /proc/stat      │◄─────│  │Module  │ │       │ │Module   │  │
│  /proc/meminfo   │     │  └────────┘ └────────┘ └──────────┘  │
└──────────────────┘     └──┬──────────────────────────────────┘
                            │
                            ▼
                     ┌──────────────────┐
                     │  REST API + Web  │
                     │  :3000/api/*     │
                     │  :3000/ (portal) │
                     └──────────────────┘
```

## GPU workload switching

Added alongside the monitoring path. On a single-GPU host only one model can be
VRAM-resident at a time, so switching is an *arbitration* problem rather than a
routing one.

```
  Dashboard / HTTP client
        │  POST /api/model {"target":"llama02"}
        ▼
  ┌────────────────────────────────────────────────────────────┐
  │ ollama_monitor (llama-mon CT)                              │
  │                                                            │
  │  SwitchPlan  ── pure state machine, no I/O, no clocks       │
  │      │  next(&WorldSnapshot) -> SwitchAction                │
  │      ▼                                                      │
  │  Switcher    ── driver: gathers snapshots, runs actions     │
  │      │              │                    │                  │
  └──────┼──────────────┼────────────────────┼──────────────────┘
         │              │                    │
         │   bearer     │  bearer            │  nvidia-smi (NVML totals)
         ▼              ▼                    ▼
  ┌────────────┐ ┌────────────┐      ┌──────────────────┐
  │ llama01 CT │ │ llama02 CT │      │ host GPU: how     │
  │ agent :8100│ │ agent :8100│      │ much card is free │
  │ + llama-   │ │ + llama-   │      └──────────────────┘
  │   server   │ │   server   │
  └────────────┘ └────────────┘      ┌────────────┐
                                     │ comfyui CT │  agent :8100
                                     └────────────┘
```

The split is deliberate and comes from a measured constraint:
`nvidia-smi --query-compute-apps` reports `process_name` as `[Not Found]` for
container processes (PID namespaces are not shared), so **NVML totals** are used as
the authoritative VRAM guard while **agents** provide attribution.

The ordering invariant — never start the target until VRAM is *observed* below
`SWITCH_VRAM_FLOOR_MIB`, and fail rather than proceed otherwise — is enforced by the
pure `SwitchPlan`, which makes it testable without HTTP, a GPU or sleeps. See
[Switcher Module](src/switcher.md).

Model discovery picks its client automatically: if any `NODE_<i>_*` llama node is
configured, the llama.cpp client is used; otherwise the Ollama client. This is why
an llama.cpp deployment no longer logs `Ollama connection failed` every cycle.

## Module summary

| Module | File | Responsibility |
|---|---|---|
| **Config** | [`src/config.rs`](src/config.md) | Load settings from env vars with defaults |
| **Ollama Client** | [`src/ollama.rs`](src/ollama.md) | HTTP client for Ollama `/api/tags` endpoint |
| **llama.cpp Client** | [`src/llamacpp.rs`](src/llamacpp.md) | HTTP client for llama-server `/v1/models` + `/props`; the only trustworthy readiness signal |
| **Switcher** | [`src/switcher.rs`](src/switcher.md) | VRAM arbitration: stop incumbent, confirm release, start target |
| **Switch Agent** | [`src/bin/switch_agent.rs`](deployment/switch-agent.md) | Second binary, deployed into each workload container; allowlisted `systemctl` behind a bearer token |
| **GPU** | [`src/gpu.rs`](src/gpu.md) | `nvidia-smi` CLI wrapper and CSV parser |
| **System Metrics** | [`src/system.rs`](src/system.md) | `/proc/stat` and `/proc/meminfo` reader |
| **Models** | [`src/models.rs`](src/models.md) | Shared data structures (serde derive) |
| **API** | [`src/api.rs`](src/api.md) | Axum REST server + embedded dashboard HTML |
| **Database** | [`src/db.rs`](db/schema.md) | SQLite pool, migrations, insert, and history queries |
| **Tests** | [`src/tests.rs`](src/tests.md) | Integration test harness with mock Ollama + mock nvidia-smi |

## Data flow

1. **Refresh loop** (`main.rs`) — A Tokio task runs `run_one_refresh` every `REFRESH_INTERVAL_SECS` seconds.
2. **Ollama fetch** — `OllamaClient` queries `{base_url}/api/tags`. On failure, model data is `None` (graceful degradation).
3. **GPU query** — `nvidia-smi --query-gpu=... --format=csv,noheader,nounits` executed on the configured device index. On failure, a placeholder `GpuMetric` (all nulls) is used.
4. **System query** — `/proc/stat` and `/proc/meminfo` read directly. On failure, a placeholder `SystemMetric` (all nulls) is used.
5. **Persistence** — A `CheckResult` row is inserted into SQLite. Rows older than 30 days are auto-pruned.
6. **State update** — Shared `Arc<RwLock<Option<MonitorStatus>>>` is updated with the latest combined status.
7. **API consumers** — The dashboard, or external clients (Home Assistant, curl, scripts), read the latest status or history from REST endpoints.

## Key design decisions

- **Graceful degradation** — Each collector (Ollama, GPU, System) is independent. Failure in one does not block the others.
- **No config files** — All configuration through environment variables. Simplifies container and systemd deployments, matches Kubernetes secrets injection patterns.
- **Generic refresh function** — `run_one_refresh` accepts a `Fn(usize) -> GpuMetric` callback. Production uses `try_query_gpu`, tests inject mock CLI scripts.
- **SQLite WAL mode** — Write-ahead logging + auto-vacuum for durability without WAL file management overhead.

## Dependencies

| Crate | Purpose |
|---|---|
| `axum 0.8` | HTTP server framework (with WebSocket support) |
| `reqwest 0.12` | HTTP client (rustls TLS) |
| `sqlx 0.8` | Async SQL with SQLite |
| `tokio 1` | Async runtime |
| `serde 1` / `serde_json 1` | Serialization/deserialization |
| `chrono 0.4` | Timestamp handling with serde support |
| `tracing` / `tracing-subscriber` | Structured logging with env filter |
| `anyhow` | Error handling |
| `regex` | Pattern matching in GPU CSV parser |

## See also

- [Config Module](src/config.md) — Environment variable configuration
- [REST API Reference](api/api-reference.md) — HTTP endpoints
- [Database Schema](db/schema.md) — SQLite schema
