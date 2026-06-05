//! Integration test harness — spins up mock services, runs a refresh cycle,
//! and verifies end-to-end flow: Ollama response -> GPU query -> DB persist -> dashboard API.

use std::time::Duration;

use crate::api;
use crate::config::Config;
use crate::db;
use crate::models::GpuMetric;
use axum::body::Body;
use axum::http::header::CONTENT_TYPE;
use axum::response::Response;
use axum::routing::get;

// ─── Mock Ollama server ───────────────────────────────────

/// Start a lightweight axum "Ollama" server on a random localhost port.
async fn start_mock_ollama() -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind mock ollama");
    let addr = listener.local_addr().unwrap();

    let tags_handler = get(|| async {
        let body = serde_json::json!({
            "models": [
                { "name": "llama3:8b", "size": 4700000000i64, "digest": "sha256:abc123", "modified_at": "2025-01-01T00:00:00Z" },
                { "name": "mistral:7b", "size": 4100000000i64, "digest": "sha256:def456", "modified_at": "2025-02-01T00:00:00Z" }
            ]
        });
        Response::builder()
            .status(200)
            .header(CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_string()))
            .unwrap()
    });

    let router = axum::Router::new().route("/api/tags", tags_handler);

    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });

    format!("http://{}", addr)
}

// ─── Mock GPU providers ───────────────────────────────────

/// Deterministic GPU metric for tests.
fn mock_gpu_query(_device_index: usize) -> GpuMetric {
    GpuMetric {
        name: Some("NVIDIA GeForce RTX 3080".into()),
        temperature_c: Some(67.5),
        memory_used_mib: Some(6144),
        memory_total_mib: Some(10240),
        memory_remaining_mib: Some(4096),
        utilization_pct: Some(82.0),
        power_watts: Some(245.0),
    }
}

/// GPU query that returns all-None (simulates no GPU present).
fn mock_no_gpu(_device_index: usize) -> GpuMetric {
    GpuMetric::placeholder()
}

// ─── Integration tests ────────────────────────────────────

/// Full pipeline: mock Ollama -> refresh cycle -> DB persist -> verify DB + all API endpoints.
#[tokio::test]
async fn test_full_refresh_pipeline() {
    let ollama_url = start_mock_ollama().await;
    tokio::time::sleep(Duration::from_millis(100)).await;

    let pool = db::open_pool(":memory:").await.unwrap();
    db::migrate(&pool).await.unwrap();
    let state = api::AppState::new(pool.clone());

    let config = Config {
        ollama_host: ollama_url.clone(),
        ollama_port: 0,
        server_bind: "127.0.0.1".into(),
        server_port: 0,
        refresh_interval_secs: 1,
        gpu_device_index: 0,
    };

    api::run_one_refresh(&config, &state, mock_gpu_query).await;

    // --- Verify DB row ---
    let db_results = db::query_check_results(&pool).await.unwrap();
    assert_eq!(db_results.len(), 1);
    let row = &db_results[0];
    assert!(row.ollama_reachable);
    assert_eq!(row.loaded_model, Some("llama3:8b".into()));
    assert_eq!(row.available_models_count, 2);
    assert_eq!(row.gpu_name, Some("NVIDIA GeForce RTX 3080".into()));
    assert_eq!(row.gpu_temperature_c, Some(67.5));
    assert_eq!(row.gpu_memory_used_mib, Some(6144));
    assert_eq!(row.gpu_utilization_pct, Some(82.0));
    assert_eq!(row.gpu_power_watts, Some(245.0));

    // --- Verify /api/status ---
    let http_router = api::build_router(state.clone()).await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let http_addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, http_router).await.unwrap();
    });
    tokio::time::sleep(Duration::from_millis(50)).await;

    let client = reqwest::Client::new();

    let resp = client
        .get(format!("http://{}/api/status", http_addr))
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success());
    let status: serde_json::Value = resp.json().await.unwrap();
    assert!(status["ollama_reachable"].as_bool().unwrap());
    assert_eq!(status["loaded_model"].as_str().unwrap(), "llama3:8b");
    assert_eq!(status["gpu"]["temperature_c"].as_f64().unwrap(), 67.5);
    assert_eq!(status["gpu"]["memory_used_mib"].as_u64().unwrap(), 6144);
    assert_eq!(
        status["gpu"]["name"].as_str().unwrap(),
        "NVIDIA GeForce RTX 3080"
    );

    // --- Verify /api/gpu ---
    let resp = client
        .get(format!("http://{}/api/gpu", http_addr))
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success());
    let gpu_resp: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(gpu_resp["gpu"]["temperature_c"].as_f64().unwrap(), 67.5);

    // --- Verify /api/models ---
    let resp = client
        .get(format!("http://{}/api/models", http_addr))
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success());
    let models: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(models["loaded_model"].as_str().unwrap(), "llama3:8b");
    assert_eq!(models["total_count"].as_u64().unwrap(), 2);

    // --- Verify dashboard HTML ---
    let resp = client
        .get(format!("http://{}", http_addr))
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success());
    assert!(resp.text().await.unwrap().contains("Ollama Monitor"));
}

/// Multiple refresh cycles accumulate separate DB rows.
#[tokio::test]
async fn test_multiple_refresh_cycles_accumulate() {
    let ollama_url = start_mock_ollama().await;
    tokio::time::sleep(Duration::from_millis(100)).await;

    let pool = db::open_pool(":memory:").await.unwrap();
    db::migrate(&pool).await.unwrap();
    let state = api::AppState::new(pool.clone());

    let config = Config {
        ollama_host: ollama_url.clone(),
        ollama_port: 0,
        server_bind: "127.0.0.1".into(),
        server_port: 0,
        refresh_interval_secs: 1,
        gpu_device_index: 0,
    };

    for _ in 0..3 {
        api::run_one_refresh(&config, &state, mock_gpu_query).await;
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    let results = db::query_check_results(&pool).await.unwrap();
    assert_eq!(results.len(), 3);
    for row in &results {
        assert_eq!(row.loaded_model, Some("llama3:8b".into()));
        assert_eq!(row.available_models_count, 2);
    }
}

/// When Ollama is unreachable, ollama_reachable should be false.
#[tokio::test]
async fn test_unreachable_ollama() {
    let config = Config {
        ollama_host: "http://127.0.0.1:59999".into(),
        ollama_port: 0,
        server_bind: "127.0.0.1".into(),
        server_port: 0,
        refresh_interval_secs: 1,
        gpu_device_index: 0,
    };

    let pool = db::open_pool(":memory:").await.unwrap();
    db::migrate(&pool).await.unwrap();
    let state = api::AppState::new(pool.clone());

    api::run_one_refresh(&config, &state, mock_gpu_query).await;

    let results = db::query_check_results(&pool).await.unwrap();
    assert_eq!(results.len(), 1);
    let row = &results[0];
    assert!(!row.ollama_reachable);
    assert_eq!(row.loaded_model, None);
    assert_eq!(row.available_models_count, 0);
    // GPU data still present from mock.
    assert_eq!(row.gpu_name, Some("NVIDIA GeForce RTX 3080".into()));
}

/// When no GPU is present, DB row has nulls for GPU fields.
#[tokio::test]
async fn test_no_gpu_placeholder() {
    let config = Config {
        ollama_host: "http://127.0.0.1:59998".into(),
        ollama_port: 0,
        server_bind: "127.0.0.1".into(),
        server_port: 0,
        refresh_interval_secs: 1,
        gpu_device_index: 0,
    };

    let pool = db::open_pool(":memory:").await.unwrap();
    db::migrate(&pool).await.unwrap();
    let state = api::AppState::new(pool.clone());

    api::run_one_refresh(&config, &state, mock_no_gpu).await;

    let results = db::query_check_results(&pool).await.unwrap();
    let row = &results[0];
    assert_eq!(row.gpu_name, None);
    assert_eq!(row.gpu_temperature_c, None);
    assert_eq!(row.gpu_memory_used_mib, None);
    assert_eq!(row.gpu_utilization_pct, None);
    assert_eq!(row.gpu_power_watts, None);
}
