//! Running Goals (`goals` → Evaluation after each turn, Continuation on not met,
//! Completion, Impossible and blocking errors, Verification check command, Goal budgets,
//! Deferred evaluation during background work, Goals survive resume and restart).
//!
//! The Drain calls [`Runtime::goal_after_turn`] at its idle exit, after Stop hooks: the
//! active Goal is evaluated on the Turn that just ended and the Drain continues when a
//! continuation or the next Goal's prompt was admitted. Those prompts are committed as
//! raw `steer` rows (as Stop-hook continuations are), never through admission, so the
//! Drain never waits on the lifecycle guard a shutdown holds.

use std::sync::Arc;

use cyber_core::budget::Budget;
use cyber_core::goals::{GoalSettings, GoalStatus, OnImpossible, Verdict};
use cyber_llm::Content;
use cyber_store::NewEvent;
use serde_json::json;
use tokio_util::sync::CancellationToken;

use super::evaluate::{self, Judgement};
use super::records::{self, Activated, CheckRun, Evaluated, Goal, GoalRef, Paused};
use super::service::{continuation_prompt, ended, goal_prompt, now_ms};
use crate::runtime::deliveries::Notification;
use crate::runtime::events::{ADMITTED, AUXILIARY_USAGE, Admitted, AuxiliaryUsage, event};
use crate::runtime::{Delivery, Handle, JobStatus, Runtime, RuntimeError};

/// Lines of a failed check kept for the continuation prompt.
const CHECK_TAIL_LINES: usize = 50;

/// What the locked first step found.
struct Due {
    goal: Goal,
    session_id: String,
    settings: GoalSettings,
    evidence: String,
    spent_cost: f64,
    spent_tokens: u64,
    /// The Session's model, the evaluator of last resort.
    model: String,
}

impl Runtime {
    /// The Drain's idle exit: evaluate the active Goal on the Turn that just ended.
    /// `true` when a prompt was admitted and the Drain should continue.
    pub(crate) async fn goal_after_turn(
        &self,
        handle: &Arc<Handle>,
        cancel: &CancellationToken,
    ) -> Result<bool, RuntimeError> {
        let Some(due) = self.goal_due(handle).await? else {
            return Ok(false);
        };
        let turn = due.goal.turns + 1;
        if let Some(reason) = spend_exceeded(&due.goal.budget, &due, now_ms()) {
            self.goal_stopped(handle, &due.goal, GoalStatus::BudgetExceeded, &reason)
                .await?;
            return Ok(false);
        }
        let judgement = evaluate::judge(
            &self.inner,
            handle,
            &due.model,
            due.evidence.clone(),
            cancel,
        )
        .await?;
        self.bill_evaluator(handle, &judgement).await?;
        let (verdict, reason, check) = self.apply_check(handle, &due, &judgement, cancel).await;
        let evaluated = Evaluated {
            goal_id: due.goal.goal_id.clone(),
            verdict,
            reason: evaluate::clip(&reason),
            check: check.clone(),
            turn,
            eval_tokens: judgement.tokens,
            eval_cost_usd: judgement.cost,
            tokens: due.spent_tokens,
            cost_usd: due.spent_cost,
            evaluated_ms: now_ms(),
        };
        let continued = self
            .act(handle, &due, evaluated.clone(), &reason, check.as_ref())
            .await?;
        self.goal_hook(
            handle,
            "GoalEvaluated",
            &due.goal.goal_id,
            json!({
            "goal": self.goal_snapshot(handle, &due.goal.goal_id).await,
            "verdict": evaluated.verdict, "reason": evaluated.reason}),
        )
        .await;
        Ok(continued)
    }

    /// The active Goal with its evidence, or `None` when there is nothing to evaluate:
    /// no active Goal, or background work the Session started still runs (evaluation
    /// waits for a later Turn, after its handback).
    async fn goal_due(&self, handle: &Arc<Handle>) -> Result<Option<Due>, RuntimeError> {
        let (goal, session_id, directory, model) = {
            let state = handle.state.lock().await;
            let Some(goal) = state.goals.active().cloned() else {
                return Ok(None);
            };
            let ids = (state.info.id.clone(), state.info.directory.clone());
            (goal, ids.0, ids.1, state.info.model.clone())
        };
        if self.background_running(&session_id)? {
            handle.state.lock().await.goals.pending_eval = true;
            return Ok(None);
        }
        let (settings, _) = self
            .inner
            .tools
            .goal_settings(&directory)
            .map_err(RuntimeError::Invalid)?;
        let state = handle.state.lock().await;
        let (cost, tokens) = self.spend(&session_id, &state)?;
        let evidence = evaluate::evidence(&state, &goal, settings.evaluator_max_input_tokens);
        Ok(Some(Due {
            spent_cost: (cost - goal.cost_before).max(0.0),
            spent_tokens: tokens.saturating_sub(goal.tokens_before),
            goal,
            session_id,
            settings,
            evidence,
            model,
        }))
    }

    /// Jobs, background subagents and workflow runs the Session started that still run
    /// or owe their handback.
    fn background_running(&self, session_id: &str) -> Result<bool, RuntimeError> {
        let jobs = self.jobs(Some(session_id))?;
        if jobs
            .iter()
            .any(|job| job.status == JobStatus::Running || !job.notified)
        {
            return Ok(true);
        }
        let runs = self.workflow_runs(Some(session_id))?;
        Ok(runs.iter().any(|run| run.status.is_active()))
    }

    /// Record the evaluator's tokens as a hidden `goal_evaluator` call: counted once in
    /// the Session's totals, the usage ledger and stats.
    async fn bill_evaluator(
        &self,
        handle: &Arc<Handle>,
        judgement: &Judgement,
    ) -> Result<(), RuntimeError> {
        if judgement.tokens == 0 || !judgement.reservation_requests.is_empty() {
            return Ok(());
        }
        let usage = AuxiliaryUsage {
            provider: String::new(),
            model: String::new(),
            purpose: "goal_evaluator".into(),
            call_id: None,
            duration_ms: 0,
            usage: cyber_llm::Usage {
                input: judgement.tokens,
                ..cyber_llm::Usage::default()
            },
            cost: Some(judgement.cost),
        };
        self.inner
            .commit(handle, vec![event(AUXILIARY_USAGE, &usage)])
            .await
            .map(drop)
    }

    /// A `met` verdict holds only when the Goal's `check` passes; a failing or broken
    /// check turns it into `not_met` with its output as the reason. The check is the
    /// user's own command: it runs sandboxed in the Location without a permission ask,
    /// as `!` and loop `--until` do, so `cyber exec --goal-check` works unattended.
    async fn apply_check(
        &self,
        handle: &Arc<Handle>,
        due: &Due,
        judgement: &Judgement,
        cancel: &CancellationToken,
    ) -> (Verdict, String, Option<CheckRun>) {
        let (verdict, reason) = (judgement.verdict, judgement.reason.clone());
        let (Verdict::Met, Some(command)) = (verdict, &due.goal.check) else {
            return (verdict, reason, None);
        };
        let directory = handle.state.lock().await.info.directory.clone();
        let timeout = due.settings.check_timeout_seconds.saturating_mul(1000);
        let ran = self
            .inner
            .tools
            .check(
                &directory,
                &due.session_id,
                command,
                timeout,
                cancel.child_token(),
            )
            .await;
        match ran {
            Ok(outcome) => {
                let check = CheckRun {
                    command: command.clone(),
                    exit_code: outcome.exit_code,
                    timed_out: outcome.timed_out,
                    tail: last_lines(&outcome.tail, CHECK_TAIL_LINES),
                };
                if outcome.exit_code == Some(0) {
                    return (Verdict::Met, reason, Some(check));
                }
                let why = if outcome.timed_out {
                    format!(
                        "check `{command}` timed out after {}s",
                        due.settings.check_timeout_seconds
                    )
                } else {
                    format!(
                        "check `{command}` failed (exit {})",
                        exit_text(outcome.exit_code)
                    )
                };
                (Verdict::NotMet, why, Some(check))
            }
            Err(error) => (
                Verdict::NotMet,
                format!("check could not run: {error}"),
                None,
            ),
        }
    }

    /// Commit the verdict and what follows from it; `true` when a prompt was admitted.
    async fn act(
        &self,
        handle: &Arc<Handle>,
        due: &Due,
        evaluated: Evaluated,
        reason: &str,
        check: Option<&CheckRun>,
    ) -> Result<bool, RuntimeError> {
        let goal = &due.goal;
        let mut events = vec![event(records::EVALUATED, &evaluated)];
        match evaluated.verdict {
            Verdict::NotMet if evaluated.turn >= goal.budget.max_turns.unwrap_or(u64::MAX) => {
                let why = format!(
                    "max_turns {}/{}",
                    evaluated.turn,
                    goal.budget.max_turns.unwrap_or(0)
                );
                events.push(ended(&goal.goal_id, GoalStatus::BudgetExceeded, &why));
                self.inner.commit(handle, events).await?;
                self.goal_notify(
                    &due.session_id,
                    goal,
                    "Goal stopped: budget exceeded",
                    &spend_text(&why, &evaluated),
                )
                .await;
                Ok(false)
            }
            Verdict::NotMet => {
                let mut why = reason.to_owned();
                if let Some(check) = check.filter(|c| !c.tail.is_empty()) {
                    why.push_str(&format!("\n\nCheck output (last lines):\n{}", check.tail));
                }
                events.push(steer(goal, continuation_prompt(goal, &why)));
                self.inner.commit(handle, events).await?;
                Ok(true)
            }
            Verdict::Met => {
                events.push(event(
                    records::COMPLETED,
                    &GoalRef {
                        goal_id: goal.goal_id.clone(),
                    },
                ));
                let next = self.next_goal(handle, &mut events).await?;
                self.inner.commit(handle, events).await?;
                self.goal_notify(&due.session_id, goal, "Goal met", &evaluated.reason)
                    .await;
                self.goal_hook(
                    handle,
                    "GoalCompleted",
                    &goal.goal_id,
                    json!({
                    "goal": self.goal_snapshot(handle, &goal.goal_id).await}),
                )
                .await;
                Ok(next)
            }
            Verdict::Impossible => {
                events.push(ended(
                    &goal.goal_id,
                    GoalStatus::Impossible,
                    &evaluated.reason,
                ));
                let next = due.settings.on_impossible == OnImpossible::Continue
                    && self.next_goal(handle, &mut events).await?;
                self.inner.commit(handle, events).await?;
                self.goal_notify(&due.session_id, goal, "Goal impossible", &evaluated.reason)
                    .await;
                Ok(next)
            }
        }
    }

    /// Append the activation of the next queued Goal (and its prompt) to `events`;
    /// whether there was one. Totals are read now, so its Budget counts from here.
    async fn next_goal(
        &self,
        handle: &Arc<Handle>,
        events: &mut Vec<NewEvent>,
    ) -> Result<bool, RuntimeError> {
        let state = handle.state.lock().await;
        let Some(next) = state
            .goals
            .queue
            .first()
            .and_then(|id| state.goals.goals.get(id))
            .cloned()
        else {
            return Ok(false);
        };
        let (cost, tokens) = self.spend(&state.info.id, &state)?;
        events.push(activated(&next.goal_id, cost, tokens));
        events.push(steer(&next, goal_prompt(&next)));
        Ok(true)
    }

    /// End `goal` with `status` (budget or a stop before evaluation) and notify.
    async fn goal_stopped(
        &self,
        handle: &Arc<Handle>,
        goal: &Goal,
        status: GoalStatus,
        reason: &str,
    ) -> Result<(), RuntimeError> {
        self.inner
            .commit(handle, vec![ended(&goal.goal_id, status, reason)])
            .await?;
        let session_id = handle.state.lock().await.info.id.clone();
        self.goal_notify(&session_id, goal, "Goal stopped: budget exceeded", reason)
            .await;
        Ok(())
    }

    /// The active Goal `goal_id` would pass its wall deadline waiting for a usage limit
    /// to reset (`goals` → Usage limit waits under a Goal): it ends `budget_exceeded`.
    pub(crate) async fn goal_wall_deadline(
        &self,
        handle: &Arc<Handle>,
        goal_id: &str,
        reason: &str,
    ) -> Result<(), RuntimeError> {
        let goal = handle.state.lock().await.goals.active().cloned();
        match goal.filter(|goal| goal.goal_id == goal_id) {
            Some(goal) => {
                self.goal_stopped(handle, &goal, GoalStatus::BudgetExceeded, reason)
                    .await
            }
            None => Ok(()),
        }
    }

    /// A Turn under an active Goal failed or was stopped (`goals` → Impossible and
    /// blocking errors): the Goal pauses with the reason, so nothing continues on its own.
    pub(crate) async fn goal_turn_ended_early(&self, handle: &Arc<Handle>, reason: &str) {
        let (goal, session_id) = {
            let state = handle.state.lock().await;
            match state.goals.active() {
                Some(goal) => (goal.clone(), state.info.id.clone()),
                None => return,
            }
        };
        let paused = Paused {
            goal_id: goal.goal_id.clone(),
            reason: evaluate::clip(reason),
        };
        if self
            .inner
            .commit(handle, vec![event(records::PAUSED, &paused)])
            .await
            .is_ok()
        {
            self.goal_notify(&session_id, &goal, "Goal paused", reason)
                .await;
        }
    }

    pub(super) async fn goal_notify(&self, session_id: &str, goal: &Goal, title: &str, body: &str) {
        let notification = Notification {
            call_id: goal.goal_id.clone(),
            title: title.into(),
            body: format!("{}: {body}", short(&goal.condition)),
        };
        // A notice past the per-Session rate limit is dropped; the events still say it.
        let _ = self.record_notification(session_id, notification).await;
    }

    async fn goal_hook(
        &self,
        handle: &Arc<Handle>,
        kind: &str,
        goal_id: &str,
        fields: serde_json::Value,
    ) {
        let hook = self
            .inner
            .lifecycle_hook(handle, kind, goal_id.to_owned(), fields)
            .await;
        self.inner.observe_lifecycle(hook).await;
    }

    async fn goal_snapshot(&self, handle: &Arc<Handle>, goal_id: &str) -> serde_json::Value {
        let state = handle.state.lock().await;
        state
            .goals
            .goals
            .get(goal_id)
            .map(|g| {
                json!({"goal_id": g.goal_id, "condition": g.condition, "check": g.check,
                "status": g.status, "turns": g.turns, "tokens": g.tokens, "cost_usd": g.cost_usd,
                "eval_count": g.eval_count, "eval_cost_usd": g.eval_cost_usd, "reason": g.reason})
            })
            .unwrap_or_default()
    }

    /// Restore after a restart (`goals` → Goals survive resume and restart): every
    /// active Goal of a Session that is not running is evaluated once on its last Turn,
    /// and continues only on `not_met`. Runs in the background; must start after the
    /// other recoveries.
    pub fn spawn_goal_recovery(&self) {
        let weak = self.downgrade();
        let closed = self.inner.closed.clone();
        let task = tokio::spawn(async move {
            let Some(runtime) = weak.upgrade() else {
                return;
            };
            tokio::select! {
                () = closed.cancelled() => {}
                result = runtime.recover_goals() => {
                    if let Err(error) = result {
                        cyber_core::log::warn("goals", "goal recovery failed",
                            json!({"error": error.to_string()}));
                    }
                }
            }
        });
        let mut owners = self
            .inner
            .background
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        owners.retain(|owner| !owner.is_finished());
        owners.push(task);
    }

    /// [`Self::spawn_goal_recovery`]'s work; the number of Goals evaluated.
    pub async fn recover_goals(&self) -> Result<usize, RuntimeError> {
        let sessions = self
            .inner
            .store
            .read(|db| Ok(records::sessions_with_goals(db)?))?;
        let mut evaluated = 0;
        for session_id in sessions {
            if self.is_running(&session_id) {
                continue;
            }
            let handle = self.inner.handle(&session_id).await?;
            if handle.state.lock().await.goals.active().is_none() {
                continue;
            }
            evaluated += 1;
            let cancel = self.inner.closed.child_token();
            if self.goal_after_turn(&handle, &cancel).await? {
                self.wake(&session_id).await?;
            }
        }
        Ok(evaluated)
    }
}

fn activated(goal_id: &str, cost: f64, tokens: u64) -> NewEvent {
    let activated = Activated {
        goal_id: goal_id.to_owned(),
        cost_before: cost,
        tokens_before: tokens,
        activated_ms: now_ms(),
    };
    event(records::ACTIVATED, &activated)
}

/// A `steer` row with `source: "goal"`, committed directly (no admission, no wake).
fn steer(goal: &Goal, text: String) -> NewEvent {
    let message_id = cyber_core::ids::new_id("msg");
    let admitted = Admitted {
        observation_scope: None,
        skill_command: None,
        admission_bindings: None,
        wake: false,
        digest: format!("goal:{}:{message_id}", goal.goal_id),
        message_id,
        parts: vec![Content::Text { text }],
        delivery: Delivery::Steer,
        source: "goal".into(),
        hook_context: None,
        hook_stop: None,
        resources: Vec::new(),
        untrusted: None,
    };
    event(ADMITTED, &admitted)
}

/// Why the Goal's tokens, cost or wall time are already spent.
fn spend_exceeded(budget: &Budget, due: &Due, now: i64) -> Option<String> {
    if let Some(max) = budget.max_cost_usd
        && due.spent_cost >= max
    {
        return Some(format!("max_cost_usd {:.2}/{max:.2}", due.spent_cost));
    }
    if let Some(max) = budget.max_tokens
        && due.spent_tokens >= max
    {
        return Some(format!("max_tokens {}/{max}", due.spent_tokens));
    }
    let wall = budget.max_wall_seconds?;
    let started = due.goal.activated_ms.unwrap_or(due.goal.created_ms);
    let elapsed = (now - started) as f64 / 1000.0;
    (elapsed >= wall).then(|| format!("max_wall_seconds {elapsed:.0}/{wall:.0}"))
}

fn spend_text(limit: &str, e: &Evaluated) -> String {
    format!(
        "{limit}; spent {} tokens and ${:.2} over {} turns",
        e.tokens, e.cost_usd, e.turn
    )
}

fn last_lines(text: &str, n: usize) -> String {
    let lines: Vec<&str> = text.lines().collect();
    lines[lines.len().saturating_sub(n)..].join("\n")
}

fn exit_text(code: Option<i32>) -> String {
    code.map_or("by a signal".into(), |c| c.to_string())
}

fn short(text: &str) -> String {
    let first = text.lines().next().unwrap_or_default();
    if first.chars().count() <= 80 {
        return first.to_owned();
    }
    let mut cut: String = first.chars().take(79).collect();
    cut.push('…');
    cut
}
