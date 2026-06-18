---
type: Source Module
title: GPU Module
description: nvidia-smi CLI wrapper and CSV parser for GPU metrics.
resource: src/gpu.rs
tags: [rust, gpu, nvidia-smi]
timestamp: 2026-06-18T00:00:00Z
---

# GPU Module

Wraps the `nvidia-smi` CLI tool to collect GPU metrics. Uses CSV-mode output for reliable parsing.

## Public API

| Function | Returns | Description |
|---|---|---|
| `query_gpu(index)` | `Result<GpuMetric>` | Query GPU at given index via system `nvidia-smi`. Returns error on failure. |
| `query_gpu_bin(path, index)` | `Result<GpuMetric>` | Query GPU using binary at `path`. Used in tests with mock scripts. |
| `try_query_gpu(index)` | `GpuMetric` | Best-effort query. Returns `GpuMetric::placeholder()` (all nulls) on failure. |

## Command executed

```bash
nvidia-smi --query-gpu=index,name,temperature.gpu,memory.used,memory.total,utilization.gpu,power.draw \
  --format=csv,noheader,nounits --id 0
```

## CSV parsing

- `parse_gpu_csv_line()` — Splits on `", "` delimiter. GPU name may contain commas, so parsing works from the fixed-position numeric fields at the start (index) and end (power, util, mem_total, mem_used, temp).
- Validates that the returned GPU index matches the expected index.
- Computes `memory_remaining_mib` as `memory_total - memory_used`.

## Error handling

- Execution failure (binary not found, permissions) → error propagated from `query_gpu()`, placeholder from `try_query_gpu()`.
- Non-zero exit code → error with stderr text included.
- Unexpected field count → error.

## See also

- [Architecture](../architecture.md) — Where GPU module fits in the data flow
