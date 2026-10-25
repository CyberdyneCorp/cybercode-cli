//! The export pipeline (`observability-costs` → OpenTelemetry export): the SDK providers
//! for the enabled signals over their protocol, the resource, and the mapper task that
//! feeds them from the runtime's live events.
//!
//! The batch span and log processors and the periodic metric reader run on the server's
//! Tokio runtime. Nothing in a Turn waits on them: a slow or unreachable collector only
//! fills the SDK queues, which drop. Every exporter is [`Counted`].

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use cyber_server::runtime::{LiveEvent, SessionInfo};
use futures::future::BoxFuture;
use opentelemetry::KeyValue;
use opentelemetry::logs::LoggerProvider as _;
use opentelemetry::metrics::MeterProvider as _;
use opentelemetry::trace::TracerProvider as _;
use opentelemetry_otlp::{WithExportConfig as _, WithHttpConfig as _};
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::logs::SdkLoggerProvider;
use opentelemetry_sdk::logs::log_processor_with_async_runtime::BatchLogProcessor;
use opentelemetry_sdk::metrics::SdkMeterProvider;
use opentelemetry_sdk::metrics::periodic_reader_with_async_runtime::PeriodicReader;
use opentelemetry_sdk::runtime::Tokio;
use opentelemetry_sdk::trace::SdkTracerProvider;
use opentelemetry_sdk::trace::span_processor_with_async_runtime::BatchSpanProcessor;
use tokio::sync::{broadcast, oneshot};

use crate::counted::{Counted, Health};
use crate::logs::Bridge;
use crate::mapper::Mapper;
use crate::privacy::Content;
use crate::settings::{Settings, Signal, SignalSettings};
use cyber_core::config::OtlpProtocol;

/// The instrumentation scope of every span and metric.
const SCOPE: &str = "cyber";
/// How long shutdown may take to flush, in total.
pub const SHUTDOWN: Duration = Duration::from_secs(3);

/// Reads a Session the mapper never saw created (it was created before a restart).
pub trait SessionLookup: Send + Sync {
    fn session(&self, id: &str) -> BoxFuture<'_, Option<SessionInfo>>;
}

impl SessionLookup for cyber_server::runtime::Runtime {
    fn session(&self, id: &str) -> BoxFuture<'_, Option<SessionInfo>> {
        let id = id.to_string();
        Box::pin(async move { self.session_info(&id).await.ok() })
    }
}

/// The signed-in account at server start, for the resource.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountIds {
    pub sub: String,
    pub org_id: Option<String>,
}

/// The resource of every signal: the resolved attributes plus the account's ids.
pub fn resource(settings: &Settings, account: Option<&AccountIds>) -> Resource {
    let mut attributes: Vec<KeyValue> = settings
        .resource
        .iter()
        .map(|(key, value)| KeyValue::new(key.clone(), value.clone()))
        .collect();
    if let Some(account) = account {
        attributes.push(KeyValue::new("cyber.account.sub", account.sub.clone()));
        if let Some(org) = &account.org_id {
            attributes.push(KeyValue::new("cyber.account.org_id", org.clone()));
        }
    }
    Resource::builder_empty()
        .with_attributes(attributes)
        .build()
}

/// The providers of the enabled signals.
pub struct Providers {
    traces: Option<SdkTracerProvider>,
    metrics: Option<SdkMeterProvider>,
    logs: Option<SdkLoggerProvider>,
}

impl Providers {
    /// Build the counted exporters of the enabled signals over `resource`. An error
    /// names the signal, never a header value.
    pub fn build(
        settings: &Settings,
        resource: &Resource,
        health: &Arc<Health>,
    ) -> Result<Self, String> {
        let enabled = |signal| Some(settings.signal(signal)).filter(|s| s.enabled);
        let traces = enabled(Signal::Traces)
            .map(|signal| tracer_provider(signal, resource.clone(), health))
            .transpose()?;
        let metrics = enabled(Signal::Metrics)
            .map(|signal| meter_provider(signal, resource.clone(), health))
            .transpose()?;
        let logs = enabled(Signal::Logs)
            .map(|signal| logger_provider(signal, resource.clone(), health))
            .transpose()?;
        Ok(Self {
            traces,
            metrics,
            logs,
        })
    }

    /// Providers over the given ones, for tests with in-memory exporters.
    pub fn of(traces: SdkTracerProvider, metrics: SdkMeterProvider) -> Self {
        Self {
            traces: Some(traces),
            metrics: Some(metrics),
            logs: None,
        }
    }

    /// The log sink over the logs provider, when logs are exported.
    pub fn log_sink(&self, content: Content) -> Option<cyber_core::log::Sink> {
        let logs = self.logs.as_ref()?;
        Some(Bridge::new(logs.logger(SCOPE), content).sink())
    }

    /// A mapper over these providers. A signal that is off gets the SDK's no-op one.
    pub fn mapper(&self, content: Content) -> Mapper {
        let tracer = self
            .traces
            .clone()
            .unwrap_or_else(|| SdkTracerProvider::builder().build())
            .tracer(SCOPE);
        let meter = self
            .metrics
            .clone()
            .unwrap_or_else(|| SdkMeterProvider::builder().build())
            .meter(SCOPE);
        Mapper::new(tracer, &meter, content)
    }

    /// Flush and shut down every provider, each on its own blocking thread so that a
    /// hung signal never holds up the others. Bound it with a timeout.
    pub async fn shutdown(&self) {
        let mut flushes = Vec::new();
        if let Some(traces) = self.traces.clone() {
            flushes.push(tokio::task::spawn_blocking(move || {
                let _ = traces.force_flush();
                let _ = traces.shutdown();
            }));
        }
        if let Some(metrics) = self.metrics.clone() {
            flushes.push(tokio::task::spawn_blocking(move || {
                let _ = metrics.force_flush();
                let _ = metrics.shutdown();
            }));
        }
        if let Some(logs) = self.logs.clone() {
            flushes.push(tokio::task::spawn_blocking(move || {
                let _ = logs.force_flush();
                let _ = logs.shutdown();
            }));
        }
        futures::future::join_all(flushes).await;
    }
}

fn tracer_provider(
    signal: &SignalSettings,
    resource: Resource,
    health: &Arc<Health>,
) -> Result<SdkTracerProvider, String> {
    let exporter = span_exporter(signal).map_err(|error| failed(signal, &error))?;
    let exporter = counted(exporter, signal, health);
    Ok(SdkTracerProvider::builder()
        .with_span_processor(BatchSpanProcessor::builder(exporter, Tokio).build())
        .with_resource(resource)
        .build())
}

fn meter_provider(
    signal: &SignalSettings,
    resource: Resource,
    health: &Arc<Health>,
) -> Result<SdkMeterProvider, String> {
    let exporter = metric_exporter(signal).map_err(|error| failed(signal, &error))?;
    let exporter = counted(exporter, signal, health);
    Ok(SdkMeterProvider::builder()
        .with_reader(PeriodicReader::builder(exporter, Tokio).build())
        .with_resource(resource)
        .build())
}

fn logger_provider(
    signal: &SignalSettings,
    resource: Resource,
    health: &Arc<Health>,
) -> Result<SdkLoggerProvider, String> {
    let exporter = log_exporter(signal).map_err(|error| failed(signal, &error))?;
    let exporter = counted(exporter, signal, health);
    Ok(SdkLoggerProvider::builder()
        .with_log_processor(BatchLogProcessor::builder(exporter, Tokio).build())
        .with_resource(resource)
        .build())
}

fn counted<E>(exporter: E, signal: &SignalSettings, health: &Arc<Health>) -> Counted<E> {
    Counted::new(
        exporter,
        signal.signal,
        signal.endpoint.value.clone(),
        Arc::clone(health),
    )
}

/// The reason a signal's exporter could not be built (an endpoint the SDK's URI parser
/// rejects, say). An error can quote the endpoint, which can carry credentials, so it
/// and the header values are redacted.
fn failed(signal: &SignalSettings, error: &opentelemetry_otlp::ExporterBuildError) -> String {
    let headers = signal.headers.value.iter().map(|(_, value)| value.clone());
    let secrets = headers.chain([signal.endpoint.value.clone()]);
    let error = Content::new(false, secrets).redact(&error.to_string());
    format!(
        "the {} exporter could not be built: {error}",
        signal.signal.name()
    )
}

fn span_exporter(
    signal: &SignalSettings,
) -> Result<opentelemetry_otlp::SpanExporter, opentelemetry_otlp::ExporterBuildError> {
    match signal.protocol() {
        #[cfg(feature = "otlp-grpc")]
        Some(OtlpProtocol::Grpc) => grpc::span_exporter(signal),
        protocol => opentelemetry_otlp::SpanExporter::builder()
            .with_http()
            .with_protocol(http_protocol(protocol))
            .with_endpoint(signal.endpoint.value.clone())
            .with_timeout(signal.timeout.value)
            .with_headers(headers(signal))
            .with_http_client(http_client(signal)?)
            .build(),
    }
}

fn metric_exporter(
    signal: &SignalSettings,
) -> Result<opentelemetry_otlp::MetricExporter, opentelemetry_otlp::ExporterBuildError> {
    match signal.protocol() {
        #[cfg(feature = "otlp-grpc")]
        Some(OtlpProtocol::Grpc) => grpc::metric_exporter(signal),
        protocol => opentelemetry_otlp::MetricExporter::builder()
            .with_http()
            .with_protocol(http_protocol(protocol))
            .with_endpoint(signal.endpoint.value.clone())
            .with_timeout(signal.timeout.value)
            .with_headers(headers(signal))
            .with_http_client(http_client(signal)?)
            .build(),
    }
}

fn log_exporter(
    signal: &SignalSettings,
) -> Result<opentelemetry_otlp::LogExporter, opentelemetry_otlp::ExporterBuildError> {
    match signal.protocol() {
        #[cfg(feature = "otlp-grpc")]
        Some(OtlpProtocol::Grpc) => grpc::log_exporter(signal),
        protocol => opentelemetry_otlp::LogExporter::builder()
            .with_http()
            .with_protocol(http_protocol(protocol))
            .with_endpoint(signal.endpoint.value.clone())
            .with_timeout(signal.timeout.value)
            .with_headers(headers(signal))
            .with_http_client(http_client(signal)?)
            .build(),
    }
}

/// The HTTP encoding of `protocol`; settings never let `grpc` reach here without the
/// `otlp-grpc` feature.
fn http_protocol(protocol: Option<OtlpProtocol>) -> opentelemetry_otlp::Protocol {
    match protocol {
        Some(OtlpProtocol::HttpJson) => opentelemetry_otlp::Protocol::HttpJson,
        _ => opentelemetry_otlp::Protocol::HttpBinary,
    }
}

fn headers(signal: &SignalSettings) -> HashMap<String, String> {
    signal.headers.value.iter().cloned().collect()
}

/// The workspace `reqwest` client. A collector on this machine is never reached
/// through a proxy. Redirects are never followed: a 3xx would replay the export and its
/// secret headers to a host that was never configured, past the https-or-loopback check
/// of the configured endpoint, so it counts as a failed export.
fn http_client(
    signal: &SignalSettings,
) -> Result<reqwest::Client, opentelemetry_otlp::ExporterBuildError> {
    let local = url::Url::parse(&signal.endpoint.value)
        .map(|url| crate::settings::loopback(&url))
        .unwrap_or(false);
    let mut builder = reqwest::Client::builder()
        .timeout(signal.timeout.value)
        .redirect(reqwest::redirect::Policy::none());
    if local {
        builder = builder.no_proxy();
    }
    builder
        .build()
        .map_err(|error| opentelemetry_otlp::ExporterBuildError::InternalFailure(error.to_string()))
}

#[cfg(feature = "otlp-grpc")]
mod grpc {
    use opentelemetry_otlp::tonic_types::metadata::MetadataMap;
    use opentelemetry_otlp::tonic_types::transport::ClientTlsConfig;
    use opentelemetry_otlp::{
        ExporterBuildError, LogExporter, MetricExporter, SpanExporter, WithExportConfig,
        WithTonicConfig,
    };

    use crate::settings::SignalSettings;

    fn configure<B: WithExportConfig + WithTonicConfig>(builder: B, signal: &SignalSettings) -> B {
        let mut headers = http::HeaderMap::new();
        for (name, value) in &signal.headers.value {
            if let (Ok(name), Ok(value)) = (
                http::HeaderName::try_from(name.as_str()),
                http::HeaderValue::try_from(value.as_str()),
            ) {
                headers.insert(name, value);
            }
        }
        let builder = builder
            .with_endpoint(signal.endpoint.value.clone())
            .with_timeout(signal.timeout.value)
            .with_metadata(MetadataMap::from_headers(headers));
        if signal.endpoint.value.starts_with("https://") {
            builder.with_tls_config(ClientTlsConfig::new().with_native_roots())
        } else {
            builder
        }
    }

    pub(super) fn span_exporter(
        signal: &SignalSettings,
    ) -> Result<SpanExporter, ExporterBuildError> {
        configure(SpanExporter::builder().with_tonic(), signal).build()
    }

    pub(super) fn metric_exporter(
        signal: &SignalSettings,
    ) -> Result<MetricExporter, ExporterBuildError> {
        configure(MetricExporter::builder().with_tonic(), signal).build()
    }

    pub(super) fn log_exporter(signal: &SignalSettings) -> Result<LogExporter, ExporterBuildError> {
        configure(LogExporter::builder().with_tonic(), signal).build()
    }
}

/// The running export: the providers, the mapper task and the log sink.
pub struct Exporter {
    stop: oneshot::Sender<()>,
    task: tokio::task::JoinHandle<()>,
    providers: Providers,
    sink: Option<cyber_core::log::Sink>,
}

impl Exporter {
    /// Start the mapper task on `events` over `providers`, and send the structured log
    /// to the logs provider when logs are exported.
    pub fn start(
        providers: Providers,
        content: Content,
        events: broadcast::Receiver<LiveEvent>,
        lookup: Arc<dyn SessionLookup>,
        health: Arc<Health>,
    ) -> Self {
        let mapper = providers.mapper(content.clone());
        let sink = providers.log_sink(content);
        if let Some(sink) = &sink {
            cyber_core::log::set_sink(Some(Arc::clone(sink)));
        }
        let (stop, stopped) = oneshot::channel();
        let task = tokio::spawn(run(mapper, events, lookup, health, stopped));
        Self {
            stop,
            task,
            providers,
            sink,
        }
    }

    /// Map the events already published, end the open spans as interrupted, stop the
    /// log sink, then flush and shut down the providers, bounded by [`SHUTDOWN`] in
    /// total. What is not exported by then is dropped.
    pub async fn shutdown(self) {
        let _ = self.stop.send(());
        let Self {
            task,
            providers,
            sink,
            ..
        } = self;
        let _ = tokio::time::timeout(SHUTDOWN, async move {
            let _ = task.await;
            if let Some(sink) = &sink {
                cyber_core::log::clear_sink(sink);
            }
            providers.shutdown().await;
        })
        .await;
    }
}

async fn run(
    mut mapper: Mapper,
    mut events: broadcast::Receiver<LiveEvent>,
    lookup: Arc<dyn SessionLookup>,
    health: Arc<Health>,
    mut stopped: oneshot::Receiver<()>,
) {
    loop {
        let event = tokio::select! {
            biased;
            _ = &mut stopped => break,
            event = events.recv() => event,
        };
        match event {
            Ok(event) => feed(&mut mapper, lookup.as_ref(), &event).await,
            Err(broadcast::error::RecvError::Lagged(missed)) => health.dropped(missed),
            Err(broadcast::error::RecvError::Closed) => break,
        }
    }
    loop {
        match events.try_recv() {
            Ok(event) => feed(&mut mapper, lookup.as_ref(), &event).await,
            Err(broadcast::error::TryRecvError::Lagged(missed)) => health.dropped(missed),
            Err(_) => break,
        }
    }
    mapper.shutdown();
}

/// Observe `event`, reading its Session first when the mapper has never seen it.
pub async fn feed(mapper: &mut Mapper, lookup: &dyn SessionLookup, event: &LiveEvent) {
    if let Some(id) = mapper.unknown_session(event) {
        let info = lookup.session(id).await;

        mapper.learn(id, info.as_ref());
    }
    mapper.observe(event);
}
