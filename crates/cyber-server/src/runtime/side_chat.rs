//! Side chat (`session-runtime` → Side chat, `tui` → Subagent threads and side chat): a
//! question asked beside the Session with a read-only copy of its context, answered by the
//! Session's model with read-only tools only. Nothing enters the Session's history; only
//! the call's usage is recorded, as for a recap.

use cyber_llm::catalog::compute_cost;
use cyber_llm::{Adapter, Content, Message, Role, Usage, collect};
use serde::Serialize;
use tokio_util::sync::CancellationToken;

use super::events::{AUXILIARY_USAGE, AuxiliaryUsage, event};
use super::{Asker, Inner, Invocation, Runtime, RuntimeError, ToolDef, ToolOutcome};

/// The tools a side chat may call; each still passes the Session's permission rules,
/// and anything that would ask is refused.
pub const READ_ONLY_TOOLS: &[&str] = &["read", "list", "glob", "grep"];
/// Model requests one side question may take.
const MAX_STEPS: usize = 8;
/// The longest question accepted.
const MAX_QUESTION_CHARS: usize = 8_000;
const SYSTEM: &str = "The user is asking a side question while the session continues. Answer it \
directly and briefly from the conversation so far and the project. You can only read the \
project with read, list, glob and grep; you cannot change anything, and nothing in this side \
chat is added to the session.";

/// The answer to a side question.
#[derive(Debug, Clone, PartialEq, Serialize, schemars::JsonSchema)]
pub struct SideAnswer {
    pub answer: String,
    /// The model reference that answered.
    pub model: String,
    /// The read-only tools it called, in order.
    pub tools: Vec<String>,
}

/// What the side chat takes from the Session.
struct Snapshot {
    info: super::SessionInfo,
    system: Vec<String>,
    messages: Vec<Message>,
}

impl Inner {
    async fn side_chat(&self, id: &str, question: &str) -> Result<SideAnswer, RuntimeError> {
        let question = question.trim();
        if question.is_empty() || question.chars().count() > MAX_QUESTION_CHARS {
            return Err(RuntimeError::Invalid(format!(
                "A side question needs 1 to {MAX_QUESTION_CHARS} characters"
            )));
        }
        let handle = self.handle(id).await?;
        let (snapshot, resolved) = {
            let state = handle.state.lock().await;
            let resolved = self
                .resolver
                .resolve(&state.info.model)
                .map_err(RuntimeError::Model)?;
            (snapshot(&state, &resolved), resolved)
        };
        let turn =
            super::drain::turn_context_for_info(&snapshot.info, resolved.prefers_apply_patch());
        let mut defs = self.tools.definitions(&turn);
        defs.retain(|def| READ_ONLY_TOOLS.contains(&def.spec.name.as_str()) && !def.deferred);
        let mut request = resolved.template.clone();
        request.system = snapshot.system.clone();
        request.messages = snapshot.messages.clone();
        self.project_attachments(id, &mut request.messages, &resolved, None);
        request.messages.push(Message::user_text(question));
        request.tools = defs.iter().map(|def| def.spec.clone()).collect();
        request.cache_key = None;
        self.check_budget(&handle).await?;
        let mut tools = Vec::new();
        let mut usage = Usage::default();
        let started = std::time::Instant::now();
        let mut answer = String::new();
        let gated = super::reservations::GatedAdapter::new(self, &handle, &resolved, "side_chat");
        for step in 0..MAX_STEPS {
            request.tools_disabled = step + 1 == MAX_STEPS;
            request.initiator = Some(if step == 0 {
                cyber_llm::Initiator::User
            } else {
                cyber_llm::Initiator::Agent
            });
            let stream = gated.stream(request.clone()).await;
            let out = match stream {
                Ok(stream) => collect(stream, |_| {}).await,
                Err(error) => Err(error),
            };
            gated.flush().await?;
            if let Some(error) = gated.take_error() {
                return Err(error);
            }
            let out = out.map_err(|e| RuntimeError::Model(e.to_string()))?;
            usage.add(&out.usage);
            answer = out.text.trim().to_string();
            if out.tool_calls.is_empty() || request.tools_disabled {
                break;
            }
            let (call_message, results) = self
                .side_tools(&snapshot.info, &defs, &out.text, out.tool_calls, &mut tools)
                .await;
            request.messages.push(call_message);
            request.messages.push(results);
        }
        let record = AuxiliaryUsage {
            provider: resolved.provider.clone(),
            model: snapshot.info.model.clone(),
            purpose: "side_chat".into(),
            call_id: None,
            duration_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
            cost: compute_cost(resolved.cost.as_ref(), &usage),
            usage,
        };
        let mut billed = event(AUXILIARY_USAGE, &record);
        super::reservations::mark_accounted(&mut billed, &gated.request_ids());
        self.commit(&handle, vec![billed]).await?;
        self.observe_budget(&handle).await?;
        if answer.is_empty() {
            return Err(RuntimeError::Model(
                "The model returned an empty answer".into(),
            ));
        }
        Ok(SideAnswer {
            answer,
            model: snapshot.info.model,
            tools,
        })
    }

    /// Run the model's calls of one step with an asker no person can answer, so
    /// whatever would ask is refused; return the assistant message and the results.
    async fn side_tools(
        &self,
        info: &super::SessionInfo,
        defs: &[ToolDef],
        text: &str,
        calls: Vec<cyber_llm::ToolCall>,
        used: &mut Vec<String>,
    ) -> (Message, Message) {
        let mut content = Vec::new();
        if !text.is_empty() {
            content.push(Content::Text { text: text.into() });
        }
        let mut results = Vec::new();
        for call in calls {
            let input = call
                .input
                .clone()
                .or_else(|| serde_json::from_str(&call.arguments).ok())
                .unwrap_or_else(|| serde_json::json!({}));
            content.push(Content::ToolCall {
                id: call.id.clone(),
                name: call.name.clone(),
                input: input.clone(),
                opaque: None,
            });
            let (output, is_error) = match defs.iter().find(|def| def.spec.name == call.name) {
                Some(def) => {
                    used.push(call.name.clone());
                    self.side_call(info, def, &call.id, input).await
                }
                None => (
                    format!(
                        "Unknown tool in a side chat: {} (read-only tools only)",
                        call.name
                    ),
                    true,
                ),
            };
            results.push(Content::ToolResult {
                call_id: call.id,
                output,
                is_error,
            });
        }
        (
            Message {
                role: Role::Assistant,
                content,
            },
            Message {
                role: Role::User,
                content: results,
            },
        )
    }

    async fn side_call(
        &self,
        info: &super::SessionInfo,
        def: &ToolDef,
        call_id: &str,
        input: serde_json::Value,
    ) -> (String, bool) {
        let invocation = Invocation {
            tool_output: Default::default(),
            registration: def.registration.clone(),
            session_id: info.id.clone(),
            directory: info.directory.clone(),
            agent: info.agent.clone(),
            mode: info.mode.clone(),
            message_id: cyber_core::ids::new_id("msg"),
            call_id: call_id.into(),
            name: def.spec.name.clone(),
            input,
            attempt: 1,
            operation_key: format!("side:{call_id}"),
            asker: Asker::detached(),
            rules: info.rules.clone(),
        };
        match self
            .tools
            .execute(invocation, CancellationToken::new())
            .await
        {
            ToolOutcome::Ok(output)
            | ToolOutcome::Structured { output, .. }
            | ToolOutcome::SkillLoaded { output, .. } => (output, false),
            ToolOutcome::SkillSuggestions { output, failed, .. }
            | ToolOutcome::External { output, failed, .. } => (output, failed),
            ToolOutcome::Failed(message) => (message, true),
            ToolOutcome::Aborted => ("The tool was stopped".into(), true),
            ToolOutcome::Crashed(_) => ("The tool failed".into(), true),
        }
    }
}

/// The Session's system prompt and model view, copied under its lock.
fn snapshot(state: &super::model::SessionState, resolved: &super::ResolvedModel) -> Snapshot {
    let mut system = match &state.epoch {
        Some(epoch) => vec![
            epoch
                .system_prefix
                .clone()
                .unwrap_or_else(|| super::context::base_prompt(resolved.family)),
            epoch.baseline.clone(),
        ],
        None => vec![super::context::base_prompt(resolved.family)],
    };
    system.push(SYSTEM.into());
    Snapshot {
        messages: super::view::messages(state, &super::view::Replay::of(resolved)),
        system,
        info: state.info.clone(),
    }
}

impl Runtime {
    /// Answer `question` beside the Session (`/btw`), leaving its history unchanged.
    pub async fn side_chat(
        &self,
        session_id: &str,
        question: &str,
    ) -> Result<SideAnswer, RuntimeError> {
        self.inner.side_chat(session_id, question).await
    }
}
