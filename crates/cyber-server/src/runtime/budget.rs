//! Durable soft dispatch boundaries. Reservations remain a separate enforcement mode.
use super::{Handle, Inner, RuntimeError};
use cyber_core::budget::{Budget, Enforcement};
use cyber_store::{EventRegistry, Expected, NewEvent, Store, StoreError, StoredEvent};
use rusqlite::{Transaction, params};
use serde_json::json;

const ACTIVATED: &str = "budget.activated.1";
const WARNED: &str = "budget.warned.1";
const EXCEEDED: &str = "budget.exceeded.1";
pub(super) fn register(registry: &mut EventRegistry) {
    for kind in [ACTIVATED, WARNED, EXCEEDED] {
        registry.register(kind).expect("valid budget event");
    }
}
pub(super) fn validate(budget: Option<&Budget>) -> Result<(), RuntimeError> {
    if let Some(budget) = budget {
        budget.validate().map_err(RuntimeError::Invalid)?;
        if budget.enforcement == Enforcement::Reserved {
            return Err(RuntimeError::Invalid(
                "Reserved Session budget enforcement is not available; no soft fallback is applied"
                    .into(),
            ));
        }
    }
    Ok(())
}
pub(super) fn project(tx: &Transaction<'_>, event: &StoredEvent) -> rusqlite::Result<()> {
    let data = &event.data;
    if event.kind == ACTIVATED {
        tx.execute("UPDATE session_budget SET activated_ms=?2 WHERE session_id=?1 AND activated_ms IS NULL",
            params![data["scope_id"].as_str(),data["activated_ms"].as_i64()])?;
    } else if event.kind == WARNED || event.kind == EXCEEDED {
        let scope = data["scope_id"].as_str().unwrap_or_default();
        tx.execute(&format!("INSERT INTO {}(scope_id,limit_name,level) VALUES (?1,?2,?3) ON CONFLICT DO NOTHING", signal_table(scope)),
            params![scope,data["limit"].as_str(),if event.kind==WARNED {"warning"} else {"exceeded"}])?;
    }
    Ok(())
}

/// Workflow Run scopes (`run_` IDs) keep their signals beside the run, not the Session.
fn signal_table(scope: &str) -> &'static str {
    if scope.starts_with("run_") {
        "workflow_run_budget_signal"
    } else {
        "session_budget_signal"
    }
}

pub(super) struct Scope {
    pub(super) id: String,
    spec: String,
    pub(super) cost: f64,
    pub(super) tokens: u64,
    pub(super) turns: u64,
    activated: Option<i64>,
    complete: bool,
}
fn scopes(tx: &Transaction<'_>, id: &str) -> Result<Vec<Scope>, StoreError> {
    let mut statement = tx.prepare("WITH RECURSIVE scopes(id) AS (
        SELECT id FROM session WHERE id=?1 UNION SELECT s.parent_id FROM scopes a JOIN session s ON s.id=a.id WHERE s.parent_id IS NOT NULL
    ) SELECT s.id,b.spec,s.cost+s.children_cost,
        s.input_tokens+s.output_tokens+s.reasoning_tokens+s.cache_read_tokens+s.cache_write_tokens+s.children_tokens,
        (SELECT COUNT(*) FROM event WHERE aggregate_id=s.id AND type='session.step.ended.1')+
        COALESCE((SELECT SUM(turns) FROM session_children_charge WHERE parent_id=s.id),0),b.activated_ms,s.children_usage_complete
        FROM scopes a JOIN session s ON s.id=a.id JOIN session_budget b ON b.session_id=s.id ORDER BY s.id")?;
    Ok(statement
        .query_map([id], |r| {
            Ok(Scope {
                id: r.get(0)?,
                spec: r.get(1)?,
                cost: r.get(2)?,
                tokens: r.get::<_, i64>(3)? as u64,
                turns: r.get::<_, i64>(4)? as u64,
                activated: r.get(5)?,
                complete: r.get::<_, i64>(6)? != 0,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?)
}

/// The run budget joined through the `workflow_run_id` of `id` or an ancestor
/// (`agents-subagents` → cost attribution to the workflow budget). Its spend is every
/// Session of the run with its descendants; wall time excludes paused segments.
fn run_scopes(tx: &Transaction<'_>, id: &str, now: i64) -> Result<Vec<Scope>, StoreError> {
    let mut statement = tx.prepare("WITH RECURSIVE scopes(id) AS (
        SELECT id FROM session WHERE id=?1 UNION SELECT s.parent_id FROM scopes a JOIN session s ON s.id=a.id WHERE s.parent_id IS NOT NULL
    ) SELECT DISTINCT s.workflow_run_id FROM scopes a JOIN session s ON s.id=a.id WHERE s.workflow_run_id IS NOT NULL ORDER BY 1")?;
    let runs = statement
        .query_map([id], |r| r.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    let mut scopes = Vec::new();
    for run in runs {
        scopes.extend(run_scope(tx, &run, now)?);
    }
    Ok(scopes)
}

pub(super) fn run_scope(
    tx: &rusqlite::Connection,
    run: &str,
    now: i64,
) -> Result<Option<Scope>, StoreError> {
    use rusqlite::OptionalExtension;
    Ok(tx.query_row("SELECT b.run_id,b.spec,
        COALESCE((SELECT SUM(m.cost+m.children_cost) FROM session m WHERE m.workflow_run_id=b.run_id),0),
        COALESCE((SELECT SUM(m.input_tokens+m.output_tokens+m.reasoning_tokens+m.cache_read_tokens+m.cache_write_tokens+m.children_tokens) FROM session m WHERE m.workflow_run_id=b.run_id),0),
        COALESCE((SELECT COUNT(*) FROM event e JOIN session m ON e.aggregate_id=m.id WHERE m.workflow_run_id=b.run_id AND e.type='session.step.ended.1'),0)+
        COALESCE((SELECT SUM(c.turns) FROM session_children_charge c JOIN session m ON c.parent_id=m.id WHERE m.workflow_run_id=b.run_id),0),
        COALESCE(b.activated_ms,?2)-b.active_ms,
        COALESCE((SELECT MIN(m.children_usage_complete) FROM session m WHERE m.workflow_run_id=b.run_id),1)
        FROM workflow_run_budget b WHERE b.run_id=?1", params![run, now], |r| {
        Ok(Scope {
            id: r.get(0)?,
            spec: r.get(1)?,
            cost: r.get(2)?,
            tokens: r.get::<_, i64>(3)? as u64,
            turns: r.get::<_, i64>(4)? as u64,
            activated: r.get(5)?,
            complete: r.get::<_, i64>(6)? != 0,
        })
    }).optional()?)
}

fn signal(
    tx: &Transaction<'_>,
    events: &mut Vec<NewEvent>,
    scope: &Scope,
    limit: &str,
    value: f64,
    cap: f64,
    exceeded: bool,
) -> Result<bool, StoreError> {
    if !exceeded && value < cap * 0.8 {
        return Ok(false);
    }
    let level = if exceeded { "exceeded" } else { "warning" };
    let already: bool = tx.query_row(
        &format!(
            "SELECT EXISTS(SELECT 1 FROM {} WHERE scope_id=?1 AND limit_name=?2 AND level=?3)",
            signal_table(&scope.id)
        ),
        params![scope.id, limit, level],
        |r| r.get(0),
    )?;
    if !already {
        events.push(NewEvent::new(if exceeded {EXCEEDED} else {WARNED}, json!({"scope_id":scope.id,"limit":limit,"spent":value,"maximum":cap,"enforcement":"soft","in_flight_overshoot_possible":true})));
    }
    Ok(exceeded)
}
struct Decision {
    events: Vec<NewEvent>,
    blocked: Option<(String, String)>,
}
fn check(tx: &Transaction<'_>, id: &str, now: i64) -> Result<Decision, StoreError> {
    let mut decision = Decision {
        events: Vec::new(),
        blocked: None,
    };
    let mut all = scopes(tx, id)?;
    all.extend(run_scopes(tx, id, now)?);
    for scope in all {
        if let Some(limit) = check_scope(tx, &mut decision.events, &scope, now)? {
            decision.blocked.get_or_insert((scope.id.clone(), limit));
        }
    }
    Ok(decision)
}

/// The first exceeded limit of one scope, appending its warning and exceeded signals.
pub(super) fn check_scope(
    tx: &Transaction<'_>,
    events: &mut Vec<NewEvent>,
    scope: &Scope,
    now: i64,
) -> Result<Option<String>, StoreError> {
    let budget: Budget = serde_json::from_str(&scope.spec).map_err(|e| StoreError::Projector {
        kind: "budget".into(),
        reason: e.to_string(),
    })?;
    if !scope.complete || budget.enforcement != Enforcement::Soft {
        return Ok(Some("unavailable or incomplete budget accounting".into()));
    }
    if scope.activated.is_none() {
        events.push(NewEvent::new(
            ACTIVATED,
            json!({"scope_id":scope.id,"activated_ms":now}),
        ));
    }
    let elapsed = now.saturating_sub(scope.activated.unwrap_or(now)).max(0) as f64 / 1000.0;
    let mut blocked = None;
    for (name, value, cap, exceeded) in [
        (
            "max_tokens",
            scope.tokens as f64,
            budget.max_tokens.map(|v| v as f64),
            budget.max_tokens.is_some_and(|v| scope.tokens >= v),
        ),
        (
            "max_cost_usd",
            scope.cost,
            budget.max_cost_usd,
            budget.max_cost_usd.is_some_and(|v| scope.cost >= v),
        ),
        (
            "max_turns",
            scope.turns as f64,
            budget.max_turns.map(|v| v as f64),
            budget.max_turns.is_some_and(|v| scope.turns >= v),
        ),
        (
            "max_wall_seconds",
            elapsed,
            budget.max_wall_seconds,
            budget.max_wall_seconds.is_some_and(|v| elapsed >= v),
        ),
    ] {
        if let Some(cap) = cap
            && signal(tx, events, scope, name, value, cap, exceeded)?
        {
            blocked.get_or_insert(name.to_owned());
        }
    }
    Ok(blocked)
}
/// The spend of a Workflow Run budget scope.
#[derive(Debug, Clone, Default)]
pub(super) struct RunSpend {
    pub(super) cost: f64,
    pub(super) tokens: u64,
    pub(super) turns: u64,
    pub(super) elapsed_seconds: f64,
}

impl RunSpend {
    pub(super) fn of(scope: &Scope, now: i64) -> Self {
        Self {
            cost: scope.cost,
            tokens: scope.tokens,
            turns: scope.turns,
            elapsed_seconds: now.saturating_sub(scope.activated.unwrap_or(now)).max(0) as f64
                / 1000.0,
        }
    }
}

/// The current spend of a run, for `ctx.budget`.
pub(super) fn run_spend(store: &Store, run_id: &str) -> Result<RunSpend, StoreError> {
    let run = run_id.to_owned();
    let now = chrono::Utc::now().timestamp_millis();
    store.read(move |db| {
        Ok(run_scope(db, &run, now)?
            .map(|scope| RunSpend::of(&scope, now))
            .unwrap_or_default())
    })
}

/// Check a run before it starts another agent: its soft limits plus `max_agents`
/// (`started` of `max_agents`), recording warning and exceeded signals on the run.
/// Returns the spend and the first exceeded limit.
pub(super) fn check_run(
    store: &Store,
    run_id: &str,
    started: u64,
    max_agents: u64,
) -> Result<(RunSpend, Option<String>), StoreError> {
    let run = run_id.to_owned();
    let now = chrono::Utc::now().timestamp_millis();
    let (_, decision) = store.append_checked(run_id, Expected::Any, move |tx| {
        let Some(scope) = run_scope(tx, &run, now)? else {
            return Ok((Vec::new(), (RunSpend::default(), None)));
        };
        let mut events = Vec::new();
        let mut blocked = None;
        let exhausted = started >= max_agents;
        if signal(
            tx,
            &mut events,
            &scope,
            "max_agents",
            started as f64,
            max_agents as f64,
            exhausted,
        )? {
            blocked = Some("max_agents".to_owned());
        }
        if let Some(limit) = check_scope(tx, &mut events, &scope, now)? {
            blocked.get_or_insert(limit);
        }
        Ok((events, (RunSpend::of(&scope, now), blocked)))
    })?;
    Ok(decision)
}

impl Inner {
    pub(crate) async fn check_budget(&self, handle: &Handle) -> Result<(), RuntimeError> {
        let mut state = handle.state.lock().await;
        if state.info.budget.is_none() && state.info.parent_id.is_none() {
            return Ok(());
        }
        let id = state.info.id.clone();
        let seq = state.last_seq;
        let now = chrono::Utc::now().timestamp_millis();
        let aggregate = id.clone();
        let (stored, blocked) =
            self.store
                .append_checked(&aggregate, Expected::Seq(seq), move |tx| {
                    check(tx, &id, now).map(|decision| (decision.events, decision.blocked))
                })?;
        for event in &stored {
            state.apply(event).map_err(RuntimeError::Corrupt)?;
        }
        self.publish(&stored);
        if let Some((scope, limit)) = blocked {
            return Err(RuntimeError::BudgetExceeded { scope, limit });
        }
        Ok(())
    }
}

impl Inner {
    pub(crate) async fn observe_budget(&self, handle: &Handle) -> Result<(), RuntimeError> {
        match self.check_budget(handle).await {
            Err(RuntimeError::BudgetExceeded { .. }) | Ok(()) => Ok(()),
            Err(error) => Err(error),
        }
    }
}

pub(super) struct GatedAdapter<'a> {
    inner: &'a Inner,
    handle: &'a Handle,
    adapter: &'a dyn cyber_llm::Adapter,
    failure: std::sync::Mutex<Option<RuntimeError>>,
}
impl<'a> GatedAdapter<'a> {
    pub fn new(inner: &'a Inner, handle: &'a Handle, adapter: &'a dyn cyber_llm::Adapter) -> Self {
        Self {
            inner,
            handle,
            adapter,
            failure: std::sync::Mutex::new(None),
        }
    }
    pub fn take_error(&self) -> Option<RuntimeError> {
        self.failure
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
    }
}
impl cyber_llm::Adapter for GatedAdapter<'_> {
    fn stream(
        &self,
        request: cyber_llm::LlmRequest,
    ) -> futures::future::BoxFuture<'_, Result<cyber_llm::EventStream, cyber_llm::LlmError>> {
        Box::pin(async move {
            if let Err(error) = self.inner.check_budget(self.handle).await {
                let message = error.to_string();
                *self
                    .failure
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(error);
                return Err(cyber_llm::LlmError::new(
                    cyber_llm::ErrorKind::InvalidRequest,
                    message,
                ));
            }
            self.adapter.stream(request).await
        })
    }
}

impl super::Asker {
    /// Hidden model calls made by tools obey the same Session and ancestor scopes.
    pub async fn check_budget(&self) -> Result<(), RuntimeError> {
        let inner = self.inner.upgrade().ok_or(RuntimeError::ShuttingDown)?;
        let handle = inner.handle(&self.session_id).await?;
        inner.check_budget(&handle).await
    }

    pub async fn record_model_usage(
        &self,
        usage: super::AuxiliaryUsage,
    ) -> Result<(), RuntimeError> {
        let inner = self.inner.upgrade().ok_or(RuntimeError::ShuttingDown)?;
        let handle = inner.handle(&self.session_id).await?;
        inner
            .commit(
                &handle,
                vec![super::events::event(super::events::AUXILIARY_USAGE, &usage)],
            )
            .await?;
        inner.observe_budget(&handle).await
    }
}
