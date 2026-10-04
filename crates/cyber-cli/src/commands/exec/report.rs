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
            calls: HashMap::new(),
            max_tokens: args.max_tokens,
            max_cost: args.max_cost,
        }
    }

    pub fn over_budget(&self) -> bool {
        self.max_tokens
            .is_some_and(|m| self.input_tokens + self.output_tokens > m)
            || self.max_cost.is_some_and(|m| self.cost > m)
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
        }
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
        let result = json!({
            "result": run.text,
            "session_id": self.session_id,
            "usage": { "input": run.input_tokens, "output": run.output_tokens },
            "cost_usd": run.cost,
            "duration_ms": run.started.elapsed().as_millis() as u64,
            "num_turns": run.turns,
            "denials": run.denials,
            "stop_reason": run.stop_reason.clone().unwrap_or_else(|| if run.error.is_some() { "error".into() } else { "end_turn".into() }),
            "error": run.error,
            "exit_code": code,
        });
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
}
