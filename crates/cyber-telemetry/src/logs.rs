//! The OTLP log bridge (`observability-costs` → OpenTelemetry export, logs): each
//! structured log record that passes the level filter becomes an OTLP log record with its
//! severity, `component` and the identifier fields. The message and every other scalar
//! field are content: many messages quote paths and error text, so they are exported,
//! redacted and capped, only with `log_prompts`.
//!
//! Emitting is a non-blocking enqueue into the SDK batch processor, and the SDK's own
//! internal logs are compiled out, so export never writes back into the log.

use std::sync::Arc;

use cyber_core::log::{Level, LogRecord, Sink};
use opentelemetry::logs::{AnyValue, LogRecord as _, Logger as _, Severity};
use opentelemetry_sdk::logs::SdkLogger;
use serde_json::Value;

use crate::privacy::Content;

/// The fields that identify work and are exported without `log_prompts`.
pub const IDENTIFIERS: [&str; 7] = [
    "session_id",
    "request_id",
    "run_id",
    "call_id",
    "kind",
    "status",
    "pid",
];

/// The keys of every line, which a field never replaces.
const RESERVED: [&str; 4] = ["ts", "level", "component", "msg"];

/// Turns log records into OTLP log records.
pub struct Bridge {
    logger: SdkLogger,
    content: Content,
}

impl Bridge {
    pub fn new(logger: SdkLogger, content: Content) -> Self {
        Self { logger, content }
    }

    /// The sink for [`cyber_core::log::set_sink`].
    pub fn sink(self) -> Sink {
        Arc::new(move |record| self.emit(record))
    }

    pub fn emit(&self, record: &LogRecord<'_>) {
        let mut out = self.logger.create_log_record();
        out.set_timestamp(record.ts.into());
        out.set_observed_timestamp(std::time::SystemTime::now());
        let (number, text) = severity(record.level);
        out.set_severity_number(number);
        out.set_severity_text(text);
        if let Some(body) = self.content.value(record.msg) {
            out.set_body(AnyValue::String(body.into()));
        }
        out.add_attribute("component", record.component.to_string());
        for (key, value) in record.fields {
            if let Some(value) = self.field(key, value) {
                out.add_attribute(key.clone(), value);
            }
        }
        self.logger.emit(out);
    }

    /// An identifier field as is (its text redacted), another scalar field only with
    /// `log_prompts` and redacted, and never an object or array.
    fn field(&self, key: &str, value: &Value) -> Option<AnyValue> {
        if RESERVED.contains(&key) {
            return None;
        }
        let identifier = IDENTIFIERS.contains(&key);
        if !identifier && !self.content.enabled() {
            return None;
        }
        match value {
            Value::String(text) if identifier => Some(self.content.redact(text).into()),
            Value::String(text) => self.content.value(text).map(Into::into),
            Value::Bool(flag) => Some((*flag).into()),
            Value::Number(number) => number
                .as_i64()
                .map(AnyValue::from)
                .or_else(|| number.as_f64().map(AnyValue::from)),
            _ => None,
        }
    }
}

fn severity(level: Level) -> (Severity, &'static str) {
    let number = match level {
        Level::Error => Severity::Error,
        Level::Warn => Severity::Warn,
        Level::Info => Severity::Info,
        Level::Debug => Severity::Debug,
        Level::Trace => Severity::Trace,
    };
    (number, level.name())
}

#[cfg(test)]
mod tests {
    use super::*;
    use opentelemetry::logs::LoggerProvider as _;
    use opentelemetry_sdk::logs::{InMemoryLogExporter, SdkLoggerProvider};
    use serde_json::{Map, json};

    /// The record's export. The provider is returned too: dropping it resets the
    /// exporter.
    fn emit(
        log_prompts: bool,
        level: Level,
        msg: &str,
        fields: Value,
    ) -> (InMemoryLogExporter, SdkLoggerProvider) {
        let exporter = InMemoryLogExporter::default();
        let provider = SdkLoggerProvider::builder()
            .with_simple_exporter(exporter.clone())
            .build();
        let content = Content::new(log_prompts, ["hdr-4471".to_string()]);
        let bridge = Bridge::new(provider.logger("cyber"), content);
        let Value::Object(fields) = fields else {
            unreachable!()
        };
        bridge.emit(&LogRecord {
            ts: chrono::Utc::now(),
            level,
            component: "server",
            msg,
            fields: &fields,
        });
        (exporter, provider)
    }

    fn attributes(exporter: &InMemoryLogExporter) -> Map<String, Value> {
        let logs = exporter.get_emitted_logs().unwrap();
        assert_eq!(logs.len(), 1);
        logs[0]
            .record
            .attributes_iter()
            .map(|(key, value)| (key.to_string(), json!(text(value))))
            .collect()
    }

    fn text(value: &AnyValue) -> String {
        match value {
            AnyValue::String(text) => text.to_string(),
            AnyValue::Int(value) => value.to_string(),
            AnyValue::Double(value) => value.to_string(),
            AnyValue::Boolean(value) => value.to_string(),
            other => format!("{other:?}"),
        }
    }

    fn fields() -> Value {
        json!({
            "session_id": "ses_1",
            "call_id": "call_1",
            "status": "failed ghp_abcdefghijklmnop",
            "pid": 42,
            "path": "/home/me/private.txt",
            "error": "denied with sk-abcdefghijklmnop",
            "retry": true,
            "args": {"command": "ls"},
            "component": "spoofed",
        })
    }

    /// By default only `component` and the identifier allowlist are attributes, and
    /// the message, which can quote a path, is not exported.
    #[test]
    fn default_records_keep_component_and_identifiers_only() {
        let msg = "Could not truncate /home/me/private.log: denied";
        let (exporter, _provider) = emit(false, Level::Warn, msg, fields());
        let logs = exporter.get_emitted_logs().unwrap();
        let record = &logs[0].record;
        assert_eq!(record.severity_number(), Some(Severity::Warn));
        assert_eq!(record.severity_text(), Some("warn"));
        assert!(record.body().is_none(), "{:?}", record.body());
        assert!(record.timestamp().is_some());
        let attributes = attributes(&exporter);
        assert_eq!(
            Value::Object(attributes),
            json!({"component": "server", "session_id": "ses_1", "call_id": "call_1", "status": "failed ***", "pid": "42"})
        );
    }

    /// With `log_prompts` the message and the other scalar fields are exported too,
    /// redacted and capped; objects and arrays never are.
    #[test]
    fn log_prompts_adds_redacted_scalar_fields() {
        let msg = format!("failed hdr-4471 {}", "x".repeat(crate::privacy::LIMIT));
        let (exporter, _provider) = emit(true, Level::Error, &msg, fields());
        let logs = exporter.get_emitted_logs().unwrap();
        let body = text(logs[0].record.body().unwrap());
        assert!(body.starts_with("failed *** x"), "{}", &body[..20]);
        assert!(body.len() <= crate::privacy::LIMIT, "the body is capped");
        let attributes = attributes(&exporter);
        assert_eq!(attributes["path"], "/home/me/private.txt");
        assert_eq!(attributes["error"], "denied with ***");
        assert_eq!(attributes["retry"], "true");
        assert_eq!(attributes["component"], "server");
        assert!(!attributes.contains_key("args"));
        assert_eq!(logs[0].record.severity_number(), Some(Severity::Error));
    }
}
