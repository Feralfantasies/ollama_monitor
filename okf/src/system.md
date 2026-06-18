---
type: Source Module
title: System Metrics Module
description: Linux /proc-based memory and CPU utilization collection.
resource: src/system.rs
tags: [rust, system, proc, linux]
timestamp: 2026-06-18T00:00:00Z
---

# System Metrics Module

Reads from Linux `/proc` filesystem to collect host-level memory and CPU utilization.

## Memory collection

Reads `MemTotal` and `MemAvailable` from `/proc/meminfo` (values in kilobytes). Converts to MiB for output.

| Computed field | Formula |
|---|---|
| `memory_used_mib` | `(MemTotal - MemAvailable) / 1024` |
| `memory_total_mib` | `MemTotal / 1024` |
| `memory_remaining_mib` | `MemAvailable / 1024` |
| `memory_usage_pct` | `100 - (MemAvailable / MemTotal) * 100` |

## CPU utilization

Reads the `cpu` line from `/proc/stat` twice, 100ms apart, to compute instantaneous utilization.

| Computed field | Formula |
|---|---|
| `cpu_utilization_pct` | `(1 - idle_diff / total_diff) * 100` |

Where `total_diff` is the sum of all tick deltas and `idle_diff` is the idle tick delta between the two samples.

## Public API

| Function | Returns | Description |
|---|---|---|
| `query_system()` | `SystemMetric` | Best-effort system metric collection. Returns partial data if one collector fails. |

Memory and CPU are collected independently — a `/proc/meminfo` failure does not block CPU collection and vice versa.

## Limitations

- Linux-only (reads `/proc/stat` and `/proc/meminfo`). Will fail on non-Linux hosts.
- CPU sample takes 100ms (thread sleep between samples).

## See also

- [Architecture](/architecture.md) — Where system metrics fit in the data flow
