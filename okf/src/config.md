---
type: Source Module
title: Config Module
description: Environment variable configuration with sensible defaults.
resource: src/config.rs
tags: [rust, configuration, environment]
timestamp: 2026-06-18T00:00:00Z
---

# Config Module

Loads application configuration from environment variables with sensible defaults. No config files are used.

## Configuration parameters

| Variable | Default | Description |
|---|---|---|
| `OLLAMA_HOST` | `http://127.0.0.1` | Ollama server base URL |
| `OLLAMA_PORT` | `11434` | Ollama server port |
| `SERVER_BIND` | `0.0.0.0` | Address to bind the API server to |
| `SERVER_PORT` | `3000` | Port for the API server |
| `REFRESH_INTERVAL_SECS` | `15` | Seconds between status refreshes |
| `GPU_DEVICE_INDEX` | `0` | NVIDIA GPU device index to query |

## Behavior notes

- `OLLAMA_HOST` can include a scheme (`http://`), a port, or just the hostname. The `ollama_base_url()` helper normalizes all forms and appends `OLLAMA_PORT` if no port is present.
- Invalid numeric values log a warning and fall back to defaults — no hard failures on bad env vars.

## See also

- [Architecture](../architecture.md) — Where config fits in the data flow
