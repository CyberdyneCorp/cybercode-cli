//! The `telemetry` route group (`observability-costs` → OpenTelemetry configuration):
//! `GET /telemetry`, the server's own OpenTelemetry export status. The values are the
//! server's because its environment is the one that exports. Header values never leave
//! this route: whatever the report holds, each is answered as `***`, and the endpoint
//! without userinfo, query or fragment.

use std::path::Path;
use std::sync::Arc;

use axum::{Json, Router, extract::State, http::request::Parts, routing::get};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{
    ApiError, AppState,
    envelope::{Located, LocationInfo, location},
};

/// What every header value is answered as.
pub const MASK: &str = "***";

/// The source of the export status: the running server's resolved settings and counters.
pub trait TelemetryReport: Send + Sync {
    /// The status, with the project values `location`'s layers would set listed as ignored.
    fn status(&self, location: &Path) -> TelemetryStatus;
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TelemetryStatus {
    /// Whether OpenTelemetry export is on.
    pub enabled: bool,
    /// Why export is off.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// `telemetry.log_prompts`.
    pub log_prompts: SourcedFlag,
    pub signals: TelemetrySignals,
    /// Runtime events the exporter could not keep up with.
    pub dropped_events: u64,
    pub resource: TelemetryResource,
    /// Project values of `telemetry.otel` and `telemetry.log_prompts`, never read.
    pub ignored: Vec<IgnoredTelemetry>,
    /// The configuration on disk differs from the running settings; they apply on restart.
    pub restart_required: bool,
    pub product: ProductTelemetry,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SourcedFlag {
    pub value: bool,
    /// `config (<layer>)`, `env (<VARIABLE>)`, `policy (<managed source>)` or `default`.
    pub source: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TelemetrySignals {
    pub traces: SignalStatus,
    pub metrics: SignalStatus,
    pub logs: SignalStatus,
}

impl TelemetrySignals {
    pub fn each_mut(&mut self) -> [&mut SignalStatus; 3] {
        [&mut self.traces, &mut self.metrics, &mut self.logs]
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SignalStatus {
    /// Whether this signal is exported.
    pub enabled: bool,
    /// Why this signal is off while export is on, such as `OTEL_TRACES_EXPORTER=none`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// The effective endpoint, without userinfo or query.
    pub endpoint: String,
    /// `http/protobuf`, `http/json` or `grpc`.
    pub protocol: String,
    pub timeout_ms: u64,
    /// Every header with its value as `***`.
    pub headers: Vec<MaskedHeader>,
    pub sources: SignalSources,
    /// Items the endpoint acknowledged.
    pub exported: u64,
    /// Items in batches the endpoint rejected or could not receive.
    pub failed: u64,
    pub dropped: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<ExportError>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MaskedHeader {
    pub name: String,
    /// Always `***`.
    pub value: String,
}

impl MaskedHeader {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            value: MASK.into(),
        }
    }
}

/// The source of each signal value, labelled as [`SourcedFlag::source`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SignalSources {
    pub endpoint: String,
    pub protocol: String,
    pub headers: String,
    pub timeout: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ExportError {
    /// The error, with header values redacted.
    pub message: String,
    /// Unix milliseconds.
    pub at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TelemetryResource {
    #[serde(rename = "service.name")]
    pub service_name: String,
    #[serde(rename = "service.version")]
    pub service_version: String,
    #[serde(rename = "os.type")]
    pub os_type: String,
    /// Whether the resource carries a Cyber Account; its identifiers are never shown.
    pub account_signed_in: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct IgnoredTelemetry {
    /// JSON pointer, such as `/telemetry/otel/endpoint`.
    pub pointer: String,
    /// The project layer, such as `project:/repo/.cyber/cyber.jsonc`.
    pub source: String,
}

/// Product telemetry is not implemented and always off.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ProductTelemetry {
    pub enabled: bool,
}

pub fn routes() -> Router<AppState> {
    Router::new().route("/telemetry", get(telemetry))
}

async fn telemetry(
    State(state): State<AppState>,
    parts: Parts,
) -> Result<Json<Located<TelemetryStatus>>, ApiError> {
    let directory = location(&parts, &state.options.default_directory)?;
    let report = state.telemetry.clone().ok_or_else(unavailable)?;
    let probe = directory.clone();
    let mut status = tokio::task::spawn_blocking(move || report.status(&probe))
        .await
        .map_err(|_| ApiError::invalid("Reading the telemetry status was interrupted"))?;
    mask(&mut status);
    Ok(Json(Located {
        location: LocationInfo::of(&directory),
        data: status,
    }))
}

fn unavailable() -> ApiError {
    ApiError::service_unavailable("telemetry", "Telemetry status is unavailable in this host")
}

/// Every header value as `***` and every endpoint without userinfo, query or fragment.
pub fn mask(status: &mut TelemetryStatus) {
    for signal in status.signals.each_mut() {
        for header in &mut signal.headers {
            header.value = MASK.into();
        }
        signal.endpoint = display_endpoint(&signal.endpoint);
    }
}

/// `endpoint` without userinfo, query or fragment; text that is no URL is not shown.
pub fn display_endpoint(endpoint: &str) -> String {
    let Ok(mut url) = url::Url::parse(endpoint) else {
        return "(invalid endpoint)".into();
    };
    if url.cannot_be_a_base() {
        return "(invalid endpoint)".into();
    }
    let _ = url.set_username("");
    let _ = url.set_password(None);
    url.set_query(None);
    url.set_fragment(None);
    url.to_string()
}

/// A report handle, for [`AppState::telemetry`].
pub type TelemetryHandle = Arc<dyn TelemetryReport>;
