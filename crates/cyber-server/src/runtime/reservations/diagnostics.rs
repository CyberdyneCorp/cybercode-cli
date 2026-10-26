//! Owner diagnostics contain bounds, usage and audit reasons, never prompt bodies or credentials.
use cyber_core::budget::{Budget, Enforcement};
use cyber_llm::Usage;
use cyber_store::{EventRegistry, Expected, NewEvent, StoreError, reservations as ledger};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::runtime::{Runtime, RuntimeError};

pub(super) const RECONCILED: &str = "budget.reservation.reconciled.1";
pub(super) fn register(registry: &mut EventRegistry) {
    registry
        .register(RECONCILED)
        .expect("valid reservation audit event");
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct ReservationEstimate {
    pub class_upper: Usage,
    pub total_tokens_upper: u64,
    pub turns_upper: u64,
    pub cost_usd_upper: Option<f64>,
}
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReservationActual {
    pub tokens: Option<Usage>,
    #[serde(default)]
    pub turns: u64,
    pub cost_usd: Option<f64>,
    pub complete: bool,
}
#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct ReservationScope {
    pub id: String,
    pub captured_budget: Budget,
    pub recorded_tokens_at_admission: u64,
    pub recorded_turns_at_admission: u64,
    pub recorded_cost_usd_at_admission: f64,
}
#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct BudgetReservation {
    pub request_id: String,
    pub session_id: String,
    pub provider: String,
    pub model: String,
    pub purpose: String,
    pub call_id: Option<String>,
    pub state: String,
    pub revision: u64,
    pub bound_source: Option<String>,
    pub normalized_sha256: Option<String>,
    pub deadline_ms: Option<i64>,
    pub estimated: ReservationEstimate,
    pub actual: Option<ReservationActual>,
    pub scopes: Vec<ReservationScope>,
    pub held_tokens: u64,
    pub held_cost_usd: Option<f64>,
    pub overrun_tokens: u64,
    pub overrun_cost_usd: Option<f64>,
    pub created_ms: i64,
    pub dispatched_ms: Option<i64>,
    pub updated_ms: i64,
    pub settled_ms: Option<i64>,
    pub released_ms: Option<i64>,
}
#[derive(Debug, Clone, Default, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReservationQuery {
    pub session_id: Option<String>,
    pub scope_id: Option<String>,
    #[serde(default)]
    pub open_only: bool,
    pub cursor: Option<String>,
    pub limit: Option<u32>,
}
#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct ReservationPage {
    pub reservations: Vec<BudgetReservation>,
    pub next_cursor: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ReservationAction {
    Release,
    Settle { actual: ReservationActual },
}
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReservationReconcile {
    pub operation_id: String,
    pub expected_revision: u64,
    pub reason: String,
    pub action: ReservationAction,
}
#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct ReservationAudit {
    pub operation_id: String,
    pub expected_revision: u64,
    pub owner: String,
    pub reason: String,
    pub action: Value,
    pub reconciled_ms: i64,
}
#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct ReservationDetail {
    pub reservation: BudgetReservation,
    pub audit: Vec<ReservationAudit>,
}

fn dollars(value: u64) -> f64 {
    value as f64 / 1_000_000.0
}
fn actual(value: ledger::Receipt) -> ReservationActual {
    ReservationActual {
        tokens: value.tokens.map(super::accounting::usage),
        turns: value.turns,
        cost_usd: value.cost_microusd.map(dollars),
        complete: value.complete,
    }
}
fn view(record: ledger::Record) -> Result<BudgetReservation, StoreError> {
    let known_tokens = record
        .actual
        .as_ref()
        .and_then(|a| a.tokens)
        .map(|t| t.total())
        .transpose()
        .map_err(StoreError::from)?
        .unwrap_or(0);
    let held = matches!(
        record.state,
        ledger::State::Reserved | ledger::State::Uncertain
    );
    Ok(BudgetReservation {
        request_id: record.request.request_id,
        session_id: record.request.session_id,
        provider: record.request.provider,
        model: record.request.model,
        purpose: record.request.purpose,
        call_id: record.request.call_id,
        state: match record.state {
            ledger::State::Reserved => "reserved",
            ledger::State::Uncertain => "uncertain",
            ledger::State::Settled => "settled",
            ledger::State::Released => "released",
        }
        .into(),
        revision: record.revision,
        bound_source: record.request.bound_source,
        normalized_sha256: record.request.normalized_sha256,
        deadline_ms: record.request.deadline_ms,
        estimated: ReservationEstimate {
            class_upper: super::accounting::usage(record.request.estimate.tokens),
            total_tokens_upper: record.request.estimate.total_tokens,
            turns_upper: record.request.estimate.turns,
            cost_usd_upper: record.request.estimate.cost_microusd.map(dollars),
        },
        held_tokens: if held {
            record
                .request
                .estimate
                .total_tokens
                .saturating_sub(known_tokens)
        } else {
            0
        },
        held_cost_usd: record.request.estimate.cost_microusd.map(|estimate| {
            if held {
                dollars(
                    estimate.saturating_sub(
                        record
                            .actual
                            .as_ref()
                            .and_then(|a| a.cost_microusd)
                            .unwrap_or(0),
                    ),
                )
            } else {
                0.0
            }
        }),
        actual: record.actual.map(actual),
        scopes: record
            .scopes
            .into_iter()
            .map(|scope| ReservationScope {
                id: scope.id,
                captured_budget: Budget {
                    max_tokens: scope.max_tokens,
                    max_turns: scope.max_turns,
                    max_cost_usd: scope.max_cost_microusd.map(dollars),
                    max_wall_seconds: None,
                    enforcement: Enforcement::Reserved,
                },
                recorded_tokens_at_admission: scope.spent_tokens,
                recorded_turns_at_admission: scope.spent_turns,
                recorded_cost_usd_at_admission: dollars(scope.spent_cost_microusd),
            })
            .collect(),
        overrun_tokens: record.overrun_tokens,
        overrun_cost_usd: record.overrun_cost_microusd.map(dollars),
        created_ms: record.created_ms,
        dispatched_ms: record.dispatched_ms,
        updated_ms: record.updated_ms,
        settled_ms: record.settled_ms,
        released_ms: record.released_ms,
    })
}

impl Runtime {
    pub fn budget_reservations(
        &self,
        query: ReservationQuery,
    ) -> Result<ReservationPage, RuntimeError> {
        let cursor = query
            .cursor
            .map(|text| {
                if text.len() > 2048 {
                    return Err(RuntimeError::Invalid("reservation cursor too long".into()));
                }
                serde_json::from_str::<ledger::Cursor>(&text)
                    .map_err(|_| RuntimeError::Invalid("invalid reservation cursor".into()))
            })
            .transpose()?;
        let filter = ledger::Filter {
            session_id: query.session_id,
            scope_id: query.scope_id,
            open_only: query.open_only,
            after: cursor,
            limit: query.limit.unwrap_or(50),
        };
        self.inner
            .store
            .read(move |db| {
                let page = ledger::list(db, &filter).map_err(StoreError::from)?;
                Ok(ReservationPage {
                    reservations: page
                        .records
                        .into_iter()
                        .map(view)
                        .collect::<Result<_, _>>()?,
                    next_cursor: page
                        .next
                        .map(|c| serde_json::to_string(&c))
                        .transpose()
                        .map_err(|e| super::refusal("invalid", &e.to_string(), None))?,
                })
            })
            .map_err(super::error)
    }
    pub fn budget_reservation(&self, id: &str) -> Result<Option<ReservationDetail>, RuntimeError> {
        let id = id.to_string();
        self.inner
            .store
            .read(move |db| {
                let Some(record) = ledger::get(db, &id).map_err(StoreError::from)? else {
                    return Ok(None);
                };
                let audits = ledger::audit(db, &id, 100)
                    .map_err(StoreError::from)?
                    .into_iter()
                    .map(|audit| ReservationAudit {
                        operation_id: audit.reconciliation.operation_id,
                        expected_revision: audit.reconciliation.expected_revision,
                        owner: audit.reconciliation.owner,
                        reason: audit.reconciliation.reason,
                        action: json!(audit.reconciliation.action),
                        reconciled_ms: audit.reconciled_ms,
                    })
                    .collect();
                Ok(Some(ReservationDetail {
                    reservation: view(record)?,
                    audit: audits,
                }))
            })
            .map_err(super::error)
    }
    /// The authenticated owner explicitly resolves a reviewed request. Live local
    /// attempts cannot be released, and lost acknowledgements reuse operation_id.
    pub async fn reconcile_budget_reservation(
        &self,
        id: &str,
        command: ReservationReconcile,
    ) -> Result<ReservationDetail, RuntimeError> {
        let key = id.to_string();
        let record = self
            .inner
            .store
            .read(move |db| ledger::get(db, &key).map_err(StoreError::from))
            .map_err(super::error)?
            .ok_or_else(|| RuntimeError::Invalid("reservation not found".into()))?;
        let handle = self.inner.handle(&record.request.session_id).await?;
        let mut state = handle.state.lock().await;
        let active = self
            .inner
            .reservation_attempts
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if active.get(id).is_some_and(|weak| weak.upgrade().is_some()) {
            return Err(RuntimeError::Busy(record.request.session_id));
        }
        let action = match command.action {
            ReservationAction::Release => ledger::ReconcileAction::Release,
            ReservationAction::Settle { actual } => ledger::ReconcileAction::Settle {
                receipt_event_id: format!("owner:{}", command.operation_id),
                actual: ledger::Receipt {
                    tokens: actual.tokens.map(super::accounting::amounts),
                    turns: actual.turns,
                    cost_microusd: actual
                        .cost_usd
                        .map(super::money)
                        .transpose()
                        .map_err(super::error)?,
                    complete: actual.complete,
                },
            },
        };
        let command = ledger::Reconciliation {
            operation_id: command.operation_id,
            expected_revision: command.expected_revision,
            owner: "authenticated-owner".into(),
            reason: command.reason,
            action,
        };
        let id = id.to_string();
        let source = state.info.id.clone();
        let (stored,())=self.inner.store.append_checked(&source,Expected::Seq(state.last_seq),move|tx|{
            let before=ledger::get(tx,&id).map_err(StoreError::from)?.ok_or_else(||super::refusal("not_found","reservation not found",None))?;
            let result=ledger::reconcile(tx,&id,&command,chrono::Utc::now().timestamp_millis()).map_err(StoreError::from)?;
            if !result.applied {return Ok((Vec::new(),()));}
            let mut events=Vec::new();
            if let (Some(delta),Some(actual))=(&result.delta,&result.record.actual) {
                if let Some(event)=super::accounting::billing(tx,&result.record.request,before.actual.as_ref(),actual,delta,&format!("owner:{}",command.operation_id),0)? {events.push(event);}
            }
            events.push(NewEvent::new(RECONCILED,json!({"request_id":id,"operation_id":command.operation_id,"reason":command.reason,"action":command.action})));
            Ok((events,()))
        }).map_err(super::error)?;
        for e in &stored {
            state.apply(e).map_err(RuntimeError::Corrupt)?;
        }
        drop(active);
        drop(state);
        self.inner.publish(&stored);
        self.budget_reservation(&record.request.request_id)?
            .ok_or_else(|| RuntimeError::Invalid("reservation disappeared".into()))
    }
}
