//! Structured JSON-lines logs (`observability-costs` → Structured logs, `cli-commands` →
//! Logging destination): `<data>/log/cyber-<YYYY-MM-DD>.log`, one file per UTC day, 14 days
//! kept, level `info` by default, optional mirroring to stderr. A record sink
//! ([`set_sink`]) also receives each written record, for OpenTelemetry log export.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, PoisonError, RwLock};

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

    pub fn name(self) -> &'static str {
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

/// One record as written: what the sink receives.
#[derive(Debug, Clone, Copy)]
pub struct LogRecord<'a> {
    pub ts: chrono::DateTime<chrono::Utc>,
    pub level: Level,
    pub component: &'a str,
    pub msg: &'a str,
    /// The extra fields, without nulls.
    pub fields: &'a Map<String, Value>,
}

/// Receives every record that passes the level filter, after it is written.
pub type Sink = Arc<dyn Fn(&LogRecord<'_>) + Send + Sync>;

static SINK: RwLock<Option<Sink>> = RwLock::new(None);

/// Send every written record to `sink` as well, replacing an earlier one; `None` stops
/// it. The sink must not block: it runs on the caller of [`log`].
pub fn set_sink(sink: Option<Sink>) {
    *SINK.write().unwrap_or_else(PoisonError::into_inner) = sink;
}

/// Stop the sink when it is still `sink`, so a later one is never removed.
pub fn clear_sink(sink: &Sink) {
    let mut current = SINK.write().unwrap_or_else(PoisonError::into_inner);
    if current.as_ref().is_some_and(|s| Arc::ptr_eq(s, sink)) {
        *current = None;
    }
}

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

/// Write one line: `{ts, level, component, msg, ...fields}`, then hand the record to the
/// sink. A no-op before [`init`].
pub fn log(level: Level, component: &str, msg: &str, fields: Value) {
    let Some(logger) = LOGGER.get() else {
        return;
    };
    let sink = SINK.read().unwrap_or_else(PoisonError::into_inner).clone();
    logger.log(sink.as_deref(), level, component, msg, fields);
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
    fn log(
        &self,
        sink: Option<&(dyn Fn(&LogRecord<'_>) + Send + Sync)>,
        level: Level,
        component: &str,
        msg: &str,
        fields: Value,
    ) {
        if level > self.level {
            return;
        }
        let now = chrono::Utc::now();
        let extra: Map<String, Value> = match fields {
            Value::Object(extra) => extra.into_iter().filter(|(_, v)| !v.is_null()).collect(),
            _ => Map::new(),
        };
        let mut line = Map::new();
        line.insert(
            "ts".into(),
            json!(now.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)),
        );
        line.insert("level".into(), json!(level.name()));
        line.insert("component".into(), json!(component));
        line.insert("msg".into(), json!(msg));
        line.extend(extra.clone());
        let text = Value::Object(line).to_string();
        if self.print {
            eprintln!("{text}");
        }
        self.write(&now.format("%Y-%m-%d").to_string(), &text);
        if let Some(sink) = sink {
            sink(&LogRecord {
                ts: now,
                level,
                component,
                msg,
                fields: &extra,
            });
        }
    }

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

    type Seen = Arc<Mutex<Vec<(Level, String, String, Map<String, Value>)>>>;

    fn collecting() -> (Seen, impl Fn(&LogRecord<'_>) + Send + Sync) {
        let seen = Seen::default();
        let sink = {
            let seen = Arc::clone(&seen);
            move |record: &LogRecord<'_>| {
                seen.lock().unwrap().push((
                    record.level,
                    record.component.to_string(),
                    record.msg.to_string(),
                    record.fields.clone(),
                ));
            }
        };
        (seen, sink)
    }

    fn logger(dir: &Path, level: Level) -> Logger {
        Logger {
            dir: dir.to_path_buf(),
            level,
            print: false,
            file: Mutex::new(None),
        }
    }

    /// The sink sees each record the file gets, with the same fields, and nothing the
    /// level filter drops.
    #[test]
    fn the_sink_sees_what_the_file_sees() {
        let tmp = tempfile::tempdir().unwrap();
        let logger = logger(tmp.path(), Level::Info);
        let (seen, sink) = collecting();
        logger.log(
            Some(&sink),
            Level::Warn,
            "server",
            "listening",
            json!({"port": 4096, "gone": null}),
        );
        logger.log(
            Some(&sink),
            Level::Debug,
            "server",
            "too verbose",
            json!({}),
        );
        logger.log(None, Level::Info, "server", "no sink", json!({}));
        logger.log(
            Some(&sink),
            Level::Error,
            "store",
            "plain",
            json!("not an object"),
        );

        let file = std::fs::read_dir(tmp.path())
            .unwrap()
            .flatten()
            .next()
            .unwrap();
        let lines: Vec<Value> = std::fs::read_to_string(file.path())
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(lines.len(), 3, "the debug line is filtered: {lines:?}");
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 2);
        let (level, component, msg, fields) = &seen[0];
        assert_eq!(
            (*level, component.as_str(), msg.as_str()),
            (Level::Warn, "server", "listening")
        );
        assert_eq!(Value::Object(fields.clone()), json!({"port": 4096}));
        assert_eq!(lines[0]["port"], 4096);
        assert_eq!(lines[0]["level"], "warn");
        assert!(lines[0].get("gone").is_none());
        assert_eq!(seen[1].0, Level::Error);
        assert!(seen[1].3.is_empty());
        assert_eq!(lines[2]["msg"], "plain");
    }

    #[test]
    fn a_cleared_sink_only_removes_itself() {
        let first: Sink = Arc::new(|_| {});
        let second: Sink = Arc::new(|_| {});
        set_sink(Some(Arc::clone(&second)));
        clear_sink(&first);
        assert!(SINK.read().unwrap().is_some());
        clear_sink(&second);
        assert!(SINK.read().unwrap().is_none());
    }

    #[test]
    fn levels_parse_and_order() {
        assert_eq!(Level::parse("WARN"), Some(Level::Warn));
        assert_eq!(Level::parse("nope"), None);
        assert!(Level::Error < Level::Info && Level::Info < Level::Debug);
    }
}
