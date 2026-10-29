//! Switch agent — minimal, token-authenticated systemd unit controller.
//!
//! Deployed inside each GPU workload container (`llama01`, `llama02`,
//! `comfyui`) so the central monitor can stop one workload before starting
//! another. Only one model can ever be VRAM-resident on the single V100, so
//! the monitor must be able to stop-then-start reliably; this agent is the
//! controlled way to do that without handing out SSH or shell access.
//!
//! # Security model
//!
//! The ONLY operations exposed are `systemctl start` / `systemctl stop` and
//! read-only `is-active` / `is-enabled`, and only for unit names that appear
//! verbatim in `SWITCH_AGENT_ALLOWED_UNITS`. There is no arbitrary command
//! execution path: unit names are passed as discrete `Command` arguments (never
//! through a shell) after an exact allowlist match plus a charset check, and the
//! string handed to `systemctl` always comes from the allowlist, never from the
//! request body.
//!
//! Every route except `/health` requires `Authorization: Bearer <token>`,
//! compared without short-circuiting. The agent refuses to start at all if no
//! token or no allowlist is configured (fail closed).
//!
//! # Configuration (environment)
//!
//! | Variable | Default | Meaning |
//! |---|---|---|
//! | `SWITCH_AGENT_TOKEN` | *(required)* | Bearer token; agent exits if unset/empty |
//! | `SWITCH_AGENT_ALLOWED_UNITS` | *(required)* | Comma-separated unit allowlist |
//! | `SWITCH_AGENT_BIND` | `0.0.0.0` | Bind address (set to the LAN IP in the unit file) |
//! | `SWITCH_AGENT_PORT` | `8100` | Listen port |
//! | `SWITCH_AGENT_LABEL` | `hostname` output, else `unknown` | Name reported in `/state` |
//! | `SWITCH_AGENT_PROPS_URL` | *(unset)* | llama-server `/props` URL; when set, `/state` also reports model readiness |

use anyhow::{Context, Result};
use axum::{
    extract::State,
    http::{header::AUTHORIZATION, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::process::Command;
use std::sync::Arc;
use tracing::{info, warn};

/// Concrete error response type, so helpers can be used in `Result` positions.
type ApiError = (StatusCode, Json<ErrorBody>);

/// Shared, immutable application state.
#[derive(Clone)]
struct AppState {
    token: Arc<String>,
    allowed: Arc<Vec<String>>,
    label: String,
    props_url: Option<String>,
    http: reqwest::Client,
    unit_re: Arc<Regex>,
}

/// One systemd unit's observed state.
#[derive(Debug, Serialize)]
struct UnitState {
    unit: String,
    /// `systemctl is-active` output.
    active: String,
    /// `systemctl is-enabled` output.
    enabled: String,
}

/// Model readiness, derived from llama-server `/props` when configured.
#[derive(Debug, Serialize)]
struct ModelState {
    /// true only when `/props` answered HTTP 200 (model fully loaded).
    ///
    /// llama-server answers 503 while a model is still loading, so "process is
    /// running" is NOT sufficient — acting on it would start a second model
    /// before VRAM is free and cause a CUDA OOM.
    ready: bool,
    model_alias: Option<String>,
    n_ctx: Option<u64>,
    total_slots: Option<u64>,
    /// Raw `/props` HTTP status, or None if unreachable.
    http_status: Option<u16>,
    error: Option<String>,
}

/// Response body for `GET /state`.
#[derive(Debug, Serialize)]
struct StateResponse {
    label: String,
    units: Vec<UnitState>,
    model: Option<ModelState>,
}

/// Request body for `POST /start` and `POST /stop`.
#[derive(Debug, Deserialize)]
struct UnitRequest {
    unit: String,
}

/// Response body for `POST /start` and `POST /stop`.
#[derive(Debug, Serialize)]
struct ActionResponse {
    unit: String,
    action: String,
    active: String,
}

/// Standard error envelope.
#[derive(Debug, Serialize)]
struct ErrorBody {
    error: String,
}

fn err(status: StatusCode, msg: impl Into<String>) -> ApiError {
    (status, Json(ErrorBody { error: msg.into() }))
}

/// Length-independent comparison that never short-circuits on a mismatch.
///
/// Avoids pulling in `subtle` for one comparison. This is not a hard guarantee
/// against a local timing attacker, but it removes the obvious byte-by-byte
/// early-exit signal and does not leak token length via an early return.
fn token_matches(provided: &str, expected: &str) -> bool {
    let p = provided.as_bytes();
    let e = expected.as_bytes();
    // Full-width usize XOR: truncating the length difference to u8 would let
    // any input longer than the token by a multiple of 256 compare equal
    // (prefix matches byte-for-byte, length diff wraps to 0).
    let mut diff = p.len() ^ e.len();
    for (i, b) in e.iter().enumerate() {
        let a = p.get(i).copied().unwrap_or(0);
        diff |= (a ^ b) as usize;
    }
    diff == 0
}

/// Extract and validate the bearer token.
fn authorize(state: &AppState, headers: &HeaderMap) -> Result<(), ApiError> {
    let raw = headers
        .get(AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let token = raw.strip_prefix("Bearer ").unwrap_or("");
    if token.is_empty() {
        return Err(err(StatusCode::UNAUTHORIZED, "missing bearer token"));
    }
    if !token_matches(token, &state.token) {
        warn!("rejected request with invalid bearer token");
        return Err(err(StatusCode::FORBIDDEN, "invalid token"));
    }
    Ok(())
}

/// Run `systemctl <verb> <unit>`, treating non-zero exit as an error.
///
/// Blocking; callers run it via `spawn_blocking`.
fn systemctl(verb: &str, unit: &str) -> Result<String> {
    let out = Command::new("systemctl")
        .args([verb, unit])
        .output()
        .with_context(|| format!("failed to execute systemctl {verb} {unit}"))?;
    let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
    if !out.status.success() {
        // `is-active`/`is-enabled` legitimately exit non-zero for
        // inactive/disabled units, so those use `systemctl_status` instead.
        anyhow::bail!(
            "systemctl {verb} {unit} exited {}: {}",
            out.status,
            if stderr.is_empty() { stdout } else { stderr }
        );
    }
    Ok(stdout)
}

/// Read-only `systemctl <verb>` that tolerates non-zero exit (inactive/disabled).
fn systemctl_status(verb: &str, unit: &str) -> String {
    Command::new("systemctl")
        .args([verb, unit])
        .output()
        .map(|o| {
            let s = String::from_utf8_lossy(&o.stdout).trim().to_string();
            if s.is_empty() {
                "unknown".to_string()
            } else {
                s
            }
        })
        .unwrap_or_else(|_| "unavailable".to_string())
}

/// Validate a unit name against the allowlist and a conservative charset.
///
/// Returns the canonical allowlisted string. Because the returned value is
/// cloned from the allowlist rather than taken from the request, a crafted name
/// can never reach `systemctl`.
fn check_unit(state: &AppState, unit: &str) -> Result<String, ApiError> {
    if unit.is_empty() || unit.len() > 200 {
        return Err(err(StatusCode::BAD_REQUEST, "unit name required"));
    }
    if !state.unit_re.is_match(unit) {
        warn!("rejected unit name failing charset validation");
        return Err(err(StatusCode::BAD_REQUEST, "invalid unit name"));
    }
    match state.allowed.iter().find(|a| a.as_str() == unit) {
        Some(canonical) => Ok(canonical.clone()),
        None => {
            warn!(unit, "rejected unit not in allowlist");
            Err(err(
                StatusCode::FORBIDDEN,
                format!("unit '{unit}' is not in the allowlist"),
            ))
        }
    }
}

async fn handle_health() -> Response {
    Json(serde_json::json!({ "ok": true })).into_response()
}

async fn handle_state(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(e) = authorize(&state, &headers) {
        return e.into_response();
    }

    let allowed = state.allowed.clone();
    let units = tokio::task::spawn_blocking(move || {
        allowed
            .iter()
            .map(|u| UnitState {
                unit: u.clone(),
                active: systemctl_status("is-active", u),
                enabled: systemctl_status("is-enabled", u),
            })
            .collect::<Vec<_>>()
    })
    .await
    .unwrap_or_default();

    let model = match &state.props_url {
        Some(url) => Some(probe_model(&state, url).await),
        None => None,
    };

    Json(StateResponse {
        label: state.label.clone(),
        units,
        model,
    })
    .into_response()
}

/// Query llama-server `/props` to determine whether a model is loaded and ready.
async fn probe_model(state: &AppState, url: &str) -> ModelState {
    let not_ready = |http_status: Option<u16>, error: String| ModelState {
        ready: false,
        model_alias: None,
        n_ctx: None,
        total_slots: None,
        http_status,
        error: Some(error),
    };

    let resp = match state.http.get(url).send().await {
        Ok(r) => r,
        Err(e) => return not_ready(None, format!("props unreachable: {e}")),
    };

    let status = resp.status();
    if !status.is_success() {
        return not_ready(
            Some(status.as_u16()),
            format!("props returned HTTP {status} (model not loaded yet)"),
        );
    }

    let v: serde_json::Value = match resp.json().await {
        Ok(v) => v,
        Err(e) => return not_ready(Some(status.as_u16()), format!("props parse error: {e}")),
    };

    let dgs = v.get("default_generation_settings");
    let n_ctx = dgs.and_then(|d| d.get("n_ctx")).and_then(|x| x.as_u64());
    let alias = v
        .get("model_alias")
        .and_then(|x| x.as_str())
        .or_else(|| {
            dgs.and_then(|d| d.get("model_alias"))
                .and_then(|x| x.as_str())
        })
        .map(|s| s.to_string());
    // A 200 /props does not alone mean the model can serve: llama-server
    // keeps answering 200 while the model is asleep. Treat the model as
    // not-ready when `is_sleeping` is true; absent on older builds (awake).
    let is_sleeping = v
        .get("is_sleeping")
        .and_then(|x| x.as_bool())
        .unwrap_or(false);

    ModelState {
        ready: !is_sleeping,
        model_alias: alias,
        n_ctx,
        total_slots: v.get("total_slots").and_then(|x| x.as_u64()),
        http_status: Some(status.as_u16()),
        error: None,
    }
}

async fn handle_start(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<UnitRequest>,
) -> Response {
    unit_action(state, headers, req, "start").await
}

async fn handle_stop(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<UnitRequest>,
) -> Response {
    unit_action(state, headers, req, "stop").await
}

async fn unit_action(
    state: AppState,
    headers: HeaderMap,
    req: UnitRequest,
    action: &'static str,
) -> Response {
    if let Err(e) = authorize(&state, &headers) {
        return e.into_response();
    }
    let unit = match check_unit(&state, &req.unit) {
        Ok(u) => u,
        Err(e) => return e.into_response(),
    };

    info!(unit = %unit, action, "executing systemctl");
    let target = unit.clone();
    let result = tokio::task::spawn_blocking(move || {
        let r = systemctl(action, &target);
        let active = systemctl_status("is-active", &target);
        (r, active)
    })
    .await;

    match result {
        Ok((Ok(_), active)) => Json(ActionResponse {
            unit,
            action: action.to_string(),
            active,
        })
        .into_response(),
        Ok((Err(e), active)) => {
            warn!(unit = %unit, action, error = %e, "systemctl action failed");
            err(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("{action} failed: {e} (is-active={active})"),
            )
            .into_response()
        }
        Err(e) => err(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("blocking task join error: {e}"),
        )
        .into_response(),
    }
}

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

/// Best-effort label: env override, else `hostname(1)`, else "unknown".
///
/// Deliberately avoids a hostname crate dependency.
fn resolve_label() -> String {
    if let Ok(v) = std::env::var("SWITCH_AGENT_LABEL") {
        if !v.trim().is_empty() {
            return v.trim().to_string();
        }
    }
    Command::new("hostname")
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".to_string())
}

#[tokio::main]
async fn main() -> Result<()> {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    tracing_subscriber::fmt().with_env_filter(filter).init();

    // --- Fail closed on missing security configuration -------------------
    let token = std::env::var("SWITCH_AGENT_TOKEN").unwrap_or_default();
    if token.trim().is_empty() {
        anyhow::bail!(
            "SWITCH_AGENT_TOKEN must be set and non-empty (refusing to start unauthenticated)"
        );
    }
    let raw_units = std::env::var("SWITCH_AGENT_ALLOWED_UNITS").unwrap_or_default();
    let allowed: Vec<String> = raw_units
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    if allowed.is_empty() {
        anyhow::bail!(
            "SWITCH_AGENT_ALLOWED_UNITS must list at least one unit (refusing to start with an empty allowlist)"
        );
    }

    let unit_re =
        Regex::new(r"^[A-Za-z0-9@:_.\-]{1,200}$").context("failed to compile unit-name regex")?;
    for u in &allowed {
        if !unit_re.is_match(u) {
            anyhow::bail!("allowlisted unit name '{u}' fails charset validation");
        }
    }

    let bind = env_or("SWITCH_AGENT_BIND", "0.0.0.0");
    let port: u16 = env_or("SWITCH_AGENT_PORT", "8100")
        .parse()
        .context("SWITCH_AGENT_PORT must be a u16")?;
    let props_url = match std::env::var("SWITCH_AGENT_PROPS_URL") {
        Ok(v) if !v.trim().is_empty() => Some(v.trim().to_string()),
        _ => None,
    };
    let label = resolve_label();

    let state = AppState {
        token: Arc::new(token),
        allowed: Arc::new(allowed.clone()),
        label: label.clone(),
        props_url,
        http: reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(4))
            .build()
            .context("failed to build reqwest client")?,
        unit_re: Arc::new(unit_re),
    };

    let app = Router::new()
        .route("/health", get(handle_health))
        .route("/state", get(handle_state))
        .route("/start", post(handle_start))
        .route("/stop", post(handle_stop))
        .with_state(state);

    let addr = format!("{bind}:{port}");
    info!(
        label = %label,
        addr = %addr,
        allowed_units = ?allowed,
        "starting switch agent"
    );
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .with_context(|| format!("failed to bind {addr}"))?;
    Ok(axum::serve(listener, app).await?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_compare_matches_and_rejects() {
        assert!(token_matches("abc123", "abc123"));
        assert!(!token_matches("abc123", "abc124"));
        assert!(!token_matches("abc12", "abc123"));
        assert!(!token_matches("abc1234", "abc123"));
        assert!(!token_matches("", "abc123"));
        assert!(!token_matches("abc123", ""));
    }

    #[test]
    fn token_compare_rejects_prefix_with_256_byte_suffix() {
        // Regression for the `as u8` length truncation: the token as an exact
        // prefix plus a 256-byte multiple used to compare equal.
        assert!(token_matches("abc123", "abc123"));
        assert!(!token_matches(
            &format!("abc123{}", "a".repeat(256)),
            "abc123"
        ));
        assert!(!token_matches(
            &format!("abc123{}", "a".repeat(512)),
            "abc123"
        ));
        assert!(!token_matches(
            "abc123",
            &format!("abc123{}", "a".repeat(256))
        ));
    }

    #[test]
    fn unit_charset_rejects_injection_attempts() {
        let re = Regex::new(r"^[A-Za-z0-9@:_.\-]{1,200}$").unwrap();
        assert!(re.is_match("llama-server.service"));
        assert!(re.is_match("comfyui.service"));
        // shell metacharacters and path traversal must be rejected
        for bad in [
            "llama-server.service; rm -rf /",
            "$(reboot)",
            "a b.service",
            "../../etc/passwd",
            "unit`id`",
            "x|y",
            "",
        ] {
            assert!(!re.is_match(bad), "should have rejected: {bad:?}");
        }
    }

    #[test]
    fn allowlist_returns_canonical_not_request_string() {
        let state = AppState {
            token: Arc::new("t".into()),
            allowed: Arc::new(vec!["llama-server.service".to_string()]),
            label: "test".into(),
            props_url: None,
            http: reqwest::Client::new(),
            unit_re: Arc::new(Regex::new(r"^[A-Za-z0-9@:_.\-]{1,200}$").unwrap()),
        };
        assert_eq!(
            check_unit(&state, "llama-server.service").unwrap(),
            "llama-server.service"
        );
        // not allowlisted -> 403
        let e = check_unit(&state, "comfyui.service").unwrap_err();
        assert_eq!(e.0, StatusCode::FORBIDDEN);
        // charset failure -> 400
        let e = check_unit(&state, "llama-server.service; reboot").unwrap_err();
        assert_eq!(e.0, StatusCode::BAD_REQUEST);
    }

    #[test]
    fn authorize_requires_bearer() {
        let state = AppState {
            token: Arc::new("secret-token".into()),
            allowed: Arc::new(vec!["llama-server.service".into()]),
            label: "test".into(),
            props_url: None,
            http: reqwest::Client::new(),
            unit_re: Arc::new(Regex::new(r"^[A-Za-z0-9@:_.\-]{1,200}$").unwrap()),
        };
        let mut h = HeaderMap::new();
        // no header at all
        assert_eq!(
            authorize(&state, &h).unwrap_err().0,
            StatusCode::UNAUTHORIZED
        );
        // wrong scheme
        h.insert(AUTHORIZATION, "Basic secret-token".parse().unwrap());
        assert_eq!(
            authorize(&state, &h).unwrap_err().0,
            StatusCode::UNAUTHORIZED
        );
        // wrong token
        h.insert(AUTHORIZATION, "Bearer nope".parse().unwrap());
        assert_eq!(authorize(&state, &h).unwrap_err().0, StatusCode::FORBIDDEN);
        // correct
        h.insert(AUTHORIZATION, "Bearer secret-token".parse().unwrap());
        assert!(authorize(&state, &h).is_ok());
    }
}
