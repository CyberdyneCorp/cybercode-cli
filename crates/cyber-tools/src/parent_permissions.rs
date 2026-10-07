//! Existing durable children inherit every ancestor's deny rules at dispatch.

use crate::{
    BuiltinHost,
    permissions::{Effect, Mode, Rule},
};
use cyber_server::runtime::SessionState;
use std::path::Path;

#[derive(Default)]
pub(crate) struct ParentPermissions {
    pub rules: Vec<Rule>,
    pub modes: Vec<Mode>,
}

impl BuiltinHost {
    pub(crate) async fn inherited_permissions(
        &self,
        session_id: &str,
    ) -> Result<ParentPermissions, String> {
        let page = self
            .opts
            .store
            .read_events(session_id, -1, 1)
            .map_err(|error| error.to_string())?;
        if page.events.is_empty() {
            // Standalone tool invocations and precreation worktree operations have no Session yet.
            return Ok(ParentPermissions::default());
        }
        let info = SessionState::replay(&page.events)?.info;
        if info.parent_id.is_none() {
            return Ok(ParentPermissions::default());
        }
        let runtime = self
            .runtime()
            .ok_or("Parent permission resolution requires the runtime")?;
        let ancestors = runtime
            .ancestor_authorities(&info)
            .await
            .map_err(|error| error.to_string())?;
        let mut inherited = ParentPermissions::default();
        for authority in ancestors {
            let parent = authority.info;
            let mode = Mode::checked_parse(&authority.effective_mode).ok_or_else(|| {
                format!(
                    "Unknown parent Mode {:?} in Session {}",
                    authority.effective_mode, parent.id
                )
            })?;
            inherited.modes.push(mode);
            let rules = self.session_rules(
                Path::new(&parent.directory),
                Some(&authority.effective_agent),
                &parent.rules,
            )?;
            inherited.rules.extend(
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
