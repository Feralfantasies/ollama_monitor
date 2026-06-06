//! Integration test harness - spins up a mock Ollama HTTP server and a mock
//! `nvidia-smi` CLI binary. A copy of the real refresh loop runs against them
//! and we verify the DB history it produces.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::api;
use crate::config::Config;
use crate::db;
use crate::gpu::GpuQueryFn;
use crate::models::GpuMetric;
use axum::body::Body;
use axum::http::header::CONTENT_TYPE;
use axum::response::Response;
use axum::routing::get;
use sqlx::sqlite::SqliteConnectOptions;

// - Helpers -----------------------------------------------------------

/// Open a single-connection SQLite pool on a temp file (isolated per call).
async fn open_test_pool() -> sqlx::SqlitePool {
    let dir = std::env::temp_dir();
    let path = dir.join(format!(
        "ollama_monitor_integ_{}_{}.db",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let options = SqliteConnectOptions::new().filename(&path).create_if_missing(true);
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await
        .unwrap();
    db::migrate(&pool).await.unwrap();
    pool
}

// - Mock Ollama HTTP server --------------------------------------------

/// Start a lightweight axum "Ollama" server on a random localhost port.
async fn start_mock_ollama() -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind mock ollama");
    let addr = listener.local_addr().unwrap();

    let tags_handler = get(|| async {
        let body = serde_json::json!({
            "models": [
                { "name": "llama3:8b", "size": 4_700_000_000i64,
                  "digest": "sha256:abc123", "modified_at": "2025-01-01T00:00:00Z" },
                { "name": "mistral:7b", "size": 4_100_000_000i64,
                  "digest": "sha256:def456", "modified_at": "2025-02-01T00:00:00Z" }
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

// - Mock nvidia-smi binary ---------------------------------------------

/// CSV line the mock nvidia-smi should print.
const DEFAULT_GPU_CSV: &str =
    "0, NVIDIA GeForce RTX 3080, 67.5, 6144, 10240, 82.0, 245.0\n";

/// Write a shell script named `nvidia-smi` to a temp dir.
fn create_mock_nvidia_smi(csv_output: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "mock_nvidia_smi_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();

    let script_path = dir.join("nvidia-smi");
    let mut file = std::fs::File::create(&script_path).unwrap();
    writeln!(file, "#!/bin/sh").unwrap();
    writeln!(file, "echo '{}'", csv_output).unwrap();
    file.flush().unwrap();

    let mut perms = script_path.metadata().unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&script_path, perms).unwrap();
    dir
}

/// GPU-query fn that calls the mock nvidia-smi at the given directory.
fn mock_gpu_with_bin(bin_dir: &Path) -> GpuQueryFn {
    let bin_str = bin_dir.join("nvidia-smi").to_string_lossy().to_string();
    move |idx: usize| -> GpuMetric {
        match crate::gpu::query_gpu_bin(&bin_str, idx) {
            Ok(m) => m,
            Err(e) => {
                tracing::warn!("mock nvidia-smi failed: {}", e);
                GpuMetric::placeholder()
            }
        }
    }
}

/// Build a Config pointing at the mock Ollama server.
fn cfg_for_ollama(ollama_url: &str) -> Config {
    Config {
        ollama_host: ollama_url.to_string(),
        ollama_port: 0,
        server_bind: "127.0.0.1".into(),
        server_port: 0,
        refresh_interval_secs: 1,
        gpu_device_index: 0,
    }
}

// - Integration tests --------------------------------------------------

/// Mock Ollama HTTP + mock nvidia-smi CLI.  Runs 3 refresh cycles and
/// verifies the DB holds 3 rows plus the history query returns them.
#[tokio::test]
async fn test_full_pipeline_history_accumulates() {
    let ollama_url = start_mock_ollama().await;
    tokio::time::sleep(Duration::from_millis(100)).await;

    let mock_nvidia_dir = create_mock_nvidia_smi(DEFAULT_GPU_CSV);
    let gpu_fn = mock_gpu_with_bin(&mock_nvidia_dir);

    let pool = open_test_pool().await;
    let state = api::AppState::new(pool.clone());
    let config = cfg_for_ollama(&ollama_url);

    for _ in 0..3 {
        api::run_one_refresh(&config, &state, gpu_fn).await;
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    let db_results = db::query_check_results(&pool).await.unwrap();
    assert_eq!(db_results.len(), 3, "expected 3 check_result rows");

    for row in &db_results {
        assert!(row.ollama_reachable);
        assert_eq!(row.loaded_model, Some("llama3:8b".into()));
        assert_eq!(row.available_models_count, 2);
        assert_eq!(row.gpu_name, Some("NVIDIA GeForce RTX 3080".into()));
        assert_eq!(row.gpu_temperature_c, Some(67.5));
        assert_eq!(row.gpu_memory_used_mib, Some(6144));
        assert_eq!(row.gpu_memory_total_mib, Some(10240));
        assert_eq!(row.gpu_utilization_pct, Some(82.0));
        assert_eq!(row.gpu_power_watts, Some(245.0));
    }

    let history = db::query_history(&pool, db::HistoryRange::Last15Minutes).await.unwrap();
    assert_eq!(history.len(), 3, "history should have 3 points");
    for point in &history {
        assert_eq!(point.memory_used_mib, Some(6144));
        assert_eq!(point.temperature_c, Some(67.5));
    }

    let latest = state.latest_status.read().await;
    let latest = latest.as_ref().expect("status should be populated");
    assert!(latest.ollama_reachable);
    assert_eq!(latest.loaded_model, Some("llama3:8b".into()));
    assert_eq!(latest.available_models.len(), 2);
    assert_eq!(latest.gpu.name, Some("NVIDIA GeForce RTX 3080".into()));

    let _ = std::fs::remove_dir_all(&mock_nvidia_dir);
}

/// History rows are in ASC order with strictly increasing timestamps.
#[tokio::test]
async fn test_history_timestamps_ordered() {
    let ollama_url = start_mock_ollama().await;
    tokio::time::sleep(Duration::from_millis(100)).await;

    let mock_nvidia_dir = create_mock_nvidia_smi(DEFAULT_GPU_CSV);
    let gpu_fn = mock_gpu_with_bin(&mock_nvidia_dir);

    let pool = open_test_pool().await;
    let state = api::AppState::new(pool.clone());
    let config = cfg_for_ollama(&ollama_url);

    for _ in 0..4 {
        api::run_one_refresh(&config, &state, gpu_fn).await;
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    let history = db::query_history(&pool, db::HistoryRange::Last15Minutes).await.unwrap();
    assert_eq!(history.len(), 4, "expected 4 history points");

    for i in 1..history.len() {
        assert!(
            history[i].timestamp > history[i - 1].timestamp,
            "timestamps should be strictly ascending"
        );
    }

    let _ = std::fs::remove_dir_all(&mock_nvidia_dir);
}

/// No mock Ollama server - Ollama marked unreachable, GPU still recorded.
#[tokio::test]
async fn test_unreachable_ollama_records_correctly() {
    let mock_nvidia_dir = create_mock_nvidia_smi(DEFAULT_GPU_CSV);
    let gpu_fn = mock_gpu_with_bin(&mock_nvidia_dir);

    let pool = open_test_pool().await;
    let state = api::AppState::new(pool.clone());

    let config = Config {
        ollama_host: "http://127.0.0.1:59999".into(),
        ollama_port: 0,
        server_bind: "127.0.0.1".into(),
        server_port: 0,
        refresh_interval_secs: 1,
        gpu_device_index: 0,
    };

    api::run_one_refresh(&config, &state, gpu_fn).await;

    let results = db::query_check_results(&pool).await.unwrap();
    assert_eq!(results.len(), 1);
    let row = &results[0];

    assert!(!row.ollama_reachable);
    assert_eq!(row.loaded_model, None);
    assert_eq!(row.available_models_count, 0);
    assert_eq!(row.gpu_name, Some("NVIDIA GeForce RTX 3080".into()));
    assert_eq!(row.gpu_temperature_c, Some(67.5));
    assert_eq!(row.gpu_memory_used_mib, Some(6144));

    let _ = std::fs::remove_dir_all(&mock_nvidia_dir);
}

/// GPU callback returns placeholder.  Ollama data still recorded.
#[tokio::test]
async fn test_no_gpu_records_nulls() {
    let ollama_url = start_mock_ollama().await;
    tokio::time::sleep(Duration::from_millis(100)).await;

    let no_gpu_fn: GpuQueryFn = |_: usize| GpuMetric::placeholder();

    let pool = open_test_pool().await;
    let state = api::AppState::new(pool.clone());
    let config = cfg_for_ollama(&ollama_url);

    api::run_one_refresh(&config, &state, no_gpu_fn).await;

    let results = db::query_check_results(&pool).await.unwrap();
    assert_eq!(results.len(), 1);
    let row = &results[0];

    assert!(row.ollama_reachable);
    assert_eq!(row.loaded_model, Some("llama3:8b".into()));
    assert_eq!(row.gpu_name, None);
    assert_eq!(row.gpu_temperature_c, None);
    assert_eq!(row.gpu_memory_used_mib, None);
    assert_eq!(row.gpu_memory_total_mib, None);
    assert_eq!(row.gpu_utilization_pct, None);
    assert_eq!(row.gpu_power_watts, None);
}

/// Mixed GPU availability: 2 good cycles, 1 no-GPU, 1 good again.
#[tokio::test]
async fn test_mixed_gpu_availability() {
    let ollama_url = start_mock_ollama().await;
    tokio::time::sleep(Duration::from_millis(100)).await;

    let good_dir = create_mock_nvidia_smi(DEFAULT_GPU_CSV);
    let good_fn = mock_gpu_with_bin(&good_dir);
    let no_gpu_fn: GpuQueryFn = |_: usize| GpuMetric::placeholder();

    let pool = open_test_pool().await;
    let state = api::AppState::new(pool.clone());
    let config = cfg_for_ollama(&ollama_url);

    api::run_one_refresh(&config, &state, good_fn).await;
    tokio::time::sleep(Duration::from_millis(10)).await;
    api::run_one_refresh(&config, &state, good_fn).await;
    tokio::time::sleep(Duration::from_millis(10)).await;
    api::run_one_refresh(&config, &state, no_gpu_fn).await;
    tokio::time::sleep(Duration::from_millis(10)).await;
    api::run_one_refresh(&config, &state, good_fn).await;

    let results = db::query_check_results(&pool).await.unwrap();
    assert_eq!(results.len(), 4);

    // DESC by id: 4=good, 3=none, 2=good, 1=good
    assert_eq!(results[0].gpu_name, Some("NVIDIA GeForce RTX 3080".into()));
    assert_eq!(results[1].gpu_name, None);
    assert_eq!(results[2].gpu_name, Some("NVIDIA GeForce RTX 3080".into()));
    assert_eq!(results[3].gpu_name, Some("NVIDIA GeForce RTX 3080".into()));

    for row in &results {
        assert!(row.ollama_reachable);
        assert_eq!(row.loaded_model, Some("llama3:8b".into()));
    }

    let _ = std::fs::remove_dir_all(&good_dir);
}

/// Dashboard HTTP end-to-end: refresh cycles then verify all API endpoints.
#[tokio::test]
async fn test_dashboard_api_endpoints() {
    let ollama_url = start_mock_ollama().await;
    tokio::time::sleep(Duration::from_millis(100)).await;

    let mock_nvidia_dir = create_mock_nvidia_smi(DEFAULT_GPU_CSV);
    let gpu_fn = mock_gpu_with_bin(&mock_nvidia_dir);

    let pool = open_test_pool().await;
    let state = api::AppState::new(pool.clone());
    let config = cfg_for_ollama(&ollama_url);

    for _ in 0..2 {
        api::run_one_refresh(&config, &state, gpu_fn).await;
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    let router = api::build_router(state.clone()).await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let http_addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap(); });
    tokio::time::sleep(Duration::from_millis(50)).await;

    let client = reqwest::Client::new();
    let base = format!("http://{}", http_addr);

    let resp = client.get(format!("{}/api/status", base)).send().await.unwrap();
    assert!(resp.status().is_success());
    let status: serde_json::Value = resp.json().await.unwrap();
    assert!(status["ollama_reachable"].as_bool().unwrap());
    assert_eq!(status["loaded_model"].as_str().unwrap(), "llama3:8b");
    assert_eq!(status["gpu"]["temperature_c"].as_f64().unwrap(), 67.5);

    let resp = client.get(format!("{}/api/gpu", base)).send().await.unwrap();
    assert!(resp.status().is_success());
    let gpu: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(gpu["gpu"]["temperature_c"].as_f64().unwrap(), 67.5);

    let resp = client.get(format!("{}/api/models", base)).send().await.unwrap();
    assert!(resp.status().is_success());
    let models: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(models["total_count"].as_u64().unwrap(), 2);

    let resp = client
        .get(format!("{}/api/history?range=15m", base))
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success());
    let history: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(history["points"].as_array().unwrap().len(), 2);

    let resp = client.get(&base).send().await.unwrap();
    assert!(resp.status().is_success());
    assert!(resp.text().await.unwrap().contains("Ollama Monitor"));

    let _ = std::fs::remove_dir_all(&mock_nvidia_dir);
}
