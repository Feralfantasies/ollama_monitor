/// Ollama Monitor — main entry point.

mod api;
mod config;
mod gpu;
mod models;
mod ollama;

use anyhow::Result;
use tracing_subscriber::{fmt, EnvFilter};

#[tokio::main]
async fn main() -> Result<()> {
    let env_filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info"));
    fmt::Subscriber::builder()
        .with_env_filter(env_filter)
        .init();

    let config = config::Config::load();
    tracing::info!(?config, "Configuration loaded");

    let state = api::AppState::default();

    let cfg_clone = config.clone();
    let state_clone = state.clone();
    tokio::spawn(async move {
        api::run_refresh_loop(&cfg_clone, state_clone).await;
    });

    let addr = api::bind_listener(&config).await?;
    let app = api::build_router(state).await;

    tracing::info!("Starting Ollama Monitor on {:?}/api/status", addr);
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    Ok(axum::serve(listener, app).await?)
}
