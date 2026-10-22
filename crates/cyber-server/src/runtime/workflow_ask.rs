//! `ask()` of a Workflow script (`workflows` → ask() user interaction, Run lifecycle
//! states).
//!
//! A question is a string or `{ question, options?, multiple? }`. An answer the invocation
//! declared in `args.answers` (keyed by the question text) is used without asking. A
//! non-interactive run has no one to ask and rejects with `NoInteractiveUser`. Otherwise the
//! run moves to `waiting_input` (`workflow.run.waiting.1`), the question goes to the owner
//! Session's clients through the question flow, and the answer moves the run back to
//! `running` (`workflow.run.answered.1`) before the script sees it. A pause or stop while
//! waiting withdraws the question and ends the attempt without recording the call, so a
//! resume asks again.

use std::sync::Arc;

use cyber_workflow::{Diagnostic, DiagnosticKind};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use super::requests::{Question, QuestionOption, QuestionReply};
use super::workflow_control::{Control, Signal};
use super::workflows::{RUN_ANSWERED, RUN_WAITING, WorkflowRunStatus};
use super::{Runtime, RuntimeError};

/// The longest question text, in characters.
const MAX_QUESTION_CHARS: usize = 2_000;
/// The most options a question may offer.
const MAX_OPTIONS: usize = 20;

/// A script's question, validated.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct ScriptQuestion {
    pub text: String,
    pub options: Vec<QuestionOption>,
    pub multiple: bool,
}

/// How waiting for an answer ended.
pub(super) enum Asked {
    Answer(Value),
    /// No answer will come: dismissed, or no client can answer.
    Unanswered(Diagnostic),
    /// The run was paused, stopped or shut down while waiting.
    Halted,
}

impl ScriptQuestion {
    /// `ask(question)`'s argument: a string or `{ question, options?, multiple? }`, where an
    /// option is a label or `{ label, description? }`.
    pub(super) fn parse(value: &Value) -> Result<Self, Diagnostic> {
        let (text, options, multiple) = match value {
            Value::String(text) => (text.as_str(), &Value::Null, false),
            Value::Object(map) => (
                map.get("question").and_then(Value::as_str).unwrap_or_default(),
                map.get("options").unwrap_or(&Value::Null),
                map.get("multiple").and_then(Value::as_bool).unwrap_or(false),
            ),
            _ => return Err(invalid("ask() expects a question string or { question }")),
        };
        if text.trim().is_empty() || text.chars().count() > MAX_QUESTION_CHARS {
            return Err(invalid(&format!(
                "ask() needs a question of 1 to {MAX_QUESTION_CHARS} characters"
            )));
        }
        Ok(Self {
            text: text.to_owned(),
            options: question_options(options)?,
            multiple,
        })
    }

    /// The answer `args.answers` declares for this question, for runs without a user.
    pub(super) fn declared(&self, args: &Value) -> Option<Value> {
        args.get("answers")?.get(&self.text).cloned()
    }

    fn request(&self, run_name: &str) -> Question {
        Question {
            question: self.text.clone(),
            header: format!("Workflow {run_name}"),
            options: self.options.clone(),
            multi_select: self.multiple,
            allow_custom: self.options.is_empty(),
        }
    }

    /// The script's value of an answer: the selections with `multiple`, else one string.
    fn value(&self, mut answers: Vec<Vec<String>>) -> Value {
        let selected = if answers.is_empty() {
            Vec::new()
        } else {
            answers.swap_remove(0)
        };
        if self.multiple {
            json!(selected)
        } else {
            json!(selected.into_iter().next().unwrap_or_default())
        }
    }
}

fn question_options(value: &Value) -> Result<Vec<QuestionOption>, Diagnostic> {
    let items = match value {
        Value::Null => return Ok(Vec::new()),
        Value::Array(items) if items.len() <= MAX_OPTIONS => items,
        _ => {
            return Err(invalid(&format!(
                "ask() options must be an array of at most {MAX_OPTIONS} labels"
            )));
        }
    };
    items
        .iter()
        .map(|item| {
            let (label, description) = match item {
                Value::String(label) => (label.as_str(), ""),
                Value::Object(map) => (
                    map.get("label").and_then(Value::as_str).unwrap_or_default(),
                    map.get("description").and_then(Value::as_str).unwrap_or_default(),
                ),
                _ => ("", ""),
            };
            if label.trim().is_empty() {
                return Err(invalid("each ask() option needs a label"));
            }
            Ok(QuestionOption {
                label: label.to_owned(),
                description: description.to_owned(),
            })
        })
        .collect()
}

fn invalid(message: &str) -> Diagnostic {
    Diagnostic::execution(message)
}

/// The rejection of an `ask()` no one can answer.
pub(super) fn no_user(message: &str) -> Diagnostic {
    Diagnostic::new(DiagnosticKind::NoInteractiveUser, message)
}

impl Runtime {
    /// Ask the owner's clients and wait for the answer, keeping the run `waiting_input`
    /// meanwhile.
    pub(super) async fn workflow_ask(
        &self,
        run_id: &str,
        question: &ScriptQuestion,
        control: Arc<Control>,
    ) -> Result<Asked, RuntimeError> {
        let Some(run) = self.mark_waiting(run_id, true).await? else {
            return Ok(Asked::Halted);
        };
        let asker = self.operation_asker(&run.session_id).await?;
        let reply = tokio::select! {
            reply = asker.question(vec![question.request(&run.name)]) => Some(reply),
            () = halted(&control) => None,
        };
        let Some(reply) = reply else {
            asker.cancel_questions().await?;
            return Ok(Asked::Halted);
        };
        if self.mark_waiting(run_id, false).await?.is_none() {
            return Ok(Asked::Halted);
        }
        Ok(match reply {
            QuestionReply::Answers { answers } => Asked::Answer(question.value(answers)),
            QuestionReply::Dismissed => {
                Asked::Unanswered(no_user("the user dismissed the question"))
            }
            QuestionReply::Unattended => Asked::Unanswered(no_user(
                "no interactive user can answer ask() in this run",
            )),
        })
    }

    /// Move a running run to `waiting_input` (`waiting`) or a waiting one back to
    /// `running`; `None` when a pause or stop came first.
    async fn mark_waiting(
        &self,
        run_id: &str,
        waiting: bool,
    ) -> Result<Option<super::WorkflowRun>, RuntimeError> {
        let lock = self.workflow_lock(run_id);
        let _guard = lock.lock().await;
        let mut run = self.existing_workflow_run(run_id)?;
        let (from, to, kind) = if waiting {
            (
                WorkflowRunStatus::Running,
                WorkflowRunStatus::WaitingInput,
                RUN_WAITING,
            )
        } else {
            (
                WorkflowRunStatus::WaitingInput,
                WorkflowRunStatus::Running,
                RUN_ANSWERED,
            )
        };
        if run.status != from {
            return Ok(None);
        }
        run.status = to;
        self.run_log().record(&run, kind, json!({}))?;
        Ok(Some(run))
    }
}

/// Resolves once the attempt is paused, stopped or shut down.
async fn halted(control: &Control) {
    let cancel: &CancellationToken = &control.cancel;
    loop {
        let changed = control.changed.notified();
        futures::pin_mut!(changed);
        changed.as_mut().enable();
        if cancel.is_cancelled() || control.signal() != Signal::Running {
            return;
        }
        tokio::select! {
            () = changed => {}
            () = cancel.cancelled() => return,
        }
    }
}
