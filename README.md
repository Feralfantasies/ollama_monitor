# Ollama Monitor

A Rust-based monitoring tool for Ollama LXC containers that collects GPU and model information and exposes it via a REST API and web dashboard. Designed for later Home Assistant integration.

## Architecture

```
┌─────────────────┐     ┌──────────────────┐
│  Ollama (192..50)│◄────│                  │
│  :11434/api/*    │      │  ollama_monitor  │
└─────────────────┘      │  (Rust + Axum)   │
                          │                  │
┌─────────────────┐     ─┤                  │
│  nvidia-smi CLI  │◄────│                  │
│  (same host GPU) │     └──┬───┬───┬───────┘
└─────────────────┘        │   │   │
                          ▼   ▼   ▼
                     ┌──────────────────┐
                     │  REST API + Web   │
                     │  :3000/api/*      │
                     │  :3000/ (portal)  │
                     └──────────────────┘
```

## Build Plan

### Stage 1: Project Scaffolding & Data Models [✅ COMPLETE]
- [x] Set up dependencies in `Cargo.toml`
- [x] Define data models: `GpuMetric`, `ModelInfo`, `OllamaState`, `MonitorStatus`
- [x] Create module structure (`ollama.rs`, `gpu.rs`, `api.rs`, `config.rs`)

### Stage 2: Ollama Client (Module: `ollama`) [✅ COMPLETE]
- [x] Query `/api/tags` for available models
- [x] Determine currently loaded model (via `/api/tags` load tracking + fallback inference)
- [x] Configurable base URL with default `http://192.168.1.50:11434`
- [x] Retry logic on connection failures

### Stage 3: nvidia-smi Parser (Module: `gpu`) [✅ COMPLETE]
- [x] Use `nvidia-smi --query-gpu=... --format=csv,noheader` for structured parsing
- [x] Extract: memory-used MB, memory-total MB, temperature °C, GPU utilisation %
- [x] Compute remaining VRAM from total - used
- [x] Graceful degradation when nvidia-smi is unavailable (returns placeholder/None)

### Stage 4: REST API Server (Module: `api`) [✅ COMPLETE]
- [x] Axum server with endpoints:
  - `GET /api/status` — full combined status (all metrics in one response)
  - `GET /api/gpu` — GPU metrics only
  - `GET /api/models` — model list + currently loaded
  - `GET /` — web portal frontend
- [x] JSON responses structured for Home Assistant REST sensor integration
- [x] Configurable bind address (default `0.0.0.0:3000`)

### Stage 5: Web Dashboard (Module: `api` inline HTML) [✅ COMPLETE]
- [x] Single-page HTML with embedded JS/SSR via Axum `Html` response
- [x] Auto-refresh polling from `/api/status`
- [x] Clean display of models, GPU memory, temperature, utilization

### Stage 6: Configuration & Hardening [✅ COMPLETE]
- [x] TOML config file (`config.toml`) for:
  - Ollama host and port
  - Refresh interval
  - Server bind address and port
  - GPU device index
- [x] `tracing` + `env_logger` for structured logging
- [x] Graceful shutdown on SIGINT/SIGTERM
- [x] Build verification

## Configuration

Copy `config.example.toml` to `config.toml` and adjust values:

```toml
[ollama]
host = "http://192.168.1.50"
port = 11434

[server]
bind = "0.0.0.0"
port = 3000
refresh_interval_secs = 15

[gpu]
device_index = 0
```

## Building & Running

```bash
cargo build --release
./target/release/ollama_monitor
```

## API Endpoints

| Endpoint     | Description                           |
|-------------|---------------------------------------|
| `GET /`     | Web dashboard                         |
| `GET /api/status` | Combined status (all metrics)   |
| `GET /api/gpu`    | GPU metrics only                |
| `GET /api/models` | Model information only          |

## Home Assistant Integration (planned)

The `/api/status` endpoint returns flat JSON suitable for HA `rest` sensors:

```yaml
# Future example
sensor:
  - platform: rest
    resource: http://<monitor-ip>:3000/api/status
    value_template: "{{ value_json.gpu.temperature }}"
    name: "Ollama GPU Temperature"
    unit_of_measurement: "°C"
```
