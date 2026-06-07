/// Axum REST API server + embedded web dashboard.
use anyhow::Result;
use axum::{extract::State, http::StatusCode, response::Html, routing::get, Router};
use std::{collections::HashMap, net::SocketAddr, sync::Arc, time::Duration};
use tokio::sync::RwLock;
use tracing::info as log_info;

use crate::config::Config;
use crate::db::{self, HistoryRange};
use crate::models::{
    ApiGpuResponse, ApiHistoryResponse, ApiModelResponse, CheckResult, GpuHistoryPoint, GpuMetric,
    MonitorStatus,
};
use crate::ollama::OllamaClient;
use sqlx::SqlitePool;

fn now_ts() -> String {
    use std::time::SystemTime;
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or("unknown".into(), |d| {
            let s = d.as_secs();
            format!("{} days {:02}:{:02}", s / 86400, (s % 3600) / 60, s % 60)
        })
}

#[derive(Clone)]
pub struct AppState {
    pub latest_status: Arc<RwLock<Option<MonitorStatus>>>,
    pub db_pool: SqlitePool,
}

impl AppState {
    pub fn new(db_pool: SqlitePool) -> Self {
        Self {
            latest_status: Arc::new(RwLock::new(None)),
            db_pool,
        }
    }
}

// ── Background refresh loop ──────────────────────────────

/// Run a single iteration of the refresh loop, updating state and persisting to DB.
pub async fn run_one_refresh<G: Fn(usize) -> GpuMetric>(
    config: &Config,
    state: &AppState,
    gpu_fn: &G,
) {
    let client = OllamaClient::new(config.ollama_base_url());

    let tags_resp = client.try_fetch_models().await;
    let available: Vec<String> = match &tags_resp {
        Some(tags) => tags.models.iter().map(|m| m.model_name.clone()).collect(),
        None => vec![],
    };
    let loaded_model = tags_resp
        .as_ref()
        .and_then(|t| t.models.first().cloned().map(|m| m.model_name));

    let gpu_metric = gpu_fn(config.gpu_device_index);

    let status = MonitorStatus {
        ollama_url: config.ollama_base_url(),
        ollama_reachable: tags_resp.is_some(),
        loaded_model: loaded_model.clone(),
        available_models: available.clone(),
        gpu: gpu_metric.clone(),
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

// ── Router builder ───────────────────────────────────────

pub async fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/", get(handle_dashboard))
        .route("/api/status", get(handle_api_status))
        .route("/api/gpu", get(handle_api_gpu))
        .route("/api/models", get(handle_api_models))
        .route("/api/history", get(handle_api_history))
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
:root{--bg:#1a1b26;--card:#24283b;--text:#c0caf5;--accent:#7aa2f7;--ok:#9ece6a;--warn:#e0af68;--err:#f7768e;--muted:#565f89;--mem:#7aa2f7;--temp:#f7768e}
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
    $e('mu').textContent=v(d.gpu.memory_used_mib)+' MiB';$e('mt').textContent=v(d.gpu.memory_total_mib)+' MiB';$e('mr').textContent=v(d.gpu.memory_remaining_mib)+' MiB';
    if(d.gpu.memory_total_mib>0){var p=Math.round(d.gpu.memory_used_mib/d.gpu.memory_total_mib*100);var b=$e('mb');b.style.width=p+'%';b.style.background=p>90?'var(--err)':p>70?'var(--warn)':'var(--ok)';}
    $e('gn').textContent=d.gpu.name||'—';
    e=$e('gp');e.textContent=(d.gpu.temperature_c!=null)?d.gpu.temperature_c.toFixed(1)+' °C':'—';e.className='value '+(d.gpu.temperature_c>80?'err':d.gpu.temperature_c>65?'warn':'ok');
    $e('gu').textContent=v(d.gpu.utilization_pct)+'%';$e('gpw').textContent=v(d.gpu.power_watts)+' W';
    $e('ts').textContent='Last updated: '+new Date().toLocaleTimeString();
  }catch(e){$e('ts').textContent='Error: '+e.message;}
}

/* ── Chart helpers ── */
function formatTime(ts,range){
  var d=new Date(ts);
  if(range==='15m'||range==='1h'||range==='6h')return d.getHours().toString().padStart(2,'0')+':'+d.getMinutes().toString().padStart(2,'0');
  return(d.getMonth()+1)+'/'+d.getDate()+' '+d.getHours().toString().padStart(2,'0')+':'+d.getMinutes().toString().padStart(2,'0');
}

function drawChart(canvasId,dataKey,color,range,minVal,maxVal){
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

  if(!historyData||historyData.points.length<2){ctx.fillStyle='#565f89';ctx.font='13px system-ui';ctx.textAlign='center';ctx.fillText('No history data yet',W/2,H/2);return;}

  var pts=historyData.points.filter(function(p){var val=p[dataKey];return val!=null&&val!==undefined;});
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
    tip.innerHTML='<div class="tt-label">'+formatTime(pts[idx].timestamp,range)+'</div>'+pts[idx][dataKey].toFixed(1)+(dataKey==='memory_used_mib'?' MiB':' °C');
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
    drawChart('memChart','memory_used_mib','rgb(122,162,247)',range,memMin,memMax);
    drawChart('tempChart','temperature_c','rgb(247,118,142)',range,tempMin,tempMax);
  }catch(e){console.warn('history load:',e);}
}

var historyData=null;

// Range bar buttons
document.querySelectorAll('#rangeBar button').forEach(function(btn){
  btn.addEventListener('click',function(){
    document.querySelectorAll('#rangeBar button').forEach(function(b){b.classList.remove('active');});
    btn.classList.add('active');
    loadHistory();
  });
});

// Init
refresh();loadHistory();
setInterval(function(){refresh();loadHistory();},30000);
window.addEventListener('resize',loadHistory);
</script></body></html>"#;
