//! Structured JSON-lines logs (`observability-costs` → Structured logs, `cli-commands` →
//! Logging destination): `<data>/log/cyber-<YYYY-MM-DD>.log`, one file per UTC day, 14 days
//! kept, level `info` by default, optional mirroring to stderr.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock, PoisonError};

use serde_json::{Map, Value, json};

const KEEP_DAYS: i64 = 14;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Error,
    Warn,
    Info,
    Debug,
    Trace,
}

impl Level {
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "error" => Some(Self::Error),
            "warn" | "warning" => Some(Self::Warn),
            "info" => Some(Self::Info),
            "debug" => Some(Self::Debug),
            "trace" => Some(Self::Trace),
            _ => None,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::Warn => "warn",
            Self::Info => "info",
            Self::Debug => "debug",
            Self::Trace => "trace",
        }
    }
}

struct Logger {
    dir: PathBuf,
    level: Level,
    print: bool,
    /// The open file and the day it belongs to.
    file: Mutex<Option<(String, File)>>,
}

static LOGGER: OnceLock<Logger> = OnceLock::new();

/// Start logging to `dir`; later calls are ignored. Deletes files older than 14 days.
pub fn init(dir: &Path, level: Level, print: bool) {
    let _ = std::fs::create_dir_all(dir);
    prune(dir, &today());
    let _ = LOGGER.set(Logger {
        dir: dir.to_path_buf(),
        level,
        print,
        file: Mutex::new(None),
    });
}

/// Whether a line at `level` would be written.
pub fn enabled(level: Level) -> bool {
    LOGGER.get().is_some_and(|l| level <= l.level)
}

/// Write one line: `{ts, level, component, msg, ...fields}`. A no-op before [`init`].
pub fn log(level: Level, component: &str, msg: &str, fields: Value) {
    let Some(logger) = LOGGER.get().filter(|l| level <= l.level) else {
        return;
    };
    let now = chrono::Utc::now();
    let mut line = Map::new();
    line.insert(
        "ts".into(),
        json!(now.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)),
    );
    line.insert("level".into(), json!(level.name()));
    line.insert("component".into(), json!(component));
    line.insert("msg".into(), json!(msg));
    if let Value::Object(extra) = fields {
        line.extend(extra.into_iter().filter(|(_, v)| !v.is_null()));
    }
    let text = Value::Object(line).to_string();
    if logger.print {
        eprintln!("{text}");
    }
    logger.write(&now.format("%Y-%m-%d").to_string(), &text);
}

pub fn error(component: &str, msg: &str, fields: Value) {
    log(Level::Error, component, msg, fields);
}

pub fn warn(component: &str, msg: &str, fields: Value) {
    log(Level::Warn, component, msg, fields);
}

pub fn info(component: &str, msg: &str, fields: Value) {
    log(Level::Info, component, msg, fields);
}

pub fn debug(component: &str, msg: &str, fields: Value) {
    log(Level::Debug, component, msg, fields);
}

impl Logger {
    fn write(&self, day: &str, text: &str) {
        let mut guard = self.file.lock().unwrap_or_else(PoisonError::into_inner);
        if guard.as_ref().is_none_or(|(d, _)| d != day) {
            prune(&self.dir, day);
            let path = self.dir.join(format!("cyber-{day}.log"));
            *guard = OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
                .ok()
                .map(|f| (day.to_string(), f));
        }
        if let Some((_, file)) = guard.as_mut() {
            let _ = writeln!(file, "{text}");
        }
    }
}

fn today() -> String {
    chrono::Utc::now().format("%Y-%m-%d").to_string()
}

/// Delete `cyber-<date>.log` files more than 14 days before `today`.
fn prune(dir: &Path, today: &str) {
    let Ok(today) = chrono::NaiveDate::parse_from_str(today, "%Y-%m-%d") else {
        return;
    };
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        let date = name
            .strip_prefix("cyber-")
            .and_then(|n| n.strip_suffix(".log"));
        if let Some(day) = date.and_then(|d| chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d").ok())
            && (today - day).num_days() > KEEP_DAYS
        {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_files_are_pruned_and_others_kept() {
        let tmp = tempfile::tempdir().unwrap();
        for name in [
            "cyber-2026-09-01.log",
            "cyber-2026-09-25.log",
            "notes.log",
            "cyber-bad.log",
        ] {
            std::fs::write(tmp.path().join(name), "x").unwrap();
        }
        prune(tmp.path(), "2026-10-04");
        let mut left: Vec<String> = std::fs::read_dir(tmp.path())
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        left.sort();
        assert_eq!(
            left,
            vec!["cyber-2026-09-25.log", "cyber-bad.log", "notes.log"]
        );
    }

    #[test]
    fn levels_parse_and_order() {
        assert_eq!(Level::parse("WARN"), Some(Level::Warn));
        assert_eq!(Level::parse("nope"), None);
        assert!(Level::Error < Level::Info && Level::Info < Level::Debug);
    }
}
