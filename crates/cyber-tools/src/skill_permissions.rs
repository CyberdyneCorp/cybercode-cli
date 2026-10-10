//! Captured skill declarations share ordinary deny ceilings and scoped approval matching.
use crate::{
    BuiltinHost,
    permissions::{Effect, Rule},
};
use cyber_server::runtime::{Invocation, SessionState};

pub(crate) fn scope_rules(state: &SessionState, message: Option<&str>) -> Vec<Rule> {
    state
        .active_skill_scopes(message)
        .into_iter()
        .flat_map(|scope| {
            let source = format!("skill:{}", scope.name);
            let declarations = scope
                .allowed_tools
                .into_iter()
                .map(|pattern| (pattern, Effect::Allow))
                .chain(
                    scope
                        .disallowed_tools
                        .into_iter()
                        .map(|pattern| (pattern, Effect::Deny)),
                );
            declarations.map(move |(pattern, effect)| {
                let (tool, resource) = pattern.split_once(':').unwrap_or((&pattern, "*"));
                let mut rule = Rule::new("*", resource, effect, &source);
                rule.tool_pattern = Some(tool.into());
                rule
            })
        })
        .collect()
}

pub(crate) fn tool_denial(rules: &[Rule], tool: &str) -> Option<String> {
    rules
        .iter()
        .find(|rule| {
            rule.effect == Effect::Deny
                && rule.resource == "*"
                && rule
                    .tool_pattern
                    .as_deref()
                    .is_some_and(|pattern| cyber_core::wildcard::matches(pattern, tool))
        })
        .map(|_| format!("Skill disallows tool: {tool}"))
}

impl BuiltinHost {
    pub(crate) async fn check_skill_tool(&self, inv: &Invocation) -> Result<(), String> {
        let mut rules = self.skill_scope_rules(inv).await?;
        let page = self
            .opts
            .store
            .read_events(&inv.session_id, -1, 1)
            .map_err(|error| error.to_string())?;
        if !page.events.is_empty() {
            let info = SessionState::replay(&page.events)?.info;
            if info.parent_id.is_some() {
                let runtime = self
                    .runtime()
                    .ok_or("Parent permission resolution requires the runtime")?;
                for parent in runtime
                    .ancestors(&info)
                    .await
                    .map_err(|error| error.to_string())?
                {
                    let state = runtime
                        .state(&parent.id)
                        .await
                        .map_err(|error| error.to_string())?;
                    rules.extend(
                        scope_rules(&state, None)
                            .into_iter()
                            .filter(|rule| rule.effect == Effect::Deny),
                    );
                }
            }
        }
        match tool_denial(&rules, &inv.name) {
            Some(reason) => Err(reason),
            None => Ok(()),
        }
    }

    pub(crate) async fn skill_scope_rules(&self, inv: &Invocation) -> Result<Vec<Rule>, String> {
        if inv.message_id.is_empty() {
            return Ok(Vec::new());
        }
        let events = self
            .opts
            .store
            .read_events(&inv.session_id, -1, 1)
            .map_err(|error| error.to_string())?;
        if events.events.is_empty() {
            return Ok(Vec::new());
        }
        let runtime = self
            .runtime()
            .ok_or("Skill activation resolution requires the runtime")?;
        let state = runtime
            .state(&inv.session_id)
            .await
            .map_err(|error| error.to_string())?;
        Ok(scope_rules(&state, Some(&inv.message_id)))
    }
}
