//! Output style selection of a Session (`system-context` → Output style selection,
//! `session-runtime` → Agent, model, and mode switching).
//!
//! A switch is durable and changes nothing else: the Drain observes the new
//! `core/output-style` source at the next Turn's Safe Boundary and starts a new Context
//! Epoch, so a running Turn keeps the prefix it started with.

use super::events::{OUTPUT_STYLE_SWITCHED, StyleSwitched, event};
use super::{Runtime, RuntimeError, SessionInfo, SessionState, context};

/// The `core/output-style` context source key.
pub(crate) const CONTEXT_KEY: &str = "core/output-style";
const CHILD: &str = "output styles apply to primary Sessions";

impl SessionInfo {
    /// Only primary Sessions have an output style: never a Session with a parent
    /// (subagents, workflow agents) or a teammate, whose output goes to another agent.
    pub fn takes_output_style(&self) -> bool {
        self.parent_id.is_none()
            && self
                .team
                .as_ref()
                .is_none_or(|tag| tag.role == super::teams::Role::Lead)
    }
}

/// Refuse an explicit selection a Session cannot take: one that has no output style,
/// or a name that is not a style name (checked before any lookup).
pub(crate) fn check(info: &SessionInfo, style: Option<&str>) -> Result<(), RuntimeError> {
    if style.is_some() && !info.takes_output_style() {
        return Err(RuntimeError::Invalid(CHILD.into()));
    }
    match style {
        Some(name) if !cyber_core::output_styles::valid_name(name) => Err(RuntimeError::Invalid(
            format!("invalid output style name \"{name}\""),
        )),
        _ => Ok(()),
    }
}

impl Runtime {
    /// Record the Session's own output style; `None` returns to the configured one.
    /// Switching to the current selection is a no-op. The caller checks that the name
    /// is defined at the Session's Location.
    pub async fn switch_output_style(
        &self,
        session_id: &str,
        style: Option<&str>,
    ) -> Result<(), RuntimeError> {
        let handle = self.inner.handle(session_id).await?;
        let mut state = handle.state.lock().await;
        if !state.info.takes_output_style() {
            return Err(RuntimeError::Invalid(CHILD.into()));
        }
        check(&state.info, style)?;
        let to = style.map(str::to_owned);
        if state.info.output_style == to {
            return Ok(());
        }
        let payload = StyleSwitched {
            from: state.info.output_style.clone(),
            to,
        };
        self.inner
            .commit_locked(&mut state, vec![event(OUTPUT_STYLE_SWITCHED, &payload)])?;
        Ok(())
    }
}

/// The Session's `core/output-style` observation; Sessions with a parent have none.
pub(crate) fn observe(tools: &dyn super::ToolHost, state: &SessionState) -> context::Observed {
    if !state.info.takes_output_style() {
        return context::Observed::Absent;
    }
    tools.output_style(&state.info.directory, state.info.output_style.as_deref())
}

/// Whether the observed style differs from the epoch's; an unavailable one keeps it.
pub(crate) fn changed(
    snapshot: &std::collections::BTreeMap<String, String>,
    observed: Option<&context::Observed>,
) -> bool {
    match observed {
        Some(context::Observed::Value(text)) => snapshot.get(CONTEXT_KEY) != Some(text),
        Some(context::Observed::Absent) | None => snapshot.contains_key(CONTEXT_KEY),
        Some(context::Observed::Unavailable(_)) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn only_a_different_available_style_is_a_change() {
        let empty = BTreeMap::new();
        let styled = BTreeMap::from([(CONTEXT_KEY.to_owned(), "Output style: concise".to_owned())]);
        let value = |text: &str| context::Observed::Value(text.into());
        assert!(!changed(&empty, None));
        assert!(!changed(&empty, Some(&context::Observed::Absent)));
        assert!(changed(&empty, Some(&value("Output style: concise"))));
        assert!(!changed(&styled, Some(&value("Output style: concise"))));
        assert!(changed(&styled, Some(&value("Output style: learning"))));
        assert!(changed(&styled, Some(&context::Observed::Absent)));
        let unreadable = context::Observed::Unavailable("io".into());
        assert!(!changed(&styled, Some(&unreadable)));
        assert!(!changed(&empty, Some(&unreadable)));
    }

    #[test]
    fn explicit_selections_are_checked_before_any_lookup() {
        let primary = SessionInfo {
            parent_id: None,
            ..child()
        };
        assert!(check(&primary, Some("concise")).is_ok());
        assert!(check(&primary, None).is_ok());
        assert!(check(&child(), None).is_ok());
        let invalid = check(&primary, Some("../../etc/passwd"))
            .unwrap_err()
            .to_string();
        assert!(invalid.contains("invalid output style name"), "{invalid}");
        let refused = check(&child(), Some("concise")).unwrap_err().to_string();
        assert!(refused.contains(CHILD), "{refused}");
        let teammate = |role| SessionInfo {
            parent_id: None,
            team: Some(super::super::teams::TeamTag {
                team_id: "team_a".into(),
                role,
                name: "lead".into(),
            }),
            ..child()
        };
        assert!(teammate(super::super::teams::Role::Lead).takes_output_style());
        assert!(!teammate(super::super::teams::Role::Teammate).takes_output_style());
    }

    fn child() -> SessionInfo {
        serde_json::from_value(serde_json::json!({
            "id": "ses_child", "title": "t", "directory": "/repo", "parent_id": "ses_parent",
            "agent": "build", "model": "test/main", "mode": "default", "created_ms": 0,
        }))
        .unwrap()
    }
}
