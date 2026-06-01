/// Axum REST API server + embedded web dashboard.
use anyhow::Result;
use axum::{extract::State, http::StatusCode, response::Html, routing::get, Router};
use std::{net::SocketAddr, sync::Arc, time::Duration};
use tokio::sync::RwLock;
use tracing::info as log_info;

use crate::config::Config;
use crate::gpu::try_query_gpu;
use crate::models::{ApiGpuResponse, ApiModelResponse, MonitorStatus};
use crate::ollama::OllamaClient;

fn now_ts() -> String {
    use std::time::SystemTime;
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or("unknown".into(), |d| {
            let s = d.as_secs();
            format!("{} days {:02}:{:02}", s / 86400, (s % 3600) / 60, s % 60)
        })
}

#[derive(Default, Clone)]
pub struct AppState {
    pub latest_status: Arc<RwLock<Option<MonitorStatus>>>,
}

// ── Background refresh loop ──────────────────────────────

pub async fn run_refresh_loop(config: &Config, state: AppState) {
    let client = OllamaClient::new(config.ollama_base_url());
    let interval = Duration::from_secs(config.refresh_interval_secs);

    loop {
        tokio::time::sleep(interval).await;

        let tags_resp = client.try_fetch_models().await;
        let available: Vec<String> = match &tags_resp {
            Some(tags) => tags.models.iter().map(|m| m.model_name.clone()).collect(),
            None => vec![],
        };
        let loaded_model = tags_resp
            .as_ref()
            .and_then(|t| t.models.first().cloned().map(|m| m.model_name));

        let gpu_metric = try_query_gpu(config.gpu_device_index);

        let status = MonitorStatus {
            ollama_url: config.ollama_base_url(),
            ollama_reachable: tags_resp.is_some(),
            loaded_model,
            available_models: available.clone(),
            gpu: gpu_metric,
            timestamp: now_ts(),
        };

        let mut latest = state.latest_status.write().await;
        *latest = Some(status);
    }
}

// ── HTTP Handlers ────────────────────────────────────────

async fn handle_dashboard() -> Html<&'static str> {
    Html(DASHBOARD_HTML)
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

// ── Router builder ───────────────────────────────────────

pub async fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/", get(handle_dashboard))
        .route("/api/status", get(handle_api_status))
        .route("/api/gpu", get(handle_api_gpu))
        .route("/api/models", get(handle_api_models))
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

const DASHBOARD_HTML: &'static str = r#"<!DOCTYPE html>
<html lang="en"><head><meta charset="utf-8"/><meta name="viewport" content="width=device-width,initial-scale=1"/>
<title>Ollama Monitor</title>
<style>
:root{--bg:#1a1b26;--card:#24283b;--text:#c0caf5;--accent:#7aa2f7;--ok:#9ece6a;--warn:#e0af68;--err:#f7768e;--muted:#565f89}
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
</div><p id="ts">Loading…</p>
<script>
async function refresh(){
  try{
    const r=await fetch('/api/status');if(!r.ok)throw new Error('HTTP '+r.status);
    const d=await r.json();
    $e('url').textContent=d.ollama_url||'—';
    var e=$e('reach');e.innerHTML=d.ollama_reachable?'<span class="dot" style="background:var(--ok)"></span>Yes':'<span class="dot" style="background:var(--err)"></span>No';
    $e('loaded').textContent=d.loaded_model||'None';
    $e('avail').textContent=d.available_models.length+' model(s)';
    $e('mu').textContent=v(d.gpu.memory_used_mib)+' MiB';$e('mt').textContent=v(d.gpu.memory_total_mib)+' MiB';$e('mr').textContent=v(d.gpu.memory_remaining_mib)+' MiB';
    if(d.gpu.memory_total_mib>0){var p=Math.round(d.gpu.memory_used_mib/d.gpu.memory_total_mib*100);var b=$e('mb');b.style.width=p+'%';b.style.background=p>90?'var(--err)':p>70?'var(--warn)':'var(--ok)';}
    $e('gn').textContent=d.gpu.name||'—';
    e=$e('gp');e.textContent=(d.gpu.temperature_c!=null)?d.gpu.temperature_c.toFixed(1)+' °C':'—';e.className='value '+(d.gpu.temperature_c>80?'err':d.gpu.temperature_c>65?'warn':'ok');
    $e('gu').textContent=v(d.gpu.utilization_pct)+'%';$e('gpw').textContent=v(d.gpu.power_watts)+' W';
    $e('ts').textContent='Last updated: '+new Date().toLocaleTimeString();
  }catch(e){$e('ts').textContent='Error: '+e.message;}
}function $e(i){return document.getElementById(i);}function v(x){return x!=null?Math.round(x):'—';}
refresh();setInterval(refresh,15000);</script></body></html>"#;
