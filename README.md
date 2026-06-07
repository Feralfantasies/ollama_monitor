# Ollama Monitor

A Rust-based monitoring tool for Ollama that collects GPU and model information and exposes it via a REST API, web dashboard, and a native [Home Assistant custom integration](#home-assistant-integration). All configuration via environment variables — no config files needed — making it easy to deploy with systemd or Docker.

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

### CI Verification

The project uses GitHub Actions to enforce code quality on every push and PR. To reproduce the same checks locally:

```bash
# format check (fails on any style deviation)
cargo fmt --all -- --check

# linting — deny all warnings
cargo clippy --all-targets -- -D warnings

# build — fail on any compiler warning
RUSTFLAGS="-D warnings" cargo build --release

# unit tests
cargo test --all-targets
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

## Testing

Run:

```bash
cargo test
```

All tests run **without a real GPU or running Ollama instance** — they use:

- **Mock Ollama HTTP server** — a lightweight [axum](https://crates.io/crates/axum) server started on a random port that returns deterministic model data on `/api/tags`.
- **Mock `nvidia-smi` binary** — a shell script that prints the same CSV the real CLI produces, passed to `query_gpu_bin()` instead of the system binary.

### Test matrix

| Test | What it verifies |
|------|------------------|
| `test_full_pipeline_history_accumulates` | 3 refresh cycles → 3 DB rows, history query returns correct points |
| `test_history_timestamps_ordered` | 4 cycles with gaps → timestamps strictly ascending |
| `test_unreachable_ollama_records_correctly` | Ollama down → `ollama_reachable: false`, GPU still populated |
| `test_no_gpu_records_nulls` | GPU placeholder → all GPU fields null, Ollama still recorded |
| `test_mixed_gpu_availability` | Alternating GPU present/absent → DB tolerates mixed nulls |
| `test_dashboard_api_endpoints` | Full HTTP API (`/api/status`, `/api/gpu`, `/api/models`, `/api/history`, `/`) |
| `test_insert_and_query_check_result` | Unit test — DB insert then query round-trip |
| `test_multiple_check_results_ordered_desc` | Unit test — multiple rows returned newest-first |
| `test_insert_check_result_with_null_fields` | Unit test — all-null GPU row round-trip |

## API Endpoints

| Endpoint               | Description                        |
|------------------------|------------------------------------|
| `GET /`                | Web dashboard                      |
| `GET /api/status`      | Combined status (all metrics)      |
| `GET /api/gpu`         | GPU metrics only                   |
| `GET /api/models`      | Model information only             |

## Home Assistant Integration

The ollama_monitor ships with a **native Home Assistant custom integration** — no YAML, no MQTT, no terminal access needed. Once installed, your GPU and Ollama metrics appear as real HA sensors that you can use in dashboards, automations, and graphs.

### Prerequisites

1. **ollama_monitor is running** and serving its API (verify with `curl http://<monitor-ip>:3000/api/status`)
2. Home Assistant can reach the monitor URL over the network
3. If ollama_monitor runs in Docker with `--network host`, use the host IP. Otherwise use the container IP or published port

### Installation

#### Option 1: HACS (Recommended)

HACS (Home Assistant Community Store) makes installation and updates straightforward.

1. Make sure **HACS** is installed in your Home Assistant instance (see [hacs.xyz](https://hacs.xyz))
2. Open **HACS** → **Integrations** → click **⋮** (top right) → **Custom repositories**
3. Enter this repo URL:
   ```
   https://github.com/Feralfantasies/ollama_monitor
   ```
4. Set **Category** to **Integration** and click **Add** (you'll be prompted to confirm — click the red warning popup)
5. Go back to the Integrations list, search for **Ollama Monitor**, and click **Download**
6. **Restart Home Assistant** (HACS will prompt you, or go to **Settings → System → Restart**)

#### Option 2: Manual Installation

Copy the integration folder into your Home Assistant `custom_components` directory:

```bash
# From your ollama_monitor checkout, copy to your HA config folder
# HA config is typically at:
#   /config          (HA supervised/docker)
#   ~/.homeassistant  (HA core)
#   /usr/share/hassio/homeassistant  (HA OS)

mkdir -p /config/custom_components/ollama_monitor
cp -r /path/to/ollama_monitor/ha_integration/custom_components/ollama_monitor/* \
    /config/custom_components/ollama_monitor/
```

Then **restart Home Assistant**.

> **After updating** the integration (manual install), delete the old folder first, copy the new version, and restart to clear any cached Python bytecode.

### Setup (GUI only — no YAML)

1. Go to **Settings** → **Devices & Services** → click **⚡ Add integration** (bottom left)
2. Search for **Ollama Monitor** and select it
3. You'll see a form asking for the **Monitor URL**:
   - Enter the base URL of your running ollama_monitor, e.g. `http://192.168.1.10:3000`
   - The integration will **test the connection** immediately by hitting `/api/status`
4. If the connection succeeds, click **Submit** — the sensors will appear automatically
5. If it fails, double-check the URL and make sure ollama_monitor is running

### Sensors Created

All sensors appear under a single **Ollama Monitor** device entry:

| Sensor | Unit | State Values | Attributes |
|---|---|---|---|
| **Ollama Status** | — | `online` / `offline` | `loaded_model`, `model_count`, `ollama_url` |
| **GPU Temperature** | °C | numeric | `gpu_name` |
| **GPU Memory Used** | MiB | numeric | `gpu_name`, `memory_total_mib`, `memory_remaining_mib`, `memory_usage_pct` |
| **GPU Utilization** | % | numeric | `gpu_name` |
| **GPU Power** | W | numeric | `gpu_name` |

All numeric sensors have `state_class: measurement` so Home Assistant automatically generates statistics and history graphs.

### Dashboard Setup

#### Quick: Entities Card

The fastest way to see all values at once:

1. Open **Settings** → **Dashboards** → click **+ Add card** → **Entities**
2. Search for the ollama_monitor sensors (or use the entity IDs below) and add them
3. Click **Save**

#### Lovelace YAML: Overview Card

For a polished dashboard, add this YAML to your dashboard (via **Edit dashboard → ⋮ → Edit in YAML**):

```yaml
- type: entities
  title: Ollama Monitor
  show_header_toggle: false
  entities:
    - type: custom:multiple-entity-row
      entity: sensor.ollama_monitor_ollama_status
      state_color: true
    - sensor.ollama_monitor_gpu_temperature
    - sensor.ollama_monitor_gpu_memory_used
    - sensor.ollama_monitor_gpu_utilization
    - sensor.ollama_monitor_gpu_power
```

#### Lovelace YAML: Gauges

For visual GPU metrics:

```yaml
- type: horizontal-stack
  cards:
    - type: gauge
      entity: sensor.ollama_monitor_gpu_temperature
      min: 0
      max: 100
      unit: °C
      severity:
        green: 0
        yellow: 70
        red: 85
    - type: gauge
      entity: sensor.ollama_monitor_gpu_utilization
      unit: '%'
      severity:
        green: 0
        yellow: 50
        red: 90
    - type: gauge
      entity: sensor.ollama_monitor_gpu_power
      min: 0
      max: 450
      unit: W
      severity:
        green: 0
        yellow: 250
        red: 350
```

### Automation Examples

#### Alert when GPU temperature is too high

```yaml
alias: GPU overheat warning
trigger:
  - platform: numeric_state
    entity_id: sensor.ollama_monitor_gpu_temperature
    above: 85
action:
  - service: persistent_notification.create
    data:
      title: GPU Overheat Warning
      message: >
        GPU temperature is {{ states('sensor.ollama_monitor_gpu_temperature') }}°C
```

#### Log when Ollama comes back online

```yaml
alias: Ollama back online
trigger:
  - platform: state
    entity_id: sensor.ollama_monitor_ollama_status
    from: offline
    to: online
action:
  - service: system_log.write
    data:
      message: "Ollama is back online — model: {{ state_attr('sensor.ollama_monitor_ollama_status', 'loaded_model') }}"
```

> These YAML automations go in `automations.yaml` or can be created via **Settings → Automations & Scenes** → **⊕ Create automation** → **Start from scratch**. The entity references work identically in the GUI builder.

### Using Sensors in Templates

```jinja2
{{ state_attr('sensor.ollama_monitor_ollama_status', 'loaded_model') }}
{{ state_attr('sensor.ollama_monitor_gpu_memory_used', 'memory_usage_pct') }}%
```

### Troubleshooting

| Problem | Solution |
|---|---|
| **"Could not connect" during setup** | Ensure ollama_monitor is running. Test from your HA host: `curl http://<monitor-ip>:3000/api/status`. Check firewalls, Docker network mode. |
| **Sensors showing `unknown`** | Check ollama_monitor logs (`journalctl -u ollama_monitor -f`). GPU sensors will be `unknown` if `nvidia-smi` is not available on the host. |
| **Integration doesn't appear after install** | You must **restart Home Assistant** after installing. Then go to **Settings → Devices & Services → Add integration** and search again. |
| **No entities in dashboard** | After first setup, sensors take up to 15 seconds (one poll cycle) to appear. Refresh the page. |
| **Cannot connect from Docker HA** | If ollama_monitor uses `--network host`, reference the host IP. If ollama_monitor is also in Docker, ensure they share a network or use the host bridge. |
| **Check HA logs for errors** | Add to `configuration.yaml`: `logger:\n  logs:\n    custom_components.ollama_monitor: debug` |
