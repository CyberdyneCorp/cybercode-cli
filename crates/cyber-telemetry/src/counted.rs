//! Delivery accounting (`observability-costs` → OpenTelemetry export, delivery): every
//! exporter is wrapped in a [`Counted`] that counts what the endpoint acknowledged and
//! what it did not, keeps the last error with secrets redacted, and reports a failing
//! endpoint once.
//!
//! The report is shared by the three signals, because one unreachable collector fails
//! them all: one `warn` line when export starts failing (no signal was failing before),
//! and one `info` line when it recovers (no signal is failing any more).

use std::fmt;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use cyber_core::log::Level;
use cyber_server::http::telemetry::{ExportError, display_endpoint};
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::error::OTelSdkResult;
use opentelemetry_sdk::logs::{LogBatch, LogExporter};
use opentelemetry_sdk::metrics::Temporality;
use opentelemetry_sdk::metrics::data::ResourceMetrics;
use opentelemetry_sdk::metrics::exporter::PushMetricExporter;
use opentelemetry_sdk::trace::{SpanData, SpanExporter};
use serde_json::{Value, json};

use crate::privacy::Content;
use crate::settings::Signal;

/// Where the transitions are reported: the structured log, or a test's collector.
pub type Report = dyn Fn(Level, &str, Value) + Send + Sync;

/// One signal's counters.
#[derive(Default)]
struct Counts {
    exported: AtomicU64,
    failed: AtomicU64,
    failing: AtomicBool,
    last_error: Mutex<Option<ExportError>>,
}

/// A snapshot of one signal's counters.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SignalCounts {
    pub exported: u64,
    pub failed: u64,
    pub last_error: Option<ExportError>,
}

/// The delivery counters of every signal, and the bus events the mapper missed.
pub struct Health {
    signals: [Counts; 3],
    dropped_events: AtomicU64,
    /// How many signals are failing now.
    failing: AtomicUsize,
    redact: Content,
    report: Box<Report>,
}

impl Health {
    /// Counters that report to the structured log; `redact` masks secrets in errors.
    pub fn new(redact: Content) -> Self {
        Self::reporting(
            redact,
            Box::new(|level, msg, fields| cyber_core::log::log(level, "telemetry", msg, fields)),
        )
    }

    pub fn reporting(redact: Content, report: Box<Report>) -> Self {
        Self {
            signals: Default::default(),
            dropped_events: AtomicU64::new(0),
            failing: AtomicUsize::new(0),
            redact,
            report,
        }
    }

    pub fn counts(&self, signal: Signal) -> SignalCounts {
        let counts = &self.signals[signal as usize];
        SignalCounts {
            exported: counts.exported.load(Ordering::Relaxed),
            failed: counts.failed.load(Ordering::Relaxed),
            last_error: counts
                .last_error
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone(),
        }
    }

    /// Runtime events the mapper could not keep up with (`RecvError::Lagged`).
    pub fn dropped(&self, events: u64) {
        self.dropped_events.fetch_add(events, Ordering::Relaxed);
    }

    pub fn dropped_events(&self) -> u64 {
        self.dropped_events.load(Ordering::Relaxed)
    }

    /// Count one export of `items` to `endpoint` and report a transition.
    pub fn record(&self, signal: Signal, items: u64, result: &OTelSdkResult, endpoint: &str) {
        let counts = &self.signals[signal as usize];
        match result {
            Ok(()) => {
                counts.exported.fetch_add(items, Ordering::Relaxed);
                if counts.failing.swap(false, Ordering::AcqRel)
                    && self.failing.fetch_sub(1, Ordering::AcqRel) == 1
                {
                    (self.report)(
                        Level::Info,
                        "telemetry export recovered",
                        json!({ "signal": signal.name() }),
                    );
                }
            }
            Err(error) => {
                counts.failed.fetch_add(items, Ordering::Relaxed);
                let message = self.redact.redact(&error.to_string());
                *counts
                    .last_error
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner) = Some(ExportError {
                    message: message.clone(),
                    at_ms: now_ms(),
                });
                if !counts.failing.swap(true, Ordering::AcqRel)
                    && self.failing.fetch_add(1, Ordering::AcqRel) == 0
                {
                    (self.report)(
                        Level::Warn,
                        "telemetry export failing",
                        json!({
                            "signal": signal.name(),
                            "endpoint": display_endpoint(endpoint),
                            "error": message,
                        }),
                    );
                }
            }
        }
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_millis() as i64)
}

/// An exporter whose results are counted in [`Health`].
pub struct Counted<E> {
    inner: E,
    signal: Signal,
    endpoint: String,
    health: Arc<Health>,
}

impl<E> Counted<E> {
    pub fn new(inner: E, signal: Signal, endpoint: String, health: Arc<Health>) -> Self {
        Self {
            inner,
            signal,
            endpoint,
            health,
        }
    }

    async fn counted(
        &self,
        items: u64,
        export: impl Future<Output = OTelSdkResult>,
    ) -> OTelSdkResult {
        let result = export.await;
        self.health
            .record(self.signal, items, &result, &self.endpoint);
        result
    }
}

/// The endpoint is left out: it is only shown masked.
impl<E> fmt::Debug for Counted<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Counted")
            .field("signal", &self.signal.name())
            .finish_non_exhaustive()
    }
}

impl<E: SpanExporter> SpanExporter for Counted<E> {
    async fn export(&self, batch: Vec<SpanData>) -> OTelSdkResult {
        let items = batch.len() as u64;
        self.counted(items, self.inner.export(batch)).await
    }

    fn shutdown_with_timeout(&self, timeout: Duration) -> OTelSdkResult {
        self.inner.shutdown_with_timeout(timeout)
    }

    fn force_flush(&self) -> OTelSdkResult {
        self.inner.force_flush()
    }

    fn set_resource(&mut self, resource: &Resource) {
        self.inner.set_resource(resource);
    }
}

impl<E: PushMetricExporter> PushMetricExporter for Counted<E> {
    /// The items of a metric export are its metrics (instruments).
    async fn export(&self, metrics: &ResourceMetrics) -> OTelSdkResult {
        let items = metrics
            .scope_metrics()
            .map(|scope| scope.metrics().count() as u64)
            .sum();
        self.counted(items, self.inner.export(metrics)).await
    }

    fn force_flush(&self) -> OTelSdkResult {
        self.inner.force_flush()
    }

    fn shutdown_with_timeout(&self, timeout: Duration) -> OTelSdkResult {
        self.inner.shutdown_with_timeout(timeout)
    }

    fn temporality(&self) -> Temporality {
        self.inner.temporality()
    }
}

impl<E: LogExporter> LogExporter for Counted<E> {
    async fn export(&self, batch: LogBatch<'_>) -> OTelSdkResult {
        let items = batch.iter().count() as u64;
        self.counted(items, self.inner.export(batch)).await
    }

    fn shutdown_with_timeout(&self, timeout: Duration) -> OTelSdkResult {
        self.inner.shutdown_with_timeout(timeout)
    }

    fn set_resource(&mut self, resource: &Resource) {
        self.inner.set_resource(resource);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use opentelemetry_sdk::error::OTelSdkError;

    type Lines = Arc<Mutex<Vec<(Level, String, Value)>>>;

    fn health(exact: &str) -> (Arc<Health>, Lines) {
        let lines = Lines::default();
        let report = {
            let lines = Arc::clone(&lines);
            Box::new(move |level, msg: &str, fields| {
                lines.lock().unwrap().push((level, msg.to_string(), fields));
            })
        };
        let redact = Content::new(false, [exact.to_string()]);
        (Arc::new(Health::reporting(redact, report)), lines)
    }

    /// Fails its first `failures` exports, quoting a secret, then succeeds.
    #[derive(Debug)]
    struct Flaky {
        failures: AtomicUsize,
    }

    impl SpanExporter for Flaky {
        async fn export(&self, _batch: Vec<SpanData>) -> OTelSdkResult {
            let left = self.failures.load(Ordering::Relaxed);
            if left == 0 {
                return Ok(());
            }
            self.failures.store(left - 1, Ordering::Relaxed);
            Err(OTelSdkError::InternalFailure(
                "refused with key hdr-9931 and sk-abcdefghijklmnop".into(),
            ))
        }
    }

    fn counted(failures: usize, health: &Arc<Health>) -> Counted<Flaky> {
        let flaky = Flaky {
            failures: AtomicUsize::new(failures),
        };
        Counted::new(
            flaky,
            Signal::Traces,
            "http://user:pw@127.0.0.1:4318/v1/traces?key=hdr-9931".into(),
            Arc::clone(health),
        )
    }

    fn spans(n: usize) -> Vec<SpanData> {
        let exporter = opentelemetry_sdk::trace::InMemorySpanExporter::default();
        let provider = opentelemetry_sdk::trace::SdkTracerProvider::builder()
            .with_simple_exporter(exporter.clone())
            .build();
        let tracer = opentelemetry::trace::TracerProvider::tracer(&provider, "test");
        for _ in 0..n {
            drop(opentelemetry::trace::Tracer::start(&tracer, "span"));
        }
        exporter.get_finished_spans().unwrap()
    }

    #[tokio::test]
    async fn failing_then_recovering_reports_once_each() {
        let (health, lines) = health("hdr-9931");
        let exporter = counted(3, &health);
        for _ in 0..5 {
            let _ = exporter.export(spans(2)).await;
        }
        let counts = health.counts(Signal::Traces);
        assert_eq!((counts.exported, counts.failed), (4, 6));
        let error = counts.last_error.expect("the last error is kept");
        assert_eq!(
            error.message,
            "Operation failed: refused with key *** and ***"
        );
        assert!(error.at_ms > 0);

        let lines = lines.lock().unwrap();
        assert_eq!(lines.len(), 2, "{lines:?}");
        assert_eq!(lines[0].0, Level::Warn);
        assert_eq!(lines[0].1, "telemetry export failing");
        assert_eq!(lines[0].2["signal"], "traces");
        assert_eq!(lines[0].2["endpoint"], "http://127.0.0.1:4318/v1/traces");
        assert!(!lines[0].2.to_string().contains("hdr-9931"), "{lines:?}");
        assert_eq!(lines[1].0, Level::Info);
        assert_eq!(lines[1].1, "telemetry export recovered");
    }

    /// One unreachable collector fails every signal: one line, not one per signal.
    #[tokio::test]
    async fn every_signal_failing_is_one_report() {
        let (health, lines) = health("x");
        let error: OTelSdkResult = Err(OTelSdkError::InternalFailure("refused".into()));
        for signal in Signal::ALL {
            health.record(signal, 1, &error, "http://127.0.0.1:1");
            health.record(signal, 1, &error, "http://127.0.0.1:1");
        }
        health.record(Signal::Traces, 1, &Ok(()), "http://127.0.0.1:1");
        assert_eq!(
            lines.lock().unwrap().len(),
            1,
            "still failing: no recovery yet"
        );
        health.record(Signal::Metrics, 1, &Ok(()), "http://127.0.0.1:1");
        health.record(Signal::Logs, 1, &Ok(()), "http://127.0.0.1:1");
        let lines = lines.lock().unwrap();
        let messages: Vec<_> = lines.iter().map(|(_, msg, _)| msg.as_str()).collect();
        assert_eq!(
            messages,
            ["telemetry export failing", "telemetry export recovered"]
        );
        assert_eq!(health.counts(Signal::Logs).failed, 2);
    }

    #[test]
    fn lagged_events_are_counted_as_dropped() {
        let (health, _) = health("x");
        health.dropped(3);
        health.dropped(4);
        assert_eq!(health.dropped_events(), 7);
    }
}
