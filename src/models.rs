use serde::{Deserialize, Serialize};

// ── Ollama API response types ───────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OllamaTagsResponse {
    pub models: Vec<OllamaModelEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OllamaModelEntry {
    #[serde(rename = "name")]
    pub model_name: String,
    #[serde(rename = "size")]
    pub size_bytes: Option<u64>,
    #[serde(rename = "digest")]
    pub digest: Option<String>,
    #[serde(rename = "modified_at")]
    pub modified_at: Option<String>,
}

// ── Aggregated status (exposed by REST API) ─────────────────────

#[derive(Debug, Clone, Serialize)]
pub struct MonitorStatus {
    pub ollama_url: String,
    pub ollama_reachable: bool,
    pub loaded_model: Option<String>,
    pub available_models: Vec<String>,
    pub gpu: GpuMetric,
    pub system: SystemMetric,
    pub timestamp: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct GpuMetric {
    pub name: Option<String>,
    pub temperature_c: Option<f64>,
    pub memory_used_mib: Option<u64>,
    pub memory_total_mib: Option<u64>,
    pub memory_remaining_mib: Option<u64>,
    pub utilization_pct: Option<f64>,
    pub power_watts: Option<f64>,
}

impl GpuMetric {
    pub fn placeholder() -> Self {
        Self {
            name: None,
            temperature_c: None,
            memory_used_mib: None,
            memory_total_mib: None,
            memory_remaining_mib: None,
            utilization_pct: None,
            power_watts: None,
        }
    }
}

// ── System metrics ───────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
pub struct SystemMetric {
    pub memory_used_mib: Option<u64>,
    pub memory_total_mib: Option<u64>,
    pub memory_remaining_mib: Option<u64>,
    pub memory_usage_pct: Option<f64>,
    pub cpu_utilization_pct: Option<f64>,
}

impl SystemMetric {
    pub fn placeholder() -> Self {
        Self {
            memory_used_mib: None,
            memory_total_mib: None,
            memory_remaining_mib: None,
            memory_usage_pct: None,
            cpu_utilization_pct: None,
        }
    }
}

// ── Web API response types (subset views) ──────────────────────

#[derive(Debug, Clone, Serialize)]
pub struct ApiModelResponse {
    pub loaded_model: Option<String>,
    pub available_models: Vec<String>,
    pub total_count: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct ApiGpuResponse {
    pub gpu: GpuMetric,
    pub timestamp: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ApiSystemResponse {
    pub system: SystemMetric,
    pub timestamp: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct GpuHistoryPoint {
    pub timestamp: i64,
    pub memory_used_mib: Option<u64>,
    pub temperature_c: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ApiHistoryResponse {
    pub points: Vec<GpuHistoryPoint>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SystemHistoryPoint {
    pub timestamp: i64,
    pub memory_used_mib: Option<u64>,
    pub cpu_utilization_pct: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ApiSystemHistoryResponse {
    pub points: Vec<SystemHistoryPoint>,
}

// ── Database record for each check result ──────────────────

#[derive(Debug, Clone)]
pub struct CheckResult {
    pub ollama_url: String,
    pub ollama_reachable: bool,
    pub loaded_model: Option<String>,
    pub available_models_count: usize,
    pub gpu_name: Option<String>,
    pub gpu_temperature_c: Option<f64>,
    pub gpu_memory_used_mib: Option<u64>,
    pub gpu_memory_total_mib: Option<u64>,
    pub gpu_utilization_pct: Option<f64>,
    pub gpu_power_watts: Option<f64>,
    pub sys_memory_used_mib: Option<u64>,
    pub sys_memory_total_mib: Option<u64>,
    pub sys_memory_remaining_mib: Option<u64>,
    pub sys_memory_usage_pct: Option<f64>,
    pub sys_cpu_utilization_pct: Option<f64>,
}
