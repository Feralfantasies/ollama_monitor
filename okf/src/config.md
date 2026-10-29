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
| `GPU_QUERY_BIN` | `nvidia-smi` | GPU query binary; overridable so tests can inject a mock |
| `MONITOR_API_TOKEN` | *(unset)* | If set, `POST /api/model` requires `Authorization: Bearer <token>`. If unset the mutating endpoint is open to anything that can reach it, and a warning is logged at startup |

## GPU workload nodes (the switcher)

Nodes are declared by scanning `NODE_<i>_*` for `i` = 0, 1, 2, … and **stopping at
the first index with no `NODE_<i>_NAME`**. That keeps the list open-ended, so a
future `llama03` is a config change rather than a code change. A gap ends the scan;
later indices are ignored.

| Variable | Default | Description |
|---|---|---|
| `NODE_<i>_NAME` | *(required)* | Stable node id, e.g. `llama01`. Absent ⇒ scan stops |
| `NODE_<i>_KIND` | `llama` | `llama` (readiness = `/props` 200) or `comfyui` (readiness = unit active). Unknown values warn and fall back to `llama` |
| `NODE_<i>_SERVER_URL` | *(none)* | llama-server root, e.g. `http://host:8000`. Omit for non-llama nodes |
| `NODE_<i>_AGENT_URL` | *(none)* | Switch-agent root, e.g. `http://host:8100`. Omit ⇒ display-only node |
| `NODE_<i>_AGENT_TOKEN` | *(none)* | Bearer token for that agent. **Secret** — redacted from `Debug`/logs |
| `NODE_<i>_UNIT` | *(none)* | systemd unit the agent controls, e.g. `llama-server.service`. Required to switch |
| `NODE_<i>_MODEL_ALIAS` | *(none)* | Model alias this node serves; `POST /api/model {"model": …}` resolves through it |
| `NODE_SCAN_MAX` | `16` | Upper bound on indices scanned |

## Switcher timing / safety

| Variable | Default | Description |
|---|---|---|
| `SWITCH_VRAM_FLOOR_MIB` | `2000` | Host VRAM must fall below this before the target starts. Starting above it is a guaranteed CUDA OOM on a single-GPU box |
| `SWITCH_RELEASE_TIMEOUT_SECS` | `180` | How long to wait for VRAM to fall below the floor before failing |
| `SWITCH_READY_TIMEOUT_SECS` | `600` | How long to wait for the target to become ready. Model loads measured 36–161 s, so this is deliberately generous |
| `SWITCH_POLL_INTERVAL_SECS` | `5` | Poll interval for both waits |

## Secrets

`MONITOR_API_TOKEN` and `NODE_<i>_AGENT_TOKEN` are wrapped in a `Secret` type whose
hand-written `Debug` renders `[REDACTED]`. This matters because `main.rs` logs the
whole struct via `tracing::info!(?config, "Configuration loaded")` — a plain
`String` field would land in the journal in cleartext. Neither token is ever sent to
the browser: agent tokens stay server-side, and the dashboard prompts for the API
token at runtime into `localStorage`, so no secret appears in the served HTML or JS.

`Secret::matches()` compares length-independently without short-circuiting on a
mismatched byte. Values are trimmed, so `"  tok  "` is stored as `"tok"` and a stray
space in an env file cannot break auth.

## Behavior notes

- `OLLAMA_HOST` can include a scheme (`http://`), a port, or just the hostname. The `ollama_base_url()` helper normalizes all forms and appends `OLLAMA_PORT` if no port is present.
- Invalid numeric values log a warning and fall back to defaults — no hard failures on bad env vars.
- A node missing its agent URL, token or unit is **kept but marked
  `switchable: false`**, with a warning logged. One misconfigured node degrades to
  display-only rather than blinding the whole dashboard or aborting startup.
- When any `NODE_<i>_*` llama node is configured, model discovery uses the
  [llama.cpp client](llamacpp.md); otherwise it falls back to the Ollama client.
  Discovery is automatic, so `OLLAMA_HOST`/`OLLAMA_PORT` are unused in a
  llama.cpp deployment.
- Config is loaded through an injectable lookup (`Config::load_from(&get)`) so
  tests never touch process-global env — that would make them order-dependent under
  Rust's parallel test runner.

## See also

- [Architecture](../architecture.md) — Where config fits in the data flow
- [llama.cpp Client](llamacpp.md) — what `SERVER_URL` endpoints must serve
- [Switcher Module](switcher.md) — what the timing/safety thresholds control
- [Switch Agent Deployment](../deployment/switch-agent.md) — the agent side of `AGENT_URL`/`AGENT_TOKEN`
