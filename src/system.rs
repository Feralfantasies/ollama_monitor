/// System metrics collector — reads from /proc on Linux.
use anyhow::{Context, Result};
use tracing::{debug, warn};

use crate::models::SystemMetric;

/// Read CPU utilisation from /proc/stat.
///
/// Takes two samples 100 ms apart to derive an instantaneous utilisation %
/// (idle / total transition over the interval).
fn read_cpu_util() -> Result<f64> {
    let stat1 = read_proc_stat()?;
    std::thread::sleep(std::time::Duration::from_millis(100));
    let stat2 = read_proc_stat()?;

    let total1: u64 = stat1.iter().sum();
    let total2: u64 = stat2.iter().sum();
    let idle1: u64 = stat1.get(3).copied().unwrap_or(0);
    let idle2: u64 = stat2.get(3).copied().unwrap_or(0);

    let total_diff = total2.saturating_sub(total1);
    let idle_diff = idle2.saturating_sub(idle1);

    if total_diff == 0 {
        return Ok(0.0);
    }

    let utilization = (1.0 - (idle_diff as f64 / total_diff as f64)) * 100.0;
    Ok(utilization)
}

/// Parse the CPU line from `/proc/stat` into a Vec of tick counts.
///
/// Fields: user, nice, system, idle, iowait, irq, softirq, steal, …
fn read_proc_stat() -> Result<Vec<u64>> {
    let content = std::fs::read_to_string("/proc/stat").context("Failed to read /proc/stat")?;

    let cpu_line = content
        .lines()
        .find(|l| l.starts_with("cpu "))
        .context("No 'cpu ' line in /proc/stat")?;

    // "cpu  user nice system idle iowait irq softirq steal …"
    let fields: Vec<&str> = cpu_line.split_whitespace().collect();
    let ticks: Vec<u64> = fields[1..]
        .iter()
        .map(|s| {
            s.parse::<u64>()
                .with_context(|| format!("Invalid tick value: {}", s))
        })
        .collect::<Result<Vec<_>>>()?;

    Ok(ticks)
}

/// Read total and available memory from `/proc/meminfo` (values in kB).
fn read_meminfo() -> Result<(u64, u64)> {
    let content =
        std::fs::read_to_string("/proc/meminfo").context("Failed to read /proc/meminfo")?;

    let mut total_kb: Option<u64> = None;
    let mut available_kb: Option<u64> = None;

    for line in content.lines() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.is_empty() {
            continue;
        }

        match parts[0] {
            "MemTotal:" => {
                total_kb = parts.get(1).and_then(|s| s.parse::<u64>().ok());
            }
            "MemAvailable:" => {
                available_kb = parts.get(1).and_then(|s| s.parse::<u64>().ok());
            }
            _ => {}
        }

        // Short-circuit once we have both.
        if total_kb.is_some() && available_kb.is_some() {
            break;
        }
    }

    let total = total_kb.context("MemTotal not found in /proc/meminfo")?;
    let available = available_kb.context("MemAvailable not found in /proc/meminfo")?;
    Ok((total, available))
}

/// Best-effort system metric collector — returns placeholder on failure.
pub fn query_system() -> SystemMetric {
    let mem_result = read_meminfo();
    let cpu_result = read_cpu_util();

    let (total_kb, available_kb) = match mem_result {
        Ok((t, a)) => (t, a),
        Err(e) => {
            warn!("Memory query failed: {}", e);
            return SystemMetric::placeholder();
        }
    };

    let used_kb = total_kb.saturating_sub(available_kb);

    let cpu_pct = match cpu_result {
        Ok(v) => v,
        Err(e) => {
            warn!("CPU query failed: {}", e);
            return SystemMetric::placeholder();
        }
    };

    let remaining_pct = if total_kb > 0 {
        (available_kb as f64 / total_kb as f64) * 100.0
    } else {
        0.0
    };

    let usage_pct = 100.0 - remaining_pct;

    debug!(
        "System memory: used={:.1} MiB, total={:.1} MiB, cpu={:.1}%",
        used_kb as f64 / 1024.0,
        total_kb as f64 / 1024.0,
        cpu_pct
    );

    SystemMetric {
        memory_used_mib: Some(used_kb / 1024),
        memory_total_mib: Some(total_kb / 1024),
        memory_remaining_mib: Some(available_kb / 1024),
        memory_usage_pct: Some(usage_pct),
        cpu_utilization_pct: Some(cpu_pct),
    }
}

// ── Tests ─────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// Verify /proc/stat parsing works on the live system.
    #[test]
    fn test_proc_stat_parses() {
        let ticks = read_proc_stat().expect("Could not read /proc/stat");
        assert!(
            ticks.len() >= 4,
            "/proc/stat should have at least 4 tick fields (user, nice, system, idle)"
        );
        assert!(
            ticks.iter().any(|&t| t > 0),
            "At least one tick value should be positive"
        );
    }

    /// Verify /proc/meminfo parsing works on the live system.
    #[test]
    fn test_meminfo_parses() {
        let (total, available) = read_meminfo().expect("Could not read /proc/meminfo");
        assert!(total > 0, "Total memory should be positive");
        assert!(
            available <= total,
            "Available memory should not exceed total"
        );
    }

    /// Query system metrics — they should contain at least one populated field.
    #[test]
    fn test_query_system_returns_values() {
        let metric = query_system();
        assert!(
            metric.memory_total_mib.is_some(),
            "memory_total should be populated"
        );
        assert!(
            metric.cpu_utilization_pct.is_some(),
            "cpu_utilization should be populated"
        );
    }

    /// Verify placeholder returns all None.
    #[test]
    fn test_system_metric_placeholder() {
        let p = SystemMetric::placeholder();
        assert!(p.memory_used_mib.is_none());
        assert!(p.memory_total_mib.is_none());
        assert!(p.memory_remaining_mib.is_none());
        assert!(p.memory_usage_pct.is_none());
        assert!(p.cpu_utilization_pct.is_none());
    }
}
