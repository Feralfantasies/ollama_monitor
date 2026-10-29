---
type: Application
title: Overview
description: What ollama_monitor is and what it does.
resource: https://github.com/Feralfantasies/ollama_monitor
tags: [monitoring, ollama, gpu, rust, home-assistant]
timestamp: 2026-06-18T00:00:00Z
---

# Ollama Monitor

A Rust-based monitoring tool for local AI servers. Collects GPU metrics (via `nvidia-smi`) and model information (via the Ollama REST API **or** llama.cpp's `llama-server`), exposes everything via REST endpoints, an embedded web dashboard, and a native Home Assistant custom integration so metrics appear as native HA sensors.

It also **arbitrates a single GPU between mutually exclusive workloads**: on a host
where only one model fits in VRAM at a time, the dashboard can switch the resident
model safely — stopping the incumbent, confirming VRAM was actually released, then
loading the target. See [Switcher Module](src/switcher.md).

## What it does

- **Polls the model backend** — Discovers available models and the currently loaded one. Uses the [llama.cpp client](src/llamacpp.md) (`/v1/models` + `/props`) when `NODE_<i>_*` entries are configured, otherwise the Ollama client (`/api/tags`). llama-server does **not** implement Ollama's API — `/api/tags` and `/api/ps` both return 404 — so the two clients are not interchangeable.
- **Queries GPU** — Runs `nvidia-smi` to read GPU temperature, memory usage, utilization, and power draw.
- **Switches GPU workloads** — `POST /api/model` moves VRAM between declared nodes (llama.cpp models and ComfyUI), enforcing a never-start-before-release invariant. Driven by per-container [switch agents](deployment/switch-agent.md) behind bearer tokens and an allowlist.
- **Collects system metrics** — Reads `/proc/meminfo` and `/proc/stat` for host memory and CPU utilization.
- **Persists history** — Stores a row per refresh cycle in a local SQLite database. Auto-prunes rows older than 30 days.
- **Serves metrics** — REST API + dark-themed web dashboard with interactive charts for GPU memory, temperature, system memory, and CPU over time, plus a Model Switcher panel.
- **Home Assistant sensors** — Ships a native HA custom integration exposing 5 sensors (ollama status, GPU temperature, GPU memory, GPU utilization, GPU power) for dashboards, automations, and graphs.

## Two binaries

| Binary | Source | Role |
|---|---|---|
| `ollama_monitor` | `src/main.rs` | Monitor, dashboard and switch **orchestrator**. Runs once, centrally. |
| `switch_agent` | `src/bin/switch_agent.rs` | Minimal allowlisted `systemctl` controller. Runs **inside each** GPU workload container. |

Both build as static musl executables.

## Key design principles

- **Environment-only configuration** — No config files. All settings via environment variables with sensible defaults.
- **Graceful degradation** — If the model backend is unreachable, GPU is absent, or the system has no `/proc`, the application continues operating with partial data. One misconfigured node becomes display-only; it does not abort startup.
- **Fail closed on VRAM** — If VRAM cannot be read, or does not fall below the floor in time, a switch **fails** rather than starting a second model onto an occupied card.
- **Secrets never logged or served** — Agent and API tokens are wrapped in a `Secret` type whose `Debug` renders `[REDACTED]`, because `main.rs` logs the whole config. No token appears in the served dashboard HTML or JS.
- **Zero config for simple use cases** — Default config works when the backend is on the same host. Change `OLLAMA_HOST`, or declare `NODE_<i>_*`, to monitor something else.

## See also

- [Architecture](architecture.md) — System architecture and data flow
- [Switcher Module](src/switcher.md) — VRAM arbitration and the ordering invariant
- [llama.cpp Client](src/llamacpp.md) — why `/props` 200 is the only readiness signal
- [Switch Agent Deployment](deployment/switch-agent.md) — per-container agent
- [REST API Reference](api/api-reference.md) — HTTP endpoints
