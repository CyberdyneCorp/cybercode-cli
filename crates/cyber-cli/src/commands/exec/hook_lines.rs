//! `--include-hook-events` (`exec-mode` → Hook events in exec): one `hook_event` line
//! for each hook execution of this run's Session once it settles.
//!
//! Lines come from the IO-free receipt summaries of the instance stream
//! (`include=hook_receipts`), in the order the runtime settled them, so a gating hook's
//! line precedes the line of what it gated. Before each `result` the durable receipt
//! listing fills in what the live stream did not carry: receipts settled before the run
//! subscribed, or dropped when the stream lagged. Only summary fields are copied, so
//! hook stdin, stdout, stderr, decision reasons and environment never reach a line, even
//! when `telemetry.log_hook_io` keeps IO on the receipt.

use std::collections::HashSet;

use cyber_client::{Client, Event};
use serde_json::{Value, json};

use super::ExecArgs;
use super::report::Out;
use crate::commands::sessions::encode;

/// Receipts read per listing page, and pages read per reconciliation at most.
const PAGE: u32 = 500;
const MAX_PAGES: usize = 20;

/// The live event that carries a receipt summary.
pub(super) const RECEIPT_EVENT: &str = "session.hook.receipt";

/// A receipt's position in the server's listing order (newest first).
type Position = (i64, String);

pub(super) struct HookLines {
    enabled: bool,
    session: String,
    /// For a reused Session, the newest receipt admitted before this run: it and every
    /// older receipt belong to earlier runs.
    floor: Option<Position>,
    emitted: HashSet<String>,
}

impl HookLines {
    pub fn disabled() -> Self {
        Self {
            enabled: false,
            session: String::new(),
            floor: None,
            emitted: HashSet::new(),
        }
    }

    /// Lines for `session` when the flag is set. A Session this run created (or forked)
    /// has only this run's receipts, so those already settled are emitted now; a reused
    /// Session's earlier receipts are fenced off instead.
    pub async fn start(client: &Client, args: &ExecArgs, session: &str, out: &Out) -> Self {
        if !args.include_hook_events {
            return Self::disabled();
        }
        let mut lines = Self {
            enabled: true,
            session: session.to_owned(),
            ..Self::disabled()
        };
        if creates_session(args) {
            lines.reconcile(client, out).await;
        } else {
            // Without the fence, earlier runs' receipts would pass as this run's: report
            // the failure and print no hook lines rather than wrong ones.
            match page(client, session, 1, None).await {
                Ok((rows, _)) => lines.floor = rows.first().map(position),
                Err(error) => {
                    out.error(&format!("hook events disabled: {error}"));
                    return Self::disabled();
                }
            }
        }
        lines
    }

    /// Emit the line of a live receipt summary of this Session once it has settled.
    pub fn live(&mut self, event: &Event, out: &Out) {
        if !self.enabled || event.kind != RECEIPT_EVENT {
            return;
        }
        let receipt = &event.data["receipt"];
        if event.data["session_id"] == self.session.as_str() {
            self.emit(receipt, &receipt["decision"], out);
        }
    }

    /// Emit, oldest first, every settled receipt of this run the stream did not deliver.
    pub async fn reconcile(&mut self, client: &Client, out: &Out) {
        if !self.enabled {
            return;
        }
        match self.missing(client).await {
            Ok(rows) => {
                for row in rows.iter().rev() {
                    self.emit(row, &row["decision"]["decision"], out);
                }
            }
            Err(error) => out.error(&format!("hook events: {error}")),
        }
    }

    /// Durable receipts newer than the floor that have no line yet, newest first.
    async fn missing(&self, client: &Client) -> Result<Vec<Value>, String> {
        let mut found = Vec::new();
        let mut cursor = None;
        for _ in 0..MAX_PAGES {
            let (rows, next) = page(client, &self.session, PAGE, cursor.as_deref()).await?;
            for row in rows {
                if !self.after_floor(&position(&row)) {
                    return Ok(found);
                }
                found.push(row);
            }
            match next {
                Some(next) => cursor = Some(next),
                None => break,
            }
        }
        Ok(found)
    }

    fn after_floor(&self, at: &Position) -> bool {
        self.floor.as_ref().is_none_or(|floor| at > floor)
    }

    /// One `hook_event` line per settled receipt of this run, never twice.
    fn emit(&mut self, receipt: &Value, decision: &Value, out: &Out) {
        let Some(id) = receipt["id"].as_str() else {
            return;
        };
        if receipt["outcome"].is_null()
            || self.emitted.contains(id)
            || !self.after_floor(&position(receipt))
        {
            return;
        }
        self.emitted.insert(id.to_owned());
        out.stream_line("hook_event", line(receipt, decision));
    }
}

/// Whether this run starts its own Session rather than reusing one.
fn creates_session(args: &ExecArgs) -> bool {
    args.fork || (args.session.is_none() && !args.resume_last)
}

fn position(receipt: &Value) -> Position {
    (
        receipt["started_ms"].as_i64().unwrap_or_default(),
        receipt["id"].as_str().unwrap_or_default().to_owned(),
    )
}

/// The line's fields: identity, what the hook ran for, and how it ended. Free text
/// (the configured hook ID and the tool name) goes through credential redaction.
fn line(receipt: &Value, decision: &Value) -> Value {
    let text = |key: &str| {
        receipt[key]
            .as_str()
            .map(|value| cyber_core::redact::redact_credentials_with(value, "[REDACTED]"))
    };
    let mut line = json!({
        "receipt_id": receipt["id"],
        "hook_id": text("hook_id"),
        "event": receipt["event"],
        "scope": receipt["scope"],
        "outcome": receipt["outcome"],
        "decision": decision.as_str(),
        "duration_ms": receipt["duration_ms"],
        "started_ms": receipt["started_ms"],
    });
    if let Some(tool) = text("tool_name") {
        line["tool_name"] = json!(tool);
    }
    if let Some(call) = receipt["call_id"].as_str() {
        line["call_id"] = json!(call);
    }
    line
}

/// One page of the Session's durable receipts, newest first, and the next cursor.
async fn page(
    client: &Client,
    session: &str,
    limit: u32,
    cursor: Option<&str>,
) -> Result<(Vec<Value>, Option<String>), String> {
    let cursor = cursor
        .map(|cursor| format!("&cursor={}", encode(cursor)))
        .unwrap_or_default();
    let path = format!(
        "/sessions/{}/hook-executions?limit={limit}{cursor}",
        encode(session)
    );
    let body = client.get(&path).await.map_err(|e| e.to_string())?;
    let rows = body["data"].as_array().cloned().unwrap_or_default();
    let next = body["cursor"]["next"].as_str().map(str::to_owned);
    Ok((rows, next))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn receipt(id: &str, started: i64, outcome: Value) -> Value {
        json!({
            "id": id, "hook_id": "guard", "event": "PreToolUse", "scope": "global",
            "tool_name": "bash", "call_id": "call_1", "started_ms": started,
            "duration_ms": 4, "outcome": outcome,
            "io": {"stdin": "SECRET", "stdout": "SECRET", "stderr": "SECRET", "truncated": false},
        })
    }

    #[test]
    fn a_line_carries_the_summary_and_never_io() {
        let line = line(&receipt("hke_1", 10, json!("blocked")), &json!("deny"));
        assert_eq!(line["receipt_id"], "hke_1");
        assert_eq!(line["outcome"], "blocked");
        assert_eq!(line["decision"], "deny");
        assert_eq!(line["tool_name"], "bash");
        assert!(!line.to_string().contains("SECRET"), "{line}");
    }

    #[test]
    fn credential_shaped_hook_ids_are_redacted() {
        let mut row = receipt("hke_1", 10, json!("ok"));
        row["hook_id"] = json!("ghp_0123456789abcdefghijABCDEFGHIJ012345");
        let line = line(&row, &Value::Null);
        assert_eq!(line["hook_id"], "[REDACTED]", "{line}");
        assert!(line["decision"].is_null());
    }

    #[test]
    fn unsettled_repeated_and_earlier_receipts_get_no_line() {
        let mut lines = HookLines {
            enabled: true,
            session: "ses_1".into(),
            floor: Some((10, "hke_old".into())),
            emitted: HashSet::new(),
        };
        let out = Out::new(crate::cli::Format::Json, &ExecArgs::default(), &json!({}));
        lines.emit(&receipt("hke_run", 10, Value::Null), &Value::Null, &out);
        assert!(
            lines.emitted.is_empty(),
            "a running receipt has no line yet"
        );
        lines.emit(&receipt("hke_old", 10, json!("ok")), &Value::Null, &out);
        lines.emit(&receipt("hke_a", 9, json!("ok")), &Value::Null, &out);
        assert!(lines.emitted.is_empty(), "receipts of earlier runs");
        lines.emit(&receipt("hke_run", 10, json!("ok")), &Value::Null, &out);
        lines.emit(&receipt("hke_run", 10, json!("ok")), &Value::Null, &out);
        assert_eq!(lines.emitted.len(), 1);
    }

    #[test]
    fn only_new_sessions_skip_the_floor() {
        let args = |session: Option<&str>, resume: bool, fork: bool| ExecArgs {
            session: session.map(str::to_owned),
            resume_last: resume,
            fork,
            ..ExecArgs::default()
        };
        assert!(creates_session(&args(None, false, false)));
        assert!(creates_session(&args(Some("ses_1"), false, true)));
        assert!(!creates_session(&args(Some("ses_1"), false, false)));
        assert!(!creates_session(&args(None, true, false)));
    }
}
