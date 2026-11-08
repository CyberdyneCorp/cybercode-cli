//! The model fallback chain (`provider-catalog` → Model fallback chain). A Turn that fails
//! before any output with an overload, quota, transport or authentication error, after
//! the retry policy, is re-sent on the next available model of `fallback_models`. The
//! durable `session.model.fallback.1` records it; the Session uses the fallback until
//! `fallback.sticky_minutes` pass or the user switches model.

use cyber_core::model_fallback::FallbackSettings;
use cyber_llm::{ErrorKind, LlmError};
use cyber_store::{EventRegistry, StoredEvent};
use serde::{Deserialize, Serialize};

use super::events::Switched;
use super::model::SessionState;
use super::selection::ModelSelection;
use super::{Handle, Inner, Runtime};

pub const FALLBACK: &str = "session.model.fallback.1";

/// `session.model.fallback.1`, and the active fallback of the Session view.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ModelFallback {
    /// The model whose request failed.
    pub from: String,
    /// The model the request is re-sent on.
    pub to: String,
    /// The error kind, such as `provider_internal` or `quota_exceeded`.
    pub kind: String,
    pub reason: String,
    pub started_ms: i64,
    /// When the Session returns to its configured model.
    pub until_ms: i64,
}

/// Kinds that move to the next model once the retry policy is done with them.
fn eligible(kind: ErrorKind) -> bool {
    matches!(
        kind,
        ErrorKind::RateLimit
            | ErrorKind::QuotaExceeded
            | ErrorKind::ProviderInternal
            | ErrorKind::Transport
            | ErrorKind::Authentication
    )
}

pub(super) fn register(registry: &mut EventRegistry) {
    registry
        .register_with(FALLBACK, |v| {
            serde_json::from_value::<ModelFallback>(v.clone())
                .map(drop)
                .map_err(|e| e.to_string())
        })
        .expect("valid fallback event");
}

/// Fold `session.model.fallback.1` into the Session state.
pub(crate) fn fold(state: &mut Option<ModelFallback>, e: &StoredEvent) -> Result<(), String> {
    *state = Some(serde_json::from_value(e.data.clone()).map_err(|e| e.to_string())?);
    Ok(())
}

/// A model switch ends the fallback: the user's own switch, or the automatic switch away
/// from it when it expires.
pub(crate) fn switched(state: &mut Option<ModelFallback>, switch: &Switched) {
    if state
        .as_ref()
        .is_some_and(|active| !switch.automatic || switch.to != active.to)
    {
        *state = None;
    }
}

impl SessionState {
    /// The fallback in effect at `now_ms`.
    pub(crate) fn active_fallback(&self, now_ms: i64) -> Option<&ModelFallback> {
        self.fallback.as_ref().filter(|f| f.until_ms > now_ms)
    }

    /// The selection a Turn resolves: the active fallback, else the Session's own.
    pub(crate) fn effective_selection(&self, now_ms: i64) -> ModelSelection {
        self.active_fallback(now_ms).map_or_else(
            || self.model_selection.clone(),
            |f| ModelSelection::explicit(f.to.clone()),
        )
    }
}

impl Inner {
    pub(crate) fn now_ms(&self) -> i64 {
        self.wait_clock().now_ms()
    }

    /// The next model of the chain after the one that failed, when the error moves on and
    /// one resolves. Models already passed in the chain, and the configured one, are skipped.
    pub(crate) async fn next_fallback(
        &self,
        handle: &Handle,
        error: &LlmError,
    ) -> Option<ModelFallback> {
        if !eligible(error.kind) {
            return None;
        }
        let state = handle.state.lock().await;
        let settings = self.tools.fallback(&state.info.directory);
        let failed = state.info.model.as_str();
        let configured = state.model_selection.reference.as_str();
        let start = chain_start(&settings, failed);
        let to = settings.models[start..].iter().find(|candidate| {
            candidate.as_str() != failed
                && candidate.as_str() != configured
                && self
                    .resolve_selection(&state.info, &ModelSelection::explicit((*candidate).clone()))
                    .is_ok()
        })?;
        let now = self.now_ms();
        let minutes = i64::try_from(settings.sticky_minutes).unwrap_or(i64::MAX);
        Some(ModelFallback {
            from: failed.to_owned(),
            to: to.clone(),
            kind: super::drain::kind_name(error.kind).to_owned(),
            reason: error.message.clone(),
            started_ms: now,
            until_ms: now.saturating_add(minutes.saturating_mul(60_000)),
        })
    }
}

/// Where the search starts: after the failed model when it is in the chain, else at the top.
fn chain_start(settings: &FallbackSettings, failed: &str) -> usize {
    settings
        .models
        .iter()
        .position(|model| model == failed)
        .map_or(0, |index| index + 1)
}

impl Runtime {
    /// The Session's fallback in effect now, for the Session view.
    pub fn model_fallback(&self, state: &SessionState) -> Option<ModelFallback> {
        state.active_fallback(self.inner.now_ms()).cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fallback(to: &str) -> Option<ModelFallback> {
        Some(ModelFallback {
            from: "a/primary".into(),
            to: to.into(),
            kind: "provider_internal".into(),
            reason: "overloaded".into(),
            started_ms: 0,
            until_ms: 600_000,
        })
    }

    fn switch(to: &str, automatic: bool) -> Switched {
        Switched {
            automatic,
            from: "a/primary".into(),
            to: to.into(),
        }
    }

    #[test]
    fn only_a_user_switch_or_the_switch_away_ends_a_fallback() {
        let mut state = fallback("b/backup");
        switched(&mut state, &switch("b/backup", true));
        assert!(
            state.is_some(),
            "the automatic switch onto the fallback keeps it"
        );
        switched(&mut state, &switch("a/primary", true));
        assert!(state.is_none(), "the switch back when it expires ends it");
        let mut state = fallback("b/backup");
        switched(&mut state, &switch("b/backup", false));
        assert!(state.is_none(), "a user switch ends it");
    }

    #[test]
    fn the_chain_continues_after_the_failed_model() {
        let settings = FallbackSettings {
            models: vec!["b/one".into(), "c/two".into()],
            sticky_minutes: 10,
        };
        assert_eq!(chain_start(&settings, "a/primary"), 0);
        assert_eq!(chain_start(&settings, "b/one"), 1);
        assert_eq!(chain_start(&settings, "c/two"), 2);
    }

    #[test]
    fn eligible_kinds() {
        assert!(eligible(ErrorKind::ProviderInternal) && eligible(ErrorKind::Authentication));
        assert!(!eligible(ErrorKind::ContextOverflow) && !eligible(ErrorKind::InvalidRequest));
        assert!(!eligible(ErrorKind::ContentPolicy));
    }
}
