//! The status behind `GET /api/v1/telemetry` and `cyber telemetry show`: the settings
//! the server resolved at start, the project values a Location's layers would set, and
//! whether the configuration on disk has changed since.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use cyber_core::config::TelemetryLoad;
use cyber_core::env::EnvSource;
use cyber_server::http::telemetry::{
    IgnoredTelemetry, MaskedHeader, ProductTelemetry, SignalSources, SignalStatus, SourcedFlag,
    TelemetryReport, TelemetryResource, TelemetrySignals, TelemetryStatus,
};

use cyber_server::runtime::LiveEvent;
use tokio::sync::broadcast;

use crate::counted::Health;
use crate::pipeline::{self, AccountIds, Exporter, Providers, SessionLookup};
use crate::privacy::Content;
use crate::settings::{self, Settings, Signal, SignalSettings};

/// Reads the `telemetry` configuration for a Location.
pub type LoadFn = dyn Fn(&Path) -> TelemetryLoad + Send + Sync;

/// The running server's telemetry: its settings, resolved once, and the exporter.
pub struct Telemetry {
    settings: Settings,
    load: Arc<LoadFn>,
    env: Arc<dyn EnvSource + Send + Sync>,
    exporter: Mutex<Option<Exporter>>,
    /// Why the exporters could not be built, which keeps export off.
    failure: Mutex<Option<String>>,
    account_signed_in: AtomicBool,
    health: Arc<Health>,
}

impl Telemetry {
    /// Resolve the settings of `location` once. A fail-closed reason is logged as one
    /// `warn` line; the server starts either way.
    pub fn start(
        load: Arc<LoadFn>,
        env: Arc<dyn EnvSource + Send + Sync>,
        location: &Path,
    ) -> Self {
        let settings = settings::resolve(&load(location), env.as_ref(), crate::GRPC);
        if settings.fail_closed {
            cyber_core::log::warn(
                "telemetry",
                "telemetry export off",
                serde_json::json!({ "reason": settings.reason }),
            );
        }
        let health = Arc::new(Health::new(Content::new(
            false,
            settings.header_values().map(str::to_string),
        )));
        Self {
            settings,
            load,
            env,
            exporter: Mutex::new(None),
            failure: Mutex::new(None),
            account_signed_in: AtomicBool::new(false),
            health,
        }
    }

    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    /// Start exporting `events` when export is on. `account` is the signed-in account
    /// at start; its ids go into the resource and `show` reports only that one is. When
    /// the exporters cannot be built, export stays off and `show` reports why.
    pub fn export(
        &self,
        events: broadcast::Receiver<LiveEvent>,
        lookup: Arc<dyn SessionLookup>,
        account: Option<AccountIds>,
    ) {
        if !self.settings.enabled {
            return;
        }
        let resource = pipeline::resource(&self.settings, account.as_ref());
        let providers = match Providers::build(&self.settings, &resource, &self.health) {
            Ok(providers) => providers,
            Err(reason) => {
                cyber_core::log::warn(
                    "telemetry",
                    "telemetry export off",
                    serde_json::json!({ "reason": reason }),
                );
                *self.failure.lock().unwrap_or_else(PoisonError::into_inner) = Some(reason);
                return;
            }
        };
        let content = Content::new(
            self.settings.log_prompts.value,
            self.settings.header_values().map(str::to_string),
        );
        let health = Arc::clone(&self.health);
        let exporter = Exporter::start(providers, content, events, lookup, health);
        self.account_signed_in
            .store(account.is_some(), Ordering::Relaxed);
        *self.exporter.lock().unwrap_or_else(PoisonError::into_inner) = Some(exporter);
    }

    /// End the open spans and flush, bounded by [`pipeline::SHUTDOWN`], then log the
    /// final counters. Nothing is exported after it.
    pub async fn shutdown(&self) {
        let exporter = self
            .exporter
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        let Some(exporter) = exporter else {
            return;
        };
        exporter.shutdown().await;
        let mut fields = serde_json::Map::new();
        for signal in Signal::ALL {
            let counts = self.health.counts(signal);
            fields.insert(
                signal.name().into(),
                serde_json::json!({ "exported": counts.exported, "failed": counts.failed }),
            );
        }
        fields.insert("dropped_events".into(), self.health.dropped_events().into());
        cyber_core::log::info(
            "telemetry",
            "telemetry export stopped",
            serde_json::Value::Object(fields),
        );
    }
}

impl TelemetryReport for Telemetry {
    fn status(&self, location: &Path) -> TelemetryStatus {
        let load = (self.load)(location);
        let current = settings::resolve(&load, self.env.as_ref(), crate::GRPC);
        let mut status = status(&self.settings);
        for (signal, report) in Signal::ALL.into_iter().zip(status.signals.each_mut()) {
            let counts = self.health.counts(signal);
            report.exported = counts.exported;
            report.failed = counts.failed;
            report.last_error = counts.last_error;
        }
        status.dropped_events = self.health.dropped_events();
        status.resource.account_signed_in = self.account_signed_in.load(Ordering::Relaxed);
        status.ignored = load
            .ignored
            .into_iter()
            .map(|ignored| IgnoredTelemetry {
                pointer: ignored.pointer,
                source: ignored.source,
            })
            .collect();
        status.restart_required = current != self.settings;
        let failure = self.failure.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(reason) = failure.as_ref() {
            switch_off(&mut status, reason);
        }
        status
    }
}

/// `status` with export and every signal off for `reason`.
fn switch_off(status: &mut TelemetryStatus, reason: &str) {
    status.enabled = false;
    status.reason = Some(reason.to_string());
    for signal in status.signals.each_mut() {
        if signal.enabled {
            signal.enabled = false;
            signal.reason = Some(reason.to_string());
        }
    }
}

/// The status of `settings` with zero counters, nothing ignored and no restart required.
pub fn status(settings: &Settings) -> TelemetryStatus {
    let signal = |signal| signal_status(settings.signal(signal));
    let attribute = |key: &str| {
        settings
            .resource
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, value)| value.clone())
            .unwrap_or_default()
    };
    TelemetryStatus {
        enabled: settings.enabled,
        reason: settings.reason.clone(),
        log_prompts: SourcedFlag {
            value: settings.log_prompts.value,
            source: settings.log_prompts.source.to_string(),
        },
        signals: TelemetrySignals {
            traces: signal(Signal::Traces),
            metrics: signal(Signal::Metrics),
            logs: signal(Signal::Logs),
        },
        dropped_events: 0,
        resource: TelemetryResource {
            service_name: attribute("service.name"),
            service_version: attribute("service.version"),
            os_type: attribute("os.type"),
            account_signed_in: false,
        },
        ignored: Vec::new(),
        restart_required: false,
        product: ProductTelemetry { enabled: false },
    }
}

fn signal_status(signal: &SignalSettings) -> SignalStatus {
    SignalStatus {
        enabled: signal.enabled,
        reason: signal.switched_off.clone(),
        endpoint: signal.endpoint.value.clone(),
        protocol: signal.protocol.value.clone(),
        timeout_ms: signal.timeout.value.as_millis() as u64,
        headers: signal
            .headers
            .value
            .iter()
            .map(|(name, _)| MaskedHeader::new(name.clone()))
            .collect(),
        sources: SignalSources {
            endpoint: signal.endpoint.source.to_string(),
            protocol: signal.protocol.source.to_string(),
            headers: signal.headers.source.to_string(),
            timeout: signal.timeout.source.to_string(),
        },
        exported: 0,
        failed: 0,
        dropped: 0,
        last_error: None,
    }
}
