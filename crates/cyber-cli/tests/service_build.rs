//! `server-api` → Background service management: a registered server of another build is
//! replaced when it is idle and runs this install's executable, and kept with a warning
//! otherwise. The old server is a stub; the replacement is the real `cyber serve`.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use serde_json::{Value, json};

const RESTARTED: &str = "restarted the Cyber server: it was running an older build";
const KEPT: &str = "The Cyber server runs an older build";

struct Env {
    _dir: tempfile::TempDir,
    root: PathBuf,
}

impl Env {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        Self { _dir: dir, root }
    }

    fn state(&self) -> PathBuf {
        self.root.join("cyber-home/state")
    }

    fn cyber(&self, args: &[&str]) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_cyber"));
        command
            .args(args)
            .env_clear()
            .env("HOME", self.root.join("home"))
            .env("CYBER_HOME", self.root.join("cyber-home"))
            .env("PATH", std::env::var("PATH").unwrap_or_default())
            .current_dir(&self.root);
        #[cfg(windows)]
        if let Some(system) = std::env::var_os("SystemRoot") {
            command.env("SystemRoot", system);
        }
        command.output().unwrap()
    }
}

/// How the stub old server answers.
struct Old {
    /// `GET /service` counts, or no route at all (a build that predates it).
    running: Option<usize>,
    /// The executable the registration names.
    binary: String,
}

/// A stub server of an older build, registered in `env`; returns its URL and whether it
/// was asked to stop.
fn register_old(
    env: &Env,
    runtime: &tokio::runtime::Runtime,
    old: Old,
) -> (String, Arc<AtomicBool>) {
    let listener = runtime
        .block_on(tokio::net::TcpListener::bind("127.0.0.1:0"))
        .unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let stopped = Arc::new(AtomicBool::new(false));
    let registration = env.state().join("server.json");
    let mut router = axum::Router::new().route(
        "/api/v1/health",
        axum::routing::get(|| async {
            axum::Json(json!({"healthy":true,"version":cyber_app::version(),"build":"older-build"}))
        }),
    );
    if let Some(running) = old.running {
        router = router.route(
            "/api/v1/service",
            axum::routing::get(move || async move {
                axum::Json(json!({"id":"srv_old","version":cyber_app::version(),
                    "build":"older-build","pid":1,"binary":null,
                    "running_sessions":running,"active_workflow_runs":0}))
            }),
        );
    }
    let flag = stopped.clone();
    let file = registration.clone();
    router = router.route(
        "/api/v1/service/stop",
        axum::routing::post(move || async move {
            flag.store(true, Ordering::SeqCst);
            let _ = std::fs::remove_file(&file);
            axum::Json(json!({"id":"srv_old","stopping":true}))
        }),
    );
    runtime.spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    std::fs::create_dir_all(env.state()).unwrap();
    std::fs::write(env.state().join("password"), "old-build-server-password").unwrap();
    std::fs::write(
        &registration,
        json!({"id":"srv_old","version":cyber_app::version(),"url":url,"socket":null,
            "pid":1,"build":"older-build","binary":old.binary,"binary_stamp":"0-0"})
        .to_string(),
    )
    .unwrap();
    (url, stopped)
}

fn this_binary() -> String {
    Path::new(env!("CARGO_BIN_EXE_cyber"))
        .canonicalize()
        .unwrap()
        .display()
        .to_string()
}

fn started(output: &Output) -> (Value, String) {
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    assert!(output.status.success(), "{stderr}");
    (serde_json::from_slice(&output.stdout).unwrap(), stderr)
}

#[test]
fn an_idle_server_of_an_older_build_is_replaced_by_this_build() {
    let env = Env::new();
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let (old_url, stopped) = register_old(
        &env,
        &runtime,
        Old {
            running: Some(0),
            binary: this_binary(),
        },
    );
    let (report, stderr) = started(&env.cyber(&["service", "start", "--format", "json"]));
    assert!(
        stopped.load(Ordering::SeqCst),
        "the old server was not stopped"
    );
    assert!(stderr.contains(RESTARTED), "{stderr}");
    assert_ne!(report["url"], old_url.as_str(), "{report}");
    let registration: Value =
        serde_json::from_slice(&std::fs::read(env.state().join("server.json")).unwrap()).unwrap();
    assert_eq!(
        registration["build"],
        cyber_core::version::build_info().build,
        "{registration}"
    );
    assert_eq!(registration["binary"], this_binary());
    // The new server is current: starting again reuses it without a notice.
    let (again, stderr) = started(&env.cyber(&["service", "start", "--format", "json"]));
    assert_eq!(again["url"], report["url"]);
    assert!(!stderr.contains(RESTARTED), "{stderr}");
    let stop = env.cyber(&["service", "stop", "--format", "json"]);
    assert!(
        stop.status.success(),
        "{}",
        String::from_utf8_lossy(&stop.stderr)
    );
}

#[test]
fn a_busy_server_of_an_older_build_is_kept_with_a_warning() {
    let env = Env::new();
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let (old_url, stopped) = register_old(
        &env,
        &runtime,
        Old {
            running: Some(1),
            binary: this_binary(),
        },
    );
    let (report, stderr) = started(&env.cyber(&["service", "start", "--format", "json"]));
    assert!(!stopped.load(Ordering::SeqCst), "a busy server was stopped");
    assert!(stderr.contains(KEPT), "{stderr}");
    assert!(stderr.contains("cyber service restart"), "{stderr}");
    assert_eq!(report["url"], old_url.as_str(), "{report}");
    let status = env.cyber(&["service", "status", "--format", "json"]);
    let (status, _) = started(&status);
    assert_eq!(status["stale"], true, "{status}");
}

#[test]
fn an_older_server_of_another_install_or_without_activity_is_never_stopped() {
    for old in [
        Old {
            running: Some(0),
            binary: "/elsewhere/bin/cyber".into(),
        },
        Old {
            running: None,
            binary: this_binary(),
        },
    ] {
        let env = Env::new();
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let (old_url, stopped) = register_old(&env, &runtime, old);
        let (report, stderr) = started(&env.cyber(&["service", "start", "--format", "json"]));
        assert!(!stopped.load(Ordering::SeqCst));
        assert!(stderr.contains(KEPT), "{stderr}");
        assert_eq!(report["url"], old_url.as_str(), "{report}");
    }
}

/// A killed server's registration can name a port another Cyber server (another user or
/// install, or a parallel test) took over. That server is current and healthy, but its
/// password differs, so reusing it fails every call with `UnauthorizedError`. The client
/// must notice that a different instance answers and start its own server.
#[test]
fn a_registration_whose_port_another_server_took_over_is_not_reused() {
    let env = Env::new();
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let listener = runtime
        .block_on(tokio::net::TcpListener::bind("127.0.0.1:0"))
        .unwrap();
    let foreign_url = format!("http://{}", listener.local_addr().unwrap());
    let stopped = Arc::new(AtomicBool::new(false));
    let flag = stopped.clone();
    let router = axum::Router::new()
        .route(
            "/api/v1/health",
            axum::routing::get(|| async {
                axum::Json(json!({"healthy":true,"version":cyber_app::version(),
                    "build":cyber_core::version::build_info().build,"id":"srv_foreign"}))
            }),
        )
        .route(
            "/api/v1/service/stop",
            axum::routing::post(move || async move {
                flag.store(true, Ordering::SeqCst);
                axum::Json(json!({"id":"srv_foreign","stopping":true}))
            }),
        );
    runtime.spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    std::fs::create_dir_all(env.state()).unwrap();
    std::fs::write(
        env.state().join("server.json"),
        json!({"id":"srv_killed","version":cyber_app::version(),"url":foreign_url,
            "socket":null,"pid":1,"build":cyber_core::version::build_info().build})
        .to_string(),
    )
    .unwrap();
    let (report, stderr) = started(&env.cyber(&["service", "start", "--format", "json"]));
    assert_ne!(report["url"], foreign_url.as_str(), "{report} {stderr}");
    assert!(
        !stopped.load(Ordering::SeqCst),
        "another server must never be stopped"
    );
    let registration: Value =
        serde_json::from_slice(&std::fs::read(env.state().join("server.json")).unwrap()).unwrap();
    assert_ne!(registration["id"], "srv_killed", "{registration}");
    // The next authenticated call reaches the new server with this home's password.
    let list = env.cyber(&["sessions", "list", "--format", "json"]);
    assert!(
        list.status.success(),
        "{}",
        String::from_utf8_lossy(&list.stderr)
    );
    let stop = env.cyber(&["service", "stop", "--format", "json"]);
    assert!(
        stop.status.success(),
        "{}",
        String::from_utf8_lossy(&stop.stderr)
    );
}
