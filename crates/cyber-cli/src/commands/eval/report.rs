//! The versioned evaluation report (`harness-evaluation` → Coding quality and cost measures).

use std::collections::BTreeMap;
use std::path::Path;

use cyber_core::eval::{Manifest, Task};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::trial::TrialResult;
use crate::error::CliError;

pub const SCHEMA_VERSION: u32 = 1;

pub fn now() -> String {
    chrono_like_now()
}

/// RFC 3339 UTC timestamp without extra dependencies.
fn chrono_like_now() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs()) as i64;
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    let (y, m, d) = civil(days);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

/// Days since 1970-01-01 to a calendar date (Howard Hinnant's algorithm).
fn civil(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

/// 95% Wilson score interval for `k` successes in `n` trials.
pub fn wilson(k: usize, n: usize) -> (f64, f64) {
    if n == 0 {
        return (0.0, 0.0);
    }
    let (k, n, z) = (k as f64, n as f64, 1.96f64);
    let p = k / n;
    let denom = 1.0 + z * z / n;
    let centre = (p + z * z / (2.0 * n)) / denom;
    let margin = z * (p * (1.0 - p) / n + z * z / (4.0 * n * n)).sqrt() / denom;
    ((centre - margin).max(0.0), (centre + margin).min(1.0))
}

fn median(mut v: Vec<f64>) -> Option<f64> {
    if v.is_empty() {
        return None;
    }
    v.sort_by(|a, b| a.total_cmp(b));
    let mid = v.len() / 2;
    Some(if v.len().is_multiple_of(2) {
        (v[mid - 1] + v[mid]) / 2.0
    } else {
        v[mid]
    })
}

fn round(x: f64, places: i32) -> f64 {
    let f = 10f64.powi(places);
    (x * f).round() / f
}

pub fn build(
    manifest: &Manifest,
    path: &Path,
    model: &Value,
    trials: u32,
    started: String,
    tasks: &[Task],
    mut results: Vec<TrialResult>,
) -> Value {
    results.sort_by(|a, b| a.task.cmp(&b.task).then(a.trial.cmp(&b.trial)));
    let n = results.len();
    let passed = results.iter().filter(|r| r.passed).count();
    let cost: f64 = results.iter().map(|r| r.cost_usd).sum();
    let (lo, hi) = wilson(passed, n);
    let successful_costs: Vec<f64> = results
        .iter()
        .filter(|r| r.passed)
        .map(|r| r.cost_usd)
        .collect();
    let info = cyber_core::version::build_info();
    json!({
        "schema_version": SCHEMA_VERSION,
        "report": "cyber-eval",
        "harness": { "version": info.version, "git_sha": info.git_sha, "target": info.target },
        "manifest": { "id": manifest.id, "path": path.display().to_string(), "sha256": file_sha(path), "kind": format!("{:?}", manifest.kind).to_lowercase() },
        "model": model,
        "deterministic": false,
        "started_at": started,
        "finished_at": now(),
        "trials_per_task": trials,
        "options": { "mode": "bypass", "sandbox": "workspace-write", "interactive": false },
        "summary": {
            "tasks": tasks.len(),
            "trials": n,
            "passed": passed,
            "success_rate": round(if n == 0 { 0.0 } else { passed as f64 / n as f64 }, 4),
            "success_ci95": [round(lo, 4), round(hi, 4)],
            "infrastructure_failures": results.iter().filter(|r| r.infrastructure_failure).count(),
            "human_interventions": 0,
            "cost_usd": round(cost, 6),
            "cost_per_success_usd": if passed == 0 { Value::Null } else { json!(round(cost / passed as f64, 6)) },
            "median_cost_per_successful_task_usd": median(successful_costs).map(|c| round(c, 6)),
            "median_latency_seconds": median(results.iter().map(|r| r.latency_seconds).collect()).map(|s| round(s, 1)),
            "unpriced": results.iter().any(|r| r.unpriced),
            "tokens": tokens(&results),
        },
        "tasks": per_task(tasks, &results),
        "trials": results,
    })
}

fn tokens(results: &[TrialResult]) -> Value {
    let sum = |f: fn(&TrialResult) -> u64| results.iter().map(f).sum::<u64>();
    json!({
        "input": sum(|r| r.tokens.input),
        "output": sum(|r| r.tokens.output),
        "cache_read": sum(|r| r.tokens.cache_read),
        "cache_write": sum(|r| r.tokens.cache_write),
    })
}

fn per_task(tasks: &[Task], results: &[TrialResult]) -> Vec<Value> {
    let mut by_task: BTreeMap<&str, Vec<&TrialResult>> = BTreeMap::new();
    for r in results {
        by_task.entry(r.task.as_str()).or_default().push(r);
    }
    tasks
        .iter()
        .map(|t| {
            let rs = by_task.get(t.id.as_str()).cloned().unwrap_or_default();
            let passed = rs.iter().filter(|r| r.passed).count();
            json!({
                "id": t.id,
                "tags": t.tags,
                "trials": rs.len(),
                "passed": passed,
                "infrastructure_failures": rs.iter().filter(|r| r.infrastructure_failure).count(),
                "cost_usd": round(rs.iter().map(|r| r.cost_usd).sum(), 6),
                "median_turns": median(rs.iter().map(|r| f64::from(r.turns)).collect()),
            })
        })
        .collect()
}

fn file_sha(path: &Path) -> String {
    std::fs::read(path)
        .map(|b| format!("sha256:{:x}", Sha256::digest(b)))
        .unwrap_or_default()
}

pub fn headline(report: &Value) -> String {
    let s = &report["summary"];
    format!(
        "{} passed {}/{} ({:.1}%, 95% CI {:.1}-{:.1}%), cost ${:.4}, infra failures {}",
        report["model"]["ref"].as_str().unwrap_or_default(),
        s["passed"],
        s["trials"],
        s["success_rate"].as_f64().unwrap_or(0.0) * 100.0,
        s["success_ci95"][0].as_f64().unwrap_or(0.0) * 100.0,
        s["success_ci95"][1].as_f64().unwrap_or(0.0) * 100.0,
        s["cost_usd"].as_f64().unwrap_or(0.0),
        s["infrastructure_failures"],
    )
}

pub fn write(dir: &Path, report: &Value) -> Result<(), CliError> {
    std::fs::write(
        dir.join("report.json"),
        serde_json::to_string_pretty(report).unwrap_or_default() + "\n",
    )?;
    std::fs::write(dir.join("report.md"), markdown(report))?;
    Ok(())
}

fn markdown(r: &Value) -> String {
    let s = &r["summary"];
    let mut out = format!(
        "# Evaluation: {} on {}\n\n{}\n\nHarness {} ({}), {} trials per task, started {}.\n\n| Task | Tags | Passed | Infra failures | Cost (USD) | Median turns |\n|---|---|---|---|---|---|\n",
        r["manifest"]["id"].as_str().unwrap_or_default(),
        r["model"]["ref"].as_str().unwrap_or_default(),
        headline(r),
        r["harness"]["version"].as_str().unwrap_or_default(),
        r["harness"]["git_sha"].as_str().unwrap_or_default(),
        r["trials_per_task"],
        r["started_at"].as_str().unwrap_or_default(),
    );
    for t in r["tasks"].as_array().into_iter().flatten() {
        let tags: Vec<&str> = t["tags"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .collect();
        out.push_str(&format!(
            "| {} | {} | {}/{} | {} | {:.4} | {} |\n",
            t["id"].as_str().unwrap_or_default(),
            tags.join(", "),
            t["passed"],
            t["trials"],
            t["infrastructure_failures"],
            t["cost_usd"].as_f64().unwrap_or(0.0),
            t["median_turns"],
        ));
    }
    out.push_str(&format!(
        "\nCost per success: {}. Median cost per successful task: {}. Median latency: {} s. Unpriced results: {}.\n",
        s["cost_per_success_usd"], s["median_cost_per_successful_task_usd"], s["median_latency_seconds"], s["unpriced"]
    ));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wilson_interval_bounds() {
        let (lo, hi) = wilson(0, 3);
        assert!(lo == 0.0 && hi > 0.5 && hi < 0.6);
        let (lo, hi) = wilson(60, 60);
        assert!(lo > 0.93 && hi > 0.9999);
        assert_eq!(wilson(0, 0), (0.0, 0.0));
    }

    #[test]
    fn medians_and_dates() {
        assert_eq!(median(vec![3.0, 1.0, 2.0]), Some(2.0));
        assert_eq!(median(vec![4.0, 1.0]), Some(2.5));
        assert_eq!(civil(0), (1970, 1, 1));
        assert_eq!(civil(20_365), (2025, 10, 4));
    }
}
