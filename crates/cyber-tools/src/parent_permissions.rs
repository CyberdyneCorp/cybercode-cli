//! Existing durable children inherit every ancestor's deny rules at dispatch.

use crate::{
    BuiltinHost,
    permissions::{Effect, Rule},
};
use cyber_server::runtime::SessionState;
use std::path::Path;

impl BuiltinHost {
    pub(crate) async fn inherited_denies(&self, session_id: &str) -> Result<Vec<Rule>, String> {
        let page = self
            .opts
            .store
            .read_events(session_id, -1, 1)
            .map_err(|error| error.to_string())?;
        if page.events.is_empty() {
            // Standalone tool invocations and precreation worktree operations have no Session yet.
            return Ok(Vec::new());
        }
        let info = SessionState::replay(&page.events)?.info;
        if info.parent_id.is_none() {
            return Ok(Vec::new());
        }
        let runtime = self
            .runtime()
            .ok_or("Parent permission resolution requires the runtime")?;
        let ancestors = runtime
            .ancestors(&info)
            .await
            .map_err(|error| error.to_string())?;
        let mut inherited = Vec::new();
        for parent in ancestors {
            let rules = self.session_rules(
                Path::new(&parent.directory),
                Some(&parent.agent),
                &parent.rules,
            )?;
            inherited.extend(
                rules
                    .into_iter()
                    .filter(|rule| rule.effect == Effect::Deny)
                    .map(|mut rule| {
                        rule.source = format!("parent:{}:{}", parent.id, rule.source);
                        rule
                    }),
            );
        }
        Ok(inherited)
    }
}
