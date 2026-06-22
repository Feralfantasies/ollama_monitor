---
type: Application
title: Overview
description: What ollama_monitor is and what it does.
resource: https://github.com/Feralfantasies/ollama_monitor
tags: [monitoring, ollama, gpu, rust, home-assistant]
timestamp: 2026-06-18T00:00:00Z
---

# Ollama Monitor

A Rust-based monitoring tool for Ollama AI servers. Collects GPU metrics (via `nvidia-smi`) and Ollama model information (via the Ollama REST API), exposes everything via REST endpoints, an embedded web dashboard, and a native Home Assistant custom integration so metrics appear as native HA sensors.

## What it does

- **Polls Ollama** — Queries `OLLAMA_HOST/api/tags` to discover available models and identify the currently loaded model.
- **Queries GPU** — Runs `nvidia-smi` to read GPU temperature, memory usage, utilization, and power draw.
- **Collects system metrics** — Reads `/proc/meminfo` and `/proc/stat` for host memory and CPU utilization.
- **Persists history** — Stores a row per refresh cycle in a local SQLite database. Auto-prunes rows older than 30 days.
- **Serves metrics** — REST API + dark-themed web dashboard with interactive charts for GPU memory, temperature, system memory, and CPU over time.
- **Home Assistant sensors** — Ships a native HA custom integration exposing 5 sensors (ollama status, GPU temperature, GPU memory, GPU utilization, GPU power) for dashboards, automations, and graphs.

## Key design principles

- **Environment-only configuration** — No config files. All settings via environment variables with sensible defaults.
- **Graceful degradation** — If Ollama is unreachable, GPU is absent, or the system has no `/proc`, the application continues operating with partial data.
- **Zero config for simple use cases** — Default config works when Ollama is on the same host. Change `OLLAMA_HOST` to monitor a remote instance.

## See also

- [Architecture](architecture.md) — System architecture and data flow
- [REST API Reference](api/api-reference.md) — HTTP endpoints
