---
type: Testing Document
title: Testing Strategy
description: Test matrix, mock infrastructure, and CI verification steps.
tags: [testing, rust, mock, ci]
timestamp: 2026-06-18T00:00:00Z
---

# Testing Strategy

All tests run without a real GPU or running Ollama instance. Two test categories cover the codebase.

## Unit tests

Inline `#[cfg(test)]` modules inside source files.

### Database tests (`src/db.rs`)

| Test | Verifies |
|---|---|
| `test_insert_and_query_check_result` | Insert + query round-trip preserves all fields |
| `test_multiple_check_results_ordered_desc` | Multiple rows return newest-first |
| `test_insert_check_result_with_null_fields` | All-null GPU row round-trip |

Uses per-test SQLite temp pools (single connection each) for full isolation.

### System module tests (`src/system.rs`)

| Test | Verifies |
|---|---|
| `test_proc_stat_parses` | Live `/proc/stat` reads and parses |
| `test_meminfo_parses` | Live `/proc/meminfo` reads and parses |
| `test_query_system_returns_values` | `query_system()` populates at least memory_total and cpu_utilization |
| `test_system_metric_placeholder` | Placeholder returns all `None` |

These require a Linux environment with `/proc` mounted.

## Integration tests (`src/tests.rs`)

Full refresh-loop tests using mock infrastructure.

| Test | Verifies |
|---|---|
| `test_full_pipeline_history_accumulates` | 3 cycles → 3 DB rows, history returns correct points + in-memory state matches |
| `test_history_timestamps_ordered` | 4 cycles with 1s gaps → strictly ascending timestamps |
| `test_unreachable_ollama_records_correctly` | No Ollama → `ollama_reachable: false`, GPU still populated |
| `test_no_gpu_records_nulls` | Placeholder GPU → all GPU fields null, Ollama still recorded |
| `test_mixed_gpu_availability` | Alternating GPU good/placeholder → DB tolerates mixed |
| `test_dashboard_api_endpoints` | Full HTTP API on all endpoints (/api/status, /api/gpu, /api/models, /api/history, /) |

## Mock infrastructure

- **Mock Ollama** — axum server on random port returning deterministic JSON on `/api/tags`
- **Mock nvidia-smi** — Executable shell script in temp dir printing deterministic CSV
- **Temp databases** — Unique filenames (PID + nanosecond) for full test isolation

## CI verification (local)

```bash
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
RUSTFLAGS="-D warnings" cargo build --release
RUSTFLAGS="-D warnings" cargo test --all-targets
```

## See also

- [Tests Module](../src/tests.md) — Test harness implementation details
- [CI/CD Pipeline](ci-cd.md) — GitHub Actions workflow definitions
