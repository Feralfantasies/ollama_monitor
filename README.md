# Ollama Monitor

A Rust-based monitoring tool for Ollama that collects GPU and model information and exposes it via a REST API and web dashboard. Designed for later Home Assistant integration.

All configuration is via **environment variables** — no config files needed — making it easy to deploy with systemd or Docker.

## Architecture

```text
┌─────────────────┐     ┌──────────────────┐
│  Ollama (localhost)    │  :11434/api/*       │      │                  │
└─────────────────┘ ─────│ ollama_monitor   │
                          │  (Rust + Axum)   │
┌─────────────────┐     ─┤                  │
│  nvidia-smi CLI  │◄────│                  │
│  (same host GPU) │     └──┬───┬───┬──────┘
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
- [x] Configurable base URL with default `http://127.0.0.1:11434`
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
- [x] Environment variable configuration (no config files):
  - `OLLAMA_HOST`, `OLLAMA_PORT` — Ollama server location
  - `SERVER_BIND`, `SERVER_PORT` — API server listen address
  - `REFRESH_INTERVAL_SECS` — polling interval
  - `GPU_DEVICE_INDEX` — NVIDIA GPU device to query
- [x] `tracing` + `env_filter` for structured logging
- [x] Graceful shutdown on SIGINT/SIGTERM
- [x] Build verification

## Configuration

All settings are controlled via environment variables. None are required — sensible defaults are used when a variable is not set.

| Variable                | Default              | Description                               |
|-------------------------|----------------------|-------------------------------------------|
| `OLLAMA_HOST`           | `http://127.0.0.1`   | Ollama server base URL                    |
| `OLLAMA_PORT`           | `11434`              | Ollama server port                        |
| `SERVER_BIND`           | `0.0.0.0`            | Address to bind the API server to         |
| `SERVER_PORT`           | `3000`               | Port for the API server                   |
| `REFRESH_INTERVAL_SECS` | `15`                 | Seconds between status refreshes          |
| `GPU_DEVICE_INDEX`      | `0`                  | NVIDIA GPU device index to query          |

### Quick Start (defaults)

```bash
cargo build --release
./target/release/ollama_monitor
```

With custom settings:

```bash
OLLAMA_HOST=http://192.168.1.50 REFRESH_INTERVAL_SECS=30 ./target/release/ollama_monitor
```

## Building & Running

### Development

```bash
cargo build
# or for release:
cargo build --release
```

### Docker

```dockerfile
FROM rust:1-bookworm AS builder
WORKDIR /app
COPY . .
RUN cargo build --release

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y nvidia-utils && rm -rf /var/lib/apt/lists/*
COPY --from=builder /app/target/release/ollama_monitor /usr/local/bin/
EXPOSE 3000
ENTRYPOINT ["/usr/local/bin/ollama_monitor"]
```

Run:

```bash
docker run -d \
  --name ollama_monitor \
  --gpus all \
  --network host \
  -e OLLAMA_HOST=http://192.168.1.50 \
  -e REFRESH_INTERVAL_SECS=15 \
  -e RUST_LOG=info \
  ollama_monitor
```

> **Note:** When running in Docker without `--network host`, you'll need to map ports and point `OLLAMA_HOST` at the host machine's IP.

## Systemd Service Deployment

To run `ollama_monitor` as a persistent background service:

### 1. Install the binary

```bash
sudo cp ./target/release/ollama_monitor /usr/local/bin/
sudo chmod 755 /usr/local/bin/ollama_monitor
```

### 2. Create a systemd service unit with environment variables

Create `/etc/systemd/system/ollama_monitor.service`:

```ini
[Unit]
Description=Ollama Monitor
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
ExecStart=/usr/local/bin/ollama_monitor
Restart=always
RestartSec=5
Environment=RUST_LOG=info
Environment=OLLAMA_HOST=http://127.0.0.1
Environment=OLLAMA_PORT=11434
Environment=SERVER_BIND=0.0.0.0
Environment=SERVER_PORT=3000
Environment=REFRESH_INTERVAL_SECS=15
Environment=GPU_DEVICE_INDEX=0
StandardOutput=journal
StandardError=journal
User=root
Group=root

[Install]
WantedBy=multi-user.target
```

### 3. Enable and start the service

```bash
sudo systemctl daemon-reload
sudo systemctl enable ollama_monitor
sudo systemctl start ollama_monitor
```

### Useful management commands

```bash
# Check service status
sudo systemctl status ollama_monitor

# View logs
sudo journalctl -u ollama_monitor -f

# Restart after changes
sudo systemctl restart ollama_monitor

# Stop and disable
sudo systemctl stop ollama_monitor
sudo systemctl disable ollama_monitor
```

## API Endpoints

| Endpoint               | Description                        |
|------------------------|------------------------------------|
| `GET /`                | Web dashboard                      |
| `GET /api/status`      | Combined status (all metrics)      |
| `GET /api/gpu`         | GPU metrics only                   |
| `GET /api/models`      | Model information only             |

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
