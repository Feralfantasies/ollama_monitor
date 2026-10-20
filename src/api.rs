/// Axum REST API server + embedded web dashboard.
use anyhow::Result;
use axum::{
    extract::State,
    http::{header::AUTHORIZATION, HeaderMap, StatusCode},
    response::{Html, IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, net::SocketAddr, sync::Arc, time::Duration};
use tokio::sync::RwLock;
use tracing::info as log_info;

use crate::config::{Config, NodeKind};
use crate::db::{self, HistoryRange};
use crate::llamacpp::LlamaCppClient;
use crate::models::{
    ApiGpuResponse, ApiHistoryResponse, ApiModelResponse, ApiSystemHistoryResponse,
    ApiSystemResponse, CheckResult, GpuHistoryPoint, GpuMetric, MonitorStatus, SystemHistoryPoint,
};
use crate::ollama::OllamaClient;
use crate::switcher::{NodeStatus, SwitchProgress, Switcher};
use sqlx::SqlitePool;

/// Render the current time as an RFC 3339 UTC timestamp.
///
/// This previously formatted a `Duration`-since-epoch as `"{days} days {HH}:{MM}"`,
/// which produced values like `"20745 days 22:38"` — a day count since 1970 rather
/// than a wall-clock time, and unparseable by any standard tool. Observed live on
/// the deployed monitor (`/api/gpu` and `/api/status`).
///
/// The Home Assistant integration treats this field as an opaque string
/// (`data.get("timestamp", "")` in `coordinator.py`), so the format change is safe;
/// no test asserts on the old shape.
fn now_ts() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

#[derive(Clone)]
pub struct AppState {
    pub latest_status: Arc<RwLock<Option<MonitorStatus>>>,
    pub db_pool: SqlitePool,
    pub config: Config,
    /// Executes GPU workload switches against the per-container agents.
    pub switcher: Switcher,
    /// Shared progress of the in-flight (or most recent) switch.
    pub switch_progress: Arc<RwLock<SwitchProgress>>,
}

impl AppState {
    pub fn with_config(db_pool: SqlitePool, config: Config) -> Self {
        let switcher = Switcher::new(config.clone());
        Self {
            latest_status: Arc::new(RwLock::new(None)),
            db_pool,
            config,
            switcher,
            switch_progress: Arc::new(RwLock::new(SwitchProgress::default())),
        }
    }
}

// ── Background refresh loop ──────────────────────────────

/// Discover served models: llama.cpp nodes first, Ollama as the fallback.
///
/// `llama-server` does **not** implement Ollama's API — `/api/tags` and `/api/ps`
/// both return 404 on b11583 — so pointing the Ollama client at it produces
/// nothing but a warning every refresh cycle. When `NODE_<i>_*` entries are
/// configured we probe each llama node's OpenAI-compatible endpoints instead.
///
/// The Ollama path is retained unchanged for deployments that still run Ollama.
///
/// Returns `(available_models, loaded_model, backend_reachable)`. A model counts
/// as `loaded` only when its node reports ready, i.e. `/props` answered 200 —
/// the same distinction that keeps the switcher from OOMing the card.
async fn discover_models(config: &Config) -> (Vec<String>, Option<String>, bool) {
    let has_llama_nodes = config
        .nodes
        .iter()
        .any(|n| n.kind == NodeKind::Llama && n.server_url.is_some());

    if has_llama_nodes {
        let mut available: Vec<String> = Vec::new();
        let mut loaded: Option<String> = None;
        let mut any_reachable = false;

        for node in &config.nodes {
            if node.kind != NodeKind::Llama {
                continue;
            }
            let Some(url) = node.server_url.as_ref() else {
                continue;
            };
            let probe = LlamaCppClient::new(url.clone()).probe().await;
            if probe.reachable {
                any_reachable = true;
            }
            // The resident model is the one that is actually loaded and serving.
            if probe.ready {
                if let Some(alias) = probe.alias.clone() {
                    if !available.contains(&alias) {
                        available.push(alias.clone());
                    }
                    if loaded.is_none() {
                        loaded = Some(alias);
                    }
                }
            }
            for id in &probe.available_models {
                if !available.contains(id) {
                    available.push(id.clone());
                }
            }
        }
        return (available, loaded, any_reachable);
    }

    let client = OllamaClient::new(config.ollama_base_url());
    let tags_resp = client.try_fetch_models().await;
    let available: Vec<String> = match &tags_resp {
        Some(tags) => tags.models.iter().map(|m| m.model_name.clone()).collect(),
        None => vec![],
    };
    let loaded_model = tags_resp
        .as_ref()
        .and_then(|t| t.models.first().cloned().map(|m| m.model_name));
    (available, loaded_model, tags_resp.is_some())
}

/// Run a single iteration of the refresh loop, updating state and persisting to DB.
pub async fn run_one_refresh<G: Fn(usize) -> GpuMetric>(
    config: &Config,
    state: &AppState,
    gpu_fn: &G,
) {
    let (available, loaded_model, backend_reachable) = discover_models(config).await;

    let gpu_metric = gpu_fn(config.gpu_device_index);

    let system_metric = tokio::task::spawn_blocking(crate::system::query_system)
        .await
        .unwrap_or_else(|e| {
            tracing::warn!("spawn_blocking for system metrics panicked: {}", e);
            crate::models::SystemMetric::placeholder()
        });

    let status = MonitorStatus {
        ollama_url: config.ollama_base_url(),
        // Field name retained for Home Assistant compatibility; the meaning has
        // broadened to "a configured model backend answered", which for a
        // llama.cpp deployment is a llama-server, not Ollama.
        ollama_reachable: backend_reachable,
        loaded_model: loaded_model.clone(),
        available_models: available.clone(),
        gpu: gpu_metric.clone(),
        system: system_metric.clone(),
        timestamp: now_ts(),
    };

    // Persist check result to SQLite.
    let check = CheckResult {
        ollama_url: status.ollama_url.clone(),
        ollama_reachable: status.ollama_reachable,
        loaded_model,
        available_models_count: available.len(),
        gpu_name: gpu_metric.name.clone(),
        gpu_temperature_c: gpu_metric.temperature_c,
        gpu_memory_used_mib: gpu_metric.memory_used_mib,
        gpu_memory_total_mib: gpu_metric.memory_total_mib,
        gpu_utilization_pct: gpu_metric.utilization_pct,
        gpu_power_watts: gpu_metric.power_watts,
        sys_memory_used_mib: system_metric.memory_used_mib,
        sys_memory_total_mib: system_metric.memory_total_mib,
        sys_memory_remaining_mib: system_metric.memory_remaining_mib,
        sys_memory_usage_pct: system_metric.memory_usage_pct,
        sys_cpu_utilization_pct: system_metric.cpu_utilization_pct,
    };

    if let Err(e) = db::insert_check_result(&state.db_pool, &check).await {
        tracing::warn!("Failed to persist check result: {}", e);
    }

    let mut latest = state.latest_status.write().await;
    *latest = Some(status);
}

pub async fn run_refresh_loop(config: &Config, state: AppState) {
    let interval = Duration::from_secs(config.refresh_interval_secs);

    loop {
        tokio::time::sleep(interval).await;
        run_one_refresh(config, &state, &crate::gpu::try_query_gpu).await;
    }
}

// ── HTTP Handlers ────────────────────────────────────────

async fn handle_dashboard(State(state): State<AppState>) -> Html<String> {
    let interval_ms = state.config.refresh_interval_secs * 1000;
    Html(DASHBOARD_HTML.replace("REFRESH_INTERVAL_MS_PLACEHOLDER", &interval_ms.to_string()))
}

async fn handle_api_status(
    State(state): State<AppState>,
) -> Result<axum::Json<MonitorStatus>, (StatusCode, String)> {
    let latest = state.latest_status.read().await;
    match latest.as_ref() {
        Some(s) => Ok(axum::Json(s.clone())),
        None => Err((
            StatusCode::SERVICE_UNAVAILABLE,
            "First refresh not yet complete".into(),
        )),
    }
}

async fn handle_api_gpu(
    State(state): State<AppState>,
) -> Result<axum::Json<ApiGpuResponse>, (StatusCode, String)> {
    let latest = state.latest_status.read().await;
    match latest.as_ref() {
        Some(s) => Ok(axum::Json(ApiGpuResponse {
            gpu: s.gpu.clone(),
            timestamp: s.timestamp.clone(),
        })),
        None => Err((
            StatusCode::SERVICE_UNAVAILABLE,
            "First refresh not yet complete".into(),
        )),
    }
}

async fn handle_api_models(
    State(state): State<AppState>,
) -> Result<axum::Json<ApiModelResponse>, (StatusCode, String)> {
    let latest = state.latest_status.read().await;
    match latest.as_ref() {
        Some(s) => Ok(axum::Json(ApiModelResponse {
            loaded_model: s.loaded_model.clone(),
            available_models: s.available_models.clone(),
            total_count: s.available_models.len(),
        })),
        None => Err((
            StatusCode::SERVICE_UNAVAILABLE,
            "First refresh not yet complete".into(),
        )),
    }
}

async fn handle_api_history(
    State(state): State<AppState>,
    axum::extract::Query(params): axum::extract::Query<HashMap<String, String>>,
) -> Result<axum::Json<ApiHistoryResponse>, (StatusCode, String)> {
    let range = params.get("range").map(|s| s.as_str()).unwrap_or("15m");
    let range = HistoryRange::parse(range);

    let rows = db::query_history(&state.db_pool, range)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to query history: {}", e),
            )
        })?;

    let points = rows
        .into_iter()
        .map(|(ts, memory, temp)| GpuHistoryPoint {
            timestamp: ts,
            memory_used_mib: memory,
            temperature_c: temp,
        })
        .collect();

    Ok(axum::Json(ApiHistoryResponse { points }))
}

async fn handle_api_system(
    State(state): State<AppState>,
) -> Result<axum::Json<ApiSystemResponse>, (StatusCode, String)> {
    let latest = state.latest_status.read().await;
    match latest.as_ref() {
        Some(s) => Ok(axum::Json(ApiSystemResponse {
            system: s.system.clone(),
            timestamp: s.timestamp.clone(),
        })),
        None => Err((
            StatusCode::SERVICE_UNAVAILABLE,
            "First refresh not yet complete".into(),
        )),
    }
}

async fn handle_api_sys_history(
    State(state): State<AppState>,
    axum::extract::Query(params): axum::extract::Query<HashMap<String, String>>,
) -> Result<axum::Json<ApiSystemHistoryResponse>, (StatusCode, String)> {
    let range = params.get("range").map(|s| s.as_str()).unwrap_or("15m");
    let range = HistoryRange::parse(range);

    let rows = db::query_system_history(&state.db_pool, range)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to query system history: {}", e),
            )
        })?;

    let points = rows
        .into_iter()
        .map(|(ts, mem, cpu)| SystemHistoryPoint {
            timestamp: ts,
            memory_used_mib: mem,
            cpu_utilization_pct: cpu,
        })
        .collect();

    Ok(axum::Json(ApiSystemHistoryResponse { points }))
}

// ── Model switching ────────────────────────────────────

/// Body of `POST /api/model`. Either selector may be used:
///
/// ```json
/// { "target": "llama02" }          // by node name
/// { "model":  "Precog-24B" }       // by served model alias
/// ```
#[derive(Debug, Clone, Deserialize)]
pub struct ModelSelectRequest {
    #[serde(default)]
    pub target: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
}

#[derive(Debug, Serialize)]
struct ApiNodesResponse {
    nodes: Vec<NodeStatus>,
    /// Names of nodes that can be switched to/from. The dashboard renders a
    /// control only for these; display-only nodes (missing agent URL, token or
    /// unit) are listed in `nodes` but cannot be selected.
    switchable_nodes: Vec<String>,
    vram_used_mib: Option<u64>,
    vram_total_mib: Option<u64>,
    vram_free_mib: Option<u64>,
    /// Alias of the model currently loaded and serving, if any.
    resident_model: Option<String>,
    /// Node holding the GPU right now, if identifiable.
    resident_node: Option<String>,
    switch: SwitchProgress,
}

fn json_err(status: StatusCode, msg: impl Into<String>) -> Response {
    (status, Json(serde_json::json!({ "error": msg.into() }))).into_response()
}

/// Resolve a switch target from either selector form.
///
/// The error type is deliberately a small `(StatusCode, String)` pair rather than
/// a built `Response`: `clippy::result_large_err` rejects a `Result` whose `Err`
/// variant carries a whole axum response (>=128 bytes) through every stack frame,
/// and the JSON envelope is only needed at the handler boundary.
fn resolve_target(
    state: &AppState,
    req: &ModelSelectRequest,
) -> Result<String, (StatusCode, String)> {
    let by_name = req
        .target
        .as_ref()
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty());
    let by_model = req
        .model
        .as_ref()
        .map(|m| m.trim().to_string())
        .filter(|m| !m.is_empty());

    let wanted = match (&by_name, &by_model) {
        (Some(n), _) => n.clone(),
        (None, Some(m)) => match state.config.node_by_alias(m) {
            Some(n) => n.name.clone(),
            None => {
                let known: Vec<String> = state
                    .config
                    .nodes
                    .iter()
                    .filter_map(|n| n.model_alias.clone())
                    .collect();
                return Err((
                    StatusCode::NOT_FOUND,
                    format!("no node serves model '{m}'; known aliases: {known:?}"),
                ));
            }
        },
        (None, None) => {
            return Err((
                StatusCode::BAD_REQUEST,
                "provide 'target' (node name) or 'model' (alias)".to_string(),
            ))
        }
    };

    match state.config.node_by_name(&wanted) {
        None => {
            let known: Vec<&str> = state.config.nodes.iter().map(|n| n.name.as_str()).collect();
            Err((
                StatusCode::NOT_FOUND,
                format!("unknown node '{wanted}'; configured nodes: {known:?}"),
            ))
        }
        Some(n) if !n.switchable() => Err((
            StatusCode::CONFLICT,
            format!(
                "node '{wanted}' is not switchable (missing agent URL, token or unit); \
                 it is display-only"
            ),
        )),
        Some(_) => Ok(wanted),
    }
}

/// `GET /api/nodes` — live per-node state plus VRAM headroom and switch progress.
async fn handle_api_nodes(State(state): State<AppState>) -> Response {
    let (nodes, vram_used) = state.switcher.node_statuses().await;

    // Total VRAM comes from the GPU metric already collected by the refresh loop,
    // so this endpoint does not shell out to nvidia-smi a second time.
    let vram_total = state
        .latest_status
        .read()
        .await
        .as_ref()
        .and_then(|s| s.gpu.memory_total_mib);
    let vram_free = match (vram_total, vram_used) {
        (Some(t), Some(u)) => Some(t.saturating_sub(u)),
        _ => None,
    };

    // Attribution comes from agent-reported state, NOT from nvidia-smi process
    // names: those render as [Not Found] across container PID namespaces.
    // Owned copies are extracted first because `nodes` is moved into the response.
    let (resident_model, resident_node) = match nodes.iter().find(|n| n.ready) {
        Some(n) => (n.model_alias.clone(), Some(n.name.clone())),
        None => (None, None),
    };
    let progress = state.switch_progress.read().await.clone();

    Json(ApiNodesResponse {
        switchable_nodes: state
            .config
            .switchable_nodes()
            .iter()
            .map(|n| n.name.clone())
            .collect(),
        nodes,
        vram_used_mib: vram_used,
        vram_total_mib: vram_total,
        vram_free_mib: vram_free,
        resident_model,
        resident_node,
        switch: progress,
    })
    .into_response()
}

/// `GET /api/switch` — progress of the in-flight or most recent switch.
async fn handle_api_switch(State(state): State<AppState>) -> Response {
    Json(state.switch_progress.read().await.clone()).into_response()
}

/// `POST /api/model` — request that a given model become the resident one.
///
/// Runs in the background and returns `202 Accepted` immediately: a switch
/// measured 36–148 s here, far too long to hold an HTTP request open behind a
/// reverse proxy. Poll `GET /api/switch` for progress.
///
/// Concurrency is refused rather than queued: two overlapping switches could
/// interleave their stop/start actions and load two models onto one card.
///
/// When `MONITOR_API_TOKEN` is set, the caller must present it as a bearer token.
/// This endpoint can stop and start production GPU workloads, so it is guarded
/// even though the service sits behind a LAN-only ingress.
async fn handle_api_model(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<ModelSelectRequest>,
) -> Response {
    if state.config.monitor_api_token.is_set() {
        let provided = headers
            .get(AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.strip_prefix("Bearer "))
            .unwrap_or("");
        if !state.config.monitor_api_token.matches(provided) {
            return json_err(
                StatusCode::UNAUTHORIZED,
                "valid bearer token required (MONITOR_API_TOKEN is set)",
            );
        }
    }
    let target = match resolve_target(&state, &req) {
        Ok(t) => t,
        Err((status, msg)) => return json_err(status, msg),
    };

    // Check-and-set under the write guard so two simultaneous POSTs cannot both
    // pass the "is a switch running" test. The guard is dropped before spawning:
    // the task itself takes this lock on its first progress update.
    {
        let mut p = state.switch_progress.write().await;
        if p.running {
            return json_err(
                StatusCode::CONFLICT,
                format!(
                    "a switch to '{}' is already in progress (phase: {}); \
                     refusing to interleave two switches",
                    p.target.clone().unwrap_or_default(),
                    p.phase
                ),
            );
        }
        *p = SwitchProgress {
            running: true,
            target: Some(target.clone()),
            phase: "queued".to_string(),
            vram_floor_mib: state.config.switch_vram_floor_mib,
            ..Default::default()
        };
    }

    let switcher = state.switcher.clone();
    let progress = state.switch_progress.clone();
    let spawn_target = target.clone();
    tokio::spawn(async move {
        if let Err(e) = switcher
            .switch_to_with_progress(&spawn_target, Some(progress.clone()))
            .await
        {
            // switch_to returns Err for setup problems (unknown node, start
            // failure); a VRAM/ready timeout is reported as a failed outcome
            // instead. Either way the job must not stay marked running.
            let mut g = progress.write().await;
            g.running = false;
            g.phase = "failed".to_string();
            g.error = Some(e.to_string());
        }
    });

    (
        StatusCode::ACCEPTED,
        Json(serde_json::json!({
            "accepted": true,
            "target": target,
            "detail": "switch started in background; poll GET /api/switch",
            "vram_floor_mib": state.config.switch_vram_floor_mib,
        })),
    )
        .into_response()
}

// ── Router builder ───────────────────────────────────────

pub async fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/", get(handle_dashboard))
        .route("/api/status", get(handle_api_status))
        .route("/api/gpu", get(handle_api_gpu))
        .route("/api/models", get(handle_api_models))
        .route("/api/history", get(handle_api_history))
        .route("/api/system", get(handle_api_system))
        .route("/api/sys-history", get(handle_api_sys_history))
        .route("/api/nodes", get(handle_api_nodes))
        .route("/api/switch", get(handle_api_switch))
        .route("/api/model", post(handle_api_model))
        .with_state(state)
}

pub async fn bind_listener(config: &Config) -> Result<SocketAddr> {
    let addr: SocketAddr = format!("{}:{}", config.server_bind, config.server_port)
        .parse()
        .expect("invalid listen address");
    log_info!("Server will bind to {}", addr);
    Ok(addr)
}

// ── Embedded Dashboard HTML ────────────────

const DASHBOARD_HTML: &str = r#"<!DOCTYPE html>
<html lang="en"><head><meta charset="utf-8"/><meta name="viewport" content="width=device-width,initial-scale=1"/>
<title>Ollama Monitor</title>
<style>
:root{--bg:#1a1b26;--card:#24283b;--text:#c0caf5;--accent:#7aa2f7;--ok:#9ece6a;--warn:#e0af68;--err:#f7768e;--muted:#565f89;--mem:#7aa2f7;--temp:#f7768e;--sysmem:#9ece6a;--cpu:#e0af68}
*{box-sizing:border-box;margin:0;padding:0}
body{font-family:'Segoe UI',system-ui,sans-serif;background:var(--bg);color:var(--text);padding:2rem}
h1{text-align:center;margin-bottom:1.5rem;color:var(--accent)}
.grid{display:grid;grid-template-columns:repeat(auto-fit,minmax(280px,1fr));gap:1.25rem;max-width:960px;margin:0 auto}
.card{background:var(--card);border-radius:12px;padding:1.5rem;box-shadow:0 2px 8px rgba(0,0,0,.3)}
.card h2{font-size:1rem;color:var(--muted);text-transform:uppercase;letter-spacing:1px;margin-bottom:.75rem}
.metric{display:flex;justify-content:space-between;padding:.4rem 0;border-bottom:1px solid #2f3650}
.metric:last-child{border-bottom:none}
.label{color:var(--muted)}.value{font-weight:600}
.ok{color:var(--ok)}.warn{color:var(--warn)}.err{color:var(--err)}
#ts{text-align:center;margin-top:1.5rem;color:var(--muted);font-size:.85rem}
.dot{display:inline-block;width:10px;height:10px;border-radius:50%;margin-right:6px;vertical-align:middle}
.bar-bg{width:100%;background:#2f3650;border-radius:4px;height:8px;margin-top:4px}
.bar-fill{height:100%;border-radius:4px;transition:width .4s ease}
.full-width{grid-column:1/-1}
.range-bar{display:flex;gap:6px;justify-content:center;margin-bottom:12px}
.range-bar button{background:#2f3650;border:1px solid #414868;color:var(--text);padding:6px 14px;border-radius:6px;cursor:pointer;font-size:.85rem;transition:all .2s}
.range-bar button:hover{background:#414868}
.range-bar button.active{background:var(--accent);color:var(--bg);border-color:var(--accent);font-weight:600}
.chart-wrap{position:relative;width:100%;height:220px;background:#1d202f;border-radius:8px;overflow:hidden}
.chart-wrap canvas{position:absolute;top:0;left:0;width:100%;height:100%}
.chart-legend{display:flex;justify-content:center;gap:20px;margin-bottom:8px}
.chart-legend span{font-size:.8rem;display:flex;align-items:center;gap:6px}
.chart-legend .swatch{width:12px;height:12px;border-radius:3px}
.chart-hint{position:absolute;top:8px;right:12px;font-size:.75rem;color:var(--muted)}
.chart-tooltip{position:absolute;display:none;background:#1f2335;border:1px solid #414868;border-radius:6px;padding:8px 12px;font-size:.8rem;pointer-events:none;color:var(--text);z-index:10}
.chart-tooltip .tt-label{color:var(--muted);font-size:.7rem}
.sw-btn{background:#2f3650;border:1px solid #414868;color:var(--text);padding:4px 10px;border-radius:6px;cursor:pointer;font-size:.78rem;margin-left:10px;transition:all .2s}
.sw-btn:hover{background:var(--accent);color:var(--bg);border-color:var(--accent)}
.sw-btn:disabled{opacity:.45;cursor:not-allowed}
.sw-resident{color:var(--ok);font-weight:600;font-size:.8rem;margin-left:10px}
.sw-note{margin-top:10px;font-size:.82rem;color:var(--muted);line-height:1.45}
</style></head><body>
<h1>Ollama Monitor</h1>
<div class="grid">
  <div class="card"><h2>Connection</h2>
    <div class="metric"><span class="label">Ollama URL</span><span class="value" id="url">—</span></div>
    <div class="metric"><span class="label">Reachable</span><span class="value" id="reach">—</span></div></div>
  <div class="card"><h2>Models</h2>
    <div class="metric"><span class="label">Loaded</span><span class="value" id="loaded">—</span></div>
    <div class="metric"><span class="label">Available</span><span class="value" id="avail">—</span></div></div>
  <div class="card"><h2>GPU Memory</h2>
    <div class="metric"><span class="label">Used</span><span class="value" id="mu">—</span></div>
    <div class="metric"><span class="label">Total</span><span class="value" id="mt">—</span></div>
    <div class="metric"><span class="label">Remaining</span><span class="value" id="mr">—</span></div>
    <div class="bar-bg"><div class="bar-fill" id="mb" style="width:0%"></div></div></div>
  <div class="card"><h2>GPU Health</h2>
    <div class="metric"><span class="label">Name</span><span class="value" id="gn">—</span></div>
    <div class="metric"><span class="label">Temperature</span><span class="value" id="gp">—</span></div>
    <div class="metric"><span class="label">Utilisation</span><span class="value" id="gu">—</span></div>
    <div class="metric"><span class="label">Power</span><span class="value" id="gpw">—</span></div></div>
  <div class="card"><h2>System Memory</h2>
    <div class="metric"><span class="label">Used</span><span class="value" id="smu">—</span></div>
    <div class="metric"><span class="label">Total</span><span class="value" id="smt">—</span></div>
    <div class="metric"><span class="label">Remaining</span><span class="value" id="smr">—</span></div>
    <div class="bar-bg"><div class="bar-fill" id="smb" style="width:0%"></div></div></div>
  <div class="card"><h2>CPU</h2>
    <div class="metric"><span class="label">Utilisation</span><span class="value" id="cpu">—</span></div></div>

  <div class="card full-width">
    <h2>Model Switcher</h2>
    <div class="metric"><span class="label">Resident model</span><span class="value" id="resModel">&mdash;</span></div>
    <div class="metric"><span class="label">Resident node</span><span class="value" id="resNode">&mdash;</span></div>
    <div class="metric"><span class="label">VRAM used</span><span class="value" id="swVram">&mdash;</span></div>
    <div class="metric"><span class="label">VRAM free</span><span class="value" id="swFree">&mdash;</span></div>
    <div class="metric"><span class="label">Switch floor</span><span class="value" id="swFloor">&mdash;</span></div>
    <div class="metric"><span class="label">Switch phase</span><span class="value" id="swPhase">&mdash;</span></div>
    <div id="nodeList" style="margin-top:10px"></div>
    <p class="sw-note" id="swMsg"></p>
    <p class="sw-note">Only one workload can be VRAM-resident at a time. Switching stops the current holder, waits for VRAM to fall below the floor, then loads the target (36&ndash;161&nbsp;s measured).</p>
  </div>

  <div class="card full-width">
    <h2>GPU History</h2>
    <div class="range-bar" id="rangeBar">
      <button data-range="15m" class="active">15 min</button>
      <button data-range="1h">1 hour</button>
      <button data-range="6h">6 hours</button>
      <button data-range="1d">1 day</button>
      <button data-range="7d">7 days</button>
      <button data-range="30d">30 days</button>
    </div>
    <div class="chart-legend"><span><span class="swatch" style="background:var(--mem)"></span>Memory (MiB)</span><span><span class="swatch" style="background:var(--temp)"></span>Temperature (°C)</span></div>
    <div class="chart-wrap" id="memWrap">
      <canvas id="memChart"></canvas>
      <div class="chart-tooltip" id="memTT"></div>
      <span class="chart-hint" id="memHint"></span>
    </div>
    <div style="margin-top:12px"></div>
    <div class="chart-wrap" id="tempWrap">
      <canvas id="tempChart"></canvas>
      <div class="chart-tooltip" id="tempTT"></div>
      <span class="chart-hint" id="tempHint"></span>
    </div>
  </div>

  <div class="card full-width">
    <h2>System History</h2>
    <div class="range-bar" id="sysRangeBar">
      <button data-range="15m" class="active">15 min</button>
      <button data-range="1h">1 hour</button>
      <button data-range="6h">6 hours</button>
      <button data-range="1d">1 day</button>
      <button data-range="7d">7 days</button>
      <button data-range="30d">30 days</button>
    </div>
    <div class="chart-legend">
      <span><span class="swatch" style="background:var(--sysmem)"></span>Memory (MiB)</span>
      <span><span class="swatch" style="background:var(--cpu)"></span>CPU (%)</span>
    </div>
    <div class="chart-wrap" id="sysMemWrap">
      <canvas id="sysMemChart"></canvas>
      <div class="chart-tooltip" id="sysMemTT"></div>
      <span class="chart-hint" id="sysMemHint"></span>
    </div>
    <div style="margin-top:12px"></div>
    <div class="chart-wrap" id="cpuWrap">
      <canvas id="cpuChart"></canvas>
      <div class="chart-tooltip" id="cpuTT"></div>
      <span class="chart-hint" id="cpuHint"></span>
    </div>
  </div>
</div><p id="ts">Loading…</p>
<script>
function $e(i){return document.getElementById(i);}function v(x){return x!=null?Math.round(x):'—';}

async function refresh(){
  try{
    const r=await fetch('/api/status');if(!r.ok)throw new Error('HTTP '+r.status);
    const d=await r.json();
    $e('url').textContent=d.ollama_url||'—';
    var e=$e('reach');e.innerHTML=d.ollama_reachable?'<span class="dot" style="background:var(--ok)"></span>Yes':'<span class="dot" style="background:var(--err)"></span>No';
    $e('loaded').textContent=d.loaded_model||'None';
    $e('avail').textContent=d.available_models.length+' model(s)';
    // GPU
    $e('mu').textContent=v(d.gpu.memory_used_mib)+' MiB';$e('mt').textContent=v(d.gpu.memory_total_mib)+' MiB';$e('mr').textContent=v(d.gpu.memory_remaining_mib)+' MiB';
    if(d.gpu.memory_total_mib>0){var p=Math.round(d.gpu.memory_used_mib/d.gpu.memory_total_mib*100);var b=$e('mb');b.style.width=p+'%';b.style.background=p>90?'var(--err)':p>70?'var(--warn)':'var(--ok)';}
    $e('gn').textContent=d.gpu.name||'—';
    e=$e('gp');e.textContent=(d.gpu.temperature_c!=null)?d.gpu.temperature_c.toFixed(1)+' °C':'—';e.className='value '+(d.gpu.temperature_c>80?'err':d.gpu.temperature_c>65?'warn':'ok');
    $e('gu').textContent=v(d.gpu.utilization_pct)+'%';$e('gpw').textContent=v(d.gpu.power_watts)+' W';
    // System
    $e('smu').textContent=v(d.system.memory_used_mib)+' MiB';$e('smt').textContent=v(d.system.memory_total_mib)+' MiB';$e('smr').textContent=v(d.system.memory_remaining_mib)+' MiB';
    if(d.system.memory_total_mib>0){var sp=Math.round(d.system.memory_used_mib/d.system.memory_total_mib*100);var sb=$e('smb');sb.style.width=sp+'%';sb.style.background=sp>90?'var(--err)':sp>70?'var(--warn)':'var(--ok)';}
    e=$e('cpu');e.textContent=(d.system.cpu_utilization_pct!=null)?d.system.cpu_utilization_pct.toFixed(1)+' %':'—';e.className='value '+(d.system.cpu_utilization_pct>90?'err':d.system.cpu_utilization_pct>60?'warn':'ok');
    $e('ts').textContent='Last updated: '+new Date().toLocaleTimeString();
  }catch(e){$e('ts').textContent='Error: '+e.message;}
}

/* ── Chart helpers ── */
function formatTime(ts,range){
  var d=new Date(ts);
  if(range==='15m'||range==='1h'||range==='6h')return d.getHours().toString().padStart(2,'0')+':'+d.getMinutes().toString().padStart(2,'0');
  return(d.getMonth()+1)+'/'+d.getDate()+' '+d.getHours().toString().padStart(2,'0')+':'+d.getMinutes().toString().padStart(2,'0');
}

function drawChart(canvasId,dataKey,color,range,minVal,maxVal,data){
  var canvas=$e(canvasId),ctx=canvas.getContext('2d');
  var rect=canvas.parentElement.getBoundingClientRect();
  canvas.width=rect.width*devicePixelRatio;canvas.height=rect.height*devicePixelRatio;
  ctx.scale(devicePixelRatio,devicePixelRatio);
  var W=rect.width,H=rect.height,P={t:20,r:50,b:25,l:50};
  var cW=W-P.l-P.r,cH=H-P.t-P.b;
  ctx.clearRect(0,0,W,H);

  // Grid
  ctx.strokeStyle='#2f3650';ctx.lineWidth=1;
  for(let i=0;i<=4;i++){
    var y=P.t+cH*(1-i/4);
    ctx.beginPath();ctx.moveTo(P.l,y);ctx.lineTo(W-P.r,y);ctx.stroke();
    ctx.fillStyle='#565f89';ctx.font='11px system-ui';ctx.textAlign='right';ctx.textBaseline='middle';
    var label=Math.round(minVal+(maxVal-minVal)*i/4);
    ctx.fillText(label,P.l-8,y);
  }

  if(!data||data.length<2){ctx.fillStyle='#565f89';ctx.font='13px system-ui';ctx.textAlign='center';ctx.fillText('No history data yet',W/2,H/2);return;}

  var pts=data.filter(function(p){var val=p[dataKey];return val!=null&&val!==undefined;});
  if(pts.length<2){ctx.fillStyle='#565f89';ctx.font='13px system-ui';ctx.textAlign='center';ctx.fillText('No '+dataKey+' data',W/2,H/2);return;}

  // X-axis labels
  var steps=Math.min(pts.length-1,6);
  ctx.fillStyle='#565f89';ctx.font='11px system-ui';ctx.textAlign='center';ctx.textBaseline='top';
  for(let i=0;i<steps;i++){
    var idx=Math.round(steps===1?0:i/(steps-1)*(pts.length-1));
    var x=P.l+(idx/(pts.length-1))*cW;
    ctx.fillText(formatTime(pts[idx].timestamp,range),x,H-P.b+6);
  }

  // Line
  ctx.beginPath();ctx.strokeStyle=color;ctx.lineWidth=2;
  for(let i=0;i<pts.length;i++){
    var x=P.l+(i/(pts.length-1))*cW;
    var yNorm=(pts[i][dataKey]-minVal)/(maxVal-minVal);
    var y=P.t+cH*(1-yNorm);
    if(i===0)ctx.moveTo(x,y);else ctx.lineTo(x,y);
  }
  ctx.stroke();

  // Fill
  ctx.lineTo(P.l+cW,P.t+cH);ctx.lineTo(P.l,P.t+cH);ctx.closePath();
  var grad=ctx.createLinearGradient(0,P.t,0,P.t+cH);
  grad.addColorStop(0,color.replace(')',',0.15)').replace('rgb','rgba'));
  grad.addColorStop(1,color.replace(')',',0)').replace('rgb','rgba'));
  ctx.fillStyle=grad;ctx.fill();

  // Hover
  canvas.onmousemove=function(ev){
    var r2=canvas.getBoundingClientRect();
    var mx=ev.clientX-r2.left,my=ev.clientY-r2.top;
    var tip=$e(canvasId.replace('Chart','TT'));
    if(mx<P.l||mx>W-P.r||my<P.t||my>P.t+cH){tip.style.display='none';return;}
    var idx=Math.round(((mx-P.l)/cW)*(pts.length-1));
    idx=Math.max(0,Math.min(idx,pts.length-1));
    tip.style.display='block';tip.style.left=(mx+10)+'px';tip.style.top=(my-10)+'px';
    tip.innerHTML='<div class="tt-label">'+formatTime(pts[idx].timestamp,range)+'</div>'+pts[idx][dataKey].toFixed(1)+(dataKey==='memory_used_mib'?' MiB':dataKey==='cpu_utilization_pct'?' %':' °C');
  };
  canvas.onmouseleave=function(){ $e(canvasId.replace('Chart','TT')).style.display='none';};
}

async function loadHistory(){
  try{
    var active=$e('rangeBar').querySelector('.active');
    var range=active?active.dataset.range:'15m';
    var r=await fetch('/api/history?range='+range);
    if(!r.ok)return;
    historyData=await r.json();
    var memVals=historyData.points.map(function(p){return p.memory_used_mib;}).filter(function(v){return v!=null;});
    var tempVals=historyData.points.map(function(p){return p.temperature_c;}).filter(function(v){return v!=null;});
    var memMin=memVals.length?Math.floor(Math.min.apply(null,memVals))*0.9:0;
    var memMax=memVals.length?Math.ceil(Math.max.apply(null,memVals)*1.1)||1024:1024;
    var tempMin=tempVals.length?Math.floor(Math.min.apply(null,tempVals)-5):30;
    var tempMax=tempVals.length?Math.ceil(Math.max.apply(null,tempVals)+5)||120:120;
    memMin=Math.max(0,memMin);tempMin=Math.max(0,tempMin);
    $e('memHint').textContent=memVals.length?memVals.length+' data points':'No data';
    $e('tempHint').textContent=tempVals.length?tempVals.length+' data points':'No data';
    drawChart('memChart','memory_used_mib','rgb(122,162,247)',range,memMin,memMax,historyData.points);
    drawChart('tempChart','temperature_c','rgb(247,118,142)',range,tempMin,tempMax,historyData.points);
  }catch(e){console.warn('history load:',e);}
}

async function loadSystemHistory(){
  try{
    var active=$e('sysRangeBar').querySelector('.active');
    var range=active?active.dataset.range:'15m';
    var r=await fetch('/api/sys-history?range='+range);
    if(!r.ok)return;
    sysHistoryData=await r.json();
    var sysMemVals=sysHistoryData.points.map(function(p){return p.memory_used_mib;}).filter(function(v){return v!=null;});
    var cpuVals=sysHistoryData.points.map(function(p){return p.cpu_utilization_pct;}).filter(function(v){return v!=null;});
    var sysMemMin=sysMemVals.length?Math.floor(Math.min.apply(null,sysMemVals))*0.9:0;
    var sysMemMax=sysMemVals.length?Math.ceil(Math.max.apply(null,sysMemVals)*1.1)||1024:1024;
    var cpuMin=cpuVals.length?Math.max(0,Math.floor(Math.min.apply(null,cpuVals)-5)):0;
    var cpuMax=cpuVals.length?Math.ceil(Math.max.apply(null,cpuVals)+5)||100:100;
    sysMemMin=Math.max(0,sysMemMin);
    $e('sysMemHint').textContent=sysMemVals.length?sysMemVals.length+' data points':'No data';
    $e('cpuHint').textContent=cpuVals.length?cpuVals.length+' data points':'No data';
    drawChart('sysMemChart','memory_used_mib','rgb(158,206,106)',range,sysMemMin,sysMemMax,sysHistoryData.points);
    drawChart('cpuChart','cpu_utilization_pct','rgb(224,175,104)',range,cpuMin,cpuMax,sysHistoryData.points);
  }catch(e){console.warn('sys history load:',e);}
}

var historyData=null;
var sysHistoryData=null;

/* ── Model switcher ── */
function esc(s){return String(s==null?'':s).replace(/[&<>"]/g,function(c){return({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;'})[c];});}

async function loadNodes(){
  try{
    var r=await fetch('/api/nodes');
    if(!r.ok)throw new Error('HTTP '+r.status);
    var d=await r.json();
    $e('resModel').textContent=d.resident_model||'None';
    $e('resNode').textContent=d.resident_node||'\u2014';
    $e('swVram').textContent=v(d.vram_used_mib)+' MiB';
    $e('swFree').textContent=v(d.vram_free_mib)+' MiB';
    var sw=d.switch||{};
    $e('swFloor').textContent=sw.vram_floor_mib?sw.vram_floor_mib+' MiB':'\u2014';
    var pe=$e('swPhase');
    pe.textContent=sw.phase||'\u2014';
    pe.className='value '+(sw.running?'warn':(sw.error?'err':'ok'));

    var switchable=d.switchable_nodes||[];
    var html='';
    (d.nodes||[]).forEach(function(n){
      var dot=n.ready?'var(--ok)':(n.unit_active?'var(--warn)':'var(--muted)');
      var state=n.ready?'ready':(n.unit_active?'loading':'stopped');
      var detail=[];
      if(n.model_alias)detail.push(esc(n.model_alias));
      if(n.n_ctx)detail.push('ctx '+n.n_ctx);
      if(n.total_slots)detail.push(n.total_slots+' slot'+(n.total_slots>1?'s':''));
      if(!n.agent_reachable)detail.push('<span class="err">agent unreachable</span>');
      else if(n.error)detail.push('<span class="warn">'+esc(n.error)+'</span>');
      var canSwitch=switchable.indexOf(n.name)>=0&&!n.ready&&!sw.running;
      html+='<div class="metric"><span class="label"><span class="dot" style="background:'+dot+'"></span>'+esc(n.name)
        +' <span style="color:var(--muted)">('+esc(n.kind)+')</span></span>'
        +'<span class="value">'+state
        +(detail.length?' &middot; '+detail.join(' &middot; '):'')
        +(n.ready?'<span class="sw-resident">resident</span>':'')
        +(canSwitch?'<button class="sw-btn" data-node="'+esc(n.name)+'">Switch to</button>':'')
        +'</span></div>';
    });
    $e('nodeList').innerHTML=html||'<div class="metric"><span class="label">No nodes configured</span></div>';
    Array.prototype.forEach.call(document.querySelectorAll('.sw-btn'),function(b){
      b.onclick=function(){switchTo(b.getAttribute('data-node'));};
    });

    var m=$e('swMsg');
    if(sw.error){m.innerHTML='<span class="err">Switch failed: '+esc(sw.error)+'</span>';}
    else if(sw.running){m.textContent='Switching to '+sw.target+'\u2026 phase '+sw.phase+', '+sw.elapsed_secs+'s, VRAM '+v(sw.vram_used_mib)+' MiB';}
    else if(sw.outcome){m.innerHTML='<span class="ok">Last switch to '+esc(sw.outcome.target)+' ('+esc(sw.outcome.target_model||'?')+') ok in '+sw.outcome.total_secs+'s</span> release '+sw.outcome.release_secs+'s, ready '+sw.outcome.ready_secs+'s, stopped ['+(sw.outcome.stopped_nodes||[]).map(esc).join(', ')+']';}
    else{m.textContent='';}
    if(sw.running){setTimeout(loadNodes,3000);}
  }catch(e){$e('swMsg').textContent='nodes error: '+e.message;}
}

async function switchTo(name){
  if(!confirm('Switch the GPU to '+name+'?\n\nThis stops the resident workload and loads '+name+'. It takes roughly 1-3 minutes and interrupts any in-flight generation.'))return;
  var msg=$e('swMsg');
  msg.textContent='Requesting switch to '+name+'\u2026';
  var hdr={'Content-Type':'application/json'};
  var tok=localStorage.getItem('monitor_api_token');
  if(tok)hdr['Authorization']='Bearer '+tok;
  try{
    var r=await fetch('/api/model',{method:'POST',headers:hdr,body:JSON.stringify({target:name})});
    var d={};
    try{d=await r.json();}catch(_){}
    if(r.status===401){
      var t=prompt('This monitor requires an API token to switch models. Enter it:');
      if(t){localStorage.setItem('monitor_api_token',t);return switchTo(name);}
      msg.innerHTML='<span class="err">Token required.</span>';
      return;
    }
    if(!r.ok){msg.innerHTML='<span class="err">HTTP '+r.status+': '+esc(d.error||'request failed')+'</span>';return;}
    msg.textContent='Switch to '+name+' accepted; polling progress\u2026';
    loadNodes();
  }catch(e){msg.textContent='switch error: '+e.message;}
}

// Range bar buttons
function setupRangeBar(barId,loadFn){
  document.querySelectorAll('#'+barId+' button').forEach(function(btn){
    btn.addEventListener('click',function(){
      document.querySelectorAll('#'+barId+' button').forEach(function(b){b.classList.remove('active');});
      btn.classList.add('active');
      loadFn();
    });
  });
}
setupRangeBar('rangeBar',loadHistory);
setupRangeBar('sysRangeBar',loadSystemHistory);

// Init
refresh();loadNodes();loadHistory();loadSystemHistory();
setInterval(refresh, REFRESH_INTERVAL_MS_PLACEHOLDER);
setInterval(loadNodes, REFRESH_INTERVAL_MS_PLACEHOLDER);
setInterval(loadHistory, REFRESH_INTERVAL_MS_PLACEHOLDER * 2);
setInterval(loadSystemHistory, REFRESH_INTERVAL_MS_PLACEHOLDER * 2);
window.addEventListener('resize',function(){loadHistory();loadSystemHistory();});
</script></body></html>"#;
