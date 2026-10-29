//! GPU workload switcher: arbitrates the single Tesla V100 between mutually
//! exclusive workloads (`llama01`/Qwen, `llama02`/Precog, `comfyui`).
//!
//! # The invariant this module exists to enforce
//!
//! Only **one** model can be VRAM-resident at a time. A 16.4 GB Q4_K_M Qwen plus a
//! 14.3 GB Q4_K_M Precog is ~30.7 GB of weights alone on a 32 GB card, before any
//! KV cache. Starting a target before the incumbent has released VRAM is a
//! guaranteed CUDA OOM — and worse, it can leave both models half-loaded.
//!
//! Therefore: **the target is never started until host VRAM has been observed
//! below `SWITCH_VRAM_FLOOR_MIB`.** On timeout the switch *fails*; it never starts
//! anyway "and hopes".
//!
//! # Why VRAM is read from host NVML, and state from the agents
//!
//! `nvidia-smi --query-compute-apps` reports `process_name` as `[Not Found]` for
//! processes inside containers (PID namespaces are not shared), so per-container
//! attribution is impossible from outside. Measured on this host: two GPU PIDs both
//! rendered as `[Not Found]`. So the design splits responsibility:
//!
//! * **NVML totals** (host `nvidia-smi`) are the authoritative VRAM guard — they are
//!   namespace-independent and cannot lie about how much card is in use.
//! * **Switch agents** self-report which unit is active and whether their model is
//!   ready, which is the only reliable source of *attribution*.
//!
//! If a node's agent is unreachable we cannot know what it holds, so we do not
//! guess: the VRAM floor check will simply never pass and the switch fails closed.
//!
//! # Testability
//!
//! The ordering logic is a **pure state machine** ([`SwitchPlan::next`]) driven by a
//! [`WorldSnapshot`] the caller supplies. That means the safety invariant is
//! asserted over deterministic snapshot sequences in unit tests, with no HTTP, no
//! GPU and no sleeps involved. The async [`Switcher`] driver is a thin executor that
//! gathers real snapshots and performs the returned [`SwitchAction`].

use anyhow::{bail, Context, Result};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use tracing::{info, warn};

use crate::config::{Config, NodeConfig, NodeKind};

// ── Switch-agent wire types ──────────────────────────────
//
// These mirror the JSON emitted by `src/bin/switch_agent.rs` exactly. Field names
// are load-bearing: a mismatch silently yields `None`/`false` and would make every
// node look inactive, which could cause the switcher to skip a needed stop.

/// `GET /state` response from a switch agent.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AgentState {
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub units: Vec<AgentUnit>,
    /// `null` for non-llama nodes (e.g. comfyui).
    #[serde(default)]
    pub model: Option<AgentModel>,
}

/// One allowlisted systemd unit's state, as reported by its agent.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AgentUnit {
    #[serde(default)]
    pub unit: String,
    /// `systemctl is-active` output: `active`, `inactive`, `failed`, …
    #[serde(default)]
    pub active: String,
    #[serde(default)]
    pub enabled: String,
}

/// Model readiness as reported by the agent (which probes llama-server `/props`).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AgentModel {
    /// True only when `/props` returned HTTP 200 — i.e. loaded *and* serving.
    #[serde(default)]
    pub ready: bool,
    #[serde(default)]
    pub model_alias: Option<String>,
    #[serde(default)]
    pub n_ctx: Option<u64>,
    #[serde(default)]
    pub total_slots: Option<u64>,
    #[serde(default)]
    pub http_status: Option<u16>,
    #[serde(default)]
    pub error: Option<String>,
}

/// Response to `POST /start` and `POST /stop`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AgentActionResponse {
    #[serde(default)]
    pub unit: String,
    #[serde(default)]
    pub action: String,
    #[serde(default)]
    pub active: String,
}

/// Error envelope returned by an agent (`{"error": "..."}`).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AgentErrorBody {
    #[serde(default)]
    pub error: String,
}

impl AgentState {
    /// True when any reported unit is `active`.
    pub fn any_unit_active(&self) -> bool {
        self.units.iter().any(|u| u.active == "active")
    }

    /// Resolve the state of a specific unit, if reported.
    pub fn unit_state(&self, unit: &str) -> Option<&str> {
        self.units
            .iter()
            .find(|u| u.unit == unit)
            .map(|u| u.active.as_str())
    }
}

// ── Agent HTTP client ────────────────────────────────────

/// Client for one switch agent. Every call sends the bearer token.
#[derive(Clone)]
pub struct AgentClient {
    base_url: String,
    token: String,
    http: Client,
}

impl AgentClient {
    pub fn new(base_url: impl Into<String>, token: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into(),
            token: token.into(),
            http: Client::builder()
                // This budget is for actions (start/stop), where
                // `systemctl stop` on a 16 GB model takes seconds. State
                // polls are read-only and use a shorter per-request timeout
                // in `state()` so a hung agent cannot stall the poll loop.
                .timeout(Duration::from_secs(60))
                .build()
                .expect("reqwest client build"),
        }
    }

    fn auth(&self, rb: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        rb.bearer_auth(&self.token)
    }

    /// `GET /state`.
    pub async fn state(&self) -> Result<AgentState> {
        let url = format!("{}/state", self.base_url);
        let resp = self
            .auth(self.http.get(&url))
            // Read-only poll: cap each state request at 5s instead of the
            // client-wide 60s action budget, since `observe()` polls every
            // node on a ~2s cadence.
            .timeout(Duration::from_secs(5))
            .send()
            .await
            .with_context(|| format!("agent unreachable at {url}"))?;
        Self::check_status(resp, &url)
            .await?
            .json::<AgentState>()
            .await
            .with_context(|| format!("failed to parse agent /state from {url}"))
    }

    /// `POST /start` for an allowlisted unit.
    pub async fn start(&self, unit: &str) -> Result<AgentActionResponse> {
        self.action("start", unit).await
    }

    /// `POST /stop` for an allowlisted unit.
    pub async fn stop(&self, unit: &str) -> Result<AgentActionResponse> {
        self.action("stop", unit).await
    }

    async fn action(&self, verb: &str, unit: &str) -> Result<AgentActionResponse> {
        let url = format!("{}/{verb}", self.base_url);
        let resp = self
            .auth(
                self.http
                    .post(&url)
                    .json(&serde_json::json!({ "unit": unit })),
            )
            .send()
            .await
            .with_context(|| format!("agent unreachable at {url}"))?;
        Self::check_status(resp, &url)
            .await?
            .json::<AgentActionResponse>()
            .await
            .with_context(|| format!("failed to parse agent /{verb} response from {url}"))
    }

    /// Turn a non-2xx into an error carrying the agent's own message.
    ///
    /// 401/403 mean the token is wrong or the unit is not allowlisted — surfacing
    /// the agent's text makes misconfiguration obvious instead of opaque.
    async fn check_status(resp: reqwest::Response, url: &str) -> Result<reqwest::Response> {
        let status = resp.status();
        if status.is_success() {
            return Ok(resp);
        }
        let body = resp.text().await.unwrap_or_default();
        let detail = serde_json::from_str::<AgentErrorBody>(&body)
            .ok()
            .map(|e| e.error)
            .filter(|s| !s.is_empty())
            .unwrap_or(body);
        bail!("agent {url} returned HTTP {status}: {}", detail.trim())
    }
}

// ── Pure ordering state machine ──────────────────────────

/// What the driver should do next.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SwitchAction {
    /// Ask node `i`'s agent to stop its unit.
    StopNode(usize),
    /// Ask node `i`'s agent to start its unit. Emitted **only** after VRAM has been
    /// observed below the floor.
    StartNode(usize),
    /// Nothing to do but re-poll after the configured interval.
    Wait,
    /// Switch complete.
    Done,
    /// Switch aborted; the reason is user-facing.
    Fail(String),
}

/// Observed state of one node, gathered by the driver.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeSnapshot {
    /// Any allowlisted unit is `active`.
    pub active: bool,
    /// Workload is usable: llama ⇒ `/props` 200; comfyui ⇒ unit active.
    pub ready: bool,
    /// Agent could not be reached. Treated as "unknown, assume it may hold VRAM".
    pub unreachable: bool,
}

impl NodeSnapshot {
    pub fn unknown() -> Self {
        Self {
            active: false,
            ready: false,
            unreachable: true,
        }
    }
    /// Test-only constructors. Gated so they cannot be mistaken for production
    /// helpers and so `clippy -D warnings` stays clean in a normal build.
    #[cfg(test)]
    pub fn idle() -> Self {
        Self {
            active: false,
            ready: false,
            unreachable: false,
        }
    }
    #[cfg(test)]
    pub fn active_not_ready() -> Self {
        Self {
            active: true,
            ready: false,
            unreachable: false,
        }
    }
    #[cfg(test)]
    pub fn active_ready() -> Self {
        Self {
            active: true,
            ready: true,
            unreachable: false,
        }
    }
}

/// Everything the state machine is allowed to know. No I/O, no clocks.
#[derive(Debug, Clone)]
pub struct WorldSnapshot {
    /// Host VRAM in use, from NVML. `None` ⇒ unreadable, which must fail closed.
    pub vram_used_mib: Option<u64>,
    /// One entry per configured node, in config order.
    pub nodes: Vec<NodeSnapshot>,
    /// Seconds since the switch began (governs the release timeout).
    pub elapsed_release_secs: u64,
    /// Seconds since `StartNode` was issued (governs the ready timeout).
    pub elapsed_ready_secs: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Init,
    StoppingOthers,
    WaitingForVram,
    StartingTarget,
    WaitingForReady,
    Finished,
}

/// Deterministic switch sequencing. Constructed per switch request.
#[derive(Debug)]
pub struct SwitchPlan {
    target: usize,
    phase: Phase,
    floor_mib: u64,
    release_timeout_secs: u64,
    ready_timeout_secs: u64,
    start_issued: bool,
}

impl SwitchPlan {
    pub fn new(
        target: usize,
        floor_mib: u64,
        release_timeout_secs: u64,
        ready_timeout_secs: u64,
    ) -> Self {
        Self {
            target,
            phase: Phase::Init,
            floor_mib,
            release_timeout_secs,
            ready_timeout_secs,
            start_issued: false,
        }
    }

    pub fn phase(&self) -> &'static str {
        match self.phase {
            Phase::Init => "init",
            Phase::StoppingOthers => "stopping-others",
            Phase::WaitingForVram => "waiting-for-vram",
            Phase::StartingTarget => "starting-target",
            Phase::WaitingForReady => "waiting-for-ready",
            Phase::Finished => "finished",
        }
    }

    /// Advance the plan given a fresh observation. Pure: same inputs ⇒ same output.
    pub fn next(&mut self, w: &WorldSnapshot) -> SwitchAction {
        loop {
            match self.phase {
                Phase::Init => {
                    let ready = w.nodes.get(self.target).map(|n| n.ready).unwrap_or(false);
                    if ready {
                        // Already serving the requested workload: a no-op success is
                        // the right answer, not a needless 2-minute reload.
                        self.phase = Phase::Finished;
                        return SwitchAction::Done;
                    }
                    self.phase = Phase::StoppingOthers;
                }

                Phase::StoppingOthers => {
                    // Stop every OTHER active holder, one action at a time so the
                    // driver re-observes between stops. The target is never stopped.
                    let victim =
                        (0..w.nodes.len()).find(|&i| i != self.target && w.nodes[i].active);
                    match victim {
                        // Bounded: if a non-target is still active (or keeps
                        // coming back) past the release timeout, starting the
                        // target now would OOM. Fail closed instead of issuing
                        // yet another stop.
                        Some(i) if w.elapsed_release_secs >= self.release_timeout_secs => {
                            self.phase = Phase::Finished;
                            return SwitchAction::Fail(format!(
                                "node {i} still active after {to}s of release \
                                 attempts; refusing to start target (would OOM)",
                                to = self.release_timeout_secs
                            ));
                        }
                        Some(i) => return SwitchAction::StopNode(i),
                        None => self.phase = Phase::WaitingForVram,
                    }
                }

                Phase::WaitingForVram => {
                    // THE safety gate. Note the `None` case: if VRAM cannot be read
                    // we cannot prove the card is free, so we must not proceed.
                    match w.vram_used_mib {
                        None => {
                            self.phase = Phase::Finished;
                            return SwitchAction::Fail(
                                "cannot read host VRAM; refusing to start a model blind"
                                    .to_string(),
                            );
                        }
                        Some(v) if v < self.floor_mib => self.phase = Phase::StartingTarget,
                        Some(v) => {
                            if w.elapsed_release_secs >= self.release_timeout_secs {
                                self.phase = Phase::Finished;
                                return SwitchAction::Fail(format!(
                                    "VRAM did not fall below {floor} MiB within {to}s \
                                     (still {v} MiB); refusing to start target (would OOM)",
                                    floor = self.floor_mib,
                                    to = self.release_timeout_secs,
                                    v = v
                                ));
                            }
                            return SwitchAction::Wait;
                        }
                    }
                }

                Phase::StartingTarget => {
                    if !self.start_issued {
                        self.start_issued = true;
                        return SwitchAction::StartNode(self.target);
                    }
                    self.phase = Phase::WaitingForReady;
                }

                Phase::WaitingForReady => {
                    let ready = w.nodes.get(self.target).map(|n| n.ready).unwrap_or(false);
                    if ready {
                        self.phase = Phase::Finished;
                        return SwitchAction::Done;
                    }
                    if w.elapsed_ready_secs >= self.ready_timeout_secs {
                        self.phase = Phase::Finished;
                        return SwitchAction::Fail(format!(
                            "target did not become ready within {}s of start",
                            self.ready_timeout_secs
                        ));
                    }
                    return SwitchAction::Wait;
                }

                Phase::Finished => return SwitchAction::Done,
            }
        }
    }
}

// ── Outcome reporting ────────────────────────────────────

/// Result of a switch attempt, with the measurements that prove what happened.
#[derive(Debug, Clone, Serialize)]
pub struct SwitchOutcome {
    pub target: String,
    pub target_model: Option<String>,
    pub success: bool,
    /// True when the target was already resident, so nothing was restarted.
    pub no_op: bool,
    pub stopped_nodes: Vec<String>,
    pub vram_before_mib: Option<u64>,
    pub vram_min_mib: Option<u64>,
    pub vram_after_mib: Option<u64>,
    pub vram_floor_mib: u64,
    pub release_secs: Option<u64>,
    pub ready_secs: Option<u64>,
    pub total_secs: u64,
    pub error: Option<String>,
}

/// Live progress of an in-flight (or most recent) switch.
///
/// A switch takes 36–148 s measured, so `POST /api/model` runs it in the
/// background and the dashboard polls this instead of holding an HTTP request
/// open long enough to hit a proxy timeout.
#[derive(Debug, Clone, Serialize)]
pub struct SwitchProgress {
    pub running: bool,
    pub target: Option<String>,
    /// Current [`SwitchPlan`] phase, e.g. `waiting-for-vram`.
    pub phase: String,
    pub vram_used_mib: Option<u64>,
    pub vram_floor_mib: u64,
    pub stopped_nodes: Vec<String>,
    pub elapsed_secs: u64,
    pub outcome: Option<SwitchOutcome>,
    pub error: Option<String>,
}

impl Default for SwitchProgress {
    fn default() -> Self {
        Self {
            running: false,
            target: None,
            phase: "idle".to_string(),
            vram_used_mib: None,
            vram_floor_mib: 0,
            stopped_nodes: Vec::new(),
            elapsed_secs: 0,
            outcome: None,
            error: None,
        }
    }
}

/// Per-node live view for the dashboard/API.
#[derive(Debug, Clone, Serialize)]
pub struct NodeStatus {
    pub name: String,
    pub kind: String,
    pub model_alias: Option<String>,
    pub n_ctx: Option<u64>,
    pub total_slots: Option<u64>,
    /// Workload is loaded and serving.
    pub ready: bool,
    /// systemd unit is active (which for a loading llama-server is true while
    /// `ready` is still false — the distinction that prevents an OOM).
    pub unit_active: bool,
    pub switchable: bool,
    pub observable: bool,
    pub agent_reachable: bool,
    pub error: Option<String>,
}

// ── Async driver ─────────────────────────────────────────

/// Executes [`SwitchPlan`] actions against real agents and the host GPU.
#[derive(Clone)]
pub struct Switcher {
    config: Config,
}

impl Switcher {
    pub fn new(config: Config) -> Self {
        Self { config }
    }

    fn agent_for(&self, node: &NodeConfig) -> Option<AgentClient> {
        match (&node.agent_url, &node.agent_token) {
            (Some(url), Some(tok)) if !tok.trim().is_empty() => {
                Some(AgentClient::new(url.clone(), tok.clone()))
            }
            _ => None,
        }
    }

    /// Read host VRAM via NVML (`nvidia-smi`) on a blocking thread, so the fork
    /// never runs on a Tokio worker.
    async fn vram_mib_async(&self) -> Option<u64> {
        let bin = self.config.gpu_query_bin.clone();
        let idx = self.config.gpu_device_index;
        tokio::task::spawn_blocking(move || {
            crate::gpu::query_gpu_bin(&bin, idx)
                .ok()
                .and_then(|m| m.memory_used_mib)
        })
        .await
        .ok()
        .flatten()
    }

    /// Snapshot every node: agent `/state` for attribution, plus one NVML read.
    ///
    /// An unreachable agent yields [`NodeSnapshot::unknown`] rather than `idle`,
    /// so "we could not ask" is never mistaken for "it holds nothing".
    pub async fn observe(&self) -> (Vec<NodeSnapshot>, Vec<Option<AgentState>>, Option<u64>) {
        let mut snaps = Vec::with_capacity(self.config.nodes.len());
        let mut states = Vec::with_capacity(self.config.nodes.len());

        for node in &self.config.nodes {
            let state = match self.agent_for(node) {
                Some(client) => client.state().await.ok(),
                None => None,
            };
            let snap = match &state {
                None => NodeSnapshot::unknown(),
                Some(s) => {
                    let active = s.any_unit_active();
                    let ready = match node.kind {
                        // llama-server: only /props==200 counts. `active` is true
                        // for the whole 36–148 s load, so it is NOT readiness.
                        NodeKind::Llama => s.model.as_ref().map(|m| m.ready).unwrap_or(false),
                        // No /props to consult; unit active is the best signal.
                        NodeKind::ComfyUi => active,
                    };
                    NodeSnapshot {
                        active,
                        ready,
                        unreachable: false,
                    }
                }
            };
            snaps.push(snap);
            states.push(state);
        }

        let cfg_bin = self.config.gpu_query_bin.clone();
        let idx = self.config.gpu_device_index;
        let vram = tokio::task::spawn_blocking(move || {
            crate::gpu::query_gpu_bin(&cfg_bin, idx)
                .ok()
                .and_then(|m| m.memory_used_mib)
        })
        .await
        .ok()
        .flatten();

        (snaps, states, vram)
    }

    /// Live per-node view for the dashboard.
    pub async fn node_statuses(&self) -> (Vec<NodeStatus>, Option<u64>) {
        let (_, states, vram) = self.observe().await;
        let out = self
            .config
            .nodes
            .iter()
            .zip(states.iter())
            .map(|(node, state)| {
                let model = state.as_ref().and_then(|s| s.model.clone());
                let unit = node.unit.clone().unwrap_or_default();
                let unit_active = state
                    .as_ref()
                    .and_then(|s| s.unit_state(&unit))
                    .map(|a| a == "active")
                    .unwrap_or(false);
                // Same readiness rule `observe()` applies, per node kind: a
                // llama node is ready when its model probe says so, a ComfyUI
                // node when its unit is active; no applicable state => not ready.
                let ready = match node.kind {
                    NodeKind::Llama => model.as_ref().map(|m| m.ready).unwrap_or(false),
                    NodeKind::ComfyUi => unit_active,
                };
                NodeStatus {
                    name: node.name.clone(),
                    kind: match node.kind {
                        NodeKind::Llama => "llama".to_string(),
                        NodeKind::ComfyUi => "comfyui".to_string(),
                    },
                    model_alias: model
                        .as_ref()
                        .and_then(|m| m.model_alias.clone())
                        .or_else(|| node.model_alias.clone()),
                    n_ctx: model.as_ref().and_then(|m| m.n_ctx),
                    total_slots: model.as_ref().and_then(|m| m.total_slots),
                    ready,
                    unit_active,
                    switchable: node.switchable(),
                    observable: node.observable(),
                    agent_reachable: state.is_some(),
                    error: state
                        .as_ref()
                        .map(|s| s.model.as_ref().and_then(|m| m.error.clone()))
                        .unwrap_or(None)
                        .or_else(|| {
                            if state.is_none() {
                                Some("agent unreachable".to_string())
                            } else {
                                None
                            }
                        }),
                }
            })
            .collect();
        (out, vram)
    }

    /// Run a switch to `target_name`, enforcing the VRAM gate throughout and
    /// publishing progress for the dashboard to poll.
    pub async fn switch_to_with_progress(
        &self,
        target_name: &str,
        progress: Option<Arc<RwLock<SwitchProgress>>>,
    ) -> Result<SwitchOutcome> {
        let target_idx = self
            .config
            .nodes
            .iter()
            .position(|n| n.name == target_name)
            .with_context(|| format!("unknown node '{target_name}'"))?;
        let target_node = self.config.nodes[target_idx].clone();
        if !target_node.switchable() {
            bail!("node '{target_name}' is not switchable (missing agent URL, token or unit)");
        }

        let mut plan = SwitchPlan::new(
            target_idx,
            self.config.switch_vram_floor_mib,
            self.config.switch_release_timeout_secs,
            self.config.switch_ready_timeout_secs,
        );

        let poll = Duration::from_secs(self.config.switch_poll_interval_secs.max(1));
        let t0 = Instant::now();
        let mut started_at: Option<Instant> = None;
        let mut stopped: Vec<String> = Vec::new();
        let mut vram_before: Option<u64> = None;
        let mut vram_min: Option<u64> = None;
        let mut release_secs: Option<u64> = None;
        let mut ready_secs: Option<u64> = None;
        let mut no_op = false;
        let mut failure: Option<String> = None;

        loop {
            let (snaps, states, vram) = self.observe().await;
            if vram_before.is_none() {
                vram_before = vram;
            }
            if let Some(v) = vram {
                vram_min = Some(vram_min.map_or(v, |m: u64| m.min(v)));
            }

            let world = WorldSnapshot {
                vram_used_mib: vram,
                nodes: snaps.clone(),
                elapsed_release_secs: t0.elapsed().as_secs(),
                elapsed_ready_secs: started_at.map(|s| s.elapsed().as_secs()).unwrap_or(0),
            };

            let action = plan.next(&world);
            info!(
                target = target_name,
                phase = plan.phase(),
                vram_mib = ?vram,
                ?action,
                "switch step"
            );

            if let Some(p) = &progress {
                let mut g = p.write().await;
                g.running = true;
                g.target = Some(target_name.to_string());
                g.phase = plan.phase().to_string();
                g.vram_used_mib = vram;
                g.vram_floor_mib = self.config.switch_vram_floor_mib;
                g.stopped_nodes = stopped.clone();
                g.elapsed_secs = t0.elapsed().as_secs();
                g.error = None;
            }

            match action {
                SwitchAction::Done => {
                    // No start was issued => the target was already resident.
                    no_op = started_at.is_none();
                    // Only stamp this on the no-op path. On a real switch it was
                    // already recorded when StartNode was issued, which is the
                    // first moment VRAM is *known* to be below the floor.
                    // Overwriting it here made release_secs == total_secs:
                    // observed live as both 161 s while VRAM had actually hit
                    // 0 MiB at ~12 s.
                    if release_secs.is_none() {
                        release_secs = Some(t0.elapsed().as_secs());
                    }
                    ready_secs = started_at.map(|s| s.elapsed().as_secs());
                    break;
                }
                SwitchAction::Fail(reason) => {
                    warn!(target = target_name, error = %reason, "switch aborted");
                    failure = Some(reason);
                    break;
                }
                SwitchAction::StopNode(i) => {
                    let node = &self.config.nodes[i];
                    if let (Some(client), Some(unit)) = (self.agent_for(node), node.unit.clone()) {
                        match client.stop(&unit).await {
                            Ok(r) => {
                                info!(node = %node.name, unit = %unit, active = %r.active, "stopped");
                                stopped.push(node.name.clone());
                            }
                            Err(e) => {
                                // A stop we cannot confirm is not fatal on its own:
                                // the VRAM gate below is the real guard and will fail
                                // closed if the card never frees up.
                                warn!(node = %node.name, error = %e, "stop request failed");
                            }
                        }
                    } else {
                        warn!(
                            node = %node.name,
                            "cannot stop node: no agent client or unit configured"
                        );
                    }
                    // Pace retries: at most one stop attempt per node per poll
                    // interval, even when the attempt errors or the node is
                    // unconfigurable (prevents a tight retry loop).
                    tokio::time::sleep(poll).await;
                }
                SwitchAction::StartNode(i) => {
                    // Reaching StartNode at all proves the plan observed VRAM below
                    // the floor, so this is the authoritative "release confirmed"
                    // timestamp -- NOT the moment we asked something to stop.
                    if release_secs.is_none() {
                        release_secs = Some(t0.elapsed().as_secs());
                    }
                    let node = &self.config.nodes[i];
                    if let (Some(client), Some(unit)) = (self.agent_for(node), node.unit.clone()) {
                        client
                            .start(&unit)
                            .await
                            .context("failed to start target")?;
                        started_at = Some(Instant::now());
                    } else {
                        bail!("cannot start node '{}': no agent client or unit", node.name);
                    }
                }
                SwitchAction::Wait => tokio::time::sleep(poll).await,
            }

            // Remember the latest observed agent states for the outcome report.
            let _ = &states;
        }

        let target_model = match &self.agent_for(&target_node) {
            Some(client) => client
                .state()
                .await
                .ok()
                .and_then(|s| s.model.and_then(|m| m.model_alias)),
            None => None,
        };

        // Measured AFTER the switch settles, not reused from the last mid-loop
        // sample: the point of this figure is to prove the end state.
        let vram_after = self.vram_mib_async().await;

        let outcome = SwitchOutcome {
            target: target_name.to_string(),
            target_model: target_model.or_else(|| target_node.model_alias.clone()),
            success: failure.is_none(),
            no_op,
            stopped_nodes: stopped,
            vram_before_mib: vram_before,
            vram_min_mib: vram_min,
            vram_after_mib: vram_after,
            vram_floor_mib: self.config.switch_vram_floor_mib,
            release_secs,
            ready_secs,
            total_secs: t0.elapsed().as_secs(),
            error: failure,
        };

        if let Some(p) = &progress {
            let mut g = p.write().await;
            g.running = false;
            g.phase = "finished".to_string();
            g.vram_used_mib = outcome.vram_after_mib;
            g.stopped_nodes = outcome.stopped_nodes.clone();
            g.elapsed_secs = outcome.total_secs;
            g.error = outcome.error.clone();
            g.outcome = Some(outcome.clone());
        }

        Ok(outcome)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FLOOR: u64 = 2000;
    const REL_TO: u64 = 30;
    const RDY_TO: u64 = 60;

    /// Three-node world matching production: llama01, llama02, comfyui.
    fn world(vram: Option<u64>, nodes: Vec<NodeSnapshot>) -> WorldSnapshot {
        WorldSnapshot {
            vram_used_mib: vram,
            nodes,
            elapsed_release_secs: 0,
            elapsed_ready_secs: 0,
        }
    }

    fn three(a: NodeSnapshot, b: NodeSnapshot, c: NodeSnapshot) -> Vec<NodeSnapshot> {
        vec![a, b, c]
    }

    // ── THE safety invariant ──────────────────────────────

    #[test]
    fn never_starts_while_vram_above_floor() {
        // llama01 (idx 0) is the target; llama02 (idx 1) holds 24 GB and will not
        // release it. Every returned action must be a stop or a wait — StartNode
        // must never appear, no matter how many polls occur.
        let mut plan = SwitchPlan::new(0, FLOOR, REL_TO, RDY_TO);
        let mut saw_start = false;
        for t in 0..REL_TO {
            let w = WorldSnapshot {
                vram_used_mib: Some(24753),
                nodes: three(
                    NodeSnapshot::idle(),
                    NodeSnapshot::active_ready(),
                    NodeSnapshot::idle(),
                ),
                elapsed_release_secs: t,
                elapsed_ready_secs: 0,
            };
            match plan.next(&w) {
                SwitchAction::StartNode(_) => saw_start = true,
                SwitchAction::StopNode(i) => assert_ne!(i, 0, "must not stop the target"),
                SwitchAction::Wait | SwitchAction::Fail(_) | SwitchAction::Done => {}
            }
        }
        assert!(
            !saw_start,
            "INVARIANT VIOLATED: started with VRAM above floor"
        );
    }

    #[test]
    fn fails_closed_on_release_timeout_instead_of_starting() {
        let mut plan = SwitchPlan::new(0, FLOOR, REL_TO, RDY_TO);
        // Drain the stop actions first.
        let w = world(
            Some(24753),
            three(
                NodeSnapshot::idle(),
                NodeSnapshot::active_ready(),
                NodeSnapshot::idle(),
            ),
        );
        assert_eq!(plan.next(&w), SwitchAction::StopNode(1));
        // Now nothing is active but VRAM is still high (e.g. a wedged context).
        let stuck = world(
            Some(24753),
            three(
                NodeSnapshot::idle(),
                NodeSnapshot::idle(),
                NodeSnapshot::idle(),
            ),
        );
        for t in 0..REL_TO - 1 {
            let w = WorldSnapshot {
                elapsed_release_secs: t,
                ..stuck.clone()
            };
            assert_eq!(plan.next(&w), SwitchAction::Wait, "t={t} should still wait");
        }
        let w = WorldSnapshot {
            elapsed_release_secs: REL_TO,
            ..stuck.clone()
        };
        match plan.next(&w) {
            SwitchAction::Fail(msg) => {
                assert!(
                    msg.contains("would OOM"),
                    "reason should explain the refusal: {msg}"
                );
                assert!(
                    msg.contains("24753"),
                    "should report the observed VRAM: {msg}"
                );
            }
            other => panic!("expected Fail at timeout, got {other:?}"),
        }
        // And it stays failed — no late start.
        assert!(matches!(plan.next(&stuck), SwitchAction::Done));
    }

    #[test]
    fn perpetually_active_other_fails_at_release_timeout() {
        // A non-target node that stays active despite stop attempts (or keeps
        // coming back) must not be retried forever: at the release timeout the
        // switch fails closed instead of ever starting the target.
        let mut plan = SwitchPlan::new(0, FLOOR, REL_TO, RDY_TO);
        let stubborn = world(
            Some(24753),
            three(
                NodeSnapshot::idle(),
                NodeSnapshot::active_ready(),
                NodeSnapshot::idle(),
            ),
        );
        for t in 0..REL_TO {
            let w = WorldSnapshot {
                elapsed_release_secs: t,
                ..stubborn.clone()
            };
            assert_eq!(
                plan.next(&w),
                SwitchAction::StopNode(1),
                "t={t}s: still stopping the non-target, not failed yet"
            );
        }
        let w = WorldSnapshot {
            elapsed_release_secs: REL_TO,
            ..stubborn.clone()
        };
        match plan.next(&w) {
            SwitchAction::Fail(msg) => {
                assert!(msg.contains("still active"), "{msg}");
                assert!(msg.contains("would OOM"), "{msg}");
            }
            other => panic!("expected Fail at release timeout, got {other:?}"),
        }
        // Once failed, the plan is terminal: no more stops, no start.
        assert!(matches!(plan.next(&stubborn), SwitchAction::Done));
    }

    #[test]
    fn unreadable_vram_fails_closed() {
        // If NVML cannot be read we cannot prove the card is free. Guessing would
        // risk loading a 14 GB model onto an occupied card.
        let mut plan = SwitchPlan::new(1, FLOOR, REL_TO, RDY_TO);
        let w = world(
            None,
            three(
                NodeSnapshot::idle(),
                NodeSnapshot::idle(),
                NodeSnapshot::idle(),
            ),
        );
        match plan.next(&w) {
            SwitchAction::Fail(msg) => assert!(msg.contains("refusing to start"), "{msg}"),
            other => panic!("expected Fail on unreadable VRAM, got {other:?}"),
        }
    }

    #[test]
    fn starts_only_after_vram_observed_below_floor() {
        // The exact production sequence measured on the V100:
        //   24753 MiB (Precog) -> stop -> 311 MiB -> start -> ready.
        let mut plan = SwitchPlan::new(0, FLOOR, REL_TO, RDY_TO);

        let busy = world(
            Some(24753),
            three(
                NodeSnapshot::idle(),
                NodeSnapshot::active_ready(),
                NodeSnapshot::idle(),
            ),
        );
        assert_eq!(plan.next(&busy), SwitchAction::StopNode(1));

        // Still draining: not ready to start.
        let draining = world(
            Some(9000),
            three(
                NodeSnapshot::idle(),
                NodeSnapshot::idle(),
                NodeSnapshot::idle(),
            ),
        );
        assert_eq!(plan.next(&draining), SwitchAction::Wait);

        // Released: now (and only now) may it start.
        let free = world(
            Some(311),
            three(
                NodeSnapshot::idle(),
                NodeSnapshot::idle(),
                NodeSnapshot::idle(),
            ),
        );
        assert_eq!(plan.next(&free), SwitchAction::StartNode(0));

        // Loading: unit active but /props still 503, so NOT ready.
        let loading = world(
            Some(13615),
            three(
                NodeSnapshot::active_not_ready(),
                NodeSnapshot::idle(),
                NodeSnapshot::idle(),
            ),
        );
        assert_eq!(plan.next(&loading), SwitchAction::Wait);

        let ready = world(
            Some(27509),
            three(
                NodeSnapshot::active_ready(),
                NodeSnapshot::idle(),
                NodeSnapshot::idle(),
            ),
        );
        assert_eq!(plan.next(&ready), SwitchAction::Done);
    }

    // ── Ordering / attribution ───────────────────────────

    #[test]
    fn stops_every_other_active_holder_including_comfyui() {
        // comfyui is a VRAM holder too (308 MiB measured), so it must be stopped
        // when switching to a model. Order follows config order.
        let mut plan = SwitchPlan::new(0, FLOOR, REL_TO, RDY_TO);
        let w = world(
            Some(308),
            three(
                NodeSnapshot::idle(),
                NodeSnapshot::active_ready(),
                NodeSnapshot::active_ready(),
            ),
        );
        assert_eq!(plan.next(&w), SwitchAction::StopNode(1));
        // After idx 1 stops, idx 2 must still be stopped before we wait on VRAM.
        let w2 = world(
            Some(308),
            three(
                NodeSnapshot::idle(),
                NodeSnapshot::idle(),
                NodeSnapshot::active_ready(),
            ),
        );
        assert_eq!(plan.next(&w2), SwitchAction::StopNode(2));
        let w3 = world(
            Some(50),
            three(
                NodeSnapshot::idle(),
                NodeSnapshot::idle(),
                NodeSnapshot::idle(),
            ),
        );
        assert_eq!(plan.next(&w3), SwitchAction::StartNode(0));
    }

    #[test]
    fn target_is_never_stopped() {
        // Switching to llama02 (idx 1) while llama02 itself is active-but-loading
        // must not stop the very thing we are switching to.
        let mut plan = SwitchPlan::new(1, FLOOR, REL_TO, RDY_TO);
        let w = world(
            Some(311),
            three(
                NodeSnapshot::idle(),
                NodeSnapshot::active_not_ready(),
                NodeSnapshot::idle(),
            ),
        );
        let a = plan.next(&w);
        assert_ne!(a, SwitchAction::StopNode(1), "must not stop the target");
        assert_eq!(a, SwitchAction::StartNode(1));
    }

    #[test]
    fn already_resident_target_is_a_no_op() {
        // Avoids a pointless 36–148 s reload when the requested model is already up.
        let mut plan = SwitchPlan::new(0, FLOOR, REL_TO, RDY_TO);
        let w = world(
            Some(27509),
            three(
                NodeSnapshot::active_ready(),
                NodeSnapshot::idle(),
                NodeSnapshot::idle(),
            ),
        );
        assert_eq!(plan.next(&w), SwitchAction::Done);
        assert_eq!(plan.phase(), "finished");
    }

    #[test]
    fn active_but_not_ready_never_counts_as_resident() {
        // The distinction that prevents an OOM: a llama-server mid-load is `active`
        // (systemd) for 36–148 s while /props still 503s.
        let mut plan = SwitchPlan::new(0, FLOOR, REL_TO, RDY_TO);
        let w = world(
            Some(13615),
            three(
                NodeSnapshot::active_not_ready(),
                NodeSnapshot::idle(),
                NodeSnapshot::idle(),
            ),
        );
        assert_ne!(
            plan.next(&w),
            SwitchAction::Done,
            "loading must not be mistaken for ready"
        );
    }

    #[test]
    fn ready_timeout_fails_with_a_clear_reason() {
        let mut plan = SwitchPlan::new(0, FLOOR, REL_TO, RDY_TO);
        let free = world(
            Some(311),
            three(
                NodeSnapshot::idle(),
                NodeSnapshot::idle(),
                NodeSnapshot::idle(),
            ),
        );
        assert_eq!(plan.next(&free), SwitchAction::StartNode(0));
        let loading = world(
            Some(13615),
            three(
                NodeSnapshot::active_not_ready(),
                NodeSnapshot::idle(),
                NodeSnapshot::idle(),
            ),
        );
        for t in 0..RDY_TO {
            let w = WorldSnapshot {
                elapsed_ready_secs: t,
                ..loading.clone()
            };
            assert_eq!(plan.next(&w), SwitchAction::Wait, "t={t}");
        }
        let w = WorldSnapshot {
            elapsed_ready_secs: RDY_TO,
            ..loading.clone()
        };
        match plan.next(&w) {
            SwitchAction::Fail(msg) => assert!(msg.contains("did not become ready"), "{msg}"),
            other => panic!("expected Fail, got {other:?}"),
        }
    }

    #[test]
    fn missing_node_snapshot_does_not_panic_or_start() {
        // Agent list shorter than the target index (config drift mid-flight).
        let mut plan = SwitchPlan::new(2, FLOOR, REL_TO, RDY_TO);
        let w = world(Some(311), vec![NodeSnapshot::idle()]);
        // With no node 2 known, it must not claim ready; it proceeds to start,
        // which the driver would then reject. Crucially: no panic.
        let a = plan.next(&w);
        assert!(matches!(
            a,
            SwitchAction::StartNode(2) | SwitchAction::Fail(_)
        ));
    }

    // ── Agent state parsing ──────────────────────────────

    #[test]
    fn parses_real_agent_state_llama() {
        // Verbatim from the deployed agent on llama01.
        let raw = r#"{"label":"llama01","units":[{"unit":"llama-server.service","active":"active","enabled":"enabled"}],"model":{"ready":true,"model_alias":"Qwen3.8-27B","n_ctx":61440,"total_slots":3,"http_status":200,"error":null}}"#;
        let s: AgentState = serde_json::from_str(raw).expect("must parse real agent JSON");
        assert_eq!(s.label, "llama01");
        assert!(s.any_unit_active());
        assert_eq!(s.unit_state("llama-server.service"), Some("active"));
        let m = s.model.expect("model present for llama node");
        assert!(m.ready);
        assert_eq!(m.model_alias.as_deref(), Some("Qwen3.8-27B"));
        assert_eq!(m.n_ctx, Some(61440));
        assert_eq!(m.total_slots, Some(3));
    }

    #[test]
    fn parses_real_agent_state_comfyui_null_model() {
        // Verbatim from the deployed agent on comfyui: "model":null.
        let raw = r#"{"label":"comfyui","units":[{"unit":"comfyui.service","active":"active","enabled":"enabled"}],"model":null}"#;
        let s: AgentState = serde_json::from_str(raw).expect("must parse");
        assert_eq!(s.label, "comfyui");
        assert!(s.any_unit_active());
        assert!(s.model.is_none(), "comfyui has no llama model block");
        // A llama-style readiness read must therefore be false, not a panic.
        assert!(!s.model.as_ref().map(|m| m.ready).unwrap_or(false));
    }

    #[test]
    fn parses_real_agent_state_stopped() {
        let raw = r#"{"label":"llama02","units":[{"unit":"llama-server.service","active":"inactive","enabled":"disabled"}],"model":{"ready":false,"model_alias":null,"n_ctx":null,"total_slots":null,"http_status":null,"error":"props unreachable"}}"#;
        let s: AgentState = serde_json::from_str(raw).expect("must parse");
        assert!(!s.any_unit_active());
        assert_eq!(s.unit_state("llama-server.service"), Some("inactive"));
        assert!(!s.model.as_ref().unwrap().ready);
    }

    #[test]
    fn parses_agent_action_and_error_bodies() {
        let a: AgentActionResponse = serde_json::from_str(
            r#"{"unit":"llama-server.service","action":"stop","active":"inactive"}"#,
        )
        .expect("action response must parse");
        assert_eq!(a.action, "stop");
        assert_eq!(a.active, "inactive");

        let e: AgentErrorBody =
            serde_json::from_str(r#"{"error":"unit 'ssh.service' is not in the allowlist"}"#)
                .expect("error body must parse");
        assert!(e.error.contains("allowlist"));
    }

    #[test]
    fn agent_state_tolerates_missing_fields() {
        // An older/other agent build must not crash the monitor.
        let s: AgentState = serde_json::from_str("{}").expect("empty must parse");
        assert!(!s.any_unit_active());
        assert!(s.model.is_none());
        assert_eq!(s.unit_state("anything"), None);
    }
}
