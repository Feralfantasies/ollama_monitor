//! Client for llama.cpp's `llama-server` OpenAI-compatible HTTP API.
//!
//! # Why this exists separately from [`crate::ollama`]
//!
//! `llama-server` exposes an OpenAI-compatible surface, **not** Ollama's. Verified
//! against a live b11583 server: `/v1/models` and `/props` return 200, while
//! `/api/tags` and `/api/ps` return **404**. The existing [`crate::ollama`] client
//! polls `/api/tags`, so it cannot be pointed at llama.cpp; it is deliberately left
//! untouched so Ollama deployments keep working.
//!
//! # The readiness rule (load-bearing for VRAM safety)
//!
//! `llama-server` answers `GET /props` with **503 while a model is still loading**
//! and 200 only once it is serving. Treating "process is up" or "TCP connects" as
//! ready is what causes a CUDA OOM: the switcher would start a second model before
//! the first released VRAM. Therefore [`LlamaProbe::ready`] is true **only** on a
//! 200 response, and callers must not infer readiness from anything weaker.
//!
//! Measured on the V100 host: a 16.4 GB Q4_K_M model takes ~66 s and a 14.3 GB one
//! ~76–148 s to reach 200, so polling must tolerate minutes, not seconds.
//!
//! # Response shapes
//!
//! Fixtures in `tests/fixtures/` are real captures from the live servers
//! (`llama01` / Qwen3.8-27B). Two quirks are handled explicitly:
//!
//! * `/v1/models` returns **both** an Ollama-compat `models[]` array and an
//!   OpenAI `data[]` array. `data[].id` is authoritative; `models[]` is parsed too
//!   so the field is not silently ignored if `data` is absent in another build.
//! * `model_alias` appears at the top level of `/props` and may also appear inside
//!   `default_generation_settings`. Top level wins, nested is the fallback.
//!
//! Every field is `Option`/`#[serde(default)]` because llama.cpp's JSON has shifted
//! between builds and an unknown or missing key must not break monitoring.

use anyhow::{Context, Result};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::time::Duration;
use tracing::{debug, warn};

/// Nested `default_generation_settings` object from `/props`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DefaultGenerationSettings {
    /// Per-slot context, i.e. what a single session actually gets.
    ///
    /// llama.cpp's `--ctx-size` is the TOTAL shared across `--parallel` slots, so
    /// `n_ctx` here is `ctx_size / parallel`. Reporting the total as if it were
    /// per-session would badly misrepresent usable context.
    #[serde(default)]
    pub n_ctx: Option<u64>,
    #[serde(default)]
    pub model_alias: Option<String>,
}

/// Parsed `GET /props`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LlamaProps {
    #[serde(default)]
    pub model_alias: Option<String>,
    #[serde(default)]
    pub model_path: Option<String>,
    #[serde(default)]
    pub model_ftype: Option<String>,
    /// Number of concurrent slots (`--parallel`).
    #[serde(default)]
    pub total_slots: Option<u64>,
    #[serde(default)]
    pub build_info: Option<String>,
    #[serde(default)]
    pub chat_format: Option<String>,
    #[serde(default)]
    pub reasoning_format: Option<String>,
    #[serde(default)]
    pub is_sleeping: Option<bool>,
    #[serde(default)]
    pub default_generation_settings: Option<DefaultGenerationSettings>,
}

impl LlamaProps {
    /// Resolve the served model alias, preferring the top-level field.
    pub fn alias(&self) -> Option<String> {
        self.model_alias.clone().or_else(|| {
            self.default_generation_settings
                .as_ref()
                .and_then(|d| d.model_alias.clone())
        })
    }

    /// Resolve per-slot context, preferring the nested field.
    pub fn n_ctx(&self) -> Option<u64> {
        self.default_generation_settings
            .as_ref()
            .and_then(|d| d.n_ctx)
    }
}

/// `meta` block of an entry in `/v1/models` `data[]`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LlamaModelMeta {
    #[serde(default)]
    pub n_ctx: Option<u64>,
    #[serde(default)]
    pub n_ctx_train: Option<u64>,
    #[serde(default)]
    pub n_embd: Option<u64>,
    #[serde(default)]
    pub n_vocab: Option<u64>,
    #[serde(default)]
    pub n_params: Option<u64>,
    /// On-disk size of the loaded GGUF in bytes.
    #[serde(default)]
    pub size: Option<u64>,
    #[serde(default)]
    pub ftype: Option<String>,
}

/// One entry of `/v1/models` `data[]` (OpenAI shape).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LlamaModelEntry {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub aliases: Vec<String>,
    #[serde(default)]
    pub object: Option<String>,
    #[serde(default)]
    pub owned_by: Option<String>,
    #[serde(default)]
    pub created: Option<u64>,
    #[serde(default)]
    pub meta: Option<LlamaModelMeta>,
}

/// One entry of `/v1/models` `models[]` (Ollama-compat shape emitted by llama.cpp).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LlamaCompatModelEntry {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub model: Option<String>,
}

/// Parsed `GET /v1/models`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LlamaModels {
    /// OpenAI-compatible list; authoritative.
    #[serde(default)]
    pub data: Vec<LlamaModelEntry>,
    /// Ollama-compatible list also emitted by llama-server.
    #[serde(default)]
    pub models: Vec<LlamaCompatModelEntry>,
    #[serde(default)]
    pub object: Option<String>,
}

impl LlamaModels {
    /// Model identifiers, preferring `data[].id` and falling back to `models[].name`.
    ///
    /// De-duplicated and order-preserving so a server emitting both arrays does not
    /// report the same model twice.
    pub fn ids(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for e in &self.data {
            if !e.id.is_empty() && !out.contains(&e.id) {
                out.push(e.id.clone());
            }
        }
        if out.is_empty() {
            for m in &self.models {
                if !m.name.is_empty() && !out.contains(&m.name) {
                    out.push(m.name.clone());
                }
            }
        }
        out
    }
}

/// Aggregate, non-throwing view of one llama-server endpoint.
///
/// This is what the switcher and dashboard consume: it distinguishes "server down",
/// "server up but still loading" and "model ready" without the caller having to
/// reason about HTTP status codes.
#[derive(Debug, Clone, Default, Serialize)]
pub struct LlamaProbe {
    /// True ONLY when `/props` returned 200 (model loaded and serving).
    pub ready: bool,
    /// True when anything answered TCP/HTTP at all (including 503 mid-load).
    pub reachable: bool,
    pub http_status: Option<u16>,
    pub alias: Option<String>,
    /// Per-slot context.
    pub n_ctx: Option<u64>,
    pub total_slots: Option<u64>,
    pub model_path: Option<String>,
    pub model_ftype: Option<String>,
    pub build_info: Option<String>,
    pub available_models: Vec<String>,
    pub error: Option<String>,
}

pub struct LlamaCppClient {
    base_url: String,
    http: Client,
}

impl LlamaCppClient {
    /// `base_url` is the server root, e.g. `http://192.0.2.10:8000` (no `/v1`).
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into(),
            http: Client::builder()
                // Short: this is polled on a refresh loop and must not block it.
                // Model LOAD waiting is the switcher's job, with its own longer budget.
                .timeout(Duration::from_secs(10))
                .build()
                .expect("reqwest client build"),
        }
    }

    /// Fetch `/v1/models`.
    pub async fn fetch_models(&self) -> Result<LlamaModels> {
        let url = format!("{}/v1/models", self.base_url);
        debug!("fetching llama.cpp models from {}", url);
        let resp = self
            .http
            .get(&url)
            .send()
            .await
            .with_context(|| format!("failed to connect to llama-server at {}", url))?;
        let status = resp.status();
        if !status.is_success() {
            anyhow::bail!("llama-server {} returned HTTP {status}", url);
        }
        resp.json::<LlamaModels>()
            .await
            .with_context(|| format!("failed to parse llama-server /v1/models from {}", url))
    }

    /// Best-effort probe that never fails: classifies the endpoint for callers.
    ///
    /// `/props` decides readiness. `/v1/models` is fetched only once props is 200,
    /// because it is likewise 503 during load and would just duplicate the error.
    pub async fn probe(&self) -> LlamaProbe {
        let url = format!("{}/props", self.base_url);
        let resp = match self.http.get(&url).send().await {
            Ok(r) => r,
            Err(e) => {
                return LlamaProbe {
                    ready: false,
                    reachable: false,
                    http_status: None,
                    error: Some(format!("unreachable: {e}")),
                    ..Default::default()
                }
            }
        };

        let status = resp.status();
        let reachable = true;
        if !status.is_success() {
            // 503 here means "loading", which is NOT ready. Surfacing the code lets
            // the dashboard say "loading…" instead of "down".
            return LlamaProbe {
                ready: false,
                reachable,
                http_status: Some(status.as_u16()),
                error: Some(format!("HTTP {status} (model not loaded)")),
                ..Default::default()
            };
        }

        let props: LlamaProps = match resp.json().await {
            Ok(p) => p,
            Err(e) => {
                return LlamaProbe {
                    ready: false,
                    reachable,
                    http_status: Some(status.as_u16()),
                    error: Some(format!("props parse error: {e}")),
                    ..Default::default()
                }
            }
        };

        let available_models = match self.fetch_models().await {
            Ok(m) => m.ids(),
            Err(e) => {
                // Props said ready, so keep ready=true; model list is a nice-to-have.
                warn!(
                    "llama-server {} ready but /v1/models failed: {}",
                    self.base_url, e
                );
                Vec::new()
            }
        };

        LlamaProbe {
            ready: true,
            reachable,
            http_status: Some(status.as_u16()),
            alias: props.alias(),
            n_ctx: props.n_ctx(),
            total_slots: props.total_slots,
            model_path: props.model_path.clone(),
            model_ftype: props.model_ftype.clone(),
            build_info: props.build_info.clone(),
            available_models,
            error: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Real capture from llama01 (Qwen3.8-27B, llama.cpp b11583).
    const PROPS_QWEN: &str = include_str!("../tests/fixtures/llamacpp_props_qwen.json");
    /// Real capture from llama01 `/v1/models`.
    const MODELS_QWEN: &str = include_str!("../tests/fixtures/llamacpp_v1models_qwen.json");
    /// Real capture from llama02 (Precog-24B-v1b Q4_K_M, FA + q8_0 KV, 2 slots).
    const PROPS_PRECOG: &str = include_str!("../tests/fixtures/llamacpp_props_precog.json");
    /// Real capture from llama02 `/v1/models`.
    const MODELS_PRECOG: &str = include_str!("../tests/fixtures/llamacpp_v1models_precog.json");

    /// A second real server with a different model AND slot count. Guards against
    /// the parser being accidentally fitted to one deployment's shape.
    #[test]
    fn parses_real_precog_props_capture() {
        let p: LlamaProps = serde_json::from_str(PROPS_PRECOG).expect("precog props must parse");
        assert_eq!(p.alias().as_deref(), Some("Precog-24B"));
        assert_eq!(p.n_ctx(), Some(61440));
        // llama02 runs --parallel 2 vs llama01's 3, but per-slot context is
        // identical — exactly what the owner's context policy required.
        assert_eq!(p.total_slots, Some(2));
        assert_eq!(p.model_ftype.as_deref(), Some("Q4_K - Medium"));
        assert_eq!(
            p.model_path.as_deref(),
            Some("/models/Precog-24B/Precog-24B-v1b-Q4_K_M.gguf")
        );
        assert_eq!(p.build_info.as_deref(), Some("b11583-988015788"));
    }

    #[test]
    fn parses_real_precog_v1_models_capture() {
        let m: LlamaModels =
            serde_json::from_str(MODELS_PRECOG).expect("precog models fixture must parse");
        assert_eq!(m.ids(), vec!["Precog-24B".to_string()]);

        let meta = m.data[0].meta.as_ref().expect("meta present");
        assert_eq!(meta.n_ctx, Some(61440));
        assert_eq!(meta.n_ctx_train, Some(131072));
        assert_eq!(meta.n_vocab, Some(131072));
        assert_eq!(meta.n_params, Some(23_572_403_200));
        assert_eq!(meta.ftype.as_deref(), Some("Q4_K - Medium"));
        // The server reports the LOADED tensor size (14,326,026,240), which is
        // slightly less than the GGUF file on disk (14,333,913,536). Encoding the
        // distinction stops anyone "fixing" this to the file size later.
        assert_eq!(meta.size, Some(14_326_026_240));
    }

    #[test]
    fn two_servers_are_distinguishable() {
        // The switcher keys decisions off the alias, so the two deployments must
        // never be confused with one another.
        let q: LlamaProps = serde_json::from_str(PROPS_QWEN).unwrap();
        let p: LlamaProps = serde_json::from_str(PROPS_PRECOG).unwrap();
        assert_ne!(q.alias(), p.alias());
        assert_ne!(q.total_slots, p.total_slots);
        assert_ne!(q.model_path, p.model_path);
        assert_eq!(
            q.n_ctx(),
            p.n_ctx(),
            "per-session context is deliberately equal across both servers"
        );
    }

    #[test]
    fn parses_real_props_capture() {
        let p: LlamaProps = serde_json::from_str(PROPS_QWEN).expect("props fixture must parse");
        assert_eq!(p.alias().as_deref(), Some("Qwen3.8-27B"));
        // per-slot context, NOT the 184320 total
        assert_eq!(p.n_ctx(), Some(61440));
        assert_eq!(p.total_slots, Some(3));
        assert_eq!(p.model_ftype.as_deref(), Some("Q4_K - Medium"));
        assert_eq!(
            p.model_path.as_deref(),
            Some("/models/Qwen3.8-27B/Qwen3.8-27B-UD-Q4_K_M.gguf")
        );
        assert_eq!(p.build_info.as_deref(), Some("b11583-988015788"));
    }

    #[test]
    fn parses_real_v1_models_capture() {
        let m: LlamaModels = serde_json::from_str(MODELS_QWEN).expect("models fixture must parse");
        // both arrays are present in the real response
        assert!(!m.data.is_empty(), "data[] must be populated");
        assert!(!m.models.is_empty(), "models[] must be populated");
        assert_eq!(m.ids(), vec!["Qwen3.8-27B".to_string()]);

        let meta = m.data[0].meta.as_ref().expect("meta present");
        assert_eq!(meta.n_ctx, Some(61440));
        assert_eq!(meta.n_ctx_train, Some(262144));
        assert_eq!(meta.n_params, Some(27_320_697_856));
        assert_eq!(meta.size, Some(16_453_443_584));
        assert_eq!(meta.ftype.as_deref(), Some("Q4_K - Medium"));
    }

    #[test]
    fn ids_are_deduplicated_across_both_arrays() {
        // llama-server emits the same model in data[] AND models[]; a naive
        // concatenation would report it twice.
        let m: LlamaModels = serde_json::from_str(MODELS_QWEN).unwrap();
        assert_eq!(m.data.len(), 1);
        assert_eq!(m.models.len(), 1);
        assert_eq!(m.ids().len(), 1, "must not double-count");
    }

    #[test]
    fn ids_fall_back_to_compat_array_when_data_absent() {
        let m: LlamaModels =
            serde_json::from_str(r#"{"object":"list","models":[{"name":"fallback-model"}]}"#)
                .expect("compat-only response must parse");
        assert!(m.data.is_empty());
        assert_eq!(m.ids(), vec!["fallback-model".to_string()]);
    }

    #[test]
    fn alias_prefers_top_level_then_nested() {
        let top: LlamaProps = serde_json::from_str(
            r#"{"model_alias":"top","default_generation_settings":{"model_alias":"nested","n_ctx":4096}}"#,
        )
        .unwrap();
        assert_eq!(top.alias().as_deref(), Some("top"));
        assert_eq!(top.n_ctx(), Some(4096));

        let nested: LlamaProps =
            serde_json::from_str(r#"{"default_generation_settings":{"model_alias":"nested"}}"#)
                .unwrap();
        assert_eq!(nested.alias().as_deref(), Some("nested"));
        assert_eq!(nested.n_ctx(), None);
    }

    #[test]
    fn tolerates_unknown_and_missing_fields() {
        // llama.cpp's JSON shifts between builds; unknown keys must not break us,
        // and a nearly-empty object must not panic.
        let sparse: LlamaProps = serde_json::from_str(r#"{"brand_new_field":123}"#)
            .expect("unknown fields must be ignored");
        assert_eq!(sparse.alias(), None);
        assert_eq!(sparse.n_ctx(), None);
        assert_eq!(sparse.total_slots, None);

        let empty: LlamaModels =
            serde_json::from_str("{}").expect("empty models object must parse");
        assert!(empty.ids().is_empty());
    }

    #[test]
    fn probe_defaults_are_not_ready() {
        // A default/failed probe must never claim readiness — that is the
        // OOM-avoidance invariant the switcher relies on.
        let p = LlamaProbe::default();
        assert!(!p.ready);
        assert!(!p.reachable);
        assert!(p.alias.is_none());
        assert!(p.available_models.is_empty());
    }

    #[tokio::test]
    async fn probe_of_unreachable_endpoint_is_not_ready() {
        // Nothing listens on this port; probe must classify it, not panic or hang.
        let c = LlamaCppClient::new("http://127.0.0.1:1");
        let p = c.probe().await;
        assert!(!p.ready, "unreachable endpoint must not be ready");
        assert!(!p.reachable);
        assert!(p.error.is_some());
    }

    /// Live end-to-end parse against a real llama-server.
    ///
    /// Ignored by default because it needs a reachable server; the fixture tests
    /// above are the hermetic CI gate. Run explicitly with:
    ///
    /// ```text
    /// LIVE_LLAMA_URL=http://192.0.2.10:8000 \
    ///   cargo test --bin ollama_monitor live_probe -- --ignored --nocapture
    /// ```
    ///
    /// This must live inside the crate (not `tests/`) because the crate is a
    /// binary with no lib target, so an integration test cannot import
    /// `crate::llamacpp`.
    #[tokio::test]
    #[ignore = "requires a live llama-server; set LIVE_LLAMA_URL"]
    async fn live_probe_against_real_server() {
        let url = std::env::var("LIVE_LLAMA_URL")
            .expect("LIVE_LLAMA_URL must be set, e.g. http://192.0.2.10:8000");
        let c = LlamaCppClient::new(&url);
        let p = c.probe().await;
        println!("live probe {url} => ready={} reachable={} status={:?} alias={:?} n_ctx={:?} slots={:?} ftype={:?} build={:?} models={:?}",
            p.ready, p.reachable, p.http_status, p.alias, p.n_ctx, p.total_slots,
            p.model_ftype, p.build_info, p.available_models);
        assert!(
            p.ready,
            "expected a loaded, serving model at {url}: {:?}",
            p.error
        );
        assert!(p.reachable);
        assert_eq!(p.http_status, Some(200));
        assert!(p.alias.is_some(), "alias must be parsed from /props");
        assert!(p.n_ctx.is_some(), "per-slot n_ctx must be parsed");
        assert!(p.total_slots.unwrap_or(0) >= 1, "total_slots must be >= 1");
        assert!(
            !p.available_models.is_empty(),
            "/v1/models must yield at least one id"
        );
        assert!(p.error.is_none());
    }
}
