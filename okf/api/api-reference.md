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

### GET /api/nodes

Live per-node GPU workload state, VRAM headroom, and current switch progress. This
is the endpoint the dashboard's Model Switcher renders.

Attribution comes from each container's [switch agent](../deployment/switch-agent.md),
**not** from `nvidia-smi` process names — those render as `[Not Found]` across
container PID namespaces. VRAM totals come from host NVML.

**Response:** `ApiNodesResponse`

```json
{
  "nodes": [
    {
      "name": "llama01",
      "kind": "llama",
      "model_alias": "Qwen3.8-27B",
      "n_ctx": 61440,
      "total_slots": 3,
      "ready": true,
      "unit_active": true,
      "switchable": true,
      "observable": true,
      "agent_reachable": true,
      "error": null
    },
    {
      "name": "comfyui",
      "kind": "comfyui",
      "model_alias": null,
      "n_ctx": null,
      "total_slots": null,
      "ready": false,
      "unit_active": true,
      "switchable": true,
      "observable": true,
      "agent_reachable": true,
      "error": null
    }
  ],
  "switchable_nodes": ["llama01", "llama02", "comfyui"],
  "vram_used_mib": 27509,
  "vram_total_mib": 32768,
  "vram_free_mib": 5259,
  "resident_model": "Qwen3.8-27B",
  "resident_node": "llama01",
  "switch": { "running": false, "phase": "idle", "outcome": null, "error": null }
}
```

Field notes:

* `ready` — workload is **loaded and serving**. For `kind: llama` this means
  `/props` returned `200`; a llama-server is `unit_active: true` for the entire
  36–161 s load while `ready` is still `false`. That distinction is what prevents
  a CUDA OOM, so do not substitute one for the other.
* `n_ctx` — **per-slot** context, not the `--ctx-size` total.
* `switchable` — the node has an agent URL, token and unit configured. Nodes that
  do not are listed but cannot be selected.
* `agent_reachable: false` means "we could not ask", which is never treated as
  "it holds no VRAM".

### GET /api/switch

Progress of the in-flight or most recent switch. Same shape as the `switch` field
above.

**Response:** `SwitchProgress`

```json
{
  "running": false,
  "target": "llama02",
  "phase": "finished",
  "vram_used_mib": 24445,
  "vram_floor_mib": 2000,
  "stopped_nodes": ["llama01", "comfyui"],
  "elapsed_secs": 161,
  "outcome": {
    "target": "llama02",
    "target_model": "Precog-24B",
    "success": true,
    "no_op": false,
    "stopped_nodes": ["llama01", "comfyui"],
    "vram_before_mib": 27509,
    "vram_min_mib": 0,
    "vram_after_mib": 24445,
    "vram_floor_mib": 2000,
    "release_secs": 16,
    "ready_secs": 152,
    "total_secs": 161,
    "error": null
  },
  "error": null
}
```

`phase` is one of `idle`, `queued`, `init`, `stopping-others`, `waiting-for-vram`,
`starting-target`, `waiting-for-ready`, `finished`, `failed`.

### POST /api/model

Request that a given workload become the VRAM-resident one.

Runs in the **background** and returns `202` immediately: a switch measured
57–161 s, far too long to hold a request open behind a reverse proxy. Poll
`GET /api/switch` for progress.

**Auth:** when `MONITOR_API_TOKEN` is set, `Authorization: Bearer <token>` is
required. This endpoint can stop and start production GPU workloads, so it is
guarded even behind a LAN-only ingress. The dashboard never embeds the token — it
prompts at runtime and stores it in `localStorage`.

**Request body** — either selector form:

```json
{ "target": "llama02" }
```
```json
{ "model": "Precog-24B" }
```

**Response `202`:**

```json
{
  "accepted": true,
  "target": "llama02",
  "detail": "switch started in background; poll GET /api/switch",
  "vram_floor_mib": 2000
}
```

**Ordering guarantee.** The target is never started until host VRAM has been
*observed* below `SWITCH_VRAM_FLOOR_MIB`. Every other active holder — including
`comfyui` — is stopped first, and the switch **fails** rather than proceeding if
VRAM does not fall, or if VRAM cannot be read at all. See
[Switcher Module](../src/switcher.md).

Switching to an already-resident target is a no-op (`"no_op": true,
"total_secs": 0`), which avoids a pointless 36–161 s reload.

## Error codes

| Status | Condition |
|---|---|
| `400 Bad Request` | `POST /api/model` with neither `target` nor `model` |
| `401 Unauthorized` | `POST /api/model` without a bearer token when `MONITOR_API_TOKEN` is set |
| `404 Not Found` | Unknown `target` node name, or `model` alias no node serves |
| `409 Conflict` | Target node is display-only (not switchable), or a switch is already running |
| `500 Internal Server Error` | Database query failure |
| `503 Service Unavailable` | First refresh cycle has not completed yet |

A refused switch is reported as `200` on `GET /api/switch` with `success: false`
and a human-readable `error` (e.g. `"VRAM did not fall below 2000 MiB within 180s
(still 24753 MiB); refusing to start target (would OOM)"`), not as an HTTP error —
the request itself succeeded, the operation did not.

Concurrency is **refused, not queued**: two overlapping switches could interleave
their stop/start actions and load two models onto one card.

## See also

- [Architecture](../architecture.md) — System architecture and data flow
- [Models Module](../src/models.md) — Full Rust type definitions
- [Switcher Module](../src/switcher.md) — VRAM arbitration semantics
- [llama.cpp Client](../src/llamacpp.md) — why `/props` 200 is the only readiness signal
- [Switch Agent Deployment](../deployment/switch-agent.md) — per-container agent API
