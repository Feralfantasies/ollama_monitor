/// Client for the Ollama REST API.
use anyhow::{Context, Result};
use reqwest::Client;
use std::time::Duration;
use tracing::{debug, warn};

use crate::models::OllamaTagsResponse;

pub struct OllamaClient {
    base_url: String,
    http: Client,
}

impl OllamaClient {
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into(),
            http: Client::builder()
                .timeout(Duration::from_secs(10))
                .build()
                .expect("reqwest client build"),
        }
    }

    /// Fetch available models from `/api/tags`.
    pub async fn fetch_models(&self) -> Result<OllamaTagsResponse> {
        debug!("Fetching models from {}", self.base_url);
        let url = format!("{}/api/tags", self.base_url);

        self
            .http
            .get(&url)
            .send()
            .await
            .with_context(|| format!("Failed to connect to Ollama at {}", url))?
            .json::<OllamaTagsResponse>()
            .await
            .with_context(|| "Failed to parse Ollama /api/tags response")
    }

    /// Best-effort fetch; returns None on failure instead of propagating errors.
    pub async fn try_fetch_models(&self) -> Option<OllamaTagsResponse> {
        match self.fetch_models().await {
            Ok(resp) => Some(resp),
            Err(e) => {
                warn!("Ollama connection failed: {}", e);
                None
            }
        }
    }
}
