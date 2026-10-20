/// Application-wide configuration loaded from environment variables with sensible defaults.
///
/// # Base settings
///
/// | Variable                  | Default               | Description                        |
/// |---------------------------|-----------------------|------------------------------------|
/// | `OLLAMA_HOST`             | `http://127.0.0.1`      | Ollama server base URL (append `:port` explicitly, or omit to use `OLLAMA_PORT` default) |
/// | `OLLAMA_PORT`             | `11434`               | Ollama server port                 |
/// | `SERVER_BIND`             | `0.0.0.0`             | Address to bind the API server to  |
/// | `SERVER_PORT`             | `3000`                | Port for the API server            |
/// | `REFRESH_INTERVAL_SECS`   | `15`                  | Seconds between status refreshes   |
/// | `GPU_DEVICE_INDEX`        | `0`                   | NVIDIA GPU device index to query   |
///
/// # GPU workload nodes (the switcher)
///
/// Nodes are declared by scanning `NODE_<i>_*` for `i` = 0, 1, 2, … and **stopping at
/// the first index with no `NODE_<i>_NAME`**. That keeps the list open-ended, so a
/// future `llama03` is a config change rather than a code change. Indices need not be
/// contiguous after the first gap is found, but a gap ends the scan.
///
/// | Variable                  | Default               | Description                        |
/// |---------------------------|-----------------------|------------------------------------|
/// | `NODE_<i>_NAME`           | *(required)*          | Stable node id, e.g. `llama01`. Absent ⇒ scan stops. |
/// | `NODE_<i>_KIND`           | `llama`               | `llama` (probe `/props` for readiness) or `comfyui` (readiness = unit active). |
/// | `NODE_<i>_SERVER_URL`     | *(none)*              | llama-server root, e.g. `http://host:8000`. Omit for non-llama nodes. |
/// | `NODE_<i>_AGENT_URL`      | *(none)*              | Switch-agent root, e.g. `http://host:8100`. Omit ⇒ node is display-only and cannot be switched to. |
/// | `NODE_<i>_AGENT_TOKEN`    | *(none)*              | Bearer token for that agent. **Secret**: redacted from `Debug`/logs. |
/// | `NODE_<i>_UNIT`           | *(none)*              | systemd unit the agent controls, e.g. `llama-server.service`. Required to switch. |
/// | `NODE_<i>_MODEL_ALIAS`    | *(none)*              | Expected served model alias, e.g. `Qwen3.8-27B`. Used to identify the resident model. |
/// | `NODE_SCAN_MAX`           | `16`                  | Upper bound on indices scanned (bounds startup work on malformed config). |
///
/// A node with a missing agent URL, token or unit is **kept but marked unusable for
/// switching**, and a warning is logged. It degrades to a display-only entry rather
/// than aborting startup: one misconfigured node must not blind the whole dashboard.
///
/// # Switcher timing / safety
///
/// | Variable                     | Default | Description                          |
/// |------------------------------|---------|--------------------------------------|
/// | `SWITCH_VRAM_FLOOR_MIB`      | `2000`  | Host VRAM must fall below this before the target is started. Starting above it is a guaranteed CUDA OOM on a single-GPU box. |
/// | `SWITCH_RELEASE_TIMEOUT_SECS`| `180`   | How long to wait for VRAM to fall below the floor. |
/// | `SWITCH_READY_TIMEOUT_SECS`  | `600`   | How long to wait for the target to become ready. Model loads measured 36–148 s, so this is deliberately generous. |
/// | `SWITCH_POLL_INTERVAL_SECS`  | `5`     | Poll interval for both waits.        |
/// | `GPU_QUERY_BIN`              | `nvidia-smi` | Binary used for GPU queries; overridable so tests can inject a mock. |
/// | `MONITOR_API_TOKEN`          | *(unset)*      | If set, `POST /api/model` requires `Authorization: Bearer <token>`. If unset the mutating endpoint is open to anything that can reach it, and a warning is logged at startup. |
///
/// # Secrets
///
/// `MONITOR_API_TOKEN` and `NODE_<i>_AGENT_TOKEN` are secrets. Both are wrapped so
/// that `Debug` — which `main.rs` emits via `tracing::info!(?config, …)` — renders
/// `[REDACTED]` instead of the value. Neither is ever sent to the browser: the
/// dashboard holds agent tokens server-side only, and prompts for the API token at
/// runtime into `localStorage`, so no secret appears in the served HTML or JS.
use serde::{Deserialize, Serialize};

/// A secret that must never reach a log line.
///
/// `Config` derives `Debug` and is logged wholesale at startup, so any secret held
/// as a plain `String`/`Option<String>` would be written to the journal in
/// cleartext. Wrapping it keeps the derived `Debug` safe by construction rather
/// than relying on callers to remember not to print it.
#[derive(Clone, Default)]
pub struct Secret(pub Option<String>);

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(if self.is_set() {
            "Secret([REDACTED])"
        } else {
            "Secret(unset)"
        })
    }
}

impl Secret {
    pub fn is_set(&self) -> bool {
        self.0.as_ref().is_some_and(|s| !s.trim().is_empty())
    }

    /// Length-independent comparison; never short-circuits on a mismatched byte.
    pub fn matches(&self, provided: &str) -> bool {
        let Some(expected) = self.0.as_deref() else {
            return false;
        };
        let p = provided.as_bytes();
        let e = expected.as_bytes();
        let mut diff = (p.len() ^ e.len()) as u8;
        for (i, b) in e.iter().enumerate() {
            diff |= p.get(i).copied().unwrap_or(0) ^ b;
        }
        diff == 0
    }
}

/// What kind of GPU workload a node hosts, which decides how readiness is probed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NodeKind {
    /// llama.cpp `llama-server`: readiness = `GET /props` returns HTTP 200.
    ///
    /// `llama-server` answers **503 while a model is still loading**, so anything
    /// weaker than a 200 check would let the switcher start a second model early.
    #[default]
    Llama,
    /// ComfyUI (or similar): no `/props`, so readiness = systemd unit active.
    ComfyUi,
}

impl NodeKind {
    /// Parse a `NODE_<i>_KIND` value. Unknown values fall back to the default
    /// with a warning rather than failing startup.
    pub fn parse(raw: &str) -> Self {
        match raw.trim().to_ascii_lowercase().as_str() {
            "llama" | "llamacpp" | "llama-server" => NodeKind::Llama,
            "comfyui" | "comfy" => NodeKind::ComfyUi,
            "" => NodeKind::default(),
            other => {
                tracing::warn!(
                    kind = other,
                    "unknown NODE_<i>_KIND, falling back to 'llama'"
                );
                NodeKind::default()
            }
        }
    }
}

/// One GPU workload container the monitor can observe and switch.
///
/// `Debug` is implemented by hand so [`Config`]'s derived `Debug` — which
/// `main.rs` logs via `tracing::info!(?config, …)` — can never leak a bearer
/// token into the journal.
#[derive(Clone)]
pub struct NodeConfig {
    /// Stable identifier, e.g. `llama01`.
    pub name: String,
    pub kind: NodeKind,
    /// llama-server root URL (no `/v1`, no `/props`). `None` for non-llama nodes.
    pub server_url: Option<String>,
    /// Switch-agent root URL. `None` ⇒ display-only node.
    pub agent_url: Option<String>,
    /// Bearer token for the agent. Secret; never logged.
    pub agent_token: Option<String>,
    /// systemd unit the agent is allowed to control.
    pub unit: Option<String>,
    /// Model alias this node serves when resident.
    pub model_alias: Option<String>,
}

impl std::fmt::Debug for NodeConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NodeConfig")
            .field("name", &self.name)
            .field("kind", &self.kind)
            .field("server_url", &self.server_url)
            .field("agent_url", &self.agent_url)
            // Show presence, never the value.
            .field(
                "agent_token",
                &self.agent_token.as_ref().map(|_| "[REDACTED]"),
            )
            .field("unit", &self.unit)
            .field("model_alias", &self.model_alias)
            .finish()
    }
}

impl NodeConfig {
    /// True when this node has everything needed to be switched to/from.
    pub fn switchable(&self) -> bool {
        self.agent_url.is_some()
            && self
                .agent_token
                .as_ref()
                .is_some_and(|t| !t.trim().is_empty())
            && self.unit.as_ref().is_some_and(|u| !u.trim().is_empty())
    }

    /// True when GPU/model state can be polled (llama nodes need a server URL).
    pub fn observable(&self) -> bool {
        match self.kind {
            NodeKind::Llama => self
                .server_url
                .as_ref()
                .is_some_and(|s| !s.trim().is_empty()),
            NodeKind::ComfyUi => self.agent_url.is_some(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Config {
    pub ollama_host: String,
    pub ollama_port: u16,
    pub server_bind: String,
    pub server_port: u16,
    pub refresh_interval_secs: u64,
    pub gpu_device_index: usize,
    /// Declared GPU workload nodes, in scan order.
    pub nodes: Vec<NodeConfig>,
    /// Upper bound on `NODE_<i>_*` indices scanned.
    pub node_scan_max: usize,
    /// Host VRAM (MiB) that must be free before starting a target model.
    pub switch_vram_floor_mib: u64,
    pub switch_release_timeout_secs: u64,
    pub switch_ready_timeout_secs: u64,
    pub switch_poll_interval_secs: u64,
    /// GPU query binary; injectable so tests can substitute a mock.
    pub gpu_query_bin: String,
    /// Bearer token guarding `POST /api/model`. Unset ⇒ endpoint is open.
    pub monitor_api_token: Secret,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            ollama_host: "http://127.0.0.1".into(),
            ollama_port: 11434,
            server_bind: "0.0.0.0".into(),
            server_port: 3000,
            refresh_interval_secs: 15,
            gpu_device_index: 0,
            nodes: Vec::new(),
            node_scan_max: 16,
            switch_vram_floor_mib: 2000,
            switch_release_timeout_secs: 180,
            switch_ready_timeout_secs: 600,
            switch_poll_interval_secs: 5,
            gpu_query_bin: "nvidia-smi".into(),
            monitor_api_token: Secret(None),
        }
    }
}

/// Parse a positive integer env value, warning and keeping `default` on garbage.
fn parse_u64(get: &dyn Fn(&str) -> Option<String>, key: &str, default: u64) -> u64 {
    match get(key) {
        Some(v) => v.trim().parse::<u64>().unwrap_or_else(|_| {
            tracing::warn!(key, value = %v, "invalid unsigned integer, using default");
            default
        }),
        None => default,
    }
}

/// Trim a raw env value to a non-empty `Option<String>`.
fn opt(get: &dyn Fn(&str) -> Option<String>, key: &str) -> Option<String> {
    get(key)
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

/// Scan `NODE_<i>_*` into [`NodeConfig`]s, stopping at the first index with no name.
///
/// Takes an injectable lookup so tests never touch process-global env (which would
/// make them order-dependent under Rust's parallel test runner).
pub fn load_nodes(get: &dyn Fn(&str) -> Option<String>, scan_max: usize) -> Vec<NodeConfig> {
    let mut nodes = Vec::new();
    for i in 0..scan_max {
        let name = match opt(get, &format!("NODE_{i}_NAME")) {
            Some(n) => n,
            // A gap ends the scan; later indices are ignored by design.
            None => break,
        };

        let kind =
            NodeKind::parse(&opt(get, &format!("NODE_{i}_KIND")).unwrap_or_else(|| "llama".into()));
        let server_url = opt(get, &format!("NODE_{i}_SERVER_URL"));
        let agent_url = opt(get, &format!("NODE_{i}_AGENT_URL"));
        let agent_token = opt(get, &format!("NODE_{i}_AGENT_TOKEN"));
        let unit = opt(get, &format!("NODE_{i}_UNIT"));
        let model_alias = opt(get, &format!("NODE_{i}_MODEL_ALIAS"));

        let node = NodeConfig {
            name: name.clone(),
            kind,
            server_url,
            agent_url,
            agent_token,
            unit,
            model_alias,
        };

        // Degrade loudly but keep going: a half-configured node stays visible on
        // the dashboard instead of taking the whole monitor down.
        if !node.switchable() {
            tracing::warn!(
                node = %name,
                "node is not switchable (needs NODE_{i}_AGENT_URL, _AGENT_TOKEN and _UNIT); \
                 it will be display-only"
            );
        }
        if !node.observable() {
            tracing::warn!(
                node = %name,
                kind = ?kind,
                "node is not observable (llama nodes need NODE_{i}_SERVER_URL; \
                 comfyui nodes need _AGENT_URL)"
            );
        }

        nodes.push(node);
    }
    nodes
}

impl Config {
    /// Load config from environment variables, falling back to defaults for unset values.
    pub fn load() -> Self {
        Self::load_from(&|k| std::env::var(k).ok())
    }

    /// Load config from an injectable env lookup. See [`load_nodes`] for why.
    pub fn load_from(get: &dyn Fn(&str) -> Option<String>) -> Self {
        let mut config = Self::default();

        if let Some(val) = opt(get, "OLLAMA_HOST") {
            config.ollama_host = val;
        }
        config.ollama_port = parse_port(get, "OLLAMA_PORT", config.ollama_port);
        if let Some(val) = opt(get, "SERVER_BIND") {
            config.server_bind = val;
        }
        config.server_port = parse_port(get, "SERVER_PORT", config.server_port);
        config.refresh_interval_secs =
            parse_u64(get, "REFRESH_INTERVAL_SECS", config.refresh_interval_secs);
        if let Some(val) = get("GPU_DEVICE_INDEX") {
            match val.trim().parse::<usize>() {
                Ok(index) => config.gpu_device_index = index,
                Err(_) => tracing::warn!("Invalid GPU_DEVICE_INDEX '{}', using default", val),
            }
        }

        config.node_scan_max =
            parse_u64(get, "NODE_SCAN_MAX", config.node_scan_max as u64) as usize;
        config.switch_vram_floor_mib =
            parse_u64(get, "SWITCH_VRAM_FLOOR_MIB", config.switch_vram_floor_mib);
        config.switch_release_timeout_secs = parse_u64(
            get,
            "SWITCH_RELEASE_TIMEOUT_SECS",
            config.switch_release_timeout_secs,
        );
        config.switch_ready_timeout_secs = parse_u64(
            get,
            "SWITCH_READY_TIMEOUT_SECS",
            config.switch_ready_timeout_secs,
        );
        config.switch_poll_interval_secs = parse_u64(
            get,
            "SWITCH_POLL_INTERVAL_SECS",
            config.switch_poll_interval_secs,
        );
        if let Some(val) = opt(get, "GPU_QUERY_BIN") {
            config.gpu_query_bin = val;
        }

        config.nodes = load_nodes(get, config.node_scan_max);
        config.monitor_api_token = Secret(opt(get, "MONITOR_API_TOKEN"));

        // Only warn for real deployments: with no nodes configured there is
        // nothing to switch, so an unset token is not a misconfiguration.
        if !config.nodes.is_empty() && !config.monitor_api_token.is_set() {
            tracing::warn!(
                "MONITOR_API_TOKEN is not set: POST /api/model is unauthenticated and can \
                 stop/start GPU workloads for anything that can reach this service"
            );
        }

        config
    }

    pub fn ollama_base_url(&self) -> String {
        // If ollama_host already has a scheme, check whether a port is already specified.
        // If not, append the configured OLLAMA_PORT. This handles:
        //   "http://127.0.0.1"          → "http://127.0.0.1:11434"
        //   "http://127.0.0.1:12345"    → "http://127.0.0.1:12345" (unchanged)
        //   "127.0.0.1"                 → "127.0.0.1:11434"
        if self.ollama_host.starts_with("http://") || self.ollama_host.starts_with("https://") {
            let host_part = self
                .ollama_host
                .strip_prefix("http://")
                .or_else(|| self.ollama_host.strip_prefix("https://"))
                .unwrap();
            if host_part.contains(':') {
                self.ollama_host.clone()
            } else {
                format!("{}:{}", self.ollama_host, self.ollama_port)
            }
        } else {
            format!("{}:{}", self.ollama_host, self.ollama_port)
        }
    }

    /// Look up a node by name (the switcher's target key).
    pub fn node_by_name(&self, name: &str) -> Option<&NodeConfig> {
        self.nodes.iter().find(|n| n.name == name)
    }

    /// Look up a node by the model alias it serves.
    pub fn node_by_alias(&self, alias: &str) -> Option<&NodeConfig> {
        self.nodes
            .iter()
            .find(|n| n.model_alias.as_deref() == Some(alias))
    }

    /// Nodes that can actually be switched to/from.
    pub fn switchable_nodes(&self) -> Vec<&NodeConfig> {
        self.nodes.iter().filter(|n| n.switchable()).collect()
    }
}

/// Parse a u16 port env value, warning and keeping `default` on garbage.
///
/// Preserves the original behaviour (and its warning wording) for `OLLAMA_PORT`,
/// `SERVER_PORT`.
fn parse_port(get: &dyn Fn(&str) -> Option<String>, key: &str, default: u16) -> u16 {
    match get(key) {
        Some(val) => val.trim().parse::<u16>().unwrap_or_else(|_| {
            tracing::warn!("Invalid {} '{}', using default", key, val);
            default
        }),
        None => default,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// Build an env lookup from a map, so tests never touch process-global env.
    fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |k: &str| map.get(k).cloned()
    }

    /// A realistic three-node deployment (no real IPs or tokens: these are
    /// placeholders, and the contract forbids hardcoding either).
    fn three_node_env() -> impl Fn(&str) -> Option<String> {
        env(&[
            ("NODE_0_NAME", "llama01"),
            ("NODE_0_KIND", "llama"),
            ("NODE_0_SERVER_URL", "http://node-a.test:8000"),
            ("NODE_0_AGENT_URL", "http://node-a.test:8100"),
            ("NODE_0_AGENT_TOKEN", "token-a-secret"),
            ("NODE_0_UNIT", "llama-server.service"),
            ("NODE_0_MODEL_ALIAS", "CodingModel"),
            ("NODE_1_NAME", "llama02"),
            ("NODE_1_KIND", "llama"),
            ("NODE_1_SERVER_URL", "http://node-b.test:8000"),
            ("NODE_1_AGENT_URL", "http://node-b.test:8100"),
            ("NODE_1_AGENT_TOKEN", "token-b-secret"),
            ("NODE_1_UNIT", "llama-server.service"),
            ("NODE_1_MODEL_ALIAS", "RoleplayModel"),
            ("NODE_2_NAME", "comfyui"),
            ("NODE_2_KIND", "comfyui"),
            ("NODE_2_AGENT_URL", "http://node-c.test:8100"),
            ("NODE_2_AGENT_TOKEN", "token-c-secret"),
            ("NODE_2_UNIT", "comfyui.service"),
        ])
    }

    #[test]
    fn defaults_are_sane_and_tokenless() {
        let c = Config::default();
        assert_eq!(c.server_port, 3000);
        assert_eq!(c.ollama_port, 11434);
        assert_eq!(c.refresh_interval_secs, 15);
        assert!(c.nodes.is_empty());
        assert_eq!(c.node_scan_max, 16);
        assert_eq!(c.switch_vram_floor_mib, 2000);
        assert_eq!(c.switch_release_timeout_secs, 180);
        assert_eq!(c.switch_ready_timeout_secs, 600);
        assert_eq!(c.switch_poll_interval_secs, 5);
        assert_eq!(c.gpu_query_bin, "nvidia-smi");
    }

    #[test]
    fn parses_three_nodes_in_order() {
        let c = Config::load_from(&three_node_env());
        assert_eq!(c.nodes.len(), 3);
        let names: Vec<&str> = c.nodes.iter().map(|n| n.name.as_str()).collect();
        assert_eq!(names, vec!["llama01", "llama02", "comfyui"]);
        assert_eq!(c.nodes[0].kind, NodeKind::Llama);
        assert_eq!(c.nodes[2].kind, NodeKind::ComfyUi);
        assert_eq!(c.nodes[0].model_alias.as_deref(), Some("CodingModel"));
        assert_eq!(
            c.nodes[0].server_url.as_deref(),
            Some("http://node-a.test:8000")
        );
        // all three are fully switchable
        assert_eq!(c.switchable_nodes().len(), 3);
    }

    #[test]
    fn scan_stops_at_first_gap() {
        // NODE_1 absent ⇒ NODE_2 must be ignored even though it is well-formed.
        let c = Config::load_from(&env(&[
            ("NODE_0_NAME", "first"),
            ("NODE_0_UNIT", "a.service"),
            ("NODE_2_NAME", "third"),
            ("NODE_2_UNIT", "c.service"),
        ]));
        assert_eq!(c.nodes.len(), 1);
        assert_eq!(c.nodes[0].name, "first");
    }

    #[test]
    fn empty_env_yields_no_nodes_not_a_panic() {
        let c = Config::load_from(&env(&[]));
        assert!(c.nodes.is_empty());
        assert!(c.switchable_nodes().is_empty());
        assert!(c.node_by_name("llama01").is_none());
    }

    #[test]
    fn half_configured_node_is_kept_but_not_switchable() {
        // Missing agent token: must degrade to display-only, not abort startup.
        let c = Config::load_from(&env(&[
            ("NODE_0_NAME", "partial"),
            ("NODE_0_SERVER_URL", "http://node-a.test:8000"),
            ("NODE_0_AGENT_URL", "http://node-a.test:8100"),
            ("NODE_0_UNIT", "llama-server.service"),
        ]));
        assert_eq!(c.nodes.len(), 1);
        assert!(!c.nodes[0].switchable(), "no token ⇒ not switchable");
        assert!(c.nodes[0].observable(), "server_url present ⇒ observable");
        assert!(c.switchable_nodes().is_empty());
    }

    #[test]
    fn blank_token_is_treated_as_missing() {
        let c = Config::load_from(&env(&[
            ("NODE_0_NAME", "blank"),
            ("NODE_0_AGENT_URL", "http://node-a.test:8100"),
            ("NODE_0_AGENT_TOKEN", "   "),
            ("NODE_0_UNIT", "llama-server.service"),
        ]));
        assert_eq!(c.nodes[0].agent_token, None, "whitespace must not count");
        assert!(!c.nodes[0].switchable());
    }

    #[test]
    fn llama_node_without_server_url_is_not_observable() {
        let c = Config::load_from(&env(&[
            ("NODE_0_NAME", "no-server"),
            ("NODE_0_KIND", "llama"),
            ("NODE_0_AGENT_URL", "http://node-a.test:8100"),
            ("NODE_0_AGENT_TOKEN", "t"),
            ("NODE_0_UNIT", "llama-server.service"),
        ]));
        assert!(!c.nodes[0].observable());
        assert!(c.nodes[0].switchable());
    }

    #[test]
    fn unknown_kind_falls_back_to_llama() {
        let c = Config::load_from(&env(&[("NODE_0_NAME", "x"), ("NODE_0_KIND", "banana")]));
        assert_eq!(c.nodes[0].kind, NodeKind::Llama);
    }

    #[test]
    fn kind_parsing_accepts_aliases_and_is_case_insensitive() {
        assert_eq!(NodeKind::parse("ComfyUI"), NodeKind::ComfyUi);
        assert_eq!(NodeKind::parse("comfy"), NodeKind::ComfyUi);
        assert_eq!(NodeKind::parse("LLAMA"), NodeKind::Llama);
        assert_eq!(NodeKind::parse("llama-server"), NodeKind::Llama);
        assert_eq!(NodeKind::parse(""), NodeKind::Llama);
    }

    #[test]
    fn debug_output_redacts_tokens() {
        // main.rs logs `?config`; a leaked token would land in the journal.
        let c = Config::load_from(&three_node_env());
        let dbg = format!("{c:?}");
        assert!(!dbg.contains("token-a-secret"), "token leaked: {dbg}");
        assert!(!dbg.contains("token-b-secret"), "token leaked: {dbg}");
        assert!(!dbg.contains("token-c-secret"), "token leaked: {dbg}");
        assert!(dbg.contains("[REDACTED]"), "must show redaction marker");
        // non-secret fields stay useful for debugging
        assert!(dbg.contains("llama01"));
        assert!(dbg.contains("node-a.test:8000"));
    }

    #[test]
    fn node_debug_redacts_token_too() {
        let n = NodeConfig {
            name: "n".into(),
            kind: NodeKind::Llama,
            server_url: Some("http://s.test:8000".into()),
            agent_url: Some("http://s.test:8100".into()),
            agent_token: Some("super-secret".into()),
            unit: Some("u.service".into()),
            model_alias: Some("M".into()),
        };
        let dbg = format!("{n:?}");
        assert!(!dbg.contains("super-secret"));
        assert!(dbg.contains("[REDACTED]"));
    }

    #[test]
    fn lookups_by_name_and_alias() {
        let c = Config::load_from(&three_node_env());
        assert_eq!(c.node_by_name("llama02").unwrap().name, "llama02");
        assert_eq!(
            c.node_by_alias("RoleplayModel").unwrap().name,
            "llama02",
            "alias lookup is how a model-selection request maps to a node"
        );
        assert!(c.node_by_alias("Nonexistent").is_none());
        assert!(c.node_by_name("nope").is_none());
    }

    #[test]
    fn numeric_env_parsing_and_garbage_fallback() {
        let c = Config::load_from(&env(&[
            ("SERVER_PORT", "4000"),
            ("REFRESH_INTERVAL_SECS", "30"),
            ("SWITCH_VRAM_FLOOR_MIB", "1500"),
            ("SWITCH_RELEASE_TIMEOUT_SECS", "90"),
            ("SWITCH_READY_TIMEOUT_SECS", "900"),
            ("SWITCH_POLL_INTERVAL_SECS", "2"),
            ("GPU_DEVICE_INDEX", "1"),
            ("GPU_QUERY_BIN", "/mock/nvidia-smi"),
        ]));
        assert_eq!(c.server_port, 4000);
        assert_eq!(c.refresh_interval_secs, 30);
        assert_eq!(c.switch_vram_floor_mib, 1500);
        assert_eq!(c.switch_release_timeout_secs, 90);
        assert_eq!(c.switch_ready_timeout_secs, 900);
        assert_eq!(c.switch_poll_interval_secs, 2);
        assert_eq!(c.gpu_device_index, 1);
        assert_eq!(c.gpu_query_bin, "/mock/nvidia-smi");

        // garbage must warn-and-default, never panic
        let bad = Config::load_from(&env(&[
            ("SERVER_PORT", "not-a-port"),
            ("REFRESH_INTERVAL_SECS", "-5"),
            ("SWITCH_VRAM_FLOOR_MIB", "abc"),
        ]));
        assert_eq!(bad.server_port, 3000);
        assert_eq!(bad.refresh_interval_secs, 15);
        assert_eq!(bad.switch_vram_floor_mib, 2000);
    }

    #[test]
    fn ollama_base_url_behaviour_unchanged() {
        // Regression guard: this helper predates the switcher work.
        let c = Config::load_from(&env(&[("OLLAMA_HOST", "http://ollama.test")]));
        assert_eq!(c.ollama_base_url(), "http://ollama.test:11434");
        let c = Config::load_from(&env(&[("OLLAMA_HOST", "http://ollama.test:1234")]));
        assert_eq!(c.ollama_base_url(), "http://ollama.test:1234");
        let c = Config::load_from(&env(&[("OLLAMA_HOST", "ollama.test")]));
        assert_eq!(c.ollama_base_url(), "ollama.test:11434");
        let c = Config::default();
        assert_eq!(c.ollama_base_url(), "http://127.0.0.1:11434");
    }

    #[test]
    fn node_scan_max_bounds_the_scan() {
        let mut pairs: Vec<(String, String)> = Vec::new();
        for i in 0..40 {
            pairs.push((format!("NODE_{i}_NAME"), format!("n{i}")));
        }
        let map: HashMap<String, String> = pairs.into_iter().collect();
        let get = move |k: &str| map.get(k).cloned();
        let nodes = load_nodes(&get, 8);
        assert_eq!(nodes.len(), 8, "scan must respect NODE_SCAN_MAX");
        assert_eq!(nodes[7].name, "n7");
    }

    #[test]
    fn whitespace_is_trimmed_in_values() {
        let c = Config::load_from(&env(&[
            ("NODE_0_NAME", "  llama01  "),
            ("NODE_0_UNIT", "  llama-server.service "),
            ("NODE_0_AGENT_URL", " http://node-a.test:8100 "),
            ("NODE_0_AGENT_TOKEN", " tok "),
        ]));
        assert_eq!(c.nodes[0].name, "llama01");
        assert_eq!(c.nodes[0].unit.as_deref(), Some("llama-server.service"));
        assert_eq!(
            c.nodes[0].agent_url.as_deref(),
            Some("http://node-a.test:8100")
        );
        assert_eq!(c.nodes[0].agent_token.as_deref(), Some("tok"));
        assert!(c.nodes[0].switchable());
    }

    // ── Secret handling ────────────────────────────────

    #[test]
    fn secret_is_set_ignores_blank_values() {
        assert!(!Secret(None).is_set());
        assert!(!Secret(Some(String::new())).is_set());
        assert!(!Secret(Some("   ".into())).is_set());
        assert!(Secret(Some("tok".into())).is_set());
    }

    #[test]
    fn secret_matches_without_short_circuit() {
        let s = Secret(Some("s3cr3t-token".into()));
        assert!(s.matches("s3cr3t-token"));
        assert!(!s.matches("s3cr3t-tokem"));
        assert!(!s.matches("s3cr3t"));
        assert!(!s.matches("s3cr3t-token-longer"));
        assert!(!s.matches(""));
        // An unset secret must never authenticate anything, including "".
        assert!(!Secret(None).matches(""));
        assert!(!Secret(None).matches("anything"));
    }

    #[test]
    fn secret_debug_never_renders_the_value() {
        let s = Secret(Some("super-secret-token".into()));
        let dbg = format!("{s:?}");
        assert!(!dbg.contains("super-secret-token"), "leaked: {dbg}");
        assert!(dbg.contains("REDACTED"), "should show marker: {dbg}");
        assert_eq!(format!("{:?}", Secret(None)), "Secret(unset)");
    }

    #[test]
    fn config_debug_redacts_monitor_api_token() {
        // main.rs logs `?config` wholesale; this is the guard that keeps the
        // mutating-endpoint token out of the journal.
        let c = Config::load_from(&env(&[("MONITOR_API_TOKEN", "mon-secret-xyz")]));
        assert!(c.monitor_api_token.is_set());
        let dbg = format!("{c:?}");
        assert!(!dbg.contains("mon-secret-xyz"), "token leaked: {dbg}");
        assert!(dbg.contains("REDACTED"));
    }

    #[test]
    fn monitor_api_token_defaults_unset() {
        let c = Config::load_from(&env(&[]));
        assert!(!c.monitor_api_token.is_set());
        assert!(!Config::default().monitor_api_token.is_set());
    }

    #[test]
    fn monitor_api_token_is_trimmed() {
        // `opt()` trims every env value, so a stray space in an env file cannot
        // break auth. The stored secret is the trimmed form, and the untrimmed
        // raw value must NOT match — asserting the opposite (as an earlier
        // version of this test did) encodes a bug, not a guarantee.
        let c = Config::load_from(&env(&[("MONITOR_API_TOKEN", "  tok  ")]));
        assert!(c.monitor_api_token.is_set());
        assert!(
            c.monitor_api_token.matches("tok"),
            "trimmed value must match"
        );
        assert!(
            !c.monitor_api_token.matches("  tok  "),
            "untrimmed raw value must not match"
        );
    }
}
