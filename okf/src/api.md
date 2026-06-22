---
type: Source Module
title: API Module
description: Axum REST API server, embedded web dashboard, and background refresh loop.
resource: src/api.rs
tags: [rust, axum, rest, dashboard]
timestamp: 2026-06-18T00:00:00Z
---

# API Module

Axum HTTP server with embedded dark-themed web dashboard and background refresh orchestration.

## Refresh loop

- `run_one_refresh()` — Runs one iteration: fetches Ollama models, queries GPU, collects system metrics, persists `CheckResult` to SQLite, updates shared `AppState`.
- `run_refresh_loop()` — Loops infinitely sleeping `REFRESH_INTERVAL_SECS` between iterations.
- The refresh function is generic over `Fn(usize) -> GpuMetric` — allows test injection of mock GPU data.

## HTTP endpoints

| Method | Path | Returns |
|---|---|---|
| `GET` | `/` | Embedded HTML dashboard |
| `GET` | `/api/status` | Full `MonitorStatus` (Ollama + GPU + system) |
| `GET` | `/api/gpu` | `ApiGpuResponse` (GPU only) |
| `GET` | `/api/models` | `ApiModelResponse` (models only) |
| `GET` | `/api/history?range=...` | `ApiHistoryResponse` (GPU time-series) |
| `GET` | `/api/system` | `ApiSystemResponse` (system only) |
| `GET` | `/api/sys-history?range=...` | `ApiSystemHistoryResponse` (system time-series) |

All endpoint handlers read from shared `Arc<RwLock<Option<MonitorStatus>>>`. Return `503 Service Unavailable` before first refresh completes.

## Dashboard

Single-file dark-themed HTML/JS dashboard with:

- Live metric cards (connection, models, GPU memory, GPU health, system memory, CPU)
- Interactive canvas charts for GPU memory/temperature history and system memory/CPU history
- Time range selector buttons (15m, 1h, 6h, 1d, 7d, 30d)
- Auto-refresh every 30 seconds

## See also

- [REST API Reference](../api/api-reference.md) — Detailed request/response schema
- [Models Module](models.md) — `MonitorStatus` and other data structures
