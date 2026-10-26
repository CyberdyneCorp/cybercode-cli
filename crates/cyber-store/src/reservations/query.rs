use super::{
    Audit, Cursor, Estimate, Filter, LedgerError, Page, Receipt, Reconciliation, Record, Request,
    Scope, State, decode, integer, invalid, optional_integer,
};
use rusqlite::{Connection, OptionalExtension, params};

struct RawRecord {
    request_id: String,
    session_id: String,
    provider: String,
    model: String,
    purpose: String,
    call_id: Option<String>,
    bound_source: Option<String>,
    normalized_sha256: Option<String>,
    deadline_ms: Option<i64>,
    state: String,
    revision: u64,
    estimated: String,
    token_upper: u64,
    turns_upper: u64,
    cost_upper: Option<u64>,
    actual_tokens: Option<String>,
    actual_turns: Option<u64>,
    actual_cost: Option<u64>,
    complete: bool,
    receipt: Option<String>,
    created: i64,
    dispatched: Option<i64>,
    updated: i64,
    settled: Option<i64>,
    released: Option<i64>,
}

/// Inspect an allocation without loading a transcript or any request body/credential.
pub fn get(conn: &Connection, id: &str) -> Result<Option<Record>, LedgerError> {
    let raw=conn.query_row("SELECT request_id,session_id,provider,model,purpose,state,revision,estimated_tokens,
        token_upper,turns_upper,cost_upper_microusd,actual_tokens,actual_turns,actual_cost_microusd,usage_complete,
        receipt_event_id,created_ms,dispatched_ms,updated_ms,settled_ms,released_ms,bound_source,normalized_sha256,call_id,deadline_ms FROM budget_reservation WHERE request_id=?1",[id],|r|Ok(RawRecord{
        request_id:r.get(0)?,session_id:r.get(1)?,provider:r.get(2)?,model:r.get(3)?,purpose:r.get(4)?,state:r.get(5)?,
        revision:integer(r,6)?,estimated:r.get(7)?,token_upper:integer(r,8)?,turns_upper:integer(r,9)?,cost_upper:optional_integer(r,10)?,
        actual_tokens:r.get(11)?,actual_turns:optional_integer(r,12)?,actual_cost:optional_integer(r,13)?,complete:r.get(14)?,receipt:r.get(15)?,
        created:r.get(16)?,dispatched:r.get(17)?,updated:r.get(18)?,settled:r.get(19)?,released:r.get(20)?,bound_source:r.get(21)?,normalized_sha256:r.get(22)?,call_id:r.get(23)?,deadline_ms:r.get(24)?})).optional()?;
    raw.map(|raw| record(conn, raw)).transpose()
}

fn record(conn: &Connection, raw: RawRecord) -> Result<Record, LedgerError> {
    let scopes = scopes(conn, &raw.request_id)?;
    let actual = raw
        .receipt
        .as_ref()
        .map(|_| {
            Ok::<_, LedgerError>(Receipt {
                tokens: raw.actual_tokens.as_deref().map(decode).transpose()?,
                turns: raw.actual_turns.unwrap_or(0),
                cost_microusd: raw.actual_cost,
                complete: raw.complete,
            })
        })
        .transpose()?;
    let overrun_tokens = actual
        .as_ref()
        .and_then(|a| a.tokens)
        .map(|t| t.total())
        .transpose()?
        .unwrap_or(0)
        .saturating_sub(raw.token_upper);
    let overrun_cost_microusd = actual
        .as_ref()
        .and_then(|a| a.cost_microusd)
        .zip(raw.cost_upper)
        .map(|(a, b)| a.saturating_sub(b));
    Ok(Record {
        request: Request {
            request_id: raw.request_id,
            session_id: raw.session_id,
            provider: raw.provider,
            model: raw.model,
            purpose: raw.purpose,
            call_id: raw.call_id,
            bound_source: raw.bound_source,
            normalized_sha256: raw.normalized_sha256,
            deadline_ms: raw.deadline_ms,
            estimate: Estimate {
                tokens: decode(&raw.estimated)?,
                total_tokens: raw.token_upper,
                turns: raw.turns_upper,
                cost_microusd: raw.cost_upper,
            },
        },
        state: State::parse(&raw.state)?,
        revision: raw.revision,
        created_ms: raw.created,
        dispatched_ms: raw.dispatched,
        updated_ms: raw.updated,
        settled_ms: raw.settled,
        released_ms: raw.released,
        actual,
        receipt_event_id: raw.receipt,
        overrun_tokens,
        overrun_cost_microusd,
        scopes,
    })
}

fn scopes(conn: &Connection, id: &str) -> Result<Vec<Scope>, LedgerError> {
    let mut q=conn.prepare("SELECT scope_id,max_tokens,max_turns,max_cost_microusd,spent_tokens,spent_turns,
        spent_cost_microusd FROM budget_reservation_allocation WHERE request_id=?1 ORDER BY scope_id")?;
    Ok(q.query_map([id], |r| {
        Ok(Scope {
            id: r.get(0)?,
            max_tokens: optional_integer(r, 1)?,
            max_turns: optional_integer(r, 2)?,
            max_cost_microusd: optional_integer(r, 3)?,
            spent_tokens: integer(r, 4)?,
            spent_turns: integer(r, 5)?,
            spent_cost_microusd: integer(r, 6)?,
            uncertain: false,
        })
    })?
    .collect::<Result<_, _>>()?)
}

pub(super) fn required(conn: &Connection, id: &str) -> Result<Record, LedgerError> {
    get(conn, id)?.ok_or_else(|| LedgerError::NotFound(id.into()))
}

/// Oldest first, so unresolved historical allocations remain discoverable. The cursor
/// and bounded page are independent of deleted Session/transcript records.
pub fn list(conn: &Connection, filter: &Filter) -> Result<Page, LedgerError> {
    if !(1..=500).contains(&filter.limit) {
        return Err(invalid("limit must be 1..=500"));
    }
    let after = filter.after.as_ref();
    let mut q=conn.prepare("SELECT request_id FROM budget_reservation r
        WHERE (?1 IS NULL OR session_id=?1) AND (?2 IS NULL OR EXISTS(SELECT 1 FROM budget_reservation_allocation a WHERE a.request_id=r.request_id AND a.scope_id=?2))
        AND (NOT ?3 OR state IN ('reserved','uncertain'))
        AND (?4 IS NULL OR created_ms>?4 OR (created_ms=?4 AND request_id>?5))
        ORDER BY created_ms,request_id LIMIT ?6")?;
    let mut ids = q
        .query_map(
            params![
                filter.session_id,
                filter.scope_id,
                filter.open_only,
                after.map(|a| a.created_ms),
                after.map(|a| a.request_id.as_str()),
                i64::from(filter.limit) + 1
            ],
            |r| r.get::<_, String>(0),
        )?
        .collect::<Result<Vec<_>, _>>()?;
    let more = ids.len() > filter.limit as usize;
    ids.truncate(filter.limit as usize);
    let records = ids
        .into_iter()
        .map(|id| required(conn, &id))
        .collect::<Result<Vec<_>, _>>()?;
    let next = more
        .then(|| {
            records.last().map(|r| Cursor {
                created_ms: r.created_ms,
                request_id: r.request.request_id.clone(),
            })
        })
        .flatten();
    Ok(Page { records, next })
}

pub fn audit(conn: &Connection, id: &str, limit: u32) -> Result<Vec<Audit>, LedgerError> {
    if !(1..=500).contains(&limit) {
        return Err(invalid("limit must be 1..=500"));
    }
    let mut q=conn.prepare("SELECT operation_id,expected_revision,owner,reason,action,reconciled_ms FROM budget_reservation_reconciliation
        WHERE request_id=?1 ORDER BY reconciled_ms DESC,operation_id DESC LIMIT ?2")?;
    let rows = q
        .query_map(params![id, i64::from(limit)], |r| {
            Ok((
                r.get::<_, String>(0)?,
                integer(r, 1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, i64>(5)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    rows.into_iter()
        .map(
            |(operation_id, expected_revision, owner, reason, action, reconciled_ms)| {
                Ok(Audit {
                    request_id: id.into(),
                    reconciliation: Reconciliation {
                        operation_id,
                        expected_revision,
                        owner,
                        reason,
                        action: decode(&action)?,
                    },
                    reconciled_ms,
                })
            },
        )
        .collect()
}
