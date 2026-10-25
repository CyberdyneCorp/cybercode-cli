//! An in-process OTLP/HTTP collector for production-path telemetry tests: it accepts
//! `http/protobuf` exports on `127.0.0.1`, decodes them with `opentelemetry-proto` and
//! keeps the raw bodies and headers. It never reaches the network. [`refused`] and
//! [`Silent`] are collectors that cannot be reached and that never answer.
#![allow(dead_code)]

use std::sync::{Arc, Mutex};

use axum::body::Bytes;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::routing::post;
use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest;
use opentelemetry_proto::tonic::collector::metrics::v1::ExportMetricsServiceRequest;
use opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest;
use opentelemetry_proto::tonic::common::v1::{AnyValue, KeyValue, any_value};
pub use opentelemetry_proto::tonic::logs::v1::LogRecord;
use opentelemetry_proto::tonic::metrics::v1::{metric, number_data_point};
use opentelemetry_proto::tonic::trace::v1::Span;
use prost::Message;

#[derive(Default)]
struct Received {
    traces: Vec<ExportTraceServiceRequest>,
    metrics: Vec<ExportMetricsServiceRequest>,
    logs: Vec<ExportLogsServiceRequest>,
    bodies: Vec<Vec<u8>>,
    headers: Vec<HeaderMap>,
}

type Shared = Arc<Mutex<Received>>;

pub struct Receiver {
    pub url: String,
    received: Shared,
    task: tokio::task::JoinHandle<()>,
}

/// A metric data point: its value and attributes.
#[derive(Debug, Clone)]
pub struct Point {
    pub name: String,
    pub value: f64,
    pub attributes: Vec<(String, String)>,
}

impl Point {
    pub fn attribute(&self, key: &str) -> Option<&str> {
        self.attributes
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, value)| value.as_str())
    }
}

impl Receiver {
    pub async fn start() -> Self {
        let received = Shared::default();
        let router = axum::Router::new()
            .route("/v1/traces", post(traces))
            .route("/v1/metrics", post(metrics))
            .route("/v1/logs", post(logs))
            .with_state(Arc::clone(&received));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        Self {
            url,
            received,
            task,
        }
    }

    /// Every exported span.
    pub fn spans(&self) -> Vec<Span> {
        let received = self.received.lock().unwrap();
        received
            .traces
            .iter()
            .flat_map(|request| &request.resource_spans)
            .flat_map(|resource| &resource.scope_spans)
            .flat_map(|scope| scope.spans.clone())
            .collect()
    }

    /// Every exported log record.
    pub fn log_records(&self) -> Vec<LogRecord> {
        let received = self.received.lock().unwrap();
        received
            .logs
            .iter()
            .flat_map(|request| &request.resource_logs)
            .flat_map(|resource| &resource.scope_logs)
            .flat_map(|scope| scope.log_records.clone())
            .collect()
    }

    pub fn named(&self, name: &str) -> Vec<Span> {
        self.spans()
            .into_iter()
            .filter(|span| span.name == name)
            .collect()
    }

    /// The resource attributes of the first trace and metric exports.
    pub fn resources(&self) -> Vec<Vec<(String, String)>> {
        let received = self.received.lock().unwrap();
        let traces = received
            .traces
            .iter()
            .flat_map(|request| &request.resource_spans)
            .filter_map(|resource| resource.resource.as_ref());
        let metrics = received
            .metrics
            .iter()
            .flat_map(|request| &request.resource_metrics)
            .filter_map(|resource| resource.resource.as_ref());
        traces
            .chain(metrics)
            .map(|resource| pairs(&resource.attributes))
            .collect()
    }

    /// The data points of the last metric export (counters are cumulative).
    pub fn points(&self) -> Vec<Point> {
        let received = self.received.lock().unwrap();
        let Some(last) = received.metrics.last() else {
            return Vec::new();
        };
        let mut points = Vec::new();
        for metric in last
            .resource_metrics
            .iter()
            .flat_map(|resource| &resource.scope_metrics)
            .flat_map(|scope| &scope.metrics)
        {
            let Some(metric::Data::Sum(sum)) = &metric.data else {
                continue;
            };
            for point in &sum.data_points {
                let value = match point.value {
                    Some(number_data_point::Value::AsInt(value)) => value as f64,
                    Some(number_data_point::Value::AsDouble(value)) => value,
                    None => 0.0,
                };
                points.push(Point {
                    name: metric.name.clone(),
                    value,
                    attributes: pairs(&point.attributes),
                });
            }
        }
        points
    }

    /// The sum of `name`'s points carrying every one of `attributes`.
    pub fn total(&self, name: &str, attributes: &[(&str, &str)]) -> f64 {
        self.points()
            .iter()
            .filter(|point| point.name == name)
            .filter(|point| {
                attributes
                    .iter()
                    .all(|(key, value)| point.attribute(key) == Some(*value))
            })
            .map(|point| point.value)
            .sum()
    }

    /// Every received body as text, to search for content that must not be there.
    pub fn raw(&self) -> String {
        let received = self.received.lock().unwrap();
        received
            .bodies
            .iter()
            .map(|body| String::from_utf8_lossy(body).into_owned())
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// The values of request header `name`.
    pub fn header(&self, name: &str) -> Vec<String> {
        let received = self.received.lock().unwrap();
        received
            .headers
            .iter()
            .filter_map(|headers| headers.get(name))
            .filter_map(|value| value.to_str().ok().map(str::to_string))
            .collect()
    }
}

impl Drop for Receiver {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// A span attribute as text.
pub fn attribute(span: &Span, key: &str) -> Option<String> {
    span.attributes
        .iter()
        .find(|kv| kv.key == key)
        .map(|kv| text(kv.value.as_ref()))
}

/// A log record's attributes as text.
pub fn log_attributes(record: &LogRecord) -> Vec<(String, String)> {
    pairs(&record.attributes)
}

/// A log record's body as text.
pub fn log_body(record: &LogRecord) -> String {
    text(record.body.as_ref())
}

pub fn is_child(child: &Span, parent: &Span) -> bool {
    child.parent_span_id == parent.span_id && child.trace_id == parent.trace_id
}

fn pairs(attributes: &[KeyValue]) -> Vec<(String, String)> {
    attributes
        .iter()
        .map(|kv| (kv.key.clone(), text(kv.value.as_ref())))
        .collect()
}

fn text(value: Option<&AnyValue>) -> String {
    match value.and_then(|value| value.value.as_ref()) {
        Some(any_value::Value::StringValue(text)) => text.clone(),
        Some(any_value::Value::BoolValue(value)) => value.to_string(),
        Some(any_value::Value::IntValue(value)) => value.to_string(),
        Some(any_value::Value::DoubleValue(value)) => value.to_string(),
        _ => String::new(),
    }
}

fn record<'a>(
    received: &'a Shared,
    headers: HeaderMap,
    body: &Bytes,
) -> std::sync::MutexGuard<'a, Received> {
    let mut received = received.lock().unwrap();
    received.bodies.push(body.to_vec());
    received.headers.push(headers);
    received
}

async fn traces(State(received): State<Shared>, headers: HeaderMap, body: Bytes) -> Vec<u8> {
    let request = ExportTraceServiceRequest::decode(body.as_ref()).expect("OTLP traces");
    record(&received, headers, &body).traces.push(request);
    Vec::new()
}

async fn metrics(State(received): State<Shared>, headers: HeaderMap, body: Bytes) -> Vec<u8> {
    let request = ExportMetricsServiceRequest::decode(body.as_ref()).expect("OTLP metrics");
    record(&received, headers, &body).metrics.push(request);
    Vec::new()
}

async fn logs(State(received): State<Shared>, headers: HeaderMap, body: Bytes) -> Vec<u8> {
    let request = ExportLogsServiceRequest::decode(body.as_ref()).expect("OTLP logs");
    record(&received, headers, &body).logs.push(request);
    Vec::new()
}

/// A loopback URL where nothing listens: every connection is refused.
pub async fn refused() -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    drop(listener);
    url
}

/// A collector that accepts connections and never answers.
pub struct Silent {
    pub url: String,
    task: tokio::task::JoinHandle<()>,
}

impl Silent {
    pub async fn start() -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            let mut held = Vec::new();
            while let Ok((socket, _)) = listener.accept().await {
                held.push(socket);
            }
        });
        Self { url, task }
    }
}

impl Drop for Silent {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// A collector that answers every export with `307 Temporary Redirect` to `target`,
/// which would replay the body and its headers there.
pub struct Redirecting {
    pub url: String,
    task: tokio::task::JoinHandle<()>,
}

impl Redirecting {
    pub async fn start(target: &str) -> Self {
        let target = target.to_string();
        let redirect = move |uri: axum::http::Uri| {
            let location = format!("{target}{}", uri.path());
            async move {
                (
                    axum::http::StatusCode::TEMPORARY_REDIRECT,
                    [(axum::http::header::LOCATION, location)],
                )
            }
        };
        let router = axum::Router::new().fallback(redirect);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        Self { url, task }
    }
}

impl Drop for Redirecting {
    fn drop(&mut self) {
        self.task.abort();
    }
}
