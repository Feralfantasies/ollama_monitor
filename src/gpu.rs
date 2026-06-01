/// Parses `nvidia-smi` output to extract GPU metrics.
use anyhow::{Context, Result};
use std::process::Command;
use tracing::{debug, warn};

use crate::models::GpuMetric;

/// Query structured GPU data via nvidia-smi CSV mode (no header).
pub fn query_gpu(device_index: usize) -> Result<GpuMetric> {
    let output = Command::new("nvidia-smi")
        .args(&[
            "--query-gpu=index,name,temperature.gpu,memory.used,memory.total,utilization.gpu,power.draw",
            "--format=csv,noheader,nounits",
            "--id",
            &device_index.to_string(),
        ])
        .output()
        .context("Failed to execute nvidia-smi (is it installed?). Using placeholder GPU metrics.")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("nvidia-smi failed: {}", stderr.trim());
    }

    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    debug!("nvidia-smi raw output: {}", stdout);

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
    // nvidia-smi output has fields separated by ", ". GPU name can contain commas
    // but since we query a specific device by --id, there is exactly one line.
    // We split from the right for numeric fields and treat the middle as name.

    let parts: Vec<&str> = line.split(", ").collect();

    if parts.len() < 7 {
        anyhow::bail!("Unexpected nvidia-smi CSV fields count: {}", parts.len());
    }

    // Parse from right for numeric fields, name is everything between index and temperature.
    let power = parse_optional_f64(parts.last().copied());
    let utilization = parse_optional_f64(parts.get(parts.len() - 2).copied());
    let mem_total_raw = parts.get(parts.len() - 3).copied();
    let mem_used_raw = parts.get(parts.len() - 4).copied();
    let temp = parse_optional_f64(parts.get(parts.len() - 5).copied());

    // Name is in the middle (from index 1 to len-6)
    let name_fields: Vec<_> = parts[1..parts.len() - 6].iter().map(|s| *s).collect();
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
