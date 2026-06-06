/// Parses `nvidia-smi` output to extract GPU metrics.
use anyhow::{Context, Result};
use std::process::Command;
use tracing::{debug, warn};

use crate::models::GpuMetric;

/// Type alias for a GPU query function, used for injecting mocks in tests.
pub type GpuQueryFn = fn(usize) -> GpuMetric;

/// Query structured GPU data via `nvidia-smi` CSV mode (no header).
///
/// Uses the system `$PATH` to locate the `nvidia-smi` binary.
pub fn query_gpu(device_index: usize) -> Result<GpuMetric> {
    query_gpu_bin("nvidia-smi", device_index)
}

/// Query structured GPU data from a specific binary path.
///
/// Used in tests to point at a mock `nvidia-smi` script instead of the system binary.
pub fn query_gpu_bin(bin_path: &str, device_index: usize) -> Result<GpuMetric> {
    let output = Command::new(bin_path)
        .args([
            "--query-gpu=index,name,temperature.gpu,memory.used,memory.total,utilization.gpu,power.draw",
            "--format=csv,noheader,nounits",
            "--id",
            &device_index.to_string(),
        ])
        .output()
        .context(format!(
            "Failed to execute {} (is it installed?). Using placeholder GPU metrics.",
            bin_path
        ))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("{} failed: {}", bin_path, stderr.trim());
    }

    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    debug!("{} raw output: {}", bin_path, stdout);

    parse_gpu_csv_line(&stdout, device_index)
}

/// Best-effort GPU query — returns a placeholder metric on failure.
pub fn try_query_gpu(device_index: usize) -> GpuMetric {
    match query_gpu(device_index) {
        Ok(metric) => metric,
        Err(e) => {
            warn!(
                "GPU query failed (this is expected in non-GPU dev environments): {}",
                e
            );
            GpuMetric::placeholder()
        }
    }
}

/// Parse a single CSV line from `nvidia-smi --query-... --format=csv,noheader,nounits`.
/// Expected columns: index, name, temp, mem_used, mem_total, util_gpu, power_draw
fn parse_gpu_csv_line(line: &str, expected_index: usize) -> Result<GpuMetric> {
    let parts: Vec<&str> = line.split(", ").collect();

    if parts.len() < 7 {
        anyhow::bail!("Unexpected nvidia-smi CSV fields count: {}", parts.len());
    }

    let power = parse_optional_f64(parts.last().copied());
    let utilization = parse_optional_f64(parts.get(parts.len() - 2).copied());
    let mem_total_raw = parts.get(parts.len() - 3).copied();
    let mem_used_raw = parts.get(parts.len() - 4).copied();
    let temp = parse_optional_f64(parts.get(parts.len() - 5).copied());

    let name_fields: Vec<_> = parts[1..parts.len() - 6].to_vec();
    let name = if !name_fields.is_empty() {
        Some(name_fields.join(", "))
    } else {
        None
    };

    let idx_parsed = parts.first().and_then(|s| s.trim().parse::<usize>().ok());
    if Some(expected_index) != idx_parsed {
        anyhow::bail!(
            "GPU index mismatch: expected {}, got {:?}",
            expected_index,
            idx_parsed
        );
    }

    let mem_used = parse_optional_u64(mem_used_raw);
    let mem_total = parse_optional_u64(mem_total_raw);

    let remaining = match (mem_used, mem_total) {
        (Some(u), Some(t)) => Some(t.saturating_sub(u)),
        _ => None,
    };

    Ok(GpuMetric {
        name,
        temperature_c: temp,
        memory_used_mib: mem_used,
        memory_total_mib: mem_total,
        memory_remaining_mib: remaining,
        utilization_pct: utilization,
        power_watts: power,
    })
}

fn parse_optional_f64(s: Option<&str>) -> Option<f64> {
    s.and_then(|v| v.trim().parse::<f64>().ok())
}

fn parse_optional_u64(s: Option<&str>) -> Option<u64> {
    s.and_then(|v| v.trim().parse::<u64>().ok())
}
