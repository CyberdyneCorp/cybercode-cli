//! Native service shutdown while built-in shells own live descendants.
#![cfg(windows)]
#![allow(unsafe_code)]

use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use cyber_app::{App, AppOptions, ServeOptions};
use cyber_core::paths::{DatabaseLocation, Paths};
use cyber_llm::adapters::{ScriptStep, ScriptedAdapter};
use cyber_llm::{FinishReason, LlmEvent, LlmRequest, ToolCall};
use cyber_server::runtime::{
    Admission, CallStatus, CompactionConfig, CreateSession, Delivery, ModelResolver, NoSnapshots,
    PendingKind, PermissionReply, ResolvedModel, Runtime, RuntimeOptions,
};
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

fn model(tool: &str) -> Arc<Model> {
    let command = if tool == "powershell" {
        "& './service child.exe' --exact native_service_worker --nocapture"
    } else {
        "./'service child.exe' --exact native_service_worker --nocapture"
    };
    let input = serde_json::json!({"command": command, "timeout_ms": 60000});
    Arc::new(Model(Arc::new(ScriptedAdapter::new(vec![vec![
        ScriptStep::Event(LlmEvent::ToolCallDone(ToolCall {
            id: "service_call".into(),
            name: tool.into(),
            arguments: input.to_string(),
            input: Some(input),
        })),
        ScriptStep::Event(LlmEvent::Finish {
            reason: FinishReason::ToolCalls,
        }),
    ]]))))
}

async fn application(root: &Path, model: Arc<Model>) -> App {
    let paths = Paths {
        data: root.join("data"),
        config: root.join("config"),
        state: root.join("state"),
        cache: root.join("cache"),
        tmp: root.join("tmp"),
    };
    paths.ensure().unwrap();
    std::fs::create_dir(root.join(".git")).unwrap();
    std::fs::write(
        paths.config.join("cyber.json"),
        serde_json::json!({
            "shell": bash(), "permissions": {"bash": "allow"}, "compat": {"claude": false}
        })
        .to_string(),
    )
    .unwrap();
    std::fs::write(paths.state.join("password"), "test-password-123456").unwrap();
    let mut app = App::build(AppOptions {
        database: DatabaseLocation::File(paths.data.join("cyber.db")),
        paths,
        home: root.join("home"),
        default_directory: root.to_path_buf(),
        sandbox_policy: Some("full-access".into()),
        snapshots: false,
        interactive: true,
        password: Some("test-password-123456".into()),
    })
    .await
    .unwrap();
    app.runtime.shutdown().await;
    app.runtime = runtime(&app, model);
    app.state.runtime = app.runtime.clone();
    app
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
    let model = model(tool);
    let app = application(root, model.clone()).await;
    let paths = app.paths.clone();
    let running = app.runtime.clone();
    let reloaded = runtime(&app, model.clone());
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
            model: "test/scripted".into(),
            title: Some("Active native service shutdown".into()),
            ..Default::default()
        })
        .await
        .unwrap()
        .id;
    running
        .admit(&id, Admission::text("run the worker", Delivery::Queue))
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
    assert!(!running.is_running(&id));
    assert_eq!(
        reloaded.state(&id).await.unwrap().calls["service_call"].status,
        CallStatus::OutcomeUnknown
    );
    assert!(cyber_app::read_registration(&paths).is_none());
    assert_eq!(
        model.0.requests().len(),
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
