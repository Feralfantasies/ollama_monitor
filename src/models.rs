/// Shared data types for Ollama and GPU state.
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
