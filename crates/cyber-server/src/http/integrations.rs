//! Owner-only queue facade over the durable workflow engine. Request Locations
//! and optimistic revisions fence every action; model tools cannot publish.
use axum::{
    Json, Router,
    extract::{Path, State},
    http::{StatusCode, request::Parts},
    routing::{get, post},
};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use super::{
    ApiError, AppState,
    envelope::{Data, location},
};
use crate::runtime::{
    RunStarter, StartWorkflowRun, integrations::IntegrationBatch, workflow_start,
};

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct IntegrationSubmit {
    pub session_id: String,
    pub manifest: cyber_workflow::integration::Manifest,
    pub budget: Option<Value>,
}
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct IntegrationRevision {
    pub expected_revision: u64,
}
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct IntegrationPublish {
    pub expected_revision: u64,
    pub operation_id: String,
    pub remote: String,
    pub repository: String,
    pub title: String,
    pub body: String,
    #[serde(default)]
    pub acknowledge_review_unavailable: bool,
}
pub(super) fn routes() -> Router<AppState> {
    Router::new()
        .route("/workflows/integrations", get(list).post(submit))
        .route("/workflows/integrations/{id}", get(show))
        .route("/workflows/integrations/{id}/report", get(show))
        .route("/workflows/integrations/{id}/run", post(run))
        .route("/workflows/integrations/{id}/cancel", post(cancel))
        .route("/workflows/integrations/{id}/publish", post(publish))
}
fn module() -> cyber_workflow::WorkflowModule {
    cyber_workflow::bundled::module("integrate")
        .expect("embedded integration module")
        .clone()
}
async fn existing(state: &AppState, id: &str, parts: &Parts) -> Result<IntegrationBatch, ApiError> {
    let batch = state
        .runtime
        .integration_batch(id)
        .map_err(|error| ApiError::not_found("IntegrationNotFoundError", error.to_string()))?;
    let request = location(parts, &state.options.default_directory)?;
    if request
        != std::fs::canonicalize(&batch.directory)
            .map_err(|_| ApiError::invalid("integration Location is unavailable"))?
    {
        return Err(ApiError::forbidden(
            "integration belongs to another Location",
        ));
    }
    Ok(batch)
}
async fn submit(
    State(state): State<AppState>,
    parts: Parts,
    Json(body): Json<IntegrationSubmit>,
) -> Result<(StatusCode, Json<Data<IntegrationBatch>>), ApiError> {
    body.manifest.validate().map_err(ApiError::invalid)?;
    let owner = state.runtime.state(&body.session_id).await?.info;
    if location(&parts, &state.options.default_directory)?
        != std::fs::canonicalize(&owner.directory)
            .map_err(|_| ApiError::invalid("owner Location unavailable"))?
    {
        return Err(ApiError::forbidden(
            "owner Session belongs to another Location",
        ));
    }
    let owner = super::workflows::owner_context(&owner);
    let module = module();
    state.services.authorize_workflow_run(&owner, &module)?;
    let args = state
        .services
        .prepare_workflow_args(
            &owner,
            &module,
            serde_json::to_value(body.manifest)
                .map_err(|error| ApiError::invalid(error.to_string()))?,
        )
        .await?;
    let batch = state
        .runtime
        .integration_submit(
            &owner.session_id,
            args,
            body.budget,
            parts
                .headers
                .get("idempotency-key")
                .and_then(|value| value.to_str().ok())
                .map(str::to_owned),
        )
        .await?;
    Ok((StatusCode::CREATED, Json(Data { data: batch })))
}
async fn list(
    State(state): State<AppState>,
    parts: Parts,
) -> Result<Json<Data<Vec<IntegrationBatch>>>, ApiError> {
    let directory = location(&parts, &state.options.default_directory)?
        .display()
        .to_string();
    let mut batches = state.runtime.integration_batches(&directory)?;
    for batch in &mut batches {
        revalidate(&state, batch).await?;
    }
    Ok(Json(Data { data: batches }))
}
async fn show(
    State(state): State<AppState>,
    Path(id): Path<String>,
    parts: Parts,
) -> Result<Json<Data<IntegrationBatch>>, ApiError> {
    let mut batch = existing(&state, &id, &parts).await?;
    revalidate(&state, &mut batch).await?;
    Ok(Json(Data { data: batch }))
}
async fn revalidate(state: &AppState, batch: &mut IntegrationBatch) -> Result<(), ApiError> {
    if let Some(run) = batch.run_id.as_deref()
        && matches!(
            batch.status.as_str(),
            "ready" | "failed" | "stale" | "unknown"
        )
    {
        let owner = state.runtime.state(&batch.session_id).await?.info;
        match state
            .services
            .integration_operation(
                &super::workflows::owner_context(&owner),
                run,
                "report",
                json!({}),
            )
            .await
        {
            Ok(evidence) => {
                if let Some(status @ ("unknown" | "stale" | "failed" | "conflicted")) =
                    evidence["status"].as_str()
                {
                    batch.status = status.into();
                }
                batch.report = Some(json!({"host":evidence,"workflow":batch.report}));
            }
            Err(error) => {
                batch.status = "unknown".into();
                batch.report = Some(
                    json!({"host":{"eligible":false,"limitation":error.body.message},"workflow":batch.report}),
                );
            }
        }
    }
    Ok(())
}
async fn run(
    State(state): State<AppState>,
    Path(id): Path<String>,
    parts: Parts,
    Json(body): Json<IntegrationRevision>,
) -> Result<Json<Data<IntegrationBatch>>, ApiError> {
    let batch = existing(&state, &id, &parts).await?;
    if batch.revision != body.expected_revision {
        return Err(ApiError::conflict("Integration revision changed"));
    }
    let owner = state.runtime.state(&batch.session_id).await?.info;
    let owner = super::workflows::owner_context(&owner);
    let module = module();
    state.services.authorize_workflow_run(&owner, &module)?;
    let args = state
        .services
        .prepare_workflow_args(
            &owner,
            &module,
            serde_json::to_value(&batch.manifest)
                .map_err(|error| ApiError::invalid(error.to_string()))?,
        )
        .await?;
    if args["integration_admission"]["base"] != batch.args["integration_admission"]["base"] {
        return Err(ApiError::conflict(
            "integration base changed; submit a new immutable batch",
        ));
    }
    let settings = state
        .services
        .workflow_location(std::path::Path::new(&batch.directory))?;
    let budget = workflow_start::client_budget(&settings, &module, batch.budget.as_ref())
        .map_err(super::workflows::start_error)?;
    let share =
        workflow_start::share(&settings, &module, None).map_err(super::workflows::start_error)?;
    let claimed = state
        .runtime
        .integration_claim(&id, body.expected_revision, args)
        .map_err(|error| ApiError::conflict(error.to_string()))?;
    let started = state
        .runtime
        .start_workflow_run(StartWorkflowRun {
            module,
            args: claimed.args,
            session_id: owner.session_id,
            limits: settings.limits,
            tiers: settings.tiers,
            budget,
            share,
            starter: RunStarter::Api,
            tools: vec![],
        })
        .await;
    let run = match started {
        Ok(run) => run,
        Err(error) => {
            state
                .runtime
                .integration_start_failed(&id, error.to_string())?;
            return Err(error.into());
        }
    };
    let batch = state.runtime.integration_bind(&id, &run.id)?;
    if batch.status == "cancelled" {
        let _ = state.runtime.stop_workflow_run(&run.id).await;
    }
    Ok(Json(Data { data: batch }))
}
async fn cancel(
    State(state): State<AppState>,
    Path(id): Path<String>,
    parts: Parts,
    Json(body): Json<IntegrationRevision>,
) -> Result<Json<Data<IntegrationBatch>>, ApiError> {
    existing(&state, &id, &parts).await?;
    Ok(Json(Data {
        data: state
            .runtime
            .integration_cancel(&id, body.expected_revision)
            .await
            .map_err(|error| ApiError::conflict(error.to_string()))?,
    }))
}
async fn publish(
    State(state): State<AppState>,
    Path(id): Path<String>,
    parts: Parts,
    Json(body): Json<IntegrationPublish>,
) -> Result<Json<Data<IntegrationBatch>>, ApiError> {
    let batch = existing(&state, &id, &parts).await?;
    if batch.revision != body.expected_revision || batch.status != "ready" {
        return Err(ApiError::conflict(
            "publication requires the current ready integration attempt",
        ));
    }
    let run = batch
        .run_id
        .as_deref()
        .ok_or_else(|| ApiError::conflict("integration run is unavailable"))?;
    let owner = state.runtime.state(&batch.session_id).await?.info;
    let input = json!({"operation_id":body.operation_id,"remote":body.remote,"repository":body.repository,"title":body.title,"body":body.body,"acknowledge_review_unavailable":body.acknowledge_review_unavailable});
    let publication = state
        .services
        .integration_operation(
            &super::workflows::owner_context(&owner),
            run,
            "publish",
            input,
        )
        .await?;
    Ok(Json(Data {
        data: state.runtime.integration_publication(
            &id,
            body.expected_revision,
            run,
            publication,
        )?,
    }))
}
