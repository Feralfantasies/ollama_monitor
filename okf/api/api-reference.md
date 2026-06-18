---
type: API Reference
title: REST API Reference
description: HTTP endpoints, request parameters, and response schema.
tags: [rest, http, api, json]
timestamp: 2026-06-18T00:00:00Z
---

# REST API Reference

All endpoints return `application/json` except `GET /` which returns `text/html`. Server binds to `SERVER_BIND:SERVER_PORT` (default `0.0.0.0:3000`).

## Endpoints

### GET /

Returns the embedded dark-themed web dashboard.

- **Content-Type:** `text/html`
- **Response:** Full HTML page with inline CSS/JS

### GET /api/status

Combined status — all metrics for the latest refresh cycle.

**Response:** `MonitorStatus`

```json
{
  "ollama_url": "http://127.0.0.1:11434",
  "ollama_reachable": true,
  "loaded_model": "llama3:8b",
  "available_models": ["llama3:8b", "mistral:7b"],
  "gpu": {
    "name": "NVIDIA GeForce RTX 3080",
    "temperature_c": 67.5,
    "memory_used_mib": 6144,
    "memory_total_mib": 10240,
    "memory_remaining_mib": 4096,
    "utilization_pct": 82.0,
    "power_watts": 245.0
  },
  "system": {
    "memory_used_mib": 8192,
    "memory_total_mib": 16384,
    "memory_remaining_mib": 8192,
    "memory_usage_pct": 50.0,
    "cpu_utilization_pct": 35.0
  },
  "timestamp": "0 days 12:30"
}
```

All numeric fields are nullable — `null` when the collector failed.

**Error:** `503 Service Unavailable` — First refresh cycle not yet complete.

### GET /api/gpu

GPU metrics only from the latest refresh.

**Response:** `ApiGpuResponse`

```json
{
  "gpu": { ... },
  "timestamp": "0 days 12:30"
}
```

### GET /api/models

Model information only from the latest refresh.

**Response:** `ApiModelResponse`

```json
{
  "loaded_model": "llama3:8b",
  "available_models": ["llama3:8b", "mistral:7b"],
  "total_count": 2
}
```

### GET /api/history?range=15m

GPU time-series history.

**Query parameters:**

| Parameter | Values | Default |
|---|---|---|
| `range` | `15m`, `1h`, `6h`, `1d`, `7d`, `30d` | `15m` |

**Response:** `ApiHistoryResponse`

```json
{
  "points": [
    { "timestamp": 1700000000000, "memory_used_mib": 6144, "temperature_c": 67.5 }
  ]
}
```

### GET /api/system

System metrics only from the latest refresh.

**Response:** `ApiSystemResponse`

```json
{
  "system": {
    "memory_used_mib": 8192,
    "memory_total_mib": 16384,
    "memory_remaining_mib": 8192,
    "memory_usage_pct": 50.0,
    "cpu_utilization_pct": 35.0
  },
  "timestamp": "0 days 12:30"
}
```

### GET /api/sys-history?range=15m

System time-series history.

**Query parameters:**

| Parameter | Values | Default |
|---|---|---|
| `range` | `15m`, `1h`, `6h`, `1d`, `7d`, `30d` | `15m` |

**Response:** `ApiSystemHistoryResponse`

```json
{
  "points": [
    { "timestamp": 1700000000000, "memory_used_mib": 8192, "cpu_utilization_pct": 35.0 }
  ]
}
```

## Error codes

| Status | Condition |
|---|---|
| `500 Internal Server Error` | Database query failure |
| `503 Service Unavailable` | First refresh cycle has not completed yet |

## See also

- [Architecture](/architecture.md) — System architecture and data flow
- [Models Module](/src/models.md) — Full Rust type definitions
