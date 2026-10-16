//! The usage step ledger (`observability-costs` → Usage step ledger): one `usage_step` row
//! per provider step and per auxiliary model call, projected in the same transaction as
//! its event. Events stored before the ledger existed are projected by a resumable,
//! batched backfill, and rows older than the retention window fold into `usage_daily`.

use std::sync::PoisonError;

use cyber_store::StoredEvent;
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde_json::Value;

use super::events::{
    AUTO_DECIDED, AUXILIARY_USAGE, COMPACTION_COMPLETED, STEP_ENDED, STEP_FAILED, STEP_STARTED,
    TITLE_GENERATED,
};
use super::{Runtime, RuntimeError};

/// Events projected per backfill transaction.
pub(crate) const BACKFILL_BATCH: i64 = 5_000;
/// The `data_migration` row recorded when the backfill finishes.
const BACKFILL_DONE: &str = "usage_ledger";
/// Event types that can produce a ledger row.
const KINDS: [&str; 6] = [
    STEP_ENDED,
    STEP_FAILED,
    COMPACTION_COMPLETED,
    AUXILIARY_USAGE,
    TITLE_GENERATED,
    AUTO_DECIDED,
];

/// What the ledger needs of the Session an event belongs to.
struct Owner {
    root: String,
    parent: Option<String>,
    run: Option<String>,
    agent: String,
    model: String,
    directory: String,
}

/// The model call an event records.
struct Call {
    purpose: String,
    failed: bool,
    provider: String,
    model: String,
    agent: Option<String>,
    started_ms: i64,
}

/// Write the ledger row of `e`, if it records a model call. Idempotent per event.
pub(super) fn project(tx: &Connection, e: &StoredEvent) -> rusqlite::Result<()> {
    let Some(owner) = owner(tx, &e.aggregate_id)? else {
        return Ok(());
    };
    let Some(call) = call(tx, e, &owner)? else {
        return Ok(());
    };
    let usage = &e.data["usage"];
    let n = |key: &str| usage[key].as_i64().unwrap_or(0).max(0);
    let cost = if call.failed {
        Some(0.0)
    } else {
        e.data["cost"].as_f64()
    };
    tx.execute(
        "INSERT INTO usage_step (event_id, session_id, root_id, parent_id, workflow_run_id, agent,
            purpose, failed, provider, model, started_ms, ended_ms, input_tokens, output_tokens,
            reasoning_tokens, cache_read_tokens, cache_write_tokens, prompt_tokens, cost, directory)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20)
         ON CONFLICT(event_id) DO NOTHING",
        params![
            e.id,
            e.aggregate_id,
            owner.root,
            owner.parent,
            owner.run,
            call.agent.unwrap_or(owner.agent),
            call.purpose,
            call.failed,
            call.provider,
            call.model,
            call.started_ms.min(e.time_ms),
            e.time_ms,
            n("input"),
            n("output"),
            n("reasoning"),
            n("cache_read"),
            n("cache_write"),
            n("input")
                .saturating_add(n("cache_read"))
                .saturating_add(n("cache_write")),
            cost,
            owner.directory,
        ],
    )?;
    Ok(())
}

fn owner(tx: &Connection, id: &str) -> rusqlite::Result<Option<Owner>> {
    tx.query_row(
        "WITH RECURSIVE up(id, parent_id, depth) AS (
            SELECT id, parent_id, 0 FROM session WHERE id = ?1
            UNION ALL SELECT s.id, s.parent_id, up.depth + 1 FROM up JOIN session s ON s.id = up.parent_id
            WHERE up.depth < 256 AND s.id != ?1
        ) SELECT (SELECT id FROM up ORDER BY depth DESC LIMIT 1), parent_id, workflow_run_id,
            agent, model, directory FROM session WHERE id = ?1",
        [id],
        |r| {
            Ok(Owner {
                root: r.get(0)?,
                parent: r.get(1)?,
                run: r.get(2)?,
                agent: r.get(3)?,
                model: r.get(4)?,
                directory: r.get(5)?,
            })
        },
    )
    .optional()
}

fn call(tx: &Connection, e: &StoredEvent, owner: &Owner) -> rusqlite::Result<Option<Call>> {
    let d = &e.data;
    let (purpose, reference, provider, duration) = match e.kind.as_str() {
        STEP_ENDED | STEP_FAILED => return step(tx, e, owner),
        AUXILIARY_USAGE => (
            d["purpose"].as_str().unwrap_or("auxiliary"),
            d["model"].as_str(),
            d["provider"].as_str(),
            d["duration_ms"].as_i64().unwrap_or(0),
        ),
        TITLE_GENERATED if d["usage"].is_object() => ("title", d["model"].as_str(), None, 0),
        COMPACTION_COMPLETED => ("compaction", d["model"].as_str(), None, 0),
        AUTO_DECIDED if d["usage"].is_object() => (
            "evaluator",
            d["model"].as_str().or(d["evaluator"].as_str()),
            None,
            0,
        ),
        _ => return Ok(None),
    };
    let (provider, model) = split(provider, reference.unwrap_or(&owner.model));
    Ok(Some(Call {
        purpose: purpose.into(),
        failed: false,
        provider,
        model,
        agent: None,
        started_ms: e.time_ms.saturating_sub(duration.max(0)),
    }))
}

/// A provider step: its provider, model and agent come from `session.step.started.1`.
/// A failure without a started step made no provider call and records nothing.
fn step(tx: &Connection, e: &StoredEvent, owner: &Owner) -> rusqlite::Result<Option<Call>> {
    let message = e.data["message_id"].as_str().unwrap_or_default();
    let started: Option<(i64, String)> = tx
        .query_row(
            "SELECT time, data FROM event WHERE aggregate_id = ?1 AND seq < ?2 AND type = ?3
                AND json_extract(data, '$.message_id') = ?4 ORDER BY seq DESC LIMIT 1",
            params![e.aggregate_id, e.seq, STEP_STARTED, message],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    let failed = e.kind == STEP_FAILED;
    let Some((time, data)) = started else {
        if failed {
            return Ok(None);
        }
        let (provider, model) = split(None, &owner.model);
        return Ok(Some(Call {
            purpose: "turn".into(),
            failed,
            provider,
            model,
            agent: None,
            started_ms: e.time_ms,
        }));
    };
    let data: Value = serde_json::from_str(&data).unwrap_or_default();
    let (provider, model) = match (data["provider"].as_str(), data["model"].as_str()) {
        (Some(provider), Some(model)) => split(Some(provider), model),
        _ => split(None, &owner.model),
    };
    Ok(Some(Call {
        purpose: "turn".into(),
        failed,
        provider,
        model,
        agent: data["agent"].as_str().map(str::to_owned),
        started_ms: time,
    }))
}

/// `(provider, model)` of a model reference such as `anthropic/claude-opus-5-5`.
fn split(provider: Option<&str>, reference: &str) -> (String, String) {
    match provider {
        Some(provider) => {
            let model = reference
                .strip_prefix(provider)
                .and_then(|rest| rest.strip_prefix('/'))
                .unwrap_or(reference);
            (provider.into(), model.into())
        }
        None => match reference.split_once('/') {
            Some((provider, model)) => (provider.into(), model.into()),
            None => (String::new(), reference.into()),
        },
    }
}

/// Project one batch of the stored events the ledger has not seen; `true` when done.
fn backfill_batch(tx: &Transaction<'_>, now_ms: i64) -> rusqlite::Result<bool> {
    let Some((cursor, target)) = tx
        .query_row(
            "SELECT cursor, target FROM usage_backfill WHERE id = 1",
            [],
            |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)),
        )
        .optional()?
    else {
        return Ok(true);
    };
    // Every event counts toward the batch; only ledger kinds carry their payload.
    let mut statement = tx.prepare(
        "SELECT rowid, id, aggregate_id, seq, type,
            CASE WHEN type IN (?4, ?5, ?6, ?7, ?8, ?9) THEN data END, time, causation_id
         FROM event WHERE rowid > ?1 AND rowid <= ?2 ORDER BY rowid LIMIT ?3",
    )?;
    let rows = statement
        .query_map(
            params![
                cursor,
                target,
                BACKFILL_BATCH,
                KINDS[0],
                KINDS[1],
                KINDS[2],
                KINDS[3],
                KINDS[4],
                KINDS[5]
            ],
            |r| {
                let rowid = r.get::<_, i64>(0)?;
                let Some(data) = r.get::<_, Option<String>>(5)? else {
                    return Ok((rowid, None));
                };
                let event = StoredEvent {
                    id: r.get(1)?,
                    aggregate_id: r.get(2)?,
                    seq: r.get(3)?,
                    kind: r.get(4)?,
                    data: serde_json::from_str(&data).unwrap_or_default(),
                    time_ms: r.get(6)?,
                    causation_id: r.get(7)?,
                };
                Ok((rowid, Some(event)))
            },
        )?
        .collect::<Result<Vec<_>, _>>()?;
    drop(statement);
    for event in rows.iter().filter_map(|(_, event)| event.as_ref()) {
        project(tx, event)?;
    }
    match rows.last() {
        Some((rowid, _)) if rows.len() as i64 == BACKFILL_BATCH => {
            tx.execute(
                "UPDATE usage_backfill SET cursor = ?1 WHERE id = 1",
                [rowid],
            )?;
            Ok(false)
        }
        _ => {
            tx.execute("DELETE FROM usage_backfill", [])?;
            tx.execute(
                "INSERT INTO data_migration (id, time_completed) VALUES (?1, ?2)
                 ON CONFLICT(id) DO NOTHING",
                params![BACKFILL_DONE, now_ms / 1000],
            )?;
            Ok(true)
        }
    }
}

/// Project the events of `session_id` the running backfill has not reached yet, so a
/// Session deleted mid-backfill keeps its ledger rows. Call before its events and its
/// `session` row are deleted; nothing happens when no backfill is pending.
pub(super) fn project_unreached(tx: &Connection, session_id: &str) -> rusqlite::Result<()> {
    let Some((cursor, target)) = tx
        .query_row(
            "SELECT cursor, target FROM usage_backfill WHERE id = 1",
            [],
            |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)),
        )
        .optional()?
    else {
        return Ok(());
    };
    let mut statement = tx.prepare(
        "SELECT id, aggregate_id, seq, type, data, time, causation_id FROM event
         WHERE aggregate_id = ?1 AND rowid > ?2 AND rowid <= ?3
            AND type IN (?4, ?5, ?6, ?7, ?8, ?9)
         ORDER BY rowid",
    )?;
    let events = statement
        .query_map(
            params![
                session_id, cursor, target, KINDS[0], KINDS[1], KINDS[2], KINDS[3], KINDS[4],
                KINDS[5]
            ],
            |r| {
                Ok(StoredEvent {
                    id: r.get(0)?,
                    aggregate_id: r.get(1)?,
                    seq: r.get(2)?,
                    kind: r.get(3)?,
                    data: serde_json::from_str(&r.get::<_, String>(4)?).unwrap_or_default(),
                    time_ms: r.get(5)?,
                    causation_id: r.get(6)?,
                })
            },
        )?
        .collect::<Result<Vec<_>, _>>()?;
    drop(statement);
    events.iter().try_for_each(|event| project(tx, event))
}

/// Fold rows that ended before `before_ms` into `usage_daily` by UTC day.
fn fold(tx: &Transaction<'_>, before_ms: i64) -> rusqlite::Result<usize> {
    tx.execute(
        "INSERT INTO usage_daily (day, root_id, agent, provider, model, directory, steps, turns,
            input_tokens, output_tokens, reasoning_tokens, cache_read_tokens, cache_write_tokens,
            cost, unpriced_steps, max_prompt_tokens, api_duration_ms)
         SELECT strftime('%Y-%m-%d', ended_ms / 1000, 'unixepoch'), root_id, agent, provider, model,
            MAX(directory), SUM(failed = 0), SUM(purpose = 'turn' AND failed = 0),
            SUM(input_tokens), SUM(output_tokens), SUM(reasoning_tokens), SUM(cache_read_tokens),
            SUM(cache_write_tokens), TOTAL(cost), SUM(cost IS NULL), MAX(prompt_tokens),
            SUM(ended_ms - started_ms)
         FROM usage_step WHERE ended_ms < ?1 GROUP BY 1, 2, 3, 4, 5
         ON CONFLICT(day, root_id, agent, provider, model) DO UPDATE SET
            steps = steps + excluded.steps, turns = turns + excluded.turns,
            input_tokens = input_tokens + excluded.input_tokens,
            output_tokens = output_tokens + excluded.output_tokens,
            reasoning_tokens = reasoning_tokens + excluded.reasoning_tokens,
            cache_read_tokens = cache_read_tokens + excluded.cache_read_tokens,
            cache_write_tokens = cache_write_tokens + excluded.cache_write_tokens,
            cost = cost + excluded.cost, unpriced_steps = unpriced_steps + excluded.unpriced_steps,
            max_prompt_tokens = MAX(max_prompt_tokens, excluded.max_prompt_tokens),
            api_duration_ms = api_duration_ms + excluded.api_duration_ms",
        [before_ms],
    )?;
    tx.execute("DELETE FROM usage_step WHERE ended_ms < ?1", [before_ms])
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

impl Runtime {
    /// Whether stored events are still being projected into the ledger.
    pub fn usage_backfilling(&self) -> Result<bool, RuntimeError> {
        Ok(self.inner.store.read(|db| {
            Ok(
                db.query_row("SELECT EXISTS(SELECT 1 FROM usage_backfill)", [], |r| {
                    r.get(0)
                })?,
            )
        })?)
    }

    /// Project one batch of the backfill; `true` once it has finished.
    pub fn backfill_usage_batch(&self) -> Result<bool, RuntimeError> {
        let now = now_ms();
        Ok(self
            .inner
            .store
            .transaction(move |tx| Ok(backfill_batch(tx, now)?))?)
    }

    /// Run the backfill in the background, one writer transaction per batch, so live
    /// commits interleave with it. A restart resumes from the stored cursor.
    pub fn spawn_usage_backfill(&self) {
        if !self.usage_backfilling().unwrap_or(false) {
            return;
        }
        let weak = self.downgrade();
        let task = tokio::spawn(async move {
            loop {
                let Some(runtime) = weak.upgrade() else {
                    return;
                };
                let batch = tokio::task::spawn_blocking(move || runtime.backfill_usage_batch());
                match batch.await {
                    Ok(Ok(false)) => tokio::task::yield_now().await,
                    Ok(Ok(true)) => return,
                    Ok(Err(error)) => {
                        cyber_core::log::warn(
                            "usage",
                            "backfill",
                            serde_json::json!({ "error": error.to_string() }),
                        );
                        return;
                    }
                    Err(_) => return,
                }
            }
        });
        let mut owners = self
            .inner
            .background
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        owners.retain(|owner| !owner.is_finished());
        owners.push(task);
    }

    /// Fold ledger rows that ended before `before_ms` into daily aggregates
    /// (`observability-costs` → Usage data retention); returns the rows folded.
    pub fn fold_usage(&self, before_ms: i64) -> Result<usize, RuntimeError> {
        Ok(self
            .inner
            .store
            .transaction(move |tx| Ok(fold(tx, before_ms)?))?)
    }
}

#[cfg(test)]
mod tests {
    use super::split;

    #[test]
    fn references_split_into_provider_and_model() {
        assert_eq!(
            split(Some("anthropic"), "anthropic/claude-opus-5-5"),
            ("anthropic".into(), "claude-opus-5-5".into())
        );
        assert_eq!(split(Some("test"), "main"), ("test".into(), "main".into()));
        assert_eq!(
            split(None, "ollama/qwen3:8b"),
            ("ollama".into(), "qwen3:8b".into())
        );
        assert_eq!(split(None, "bare"), (String::new(), "bare".into()));
    }
}
