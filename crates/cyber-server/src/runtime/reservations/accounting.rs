use cyber_llm::{
    LlmEvent, Usage,
    catalog::{Cost, compute_cost},
};
use cyber_store::reservations::{self as ledger, Receipt, Request, TokenAmounts};
use cyber_store::{Expected, StoreError};
use serde_json::json;
use std::sync::{Arc, Mutex, PoisonError, Weak};

use crate::runtime::{
    Handle, Inner, RuntimeError,
    events::{AUXILIARY_USAGE, AuxiliaryUsage, event},
};

#[derive(Clone, Default)]
struct Observation {
    usage: Usage,
    seen: bool,
    finished: bool,
    ended: bool,
    failed: bool,
}

pub(in crate::runtime) struct Attempt {
    pub request_id: String,
    inner: Weak<Inner>,
    handle: Arc<Handle>,
    request: Request,
    price: Option<Cost>,
    failure: Arc<Mutex<Option<RuntimeError>>>,
    observed: Mutex<Observation>,
    committed: tokio::sync::Mutex<Option<Receipt>>,
    started: std::time::Instant,
    dispatched: std::sync::atomic::AtomicBool,
    pub deadline: Option<tokio::time::Instant>,
}

impl Attempt {
    pub fn new(
        inner: Weak<Inner>,
        handle: Arc<Handle>,
        request: Request,
        price: Option<Cost>,
        failure: Arc<Mutex<Option<RuntimeError>>>,
    ) -> Self {
        let deadline = request.deadline_ms.map(|at| {
            let now = tokio::time::Instant::now();
            now.checked_add(std::time::Duration::from_millis(
                at.saturating_sub(chrono::Utc::now().timestamp_millis())
                    .max(0) as u64,
            ))
            .unwrap_or(now)
        });
        Self {
            request_id: request.request_id.clone(),
            inner,
            handle,
            request,
            price,
            failure,
            observed: Mutex::default(),
            committed: tokio::sync::Mutex::default(),
            started: std::time::Instant::now(),
            dispatched: std::sync::atomic::AtomicBool::new(false),
            deadline,
        }
    }
    pub fn mark_dispatched(self: &Arc<Self>) -> Result<(), RuntimeError> {
        let inner = self.inner.upgrade().ok_or(RuntimeError::ShuttingDown)?;
        let mut active = inner
            .reservation_attempts
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        active.retain(|_, attempt| attempt.strong_count() > 0);
        active.insert(self.request_id.clone(), Arc::downgrade(self));
        let id = self.request_id.clone();
        if let Err(error) = inner.store.transaction(move |tx| {
            ledger::mark_dispatched(tx, &id, chrono::Utc::now().timestamp_millis())
                .map_err(StoreError::from)
        }) {
            active.remove(&self.request_id);
            let id = self.request_id.clone();
            // The provider future has not been polled. Release only if the durable
            // row still confirms predispatch; storage failures retain the safe hold.
            let _ = inner.store.transaction(move |tx| {
                if ledger::get(tx, &id)
                    .map_err(StoreError::from)?
                    .is_some_and(|r| r.state == ledger::State::Reserved)
                {
                    ledger::release_before_dispatch(tx, &id, chrono::Utc::now().timestamp_millis())
                        .map_err(StoreError::from)?;
                }
                Ok(())
            });
            return Err(super::error(error));
        }
        self.dispatched
            .store(true, std::sync::atomic::Ordering::Release);
        Ok(())
    }
    pub fn observe(&self, event: &LlmEvent) -> Result<(), String> {
        let mut observation = self.observed.lock().unwrap_or_else(PoisonError::into_inner);
        match event {
            LlmEvent::Usage(value) => {
                observation.usage = checked_usage(observation.usage, *value)?;
                observation.seen = true;
            }
            LlmEvent::Finish { .. } => observation.finished = true,
            _ => {}
        }
        Ok(())
    }
    pub fn ended(&self, transport_complete: bool) {
        let mut observed = self.observed.lock().unwrap_or_else(PoisonError::into_inner);
        observed.ended = true;
        observed.failed |= !transport_complete;
    }
    pub fn failed(&self, error: RuntimeError) {
        *self.failure.lock().unwrap_or_else(PoisonError::into_inner) = Some(error);
    }
    pub fn wall_error(&self) -> RuntimeError {
        RuntimeError::BudgetExceeded {
            scope: format!("request {} (applicable wall deadline)", self.request_id),
            limit: "max_wall_seconds".into(),
        }
    }

    pub async fn flush(&self) -> Result<(), RuntimeError> {
        if !self.dispatched.load(std::sync::atomic::Ordering::Acquire) {
            return Ok(());
        }
        let mut committed = self.committed.lock().await;
        let observed = self
            .observed
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        let complete = observed.ended && observed.finished && observed.seen && !observed.failed;
        let receipt = Receipt {
            tokens: observed.seen.then(|| amounts(observed.usage)),
            turns: u64::from(self.request.purpose == "turn" && complete),
            cost_microusd: if observed.seen {
                compute_cost(self.price.as_ref(), &observed.usage)
                    .map(super::money)
                    .transpose()
                    .map_err(super::error)?
            } else {
                None
            },
            complete,
        };
        if committed.as_ref() == Some(&receipt) {
            return Ok(());
        }
        let inner = self.inner.upgrade().ok_or(RuntimeError::ShuttingDown)?;
        let mut state = self.handle.state.lock().await;
        let aggregate = state.info.id.clone();
        let request = self.request.clone();
        let actual = receipt.clone();
        let now = chrono::Utc::now().timestamp_millis();
        let receipt_id = cyber_core::ids::new_id("brec");
        let duration_ms = u64::try_from(self.started.elapsed().as_millis()).unwrap_or(u64::MAX);
        let (stored, ()) = inner
            .store
            .append_checked(&aggregate, Expected::Seq(state.last_seq), move |tx| {
                let previous = ledger::get(tx, &request.request_id)
                    .map_err(StoreError::from)?
                    .and_then(|r| r.actual);
                let settlement = ledger::settle(tx, &request.request_id, &receipt_id, &actual, now)
                    .map_err(StoreError::from)?;
                if !settlement.applied {
                    return Ok((Vec::new(), ()));
                }
                Ok((
                    billing(
                        tx,
                        &request,
                        previous.as_ref(),
                        &actual,
                        &settlement.delta,
                        &receipt_id,
                        duration_ms,
                    )?
                    .into_iter()
                    .collect(),
                    (),
                ))
            })
            .map_err(super::error)?;
        for e in &stored {
            state.apply(e).map_err(RuntimeError::Corrupt)?;
        }
        drop(state);
        inner.publish(&stored);
        *committed = Some(receipt);
        Ok(())
    }
}

pub(super) fn billing(
    tx: &rusqlite::Transaction<'_>,
    request: &Request,
    previous: Option<&Receipt>,
    actual: &Receipt,
    delta: &Receipt,
    receipt_id: &str,
    duration_ms: u64,
) -> Result<Option<cyber_store::NewEvent>, StoreError> {
    if delta.tokens.is_none() && delta.cost_microusd.is_none() {
        return Ok(None);
    }
    let bill = AuxiliaryUsage {
        provider: request.provider.clone(),
        model: request.model.clone(),
        purpose: request.purpose.clone(),
        call_id: request.call_id.clone(),
        duration_ms,
        usage: usage(delta.tokens.unwrap_or_default()),
        cost: delta.cost_microusd.map(|v| v as f64 / 1_000_000.0),
    };
    let mut event = event(AUXILIARY_USAGE, &bill);
    event.causation_id = Some(receipt_id.into());
    event.data["reservation_id"] = request.request_id.clone().into();
    event.data["reservation_receipt_id"] = receipt_id.into();
    event.data["reservation_actual"] = json!(actual);
    let other_turn = if let Some(call) = &request.call_id {
        tx.query_row("SELECT EXISTS(SELECT 1 FROM budget_reservation WHERE session_id=?1 AND call_id=?2 AND purpose='turn' AND request_id!=?3 AND actual_turns>0)",rusqlite::params![request.session_id,call,request.request_id],|row|row.get::<_,bool>(0))?
    } else {
        false
    };
    event.data["reservation_turns_increment"] = json!(if other_turn { 0 } else { delta.turns });
    event.data["failed"] = (!actual.complete).into();
    event.data["reservation_unpriced_increment"] = json!(unpriced_delta(previous, actual));
    Ok(Some(event))
}

fn unpriced_delta(previous: Option<&Receipt>, actual: &Receipt) -> i64 {
    let old = previous.is_some_and(|r| r.tokens.is_some() && r.cost_microusd.is_none());
    let new = actual.tokens.is_some() && actual.cost_microusd.is_none();
    i64::from(new) - i64::from(old)
}

pub(super) fn amounts(value: Usage) -> TokenAmounts {
    TokenAmounts {
        input: value.input,
        output: value.output,
        reasoning: value.reasoning,
        cache_read: value.cache_read,
        cache_write: value.cache_write,
    }
}
pub(super) fn usage(value: TokenAmounts) -> Usage {
    Usage {
        input: value.input,
        output: value.output,
        reasoning: value.reasoning,
        cache_read: value.cache_read,
        cache_write: value.cache_write,
    }
}

fn checked_usage(a: Usage, b: Usage) -> Result<Usage, String> {
    let add = |x: u64, y: u64| {
        x.checked_add(y)
            .ok_or_else(|| "reported token usage overflow".to_owned())
    };
    Ok(Usage {
        input: add(a.input, b.input)?,
        output: add(a.output, b.output)?,
        reasoning: add(a.reasoning, b.reasoning)?,
        cache_read: add(a.cache_read, b.cache_read)?,
        cache_write: add(a.cache_write, b.cache_write)?,
    })
}
