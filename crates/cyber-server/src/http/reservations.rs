//! These owner-only routes reuse the server's authentication, Host and Origin guard.
use super::{ApiError, AppState, envelope::Data};
use crate::runtime::{ReservationDetail, ReservationPage, ReservationQuery, ReservationReconcile};
use axum::{
    Json, Router,
    extract::{Path, Query, State},
    routing::{get, post},
};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/budgets/reservations", get(list))
        .route("/budgets/reservations/{id}", get(detail))
        .route("/budgets/reservations/{id}/reconcile", post(reconcile))
}

async fn list(
    State(state): State<AppState>,
    query: Result<Query<ReservationQuery>, axum::extract::rejection::QueryRejection>,
) -> Result<Json<Data<ReservationPage>>, ApiError> {
    let Query(query) = query.map_err(|e| {
        ApiError::new(
            axum::http::StatusCode::BAD_REQUEST,
            "InvalidRequestError",
            e.body_text(),
        )
    })?;
    Ok(Json(Data {
        data: state.runtime.budget_reservations(query)?,
    }))
}
async fn detail(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Data<ReservationDetail>>, ApiError> {
    let data = state.runtime.budget_reservation(&id)?.ok_or_else(|| {
        ApiError::not_found("BudgetReservationNotFoundError", "reservation not found")
    })?;
    Ok(Json(Data { data }))
}
async fn reconcile(
    State(state): State<AppState>,
    Path(id): Path<String>,
    command: Result<Json<ReservationReconcile>, axum::extract::rejection::JsonRejection>,
) -> Result<Json<Data<ReservationDetail>>, ApiError> {
    let Json(command) = command.map_err(|e| {
        ApiError::new(
            axum::http::StatusCode::BAD_REQUEST,
            "InvalidRequestError",
            e.body_text(),
        )
    })?;
    if state.runtime.budget_reservation(&id)?.is_none() {
        return Err(ApiError::not_found(
            "BudgetReservationNotFoundError",
            "reservation not found",
        ));
    }
    Ok(Json(Data {
        data: state
            .runtime
            .reconcile_budget_reservation(&id, command)
            .await?,
    }))
}
