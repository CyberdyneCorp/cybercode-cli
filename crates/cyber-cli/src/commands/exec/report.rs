//! Output for `cyber exec`: text, json and stream-json (`exec-mode` → Output formats,
//! Stream JSON event schema, Exit codes).

use std::collections::HashMap;
use std::io::Write;
use std::time::Instant;

use cyber_client::Client;
use serde_json::{Value, json};

use super::{EXIT_BUDGET, EXIT_DENIED, EXIT_ERROR, EXIT_INTERRUPTED, ExecArgs};
use crate::cli::Format;

const RESULT_LIMIT: usize = 2000;

/// What happened during the run.
pub struct Run {
    pub started: Instant,
    pub text: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cost: f64,
    pub turns: u32,
    pub denials: Vec<Value>,
    pub error: Option<String>,
    pub stop_reason: Option<String>,
    pub usage: Option<Value>,
    pub unpriced: bool,
    extra_tokens: u64,
    children_usage: Option<cyber_server::runtime::ChildrenUsage>,
    calls: HashMap<String, (String, Value)>,
    max_tokens: Option<u64>,
    max_cost: Option<f64>,
}

impl Run {
    pub fn new(args: &ExecArgs, started: Instant) -> Self {
        Self {
            started,
            text: String::new(),
            input_tokens: 0,
            output_tokens: 0,
            cost: 0.0,
            turns: 0,
            denials: Vec::new(),
            error: None,
            stop_reason: None,
            usage: None,
            unpriced: false,
            extra_tokens: 0,
            children_usage: None,
            calls: HashMap::new(),
            max_tokens: args.max_tokens,
            max_cost: args.max_cost,
        }
    }

    pub fn over_budget(&self) -> bool {
        self.max_tokens.is_some_and(|m| self.combined_tokens() >= m)
            || self.max_cost.is_some_and(|m| self.combined_cost() >= m)
    }

    fn combined_tokens(&self) -> u64 {
        self.input_tokens
            .saturating_add(self.output_tokens)
            .saturating_add(self.extra_tokens)
            .saturating_add(
                self.children_usage
                    .as_ref()
                    .map_or(0, |u| u.children_tokens),
            )
    }

    fn combined_cost(&self) -> f64 {
        self.cost
            + self
                .children_usage
                .as_ref()
                .map_or(0.0, |u| u.children_cost)
    }

    pub fn delegated_snapshot(&mut self, snapshot: &Value) -> Result<(), crate::error::CliError> {
        let budgeted = self.max_tokens.is_some() || self.max_cost.is_some();
        let has_usage = [
            "children_cost",
            "children_tokens",
            "children_unpriced_steps",
            "children_usage_complete",
        ]
        .iter()
        .any(|key| snapshot.get(*key).is_some());
        let children = if has_usage {
            let children: cyber_server::runtime::ChildrenUsage =
                serde_json::from_value(snapshot.clone()).map_err(|_| {
                    crate::error::CliError::runtime("Invalid descendant billing snapshot")
                })?;
            if !children.children_cost.is_finite() || children.children_cost < 0.0 {
                return Err(crate::error::CliError::runtime(
                    "Invalid descendant billing cost",
                ));
            }
            Some(children)
        } else {
            None
        };
        if budgeted && !children.as_ref().is_some_and(|u| u.children_usage_complete) {
            return Err(crate::error::CliError::runtime(
                "Budgeted delegation has incomplete descendant billing",
            ));
        }
        self.totals(&snapshot["totals"]);
        self.unpriced |= children
            .as_ref()
            .is_some_and(|u| u.children_unpriced_steps > 0);
        self.children_usage = children;
        Ok(())
    }

    pub fn totals(&mut self, totals: &Value) {
        let usage = &totals["usage"];
        self.input_tokens = usage["input"].as_u64().unwrap_or(0);
        self.output_tokens = usage["output"].as_u64().unwrap_or(0);
        self.extra_tokens = ["reasoning", "cache_read", "cache_write"]
            .iter()
            .map(|field| usage[*field].as_u64().unwrap_or(0))
            .fold(0u64, u64::saturating_add);
        self.cost = totals["cost"].as_f64().unwrap_or(0.0);
        self.turns = totals["steps"].as_u64().unwrap_or(0).min(u32::MAX as u64) as u32;
        self.unpriced = totals["unpriced_steps"].as_u64().unwrap_or(0) > 0;
        self.usage = Some(usage.clone());
    }

    fn exit_code(&self, fail_on_deny: bool) -> u8 {
        match self.stop_reason.as_deref() {
            Some("interrupted") => EXIT_INTERRUPTED,
            Some("timeout" | "budget_exceeded") => EXIT_BUDGET,
            _ if self.error.is_some() => EXIT_ERROR,
            _ if fail_on_deny && !self.denials.is_empty() => EXIT_DENIED,
            _ => 0,
        }
    }
}

pub struct Out {
    format: Format,
    verbose: bool,
    quiet: bool,
    thinking: bool,
    session_id: String,
    delegation: Option<(String, String)>,
    admission: Option<(String, String)>,
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

/// Why a tool result is a denial, if it is one.
fn denial_reason(output: &str) -> Option<&'static str> {
    if output.starts_with("Not pre-approved")
        || output.starts_with("Denied: no interactive approver")
        || output.starts_with("Blocked by auto mode")
        || output.starts_with("Plan mode is read-only")
    {
        Some("mode")
    } else if output.starts_with("Permission denied") {
        Some("rule")
    } else if output.contains("Blocked by the Cyber sandbox") {
        Some("sandbox")
    } else {
        None
    }
}

impl Out {
    pub fn new(format: Format, args: &ExecArgs, session: &Value) -> Self {
        Self {
            format,
            verbose: args.verbose,
            quiet: args.quiet,
            thinking: args.thinking,
            session_id: session["id"].as_str().unwrap_or_default().into(),
            delegation: None,
            admission: None,
        }
    }

    pub fn admission(&mut self, parent: &str, request: &str) {
        self.admission = Some((parent.into(), request.into()));
    }

    pub fn delegated(&mut self, parent: &str, job: &str, child: &str) {
        self.session_id = child.into();
        self.delegation = Some((parent.into(), job.into()));
    }

    fn line(&self, kind: &str, mut payload: Value) {
        if self.format != Format::StreamJson {
            return;
        }
        payload["type"] = json!(kind);
        payload["ts"] = json!(now_ms());
        payload["session_id"] = json!(self.session_id);
        let mut stdout = std::io::stdout().lock();
        let _ = writeln!(stdout, "{payload}");
        let _ = stdout.flush();
    }

    fn note(&self, text: &str) {
        if self.format == Format::Text && self.verbose && !self.quiet {
            eprintln!("{text}");
        }
    }

    pub async fn init(&self, client: &Client, session: &Value) {
        if self.format != Format::StreamJson {
            return;
        }
        let (agent, mode) = (
            session["agent"].as_str().unwrap_or("build"),
            session["mode"].as_str().unwrap_or("default"),
        );
        let tools = client
            .get(&format!("/tools?agent={agent}&mode={mode}"))
            .await
            .unwrap_or_default();
        let names: Vec<&str> = tools["data"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|t| t["name"].as_str())
            .collect();
        self.line(
            "system",
            json!({ "subtype": "init", "schema_version": 1, "model": session["model"], "agent": agent, "mode": mode, "cwd": session["directory"], "tools": names, "mcp_servers": [] }),
        );
    }

    pub fn error(&self, message: &str) {
        self.line("error", json!({ "message": message }));
        if self.format != Format::StreamJson && !self.quiet {
            eprintln!("error: {message}");
        }
    }

    /// A durable session event.
    pub fn durable(&mut self, run: &mut Run, kind: &str, data: &Value) {
        let base = kind.rsplit_once('.').map_or(kind, |(b, _)| b);
        match base {
            "session.text.ended" => {
                run.text = data["text"].as_str().unwrap_or_default().to_string();
                self.line(
                    "assistant_text",
                    json!({ "message_id": data["message_id"], "text": run.text }),
                );
            }
            "session.reasoning.ended" if self.thinking => {
                self.line("reasoning", json!({ "text": data["text"] }))
            }
            "session.tool.called" => self.tool_use(run, data),
            "session.tool.settled" => self.tool_result(run, data),
            "session.step.ended" => {
                run.turns += 1;
                run.input_tokens += data["usage"]["input"].as_u64().unwrap_or(0);
                run.output_tokens += data["usage"]["output"].as_u64().unwrap_or(0);
                run.cost += data["cost"].as_f64().unwrap_or(0.0);
            }
            // A failed step also publishes `session.error`, which reports it once; an
            // overflow that compaction recovers from is not an error of the run.
            "budget.exceeded" => {
                run.stop_reason
                    .get_or_insert_with(|| "budget_exceeded".into());
                self.line("budget", data.clone());
            }
            "budget.warned" => self.line("budget", data.clone()),
            "session.step.failed" => {}
            _ => {}
        }
    }

    fn tool_use(&self, run: &mut Run, data: &Value) {
        let (id, name) = (
            data["call_id"].as_str().unwrap_or_default(),
            data["name"].as_str().unwrap_or_default(),
        );
        let input = data.get("input").cloned().unwrap_or(Value::Null);
        run.calls.insert(id.into(), (name.into(), input.clone()));
        self.line(
            "tool_use",
            json!({ "call_id": id, "tool": name, "input": input }),
        );
        self.note(&format!("> {name} {}", summary(&input)));
    }

    fn tool_result(&self, run: &mut Run, data: &Value) {
        let id = data["call_id"].as_str().unwrap_or_default();
        let output = data["output"].as_str().unwrap_or_default();
        let truncated: String = output.chars().take(RESULT_LIMIT).collect();
        self.line(
            "tool_result",
            json!({ "call_id": id, "status": data["status"], "output": truncated }),
        );
        let Some(reason) = denial_reason(output) else {
            return;
        };
        let (tool, input) = run.calls.get(id).cloned().unwrap_or_default();
        let resource = summary(&input);
        let denial =
            json!({ "tool": tool, "resource": resource, "reason": reason, "message": output });
        self.line("permission_denied", denial.clone());
        if self.format == Format::Text && !self.quiet {
            eprintln!("denied: {tool} {resource} ({reason})");
        }
        run.denials.push(denial);
    }

    /// Print the result and return the exit code.
    pub fn finish(self, run: &Run, args: &ExecArgs, session: &Value) -> u8 {
        let code = run.exit_code(args.fail_on_deny);
        let mut result = json!({
            "result": run.text,
            "session_id": self.session_id,
            "usage": { "input": run.input_tokens, "output": run.output_tokens },
            "cost_usd": run.combined_cost(),
            "duration_ms": run.started.elapsed().as_millis() as u64,
            "num_turns": run.turns,
            "denials": run.denials,
            "stop_reason": run.stop_reason.clone().unwrap_or_else(|| if run.error.is_some() { "error".into() } else { "end_turn".into() }),
            "error": run.error,
            "exit_code": code,
        });
        if let Some(usage) = &run.usage {
            result["usage"] = usage.clone();
            result["cost_unpriced"] = run.unpriced.into();
        }
        if self.delegation.is_some() {
            result["own_cost_usd"] = run.cost.into();
            result["total_tokens"] = run.combined_tokens().into();
            result["children_usage_complete"] = run
                .children_usage
                .as_ref()
                .is_some_and(|u| u.children_usage_complete)
                .into();
            if let Some(children) = &run.children_usage {
                result["children_cost"] = children.children_cost.into();
                result["children_tokens"] = children.children_tokens.into();
                result["children_unpriced_steps"] = children.children_unpriced_steps.into();
            }
        }
        if let Some((parent, request)) = &self.admission {
            result["parent_session_id"] = parent.clone().into();
            result["delegation_id"] = request.clone().into();
        }
        if let Some((parent, job)) = &self.delegation {
            result["parent_session_id"] = parent.clone().into();
            result["job_id"] = job.clone().into();
        }
        match self.format {
            Format::StreamJson => self.line("result", result),
            Format::Json => println!("{result}"),
            _ => {
                if !run.text.is_empty() {
                    println!("{}", run.text);
                }
            }
        }
        if !args.ephemeral && !self.quiet && session["id"].is_string() {
            eprintln!("session: {}", self.session_id);
        }
        code
    }
}

/// A short description of a tool input for progress and denial lines.
fn summary(input: &Value) -> String {
    for key in ["command", "path", "pattern", "url", "query", "name"] {
        if let Some(v) = input[key].as_str() {
            return v.chars().take(120).collect();
        }
    }
    String::new()
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[derive(Parser)]
    struct Wrapper {
        #[command(flatten)]
        args: ExecArgs,
    }

    /// Regression: a provider error was printed twice, from the failed step and the
    /// session error event.
    #[test]
    fn a_failed_step_is_not_reported_separately_from_the_session_error() {
        let args = Wrapper::parse_from(["exec", "hi"]).args;
        let mut out = Out::new(Format::Json, &args, &json!({ "id": "ses_1" }));
        let mut run = Run::new(&args, Instant::now());
        out.durable(
            &mut run,
            "session.step.failed.1",
            &json!({ "kind": "invalid_request", "message": "no credit" }),
        );
        assert!(run.error.is_none());
        assert_eq!(run.exit_code(false), 0);
    }

    #[test]
    fn cumulative_descendant_snapshots_replace_usage_and_preserve_unpriced_charges() {
        let args = Wrapper::parse_from(["exec", "--max-tokens", "20", "hi"]).args;
        let mut run = Run::new(&args, Instant::now());
        let snapshot = json!({"totals":{"usage":{"input":2,"output":3,"reasoning":4,"cache_read":1,"cache_write":0},"steps":1,"cost":0.25,"unpriced_steps":0},
            "children_cost":0.5,"children_tokens":10,"children_unpriced_steps":1,"children_usage_complete":true});
        run.delegated_snapshot(&snapshot).unwrap();
        run.delegated_snapshot(&snapshot).unwrap();
        assert_eq!(run.combined_tokens(), 20);
        assert_eq!(run.combined_cost(), 0.75);
        assert_eq!(run.turns, 1);
        assert!(run.over_budget());
        assert!(run.unpriced);
        assert_eq!(run.usage.as_ref().unwrap()["input"], 2);
    }

    #[test]
    fn budgeted_snapshots_reject_missing_malformed_or_incomplete_descendant_usage() {
        let args = Wrapper::parse_from(["exec", "--max-cost", "2", "hi"]).args;
        let mut run = Run::new(&args, Instant::now());
        let good = json!({"totals":{"usage":{"input":1},"cost":0.1},
            "children_cost":0.5,"children_tokens":10,"children_unpriced_steps":0,"children_usage_complete":true});
        run.delegated_snapshot(&good).unwrap();
        for key in [
            "children_cost",
            "children_tokens",
            "children_unpriced_steps",
            "children_usage_complete",
        ] {
            let mut missing = good.clone();
            missing.as_object_mut().unwrap().remove(key);
            assert!(run.delegated_snapshot(&missing).is_err(), "{key}");
            let mut malformed = good.clone();
            malformed[key] = json!("invalid");
            assert!(run.delegated_snapshot(&malformed).is_err(), "{key}");
        }
        let mut negative = good.clone();
        negative["children_cost"] = json!(-1);
        assert!(run.delegated_snapshot(&negative).is_err());
        let mut incomplete = good.clone();
        incomplete["children_usage_complete"] = json!(false);
        assert!(run.delegated_snapshot(&incomplete).is_err());
        assert!(run.delegated_snapshot(&json!({"totals":{}})).is_err());
        assert_eq!(
            run.combined_cost(),
            0.6,
            "invalid snapshots do not clear known billing"
        );
    }

    #[test]
    fn unbudgeted_legacy_snapshot_preserves_unknown_descendant_attribution() {
        let args = Wrapper::parse_from(["exec", "hi"]).args;
        let mut run = Run::new(&args, Instant::now());
        run.delegated_snapshot(&json!({"totals":{"usage":{"input":1},"cost":0.1}}))
            .unwrap();
        assert!(run.children_usage.is_none());
        assert_eq!(run.combined_cost(), 0.1);
    }
    #[test]
    fn server_budget_events_preserve_soft_inflight_work_and_report_budget_exit() {
        let args = Wrapper::parse_from(["exec", "hi"]).args;
        let mut out = Out::new(Format::Json, &args, &json!({"id":"ses_1"}));
        let mut run = Run::new(&args, Instant::now());
        out.durable(&mut run, "budget.warned.1", &json!({"scope_id":"ses_1"}));
        assert!(run.stop_reason.is_none());
        out.durable(&mut run, "budget.exceeded.1", &json!({"scope_id":"ses_1"}));
        out.durable(
            &mut run,
            "session.text.ended.1",
            &json!({"text":"inflight answer"}),
        );
        assert_eq!(run.text, "inflight answer");
        assert_eq!(run.stop_reason.as_deref(), Some("budget_exceeded"));
        assert_eq!(run.exit_code(false), EXIT_BUDGET);
    }
}
