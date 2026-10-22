//! Title generation (`session-runtime` → Title generation).

use std::sync::Arc;

use cyber_llm::catalog::{ModelRole, compute_cost};
use cyber_llm::{Message, collect};

use super::events::*;
use super::model::{Entry, text_of};
use super::{Handle, Inner};

const SYSTEM: &str = "Write a short title (at most 8 words) for this coding conversation. Reply with the title only.";

impl Inner {
    /// Generate a title from the first prompt in the background. Failure keeps the default.
    pub(crate) fn spawn_title(self: &Arc<Self>, handle: Arc<Handle>) {
        let mut tasks = self
            .background
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if self.closed.is_cancelled() {
            return;
        }
        tasks.retain(|task| !task.is_finished());
        let inner = Arc::clone(self);
        tasks.push(tokio::spawn(async move {
            tokio::select! {
                biased;
                _ = inner.closed.cancelled() => {},
                _ = inner.generate_title(&handle) => {},
            }
        }));
    }

    async fn generate_title(&self, handle: &Handle) -> Result<(), String> {
        let (prompt, model) = {
            let state = handle.state.lock().await;
            let prompt = state.entries.iter().find_map(|e| match e {
                Entry::User { parts, .. } => Some(text_of(parts)),
                _ => None,
            });
            (prompt.ok_or("no prompt")?, state.info.model.clone())
        };
        let model = self.resolver.role(ModelRole::Title).unwrap_or(model);
        let resolved = self.resolver.resolve(&model)?;
        let mut request = resolved.template.clone();
        request.system = vec![SYSTEM.into()];
        request.messages = vec![Message::user_text(prompt)];
        request.tools = Vec::new();
        request.reasoning = None;
        request.max_output_tokens = Some(80);
        // The title belongs to the Turn the first prompt opened; it is not a new request.
        request.initiator = Some(cyber_llm::Initiator::Agent);
        self.check_budget(handle).await.map_err(|e| e.to_string())?;
        let started = std::time::Instant::now();
        let stream = resolved
            .adapter
            .stream(request)
            .await
            .map_err(|e| e.to_string())?;
        let out = collect(stream, |_| {}).await.map_err(|e| e.to_string())?;
        let title = clean(&out.text);
        let cost = compute_cost(resolved.cost.as_ref(), &out.usage);
        let mut state = handle.state.lock().await;
        let billing = if title.is_empty() || !state.info.default_title {
            event(
                AUXILIARY_USAGE,
                &AuxiliaryUsage {
                    provider: resolved.provider.clone(),
                    model,
                    purpose: "discarded_title".into(),
                    call_id: None,
                    duration_ms: started.elapsed().as_millis().min(u64::MAX as u128) as u64,
                    usage: out.usage,
                    cost,
                },
            )
        } else {
            event(
                TITLE_GENERATED,
                &Titled {
                    title,
                    usage: Some(out.usage),
                    cost,
                    model: Some(model),
                },
            )
        };
        self.commit_locked(&mut state, vec![billing])
            .map_err(|e| e.to_string())?;
        drop(state);
        self.observe_budget(handle)
            .await
            .map_err(|e| e.to_string())?;
        Ok(())
    }
}

/// Strip reasoning tags and quotes, keep the first line without control characters (a
/// title reaches terminal window titles and stdout), cap at 100 characters.
pub(crate) fn clean(raw: &str) -> String {
    let mut text = raw.to_string();
    while let (Some(start), Some(end)) = (text.find("<think>"), text.find("</think>")) {
        if end < start {
            break;
        }
        text.replace_range(start..end + "</think>".len(), "");
    }
    let line = text
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or_default();
    let line = line
        .trim_matches(|c| c == '"' || c == '\'' || c == '#' || c == '*')
        .trim();
    line.chars()
        .map(|c| if c == '\t' { ' ' } else { c })
        .filter(|c| !c.is_control())
        .take(100)
        .collect()
}

/// A title the user typed: whitespace runs (including newlines) become one space,
/// control characters are dropped, and the result is capped at 100 characters. `None`
/// when nothing printable is left.
pub(crate) fn user_title(raw: &str) -> Option<String> {
    let words: Vec<String> = raw
        .split(|c: char| c.is_whitespace())
        .map(|w| w.chars().filter(|c| !c.is_control()).collect::<String>())
        .filter(|w| !w.is_empty())
        .collect();
    let title: String = words.join(" ").chars().take(100).collect();
    let title = title.trim_end().to_string();
    (!title.is_empty()).then_some(title)
}

#[cfg(test)]
mod tests {
    use super::{clean, user_title};

    #[test]
    fn user_titles_are_single_line_printable_and_bounded() {
        assert_eq!(
            user_title("  Auth\nrefactor\t\x1b]0;x\x07 ").as_deref(),
            Some("Auth refactor ]0;x")
        );
        assert_eq!(user_title(&"y".repeat(150)).map(|t| t.len()), Some(100));
        assert_eq!(user_title(" \n\t\x07 "), None);
    }

    #[test]
    fn cleans_model_output() {
        assert_eq!(
            clean("<think>hmm</think>\n\"Fix flaky login test\"\n"),
            "Fix flaky login test"
        );
        assert_eq!(clean(&"x".repeat(150)).len(), 100);
    }

    #[test]
    fn drops_control_characters_a_terminal_would_act_on() {
        assert_eq!(
            clean("Evil\x07\x1b]52;c;SGFja2Vk\x07\x1b[2J\u{9b}2J\ttitle"),
            "Evil]52;c;SGFja2Vk[2J2J title"
        );
    }
}
