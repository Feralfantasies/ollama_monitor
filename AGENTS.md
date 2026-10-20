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
│   ├── llamacpp.rs               # llama.cpp llama-server client (/v1/models + /props)
│   ├── models.rs                 # Shared data structures (serde models)
│   ├── ollama.rs                 # Ollama REST API client (fallback backend)
│   ├── switcher.rs               # VRAM arbitration between mutually exclusive GPU workloads
│   ├── system.rs                 # Linux /proc-based system metric collection
│   ├── bin/
│   │   └── switch_agent.rs       # SECOND BINARY: allowlisted systemctl controller
│   └── tests.rs                  # Integration tests (mock Ollama server + mock nvidia-smi)
├── tests/fixtures/               # Real captures from live llama-server instances
├── okf/                          # Open Knowledge Format bundle (agent context)
│   ├── index.md                  # Bundle root index
│   ├── overview.md               # Project overview
│   ├── architecture.md           # System architecture and data flow
│   ├── src/                      # Source module concept documents
│   ├── db/                       # Database schema concept
│   ├── api/                      # REST API reference
│   ├── ha-integration/           # Home Assistant integration docs
│   ├── deployment/               # Deployment guides (Docker, systemd)
│   └── testing/                  # Testing strategy and CI/CD docs
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
   - `GET /` — Embedded dark-themed web dashboard (vanilla JS, canvas charts, Model Switcher panel)
   - `GET /api/status` — Full combined status (model backend + GPU metrics)
   - `GET /api/gpu` — GPU metrics only
   - `GET /api/models` — Model information only
   - `GET /api/history?range=15m|1h|6h|1d|7d|30d` — Time-series history data points
   - `GET /api/nodes` — Per-node workload state, VRAM headroom, switch progress
   - `GET /api/switch` — Progress of the in-flight or most recent switch
   - `POST /api/model` — Make a node's model VRAM-resident (background job, returns 202)
6. **llama.cpp client** (`llamacpp.rs`) — Polls `llama-server` `/props` and `/v1/models`. Needed because llama-server returns **404** for Ollama's `/api/tags` and `/api/ps`. `/props` answers **503 while a model is loading**, so HTTP 200 is the only valid readiness signal.
7. **Switcher** (`switcher.rs`) — Pure state machine (`SwitchPlan`) plus an async driver (`Switcher`). Enforces: never start a model until host VRAM is *observed* below `SWITCH_VRAM_FLOOR_MIB`; fail rather than proceed otherwise. Attribution comes from switch agents because `nvidia-smi` reports container processes as `[Not Found]`.
8. **Switch agent** (`bin/switch_agent.rs`) — Separate static binary deployed inside each GPU workload container. Exposes `GET /health|/state` and `POST /start|/stop` behind a bearer token, restricted to an allowlist of systemd units. See `okf/deployment/switch-agent.md`.
9. **Main loop** (`main.rs`) — After startup, spawns a background task that runs `run_one_refresh` at the configured interval. Each cycle discovers models (llama.cpp nodes if configured, else Ollama), queries GPU, persists to SQLite, and updates shared `AppState`.

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
| `GPU_QUERY_BIN` | `nvidia-smi` | GPU query binary; overridable for tests |
| `MONITOR_API_TOKEN` | *(unset)* | If set, `POST /api/model` requires `Authorization: Bearer <token>` |
| `NODE_<i>_NAME` | *(required)* | Node id; scanning stops at the first index with no name |
| `NODE_<i>_KIND` | `llama` | `llama` or `comfyui` — decides how readiness is probed |
| `NODE_<i>_SERVER_URL` | *(none)* | llama-server root for `/props` + `/v1/models` |
| `NODE_<i>_AGENT_URL` | *(none)* | Switch-agent root |
| `NODE_<i>_AGENT_TOKEN` | *(none)* | **Secret** — agent bearer token; redacted from logs |
| `NODE_<i>_UNIT` | *(none)* | systemd unit the agent may control |
| `NODE_<i>_MODEL_ALIAS` | *(none)* | Model alias this node serves |
| `NODE_SCAN_MAX` | `16` | Upper bound on node indices scanned |
| `SWITCH_VRAM_FLOOR_MIB` | `2000` | VRAM must fall below this before a target starts |
| `SWITCH_RELEASE_TIMEOUT_SECS` | `180` | Wait budget for VRAM release |
| `SWITCH_READY_TIMEOUT_SECS` | `600` | Wait budget for the target to become ready |
| `SWITCH_POLL_INTERVAL_SECS` | `5` | Poll interval during a switch |

Secrets (`MONITOR_API_TOKEN`, `NODE_<i>_AGENT_TOKEN`) are held in a `Secret` type
whose `Debug` renders `[REDACTED]`, because `main.rs` logs the whole config via
`tracing::info!(?config, …)`. No token is ever emitted into the served dashboard.

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

## OKF Knowledge Bundle (`okf/`)

An [Open Knowledge Format](https://github.com/GoogleCloudPlatform/knowledge-catalog/blob/main/okf/SPEC.md) bundle providing agent-readable project context. Each document has YAML frontmatter with `type`, `title`, `description`, optional `resource`, `tags`, and `timestamp`.

Start at `okf/index.md` to browse all concepts. Cross-linked via `/`-prefixed bundle-relative paths.

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
