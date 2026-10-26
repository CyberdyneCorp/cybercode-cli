//! Resolve scope ceilings and immutable child bindings in the admission transaction.

mod schedules;

use std::collections::{BTreeMap, BTreeSet};

use cyber_core::budget::{Budget, Enforcement};
use cyber_store::{StoreError, StoredEvent, reservations::Scope};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Snapshot {
    id: String,
    kind: String,
    owner: String,
    budget: Budget,
    evidence: Value,
}

#[derive(Default)]
struct Spend {
    tokens: u64,
    turns: u64,
    cost: f64,
    incomplete: bool,
    unpriced: bool,
}

fn invalid(scope: &str, message: impl ToString) -> StoreError {
    StoreError::Projector {
        kind: "budget.reservation.scope_invalid".into(),
        reason: json!({"scope":scope,"message":message.to_string()}).to_string(),
    }
}

fn budget(value: &str, scope: &str) -> Result<Budget, StoreError> {
    let budget: Budget = serde_json::from_str(value).map_err(|e| invalid(scope, e))?;
    budget.validate().map_err(|e| invalid(scope, e))?;
    Ok(budget)
}

fn money(value: f64, scope: &str) -> Result<u64, StoreError> {
    let micros = (value * 1_000_000.0).ceil();
    if !value.is_finite() || value < 0.0 || micros > i64::MAX as f64 {
        return Err(invalid(scope, "cost cannot be represented conservatively"));
    }
    Ok(micros as u64)
}

fn unsigned(value: i64, scope: &str) -> Result<u64, StoreError> {
    u64::try_from(value).map_err(|_| invalid(scope, "negative recorded token/turn total"))
}

fn capped<T: PartialOrd + Copy>(left: Option<T>, right: Option<T>) -> Option<T> {
    match (left, right) {
        (Some(a), Some(b)) => Some(if a < b { a } else { b }),
        (a, b) => a.or(b),
    }
}

fn tighten(captured: &Budget, current: &Budget) -> Budget {
    Budget {
        max_tokens: capped(captured.max_tokens, current.max_tokens),
        max_turns: capped(captured.max_turns, current.max_turns),
        max_cost_usd: capped(captured.max_cost_usd, current.max_cost_usd),
        max_wall_seconds: capped(captured.max_wall_seconds, current.max_wall_seconds),
        enforcement: Enforcement::Reserved,
    }
}

fn ancestors(tx: &Connection, id: &str) -> Result<Vec<String>, StoreError> {
    let mut statement = tx.prepare(
        "WITH RECURSIVE up(id) AS (SELECT id FROM session WHERE id=?1
        UNION SELECT s.parent_id FROM session s JOIN up ON s.id=up.id WHERE s.parent_id IS NOT NULL)
        SELECT id FROM up ORDER BY id",
    )?;
    let ids = statement
        .query_map([id], |r| r.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    if ids.is_empty() || ids.len() > 256 {
        return Err(invalid(id, "Session ancestry is missing or too deep"));
    }
    Ok(ids)
}

fn push(snapshot: Snapshot, all: &mut BTreeMap<String, Snapshot>) {
    if snapshot.budget.enforcement != Enforcement::Reserved {
        return;
    }
    if let Some(previous) = all.get_mut(&snapshot.id) {
        previous.budget = tighten(&previous.budget, &snapshot.budget);
    } else {
        all.insert(snapshot.id.clone(), snapshot);
    }
}

fn event_rows(
    tx: &Connection,
    owner: &str,
    field: &str,
    id: &str,
) -> Result<Vec<(String, Value, i64)>, StoreError> {
    // `field` is selected only by this module, never supplied by a caller or prompt.
    let mut query=tx.prepare(&format!("SELECT type,data,time FROM event WHERE aggregate_id=?1 AND json_extract(data,'$.{field}')=?2 ORDER BY seq"))?;
    let raw = query
        .query_map(params![owner, id], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, i64>(2)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    raw.into_iter()
        .map(|(kind, data, time)| {
            Ok((
                kind,
                serde_json::from_str(&data).map_err(|e| invalid(id, e))?,
                time,
            ))
        })
        .collect()
}

fn logical_snapshot(
    tx: &Connection,
    kind: &str,
    owner: &str,
    id: &str,
) -> Result<Option<Snapshot>, StoreError> {
    let field = if kind == "goal" { "goal_id" } else { "loop_id" };
    let rows = event_rows(tx, owner, field, id)?;
    let mut spec = None;
    let mut evidence = json!({"version":0});
    let mut seen = false;
    for (event, data, time) in rows {
        let created = matches!(
            event.as_str(),
            "session.goal.set.1" | "session.goal.queued.1" | "loop.created.1"
        );
        if created {
            seen = true;
            evidence["version"] = data
                .get("reservation_scope_version")
                .cloned()
                .unwrap_or(json!(0));
            if kind == "loop" {
                evidence["activation_ms"] = time.into();
            }
        }
        if (created || event == "session.goal.edited.1") && data["budget"].is_object() {
            spec = Some(budget(&data["budget"].to_string(), id)?);
        }
        if event == "session.goal.activated.1" {
            evidence["activation_ms"] = data.get("activated_ms").cloned().unwrap_or(json!(time));
        }
    }
    if !seen {
        return Err(invalid(id, "logical scope creation evidence is missing"));
    }
    Ok(spec.map(|budget| Snapshot {
        id: id.into(),
        kind: kind.into(),
        owner: owner.into(),
        budget,
        evidence,
    }))
}

fn discovered(tx: &Connection, session: &str) -> Result<BTreeMap<String, Snapshot>, StoreError> {
    discovered_with_schedule(tx, session, true)
}

fn discovered_with_schedule(
    tx: &Connection,
    session: &str,
    include_schedule: bool,
) -> Result<BTreeMap<String, Snapshot>, StoreError> {
    let mut all = BTreeMap::new();
    if include_schedule && let Some(schedule) = schedules::current(tx, session)? {
        push(schedule, &mut all);
    }
    let mut runs = BTreeSet::new();
    let mut teams = BTreeSet::new();
    for id in ancestors(tx, session)? {
        let row = tx
            .query_row(
                "SELECT spec,activated_ms FROM session_budget WHERE session_id=?1",
                [&id],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<i64>>(1)?)),
            )
            .optional()?;
        if let Some((spec, activation)) = row {
            push(
                Snapshot {
                    id: id.clone(),
                    kind: "session".into(),
                    owner: id.clone(),
                    budget: budget(&spec, &id)?,
                    evidence: json!({"activation_ms":activation}),
                },
                &mut all,
            );
        }
        let run = tx
            .query_row(
                "SELECT workflow_run_id FROM session WHERE id=?1",
                [&id],
                |r| r.get::<_, Option<String>>(0),
            )
            .optional()?
            .flatten();
        if let Some(run) = run {
            runs.insert(run);
        }
        let mut goals =
            tx.prepare("SELECT goal_id FROM goal WHERE session_id=?1 AND status='active'")?;
        for goal in goals.query_map([&id], |r| r.get::<_, String>(0))? {
            let goal = goal?;
            if let Some(snapshot) = logical_snapshot(tx, "goal", &id, &goal)? {
                push(snapshot, &mut all);
            }
        }
        let mut loops=tx.prepare("SELECT loop_id FROM session_loop WHERE session_id=?1 AND status='active' AND inflight=1")?;
        for lp in loops.query_map([&id], |r| r.get::<_, String>(0))? {
            let lp = lp?;
            if let Some(snapshot) = logical_snapshot(tx, "loop", &id, &lp)? {
                push(snapshot, &mut all);
            }
        }
        let mut members=tx.prepare("SELECT t.id FROM team t JOIN team_member m ON m.team_id=t.id WHERE m.session_id=?1 AND t.status='active'")?;
        for team in members.query_map([&id], |r| r.get::<_, String>(0))? {
            teams.insert(team?);
        }
    }
    for run in runs {
        let row = tx
            .query_row(
                "SELECT spec,activated_ms,active_ms FROM workflow_run_budget WHERE run_id=?1",
                [&run],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, Option<i64>>(1)?,
                        r.get::<_, i64>(2)?,
                    ))
                },
            )
            .optional()?;
        if let Some((spec, activation, active)) = row {
            push(
                Snapshot {
                    id: run.clone(),
                    kind: "run".into(),
                    owner: run.clone(),
                    budget: run_budget(&spec, &run)?,
                    evidence: json!({"activation_ms":activation,"active_ms":active}),
                },
                &mut all,
            );
        } else {
            missing_run(tx, session, &run)?;
        }
    }
    for team in teams {
        let row = tx
            .query_row(
                "SELECT budget,created_ms FROM team WHERE id=?1",
                [&team],
                |r| Ok((r.get::<_, Option<String>>(0)?, r.get::<_, i64>(1)?)),
            )
            .optional()?;
        if let Some((Some(spec), activation)) = row {
            push(
                Snapshot {
                    id: team.clone(),
                    kind: "team".into(),
                    owner: team.clone(),
                    budget: budget(&spec, &team)?,
                    evidence: json!({"activation_ms":activation}),
                },
                &mut all,
            );
        }
    }
    Ok(all)
}

fn missing_run(tx: &Connection, session: &str, run: &str) -> Result<(), StoreError> {
    let bound: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM budget_scope_binding WHERE session_id=?1 AND scope_id=?2)",
        params![session, run],
        |r| r.get(0),
    )?;
    if bound {
        return Ok(()); // The captured owner is resolved as uncertain by `latest`.
    }
    let original: Option<String> = tx
        .query_row("SELECT data FROM workflow_run WHERE id=?1", [run], |r| {
            r.get(0)
        })
        .optional()?;
    if let Some(original) = original {
        let value: Value = serde_json::from_str(&original).map_err(|e| invalid(run, e))?;
        if value.pointer("/budget/enforcement").and_then(Value::as_str) != Some("reserved") {
            return Ok(()); // Legacy Runs without a budget remain in soft mode.
        }
    }
    Err(invalid(run, "workflow budget evidence is missing"))
}

fn run_budget(spec: &str, id: &str) -> Result<Budget, StoreError> {
    let mut value: Value = serde_json::from_str(spec).map_err(|e| invalid(id, e))?;
    if let Some(object) = value.as_object_mut() {
        object.remove("max_agents");
    }
    budget(&value.to_string(), id)
}

fn sticky(tx: &Connection, session: &str) -> Result<Vec<Snapshot>, StoreError> {
    let mut q=tx.prepare("SELECT scope_id,kind,owner_id,spec,evidence FROM budget_scope_binding WHERE session_id=?1 ORDER BY scope_id")?;
    let rows = q
        .query_map([session], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    rows.into_iter()
        .map(|(id, kind, owner, spec, evidence)| {
            Ok(Snapshot {
                budget: budget(&spec, &id)?,
                evidence: serde_json::from_str(&evidence).map_err(|e| invalid(&id, e))?,
                id,
                kind,
                owner,
            })
        })
        .collect()
}

/// Capture inheritance in the same event transaction as child creation. Root Sessions
/// never bind their own current Goal/Loop permanently.
pub(super) fn bind_created(tx: &Transaction<'_>, event: &StoredEvent) -> Result<(), StoreError> {
    if event.kind != "session.created.1" {
        return Ok(());
    }
    let mut snapshots = BTreeMap::new();
    if let Some(parent) = event
        .data
        .pointer("/info/parent_id")
        .and_then(Value::as_str)
    {
        let saved_schedule = event.data.get("unattended").is_some();
        snapshots = discovered_with_schedule(tx, parent, !saved_schedule)?;
        if saved_schedule {
            snapshots.retain(|_, scope| scope.kind != "schedule");
        }
        for inherited in sticky(tx, parent)? {
            if !saved_schedule || inherited.kind != "schedule" {
                push(inherited, &mut snapshots);
            }
        }
    }
    // A run agent can carry its own run ID even when its initiating parent is not a
    // member. Likewise teammates are roots and their membership row may follow creation.
    for own in discovered_with_schedule(tx, &event.aggregate_id, false)?
        .into_values()
        .filter(|scope| scope.kind == "run")
    {
        push(own, &mut snapshots);
    }
    if let Some(schedule) = schedules::created(tx, event)? {
        push(schedule, &mut snapshots);
    }
    if let Some(team) = event
        .data
        .pointer("/info/team/team_id")
        .and_then(Value::as_str)
    {
        let row = tx
            .query_row(
                "SELECT budget,created_ms FROM team WHERE id=?1",
                [team],
                |r| Ok((r.get::<_, Option<String>>(0)?, r.get::<_, i64>(1)?)),
            )
            .optional()?;
        if let Some((Some(spec), activation)) = row {
            push(
                Snapshot {
                    id: team.into(),
                    kind: "team".into(),
                    owner: team.into(),
                    budget: budget(&spec, team)?,
                    evidence: json!({"activation_ms":activation}),
                },
                &mut snapshots,
            );
        }
    }
    for snapshot in snapshots.into_values() {
        tx.execute("INSERT INTO budget_scope_binding(session_id,scope_id,kind,owner_id,spec,evidence,created_ms)
            VALUES (?1,?2,?3,?4,?5,?6,?7) ON CONFLICT(session_id,scope_id) DO NOTHING",
            params![event.aggregate_id,snapshot.id,snapshot.kind,snapshot.owner,serde_json::to_string(&snapshot.budget).map_err(|e|invalid(&event.aggregate_id,e))?,snapshot.evidence.to_string(),event.time_ms])?;
    }
    Ok(())
}

fn latest(tx: &Connection, captured: &Snapshot) -> Result<(Snapshot, bool), StoreError> {
    let mut snapshot = captured.clone();
    let current = match captured.kind.as_str() {
        "session" => tx
            .query_row(
                "SELECT spec,activated_ms FROM session_budget WHERE session_id=?1",
                [&captured.owner],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<i64>>(1)?)),
            )
            .optional()?
            .map(|(spec, activation)| {
                snapshot.evidence["activation_ms"] = activation.map_or(Value::Null, Value::from);
                budget(&spec, &captured.id)
            })
            .transpose()?,
        "run" => tx
            .query_row(
                "SELECT spec,activated_ms,active_ms FROM workflow_run_budget WHERE run_id=?1",
                [&captured.owner],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, Option<i64>>(1)?,
                        r.get::<_, i64>(2)?,
                    ))
                },
            )
            .optional()?
            .map(|(spec, activation, active)| {
                snapshot.evidence = json!({"activation_ms":activation,"active_ms":active});
                run_budget(&spec, &captured.id)
            })
            .transpose()?,
        "team" => tx
            .query_row(
                "SELECT budget,created_ms FROM team WHERE id=?1",
                [&captured.owner],
                |r| Ok((r.get::<_, Option<String>>(0)?, r.get::<_, i64>(1)?)),
            )
            .optional()?
            .map(|(spec, activation)| {
                snapshot.evidence["activation_ms"] = activation.into();
                spec.map(|s| budget(&s, &captured.id)).transpose()
            })
            .transpose()?
            .flatten(),
        "schedule" => schedules::latest(tx, captured)?.map(|current| {
            snapshot.evidence = current.evidence;
            current.budget
        }),
        "goal" | "loop" => {
            let table = if captured.kind == "goal" {
                "goal"
            } else {
                "session_loop"
            };
            let key = if captured.kind == "goal" {
                "goal_id"
            } else {
                "loop_id"
            };
            let exists:bool=tx.query_row(&format!("SELECT EXISTS(SELECT 1 FROM {table} a JOIN session s ON a.session_id=s.id WHERE a.{key}=?1 AND a.session_id=?2)"),params![captured.id,captured.owner],|r|r.get(0))?;
            if exists {
                logical_snapshot(tx, &captured.kind, &captured.owner, &captured.id)?.map(
                    |current| {
                        snapshot.evidence = current.evidence;
                        current.budget
                    },
                )
            } else {
                None
            }
        }
        _ => return Err(invalid(&captured.id, "unknown inherited scope kind")),
    };
    let missing = current.is_none();
    if let Some(current) = current {
        snapshot.budget = tighten(&captured.budget, &current);
    }
    Ok((snapshot, missing))
}

fn backfilling(tx: &Connection) -> Result<bool, StoreError> {
    Ok(
        tx.query_row("SELECT EXISTS(SELECT 1 FROM usage_backfill)", [], |r| {
            r.get(0)
        })?,
    )
}

fn session_spend(tx: &Connection, id: &str) -> Result<Option<Spend>, StoreError> {
    let row=tx.query_row("SELECT cost+children_cost,
        input_tokens+output_tokens+reasoning_tokens+cache_read_tokens+cache_write_tokens+children_tokens,
        (SELECT COUNT(*) FROM event e WHERE e.aggregate_id=s.id AND e.type='session.step.ended.1' AND COALESCE(json_extract(e.data,'$.usage_accounted'),0)!=1)+
        COALESCE((SELECT SUM(c.turns) FROM session_children_charge c WHERE c.parent_id=s.id),0),
        children_usage_complete FROM session s WHERE id=?1",[id],|r|Ok((r.get::<_,f64>(0)?,r.get::<_,i64>(1)?,r.get::<_,i64>(2)?,r.get::<_,bool>(3)?))).optional()?;
    row.map(|(cost, tokens, turns, complete)| {
        Ok(Spend {
            cost,
            tokens: unsigned(tokens, id)?,
            turns: unsigned(turns, id)?
                .checked_add(reserved_turns(tx, Some(id), None)?)
                .ok_or_else(|| invalid(id, "turn total overflow"))?,
            incomplete: !complete || legacy_failed(tx, id)? || folded_ancestry(tx, id)?,
            unpriced: session_unpriced(tx, id)?,
        })
    })
    .transpose()
}

fn session_unpriced(tx: &Connection, id: &str) -> Result<bool, StoreError> {
    // Retained parent links in usage_step include purged descendants. A folded root
    // with unknown prices is conservatively unknown for any surviving subtree.
    let live: bool = tx.query_row(
        "WITH RECURSIVE down(id) AS (SELECT ?1 UNION
        SELECT s.id FROM session s JOIN down ON s.parent_id=down.id UNION
        SELECT u.session_id FROM usage_step u JOIN down ON u.parent_id=down.id)
        SELECT EXISTS(SELECT 1 FROM usage_step u JOIN down ON u.session_id=down.id
        LEFT JOIN budget_reservation r ON r.request_id=u.reservation_id
        WHERE u.cost IS NULL AND NOT (COALESCE(r.state,'')='released'
            OR (COALESCE(r.usage_complete,0)=1 AND r.actual_cost_microusd IS NOT NULL)))",
        [id],
        |r| r.get(0),
    )?;
    if live {
        return Ok(true);
    }
    let roots = ancestors(tx, id)?;
    for root in roots {
        let folded: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM usage_daily WHERE root_id=?1 AND unpriced_steps>0)",
            [root],
            |r| r.get(0),
        )?;
        if folded {
            return Ok(true);
        }
    }
    backfilling(tx)
}

fn legacy_failed(tx: &Connection, id: &str) -> Result<bool, StoreError> {
    Ok(tx.query_row("WITH RECURSIVE down(id) AS (SELECT ?1 UNION
        SELECT s.id FROM session s JOIN down ON s.parent_id=down.id UNION
        SELECT u.session_id FROM usage_step u JOIN down ON u.parent_id=down.id)
        SELECT EXISTS(SELECT 1 FROM usage_step u JOIN down ON u.session_id=down.id WHERE u.failed=1 AND u.reservation_id IS NULL)",[id],|r|r.get(0))?)
}

fn folded_ancestry(tx: &Connection, id: &str) -> Result<bool, StoreError> {
    // Folding drops the legacy failed-attempt flag. Native lifetime totals cannot
    // prove those attempts consumed zero tokens, even if successful totals survived.
    for root in ancestors(tx, id)? {
        if tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM usage_daily WHERE root_id=?1)",
            [&root],
            |r| r.get::<_, bool>(0),
        )? {
            return Ok(true);
        }
    }
    Ok(false)
}

fn run_spend(tx: &Connection, id: &str) -> Result<Spend, StoreError> {
    let mut q = tx.prepare("SELECT id FROM session WHERE workflow_run_id=?1 ORDER BY id")?;
    let members = q
        .query_map([id], |r| r.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    let mut spent = Spend::default();
    for member in members {
        let row =
            session_spend(tx, &member)?.ok_or_else(|| invalid(id, "run member disappeared"))?;
        spent.tokens = spent
            .tokens
            .checked_add(row.tokens)
            .ok_or_else(|| invalid(id, "run token overflow"))?;
        spent.turns = spent
            .turns
            .checked_add(row.turns)
            .ok_or_else(|| invalid(id, "run turn overflow"))?;
        spent.cost += row.cost;
        spent.incomplete |= row.incomplete;
        spent.unpriced |= row.unpriced;
    }
    // Native run aggregation has this same root-membership convention; retained
    // child charges belong to their member Session, never a new provider dispatch.
    Ok(spent)
}

fn reserved_turns(
    tx: &Connection,
    session: Option<&str>,
    scope: Option<&str>,
) -> Result<u64, StoreError> {
    let count:i64=tx.query_row("SELECT COALESCE(SUM(turns),0) FROM (
        SELECT MAX(COALESCE(r.actual_turns,0)) turns FROM budget_reservation r
        WHERE (?1 IS NULL OR r.session_id=?1) AND (?2 IS NULL OR EXISTS(
            SELECT 1 FROM budget_reservation_allocation a WHERE a.request_id=r.request_id AND a.scope_id=?2))
        GROUP BY r.session_id,COALESCE(r.call_id,r.request_id))",params![session,scope],|r|r.get(0))?;
    unsigned(count, scope.or(session).unwrap_or("turns"))
}

fn window_reserved_turns(
    tx: &Connection,
    roots_json: Option<&str>,
    since: i64,
    id: &str,
) -> Result<u64, StoreError> {
    let count:i64=tx.query_row("SELECT COALESCE(SUM(turns),0) FROM (
        SELECT MAX(COALESCE(r.actual_turns,0)) turns,
            MIN((SELECT MIN(p.received_ms) FROM budget_reservation_receipt p
                WHERE p.request_id=r.request_id AND json_extract(p.actual,'$.turns')>0)) first_confirmed
        FROM budget_reservation r WHERE EXISTS (SELECT 1 FROM usage_step u
            WHERE u.reservation_id=r.request_id AND (?1 IS NULL OR u.root_id IN(SELECT value FROM json_each(?1))))
        GROUP BY r.session_id,COALESCE(r.call_id,r.request_id)) WHERE first_confirmed>=?2",params![roots_json,since],|r|r.get(0))?;
    unsigned(count, id)
}

fn logical_spend(tx: &Connection, snapshot: &Snapshot) -> Result<Spend, StoreError> {
    let (cost,tokens,unpriced):(f64,i64,i64)=tx.query_row("SELECT
        COALESCE(SUM(r.actual_cost_microusd),0)/1000000.0,
        COALESCE(SUM(COALESCE(json_extract(r.actual_tokens,'$.input'),0)+COALESCE(json_extract(r.actual_tokens,'$.output'),0)+
        COALESCE(json_extract(r.actual_tokens,'$.reasoning'),0)+COALESCE(json_extract(r.actual_tokens,'$.cache_read'),0)+
        COALESCE(json_extract(r.actual_tokens,'$.cache_write'),0)),0),
        COALESCE(SUM(r.state='settled' AND r.actual_cost_microusd IS NULL),0)
        FROM budget_reservation r JOIN budget_reservation_allocation a USING(request_id) WHERE a.scope_id=?1",[&snapshot.id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?)))?;
    Ok(Spend {
        cost,
        tokens: unsigned(tokens, &snapshot.id)?,
        turns: reserved_turns(tx, None, Some(&snapshot.id))?,
        incomplete: snapshot.evidence["version"] != 1,
        unpriced: unpriced > 0,
    })
}

fn team_roots(tx: &Connection, id: &str) -> Result<Vec<String>, StoreError> {
    let mut q = tx.prepare(
        "SELECT session_id FROM team_member WHERE team_id=?1 UNION
        SELECT value FROM team t,json_each(t.former_leads) WHERE t.id=?1",
    )?;
    Ok(q.query_map([id], |r| r.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?)
}

fn ledger_window(
    tx: &Connection,
    id: &str,
    roots: Option<&[String]>,
    since: i64,
) -> Result<Spend, StoreError> {
    let roots_json = roots
        .map(serde_json::to_string)
        .transpose()
        .map_err(|e| invalid(id, e))?;
    let (cost,tokens,turns,unpriced,failed):(f64,i64,i64,i64,i64)=tx.query_row("SELECT TOTAL(cost),
        COALESCE(SUM(input_tokens+output_tokens+reasoning_tokens+cache_read_tokens+cache_write_tokens),0),
        COALESCE(SUM(purpose='turn' AND failed=0 AND reservation_id IS NULL),0),
        COALESCE(SUM(cost IS NULL AND NOT EXISTS(SELECT 1 FROM budget_reservation r
            WHERE r.request_id=u.reservation_id AND (r.state='released'
                OR (r.usage_complete=1 AND r.actual_cost_microusd IS NOT NULL)))),0),
        COALESCE(SUM(failed=1 AND reservation_id IS NULL),0) FROM usage_step u
        WHERE ended_ms>=?1 AND (?2 IS NULL OR root_id IN (SELECT value FROM json_each(?2)))",params![since,roots_json],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?)))?;
    let folded:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM usage_daily WHERE day>=strftime('%Y-%m-%d',?1/1000,'unixepoch')
        AND (?2 IS NULL OR root_id IN (SELECT value FROM json_each(?2))))",params![since,roots_json],|r|r.get(0))?;
    Ok(Spend {
        cost,
        tokens: unsigned(tokens, id)?,
        turns: unsigned(turns, id)?
            .checked_add(window_reserved_turns(tx, roots_json.as_deref(), since, id)?)
            .ok_or_else(|| invalid(id, "window turn total overflow"))?,
        incomplete: folded || failed > 0 || backfilling(tx)?,
        unpriced: unpriced > 0,
    })
}

/// Clamp huge finite budgets before conversion; fractional milliseconds round down.
fn wall_deadline(snapshot: &Snapshot, now: i64) -> Result<Option<i64>, StoreError> {
    let Some(maximum) = snapshot.budget.max_wall_seconds else {
        return Ok(None);
    };
    if !maximum.is_finite() || maximum < 0.0 {
        return Err(invalid(&snapshot.id, "invalid wall budget"));
    }
    let maximum_ms = (maximum * 1000.0).floor().min(i64::MAX as f64) as i64;
    let activation = snapshot.evidence["activation_ms"].as_i64();
    let elapsed = if snapshot.kind == "run" {
        let active = snapshot.evidence["active_ms"]
            .as_i64()
            .ok_or_else(|| invalid(&snapshot.id, "run active time evidence missing"))?;
        if active < 0 {
            return Err(invalid(&snapshot.id, "negative run active time"));
        }
        active.saturating_add(activation.map_or(0, |at| now.saturating_sub(at).max(0)))
    } else {
        let at = activation.ok_or_else(|| {
            invalid(
                &snapshot.id,
                "wall budget has no durable activation evidence",
            )
        })?;
        now.saturating_sub(at).max(0)
    };
    let remaining = maximum_ms.saturating_sub(elapsed);
    if remaining <= 0 {
        return Err(StoreError::Projector {
            kind: "budget.reservation.exceeded".into(),
            reason: json!({"scope":snapshot.id,"limit":"max_wall_seconds","message":"wall budget ended before provider dispatch"}).to_string(),
        });
    }
    Ok(Some(now.saturating_add(remaining)))
}

fn scope(
    tx: &Connection,
    captured: &Snapshot,
    now: i64,
) -> Result<(Scope, Option<i64>), StoreError> {
    let (snapshot, missing) = latest(tx, captured)?;
    let mut spent = match snapshot.kind.as_str() {
        "session" => session_spend(tx, &snapshot.owner)?.unwrap_or(Spend {
            incomplete: true,
            ..Default::default()
        }),
        "run" => run_spend(tx, &snapshot.owner)?,
        "goal" | "loop" | "schedule" => logical_spend(tx, &snapshot)?,
        "team" => {
            let since = snapshot.evidence["activation_ms"]
                .as_i64()
                .ok_or_else(|| invalid(&snapshot.id, "team creation evidence missing"))?;
            ledger_window(
                tx,
                &snapshot.id,
                Some(&team_roots(tx, &snapshot.id)?),
                since,
            )?
        }
        _ => return Err(invalid(&snapshot.id, "unknown budget scope")),
    };
    spent.incomplete |= missing;
    let mut result = as_scope(&snapshot, &spent)?;
    let deadline = wall_deadline(&snapshot, now)?;
    // Goals count evaluator/controller Turns, not individual provider requests.
    if snapshot.kind == "goal" {
        result.max_turns = None;
    }
    Ok((result, deadline))
}

fn as_scope(snapshot: &Snapshot, spent: &Spend) -> Result<Scope, StoreError> {
    let cap = snapshot
        .budget
        .max_cost_usd
        .map(|v| {
            money(v, &snapshot.id)?;
            Ok::<_, StoreError>((v * 1_000_000.0).floor() as u64)
        })
        .transpose()?;
    Ok(Scope {
        id: snapshot.id.clone(),
        max_tokens: snapshot.budget.max_tokens,
        max_turns: snapshot.budget.max_turns,
        max_cost_microusd: cap,
        spent_tokens: spent.tokens,
        spent_turns: spent.turns,
        spent_cost_microusd: money(spent.cost, &snapshot.id)?,
        uncertain: spent.incomplete || (cap.is_some() && spent.unpriced),
    })
}

fn daily_scope(
    tx: &Connection,
    budget: &Budget,
    now: i64,
) -> Result<(Scope, Option<i64>), StoreError> {
    let current = jiff::Timestamp::from_millisecond(now)
        .map_err(|e| invalid("daily", e))?
        .to_zoned(jiff::tz::TimeZone::system());
    let start = current
        .start_of_day()
        .map_err(|e| invalid("daily", e))?
        .timestamp()
        .as_millisecond();
    let zone = current.time_zone().iana_name().unwrap_or("UTC");
    // A stable ID carries only outstanding holds across the native daily reset.
    let id = "daily".to_owned();
    let snapshot = Snapshot {
        id: id.clone(),
        kind: "daily".into(),
        owner: "machine".into(),
        budget: budget.clone(),
        evidence: json!({"activation_ms":start,"zone":zone}),
    };
    budget.validate().map_err(|e| invalid(&id, e))?;
    let result = as_scope(&snapshot, &ledger_window(tx, &id, None, start)?)?;
    Ok((result, wall_deadline(&snapshot, now)?))
}

/// Resolve every live or inherited reserved ceiling without acquiring runtime state
/// locks or trusting a model-supplied scope list.
pub(super) fn collect(
    tx: &Transaction<'_>,
    session_id: &str,
    daily: Option<&Budget>,
    now: i64,
) -> Result<(Vec<Scope>, Option<i64>), StoreError> {
    let mut snapshots = discovered(tx, session_id)?;
    for inherited in sticky(tx, session_id)? {
        push(inherited, &mut snapshots);
    }
    let mut resolved = snapshots
        .values()
        .map(|snapshot| scope(tx, snapshot, now))
        .collect::<Result<Vec<_>, _>>()?;
    if let Some(daily) = daily.filter(|d| d.enforcement == Enforcement::Reserved) {
        resolved.push(daily_scope(tx, daily, now)?);
    }
    let deadline = resolved.iter().filter_map(|(_, deadline)| *deadline).min();
    Ok((
        resolved.into_iter().map(|(scope, _)| scope).collect(),
        deadline,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use cyber_core::paths::DatabaseLocation;
    use cyber_store::{EventRegistry, Store, StoreOptions, reservations as ledger};

    fn store() -> Store {
        Store::open(StoreOptions::new(
            DatabaseLocation::Memory,
            EventRegistry::default(),
        ))
        .unwrap()
    }

    fn reserved(tokens: u64) -> Budget {
        Budget {
            max_tokens: Some(tokens),
            enforcement: Enforcement::Reserved,
            ..Default::default()
        }
    }

    fn session(tx: &Transaction<'_>, id: &str, parent: Option<&str>) -> Result<(), StoreError> {
        tx.execute("INSERT INTO session(id,title,directory,agent,model,mode,created_at,updated_at,last_seq,parent_id)
            VALUES (?1,'x','/r','build','test/main','default',1,1,0,?2)",params![id,parent])?;
        Ok(())
    }

    fn own_budget(tx: &Transaction<'_>, id: &str, tokens: u64) -> Result<(), StoreError> {
        tx.execute(
            "INSERT INTO session_budget(session_id,spec,activated_ms) VALUES (?1,?2,1)",
            params![id, serde_json::to_string(&reserved(tokens)).unwrap()],
        )?;
        Ok(())
    }

    fn birth(id: &str, parent: Option<&str>, team: Option<&str>) -> StoredEvent {
        StoredEvent {
            id: format!("created_{id}"),
            aggregate_id: id.into(),
            seq: 0,
            kind: "session.created.1".into(),
            data: json!({"info":{"parent_id":parent,"team":team.map(|id|json!({"team_id":id}))}}),
            time_ms: 2,
            causation_id: None,
        }
    }

    fn goal(tx: &Transaction<'_>, id: &str, version: u32) -> Result<(), StoreError> {
        tx.execute("INSERT INTO goal(goal_id,session_id,status,condition,created_ms) VALUES (?1,'root','active','x',1)",[id])?;
        tx.execute("INSERT INTO event(id,aggregate_id,seq,type,data,time) VALUES (?1,'root',0,'session.goal.set.1',?2,1)",
            params![format!("set_{id}"),json!({"goal_id":id,"budget":reserved(100),"reservation_scope_version":version}).to_string()])?;
        tx.execute("INSERT INTO event(id,aggregate_id,seq,type,data,time) VALUES (?1,'root',1,'session.goal.activated.1',?2,1)",
            params![format!("activated_{id}"),json!({"goal_id":id,"activated_ms":1,"tokens_before":0,"cost_before":0}).to_string()])?;
        Ok(())
    }

    #[test]
    fn inherited_budget_never_widens_and_missing_parent_fails_closed() {
        let store = store();
        store
            .transaction(|tx| {
                session(tx, "root", None)?;
                own_budget(tx, "root", 10)?;
                session(tx, "child", Some("root"))?;
                bind_created(tx, &birth("child", Some("root"), None))?;
                tx.execute(
                    "UPDATE session_budget SET spec=?1 WHERE session_id='root'",
                    [serde_json::to_string(&reserved(100)).unwrap()],
                )?;
                assert_eq!(collect(tx, "child", None, 3)?.0[0].max_tokens, Some(10));
                tx.execute(
                    "UPDATE session_budget SET spec=?1 WHERE session_id='root'",
                    [serde_json::to_string(&reserved(5)).unwrap()],
                )?;
                assert_eq!(collect(tx, "child", None, 3)?.0[0].max_tokens, Some(5));
                tx.execute("DELETE FROM session WHERE id='root'", [])?;
                assert!(collect(tx, "child", None, 4)?.0[0].uncertain);
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn ended_goal_remains_on_child_but_not_on_new_root_work() {
        let store = store();
        store
            .transaction(|tx| {
                session(tx, "root", None)?;
                goal(tx, "gol_work", 1)?;
                session(tx, "child", Some("root"))?;
                bind_created(tx, &birth("child", Some("root"), None))?;
                tx.execute("UPDATE goal SET status='met',ended_ms=3", [])?;
                assert!(collect(tx, "root", None, 4)?.0.is_empty());
                let child = collect(tx, "child", None, 4)?.0;
                assert_eq!(child.len(), 1);
                assert_eq!(child[0].id, "gol_work");
                assert!(!child[0].uncertain);
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn logical_scope_uses_all_five_persisted_actual_classes_and_rejects_legacy_version() {
        let store = store();
        store.transaction(|tx|{
            session(tx,"root",None)?;goal(tx,"gol_work",1)?;
            let caps=collect(tx,"root",None,3)?.0;
            let request=ledger::Request{request_id:"req".into(),session_id:"root".into(),provider:"test".into(),model:"main".into(),purpose:"turn".into(),
                call_id:None,bound_source:None,normalized_sha256:None,deadline_ms:None,estimate:ledger::Estimate{tokens:ledger::TokenAmounts{input:20,output:20,reasoning:20,cache_read:20,cache_write:20},total_tokens:20,turns:1,cost_microusd:None}};
            ledger::reserve(tx,&request,&caps,3)?;
            ledger::mark_dispatched(tx,"req",4)?;
            ledger::settle(tx,"req","proof",&ledger::Receipt{tokens:Some(ledger::TokenAmounts{input:1,output:2,reasoning:3,cache_read:4,cache_write:5}),turns:1,cost_microusd:None,complete:true},5)?;
            let scope=collect(tx,"root",None,6)?.0.remove(0);
            assert_eq!(scope.spent_tokens,15);assert_eq!(scope.max_turns,None);
            tx.execute("UPDATE event SET data=json_set(data,'$.reservation_scope_version',0) WHERE type='session.goal.set.1'",[])?;
            assert!(collect(tx,"root",None,7)?.0[0].uncertain);
            Ok(())
        }).unwrap();
    }

    #[test]
    fn root_teammate_is_bound_before_membership_projection_and_cannot_escape_removed_team() {
        let store = store();
        store.transaction(|tx|{
            session(tx,"member",None)?;
            tx.execute("INSERT INTO team(id,name,lead_id,directory,status,budget,created_ms,updated_ms)
                VALUES ('team_work','work','lead','/r','active',?1,1,1)",[serde_json::to_string(&reserved(100)).unwrap()])?;
            bind_created(tx,&birth("member",None,Some("team_work")))?;
            tx.execute("DELETE FROM team WHERE id='team_work'",[])?;
            let scopes=collect(tx,"member",None,3)?.0;
            assert_eq!(scopes.len(),1);assert_eq!(scopes[0].id,"team_work");assert!(scopes[0].uncertain);
            Ok(())
        }).unwrap();
    }

    #[test]
    fn own_run_and_ancestor_run_are_deduplicated_and_survive_as_a_missing_owner_refusal() {
        let store = store();
        store.transaction(|tx|{
            session(tx,"root",None)?;session(tx,"child",Some("root"))?;
            tx.execute("INSERT INTO workflow_run(id,session_id,name,status,started_ms,data) VALUES ('run_work','root','x','running',1,'{}')",[])?;
            tx.execute("INSERT INTO workflow_run_budget(run_id,spec,activated_ms,active_ms) VALUES ('run_work',?1,1,0)",[serde_json::to_string(&reserved(100)).unwrap()])?;
            tx.execute("UPDATE session SET workflow_run_id='run_work'",[])?;
            bind_created(tx,&birth("child",Some("root"),None))?;
            assert_eq!(collect(tx,"child",None,3)?.0.len(),1);
            tx.execute("DELETE FROM workflow_run WHERE id='run_work'",[])?;
            assert!(collect(tx,"child",None,4)?.0[0].uncertain);
            Ok(())
        }).unwrap();
    }
    #[test]
    fn deadlines_exclude_run_pauses_and_round_down_fractional_milliseconds() {
        let mut snapshot = Snapshot {
            id: "run".into(),
            kind: "run".into(),
            owner: "run".into(),
            budget: Budget {
                max_wall_seconds: Some(2.0009),
                ..reserved(100)
            },
            evidence: json!({"activation_ms":1000,"active_ms":500}),
        };
        assert_eq!(wall_deadline(&snapshot, 1500).unwrap(), Some(2500));
        snapshot.evidence["activation_ms"] = Value::Null;
        assert_eq!(wall_deadline(&snapshot, 10000).unwrap(), Some(11500));
        snapshot.evidence["active_ms"] = json!(2000);
        assert!(wall_deadline(&snapshot, 10000).is_err());
        snapshot.kind = "session".into();
        assert!(wall_deadline(&snapshot, 10000).is_err());
    }

    #[test]
    fn daily_scope_carries_only_open_holds_across_reset_windows() {
        let store = store();
        store
            .transaction(|tx| {
                let before = daily_scope(tx, &reserved(100), 1_700_000_000_000)?.0;
                let after = daily_scope(tx, &reserved(100), 1_700_086_400_000)?.0;
                assert_eq!(before.id, "daily");
                assert_eq!(before.id, after.id);
                assert_eq!(before.spent_tokens, 0);
                assert_eq!(after.spent_tokens, 0);
                let mut request = ledger::Request {
                    request_id: "old-day".into(),
                    session_id: "root".into(),
                    provider: "test".into(),
                    model: "main".into(),
                    purpose: "turn".into(),
                    call_id: None,
                    bound_source: None,
                    normalized_sha256: None,
                    deadline_ms: None,
                    estimate: ledger::Estimate {
                        tokens: ledger::TokenAmounts {
                            input: 60,
                            ..Default::default()
                        },
                        total_tokens: 60,
                        turns: 1,
                        cost_microusd: None,
                    },
                };
                ledger::reserve(tx, &request, &[before], 1)?;
                request.request_id = "new-day".into();
                assert!(matches!(
                    ledger::reserve(tx, &request, std::slice::from_ref(&after), 2),
                    Err(ledger::LedgerError::Exceeded { .. })
                ));
                ledger::mark_dispatched(tx, "old-day", 2)?;
                ledger::settle(
                    tx,
                    "old-day",
                    "previous-day-receipt",
                    &ledger::Receipt {
                        tokens: Some(ledger::TokenAmounts {
                            input: 60,
                            ..Default::default()
                        }),
                        turns: 1,
                        cost_microusd: None,
                        complete: true,
                    },
                    3,
                )?;
                // Completed previous-day spend is outside the new native window. Its
                // settled reservation no longer holds an allocation against that window.
                assert!(ledger::reserve(tx, &request, &[after], 4)?.new);
                Ok(())
            })
            .unwrap();
    }

    fn commit_daily_receipt(
        tx: &Transaction<'_>,
        id: &str,
        receipt_id: &str,
        tokens: u64,
        complete: bool,
        now: i64,
    ) -> Result<(), StoreError> {
        let result = ledger::settle(
            tx,
            id,
            receipt_id,
            &ledger::Receipt {
                tokens: Some(ledger::TokenAmounts {
                    input: tokens,
                    ..Default::default()
                }),
                turns: 0,
                cost_microusd: Some(tokens),
                complete,
            },
            now,
        )?;
        let delta = result.delta.tokens.unwrap_or_default().input;
        tx.execute("INSERT INTO usage_step(event_id,session_id,root_id,agent,purpose,provider,model,started_ms,ended_ms,input_tokens,output_tokens,reasoning_tokens,cache_read_tokens,cache_write_tokens,prompt_tokens,cost,directory,reservation_id) VALUES (?1,'root','root','build','title','test','main',?2,?2,?3,0,0,0,0,?3,?4,'/r',?5)",params![receipt_id,now,i64::try_from(delta).unwrap(),result.delta.cost_microusd.unwrap_or(0) as f64/1_000_000.0,id])?;
        Ok(())
    }
    #[test]
    fn midnight_partial_and_unknown_settlement_charge_receipt_day_deltas_once() {
        store().transaction(|tx|{
            session(tx,"root",None)?;
            let old=1_700_000_000_000;let next=old+86_400_000;let later=next+86_400_000;
            let mut request=ledger::Request{request_id:"before-midnight".into(),session_id:"root".into(),provider:"test".into(),model:"main".into(),purpose:"title".into(),call_id:None,bound_source:None,normalized_sha256:None,deadline_ms:None,estimate:ledger::Estimate{tokens:ledger::TokenAmounts{input:100,..Default::default()},total_tokens:100,turns:0,cost_microusd:Some(100)}};
            let ceiling=Budget{max_cost_usd:Some(0.0001),..reserved(100)};
            ledger::reserve(tx,&request,&[daily_scope(tx,&ceiling,old)?.0],old)?;
            ledger::mark_dispatched(tx,&request.request_id,old)?;
            commit_daily_receipt(tx,&request.request_id,"partial-before",20,false,old)?;
            let after=daily_scope(tx,&ceiling,next)?.0;
            assert_eq!(after.spent_tokens,0);
            request.request_id="new-work".into();request.estimate.total_tokens=21;request.estimate.tokens.input=21;request.estimate.cost_microusd=Some(21);
            assert!(matches!(ledger::reserve(tx,&request,&[after],next),Err(ledger::LedgerError::Exceeded{..})),"the old request still holds its remaining80 after midnight");
            commit_daily_receipt(tx,"before-midnight","late-final",70,true,next)?;
            let after=daily_scope(tx,&ceiling,next)?.0;
            assert_eq!(after.spent_tokens,50);assert_eq!(after.spent_cost_microusd,50);
            request.estimate.total_tokens=51;request.estimate.tokens.input=51;request.estimate.cost_microusd=Some(51);
            assert!(matches!(ledger::reserve(tx,&request,&[after],next),Err(ledger::LedgerError::Exceeded{..})),"late cumulative70 must charge only the new50 delta");
            request.request_id="unknown".into();request.estimate.total_tokens=30;request.estimate.tokens.input=30;request.estimate.cost_microusd=Some(30);
            ledger::reserve(tx,&request,&[daily_scope(tx,&ceiling,next)?.0],next)?;
            ledger::mark_dispatched(tx,"unknown",next)?;
            let record=ledger::settle(tx,"unknown","lost",&ledger::Receipt{tokens:None,turns:0,cost_microusd:None,complete:false},next)?.record;
            request.request_id="third-day".into();request.estimate.total_tokens=71;request.estimate.tokens.input=71;request.estimate.cost_microusd=Some(71);
            assert!(matches!(ledger::reserve(tx,&request,&[daily_scope(tx,&ceiling,later)?.0],later),Err(ledger::LedgerError::Exceeded{..})),"uncertain old holds do not disappear on another reset");
            let command=ledger::Reconciliation{operation_id:"late-owner".into(),expected_revision:record.revision,owner:"owner".into(),reason:"recovered provider receipt".into(),action:ledger::ReconcileAction::Settle{receipt_event_id:"late-owner-receipt".into(),actual:ledger::Receipt{tokens:Some(ledger::TokenAmounts{input:25,..Default::default()}),turns:0,cost_microusd:Some(25),complete:true}}};
            ledger::reconcile(tx,"unknown",&command,later)?;
            tx.execute("INSERT INTO usage_step(event_id,session_id,root_id,agent,purpose,provider,model,started_ms,ended_ms,input_tokens,output_tokens,reasoning_tokens,cache_read_tokens,cache_write_tokens,prompt_tokens,cost,directory,reservation_id) VALUES ('late-owner-receipt','root','root','build','title','test','main',?1,?1,25,0,0,0,0,25,0.000025,'/r','unknown')",[later])?;
            assert!(!ledger::reconcile(tx,"unknown",&command,later)?.applied);
            let after=daily_scope(tx,&ceiling,later)?.0;assert_eq!(after.spent_tokens,25);assert_eq!(after.spent_cost_microusd,25);
            request.estimate.total_tokens=75;request.estimate.tokens.input=75;request.estimate.cost_microusd=Some(75);
            assert!(ledger::reserve(tx,&request,&[after],later)?.new);
            Ok(())
        }).unwrap();
    }
    #[test]
    fn legacy_run_without_a_reserved_budget_preserves_soft_behavior() {
        let store = store();
        store.transaction(|tx| {
            session(tx,"root",None)?;
            tx.execute("INSERT INTO workflow_run(id,session_id,name,status,started_ms,data) VALUES ('old_run','root','x','running',1,'{}')",[])?;
            tx.execute("UPDATE session SET workflow_run_id='old_run'",[])?;
            assert!(collect(tx,"root",None,3)?.0.is_empty());
            tx.execute("UPDATE workflow_run SET data=?1",[json!({"budget":{"enforcement":"reserved"}}).to_string()])?;
            assert!(collect(tx,"root",None,3).is_err());
            Ok(())
        }).unwrap();
    }
    #[test]
    fn owner_release_resolves_linked_unpriced_window_uncertainty_only() {
        let store = store();
        store.transaction(|tx| {
            let request = ledger::Request {
                request_id:"unknown".into(),session_id:"root".into(),provider:"test".into(),model:"main".into(),purpose:"title".into(),
                call_id:None,bound_source:None,normalized_sha256:None,deadline_ms:None,
                estimate:ledger::Estimate{tokens:ledger::TokenAmounts{input:1,..Default::default()},total_tokens:1,turns:0,cost_microusd:Some(1)},
            };
            let scope = Scope { id:"daily".into(),max_tokens:Some(100),max_turns:None,max_cost_microusd:Some(100),spent_tokens:0,spent_turns:0,spent_cost_microusd:0,uncertain:false };
            ledger::reserve(tx,&request,&[scope],1)?;
            ledger::mark_dispatched(tx,"unknown",2)?;
            let record=ledger::settle(tx,"unknown","unknown-receipt",&ledger::Receipt{tokens:Some(ledger::TokenAmounts{input:1,..Default::default()}),turns:0,cost_microusd:None,complete:false},3)?.record;
            tx.execute("INSERT INTO usage_step(event_id,session_id,root_id,agent,purpose,provider,model,started_ms,ended_ms,input_tokens,output_tokens,reasoning_tokens,cache_read_tokens,cache_write_tokens,prompt_tokens,cost,directory,reservation_id)
                VALUES ('unknown-receipt','root','root','build','title','test','main',2,3,1,0,0,0,0,1,NULL,'/r','unknown')",[])?;
            assert!(ledger_window(tx,"daily",None,1)?.unpriced);
            ledger::reconcile(tx,"unknown",&ledger::Reconciliation{operation_id:"owner-release".into(),expected_revision:record.revision,owner:"owner".into(),reason:"provider could not recover final billing".into(),action:ledger::ReconcileAction::Release},4)?;
            assert!(!ledger_window(tx,"daily",None,1)?.unpriced);
            ledger::settle(tx,"unknown","complete-priced",&ledger::Receipt{tokens:Some(ledger::TokenAmounts{input:1,..Default::default()}),turns:0,cost_microusd:Some(1),complete:true},5)?;
            assert!(!ledger_window(tx,"daily",None,1)?.unpriced);
            tx.execute("UPDATE usage_step SET failed=1 WHERE reservation_id='unknown'",[])?;
            assert!(!ledger_window(tx,"daily",None,1)?.incomplete);
            tx.execute("INSERT INTO usage_step(event_id,session_id,root_id,parent_id,workflow_run_id,agent,purpose,failed,provider,model,started_ms,ended_ms,input_tokens,output_tokens,reasoning_tokens,cache_read_tokens,cache_write_tokens,prompt_tokens,cost,directory) SELECT 'historical-unknown',session_id,root_id,parent_id,workflow_run_id,agent,purpose,failed,provider,model,started_ms,ended_ms,input_tokens,output_tokens,reasoning_tokens,cache_read_tokens,cache_write_tokens,prompt_tokens,cost,directory FROM usage_step WHERE event_id='unknown-receipt'",[])?;
            assert!(ledger_window(tx,"daily",None,1)?.unpriced);
            Ok(())
        }).unwrap();
    }
    #[test]
    fn successful_receipt_charges_session_turn_before_canonical_end_event() {
        let store = store();
        store.transaction(|tx| {
            session(tx,"root",None)?;
            own_budget(tx,"root",100)?;
            let caps=Budget{max_turns:Some(1),..reserved(100)};
            tx.execute("UPDATE session_budget SET spec=?1",[serde_json::to_string(&caps).unwrap()])?;
            let request=ledger::Request{request_id:"first".into(),session_id:"root".into(),provider:"test".into(),model:"main".into(),purpose:"turn".into(),call_id:Some("canonical".into()),bound_source:None,normalized_sha256:None,deadline_ms:None,
                estimate:ledger::Estimate{tokens:ledger::TokenAmounts{input:1,..Default::default()},total_tokens:1,turns:1,cost_microusd:None}};
            // Test native Session fixtures default historical child completeness to
            // false; this new root has no historical children.
            tx.execute("UPDATE session SET children_usage_complete=1",[])?;
            ledger::reserve(tx,&request,&collect(tx,"root",None,3)?.0,3)?;
            ledger::mark_dispatched(tx,"first",4)?;
            ledger::settle(tx,"first","finished",&ledger::Receipt{tokens:Some(ledger::TokenAmounts{input:1,..Default::default()}),turns:1,cost_microusd:None,complete:true},5)?;
            let scopes=collect(tx,"root",None,6)?.0;
            assert_eq!(scopes[0].spent_turns,1);
            session(tx,"child",Some("root"))?;
            bind_created(tx,&birth("child",Some("root"),None))?;
            let mut next=request;
            next.request_id="child-attempt".into();next.session_id="child".into();next.call_id=Some("other-canonical".into());
            assert!(matches!(ledger::reserve(tx,&next,&collect(tx,"child",None,6)?.0,6),Err(ledger::LedgerError::Exceeded{limit,..}) if limit=="max_turns"));
            tx.execute("INSERT INTO event(id,aggregate_id,seq,type,data,time) VALUES ('canonical-end','root',0,'session.step.ended.1',?1,7)",[json!({"usage_accounted":true}).to_string()])?;
            assert_eq!(collect(tx,"root",None,8)?.0[0].spent_turns,1);
            Ok(())
        }).unwrap();
    }
}
