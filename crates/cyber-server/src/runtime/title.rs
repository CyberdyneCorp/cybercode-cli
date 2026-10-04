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
        let inner = Arc::clone(self);
        tokio::spawn(async move {
            let _ = inner.generate_title(&handle).await;
        });
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
        let stream = resolved
            .adapter
            .stream(request)
            .await
            .map_err(|e| e.to_string())?;
        let out = collect(stream, |_| {}).await.map_err(|e| e.to_string())?;
        let title = clean(&out.text);
        if title.is_empty() {
            return Err("empty title".into());
        }
        let mut state = handle.state.lock().await;
        if !state.info.default_title {
            return Ok(());
        }
        let payload = Titled {
            title,
            usage: Some(out.usage),
            cost: compute_cost(resolved.cost.as_ref(), &out.usage),
        };
        self.commit_locked(&mut state, vec![event(TITLE_GENERATED, &payload)])
            .map_err(|e| e.to_string())?;
        Ok(())
    }
}

/// Strip reasoning tags and quotes, keep the first line, cap at 100 characters.
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
    line.chars().take(100).collect()
}

#[cfg(test)]
mod tests {
    use super::clean;

    #[test]
    fn cleans_model_output() {
        assert_eq!(
            clean("<think>hmm</think>\n\"Fix flaky login test\"\n"),
            "Fix flaky login test"
        );
        assert_eq!(clean(&"x".repeat(150)).len(), 100);
    }
}
