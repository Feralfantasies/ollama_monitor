---
type: Source Module
title: Models Module
description: Shared data structures for Ollama, GPU, system, and API responses.
resource: src/models.rs
tags: [rust, serde, data-models]
timestamp: 2026-06-18T00:00:00Z
---

# Models Module

Shared data structures used across modules. All serialization types derive `serde::{Serialize, Deserialize}`.

## Ollama API types

### `OllamaTagsResponse`

```rust
pub struct OllamaTagsResponse {
    pub models: Vec<OllamaModelEntry>,
}
```

### `OllamaModelEntry`

| Field | Type | Description |
|---|---|---|
| `model_name` | `String` | Model name (from JSON `name`) |
| `size_bytes` | `Option<u64>` | Model size in bytes |
| `digest` | `Option<String>` | SHA256 digest |
| `modified_at` | `Option<String>` | ISO 8601 modification timestamp |

## Aggregated status

### `MonitorStatus`

Combined status object served by `/api/status`.

| Field | Type | Description |
|---|---|---|
| `ollama_url` | `String` | Base URL of Ollama server |
| `ollama_reachable` | `bool` | Whether Ollama responded |
| `loaded_model` | `Option<String>` | First model in the list (current loaded model) |
| `available_models` | `Vec<String>` | List of all model names |
| `gpu` | `GpuMetric` | GPU metrics snapshot |
| `system` | `SystemMetric` | System metrics snapshot |
| `timestamp` | `String` | Uptime-based timestamp |

### `GpuMetric`

| Field | Type |
|---|---|
| `name` | `Option<String>` |
| `temperature_c` | `Option<f64>` |
| `memory_used_mib` | `Option<u64>` |
| `memory_total_mib` | `Option<u64>` |
| `memory_remaining_mib` | `Option<u64>` |
| `utilization_pct` | `Option<f64>` |
| `power_watts` | `Option<f64>` |

All fields are `Option` — placeholder returns all `None`.

### `SystemMetric`

| Field | Type |
|---|---|
| `memory_used_mib` | `Option<u64>` |
| `memory_total_mib` | `Option<u64>` |
| `memory_remaining_mib` | `Option<u64>` |
| `memory_usage_pct` | `Option<f64>` |
| `cpu_utilization_pct` | `Option<f64>` |

## API response subsets

- `ApiModelResponse` — `loaded_model`, `available_models`, `total_count`
- `ApiGpuResponse` — `gpu` (`GpuMetric`), `timestamp`
- `ApiSystemResponse` — `system` (`SystemMetric`), `timestamp`
- `ApiHistoryResponse` — `Vec<GpuHistoryPoint>` for `/api/history`
- `ApiSystemHistoryResponse` — `Vec<SystemHistoryPoint>` for `/api/sys-history`

## Database record

### `CheckResult`

Flat structure with all fields from `MonitorStatus` flattened for single-row SQLite insert. Includes separate GPU and system metric columns.

## See also

- [Database Schema](../db/schema.md) — How `CheckResult` maps to the SQLite table
