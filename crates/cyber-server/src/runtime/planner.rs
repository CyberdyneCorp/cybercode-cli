//! Planner and executor models (`provider-catalog` → Planner and executor models): a
//! top-level Session in `plan` mode runs its Turns on `model_roles.plan`, and every other
//! Turn on the Session's own model. The switch both ways is the ordinary automatic model
//! switch at the Safe Boundary.

use cyber_llm::catalog::{ModelRef, ModelRole};

use super::events::Switched;
use super::model::SessionState;
use super::selection::ModelSelection;
use super::{Inner, ResolvedModel, Runtime, RuntimeError};

/// `model_role` of a step run on the planner, and of the Session view while it is active.
pub const PLAN_ROLE: &str = "plan";

/// Fold a switch into the planner suspension: an explicit model switch while the Session
/// is in `plan` suspends the planner until the Mode changes again.
pub(crate) fn model_switched(state: &mut SessionState, switched: &Switched) {
    if !switched.automatic && state.info.mode == "plan" {
        state.plan_suspended = true;
    }
}

/// Any Mode change starts a new phase, so the planner applies again on the next entry.
pub(crate) fn mode_switched(state: &mut SessionState) {
    state.plan_suspended = false;
}

/// Whether two references name the same provider and model, whatever their variants.
fn same_model(reference: &str, provider: &str, model: &str) -> bool {
    ModelRef::parse(reference).is_ok_and(|r| r.provider == provider && r.model == model)
}

impl Inner {
    /// The planner for the Session's next Turn: set only for a top-level Session in
    /// `plan` whose plan phase no explicit model switch has overridden.
    pub(crate) fn plan_model(&self, state: &SessionState) -> Option<String> {
        let applies =
            state.info.mode == "plan" && state.info.parent_id.is_none() && !state.plan_suspended;
        applies
            .then(|| self.resolver.role(ModelRole::Plan))
            .flatten()
    }

    /// The selection a Turn resolves, after a command's own model: the active fallback,
    /// else the planner, else the Session's own selection.
    pub(crate) fn turn_selection(&self, state: &SessionState, now_ms: i64) -> ModelSelection {
        if state.active_fallback(now_ms).is_some() {
            return state.effective_selection(now_ms);
        }
        self.plan_model(state)
            .map_or_else(|| state.model_selection.clone(), ModelSelection::explicit)
    }

    /// `model_role` of a step about to run on `resolved`.
    pub(crate) fn step_role(
        &self,
        state: &SessionState,
        resolved: &ResolvedModel,
    ) -> Option<String> {
        self.plan_model(state)
            .filter(|plan| same_model(plan, &resolved.provider, &resolved.model))
            .map(|_| PLAN_ROLE.to_owned())
    }
}

impl Runtime {
    /// `"plan"` while the planner is the Session's active model, for the Session view.
    pub fn model_role(&self, state: &SessionState) -> Option<&'static str> {
        let plan = self.inner.plan_model(state)?;
        let active = ModelRef::parse(&state.info.model).ok()?;
        same_model(&plan, &active.provider, &active.model).then_some(PLAN_ROLE)
    }

    /// Whether the planner is the Session's active model; `plan_exit` then hands the
    /// approved plan to the executor in its result.
    pub async fn planner_active(&self, session_id: &str) -> Result<bool, RuntimeError> {
        let handle = self.inner.handle(session_id).await?;
        let state = handle.state.lock().await;
        Ok(self.model_role(&state).is_some())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn variants_do_not_change_the_model() {
        assert!(same_model("p/m#high", "p", "m"));
        assert!(same_model("p/m", "p", "m"));
        assert!(!same_model("p/other", "p", "m"));
        assert!(!same_model("not a ref", "p", "m"));
    }
}
