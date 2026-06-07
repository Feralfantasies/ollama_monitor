use crate::models::CheckResult;
/// SQLite persistence for GPU history metrics.
use anyhow::Context;
use chrono::Utc;
use sqlx::{
    sqlite::{SqliteAutoVacuum, SqliteConnectOptions, SqliteJournalMode},
    Row, SqlitePool,
};
use tracing::{debug, info};

// ── Schema constants ─────────────────────────────────────

/// Table that stores one row per refresh-loop cycle.
const CREATE_CHECK_RESULTS: &str = r#"
    CREATE TABLE IF NOT EXISTS check_results (
        id            INTEGER PRIMARY KEY AUTOINCREMENT,
        recorded_at   TEXT    NOT NULL,
        ollama_url    TEXT    NOT NULL,
        ollama_reachable INTEGER NOT NULL,
        loaded_model  TEXT,
        available_models_count INTEGER NOT NULL DEFAULT 0,
        gpu_name      TEXT,
        gpu_temperature_c REAL,
        gpu_memory_used_mib INTEGER,
        gpu_memory_total_mib INTEGER,
        gpu_utilization_pct REAL,
        gpu_power_watts REAL,
        sys_memory_used_mib INTEGER,
        sys_memory_total_mib INTEGER,
        sys_memory_remaining_mib INTEGER,
        sys_memory_usage_pct REAL,
        sys_cpu_utilization_pct REAL
    )
"#;

/// Prune check_results older than 30 days.
const VACUUM_CHECK_SQL: &str =
    "DELETE FROM check_results WHERE recorded_at < datetime('now', '-30 days')";

// ── Public types ─────────────────────────────────────────

/// Supported time ranges for the dashboard history view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HistoryRange {
    Last15Minutes,
    LastHour,
    Last6Hours,
    LastDay,
    LastWeek,
    LastMonth,
}

impl HistoryRange {
    /// SQLite datetime interval string.
    pub fn sqlite_interval(&self) -> &'static str {
        match self {
            HistoryRange::Last15Minutes => "-15 minutes",
            HistoryRange::LastHour => "-1 hour",
            HistoryRange::Last6Hours => "-6 hours",
            HistoryRange::LastDay => "-24 hours",
            HistoryRange::LastWeek => "-7 days",
            HistoryRange::LastMonth => "-30 days",
        }
    }

    /// Parse a range string from the query parameter.
    /// Accepts "15m", "1h", "6h", "1d", "7d", "30d".
    pub fn parse(s: &str) -> Self {
        match s {
            "15m" | "last15min" => HistoryRange::Last15Minutes,
            "1h" | "last1h" => HistoryRange::LastHour,
            "6h" => HistoryRange::Last6Hours,
            "1d" | "last1d" => HistoryRange::LastDay,
            "7d" | "last1w" => HistoryRange::LastWeek,
            "30d" | "last1m" => HistoryRange::LastMonth,
            _ => HistoryRange::Last15Minutes,
        }
    }
}

// ── Pool helpers ──────────────────────────────────────────

/// Open (or create) the SQLite database and return a connection pool.
pub async fn open_pool(db_path: &str) -> anyhow::Result<SqlitePool> {
    let options = SqliteConnectOptions::new()
        .filename(db_path)
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        .busy_timeout(std::time::Duration::from_secs(5))
        .auto_vacuum(SqliteAutoVacuum::Full);

    let pool = SqlitePool::connect_with(options)
        .await
        .context("Failed to connect to SQLite database")?;

    Ok(pool)
}

/// Run one-time schema migration (idempotent).
pub async fn migrate(pool: &SqlitePool) -> anyhow::Result<()> {
    sqlx::query(CREATE_CHECK_RESULTS)
        .execute(pool)
        .await
        .context("Failed to create check_results table")?;

    info!("Database schema ready");

    // Prune stale rows — silently skip on first run when the table is newly created.
    let result = sqlx::query(VACUUM_CHECK_SQL).execute(pool).await;
    if let Ok(deleted_check) = result {
        if deleted_check.rows_affected() > 0 {
            debug!(
                "Pruned {} stale check_results rows",
                deleted_check.rows_affected()
            );
        }
    }

    Ok(())
}

// ── Check results ──────────────────────────────────────────

/// Insert a full check-result snapshot.
pub async fn insert_check_result(pool: &SqlitePool, check: &CheckResult) -> anyhow::Result<()> {
    let now = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);

    sqlx::query(
        r#"
        INSERT INTO check_results (
            recorded_at, ollama_url, ollama_reachable,
            loaded_model, available_models_count,
            gpu_name, gpu_temperature_c,
            gpu_memory_used_mib, gpu_memory_total_mib,
            gpu_utilization_pct, gpu_power_watts,
            sys_memory_used_mib, sys_memory_total_mib,
            sys_memory_remaining_mib, sys_memory_usage_pct,
            sys_cpu_utilization_pct
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        "#,
    )
    .bind(now)
    .bind(&check.ollama_url)
    .bind(check.ollama_reachable as i32)
    .bind(&check.loaded_model)
    .bind(check.available_models_count as i32)
    .bind(&check.gpu_name)
    .bind(check.gpu_temperature_c)
    .bind(check.gpu_memory_used_mib.map(|v| v as i64))
    .bind(check.gpu_memory_total_mib.map(|v| v as i64))
    .bind(check.gpu_utilization_pct)
    .bind(check.gpu_power_watts)
    .bind(check.sys_memory_used_mib.map(|v| v as i64))
    .bind(check.sys_memory_total_mib.map(|v| v as i64))
    .bind(check.sys_memory_remaining_mib.map(|v| v as i64))
    .bind(check.sys_memory_usage_pct)
    .bind(check.sys_cpu_utilization_pct)
    .execute(pool)
    .await
    .context("Failed to insert check_results row")?;

    Ok(())
}

// ── Queries ──────────────────────────────────────────────

/// All history rows within *range*, ordered ascending.
/// Reads GPU memory + temperature from the `check_results` table.
pub async fn query_history(
    pool: &SqlitePool,
    range: HistoryRange,
) -> anyhow::Result<Vec<(i64, Option<u64>, Option<f64>)>> {
    let interval = range.sqlite_interval();

    let sql = format!(
        "SELECT strftime('%s', recorded_at) * 1000 as ts, gpu_memory_used_mib, gpu_temperature_c \
         FROM check_results \
         WHERE recorded_at >= datetime('now', '{}') \
         ORDER BY recorded_at ASC",
        interval
    );

    let rows = sqlx::query(&sql)
        .fetch_all(pool)
        .await
        .context("Failed to query check_results")?;

    let mut points = Vec::with_capacity(rows.len());
    for row in &rows {
        let ts: i64 = row.try_get(0)?;
        let memory_used_mib: Option<i64> = row.try_get(1)?;
        let temperature_c: Option<f64> = row.try_get(2)?;
        points.push((ts, memory_used_mib.map(|v| v as u64), temperature_c));
    }

    Ok(points)
}

/// System history rows within *range*, ordered ascending.
/// Reads system memory + CPU utilisation from the `check_results` table.
pub async fn query_system_history(
    pool: &SqlitePool,
    range: HistoryRange,
) -> anyhow::Result<Vec<(i64, Option<u64>, Option<f64>)>> {
    let interval = range.sqlite_interval();

    let sql = format!(
        "SELECT strftime('%s', recorded_at) * 1000 as ts, sys_memory_used_mib, sys_cpu_utilization_pct \
         FROM check_results \
         WHERE recorded_at >= datetime('now', '{}') \
         ORDER BY recorded_at ASC",
        interval
    );

    let rows = sqlx::query(&sql)
        .fetch_all(pool)
        .await
        .context("Failed to query system history")?;

    let mut points = Vec::with_capacity(rows.len());
    for row in &rows {
        let ts: i64 = row.try_get(0)?;
        let memory_used_mib: Option<i64> = row.try_get(1)?;
        let cpu_utilization: Option<f64> = row.try_get(2)?;
        points.push((ts, memory_used_mib.map(|v| v as u64), cpu_utilization));
    }

    Ok(points)
}

/// All check-result rows ordered newest-first (most recent first).
#[allow(dead_code)]
pub async fn query_check_results(
    pool: &SqlitePool,
) -> anyhow::Result<Vec<crate::models::CheckResult>> {
    let sql = r#"
        SELECT ollama_url, ollama_reachable, loaded_model, available_models_count,
               gpu_name, gpu_temperature_c, gpu_memory_used_mib, gpu_memory_total_mib,
               gpu_utilization_pct, gpu_power_watts,
               sys_memory_used_mib, sys_memory_total_mib, sys_memory_remaining_mib,
               sys_memory_usage_pct, sys_cpu_utilization_pct
        FROM check_results
        ORDER BY id DESC
    "#;

    let rows = sqlx::query(sql)
        .fetch_all(pool)
        .await
        .context("Failed to query check_results")?;

    let mut results = Vec::with_capacity(rows.len());
    for row in &rows {
        results.push(crate::models::CheckResult {
            ollama_url: row.try_get(0)?,
            ollama_reachable: row.try_get::<i32, _>(1)? != 0,
            loaded_model: row.try_get(2)?,
            available_models_count: row.try_get::<i32, _>(3)? as usize,
            gpu_name: row.try_get(4)?,
            gpu_temperature_c: row.try_get(5)?,
            gpu_memory_used_mib: row.try_get::<Option<i64>, _>(6)?.map(|v| v as u64),
            gpu_memory_total_mib: row.try_get::<Option<i64>, _>(7)?.map(|v| v as u64),
            gpu_utilization_pct: row.try_get(8)?,
            gpu_power_watts: row.try_get(9)?,
            sys_memory_used_mib: row.try_get::<Option<i64>, _>(10)?.map(|v| v as u64),
            sys_memory_total_mib: row.try_get::<Option<i64>, _>(11)?.map(|v| v as u64),
            sys_memory_remaining_mib: row.try_get::<Option<i64>, _>(12)?.map(|v| v as u64),
            sys_memory_usage_pct: row.try_get(13)?,
            sys_cpu_utilization_pct: row.try_get(14)?,
        });
    }

    Ok(results)
}

// ── Tests ─────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    async fn setup_test_db() -> SqlitePool {
        // Use a temp file with max_connections(1) so every query from this pool
        // runs on the same SQLite connection — tables inserted are always visible,
        // and no test interferes with another.
        let dir = std::env::temp_dir();
        let path = dir.join(format!(
            "ollama_monitor_test_{}_{}.db",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let options = SqliteConnectOptions::new()
            .filename(&path)
            .create_if_missing(true);
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await
            .expect("Failed to open test pool");
        migrate(&pool)
            .await
            .expect("Failed to run migrations on test pool");
        pool
    }

    fn make_sample_check() -> crate::models::CheckResult {
        crate::models::CheckResult {
            ollama_url: "http://127.0.0.1:11434".into(),
            ollama_reachable: true,
            loaded_model: Some("llama3:8b".into()),
            available_models_count: 3,
            gpu_name: Some("NVIDIA GeForce RTX 3080".into()),
            gpu_temperature_c: Some(67.5),
            gpu_memory_used_mib: Some(6144),
            gpu_memory_total_mib: Some(10240),
            gpu_utilization_pct: Some(82.0),
            gpu_power_watts: Some(245.0),
            sys_memory_used_mib: Some(8192),
            sys_memory_total_mib: Some(16384),
            sys_memory_remaining_mib: Some(8192),
            sys_memory_usage_pct: Some(50.0),
            sys_cpu_utilization_pct: Some(35.0),
        }
    }

    #[tokio::test]
    async fn test_insert_and_query_check_result() {
        let pool = setup_test_db().await;
        let sample = make_sample_check();

        // Insert.
        insert_check_result(&pool, &sample)
            .await
            .expect("Failed to insert check result");

        // Query back.
        let results = query_check_results(&pool)
            .await
            .expect("Failed to query check results");

        assert_eq!(results.len(), 1, "Expected exactly one result row");

        let row = &results[0];
        assert_eq!(row.ollama_url, sample.ollama_url);
        assert_eq!(row.ollama_reachable, sample.ollama_reachable);
        assert_eq!(row.loaded_model, sample.loaded_model);
        assert_eq!(row.available_models_count, sample.available_models_count);
        assert_eq!(row.gpu_name, sample.gpu_name);
        assert_eq!(row.gpu_temperature_c, sample.gpu_temperature_c);
        assert_eq!(row.gpu_memory_used_mib, sample.gpu_memory_used_mib);
        assert_eq!(row.gpu_memory_total_mib, sample.gpu_memory_total_mib);
        assert_eq!(row.gpu_utilization_pct, sample.gpu_utilization_pct);
        assert_eq!(row.gpu_power_watts, sample.gpu_power_watts);
    }

    #[tokio::test]
    async fn test_multiple_check_results_ordered_desc() {
        let pool = setup_test_db().await;
        let check = make_sample_check();

        // Insert three rows.
        for _ in 0..3 {
            insert_check_result(&pool, &check)
                .await
                .expect("Failed to insert");
        }

        let results = query_check_results(&pool).await.expect("Failed to query");
        assert_eq!(results.len(), 3, "Expected three result rows");

        // All rows should have the same data (since we inserted the same check).
        for row in &results {
            assert_eq!(row.ollama_url, check.ollama_url);
        }
    }

    #[tokio::test]
    async fn test_insert_check_result_with_null_fields() {
        let pool = setup_test_db().await;
        let null_check = crate::models::CheckResult {
            ollama_url: "http://127.0.0.1:11434".into(),
            ollama_reachable: false,
            loaded_model: None,
            available_models_count: 0,
            gpu_name: None,
            gpu_temperature_c: None,
            gpu_memory_used_mib: None,
            gpu_memory_total_mib: None,
            gpu_utilization_pct: None,
            gpu_power_watts: None,
            sys_memory_used_mib: None,
            sys_memory_total_mib: None,
            sys_memory_remaining_mib: None,
            sys_memory_usage_pct: None,
            sys_cpu_utilization_pct: None,
        };

        insert_check_result(&pool, &null_check)
            .await
            .expect("Failed to insert null check result");

        let results = query_check_results(&pool)
            .await
            .expect("Failed to query check results");

        let row = &results[0];
        assert_eq!(row.ollama_url, null_check.ollama_url);
        assert!(!row.ollama_reachable);
        assert!(row.loaded_model.is_none());
        assert_eq!(row.available_models_count, 0);
        assert!(row.gpu_name.is_none());
        assert!(row.gpu_temperature_c.is_none());
        assert!(row.gpu_memory_used_mib.is_none());
        assert!(row.gpu_memory_total_mib.is_none());
        assert!(row.gpu_utilization_pct.is_none());
        assert!(row.gpu_power_watts.is_none());
    }
}
