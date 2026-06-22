---
type: Source Module
title: Tests Module
description: Integration test harness with mock Ollama server and mock nvidia-smi.
resource: src/tests.rs
tags: [rust, testing, mock, integration]
timestamp: 2026-06-18T00:00:00Z
---

# Tests Module

Integration test harness that runs the real refresh loop against mock dependencies. No real GPU or Ollama instance required.

## Mock infrastructure

### Mock Ollama server

A lightweight axum HTTP server started on a random localhost port. Returns deterministic JSON on `/api/tags`:

```json
{
  "models": [
    { "name": "llama3:8b", "size": 4700000000, "digest": "sha256:abc123", "modified_at": "2025-01-01T00:00:00Z" },
    { "name": "mistral:7b", "size": 4100000000, "digest": "sha256:def456", "modified_at": "2025-02-01T00:00:00Z" }
  ]
}
```

### Mock nvidia-smi binary

A shell script in a temp directory that prints deterministic CSV output:

```
0, NVIDIA GeForce RTX 3080, 67.5, 6144, 10240, 82.0, 245.0
```

Script is made executable (`chmod 0755`) and passed to `query_gpu_bin()` via `mock_gpu_with_bin()`.

## Test matrix

| Test | What it verifies |
|---|---|
| `test_full_pipeline_history_accumulates` | 3 refresh cycles → 3 DB rows, history query returns 3 correct points |
| `test_history_timestamps_ordered` | 4 cycles with 1s gaps → timestamps strictly ascending |
| `test_unreachable_ollama_records_correctly` | No mock Ollama → `ollama_reachable: false`, GPU still populated |
| `test_no_gpu_records_nulls` | Placeholder GPU → all GPU fields null, Ollama still recorded |
| `test_mixed_gpu_availability` | Alternating good/placeholder GPU → DB tolerates mixed nulls |
| `test_dashboard_api_endpoints` | Full HTTP API end-to-end: all endpoints return correct data |

## Design patterns

- **Generic callback** — `run_one_refresh` accepts `Fn(usize) -> GpuMetric` allowing test injection without cargo features or conditional compilation.
- **Temp isolation** — Each test uses unique temp file names (PID + nanosecond timestamp) to avoid conflicts with the real `ollama_monitor.db`.
- **Cleanup** — Mock script directories are removed after each test via `std::fs::remove_dir_all`.

## See also

- [Testing Strategy](../testing/testing.md) — Full testing strategy including unit tests
