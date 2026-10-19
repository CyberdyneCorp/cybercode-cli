//! Goal management (`goals` → Set the active goal, Goal queue, Goal management commands):
//! set, add, pause, resume, clear, skip, edit and reorder. Every change is one or more
//! `session.goal.*` events; activating a Goal commits them with the steer prompt that
//! starts work on it, so a Goal is never active without its prompt.

use cyber_core::budget::Budget;
use cyber_core::goals::{GoalSettings, GoalStatus, check_condition, effective_budget};
use cyber_store::NewEvent;

use super::records::{self, Activated, Edited, Ended, Goal, GoalRef, NewGoal, Paused, Reordered};
use crate::runtime::events::event;
use crate::runtime::{Admission, Delivery, PromptCheck, Runtime, RuntimeError, SessionState};

/// A Goal to set or queue.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct GoalRequest {
    pub condition: String,
    pub check: Option<String>,
    pub budget: Option<Budget>,
}

/// Fields of a Goal to change; `check: Some(None)` removes the check.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct GoalEdit {
    pub condition: Option<String>,
    pub check: Option<Option<String>>,
    pub budget: Option<Budget>,
}

pub(super) fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// The prompt that starts work on a Goal.
pub(super) fn goal_prompt(goal: &Goal) -> String {
    let mut text = format!(
        "<goal id=\"{}\">\nWork toward this goal until it is met:\n{}\n",
        goal.goal_id, goal.condition
    );
    if let Some(check) = &goal.check {
        text.push_str(&format!(
            "When you believe it is met, `{check}` must also pass.\n"
        ));
    }
    text.push_str("</goal>");
    text
}

/// The prompt that continues a Goal after `not_met`.
pub(super) fn continuation_prompt(goal: &Goal, reason: &str) -> String {
    format!(
        "<goal_not_met id=\"{}\">\nThe goal is not met yet:\n{}\n\nWhy: {reason}\n\nKeep working toward it.\n</goal_not_met>",
        goal.goal_id, goal.condition
    )
}

impl Runtime {
    /// The Session's Goals.
    pub async fn goals(&self, session_id: &str) -> Result<records::Goals, RuntimeError> {
        let handle = self.inner.handle(session_id).await?;
        Ok(handle.state.lock().await.goals.clone())
    }

    fn goal_settings(
        &self,
        directory: &str,
    ) -> Result<(GoalSettings, Option<Budget>), RuntimeError> {
        self.inner
            .tools
            .goal_settings(directory)
            .map_err(RuntimeError::Invalid)
    }

    /// `/goal <condition>`: clear the current Goal and make this one active, starting a
    /// Turn toward it at once.
    pub async fn set_goal(
        &self,
        session_id: &str,
        request: GoalRequest,
    ) -> Result<Goal, RuntimeError> {
        let (new, mut events) = self.new_goal(session_id, request, true).await?;
        events.insert(0, event(records::SET, &new));
        self.activate(session_id, &new.goal_id, events).await
    }

    /// `/goal add <condition>`: queue it; it activates at once when no Goal is current.
    pub async fn add_goal(
        &self,
        session_id: &str,
        request: GoalRequest,
    ) -> Result<Goal, RuntimeError> {
        let (new, _) = self.new_goal(session_id, request, false).await?;
        let handle = self.inner.handle(session_id).await?;
        let idle = {
            let mut state = handle.state.lock().await;
            self.inner
                .commit_locked(&mut state, vec![event(records::QUEUED, &new)])?;
            state.goals.current.is_none()
        };
        if idle {
            return self.activate(session_id, &new.goal_id, Vec::new()).await;
        }
        self.goal(session_id, &new.goal_id).await
    }

    /// Validate and build a new Goal; `replace` also clears the current one.
    async fn new_goal(
        &self,
        session_id: &str,
        request: GoalRequest,
        replace: bool,
    ) -> Result<(NewGoal, Vec<NewEvent>), RuntimeError> {
        check_condition(&request.condition).map_err(RuntimeError::GoalCondition)?;
        if let Some(budget) = &request.budget {
            budget.validate().map_err(RuntimeError::Invalid)?;
        }
        let handle = self.inner.handle(session_id).await?;
        let state = handle.state.lock().await;
        let (settings, configured) = self.goal_settings(&state.info.directory)?;
        if !replace && state.goals.queue.len() as u64 >= settings.max_queue {
            return Err(RuntimeError::GoalQueueFull(settings.max_queue));
        }
        let clear = if replace {
            state
                .goals
                .current
                .iter()
                .map(|id| ended(id, GoalStatus::Cleared, "replaced by a new goal"))
                .collect()
        } else {
            Vec::new()
        };
        let new = NewGoal {
            goal_id: cyber_core::ids::new_id("gol"),
            condition: request.condition.trim().to_owned(),
            check: request.check.filter(|c| !c.trim().is_empty()),
            budget: effective_budget(request.budget, configured),
            created_ms: now_ms(),
        };
        Ok((new, clear))
    }

    /// Commit `before` and the activation of `goal_id` together with the steer prompt
    /// that starts work on it.
    pub(super) async fn activate(
        &self,
        session_id: &str,
        goal_id: &str,
        before: Vec<NewEvent>,
    ) -> Result<Goal, RuntimeError> {
        let handle = self.inner.handle(session_id).await?;
        let (cost_before, tokens_before, prompt) = {
            let state = handle.state.lock().await;
            let (cost, tokens) = self.spend(session_id, &state)?;
            let goal = pending_or_new(&state, goal_id, &before)?;
            (cost, tokens, goal_prompt(&goal))
        };
        let activated = Activated {
            goal_id: goal_id.to_owned(),
            cost_before,
            tokens_before,
            activated_ms: now_ms(),
        };
        let mut events = before;
        events.push(event(records::ACTIVATED, &activated));
        self.admit_goal_prompt(session_id, prompt, events).await?;
        self.goal(session_id, goal_id).await
    }

    /// A steer prompt with `source: "goal"`, committed with `events`.
    pub(super) async fn admit_goal_prompt(
        &self,
        session_id: &str,
        text: String,
        events: Vec<NewEvent>,
    ) -> Result<(), RuntimeError> {
        let admission = Admission {
            source: "goal".into(),
            ..Admission::text(text, Delivery::Steer)
        };
        self.admit_attempt_with(
            session_id,
            admission,
            None,
            PromptCheck::Checked(None),
            events,
        )
        .await
        .map(drop)
    }

    /// Work tokens and cost of the Session so far, children included: what a Goal's
    /// Budget counts from its activation.
    pub(super) fn spend(
        &self,
        session_id: &str,
        state: &SessionState,
    ) -> Result<(f64, u64), RuntimeError> {
        let children = self.children_usage(session_id)?;
        let usage = &state.totals.usage;
        let tokens = usage.input + usage.output + usage.reasoning;
        Ok((
            state.totals.cost + children.children_cost,
            tokens + children.children_tokens,
        ))
    }

    async fn goal(&self, session_id: &str, goal_id: &str) -> Result<Goal, RuntimeError> {
        let handle = self.inner.handle(session_id).await?;
        let state = handle.state.lock().await;
        state
            .goals
            .goals
            .get(goal_id)
            .cloned()
            .ok_or_else(|| RuntimeError::GoalNotFound(goal_id.to_owned()))
    }

    /// `/goal pause`: stop evaluation and continuation; the Goal keeps its history.
    pub async fn pause_goal(&self, session_id: &str) -> Result<Goal, RuntimeError> {
        let id = self.current_goal(session_id, GoalStatus::Active).await?;
        let paused = Paused {
            goal_id: id.clone(),
            reason: "paused by the user".into(),
        };
        self.commit_goal(session_id, vec![event(records::PAUSED, &paused)])
            .await?;
        self.goal(session_id, &id).await
    }

    /// `/goal resume`: continue a paused Goal with a fresh prompt toward it.
    pub async fn resume_goal(&self, session_id: &str) -> Result<Goal, RuntimeError> {
        let id = self.current_goal(session_id, GoalStatus::Paused).await?;
        let goal = self.goal(session_id, &id).await?;
        let resumed = GoalRef {
            goal_id: id.clone(),
        };
        let prompt = goal_prompt(&goal);
        self.admit_goal_prompt(session_id, prompt, vec![event(records::RESUMED, &resumed)])
            .await?;
        self.goal(session_id, &id).await
    }

    /// `/goal skip` (`next`): clear the current Goal and activate the next queued one.
    pub async fn skip_goal(&self, session_id: &str) -> Result<Option<Goal>, RuntimeError> {
        let handle = self.inner.handle(session_id).await?;
        let (clear, next) = {
            let state = handle.state.lock().await;
            let current = state
                .goals
                .current
                .clone()
                .ok_or_else(|| RuntimeError::GoalNotFound("active".into()))?;
            (
                ended(&current, GoalStatus::Cleared, "skipped by the user"),
                state.goals.queue.first().cloned(),
            )
        };
        match next {
            Some(next) => self
                .activate(session_id, &next, vec![clear])
                .await
                .map(Some),
            None => self
                .commit_goal(session_id, vec![clear])
                .await
                .map(|()| None),
        }
    }

    /// `/goal clear [id|all]`: end one Goal, or every current and queued one.
    pub async fn clear_goals(
        &self,
        session_id: &str,
        goal_id: Option<&str>,
    ) -> Result<Vec<String>, RuntimeError> {
        let handle = self.inner.handle(session_id).await?;
        let mut state = handle.state.lock().await;
        let targets: Vec<String> = match goal_id {
            Some(id) => {
                let live = state
                    .goals
                    .goals
                    .get(id)
                    .is_some_and(|g| !g.status.is_terminal());
                if !live {
                    return Err(RuntimeError::GoalNotFound(id.to_owned()));
                }
                vec![id.to_owned()]
            }
            None => state
                .goals
                .current
                .iter()
                .chain(state.goals.queue.iter())
                .cloned()
                .collect(),
        };
        let events: Vec<_> = targets
            .iter()
            .map(|id| ended(id, GoalStatus::Cleared, "cleared by the user"))
            .collect();
        if !events.is_empty() {
            self.inner.commit_locked(&mut state, events)?;
        }
        Ok(targets)
    }

    /// Change a live Goal's condition, check or budget.
    pub async fn edit_goal(
        &self,
        session_id: &str,
        goal_id: &str,
        edit: GoalEdit,
    ) -> Result<Goal, RuntimeError> {
        if let Some(condition) = &edit.condition {
            check_condition(condition).map_err(RuntimeError::GoalCondition)?;
        }
        if let Some(budget) = &edit.budget {
            budget.validate().map_err(RuntimeError::Invalid)?;
        }
        {
            let handle = self.inner.handle(session_id).await?;
            let mut state = handle.state.lock().await;
            let live = state
                .goals
                .goals
                .get(goal_id)
                .is_some_and(|g| !g.status.is_terminal());
            if !live {
                return Err(RuntimeError::GoalNotFound(goal_id.to_owned()));
            }
            let edited = Edited {
                goal_id: goal_id.to_owned(),
                condition: edit.condition.map(|c| c.trim().to_owned()),
                check: edit.check,
                budget: edit.budget.map(|b| effective_budget(Some(b), None)),
            };
            self.inner
                .commit_locked(&mut state, vec![event(records::EDITED, &edited)])?;
        }
        self.goal(session_id, goal_id).await
    }

    /// Reorder the pending queue; `goal_ids` must name exactly the pending Goals.
    pub async fn reorder_goals(
        &self,
        session_id: &str,
        goal_ids: Vec<String>,
    ) -> Result<Vec<String>, RuntimeError> {
        let handle = self.inner.handle(session_id).await?;
        let mut state = handle.state.lock().await;
        let mut given = goal_ids.clone();
        let mut pending = state.goals.queue.clone();
        given.sort();
        pending.sort();
        if given != pending {
            return Err(RuntimeError::Invalid(
                "goal_ids must list every pending goal exactly once".into(),
            ));
        }
        let reordered = Reordered {
            goal_ids: goal_ids.clone(),
        };
        self.inner
            .commit_locked(&mut state, vec![event(records::REORDERED, &reordered)])?;
        Ok(goal_ids)
    }

    async fn current_goal(
        &self,
        session_id: &str,
        status: GoalStatus,
    ) -> Result<String, RuntimeError> {
        let handle = self.inner.handle(session_id).await?;
        let state = handle.state.lock().await;
        state
            .goals
            .current()
            .filter(|g| g.status == status)
            .map(|g| g.goal_id.clone())
            .ok_or_else(|| RuntimeError::GoalNotFound(status.as_str().into()))
    }

    pub(super) async fn commit_goal(
        &self,
        session_id: &str,
        events: Vec<NewEvent>,
    ) -> Result<(), RuntimeError> {
        let handle = self.inner.handle(session_id).await?;
        let mut state = handle.state.lock().await;
        self.inner.commit_locked(&mut state, events).map(drop)
    }
}

pub(super) fn ended(goal_id: &str, status: GoalStatus, reason: &str) -> NewEvent {
    let ended = Ended {
        goal_id: goal_id.to_owned(),
        status,
        reason: reason.to_owned(),
    };
    event(records::ENDED, &ended)
}

/// The Goal about to activate: pending in the state, or set by an event in `before`.
fn pending_or_new(
    state: &SessionState,
    goal_id: &str,
    before: &[NewEvent],
) -> Result<Goal, RuntimeError> {
    if let Some(goal) = state.goals.goals.get(goal_id) {
        return Ok(goal.clone());
    }
    let new = before
        .iter()
        .find(|e| e.kind == records::SET && e.data["goal_id"] == goal_id)
        .ok_or_else(|| RuntimeError::GoalNotFound(goal_id.to_owned()))?;
    let mut scratch = records::Goals::default();
    let stored = cyber_store::StoredEvent {
        id: String::new(),
        aggregate_id: state.info.id.clone(),
        seq: 0,
        kind: records::SET.into(),
        data: new.data.clone(),
        time_ms: 0,
        causation_id: None,
    };
    records::fold(&mut scratch, &stored).map_err(RuntimeError::Corrupt)?;
    Ok(scratch.goals[goal_id].clone())
}
