//! Native service shutdown while built-in shells own live descendants.
#![cfg(windows)]
#![allow(unsafe_code)]

use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

#[path = "support/scripted_model.rs"]
mod scripted_model;

use cyber_app::{App, AppOptions, ServeOptions};
use cyber_core::paths::{DatabaseLocation, Paths};
use cyber_llm::LlmRequest;
use cyber_llm::adapters::ScriptedAdapter;
use cyber_server::runtime::{
    Admission, CallStatus, CompactionConfig, CreateSession, Delivery, ModelResolver, NoSnapshots,
    PendingKind, PermissionReply, ResolvedModel, Runtime, RuntimeOptions, StartWorkflowRun,
    WorkflowRunStatus,
};
use scripted_model::ScriptedModel;
use serde_json::{Value, json};
use tokio::sync::Notify;
use windows_sys::Win32::Foundation::WAIT_OBJECT_0;
use windows_sys::Win32::System::Threading::{
    OpenProcess, PROCESS_SYNCHRONIZE, PROCESS_TERMINATE, TerminateProcess, WaitForSingleObject,
};

struct Model(Arc<ScriptedAdapter>);

impl ModelResolver for Model {
    fn resolve(&self, _: &str) -> Result<ResolvedModel, String> {
        Ok(ResolvedModel {
            adapter: self.0.clone(),
            template: LlmRequest {
                model: "scripted".into(),
                ..Default::default()
            },
            provider: "test".into(),
            model: "scripted".into(),
            context_limit: 200_000,
            cost: None,
            prefers_apply_patch: false,
        })
    }

    fn role(&self, _: cyber_llm::catalog::ModelRole) -> Option<String> {
        None
    }
}

fn runtime(app: &App, model: Arc<Model>) -> Runtime {
    Runtime::new(RuntimeOptions {
        store: app.store.clone(),
        resolver: model,
        tools: app.host.clone(),
        global_config_dir: app.paths.config.clone(),
        shell: bash().display().to_string(),
        claude_compat: false,
        compaction: CompactionConfig {
            auto: false,
            ..Default::default()
        },
        retry: cyber_llm::RetryPolicy::default(),
        max_steps: None,
        today: None,
        interactive: true,
        snapshots: Arc::new(NoSnapshots),
    })
}

fn bash() -> PathBuf {
    let path = PathBuf::from(std::env::var_os("ProgramFiles").unwrap()).join("Git/bin/bash.exe");
    assert!(
        path.is_file(),
        "native service test requires installed Git Bash"
    );
    path
}

/// A resolver for the runtime that only reads durable state back; it is never asked for a
/// Turn, so its script is empty.
fn reader() -> Arc<Model> {
    Arc::new(Model(Arc::new(ScriptedAdapter::new(vec![]))))
}

/// The provider answer: a native shell call that starts the worker, with the tool named
/// in the prompt (`run the worker with <tool>`); a title request is answered plainly.
fn reply(body: &Value) -> Value {
    let text = body.to_string();
    if text.contains("Write a short title") {
        return json!({"content": "Native service shutdown"});
    }
    if text.contains("wait at the model") {
        return json!({"gate": "workflow agent"});
    }
    let (tool, command) = if text.contains("worker with powershell") {
        (
            "powershell",
            "& './service child.exe' --exact native_service_worker --nocapture",
        )
    } else {
        (
            "bash",
            "./'service child.exe' --exact native_service_worker --nocapture",
        )
    };
    let arguments = json!({"command": command, "timeout_ms": 60000}).to_string();
    json!({"tool_calls":[{"index":0,"id":"service_call","type":"function",
        "function":{"name":tool,"arguments":arguments}}]})
}

/// Turn requests the provider received, excluding Session title requests.
fn turn_requests(model: &ScriptedModel) -> usize {
    model
        .requests()
        .iter()
        .filter(|body| !body.to_string().contains("Write a short title"))
        .count()
}

fn options(root: &Path) -> AppOptions {
    let paths = Paths {
        data: root.join("data"),
        config: root.join("config"),
        state: root.join("state"),
        cache: root.join("cache"),
        tmp: root.join("tmp"),
    };
    AppOptions {
        database: DatabaseLocation::File(paths.data.join("cyber.db")),
        paths,
        home: root.join("home"),
        default_directory: root.to_path_buf(),
        sandbox_policy: Some("full-access".into()),
        snapshots: false,
        interactive: true,
        password: Some("test-password-123456".into()),
        mcp_credentials: None,
        remote_skill_fetcher: None,
        workflow_cores: None,
    }
}

/// The production App over the scripted provider. The runtime is the one `App::build` made:
/// replacing it would mean shutting it down, which closes the shared tool host (language
/// services, MCP) that a replacement would reuse.
async fn application(root: &Path, model: &ScriptedModel) -> App {
    let paths = Paths {
        data: root.join("data"),
        config: root.join("config"),
        state: root.join("state"),
        cache: root.join("cache"),
        tmp: root.join("tmp"),
    };
    paths.ensure().unwrap();
    std::fs::create_dir(root.join(".git")).unwrap();
    let mut config = model.config();
    config["shell"] = json!(bash());
    config["permissions"] = json!({"bash": "allow"});
    config["compat"] = json!({"claude": false});
    std::fs::write(paths.config.join("cyber.json"), config.to_string()).unwrap();
    std::fs::write(paths.state.join("password"), "test-password-123456").unwrap();
    App::build(options(root)).await.unwrap()
}

struct Server {
    stop: Arc<Notify>,
    task: tokio::task::JoinHandle<Result<(), String>>,
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.notify_waiters();
        self.stop.notify_one();
    }
}

struct Worker(OwnedHandle);

impl Worker {
    fn assert_stopped(&self) {
        assert_eq!(
            unsafe { WaitForSingleObject(self.0.as_raw_handle(), 1000) },
            WAIT_OBJECT_0
        );
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        // Failure cleanup targets only this retained process identity, never a registration PID.
        if unsafe { WaitForSingleObject(self.0.as_raw_handle(), 0) } != WAIT_OBJECT_0 {
            unsafe { TerminateProcess(self.0.as_raw_handle(), 1) };
            unsafe { WaitForSingleObject(self.0.as_raw_handle(), 1000) };
        }
    }
}

async fn worker(root: &Path, name: &str, server: &Server) -> Worker {
    let marker = root.join(name);
    tokio::time::timeout(Duration::from_secs(10), async {
        while !marker.is_file() {
            assert!(
                !server.task.is_finished(),
                "service stopped before its worker started"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("native tool did not publish its worker identity");
    let pid = std::fs::read_to_string(marker).unwrap().parse().unwrap();
    let raw = unsafe { OpenProcess(PROCESS_SYNCHRONIZE | PROCESS_TERMINATE, 0, pid) };
    assert!(!raw.is_null(), "{}", std::io::Error::last_os_error());
    let worker = Worker(unsafe { OwnedHandle::from_raw_handle(raw) });
    assert_ne!(
        unsafe { WaitForSingleObject(worker.0.as_raw_handle(), 0) },
        WAIT_OBJECT_0
    );
    worker
}

/// The native service stop runs the same shutdown hook as `cyber serve`: a running
/// Workflow Run is durably paused before the tool host closes, and an App rebuilt over the
/// same database (startup recovery) keeps it paused for a resume.
#[tokio::test]
async fn service_stop_pauses_running_workflow_runs_and_a_rebuilt_app_keeps_them_paused() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let model = ScriptedModel::start(reply);
    let app = application(root, &model).await;
    let paths = app.paths.clone();
    let running = app.runtime.clone();
    let stop = Arc::new(Notify::new());
    let mut server = Server {
        stop: stop.clone(),
        task: tokio::spawn(cyber_app::run_server_until(
            app,
            ServeOptions {
                port: Some(0),
                register: true,
                ..Default::default()
            },
            |_| {},
            stop,
        )),
    };
    let registration = registration(&paths).await;
    let owner = running
        .create_session(CreateSession {
            directory: root.display().to_string(),
            model: "test/main".into(),
            title: Some("Workflow service shutdown".into()),
            ..Default::default()
        })
        .await
        .unwrap()
        .id;
    let script = "export const meta = { name: 'service', description: 'Service shutdown' };\nexport default async () => (await agent('wait at the model')).text;\n";
    let run = running
        .start_workflow_run(StartWorkflowRun {
            module: cyber_workflow::WorkflowModule::inline(script).unwrap(),
            args: serde_json::json!({}),
            session_id: owner,
            limits: cyber_workflow::Limits::default(),
            tiers: Default::default(),
            budget: cyber_core::budget::RunBudget::resolve(&serde_json::json!({}), &[]).unwrap(),
            share: Default::default(),
            starter: Default::default(),
        })
        .await
        .unwrap();
    // The agent started and its child's request waits at the provider: unsettled.
    tokio::time::timeout(Duration::from_secs(30), async {
        while model.held().is_empty() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the agent did not reach the model");
    assert_eq!(
        cyber_app::stop_service(&paths).await.unwrap(),
        Some(registration)
    );
    tokio::time::timeout(Duration::from_secs(10), &mut server.task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let paused = running.workflow_run(&run.id).unwrap().unwrap();
    assert_eq!(paused.status, WorkflowRunStatus::Paused, "{paused:?}");
    assert_eq!(
        running.workflow_agents(&run.id).unwrap()[0].status,
        "interrupted"
    );

    let rebuilt = App::build(options(root)).await.unwrap();
    let recovered = rebuilt.runtime.workflow_run(&run.id).unwrap().unwrap();
    assert_eq!(recovered.status, WorkflowRunStatus::Paused);
    assert!(!rebuilt.runtime.workflow_agents_exiting(&run.id).unwrap());
    rebuilt.runtime.shutdown().await;
}

#[tokio::test]
async fn shutdown_of_active_native_shells_settles_calls_and_terminates_descendants() {
    for tool in ["bash", "powershell"] {
        service_shutdown_case(tool).await;
    }
}

async fn service_shutdown_case(tool: &str) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    for name in ["service child.exe", "service grandchild.exe"] {
        std::fs::copy(std::env::current_exe().unwrap(), root.join(name)).unwrap();
    }
    let model = ScriptedModel::start(reply);
    let app = application(root, &model).await;
    let paths = app.paths.clone();
    let running = app.runtime.clone();
    let reloaded = runtime(&app, reader());
    let stop = Arc::new(Notify::new());
    let mut server = Server {
        stop: stop.clone(),
        task: tokio::spawn(cyber_app::run_server_until(
            app,
            ServeOptions {
                port: Some(0),
                register: true,
                ..Default::default()
            },
            |_| {},
            stop,
        )),
    };
    let registration = registration(&paths).await;
    let id = running
        .create_session(CreateSession {
            directory: root.display().to_string(),
            model: "test/main".into(),
            title: Some("Active native service shutdown".into()),
            ..Default::default()
        })
        .await
        .unwrap()
        .id;
    running
        .admit(
            &id,
            Admission::text(format!("run the worker with {tool}"), Delivery::Queue),
        )
        .await
        .unwrap();
    if tool == "powershell" {
        approve_native_command(&running, &id).await;
    }
    let child = worker(root, "child.pid", &server).await;
    let grandchild = worker(root, "grandchild.pid", &server).await;
    assert_eq!(
        running.state(&id).await.unwrap().calls["service_call"].status,
        CallStatus::Dispatched
    );
    stop_and_verify(&mut server, &paths, registration, &child, &grandchild).await;
    verify_settlement(&running, &reloaded, &paths, &id, &model).await;
}

async fn verify_settlement(
    running: &Runtime,
    reloaded: &Runtime,
    paths: &Paths,
    id: &str,
    model: &ScriptedModel,
) {
    assert!(!running.is_running(id));
    assert_eq!(
        reloaded.state(id).await.unwrap().calls["service_call"].status,
        CallStatus::OutcomeUnknown
    );
    assert!(cyber_app::read_registration(paths).is_none());
    assert_eq!(
        turn_requests(model),
        1,
        "shutdown started another provider turn"
    );
}

async fn registration(paths: &Paths) -> cyber_app::Registration {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let Some(registration) = cyber_app::read_registration(paths) {
                break registration;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap()
}

async fn stop_and_verify(
    server: &mut Server,
    paths: &Paths,
    registration: cyber_app::Registration,
    child: &Worker,
    grandchild: &Worker,
) {
    let started = Instant::now();
    assert_eq!(
        cyber_app::stop_service(paths).await.unwrap(),
        Some(registration)
    );
    tokio::time::timeout(Duration::from_secs(2), &mut server.task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    child.assert_stopped();
    grandchild.assert_stopped();
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "complete service stop exceeded its deadline"
    );
}

async fn approve_native_command(runtime: &Runtime, id: &str) {
    let pending = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Some(pending) = runtime.pending_requests(Some(id)).into_iter().next() {
                break pending;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("native command did not request individual approval");
    let PendingKind::Permission(ask) = pending.kind else {
        panic!("expected command permission")
    };
    assert_eq!(ask.action, "bash");
    assert_eq!(ask.metadata["requires_confirmation"], true);
    runtime
        .reply_permission(&pending.id, PermissionReply::Once)
        .await
        .unwrap();
}

fn publish(root: &Path, name: &str) {
    let temporary = root.join(format!("{name}.tmp"));
    std::fs::write(&temporary, std::process::id().to_string()).unwrap();
    std::fs::rename(temporary, root.join(name)).unwrap();
}

#[test]
#[allow(clippy::zombie_processes)] // Descendants deliberately remain for the server-owned job.
fn native_service_worker() {
    let executable = std::env::current_exe().unwrap();
    let root = std::env::current_dir().unwrap();
    let name = executable.file_name().unwrap().to_string_lossy();
    if name == "service grandchild.exe" {
        publish(&root, "grandchild.pid");
        std::thread::sleep(Duration::from_secs(60));
    } else if name == "service child.exe" {
        publish(&root, "child.pid");
        let _child = std::process::Command::new(root.join("service grandchild.exe"))
            .args(["--exact", "native_service_worker", "--nocapture"])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        std::thread::sleep(Duration::from_secs(60));
    }
}
