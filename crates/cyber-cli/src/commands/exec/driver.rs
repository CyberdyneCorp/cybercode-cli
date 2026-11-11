//! `cyber exec --input-format stream-json` (`exec-mode` → Streaming input and partial
//! output, Bounded interactive JSONL driver): a program drives one Session over stdin
//! for the life of the process.
//!
//! Lines are read on their own task, so an `interrupt` or a reply is handled while the
//! model is still streaming. Each `user` line is admitted with a message ID chosen here,
//! which correlates its `result` and makes a retry after a lost acknowledgement the same
//! prompt. Permission and question requests go to the driver instead of being declined;
//! after `end` or EOF nothing new is admitted, unanswered requests are declined, and the
//! process exits once its owned inputs and background work have settled.

mod interruption;

use std::collections::{BTreeMap, VecDeque};
use std::time::{Duration, Instant};

use cyber_client::{Client, Event};
use futures::StreamExt;
use serde_json::{Value, json};
use tokio::io::AsyncBufReadExt;
use tokio::sync::mpsc;

use super::frames::{self, Frame, MAX_LINE};
use super::inputs::{Input, Inputs};
use super::report::{Out, Run};
use super::{EXIT_ERROR, EXIT_INTERRUPTED, ExecArgs, NO_APPROVER, api, safe_id};
use crate::error::CliError;

/// How often the interactive lease (45 s) is renewed.
const LEASE_RENEWAL: Duration = Duration::from_secs(15);
/// A fatal protocol error.
const EXIT_PROTOCOL: u8 = 2;

/// What the stdin task sends.
enum Line {
    Frame(Frame),
    /// A line that is not a valid envelope: fatal, as the stream can no longer be trusted.
    Invalid(String),
    Eof,
}

/// The control loop selects a ready source before applying its side effects.
enum Wake {
    Input(Line),
    Event(Option<Event>),
    Poll,
    Renew,
    Interrupt,
}

/// A live permission/question request bound to this driver's exact admitted inputs.
struct Ask {
    session_id: String,
    question: bool,
    inputs: Vec<String>,
}

struct Driver<'a> {
    client: &'a Client,
    id: &'a str,
    args: &'a ExecArgs,
    out: &'a mut Out,
    run: Run,
    step: Option<String>,
    seen_seq: Option<i64>,
    background_failed: bool,
    /// Each new control must cover a previously uncovered pending input, so
    /// the 32-input capacity also bounds retained overlapping controls to 32.
    interruptions: VecDeque<interruption::Pending>,
    inputs: Inputs,
    /// Exact message identities of promoted inputs whose answer is in progress.
    answering: Vec<String>,
    /// The last finished step ended the answer (no tool calls followed).
    answer_done: bool,
    asks: BTreeMap<String, Ask>,
    /// No more input: `end`, EOF, a fatal error or SIGINT.
    closed: bool,
    idle: bool,
    fatal: Option<String>,
    interrupted: bool,
    last_text: String,
    /// The interactive lease that lets this Session ask the driver (`server-api` →
    /// Interactive clients); released when input closes.
    lease: Option<String>,
}

pub async fn execute(
    client: &Client,
    session: &Value,
    args: &ExecArgs,
    mut out: Out,
) -> Result<u8, CliError> {
    let id = session["id"].as_str().unwrap_or_default().to_owned();
    let mut run = Run::new(args, Instant::now());
    let baseline = client.get(&format!("/sessions/{id}")).await.map_err(api)?;
    run.begin_session(&baseline["data"])?;
    out.init_version(client, session, 2).await;
    let mut events = Box::pin(
        client
            .session_live_event_stream(&id, baseline["data"]["seq"].as_i64().unwrap_or(-1))
            .await
            .map_err(api)?,
    );
    let (sender, mut lines) = mpsc::channel(64);
    tokio::spawn(read_stdin(sender));
    let mut driver = Driver::new(client, &id, args, &mut out, run);
    driver.seen_seq = baseline["data"]["seq"].as_i64();
    driver.attach().await;
    driver.drive(&mut events, &mut lines).await;
    driver.release().await;
    Ok(driver.finish())
}

/// Read stdin line by line, each bounded to 1 MiB; an over-long line ends the reading.
async fn read_stdin(sender: mpsc::Sender<Line>) {
    let mut reader = tokio::io::BufReader::new(tokio::io::stdin());
    loop {
        let mut line = Vec::new();
        let bounded = tokio::io::AsyncReadExt::take(&mut reader, MAX_LINE as u64 + 2);
        tokio::pin!(bounded);
        let read = bounded.read_until(b'\n', &mut line).await;
        let message = match read {
            Ok(0) => Line::Eof,
            Err(e) => Line::Invalid(format!("stdin: {e}")),
            Ok(_) if line.last() != Some(&b'\n') && line.len() > MAX_LINE => {
                Line::Invalid("line longer than 1 MiB".into())
            }
            Ok(_) => {
                let trimmed = line.trim_ascii_end();
                if trimmed.is_empty() {
                    continue;
                }
                frames::parse(trimmed).map_or_else(Line::Invalid, Line::Frame)
            }
        };
        let last = !matches!(message, Line::Frame(_));
        if sender.send(message).await.is_err() || last {
            return;
        }
    }
}

/// Durable kinds carry a numeric version; live kinds such as session.idle do not.
fn event_base(kind: &str) -> &str {
    match kind.rsplit_once('.') {
        Some((base, version)) if version.bytes().all(|byte| byte.is_ascii_digit()) => base,
        _ => kind,
    }
}

impl<'a> Driver<'a> {
    fn new(
        client: &'a Client,
        id: &'a str,
        args: &'a ExecArgs,
        out: &'a mut Out,
        run: Run,
    ) -> Self {
        Self {
            client,
            id,
            args,
            out,
            run,
            step: None,
            seen_seq: None,
            background_failed: false,
            interruptions: VecDeque::new(),
            inputs: Inputs::default(),
            answering: Vec::new(),
            answer_done: false,
            asks: BTreeMap::new(),
            closed: false,
            idle: true,
            fatal: None,
            interrupted: false,
            last_text: String::new(),
            lease: None,
        }
    }

    async fn drive<S: futures::Stream<Item = Event> + Unpin>(
        &mut self,
        events: &mut S,
        lines: &mut mpsc::Receiver<Line>,
    ) {
        let mut renewal = tokio::time::interval(LEASE_RENEWAL);
        renewal.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut settle_poll = tokio::time::interval(Duration::from_millis(500));
        settle_poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let interrupt = tokio::signal::ctrl_c();
        tokio::pin!(interrupt);
        while !self.done().await {
            let wake = tokio::select! {
                line = lines.recv(), if !self.closed => Wake::Input(line.unwrap_or(Line::Eof)),
                event = events.next() => Wake::Event(event),
                _ = settle_poll.tick(), if self.inputs.has_ready() || !self.interruptions.is_empty() => Wake::Poll,
                _ = renewal.tick(), if self.lease.is_some() => Wake::Renew,
                _ = &mut interrupt, if !self.interrupted => Wake::Interrupt,
            };
            if self.dispatch(wake).await {
                return;
            }
        }
    }

    async fn dispatch(&mut self, wake: Wake) -> bool {
        match wake {
            Wake::Input(line) => self.line(line).await,
            Wake::Event(event) => return self.consume_event(event).await,
            Wake::Poll => {
                self.poll_interruption().await;
                if !self.background_failed {
                    self.refresh_background().await;
                }
            }
            Wake::Renew => self.renew().await,
            Wake::Interrupt => {
                self.interrupted = true;
                self.closed = true;
                self.interrupt().await;
                self.release().await;
            }
        }
        false
    }

    async fn consume_event(&mut self, event: Option<Event>) -> bool {
        match event {
            Some(event) => {
                self.event(event).await;
                false
            }
            None => {
                self.fatal
                    .get_or_insert_with(|| "the server closed the event stream".into());
                true
            }
        }
    }

    async fn line(&mut self, line: Line) {
        match line {
            Line::Frame(Frame::User {
                id,
                parts,
                delivery,
            }) => self.admit(id, parts, delivery).await,
            Line::Frame(Frame::Interrupt) => self.interrupt().await,
            Line::Frame(Frame::PermissionReply {
                request_id,
                reply,
                message,
            }) => {
                let body = json!({ "reply": reply, "message": message });
                self.reply(&request_id, false, body).await;
            }
            Line::Frame(Frame::QuestionReply {
                request_id,
                answers,
            }) => {
                self.reply(&request_id, true, json!({ "answers": answers }))
                    .await;
            }
            Line::Frame(Frame::End) | Line::Eof => self.close().await,
            Line::Invalid(why) => {
                self.protocol_error(&why);
                self.fatal = Some(format!("protocol error: {why}"));
                self.close().await;
            }
        }
    }

    /// Admit a prompt under a message ID chosen here; a lost acknowledgement is retried
    /// once with the same ID, which the server answers with the original receipt.
    async fn admit(&mut self, id: Option<String>, parts: Vec<Value>, delivery: &str) {
        let message_id = cyber_core::ids::new_id("msg");
        let input = Input::new(id, message_id.clone());
        if let Err(error) = self.inputs.admit(input) {
            self.protocol_error(error);
            self.fatal = Some(format!("protocol error: {error}"));
            return self.close().await;
        }
        let mut body = json!({ "id": message_id, "parts": parts, "delivery": delivery });
        if let Some(schema) = &self.args.schema {
            body["format"] = json!({ "type": "json_schema", "schema": schema });
        }
        let path = format!("/sessions/{}/prompt", self.id);
        let mut sent = self.client.post(&path, body.clone()).await;
        if matches!(&sent, Err(cyber_client::ClientError::Transport(_))) {
            sent = self.client.post(&path, body).await;
        }
        let input = self.inputs.get_mut(&message_id).expect("reserved input");
        match sent {
            Ok(_) => {
                self.idle = false;
                self.out.stream_line(
                    "input",
                    json!({ "id": input.id, "message_id": input.message_id, "status": "admitted" }),
                );
            }
            Err(error) => {
                input.error = Some(error.to_string());
                self.out.stream_line(
                    "input",
                    json!({ "id": input.id, "message_id": input.message_id, "status": "refused", "error": input.error }),
                );
                let input = self.inputs.complete(&message_id).expect("refused input");
                self.emit_result(&input);
            }
        }
    }

    /// Answer a request bound to this driver's live inputs; any other ID is a
    /// protocol error that leaves the run going.
    async fn reply(&mut self, request_id: &str, question: bool, body: Value) {
        let owned = self
            .asks
            .get(request_id)
            .filter(|ask| ask.question == question);
        let Some(ask) = owned else {
            return self.protocol_error(&format!(
                "no pending {} {request_id} of this run",
                if question {
                    "question"
                } else {
                    "permission request"
                }
            ));
        };
        let route = if question { "questions" } else { "permissions" };
        let path = format!("/sessions/{}/{route}/{request_id}/reply", ask.session_id);
        match self.client.post(&path, body).await {
            Ok(_) => {
                self.asks.remove(request_id);
            }
            Err(error) => self.protocol_error(&format!("reply to {request_id}: {error}")),
        }
    }

    /// Hold an interactive lease while the driver is live, so the Session's requests
    /// come here instead of being denied as unattended.
    async fn attach(&mut self) {
        let path = format!("/sessions/{}/interactive-clients", self.id);
        match self.client.post(&path, json!({})).await {
            Ok(body) => self.lease = body["data"]["client_id"].as_str().map(str::to_owned),
            Err(error) => self.out.error(&format!(
                "requests will be denied: no interactive lease ({error})"
            )),
        }
    }

    async fn renew(&mut self) {
        let Some(lease) = &self.lease else { return };
        let path = format!("/sessions/{}/interactive-clients/{lease}", self.id);
        if let Err(error) = self.client.request("PUT", &path, Some(json!({}))).await {
            self.out.error(&format!("interactive lease lost: {error}"));
            self.lease = None;
        }
    }

    async fn release(&mut self) {
        if let Some(lease) = self.lease.take() {
            let path = format!("/sessions/{}/interactive-clients/{lease}", self.id);
            let _ = self.client.request("DELETE", &path, None).await;
        }
    }

    /// No more input: the lease goes and unanswered requests are declined, so nothing
    /// waits on a driver that is gone.
    async fn close(&mut self) {
        self.closed = true;
        if self.fatal.is_some() {
            self.interrupt().await;
            self.release().await;
            return;
        }
        self.release().await;
        let asks = std::mem::take(&mut self.asks);
        for (request_id, ask) in asks {
            let (route, body) = if ask.question {
                ("questions", json!({ "answers": null }))
            } else {
                (
                    "permissions",
                    json!({ "reply": "reject", "message": NO_APPROVER }),
                )
            };
            let path = format!("/sessions/{}/{route}/{request_id}/reply", ask.session_id);
            let _ = self.client.post(&path, body).await;
        }
    }

    async fn event(&mut self, event: Event) {
        if !self.accept_sequence(&event).await {
            return;
        }
        let kind = event.kind.as_str();
        let base = event_base(kind);
        match base {
            "session.idle" => self.became_idle().await,
            "session.prompt.promoted" => self.promoted(&event.data),
            "session.step.started" => self.begin_step(&event.data).await,
            "session.step.ended" if self.owns_step(&event.data) => {
                self.answer_done = event.data["finish"] != "tool_calls";
                self.out.durable(&mut self.run, kind, &event.data);
            }
            "session.text.ended" if self.owns_step(&event.data) => {
                self.record_text(kind, &event.data)
            }
            "session.structured_output.settled" => self.structured(&event.data),
            "permission.asked" | "question.asked" => self.asked(base, &event.data).await,
            "permission.replied" | "question.replied" => {
                if let Some(id) = event.data["id"].as_str() {
                    self.asks.remove(id);
                }
            }
            "session.text.delta" | "session.reasoning.delta" => self.partial(base, &event.data),
            "session.error" if event.data["kind"] == "snapshot" => {
                eprintln!(
                    "snapshot warning: {}",
                    event.data["message"]
                        .as_str()
                        .unwrap_or("code rewind unavailable")
                );
            }
            "session.retry" if self.owns_step(&event.data) => self.out.retry(&event.data),
            "session.step.failed" if self.owns_step(&event.data) => {
                self.failed(&event.data);
                self.answer_done = true;
            }
            _ if self.allows_durable(&event.data) => {
                self.out.durable(&mut self.run, kind, &event.data)
            }
            _ => {}
        }
    }

    fn allows_durable(&self, data: &Value) -> bool {
        self.step.is_some() && (data.get("message_id").is_none() || self.owns_step(data))
    }

    fn partial(&self, base: &str, data: &Value) {
        if !self.args.include_partial_messages || !self.owns_step(data) {
            return;
        }
        let line = if base == "session.text.delta" {
            "text_delta"
        } else {
            "reasoning_delta"
        };
        self.out.stream_line(
            line,
            json!({"message_id":data["message_id"],"text":data["text"]}),
        );
    }

    async fn accept_sequence(&mut self, event: &Event) -> bool {
        if event.session_id.as_deref() != Some(self.id) {
            return false;
        }
        let Some(seq) = event.seq else {
            return true;
        };
        if self.seen_seq.is_some_and(|seen| seq <= seen) {
            return false;
        }
        if self.seen_seq.is_some_and(|seen| seq != seen + 1) {
            self.background_error(
                "Session event history has a gap; retained inputs require reconciliation".into(),
            )
            .await;
            return false;
        }
        self.seen_seq = Some(seq);
        true
    }

    async fn became_idle(&mut self) {
        self.idle = true;
        if !self.answer_done && !self.answering.is_empty() {
            for id in &self.answering {
                if let Some(input) = self.inputs.get_mut(id)
                    && input.structured.is_none()
                {
                    input.error.get_or_insert_with(|| {
                        "Execution stopped before a recorded final answer; inspect the Session"
                            .into()
                    });
                }
            }
        }
        self.stage_answers();
        self.step = None;
        self.refresh_background().await;
    }

    async fn refresh_background(&mut self) {
        if let Err(error) = self.poll_background().await {
            self.background_error(error.message).await;
        }
    }

    fn record_text(&mut self, kind: &str, data: &Value) {
        self.out.durable(&mut self.run, kind, data);
        self.last_text = self.run.text.clone();
        for id in &self.answering {
            if let Some(input) = self.inputs.get_mut(id) {
                input.text = self.run.text.clone();
            }
        }
    }

    /// A promoted input of this run joins the answer in progress, or starts the next one
    /// once the previous answer has ended.
    fn promoted(&mut self, data: &Value) {
        let Some(message_id) = data["message_id"].as_str() else {
            return;
        };
        if !self.inputs.contains(message_id) {
            return;
        }
        if self.answer_done {
            self.stage_answers();
        }
        if !self.inputs.contains(message_id) || self.answering.iter().any(|id| id == message_id) {
            return;
        }
        self.answer_done = false;
        self.idle = false;
        self.run.text.clear();
        self.answering.push(message_id.to_owned());
    }

    fn structured(&mut self, data: &Value) {
        let Some(input) = data["message_id"]
            .as_str()
            .and_then(|id| self.inputs.get_mut(id))
        else {
            return;
        };
        if let Some(error) = data["error"].as_str() {
            input.error.get_or_insert_with(|| error.to_owned());
        }
        input.structured = Some(data["value"].clone()).filter(|v| !v.is_null());
    }

    /// A failed Turn fails the inputs it was answering.
    fn failed(&mut self, data: &Value) {
        let message = data["message"].as_str().unwrap_or("error").to_owned();
        self.out.error(&message);
        for message_id in &self.answering {
            if let Some(input) = self.inputs.get_mut(message_id) {
                input.error.get_or_insert_with(|| message.clone());
            }
        }
    }

    async fn asked(&mut self, base: &str, data: &Value) {
        if !self.inputs.owns_request(self.id, data) {
            return;
        }
        let (Some(request_id), Some(session_id)) =
            (data["id"].as_str(), data["session_id"].as_str())
        else {
            return;
        };
        let question = base == "question.asked";
        let prefix = if question { "que" } else { "per" };
        if !safe_id(request_id, prefix) || !safe_id(session_id, "ses") {
            return;
        }
        self.asks.insert(
            request_id.to_owned(),
            Ask {
                session_id: session_id.to_owned(),
                question,
                inputs: data["input_message_ids"]
                    .as_array()
                    .expect("validated ownership")
                    .iter()
                    .filter_map(|id| id.as_str().map(str::to_owned))
                    .collect(),
            },
        );
        let line = if question {
            "question"
        } else {
            "permission_request"
        };
        self.out.stream_line(line, data.clone());
        if self.closed {
            // Nobody will answer: decline at once.
            self.close().await;
        }
    }

    /// Emit the results of the inputs whose answer ended.
    fn stage_answers(&mut self) {
        for message_id in std::mem::take(&mut self.answering) {
            if let Some(input) = self.inputs.get_mut(&message_id) {
                input.answer_ready = true;
                input.ready_seq = self.seen_seq.unwrap_or(-1);
            }
        }
        self.run.retire_driver_answer();
        self.answer_done = false;
    }

    fn emit_result(&self, input: &Input) {
        let result = match &input.structured {
            Some(value) => value.clone(),
            None if self.args.schema.is_some() => Value::Null,
            None => input.text.clone().into(),
        };
        self.out.stream_line(
            "result",
            json!({
                "id": input.id,
                "message_id": input.message_id,
                "result": result,
                "error": input.error,
                "exit_code": input.exit_code(),
            }),
        );
    }

    fn protocol_error(&self, why: &str) {
        self.out
            .stream_line("error", json!({ "code": "protocol", "message": why }));
    }

    /// Completion follows owned inputs and retained control, independently of
    /// another driver's Session activity.
    async fn done(&mut self) -> bool {
        self.background_failed
            || (self.closed && self.inputs.is_empty() && self.interruptions.is_empty())
    }

    fn owns_step(&self, data: &Value) -> bool {
        self.step
            .as_deref()
            .is_some_and(|step| data["message_id"].as_str() == Some(step))
    }

    async fn begin_step(&mut self, data: &Value) {
        self.idle = false;
        if self.answer_done {
            self.stage_answers();
        }
        self.step = None;
        let Some(message) = data["message_id"].as_str().filter(|id| safe_id(id, "msg")) else {
            return;
        };
        let response = self
            .client
            .get(&format!(
                "/sessions/{}/steps/{message}/input-origin",
                self.id
            ))
            .await;
        let roots = response.map_err(api).and_then(|response| {
            serde_json::from_value::<Vec<String>>(response["data"].clone())
                .map_err(|_| CliError::runtime("Invalid step input provenance"))
        });
        let roots = match roots {
            Ok(roots) => roots,
            Err(error) => return self.background_error(error.message).await,
        };
        if !self.inputs.owns_cohort(&roots) {
            return;
        }
        self.step = Some(message.to_owned());
        self.answer_done = false;
        self.run.text.clear();
        for root in roots {
            if let Some(input) = self.inputs.get_mut(&root) {
                input.answer_ready = false;
            }
            if !self.answering.contains(&root) {
                self.answering.push(root);
            }
        }
    }

    async fn poll_background(&mut self) -> Result<bool, CliError> {
        let ids = self.inputs.identities();
        if ids.is_empty() {
            return Ok(true);
        }
        let response = self
            .client
            .post(
                "/jobs/input-state",
                json!({"session_id":self.id,"input_message_ids":ids}),
            )
            .await
            .map_err(api)?;
        let states: Vec<cyber_server::runtime::InputJobState> =
            serde_json::from_value(response["data"].clone())
                .map_err(|_| CliError::runtime("Invalid input Job state"))?;
        let mut returned: Vec<_> = states
            .iter()
            .map(|state| state.message_id.clone())
            .collect();
        returned.sort();
        let mut expected = ids;
        expected.sort();
        if returned != expected {
            return Err(CliError::runtime(
                "Input Job lookup returned a different cohort",
            ));
        }
        if states
            .iter()
            .any(|state| self.seen_seq.is_none_or(|seq| state.session_seq > seq))
        {
            return Ok(false);
        }
        let mut settled = true;
        for state in states {
            settled &= !(state.running || state.awaiting_handback || state.handback_pending);
            if self.stopping_input(&state.message_id) {
                continue;
            }
            let Some(input) = self.inputs.get_mut(&state.message_id) else {
                continue;
            };
            if !input.answer_ready {
                continue;
            }
            if state.awaiting_handback {
                let since = input.unnotified_since.get_or_insert_with(Instant::now);
                if since.elapsed() < cyber_server::runtime::settlement::HANDBACK_GRACE {
                    continue;
                }
                input.error.get_or_insert_with(|| {
                    "Background handback was not admitted; inspect the Session Jobs".into()
                });
            } else {
                input.unnotified_since = None;
                if state.running
                    || state.handback_pending
                    || state.handback_seq.is_some_and(|seq| input.ready_seq < seq)
                {
                    continue;
                }
            }
            if let Some(input) = self.inputs.complete(&state.message_id) {
                self.emit_result(&input);
            }
        }
        Ok(settled)
    }

    async fn background_error(&mut self, message: String) {
        self.run.error = Some(format!(
            "Background ownership could not be reconciled; inspect Session {}: {message}",
            self.id
        ));
        self.background_failed = true;
        self.closed = true;
        self.release().await;
    }

    /// The aggregate exit: 130 for SIGINT, 2 for a fatal protocol error, else the first
    /// failed input's code in admission order. Ends with one `end` line.
    fn finish(mut self) -> u8 {
        for input in self.inputs.unfinished() {
            self.emit_result(&input);
        }
        let mut code = if self.interrupted {
            EXIT_INTERRUPTED
        } else if self.fatal.is_some() {
            EXIT_PROTOCOL
        } else {
            self.inputs
                .failure_code()
                .or(self.run.error.as_ref().map(|_| EXIT_ERROR))
                .unwrap_or(0)
        };
        let mut final_error = self.fatal.or(self.run.error.clone());
        if let Some(path) = &self.args.output_last_message
            && let Err(error) = super::last_message::write(path, &self.last_text)
        {
            let message = format!("--output-last-message: {error}");
            self.out.error(&message);
            final_error.get_or_insert(message);
            if code == 0 {
                code = EXIT_ERROR;
            }
        }
        self.out.stream_line(
            "end",
            json!({
                "exit_code": code,
                "error": final_error,
                "inputs": self.inputs.total(),
                "usage": { "input": self.run.input_tokens, "output": self.run.output_tokens },
                "cost_usd": self.run.combined_cost(),
                "duration_ms": self.run.started.elapsed().as_millis() as u64,
            }),
        );
        code
    }
}

#[cfg(test)]
mod request_tests;
