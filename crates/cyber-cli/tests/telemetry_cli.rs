//! `cyber telemetry show` through the real binary against a real `cyber serve`
//! (`cli-commands` → Telemetry status command, `observability-costs` → OpenTelemetry
//! configuration). The server's environment is the one that exports, so the `OTEL_*`
//! variables go to `cyber serve` only. Every server is stopped by its PID on drop. Nothing
//! here reaches the network: exports go to in-process collectors on `127.0.0.1` and Turns
//! to the local scripted model.

#[path = "../../cyber-app/tests/support/scripted_model.rs"]
mod scripted_model;

use std::net::TcpListener;
use std::path::PathBuf;
use std::process::{Child, Command, Output, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

/// A header value that must never be printed.
const SECRET: &str = "Bearer s3cr3t-t0ken-9f";

struct Env {
    _dir: tempfile::TempDir,
    root: PathBuf,
    repo: PathBuf,
}

/// A `cyber serve --register` of an [`Env`], killed by its PID on drop.
struct Server(Child);

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

impl Server {
    /// Wait until the stopped server exits; it flushes telemetry before it does.
    fn exited(&mut self) {
        let deadline = Instant::now() + Duration::from_secs(30);
        while self.0.try_wait().unwrap().is_none() {
            assert!(Instant::now() < deadline, "the server did not exit");
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

impl Env {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let repo = root.join("repo");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        std::fs::create_dir_all(root.join("cyber-home/config")).unwrap();
        // An empty catalog: nothing is fetched.
        std::fs::write(root.join("models.json"), "{}").unwrap();
        Self {
            _dir: dir,
            root,
            repo,
        }
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_cyber"));
        command
            .args(args)
            .env_clear()
            .env("HOME", self.root.join("home"))
            .env("CYBER_HOME", self.root.join("cyber-home"))
            .env("CYBER_MANAGED_POLICY_DIR", self.root.join("managed"))
            .env("CYBER_MODELS_PATH", self.root.join("models.json"))
            .env("PATH", std::env::var("PATH").unwrap_or_default())
            .current_dir(&self.repo)
            .stdin(Stdio::null());
        #[cfg(windows)]
        if let Some(system) = std::env::var_os("SystemRoot") {
            command.env("SystemRoot", system);
        }
        command
    }

    fn cyber(&self, args: &[&str]) -> Output {
        self.command(args).output().unwrap()
    }

    fn global_config(&self, value: &Value) -> PathBuf {
        let path = self.root.join("cyber-home/config/cyber.jsonc");
        std::fs::write(&path, value.to_string()).unwrap();
        path
    }

    /// A registered server whose environment adds `vars`.
    fn serve(&self, vars: &[(&str, &str)]) -> Server {
        let mut command = self.command(&["serve", "--register", "--port", "0"]);
        command.envs(vars.iter().copied());
        let server = Server(
            command
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap(),
        );
        let registration = self.root.join("cyber-home/state/server.json");
        let deadline = Instant::now() + Duration::from_secs(30);
        while !registration.is_file() {
            assert!(Instant::now() < deadline, "no server registered");
            std::thread::sleep(Duration::from_millis(20));
        }
        server
    }

    fn show_json(&self) -> Value {
        let output = self.cyber(&["telemetry", "show", "--format", "json"]);
        assert_eq!(output.status.code(), Some(0), "{output:?}");
        serde_json::from_slice(&output.stdout)
            .unwrap_or_else(|e| panic!("{e}: {}", String::from_utf8_lossy(&output.stdout)))
    }
}

fn text(output: &Output) -> String {
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    String::from_utf8_lossy(&output.stdout).into_owned()
}

/// Scenario "Masked headers and sources".
#[test]
fn show_masks_headers_and_names_sources() {
    let env = Env::new();
    let config = env.global_config(&json!({"telemetry": {"otel": {
        "enabled": true,
        "endpoint": "http://127.0.0.1:4318",
        "headers": {"authorization": SECRET},
    }}}));
    let _server = env.serve(&[("OTEL_EXPORTER_OTLP_PROTOCOL", "http/json")]);
    let layer = format!("config (global:{})", config.display());

    let output = env.cyber(&["telemetry", "show"]);
    let shown = text(&output);
    assert!(!shown.contains("s3cr3t"), "{shown}");
    assert!(shown.contains("OpenTelemetry export: on"), "{shown}");
    assert!(
        shown.contains(&format!("headers  authorization=***  [{layer}]")),
        "{shown}"
    );
    assert!(
        shown.contains("protocol http/json  [env (OTEL_EXPORTER_OTLP_PROTOCOL)]"),
        "{shown}"
    );
    assert!(
        shown.contains(&format!(
            "endpoint http://127.0.0.1:4318/v1/traces  [{layer}]"
        )),
        "{shown}"
    );

    let output = env.cyber(&["telemetry", "show", "--format", "json"]);
    assert!(!String::from_utf8_lossy(&output.stdout).contains("s3cr3t"));
    let status: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(status["enabled"], true, "{status}");
    for signal in ["traces", "metrics", "logs"] {
        let signal = &status["signals"][signal];
        assert_eq!(
            signal["headers"],
            json!([{"name": "authorization", "value": "***"}])
        );
        assert_eq!(signal["sources"]["headers"], layer.as_str());
        assert_eq!(signal["protocol"], "http/json");
        assert_eq!(
            signal["sources"]["protocol"],
            "env (OTEL_EXPORTER_OTLP_PROTOCOL)"
        );
        assert_eq!(signal["sources"]["timeout"], "default");
    }
}

/// Every signal is off, with zero counters and no last error.
fn assert_signals_off_with_zero_counters(status: &Value) {
    for signal in ["traces", "metrics", "logs"] {
        let signal = &status["signals"][signal];
        assert_eq!(signal["enabled"], false);
        for counter in ["exported", "failed", "dropped"] {
            assert_eq!(signal[counter], 0, "{signal}");
        }
        assert!(signal.get("last_error").is_none());
    }
}

/// Scenario "Export off by default", then a configuration change the running server
/// reports as needing a restart.
#[test]
fn show_reports_export_off_by_default() {
    let env = Env::new();
    let _server = env.serve(&[]);
    let status = env.show_json();
    assert_eq!(status["enabled"], false, "{status}");
    assert_eq!(status["reason"], "telemetry.otel.enabled is not true");
    assert_eq!(
        status["log_prompts"],
        json!({"value": false, "source": "default"})
    );
    assert_eq!(status["dropped_events"], 0);
    assert_eq!(status["product"]["enabled"], false);
    assert_eq!(status["restart_required"], false);
    assert_eq!(status["resource"]["service.name"], "cyber");
    assert_signals_off_with_zero_counters(&status);
    assert_eq!(
        status["signals"]["metrics"]["endpoint"],
        "http://localhost:4318/v1/metrics"
    );
    let shown = text(&env.cyber(&["telemetry", "show"]));
    assert!(
        shown.contains("OpenTelemetry export: off (telemetry.otel.enabled is not true)"),
        "{shown}"
    );

    env.global_config(&json!({"telemetry": {"otel": {"enabled": true}}}));
    let status = env.show_json();
    assert_eq!(status["enabled"], false, "the running settings stay");
    assert_eq!(status["restart_required"], true, "{status}");
}

/// Scenario "Endpoint variable alone does not enable export".
#[test]
fn endpoint_env_alone_does_not_enable_export() {
    let env = Env::new();
    let collector = TcpListener::bind("127.0.0.1:0").unwrap();
    collector.set_nonblocking(true).unwrap();
    let endpoint = format!("http://{}", collector.local_addr().unwrap());
    let _server = env.serve(&[("OTEL_EXPORTER_OTLP_ENDPOINT", &endpoint)]);
    let status = env.show_json();
    assert_eq!(status["enabled"], false, "{status}");
    assert_eq!(status["reason"], "telemetry.otel.enabled is not true");
    let traces = &status["signals"]["traces"];
    assert_eq!(traces["endpoint"], format!("{endpoint}/v1/traces"));
    assert_eq!(
        traces["sources"]["endpoint"],
        "env (OTEL_EXPORTER_OTLP_ENDPOINT)"
    );
    assert!(
        matches!(collector.accept(), Err(e) if e.kind() == std::io::ErrorKind::WouldBlock),
        "nothing connected to the endpoint"
    );
}

/// Scenario "SDK disabled".
#[test]
fn sdk_disabled_wins_over_config() {
    let env = Env::new();
    env.global_config(&json!({"telemetry": {"otel": {"enabled": true}}}));
    let _server = env.serve(&[("OTEL_SDK_DISABLED", "true")]);
    let status = env.show_json();
    assert_eq!(status["enabled"], false, "{status}");
    assert_eq!(status["reason"], "OTEL_SDK_DISABLED=true");
    let shown = text(&env.cyber(&["telemetry", "show"]));
    assert!(shown.contains("off (OTEL_SDK_DISABLED=true)"), "{shown}");
}

/// Exporters the SDK cannot build keep export off, and `show` says so with the reason
/// instead of reporting export on: here a non-ASCII host, which the settings accept as a
/// URL and the SDK's URI parser rejects. Neither it nor the header value is shown.
#[test]
fn exporter_build_failure_is_reported_as_off() {
    let env = Env::new();
    env.global_config(&json!({"telemetry": {"otel": {
        "enabled": true,
        "endpoint": "https://collectör.example:4318",
        "headers": {"authorization": SECRET},
    }}}));
    let _server = env.serve(&[]);
    let status = env.show_json();
    assert_eq!(status["enabled"], false, "{status}");
    let reason = status["reason"].as_str().unwrap_or_default();
    assert!(reason.contains("exporter could not be built"), "{reason}");
    assert!(!reason.contains("collectör"), "{reason}");
    assert!(!status.to_string().contains("s3cr3t"), "{status}");
    for signal in ["traces", "metrics", "logs"] {
        assert_eq!(status["signals"][signal]["enabled"], false, "{status}");
    }
    let shown = text(&env.cyber(&["telemetry", "show"]));
    assert!(
        shown.contains("OpenTelemetry export: off (the traces exporter could not be built"),
        "{shown}"
    );
}

/// `OTEL_EXPORTER_OTLP_COMPRESSION=gzip`, a common machine-wide setting, never turns
/// export off: the SDK ignores what this build cannot honor.
#[test]
fn compression_variable_keeps_export_on() {
    let env = Env::new();
    // Nothing listens there: no export reaches another collector on this machine.
    let endpoint = {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        format!("http://{}", listener.local_addr().unwrap())
    };
    env.global_config(&json!({"telemetry": {"otel": {"enabled": true, "endpoint": endpoint}}}));
    let _server = env.serve(&[("OTEL_EXPORTER_OTLP_COMPRESSION", "gzip")]);
    let status = env.show_json();
    assert_eq!(status["enabled"], true, "{status}");
}

/// Scenario "Project cannot enable prompt logging", in a trusted checkout.
#[test]
fn project_telemetry_is_ignored_and_listed() {
    let env = Env::new();
    let project = env.repo.join(".cyber/cyber.jsonc");
    std::fs::create_dir_all(project.parent().unwrap()).unwrap();
    std::fs::write(
        &project,
        json!({"telemetry": {
            "log_prompts": true,
            "otel": {"enabled": true, "endpoint": "https://collector.example"},
        }})
        .to_string(),
    )
    .unwrap();
    let inspected: Value =
        serde_json::from_slice(&env.cyber(&["trust", "inspect", "--format", "json"]).stdout)
            .unwrap();
    let digest = inspected["digest"]
        .as_str()
        .expect("telemetry is sensitive");
    let approved = env.cyber(&["trust", "approve", "--digest", digest]);
    assert!(approved.status.success(), "{approved:?}");
    let _server = env.serve(&[]);

    let status = env.show_json();
    assert_eq!(status["enabled"], false, "{status}");
    assert_eq!(status["reason"], "telemetry.otel.enabled is not true");
    assert_eq!(
        status["log_prompts"],
        json!({"value": false, "source": "default"})
    );
    let source = format!("project:{}", project.display());
    assert_eq!(
        status["ignored"],
        json!([
            {"pointer": "/telemetry/otel/enabled", "source": source},
            {"pointer": "/telemetry/otel/endpoint", "source": source},
            {"pointer": "/telemetry/log_prompts", "source": source},
        ])
    );
    let shown = text(&env.cyber(&["telemetry", "show"]));
    assert!(
        shown.contains(&format!("/telemetry/log_prompts  {source}")),
        "{shown}"
    );
}

/// A server exporting to `endpoint` over `protocol` with the scripted model: run one Turn
/// with `cyber exec`, then stop the server gracefully so it flushes.
fn export_one_turn(endpoint: &str, protocol: &str) {
    let model = scripted_model::ScriptedModel::start(|_| json!({"content": "ok"}));
    let env = Env::new();
    let mut config = model.config();
    config["telemetry"] =
        json!({"otel": {"enabled": true, "endpoint": endpoint, "protocol": protocol}});
    env.global_config(&config);
    let mut server = env.serve(&[]);
    assert_eq!(env.show_json()["signals"]["traces"]["protocol"], protocol);
    let output = env.cyber(&["exec", "--format", "json", "hello"]);
    assert!(output.status.success(), "{output:?}");
    let stopped = env.cyber(&["service", "stop"]);
    assert!(stopped.status.success(), "{stopped:?}");
    server.exited();
}

/// Received OTLP/HTTP JSON bodies by signal path, with their content type.
type JsonBodies = Arc<Mutex<Vec<(String, String, Value)>>>;

async fn receive_json(
    axum::extract::State(bodies): axum::extract::State<JsonBodies>,
    axum::extract::Path(signal): axum::extract::Path<String>,
    headers: axum::http::HeaderMap,
    body: axum::body::Bytes,
) -> (
    [(axum::http::header::HeaderName, &'static str); 1],
    &'static str,
) {
    let content_type = headers
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    bodies.lock().unwrap().push((signal, content_type, value));
    (
        [(axum::http::header::CONTENT_TYPE, "application/json")],
        "{}",
    )
}

/// The span names of OTLP/JSON trace bodies.
fn json_span_names(bodies: &[(String, String, Value)]) -> Vec<String> {
    bodies
        .iter()
        .filter(|(signal, _, _)| signal == "traces")
        .flat_map(|(_, _, body)| {
            body["resourceSpans"]
                .as_array()
                .cloned()
                .unwrap_or_default()
        })
        .flat_map(|resource| {
            resource["scopeSpans"]
                .as_array()
                .cloned()
                .unwrap_or_default()
        })
        .flat_map(|scope| scope["spans"].as_array().cloned().unwrap_or_default())
        .filter_map(|span| span["name"].as_str().map(str::to_string))
        .collect()
}

/// `http/json` through the real binary: spans and metrics reach the collector as OTLP
/// JSON.
#[test]
fn http_json_export_reaches_receiver() {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let bodies = JsonBodies::default();
    let listener = runtime
        .block_on(tokio::net::TcpListener::bind("127.0.0.1:0"))
        .unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let router = axum::Router::new()
        .route("/v1/{signal}", axum::routing::post(receive_json))
        .with_state(Arc::clone(&bodies));
    runtime.spawn(async move { axum::serve(listener, router).await });

    export_one_turn(&endpoint, "http/json");

    let bodies = bodies.lock().unwrap().clone();
    let names = json_span_names(&bodies);
    for name in ["cyber.session", "cyber.turn"] {
        assert!(names.iter().any(|n| n == name), "{names:?}");
    }
    assert!(
        bodies.iter().any(|(signal, _, _)| signal == "metrics"),
        "no metrics export"
    );
    assert!(
        bodies
            .iter()
            .all(|(_, content_type, _)| content_type.starts_with("application/json")),
        "{bodies:?}"
    );
}

/// `grpc` through the real binary: a local tonic OTLP trace service receives the spans.
#[cfg(feature = "otlp-grpc")]
#[test]
fn grpc_export_reaches_receiver() {
    use opentelemetry_proto::tonic::collector::trace::v1::trace_service_server::TraceServiceServer;
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let collector = grpc::Collector::default();
    let listener = runtime
        .block_on(tokio::net::TcpListener::bind("127.0.0.1:0"))
        .unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let service = TraceServiceServer::new(collector.clone());
    runtime.spawn(
        tonic::transport::Server::builder()
            .add_service(service)
            .serve_with_incoming(tokio_stream::wrappers::TcpListenerStream::new(listener)),
    );

    export_one_turn(&endpoint, "grpc");

    let names = collector.0.lock().unwrap().clone();
    for name in ["cyber.session", "cyber.turn"] {
        assert!(names.iter().any(|n| n == name), "{names:?}");
    }
}

#[cfg(feature = "otlp-grpc")]
mod grpc {
    use std::sync::{Arc, Mutex};

    use opentelemetry_proto::tonic::collector::trace::v1::trace_service_server::TraceService;
    use opentelemetry_proto::tonic::collector::trace::v1::{
        ExportTraceServiceRequest, ExportTraceServiceResponse,
    };

    /// The span names received.
    #[derive(Clone, Default)]
    pub struct Collector(pub Arc<Mutex<Vec<String>>>);

    #[tonic::async_trait]
    impl TraceService for Collector {
        async fn export(
            &self,
            request: tonic::Request<ExportTraceServiceRequest>,
        ) -> Result<tonic::Response<ExportTraceServiceResponse>, tonic::Status> {
            let names = request
                .into_inner()
                .resource_spans
                .into_iter()
                .flat_map(|resource| resource.scope_spans)
                .flat_map(|scope| scope.spans)
                .map(|span| span.name);
            self.0.lock().unwrap().extend(names);
            Ok(tonic::Response::new(ExportTraceServiceResponse::default()))
        }
    }
}
