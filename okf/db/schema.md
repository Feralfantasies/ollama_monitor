---
type: Database Schema
title: Database Schema
description: SQLite check_results table schema, migrations, and query patterns.
resource: src/db.rs
tags: [sqlite, schema, sqlx, persistence]
timestamp: 2026-06-18T00:00:00Z
---

# Database Schema

SQLite database with WAL journal mode and full auto-vacuum. Stores one row per refresh cycle in the `check_results` table.

## Table: `check_results`

| Column | Type | Nullable | Description |
|---|---|---|---|
| `id` | `INTEGER PRIMARY KEY AUTOINCREMENT` | NO | Auto-increment row ID |
| `recorded_at` | `TEXT NOT NULL` | NO | ISO 8601 timestamp (UTC) |
| `ollama_url` | `TEXT NOT NULL` | NO | Base URL of Ollama server |
| `ollama_reachable` | `INTEGER NOT NULL` | NO | 1 = reachable, 0 = unreachable |
| `loaded_model` | `TEXT` | YES | Name of first (loaded) model |
| `available_models_count` | `INTEGER NOT NULL DEFAULT 0` | NO | Count of available models |
| `gpu_name` | `TEXT` | YES | GPU device name |
| `gpu_temperature_c` | `REAL` | YES | GPU temperature in °C |
| `gpu_memory_used_mib` | `INTEGER` | YES | GPU memory used in MiB |
| `gpu_memory_total_mib` | `INTEGER` | YES | GPU memory total in MiB |
| `gpu_utilization_pct` | `REAL` | YES | GPU utilization percentage |
| `gpu_power_watts` | `REAL` | YES | GPU power draw in watts |
| `sys_memory_used_mib` | `INTEGER` | YES | System memory used in MiB |
| `sys_memory_total_mib` | `INTEGER` | YES | System memory total in MiB |
| `sys_memory_remaining_mib` | `INTEGER` | YES | System memory remaining in MiB |
| `sys_memory_usage_pct` | `REAL` | YES | System memory usage percentage |
| `sys_cpu_utilization_pct` | `REAL` | YES | System CPU utilization percentage |

## Migrations

- **Schema migration** — `CREATE TABLE IF NOT EXISTS check_results` (idempotent).
- **Column additions** — Six `ALTER TABLE ADD COLUMN` migrations for newer fields (`gpu_power_watts`, `sys_memory_*`, `sys_cpu_utilization_pct`). Errors silently on fresh databases where columns already exist.
- **Auto-prune** — `DELETE FROM check_results WHERE recorded_at < datetime('now', '-30 days')` runs after every successful migration. Silently skipped on first run.

## Query patterns

### Insert

```sql
INSERT INTO check_results (recorded_at, ollama_url, ollama_reachable, ...)
VALUES (?, ?, ?, ...)
```

### GPU history (time-series)

```sql
SELECT strftime('%s', recorded_at) * 1000 as ts,
       gpu_memory_used_mib, gpu_temperature_c
FROM check_results
WHERE recorded_at >= datetime('now', '{interval}')
ORDER BY recorded_at ASC
```

### System history (time-series)

```sql
SELECT strftime('%s', recorded_at) * 1000 as ts,
       sys_memory_used_mib, sys_cpu_utilization_pct
FROM check_results
WHERE recorded_at >= datetime('now', '{interval}')
ORDER BY recorded_at ASC
```

### All check results (newest first)

```sql
SELECT ollama_url, ollama_reachable, loaded_model, ...
FROM check_results
ORDER BY id DESC
```

## Time ranges

| Query parameter | SQLite interval | Description |
|---|---|---|
| `15m` | `-15 minutes` | Last 15 minutes |
| `1h` | `-1 hour` | Last hour |
| `6h` | `-6 hours` | Last 6 hours |
| `1d` | `-24 hours` | Last day |
| `7d` | `-7 days` | Last week |
| `30d` | `-30 days` | Last month |

## Database configuration

- **Journal mode:** WAL (Write-Ahead Logging)
- **Auto-vacuum:** Full
- **Busy timeout:** 5 seconds
- **Connection pool:** sqlx managed with configurable max connections

## See also

- [Models Module](/src/models.md) — `CheckResult` structure that maps to this table
- [API Module](/src/api.md) — Refresh loop that inserts rows
