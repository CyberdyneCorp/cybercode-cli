use super::query::required;
use super::{
    Admission, LedgerError, Receipt, ReconcileAction, Reconciled, Reconciliation, Record, Request,
    Scope, Settlement, State, decode, encode, get, integer, invalid, sql, text,
};
use rusqlite::{OptionalExtension, Transaction, params};
use std::collections::BTreeSet;

fn validate_request(request: &Request) -> Result<(), LedgerError> {
    for (field, value) in [
        ("request_id", &request.request_id),
        ("session_id", &request.session_id),
        ("provider", &request.provider),
        ("model", &request.model),
        ("purpose", &request.purpose),
    ] {
        text(value, field, 512)?;
    }
    sql(request.estimate.total_tokens)?;
    if let Some(call) = &request.call_id {
        text(call, "call_id", 512)?;
    }
    if let Some(source) = &request.bound_source {
        text(source, "bound_source", 1024)?;
    }
    if let Some(digest) = &request.normalized_sha256 {
        let hex = digest.strip_prefix("sha256:").unwrap_or(digest);
        if hex.len() != 64 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(invalid("normalized_sha256 must be a SHA-256 digest"));
        }
    }
    sql(request.estimate.turns)?;
    if let Some(cost) = request.estimate.cost_microusd {
        sql(cost)?;
    }
    for amount in request.estimate.tokens.classes() {
        if amount > request.estimate.total_tokens {
            return Err(invalid(
                "a token-class envelope exceeds the total upper bound",
            ));
        }
    }
    Ok(())
}

fn validate_receipt(receipt: &Receipt) -> Result<(), LedgerError> {
    sql(receipt.turns)?;
    if let Some(tokens) = receipt.tokens {
        sql(tokens.total()?)?;
    }
    if let Some(cost) = receipt.cost_microusd {
        sql(cost)?;
    }
    if receipt.complete && receipt.tokens.is_none() {
        return Err(invalid("complete receipt requires token-class evidence"));
    }
    Ok(())
}

fn validate_scope(scope: &Scope) -> Result<(), LedgerError> {
    text(&scope.id, "scope_id", 512)?;
    for value in [
        scope.max_tokens,
        scope.max_turns,
        scope.max_cost_microusd,
        Some(scope.spent_tokens),
        Some(scope.spent_turns),
        Some(scope.spent_cost_microusd),
    ]
    .into_iter()
    .flatten()
    {
        sql(value)?;
    }
    Ok(())
}

/// Admit all allocations or none. Only `new == true` conveys authority to subsequently
/// call `mark_dispatched`; replaying a request ID never authorizes another provider call.
pub fn reserve(
    tx: &Transaction<'_>,
    request: &Request,
    scopes: &[Scope],
    now: i64,
) -> Result<Admission, LedgerError> {
    validate_request(request)?;
    if scopes.is_empty() || scopes.len() > 256 {
        return Err(invalid("expected 1..=256 scopes"));
    }
    let mut ids = BTreeSet::new();
    for scope in scopes {
        validate_scope(scope)?;
        if !ids.insert(scope.id.as_str()) {
            return Err(invalid("duplicate scope allocation"));
        }
    }
    if let Some(existing) = get(tx, &request.request_id)? {
        let previous: BTreeSet<_> = existing
            .scopes
            .iter()
            .map(|scope| scope.id.as_str())
            .collect();
        if existing.request != *request || previous != ids {
            return Err(LedgerError::Conflict(request.request_id.clone()));
        }
        return Ok(Admission {
            new: false,
            record: existing,
        });
    }
    ensure_live(request, now)?;
    for scope in scopes {
        admit_scope(tx, request, scope)?;
    }
    tx.execute(
        "INSERT INTO budget_reservation(request_id,session_id,provider,model,purpose,state,
        estimated_tokens,token_upper,turns_upper,cost_upper_microusd,created_ms,updated_ms,bound_source,normalized_sha256,call_id,deadline_ms)
        VALUES (?1,?2,?3,?4,?5,'reserved',?6,?7,?8,?9,?10,?10,?11,?12,?13,?14)",
        params![
            request.request_id,
            request.session_id,
            request.provider,
            request.model,
            request.purpose,
            encode(&request.estimate.tokens)?,
            sql(request.estimate.total_tokens)?,
            sql(request.estimate.turns)?,
            request.estimate.cost_microusd.map(sql).transpose()?,
            now,request.bound_source,request.normalized_sha256,request.call_id,request.deadline_ms
        ],
    )?;
    for scope in scopes {
        tx.execute("INSERT INTO budget_reservation_allocation(request_id,scope_id,max_tokens,max_turns,
            max_cost_microusd,spent_tokens,spent_turns,spent_cost_microusd) VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
            params![request.request_id,scope.id,scope.max_tokens.map(sql).transpose()?,scope.max_turns.map(sql).transpose()?,
                scope.max_cost_microusd.map(sql).transpose()?,sql(scope.spent_tokens)?,sql(scope.spent_turns)?,sql(scope.spent_cost_microusd)?])?;
    }
    Ok(Admission {
        new: true,
        record: required(tx, &request.request_id)?,
    })
}

fn ensure_live(request: &Request, now: i64) -> Result<(), LedgerError> {
    if request.deadline_ms.is_some_and(|deadline| deadline <= now) {
        return Err(LedgerError::Exceeded {
            scope: request.request_id.clone(),
            limit: "max_wall_seconds".into(),
        });
    }
    Ok(())
}

fn admit_scope(tx: &Transaction<'_>, request: &Request, scope: &Scope) -> Result<(), LedgerError> {
    if scope.uncertain {
        return Err(LedgerError::Uncertain(scope.id.clone()));
    }
    if scope.max_cost_microusd.is_some() && request.estimate.cost_microusd.is_none() {
        return Err(LedgerError::Unpriced(scope.id.clone()));
    }
    let (tokens, cost, unpriced): (u64, u64, u64) = tx.query_row(
        "SELECT COALESCE(SUM(MAX(0,r.token_upper-
        COALESCE(json_extract(r.actual_tokens,'$.input')+json_extract(r.actual_tokens,'$.output')+
        json_extract(r.actual_tokens,'$.reasoning')+json_extract(r.actual_tokens,'$.cache_read')+
        json_extract(r.actual_tokens,'$.cache_write'),0))),0),
        COALESCE(SUM(MAX(0,r.cost_upper_microusd-COALESCE(r.actual_cost_microusd,0))),0),
        COALESCE(SUM(r.cost_upper_microusd IS NULL),0)
        FROM budget_reservation_allocation a JOIN budget_reservation r USING(request_id)
        WHERE a.scope_id=?1 AND r.state IN ('reserved','uncertain')",
        [&scope.id],
        |r| Ok((integer(r, 0)?, integer(r, 1)?, integer(r, 2)?)),
    )?;
    // A retry is another priced request, but the same canonical Turn. A settled
    // successful sibling cancels its earlier uncertain attempt's Turn-only hold.
    let turns = tx.query_row(
        "SELECT COALESCE(SUM(MAX(0,held-confirmed)),0) FROM (
            SELECT MAX(CASE WHEN r.state IN ('reserved','uncertain') THEN r.turns_upper ELSE 0 END) held,
                MAX(COALESCE(r.actual_turns,0)) confirmed
            FROM budget_reservation r JOIN budget_reservation_allocation a USING(request_id)
            WHERE a.scope_id=?1 GROUP BY r.session_id,COALESCE(r.call_id,r.request_id))",
        [&scope.id], |r| integer(r,0),
    )?;
    if scope.max_cost_microusd.is_some() && unpriced > 0 {
        return Err(LedgerError::Unpriced(scope.id.clone()));
    }
    for (name, maximum, spent, held, requested) in [
        (
            "max_tokens",
            scope.max_tokens,
            scope.spent_tokens,
            tokens,
            request.estimate.total_tokens,
        ),
        (
            "max_turns",
            scope.max_turns,
            scope.spent_turns,
            turns,
            request.estimate.turns,
        ),
        (
            "max_cost_usd",
            scope.max_cost_microusd,
            scope.spent_cost_microusd,
            cost,
            request.estimate.cost_microusd.unwrap_or(0),
        ),
    ] {
        if maximum.is_some_and(|maximum| {
            spent
                .checked_add(held)
                .and_then(|n| n.checked_add(requested))
                .is_none_or(|n| n > maximum)
        }) {
            return Err(LedgerError::Exceeded {
                scope: scope.id.clone(),
                limit: name.into(),
            });
        }
    }
    Ok(())
}

/// Commit uncertainty before polling the adapter's network future. This is a one-use
/// transition; a duplicate caller must not obtain a second dispatch lease.
pub fn mark_dispatched(tx: &Transaction<'_>, id: &str, now: i64) -> Result<Record, LedgerError> {
    ensure_live(&required(tx, id)?.request, now)?;
    let n = tx.execute(
        "UPDATE budget_reservation SET state='uncertain',dispatched_ms=?2,updated_ms=?2,
        revision=revision+1 WHERE request_id=?1 AND state='reserved'",
        params![id, now],
    )?;
    if n != 1 {
        return Err(LedgerError::Transition(id.into()));
    }
    required(tx, id)
}

/// Confirmed cancellation before network dispatch. An uncertain allocation requires an
/// audited owner reconciliation instead, including after process restart.
pub fn release_before_dispatch(
    tx: &Transaction<'_>,
    id: &str,
    now: i64,
) -> Result<Record, LedgerError> {
    let record = required(tx, id)?;
    if record.state == State::Released && record.dispatched_ms.is_none() {
        return Ok(record);
    }
    if record.state != State::Reserved {
        return Err(LedgerError::Transition(id.into()));
    }
    tx.execute(
        "UPDATE budget_reservation SET state='released',released_ms=?2,updated_ms=?2,
        revision=revision+1 WHERE request_id=?1",
        params![id, now],
    )?;
    required(tx, id)
}

/// Idempotently account one request's actual usage. The caller must append/project the
/// matching native usage receipt in this same SQLite transaction. A partial receipt never
/// frees the hold; complete actuals can exceed estimates and remain visible as overruns.
pub fn settle(
    tx: &Transaction<'_>,
    id: &str,
    receipt_id: &str,
    actual: &Receipt,
    now: i64,
) -> Result<Settlement, LedgerError> {
    text(receipt_id, "receipt_event_id", 512)?;
    validate_receipt(actual)?;
    let record = required(tx, id)?;
    if let Some((owner, prior)) = tx
        .query_row(
            "SELECT request_id,actual FROM budget_reservation_receipt WHERE receipt_event_id=?1",
            [receipt_id],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
        )
        .optional()?
    {
        if owner != id || decode::<Receipt>(&prior)? != *actual {
            return Err(LedgerError::Conflict(receipt_id.into()));
        }
        return Ok(Settlement {
            record,
            delta: Receipt::default(),
            applied: false,
        });
    }
    if record.state == State::Settled && !actual.complete {
        return Err(LedgerError::Transition(id.into()));
    }
    let delta = actual.delta_since(record.actual.as_ref())?;
    let mut retained = actual.clone();
    if let Some(previous) = &record.actual {
        retained.tokens = retained.tokens.or(previous.tokens);
        retained.cost_microusd = retained.cost_microusd.or(previous.cost_microusd);
    }
    let requires_cost = record
        .scopes
        .iter()
        .any(|scope| scope.max_cost_microusd.is_some());
    let complete = actual.complete
        && actual.tokens.is_some()
        && (!requires_cost || actual.cost_microusd.is_some());
    if record.state == State::Settled && !complete {
        return Err(LedgerError::Transition(id.into()));
    }
    let state = if complete {
        "settled"
    } else if record.state == State::Released {
        "released"
    } else {
        "uncertain"
    };
    tx.execute("INSERT INTO budget_reservation_receipt(receipt_event_id,request_id,actual,received_ms) VALUES (?1,?2,?3,?4)",
        params![receipt_id,id,encode(actual)?,now])?;
    tx.execute("UPDATE budget_reservation SET state=?2,actual_tokens=?3,actual_turns=?4,actual_cost_microusd=?5,
        usage_complete=?6,receipt_event_id=?7,updated_ms=?8,settled_ms=CASE WHEN ?6 THEN ?8 ELSE settled_ms END,
        revision=revision+1 WHERE request_id=?1",params![id,state,retained.tokens.as_ref().map(encode).transpose()?,
        sql(retained.turns)?,retained.cost_microusd.map(sql).transpose()?,complete,receipt_id,now])?;
    Ok(Settlement {
        record: required(tx, id)?,
        delta,
        applied: true,
    })
}

/// Owner-only reconciliation, fenced by the inspected revision and idempotent operation
/// identity. A late provider receipt may settle a released allocation; its release audit
/// and timestamp remain permanently visible.
pub fn reconcile(
    tx: &Transaction<'_>,
    id: &str,
    command: &Reconciliation,
    now: i64,
) -> Result<Reconciled, LedgerError> {
    text(&command.operation_id, "operation_id", 512)?;
    text(&command.owner, "owner", 512)?;
    text(&command.reason, "reason", 2048)?;
    let encoded = encode(&command.action)?;
    let prior=tx.query_row("SELECT request_id,expected_revision,owner,reason,action FROM budget_reservation_reconciliation WHERE operation_id=?1",
        [&command.operation_id],|r|Ok((r.get::<_,String>(0)?,integer(r,1)?,r.get::<_,String>(2)?,r.get::<_,String>(3)?,r.get::<_,String>(4)?))).optional()?;
    if let Some((request, revision, owner, reason, action)) = prior {
        if request != id
            || revision != command.expected_revision
            || owner != command.owner
            || reason != command.reason
            || action != encoded
        {
            return Err(LedgerError::Conflict(command.operation_id.clone()));
        }
        return Ok(Reconciled {
            record: required(tx, id)?,
            delta: None,
            applied: false,
        });
    }
    let record = required(tx, id)?;
    if record.revision != command.expected_revision {
        return Err(LedgerError::Stale {
            request_id: id.into(),
            expected: command.expected_revision,
            actual: record.revision,
        });
    }
    let delta = match &command.action {
        ReconcileAction::Release => {
            if !matches!(record.state, State::Reserved | State::Uncertain) {
                return Err(LedgerError::Transition(id.into()));
            }
            tx.execute("UPDATE budget_reservation SET state='released',released_ms=?2,updated_ms=?2,revision=revision+1 WHERE request_id=?1",params![id,now])?;
            None
        }
        ReconcileAction::Settle {
            receipt_event_id,
            actual,
        } => {
            if !actual.complete {
                return Err(invalid("owner settlement must supply a complete receipt"));
            }
            let settled = settle(tx, id, receipt_event_id, actual, now)?;
            if settled.record.state != State::Settled {
                return Err(invalid("owner settlement lacks required actual cost"));
            }
            settled.applied.then_some(settled.delta)
        }
    };
    tx.execute("INSERT INTO budget_reservation_reconciliation(operation_id,request_id,expected_revision,owner,reason,action,reconciled_ms)
        VALUES (?1,?2,?3,?4,?5,?6,?7)",params![command.operation_id,id,sql(command.expected_revision)?,command.owner,command.reason,encoded,now])?;
    Ok(Reconciled {
        record: required(tx, id)?,
        delta,
        applied: true,
    })
}
