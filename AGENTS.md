# Ollama Monitor — Project Context

A monitoring tool for Ollama AI servers. Collects GPU metrics (via `nvidia-smi`) and Ollama model info (via the Ollama REST API), exposes everything via REST endpoints + an embedded web dashboard, and ships a Home Assistant custom integration so metrics appear as native HA sensors.

## Repository Layout

```
ollama_monitor/
├── src/                          # Rust application
│   ├── main.rs                   # Entry point
│   ├── api.rs                    # Axum REST API + embedded web dashboard
│   ├── config.rs                 # Environment variable configuration with defaults
│   ├── db.rs                     # SQLite persistence (history, migrations)
│   ├── gpu.rs                    # nvidia-smi CLI wrapper + CSV parser
│   ├── models.rs                 # Shared data structures (serde models)
│   ├── ollama.rs                 # Ollama REST API client
│   └── tests.rs                  # Integration tests (mock Ollama server + mock nvidia-smi)
├── ha_integration/               # Home Assistant custom integration
│   └── custom_components/ollama_monitor/
│       ├── __init__.py           # Integration setup/unload
│       ├── config_flow.py        # GUI config flow (monitor URL input)
│       ├── const.py              # Constants, sensor keys, platform list
│       ├── coordinator.py        # DataUpdateCoordinator (polls /api/status every 15s)
│       ├── manifest.json         # HA integration manifest (version 1.0.0)
│       ├── sensor.py             # Sensor platform (ollama_status, gpu_temp, gpu_mem, gpu_util, gpu_power)
│       └── strings.json          # Localized strings for config flow UI
├── .github/workflows/
│   ├── ci.yml                    # CI — fmt, clippy, build, test, musl static build
│   └── release.yml               # Auto-tag + release musl binary on push to main
└── hooks/pre-commit              # Pre-commit hook (fmt + clippy + tests)
```

## Rust Application (`src/`)

### Architecture

1. **Config** (`config.rs`) — Loaded entirely from environment variables with sensible defaults. No config files needed.
2. **Ollama client** (`ollama.rs`) — Polls `OLLAMA_HOST/api/tags` to fetch available models (~10s timeout).
3. **GPU module** (`gpu.rs`) — Runs `nvidia-smi --query-gpu=... --format=csv,noheader,nounits` and parses the CSV output. On failure returns a placeholder (all nulls).
4. **Database** (`db.rs`) — SQLite (via sqlx) stores one row per refresh cycle in `check_results` table. Auto-prunes rows older than 30 days. Auto-vacuum enabled.
5. **API server** (`api.rs`) — Axum HTTP server with:
   - `GET /` — Embedded dark-themed web dashboard (vanilla JS, canvas charts)
   - `GET /api/status` — Full combined status (Ollama + GPU metrics)
   - `GET /api/gpu` — GPU metrics only
   - `GET /api/models` — Model information only
   - `GET /api/history?range=15m|1h|6h|1d|7d|30d` — Time-series history data points
6. **Main loop** (`main.rs`) — After startup, spawns a background task that runs `run_one_refresh` at the configured interval. Each cycle fetches Ollama models, queries GPU, persists to SQLite, and updates shared `AppState`.

### Key Data Flow

```
ollama API (localhost) ──→ OllamaClient ──┐
                                            ├──→ run_one_refresh() ──→ SQLite ──→ API endpoints
nvidia-smi CLI ────────────────────────────┘                                            └──→ web dashboard
```

### Configuration (Environment Variables)

| Variable | Default | Description |
|---|---|---|
| `OLLAMA_HOST` | `http://127.0.0.1` | Ollama server base URL |
| `OLLAMA_PORT` | `11434` | Ollama server port |
| `SERVER_BIND` | `0.0.0.0` | Address to bind the API server to |
| `SERVER_PORT` | `3000` | Port for the API server |
| `REFRESH_INTERVAL_SECS` | `15` | Seconds between status refreshes |
| `GPU_DEVICE_INDEX` | `0` | NVIDIA GPU device index to query |

### Dependencies (notable)

- **axum 0.8** — HTTP server framework (with WebSocket feature)
- **reqwest 0.12** — HTTP client (rustls TLS)
- **sqlx 0.8** — Async SQL with SQLite
- **tokio 1** — Async runtime
- **tracing / tracing-subscriber** — Structured logging with env filter

### Testing

All tests run **without a real GPU or running Ollama instance**:
- **Mock Ollama server** — A lightweight axum server on a random port returns deterministic model data on `/api/tags`.
- **Mock `nvidia-smi` binary** — A shell script prints deterministic CSV output, passed to `query_gpu_bin()` instead of the system binary.
- Tests are in `src/tests.rs` (integration tests) and `src/db.rs` (unit tests).
- **Test matrix:**
  - `test_full_pipeline_history_accumulates` — 3 refresh cycles → 3 DB rows, history query returns correct data
  - `test_history_timestamps_ordered` — Strictly ascending timestamps
  - `test_unreachable_ollama_records_correctly` — Ollama down → `ollama_reachable: false`, GPU still populated
  - `test_no_gpu_records_nulls` — GPU placeholder → all GPU fields null, Ollama still recorded
  - `test_mixed_gpu_availability` — Alternating GPU present/absent → DB tolerates mixed nulls
  - `test_dashboard_api_endpoints` — Full HTTP API end-to-end test (all endpoints)
  - `test_insert_and_query_check_result` — DB insert + query round-trip
  - `test_multiple_check_results_ordered_desc` — Multiple rows returned newest-first
  - `test_insert_check_result_with_null_fields` — All-null GPU row round-trip

## Home Assistant Integration (`ha_integration/`)

A native HA custom integration — no YAML config, no MQTT, no terminal access needed. Installed via HACS or by copying to `custom_components/ollama_monitor/`.

### Installation Target

When copying manually, files from `ha_integration/custom_components/ollama_monitor/` go to:
- `/config/custom_components/ollama_monitor/` (HA supervised/Docker)
- `~/.homeassistant/custom_components/ollama_monitor/` (HA core)
- `/usr/share/hassio/homeassistant/custom_components/ollama_monitor/` (HA OS)

### Integration Architecture

- **Config Flow** (`config_flow.py`) — User enters the monitor URL via GUI. Validates connection by fetching `/api/status` before saving.
- **Coordinator** (`coordinator.py`) — `DataUpdateCoordinator` polls `/api/status` every 15 seconds. Parses JSON response into `OllamaMonitorData`.
- **Sensors** (`sensor.py`) — 5 sensors registered under a single device:

| Sensor | State | Attributes |
|---|---|---|
| Ollama Status | `online` / `offline` | `loaded_model`, `model_count`, `ollama_url` |
| GPU Temperature | °C (measurement) | `gpu_name` |
| GPU Memory Used | MiB (measurement) | `gpu_name`, `memory_total_mib`, `memory_remaining_mib`, `memory_usage_pct` |
| GPU Utilization | % (measurement) | `gpu_name` |
| GPU Power | W (measurement) | `gpu_name` |

All numeric sensors use `state_class: measurement` for HA statistics/history graphs.

## CI/CD

### CI (`.github/workflows/ci.yml`)
- Runs on custom `firethorn` runner (self-hosted Arc runner)
- **Format** → `cargo fmt --check`
- **Clippy** → `cargo clippy --all-targets -- -D warnings`
- **Build** → `RUSTFLAGS="-D warnings" cargo build`
- **Tests** → `RUSTFLAGS="-D warnings" cargo test --all-targets`
- **Musl build** → `cargo build --release --target x86_64-unknown-linux-musl`

### Releases (`.github/workflows/release.yml`)
- On push to `main`: auto-bump semantic version tag, build musl static binary, create GitHub release with binary artifact

### Pre-commit Hook (`hooks/pre-commit`)
- Runs `cargo fmt --check`, `cargo clippy`, and `cargo test` before every commit. Must pass for commit to succeed.

## Build & Run

```bash
# Development
cargo build

# Release with warnings as errors
RUSTFLAGS="-D warnings" cargo build --release

# Full CI checks locally (matches GitHub Actions)
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
RUSTFLAGS="-D warnings" cargo build --release
cargo test --all-targets
```

### Docker

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

### systemd

See README.md for the full systemd service unit file. Key service: `ollama_monitor.service`.

## Files to Exclude

- `ollama_monitor.db*` — SQLite database (auto-created, .gitignored)
- `target/` — Build artifacts (gitignored)
- `ha_integration/**/__pycache__/` and `*.pyc` — Python cache (gitignored)
