//! Output for `cyber exec`: text, json and stream-json (`exec-mode` → Output formats,
//! Stream JSON event schema, Exit codes).

use std::collections::HashMap;
use std::io::Write;
use std::time::Instant;

use cyber_client::{Client, ClientError};
use cyber_core::review::{Counts, ErrorCode, Findings, Target, TargetError};
use serde_json::{Value, json};

use super::background::ForkOutcome;
use super::{EXIT_BUDGET, EXIT_DENIED, EXIT_ERROR, EXIT_INTERRUPTED, ExecArgs};
use crate::cli::Format;
use crate::error::{CliError, EXIT_USAGE};

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
    pub stop_requested: Option<Instant>,
    pub usage: Option<Value>,
    pub unpriced: bool,
    extra_tokens: u64,
    children_usage: Option<cyber_server::runtime::ChildrenUsage>,
    session_baseline: Option<Value>,
    calls: HashMap<String, (String, Value)>,
    max_tokens: Option<u64>,
    max_cost: Option<f64>,
    /// `--goal` outcomes, in queue order.
    pub goals: Option<Vec<Value>>,
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
            stop_requested: None,
            usage: None,
            unpriced: false,
            extra_tokens: 0,
            children_usage: None,
            session_baseline: None,
            calls: HashMap::new(),
            max_tokens: args.max_tokens,
            max_cost: args.max_cost,
            goals: None,
        }
    }

    /// Whether `--max-tokens` or `--max-cost` limits this run.
    pub fn has_budget(&self) -> bool {
        self.max_tokens.is_some() || self.max_cost.is_some()
    }

    pub fn over_budget(&self) -> bool {
        self.max_tokens.is_some_and(|m| self.combined_tokens() >= m)
            || self.max_cost.is_some_and(|m| self.combined_cost() >= m)
    }

    pub fn begin_session(&mut self, snapshot: &Value) -> Result<(), crate::error::CliError> {
        validate_session_totals(snapshot)?;
        self.delegated_snapshot(snapshot)?;
        self.session_baseline = Some(snapshot.clone());
        self.session_snapshot(snapshot)
    }

    pub fn session_snapshot(&mut self, snapshot: &Value) -> Result<(), crate::error::CliError> {
        let baseline = self
            .session_baseline
            .as_ref()
            .ok_or_else(|| crate::error::CliError::runtime("Missing exec billing baseline"))?;
        let delta = session_delta(snapshot, baseline)?;
        self.delegated_snapshot(&delta)
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
                "Budgeted execution has incomplete descendant billing",
            ));
        }
        self.totals(&snapshot["totals"]);
        self.unpriced |= children
            .as_ref()
            .is_none_or(|u| !u.children_usage_complete || u.children_unpriced_steps > 0);
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
        let goal = self
            .goals
            .as_deref()
            .and_then(super::goals::exit_code)
            .map(|(code, _)| code);
        match self.stop_reason.as_deref() {
            Some("interrupted") => EXIT_INTERRUPTED,
            Some("timeout" | "budget_exceeded") => EXIT_BUDGET,
            _ if goal.is_some() => goal.unwrap_or(EXIT_ERROR),
            _ if self.error.is_some() => EXIT_ERROR,
            _ if fail_on_deny && !self.denials.is_empty() => EXIT_DENIED,
            _ => 0,
        }
    }
}

fn billing_error() -> crate::error::CliError {
    crate::error::CliError::runtime("Invalid or decreasing exec billing snapshot")
}

fn count(value: &Value) -> Result<u64, crate::error::CliError> {
    value.as_u64().ok_or_else(billing_error)
}

fn cost(value: &Value) -> Result<f64, crate::error::CliError> {
    value
        .as_f64()
        .filter(|cost| cost.is_finite() && *cost >= 0.0)
        .ok_or_else(billing_error)
}

fn validate_session_totals(snapshot: &Value) -> Result<(), crate::error::CliError> {
    if !snapshot["id"].is_string() || !snapshot["totals"]["usage"].is_object() {
        return Err(billing_error());
    }
    cost(&snapshot["totals"]["cost"])?;
    for field in ["input", "output", "reasoning", "cache_read", "cache_write"] {
        count(&snapshot["totals"]["usage"][field])?;
    }
    count(&snapshot["totals"]["steps"])?;
    count(&snapshot["totals"]["unpriced_steps"])?;
    Ok(())
}

fn session_delta(snapshot: &Value, baseline: &Value) -> Result<Value, crate::error::CliError> {
    validate_session_totals(snapshot)?;
    if [
        "children_cost",
        "children_tokens",
        "children_unpriced_steps",
        "children_usage_complete",
    ]
    .iter()
    .any(|field| snapshot.get(*field).is_some())
    {
        serde_json::from_value::<cyber_server::runtime::ChildrenUsage>(snapshot.clone())
            .map_err(|_| billing_error())?;
    }
    if snapshot["id"] != baseline["id"] {
        return Err(billing_error());
    }
    let mut delta = json!({"totals":{"usage":{}}});
    for field in ["input", "output", "reasoning", "cache_read", "cache_write"] {
        delta["totals"]["usage"][field] = count(&snapshot["totals"]["usage"][field])?
            .checked_sub(count(&baseline["totals"]["usage"][field])?)
            .ok_or_else(billing_error)?
            .into();
    }
    for field in ["steps", "unpriced_steps"] {
        delta["totals"][field] = count(&snapshot["totals"][field])?
            .checked_sub(count(&baseline["totals"][field])?)
            .ok_or_else(billing_error)?
            .into();
    }
    let own_cost = cost(&snapshot["totals"]["cost"])? - cost(&baseline["totals"]["cost"])?;
    if own_cost < 0.0 {
        return Err(billing_error());
    }
    delta["totals"]["cost"] = own_cost.into();
    if baseline.get("children_cost").is_some() {
        let children_cost = cost(&snapshot["children_cost"])? - cost(&baseline["children_cost"])?;
        if children_cost < 0.0 {
            return Err(billing_error());
        }
        delta["children_cost"] = children_cost.into();
        for field in ["children_tokens", "children_unpriced_steps"] {
            delta[field] = count(&snapshot[field])?
                .checked_sub(count(&baseline[field])?)
                .ok_or_else(billing_error)?
                .into();
        }
        delta["children_usage_complete"] = json!(
            snapshot["children_usage_complete"] == true
                && baseline["children_usage_complete"] == true
        );
    }
    Ok(delta)
}

pub struct Out {
    format: Format,
    verbose: bool,
    quiet: bool,
    thinking: bool,
    session_id: String,
    delegation: Option<(String, String)>,
    admission: Option<(String, String)>,
    review: Option<Review>,
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
        || output.starts_with(&format!("Rejected by user: {}", super::NO_APPROVER))
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
            review: None,
        }
    }

    /// Report this run as `cyber review` of `target`.
    pub fn review(&mut self, target: Target) {
        self.review = Some(Review {
            target,
            delegation: None,
            fork: None,
        });
    }

    pub fn is_review(&self) -> bool {
        self.review.is_some()
    }

    pub fn is_text(&self) -> bool {
        self.format == Format::Text
    }

    /// A Workflow Run's progress changed (`exec-mode` → Workflow exec).
    pub fn workflow_progress(&self, progress: &Value) {
        self.line("workflow_progress", progress.clone());
    }

    /// The result of an exec Workflow Run: its return value (text format prints it as
    /// JSON), status, spend and exit code.
    pub fn workflow_finish(
        self,
        run: &Value,
        code: u8,
        started: Instant,
        args: &ExecArgs,
        session: &Value,
    ) -> u8 {
        let status = run["status"].as_str().unwrap_or("failed");
        let stop_reason = match (status, code) {
            (_, super::EXIT_INTERRUPTED) => "interrupted",
            ("completed", _) => "end_turn",
            ("budget_exceeded", _) => "budget_exceeded",
            _ => "error",
        };
        let error = run
            .get("error")
            .filter(|error| !error.is_null())
            .map(|error| {
                json!(format!(
                    "{}: {}",
                    error["kind"].as_str().unwrap_or("?"),
                    error["message"].as_str().unwrap_or("?")
                ))
            })
            .or_else(|| run.get("reason").filter(|r| r.is_string()).cloned())
            .unwrap_or(Value::Null);
        if code != 0
            && let Some(message) = error.as_str()
        {
            self.error(message);
        }
        let value = run
            .get("result")
            .filter(|result| !result.is_null())
            .cloned()
            .unwrap_or(Value::Null);
        let result = json!({
            "result": value,
            "run_id": run["id"],
            "status": status,
            "result_file": run["result_file"],
            "session_id": self.session_id,
            "usage": {"tokens": run["tokens"]},
            "cost_usd": run["cost_usd"],
            "duration_ms": started.elapsed().as_millis() as u64,
            "stop_reason": stop_reason,
            "error": error,
            "exit_code": code,
        });
        let text = match (&value, run["result_file"].as_str()) {
            (Value::Null, Some(file)) => format!("result: {file}"),
            (Value::Null, None) => String::new(),
            (Value::String(text), _) => text.clone(),
            (value, _) => serde_json::to_string_pretty(value).unwrap_or_default(),
        };
        self.emit(result, &text, args, session);
        code
    }

    /// The settled reviewer delegation and its Job, when the command forked.
    pub fn review_job(&mut self, delegation: Option<&str>, fork: Option<ForkOutcome>) {
        if let Some(review) = &mut self.review {
            review.delegation = delegation.map(str::to_string);
            review.fork = fork;
        }
    }

    /// A refused admission. For `cyber review` a target error or a permission
    /// denial is the outcome (an empty diff is a completed review with nothing
    /// to report); both print the review result shape.
    pub fn refused(
        mut self,
        error: ClientError,
        run: &Run,
        args: &ExecArgs,
        session: &Value,
    ) -> Result<u8, CliError> {
        let Some(review) = self.review.take() else {
            return Err(super::setup::api(error));
        };
        let (target, kind) = (review.target.normalized(), Some(review.target.kind()));
        let mut result = self.summary(run, 0);
        let (code, text, message) = if let Some(rejected) = target_error(&error) {
            let (code, text) = rejection(&mut result, &target, kind, &rejected);
            (code, text, rejected.message)
        } else if let Some(message) = forbidden(&error) {
            let code = denied(&mut result, &target, kind, &message, args.fail_on_deny);
            (code, String::new(), message)
        } else {
            return Err(super::setup::api(error));
        };
        if code != 0 {
            self.error(&message);
        }
        self.emit(result, &text, args, session);
        Ok(code)
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
                if run.session_baseline.is_none() {
                    run.turns += 1;
                    run.input_tokens += data["usage"]["input"].as_u64().unwrap_or(0);
                    run.output_tokens += data["usage"]["output"].as_u64().unwrap_or(0);
                    run.cost += data["cost"].as_f64().unwrap_or(0.0);
                }
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
    pub fn finish(mut self, run: &Run, args: &ExecArgs, session: &Value) -> u8 {
        let mut code = run.exit_code(args.fail_on_deny);
        let mut result = self.summary(run, code);
        let text = match self.review.take() {
            Some(review) => review.report(&mut result, &mut code, run, &self),
            None => run.text.clone(),
        };
        self.emit(result, &text, args, session);
        code
    }

    fn summary(&self, run: &Run, code: u8) -> Value {
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
        if let Some(goals) = &run.goals {
            result["goals"] = goals.clone().into();
        }
        if self.delegation.is_some() || run.session_baseline.is_some() {
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
        result
    }

    fn emit(&self, result: Value, text: &str, args: &ExecArgs, session: &Value) {
        match self.format {
            Format::StreamJson => self.line("result", result),
            Format::Json => println!("{result}"),
            _ => {
                if !text.is_empty() {
                    println!("{}", text.trim_end());
                }
            }
        }
        if !args.ephemeral && !self.quiet && session["id"].is_string() {
            eprintln!("session: {}", self.session_id);
        }
    }
}

/// `cyber review`: the target and, once the run settled, the reviewer Job.
struct Review {
    target: Target,
    delegation: Option<String>,
    /// `None` for an inline (overriding) command.
    fork: Option<ForkOutcome>,
}

/// What the reviewer Job reported.
enum Verdict {
    Findings(Findings),
    /// An overriding command: no fork Job, or one without a structured result.
    Unstructured,
    Failed(String),
}

impl Review {
    fn job(&self) -> Option<&Value> {
        match &self.fork {
            Some(ForkOutcome::Job(job)) => Some(job),
            _ => None,
        }
    }

    fn verdict(&self) -> Verdict {
        let job = match &self.fork {
            None => return Verdict::Unstructured,
            Some(ForkOutcome::Ended(error)) => return Verdict::Failed(error.clone()),
            Some(ForkOutcome::Job(job)) => job,
        };
        if job["status"] != "completed" {
            let error = job["error"]
                .as_str()
                .unwrap_or("The reviewer did not complete");
            return Verdict::Failed(error.into());
        }
        match job["result"].get("result") {
            Some(value) => Findings::from_result(value).map_or_else(
                |error| Verdict::Failed(error.to_string()),
                Verdict::Findings,
            ),
            None => Verdict::Unstructured,
        }
    }

    /// Adds the review fields to `result`; returns the text output. Findings
    /// are reported only when the run itself completed.
    fn report(&self, result: &mut Value, code: &mut u8, run: &Run, out: &Out) -> String {
        target_fields(result, &self.target.normalized(), Some(self.target.kind()));
        if let Some(id) = &self.delegation {
            result["delegation_id"] = json!(id);
        }
        if let Some(job) = self.job() {
            result["job_id"] = job["id"].clone();
            result["parent_session_id"] = result["session_id"].clone();
            result["session_id"] = job["child_id"].clone();
        }
        if run.error.is_some() || run.stop_reason.is_some() {
            return run.text.clone();
        }
        match self.verdict() {
            Verdict::Findings(findings) => {
                result["reviewed"] = json!(true);
                result["findings"] = json!(findings.findings);
                result["limitations"] = json!(findings.limitations);
                result["counts"] = json!(findings.counts());
                findings.render_text()
            }
            Verdict::Unstructured => {
                result["reviewed"] = json!(true);
                run.text.clone()
            }
            Verdict::Failed(message) => {
                out.error(&message);
                *code = EXIT_ERROR;
                result["error"] = json!(message);
                result["stop_reason"] = json!("error");
                result["exit_code"] = json!(EXIT_ERROR);
                String::new()
            }
        }
    }
}

/// The review fields every outcome carries; findings stay `null` until validated.
fn target_fields(result: &mut Value, target: &str, kind: Option<&str>) {
    result["target"] = json!(target);
    result["target_kind"] = json!(kind);
    result["reviewed"] = json!(false);
    result["findings"] = Value::Null;
    result["limitations"] = json!([]);
    result["counts"] = json!(Counts::default());
}

/// Fills a rejected target's outcome and returns its exit code and text output:
/// an empty diff is a completed review with no findings, anything else exits 2.
fn rejection(
    result: &mut Value,
    target: &str,
    kind: Option<&str>,
    error: &TargetError,
) -> (u8, String) {
    target_fields(result, target, kind);
    if error.code == ErrorCode::EmptyDiff {
        result["findings"] = json!([]);
        result["exit_code"] = json!(0);
        return (0, error.message.clone());
    }
    result["code"] = json!(error.code);
    result["error"] = json!(error.message);
    result["stop_reason"] = json!("error");
    result["exit_code"] = json!(EXIT_USAGE);
    (EXIT_USAGE, String::new())
}

/// Fills a review refused by a permission rule (HTTP 403) before admission
/// and returns its exit code: 5 with `--fail-on-deny`, otherwise 1.
fn denied(
    result: &mut Value,
    target: &str,
    kind: Option<&str>,
    message: &str,
    fail_on_deny: bool,
) -> u8 {
    target_fields(result, target, kind);
    let code = if fail_on_deny {
        EXIT_DENIED
    } else {
        EXIT_ERROR
    };
    result["error"] = json!(message);
    result["stop_reason"] = json!("error");
    result["exit_code"] = json!(code);
    code
}

/// The message of a `ForbiddenError` (permission) refusal of the admission.
fn forbidden(error: &ClientError) -> Option<String> {
    match error {
        ClientError::Api { tag, message, .. } if tag == "ForbiddenError" => Some(message.clone()),
        _ => None,
    }
}

/// A `ReviewTargetError` refusal of the command admission.
fn target_error(error: &ClientError) -> Option<TargetError> {
    let ClientError::Api {
        tag, message, body, ..
    } = error
    else {
        return None;
    };
    if tag != "ReviewTargetError" {
        return None;
    }
    let code = serde_json::from_value(body["code"].clone()).ok()?;
    Some(TargetError::new(code, message.clone()))
}

/// `cyber review` with a target rejected before any server contact; returns the exit code.
pub fn target_rejected(format: Format, target: &str, error: &TargetError) -> u8 {
    let mut result = json!({ "result": "", "session_id": null, "error": null });
    let (code, _) = rejection(&mut result, target, None, error);
    match format {
        Format::Json => println!("{result}"),
        Format::StreamJson => {
            result["type"] = json!("result");
            result["ts"] = json!(now_ms());
            println!("{result}");
        }
        _ => eprintln!("Error: {}", cyber_core::review::escape(&error.message)),
    }
    code
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

    /// Regression: a reviewer fork that failed before a Job existed (delegation
    /// admission denied the spawn) was reported as a completed override review.
    #[test]
    fn a_review_fork_that_ended_without_a_job_fails_the_review() {
        let args = Wrapper::parse_from(["exec", "hi"]).args;
        let mut out = Out::new(Format::Json, &args, &json!({ "id": "ses_1" }));
        out.review(Target::Uncommitted);
        out.review_job(
            Some("op_skill_1"),
            Some(ForkOutcome::Ended(
                "Permission denied: agent reviewer".into(),
            )),
        );
        let run = Run::new(&args, Instant::now());
        let review = out.review.take().unwrap();
        let mut code = 0;
        let mut result = out.summary(&run, code);
        review.report(&mut result, &mut code, &run, &out);
        assert_eq!(code, EXIT_ERROR);
        assert_eq!(result["reviewed"], false);
        assert_eq!(result["findings"], Value::Null);
        assert_eq!(result["error"], "Permission denied: agent reviewer");
        assert_eq!(result["delegation_id"], "op_skill_1");
        assert_eq!(result["exit_code"], EXIT_ERROR);

        let mut out = Out::new(Format::Json, &args, &json!({ "id": "ses_1" }));
        out.review(Target::Uncommitted);
        out.review_job(None, None);
        let review = out.review.take().unwrap();
        let (mut code, mut result) = (0, out.summary(&run, 0));
        review.report(&mut result, &mut code, &run, &out);
        assert_eq!(
            (code, &result["reviewed"]),
            (0, &json!(true)),
            "inline override"
        );
    }

    #[test]
    fn only_forbidden_refusals_become_review_denials() {
        let api = |tag: &str| ClientError::Api {
            status: 403,
            tag: tag.into(),
            message: "Permission denied: denied by rule for bash".into(),
            body: Value::Null,
        };
        assert_eq!(
            forbidden(&api("ForbiddenError")).as_deref(),
            Some("Permission denied: denied by rule for bash")
        );
        assert_eq!(forbidden(&api("ConflictError")), None);
        let mut result = json!({});
        assert_eq!(
            denied(&mut result, "pr 3", Some("pr"), "denied", true),
            EXIT_DENIED
        );
        assert_eq!(result["findings"], Value::Null);
        assert_eq!(result["reviewed"], false);
        assert_eq!(result["target"], "pr 3");
        assert_eq!(denied(&mut result, "", None, "denied", false), EXIT_ERROR);
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

    #[test]
    fn ordinary_snapshot_deltas_replace_totals_without_duplicate_step_billing() {
        let args = Wrapper::parse_from(["exec", "hi"]).args;
        let mut run = Run::new(&args, Instant::now());
        let baseline = json!({"id":"ses_1","totals":{"usage":{"input":10,"output":0,"reasoning":0,"cache_read":0,"cache_write":0},"cost":2.0,"steps":1,"unpriced_steps":0},"children_cost":3.0,"children_tokens":10,"children_unpriced_steps":0,"children_usage_complete":true});
        run.begin_session(&baseline).unwrap();
        let mut current = baseline.clone();
        current["totals"]["usage"]["input"] = json!(20);
        current["totals"]["cost"] = json!(2.5);
        current["children_tokens"] = json!(20);
        current["children_cost"] = json!(3.5);
        run.session_snapshot(&current).unwrap();
        let mut out = Out::new(Format::Json, &args, &baseline);
        out.durable(
            &mut run,
            "session.step.ended.1",
            &json!({"usage":{"input":10},"cost":0.5}),
        );
        assert_eq!(run.combined_cost(), 1.0);
        assert_eq!(run.combined_tokens(), 20);
        run.session_snapshot(&current).unwrap();
        assert_eq!(run.combined_tokens(), 20);
        let mut invalid = current.clone();
        invalid["children_tokens"] = json!(-1);
        assert!(run.session_snapshot(&invalid).is_err());
        invalid = current.clone();
        invalid["totals"]["cost"] = json!(1.0);
        assert!(run.session_snapshot(&invalid).is_err());
        invalid = current;
        invalid["id"] = json!("ses_other");
        assert!(run.session_snapshot(&invalid).is_err());
        invalid["id"] = json!("ses_1");
        invalid["totals"]["usage"]
            .as_object_mut()
            .unwrap()
            .remove("cache_write");
        assert!(run.session_snapshot(&invalid).is_err());
        assert_eq!(run.combined_cost(), 1.0);
    }
}
