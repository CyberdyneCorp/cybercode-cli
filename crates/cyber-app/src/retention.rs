//! Retention and garbage collection (`storage-events` → Retention and garbage collection):
//! 2 minutes after the server starts, then every 6 hours. A retention value of 0 disables
//! that rule. Snapshot repositories are collected by `cyber-snapshot` on their own schedule.

use std::path::Path;
use std::time::{Duration, SystemTime};

use cyber_server::runtime::{ListFilter, Runtime};
use serde_json::{Value, json};

const FIRST_SWEEP: Duration = Duration::from_secs(120);
const INTERVAL: Duration = Duration::from_secs(6 * 3600);
const DAY: u64 = 86_400;

/// What one sweep removed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Sweep {
    pub tool_output_files: usize,
    pub archived_sessions: usize,
}

/// Retention settings from config: `storage.retention.{tool_output_days, archived_days}`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Retention {
    pub tool_output_days: u64,
    pub archived_days: u64,
}

impl Retention {
    pub fn from_config(config: &Value) -> Self {
        let get = |k: &str, d: u64| {
            config
                .pointer(&format!("/storage/retention/{k}"))
                .and_then(Value::as_u64)
                .unwrap_or(d)
        };
        Self {
            tool_output_days: get("tool_output_days", 7),
            archived_days: get("archived_days", 90),
        }
    }
}

pub fn spawn(
    runtime: Runtime,
    data_dir: std::path::PathBuf,
    retention: Retention,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        tokio::time::sleep(FIRST_SWEEP).await;
        loop {
            let swept = sweep(&runtime, &data_dir, retention, SystemTime::now()).await;
            cyber_core::log::info(
                "retention",
                "sweep",
                json!({ "tool_output_files": swept.tool_output_files, "archived_sessions": swept.archived_sessions }),
            );
            tokio::time::sleep(INTERVAL).await;
        }
    })
}

pub async fn sweep(
    runtime: &Runtime,
    data_dir: &Path,
    retention: Retention,
    now: SystemTime,
) -> Sweep {
    Sweep {
        tool_output_files: old_tool_output(
            &data_dir.join("tool-output"),
            retention.tool_output_days,
            now,
        ),
        archived_sessions: old_archived_sessions(runtime, retention.archived_days, now).await,
    }
}

fn older_than(time: SystemTime, days: u64, now: SystemTime) -> bool {
    now.duration_since(time)
        .is_ok_and(|age| age.as_secs() > days * DAY)
}

/// The only cleanup of `<data>/tool-output`.
fn old_tool_output(dir: &Path, days: u64, now: SystemTime) -> usize {
    if days == 0 {
        return 0;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    entries
        .flatten()
        .filter(|e| {
            e.metadata()
                .ok()
                .and_then(|m| m.modified().ok())
                .is_some_and(|t| older_than(t, days, now))
        })
        .filter(|e| std::fs::remove_file(e.path()).is_ok())
        .count()
}

async fn old_archived_sessions(runtime: &Runtime, days: u64, now: SystemTime) -> usize {
    if days == 0 {
        return 0;
    }
    let cutoff_ms = now
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
        - (days * DAY * 1000) as i64;
    let mut doomed = Vec::new();
    let mut cursor = None;
    loop {
        let filter = ListFilter {
            include_archived: true,
            limit: Some(200),
            cursor: cursor.clone(),
            ..ListFilter::default()
        };
        let Ok(page) = runtime.list(&filter) else {
            break;
        };
        doomed.extend(
            page.sessions
                .iter()
                .filter(|s| s.archived && s.updated_at < cutoff_ms)
                .map(|s| s.id.clone()),
        );
        cursor = page.next;
        if cursor.is_none() {
            break;
        }
    }
    let mut deleted = 0;
    for id in doomed {
        if runtime.delete(&id).await.is_ok() {
            deleted += 1;
        }
    }
    deleted
}
