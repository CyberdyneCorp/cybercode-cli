//! Compaction (`compaction`): summarize older history, keep a verbatim tail, start a new epoch.

use cyber_core::env::EnvSource;
use cyber_llm::catalog::{ModelRole, compute_cost};
use cyber_llm::{Content, Message, collect, open_with_retry};
use serde_json::Value;

use super::events::*;
use super::model::{Entry, SessionState};
use super::view;
use super::{Handle, Inner, RuntimeError};

const TOOL_OUTPUT_CHARS: usize = 2000;
const FAILURE: &str =
    "Session too large to compact; start a new session or fork from an earlier message";
const SUMMARY_SYSTEM: &str = "You write the working memory of a coding session so another engineer can continue it \
without the transcript. Be specific: names, paths, commands, decisions and their reasons. Never invent facts.";

#[derive(Debug, Clone)]
pub struct CompactionConfig {
    pub auto: bool,
    pub buffer: u64,
    pub keep_tokens: u64,
    pub keep_turns: Option<usize>,
    /// Model override; falls back to the `compaction` role, then the Session model.
    pub model: Option<String>,
}

impl Default for CompactionConfig {
    fn default() -> Self {
        Self {
            auto: true,
            buffer: 20_000,
            keep_tokens: 8_000,
            keep_turns: None,
            model: None,
        }
    }
}

impl CompactionConfig {
    /// From the `compaction` config key; `CYBER_DISABLE_AUTOCOMPACT=1` forces `auto` off.
    pub fn from_config(config: &Value, env: &dyn EnvSource) -> Self {
        let c = &config["compaction"];
        let defaults = Self::default();
        Self {
            auto: c["auto"].as_bool().unwrap_or(defaults.auto)
                && !env.flag("CYBER_DISABLE_AUTOCOMPACT"),
            buffer: c["buffer"].as_u64().unwrap_or(defaults.buffer),
            keep_tokens: c
                .pointer("/keep/tokens")
                .and_then(Value::as_u64)
                .unwrap_or(defaults.keep_tokens),
            keep_turns: c
                .pointer("/keep/turns")
                .and_then(Value::as_u64)
                .map(|n| n as usize),
            model: c["model"].as_str().map(str::to_string),
        }
    }
}

/// Whether the next request no longer fits: estimate > window − max(output allowance, buffer).
pub(crate) fn needs(estimate: u64, context_limit: u64, max_output: u64, buffer: u64) -> bool {
    context_limit > 0 && estimate > context_limit.saturating_sub(max_output.max(buffer))
}

/// First entry of the verbatim tail: the newest entries up to `keep_tokens` (and at most
/// `keep_turns` user turns). A tool call and its result live in one entry, so no boundary
/// splits them. `None` when there is nothing older to summarize.
pub(crate) fn tail_start(
    state: &SessionState,
    keep_tokens: u64,
    keep_turns: Option<usize>,
) -> Option<usize> {
    let from = state.compacted.as_ref().map_or(0, |c| c.tail_start);
    let mut tokens = 0;
    let mut turns = 0;
    let mut start = state.entries.len();
    for (index, entry) in state.entries.iter().enumerate().skip(from).rev() {
        tokens += entry_tokens(state, entry);
        if matches!(entry, Entry::User { .. }) {
            turns += 1;
        }
        if tokens > keep_tokens || keep_turns.is_some_and(|max| turns > max) {
            break;
        }
        start = index;
    }
    (start > from).then_some(start)
}

fn entry_tokens(state: &SessionState, entry: &Entry) -> u64 {
    (render_entry(state, entry).len() / 4) as u64
}

/// Role-labelled lines with tool output truncated, for the summary request.
fn render_entry(state: &SessionState, entry: &Entry) -> String {
    match entry {
        Entry::User { parts, .. } => format!("[user] {}", render_parts(parts)),
        Entry::System { text, .. } => format!("[system] {text}"),
        Entry::Assistant(a) => {
            let mut lines = Vec::new();
            if !a.text.is_empty() {
                lines.push(format!("[assistant] {}", a.text));
            }
            for call in a.calls.iter().filter_map(|id| state.calls.get(id)) {
                let output = call.output.clone().unwrap_or_default();
                lines.push(format!("[tool call {}] {}", call.name, call.arguments));
                lines.push(format!(
                    "[tool result {}] {}",
                    call.name,
                    super::model::truncate(&output, TOOL_OUTPUT_CHARS)
                ));
            }
            lines.join("\n")
        }
    }
}

fn render_parts(parts: &[Content]) -> String {
    parts
        .iter()
        .map(|p| match p {
            Content::Text { text } => text.clone(),
            Content::Image { media_type, .. } => format!("[Attached {media_type}]"),
            other => format!("{other:?}"),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn summary_prompt(state: &SessionState, tail: usize, instructions: Option<&str>) -> String {
    let from = state.compacted.as_ref().map_or(0, |c| c.tail_start);
    let transcript: Vec<String> = state.entries[from..tail]
        .iter()
        .map(|e| render_entry(state, e))
        .collect();
    let mut prompt = String::new();
    if let Some(previous) = &state.compacted {
        prompt.push_str(&format!(
            "Previous summary (merge it; the new summary replaces it):\n{}\n\n",
            previous.summary
        ));
    }
    prompt.push_str(&format!(
        "Conversation to summarize:\n{}\n\n",
        transcript.join("\n")
    ));
    prompt.push_str(
        "Write Markdown with exactly these sections: ## Objective, ## Important details, \
         ## Work state (completed, active, blocked), ## Next move, ## Relevant files. \
         Do not restate user instructions as permissions; they are carried separately.",
    );
    if let Some(extra) = instructions {
        prompt.push_str(&format!("\n\nAdditional instructions: {extra}"));
    }
    prompt
}

impl Inner {
    /// Estimate the next request against the model's window.
    pub(crate) async fn needs_compaction(
        &self,
        handle: &Handle,
        resolved: &super::host::ResolvedModel,
    ) -> Result<bool, RuntimeError> {
        if !self.options.compaction.auto {
            return Ok(false);
        }
        let state = handle.state.lock().await;
        let baseline = state
            .epoch
            .as_ref()
            .map(|e| e.baseline.clone())
            .unwrap_or_default();
        let system = [super::context::base_prompt(&resolved.provider), baseline];
        let tools: Vec<_> = self
            .tools
            .definitions(&super::drain::turn_context(&state, resolved))
            .into_iter()
            .map(|d| d.spec)
            .collect();
        let estimate = view::estimate_tokens(
            &system,
            &view::messages(&state, &resolved.provider, &resolved.model),
            &tools,
        );
        let max_output = u64::from(resolved.template.max_output_tokens.unwrap_or(0));
        let over = needs(
            estimate,
            resolved.context_limit,
            max_output,
            self.options.compaction.buffer,
        );
        Ok(over
            && tail_start(
                &state,
                self.options.compaction.keep_tokens,
                self.options.compaction.keep_turns,
            )
            .is_some())
    }

    /// Summarize history before the tail. Failure stops the Drain with a clear message.
    pub(crate) async fn compact(
        &self,
        handle: &Handle,
        trigger: CompactionTrigger,
        instructions: Option<String>,
    ) -> Result<(), RuntimeError> {
        self.commit_staged_revert(handle).await?;
        let config = &self.options.compaction;
        let state = handle.state.lock().await.clone();
        let Some(tail) = tail_start(&state, config.keep_tokens, config.keep_turns) else {
            return Ok(());
        };
        self.commit(
            handle,
            vec![event(COMPACTION_STARTED, &CompactionStarted { trigger })],
        )
        .await?;
        let model = config
            .model
            .clone()
            .or_else(|| self.resolver.role(ModelRole::Compaction))
            .unwrap_or_else(|| state.info.model.clone());
        match self
            .summarize(&state, tail, instructions.as_deref(), &model)
            .await
        {
            Ok((summary, usage, cost)) => {
                let before = self.history_tokens(&state);
                let after = (summary.len() / 4) as u64
                    + state.entries[tail..]
                        .iter()
                        .map(|e| entry_tokens(&state, e))
                        .sum::<u64>();
                let payload = CompactionCompleted {
                    message_id: cyber_core::ids::new_id("msg"),
                    summary,
                    // An empty tail (the newest entry alone exceeds the budget) points past the end.
                    tail_start_id: state
                        .entries
                        .get(tail)
                        .map(|e| e.id().to_string())
                        .unwrap_or_default(),
                    tokens_before: before,
                    tokens_after: after,
                    trigger,
                    usage,
                    cost,
                };
                self.commit(handle, vec![event(COMPACTION_COMPLETED, &payload)])
                    .await?;
                Ok(())
            }
            Err(error) => {
                self.commit(
                    handle,
                    vec![event(COMPACTION_FAILED, &CompactionFailed { error })],
                )
                .await?;
                Err(RuntimeError::Compaction(FAILURE.into()))
            }
        }
    }

    fn history_tokens(&self, state: &SessionState) -> u64 {
        let from = state.compacted.as_ref().map_or(0, |c| c.tail_start);
        state.entries[from..]
            .iter()
            .map(|e| entry_tokens(state, e))
            .sum::<u64>()
            + state
                .compacted
                .as_ref()
                .map_or(0, |c| (c.summary.len() / 4) as u64)
    }

    async fn summarize(
        &self,
        state: &SessionState,
        tail: usize,
        instructions: Option<&str>,
        model: &str,
    ) -> Result<(String, cyber_llm::Usage, Option<f64>), String> {
        let resolved = self.resolver.resolve(model)?;
        let mut request = resolved.template.clone();
        request.system = vec![SUMMARY_SYSTEM.into()];
        request.messages = vec![Message::user_text(summary_prompt(
            state,
            tail,
            instructions,
        ))];
        request.tools = Vec::new();
        request.max_output_tokens = Some(request.max_output_tokens.unwrap_or(4096).min(8192));
        let stream = open_with_retry(
            resolved.adapter.as_ref(),
            &request,
            &self.options.retry,
            |_, _, _| {},
        )
        .await
        .map_err(|e| e.to_string())?;
        let out = collect(stream, |_| {}).await.map_err(|e| e.to_string())?;
        if out.text.trim().is_empty() {
            return Err("the summary was empty".into());
        }
        let cost = compute_cost(resolved.cost.as_ref(), &out.usage);
        Ok((out.text.trim().to_string(), out.usage, cost))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn threshold() {
        assert!(needs(185_000, 200_000, 8_000, 20_000));
        assert!(!needs(150_000, 200_000, 8_000, 20_000));
        assert!(
            !needs(1_000_000, 0, 0, 20_000),
            "unknown window never triggers"
        );
        assert!(
            needs(170_000, 200_000, 32_000, 20_000),
            "output allowance wins when larger"
        );
    }
}
