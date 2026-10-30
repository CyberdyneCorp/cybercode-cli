//! Durable request reservations. Provider I/O starts only after atomic all-scope
//! admission; actual receipts and allocation settlement commit with native billing.

mod accounting;
mod diagnostics;
mod dispatch;
mod guard;
mod scopes;
mod stream;
pub use guard::GuardedModel;

pub(super) use accounting::Attempt;
pub use diagnostics::{
    BudgetReservation, ReservationAction, ReservationActual, ReservationAudit, ReservationDetail,
    ReservationEstimate, ReservationPage, ReservationQuery, ReservationReconcile, ReservationScope,
};
pub(super) use dispatch::GatedAdapter;
pub(super) fn register(registry: &mut cyber_store::EventRegistry) {
    diagnostics::register(registry);
}

use super::RuntimeError;
use cyber_store::{NewEvent, StoreError};
use serde_json::{Value, json};

pub(super) fn bind_created(
    tx: &rusqlite::Transaction<'_>,
    event: &cyber_store::StoredEvent,
) -> Result<(), StoreError> {
    scopes::bind_created(tx, event)
}

pub(super) fn mark_accounted(event: &mut NewEvent, requests: &[String]) {
    if !requests.is_empty() {
        event.data["usage_accounted"] = true.into();
        event.data["reservation_requests"] = json!(requests);
    }
}

pub(super) fn mark_for_call(
    store: &cyber_store::Store,
    session: &str,
    call: &str,
    event: &mut NewEvent,
) -> Result<(), RuntimeError> {
    let session = session.to_string();
    let call = call.to_string();
    let ids=store.read(move|db|{
        let mut statement=db.prepare("SELECT request_id FROM budget_reservation WHERE session_id=?1 AND call_id=?2 ORDER BY created_ms,request_id")?;
        Ok(statement.query_map(rusqlite::params![session,call],|row|row.get::<_,String>(0))?.collect::<Result<Vec<_>,_>>()?)
    })?;
    mark_accounted(event, &ids);
    Ok(())
}

/// Whether a reservation billed an attempt of `call`: its receipts already carry the usage.
pub(super) fn reserved_call(
    store: &cyber_store::Store,
    session: &str,
    call: &str,
) -> Result<bool, RuntimeError> {
    let (session, call) = (session.to_string(), call.to_string());
    Ok(store.read(move |db| {
        Ok(db.query_row(
            "SELECT EXISTS(SELECT 1 FROM budget_reservation WHERE session_id=?1 AND call_id=?2)",
            rusqlite::params![session, call],
            |row| row.get::<_, bool>(0),
        )?)
    })?)
}

pub(super) fn error(error: StoreError) -> RuntimeError {
    if let StoreError::Projector { kind, reason } = &error {
        let value: Value = serde_json::from_str(reason).unwrap_or(Value::Null);
        let scope = value["scope"].as_str().unwrap_or("unknown").to_string();
        match kind.as_str() {
            "budget.reservation.exceeded" => {
                return RuntimeError::BudgetExceeded {
                    scope,
                    limit: value["limit"].as_str().unwrap_or("unknown").into(),
                };
            }
            "budget.reservation.unpriced" => return RuntimeError::BudgetUnpriced { scope },
            "budget.reservation.uncertain" => return RuntimeError::BudgetUncertain { scope },
            "budget.reservation.bound_unavailable" => {
                return RuntimeError::BudgetBoundUnavailable(
                    value["message"]
                        .as_str()
                        .unwrap_or("unsupported request")
                        .into(),
                );
            }
            "budget.reservation.stale"
            | "budget.reservation.conflict"
            | "budget.reservation.transition" => {
                return RuntimeError::Conflict(
                    value["message"]
                        .as_str()
                        .unwrap_or("reservation conflict")
                        .into(),
                );
            }
            "budget.reservation.invalid" | "budget.reservation.scope_invalid" => {
                return RuntimeError::Invalid(
                    value["message"]
                        .as_str()
                        .unwrap_or("invalid reservation")
                        .into(),
                );
            }
            _ => {}
        }
    }
    RuntimeError::Store(error)
}

fn refusal(code: &str, message: &str, scope: Option<&str>) -> StoreError {
    StoreError::Projector {
        kind: format!("budget.reservation.{code}"),
        reason: json!({"message":message,"scope":scope}).to_string(),
    }
}

fn money(value: f64) -> Result<u64, StoreError> {
    let scaled = value * 1_000_000.0;
    if !scaled.is_finite() || scaled < 0.0 || scaled >= u64::MAX as f64 {
        return Err(refusal(
            "invalid",
            "invalid or overflowing monetary amount",
            None,
        ));
    }
    Ok(scaled.ceil() as u64)
}
