//! Real CLI synthetic command tests preserve Session state and explicit failures.
use serde_json::{Value, json};
use std::path::PathBuf;
use std::process::{Command, Output};

struct Env {
    _dir: tempfile::TempDir,
    root: PathBuf,
}
impl Env {
    fn new(hooks: Value) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        std::fs::create_dir_all(root.join("home")).unwrap();
        std::fs::create_dir_all(root.join("cyber/config")).unwrap();
        std::fs::write(
            root.join("cyber/config/cyber.jsonc"),
            json!({"hooks":hooks}).to_string(),
        )
        .unwrap();
        Self { _dir: dir, root }
    }
    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_cyber"));
        command
            .args(args)
            .env_clear()
            .env("HOME", self.root.join("home"))
            .env("CYBER_HOME", self.root.join("cyber"))
            .env("CYBER_DB", "test.db")
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .current_dir(&self.root);
        #[cfg(windows)]
        if let Some(root) = std::env::var_os("SystemRoot") {
            command.env("SystemRoot", root);
        }
        command
    }
    fn run(&self, args: &[&str]) -> Output {
        self.command(args).output().unwrap()
    }
    fn payload(&self, value: Value) -> PathBuf {
        let path = self.root.join("payload.json");
        std::fs::write(&path, value.to_string()).unwrap();
        path
    }
    fn assert_no_sessions(&self, receipts: i64) {
        let db = rusqlite::Connection::open(self.root.join("cyber/data/test.db")).unwrap();
        for table in ["session", "hook_execution"] {
            let count: i64 = db
                .query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
                    row.get(0)
                })
                .unwrap();
            assert_eq!(count, 0, "{table}");
        }
        let count: i64 = db
            .query_row("SELECT count(*) FROM hook_test_execution", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(count, receipts);
    }
}
fn body(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "{error}: stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    })
}

#[test]
fn payload_validation_rejects_nonobjects_spoofing_and_unknown_events_before_database_creation() {
    let env = Env::new(json!({}));
    for value in [
        json!([]),
        json!(null),
        json!({"session_id":"ses_live"}),
        json!({"synthetic":true}),
        json!({"location":{"directory":"/foreign"}}),
    ] {
        let path = env.payload(value);
        let output = env.run(&[
            "hooks",
            "test",
            "PreToolUse",
            "--payload",
            path.to_str().unwrap(),
            "--format",
            "json",
        ]);
        assert_eq!(output.status.code(), Some(2));
        assert!(!env.root.join("cyber/data/test.db").exists());
    }
    let output = env.run(&["hooks", "test", "UnknownEvent", "--format", "json"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(!env.root.join("cyber/data/test.db").exists());
}

#[test]
fn oversized_and_invalid_json_payloads_are_usage_errors() {
    let env = Env::new(json!({}));
    let path = env.root.join("payload.json");
    for bytes in [vec![b' '; 1024 * 1024 + 1], b"{ invalid".to_vec()] {
        std::fs::write(&path, bytes).unwrap();
        let output = env.run(&[
            "hooks",
            "test",
            "Stop",
            "--payload",
            path.to_str().unwrap(),
            "--format",
            "json",
        ]);
        assert_eq!(output.status.code(), Some(2));
        assert!(!env.root.join("cyber/data/test.db").exists());
    }
}

#[test]
fn unsupported_matching_transports_report_errors_instead_of_success_or_session_work() {
    let env = Env::new(
        json!({"Stop":[{"hooks":[{"type":"prompt","prompt":"review event","fail_closed":true}]}]}),
    );
    let output = env.run(&["hooks", "test", "Stop", "--format", "json"]);
    assert_eq!(output.status.code(), Some(1));
    let value = body(&output);
    assert_eq!(value["complete"], false);
    assert_eq!(value["results"][0]["outcome"], "error");
    assert_eq!(value["decision"]["decision"], "block");
    env.assert_no_sessions(0);
}

#[cfg(unix)]
#[test]
fn payload_matching_rewrites_and_decisions_run_without_sessions() {
    let env = Env::new(json!({"PreToolUse":[
        {"matcher":"edit","paths":["src/**"],"hooks":[{"type":"command","id":"rewrite","command":"cat >/dev/null; printf '{\"updated_input\":{\"path\":\"migrations/one.sql\"}}'"}]},
        {"matcher":"edit","paths":["migrations/**"],"hooks":[{"type":"command","id":"guard","if":{"field":"tool_input.path","matches":"one.sql$"},"command":"cat > captured.json; printf '{\"decision\":\"deny\",\"reason\":\"review required\"}'"}]},
        {"matcher":"bash","hooks":[{"type":"command","command":"touch must-not-run"}]}
    ]}));
    let path = env.payload(json!({"tool_name":"edit","tool_input":{"path":"src/main.rs"}}));
    let output = env.run(&[
        "hooks",
        "test",
        "PreToolUse",
        "--payload",
        path.to_str().unwrap(),
        "--format",
        "json",
    ]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value = body(&output);
    assert_eq!(value["results"].as_array().unwrap().len(), 2);
    assert_eq!(value["decision"]["decision"], "deny");
    assert_eq!(value["decision"]["reason"], "guard: review required");
    let captured: Value =
        serde_json::from_slice(&std::fs::read(env.root.join("captured.json")).unwrap()).unwrap();
    assert_eq!(captured["synthetic"], true);
    assert_eq!(captured["session_id"], value["invocation_id"]);
    assert_eq!(captured["tool_input"]["path"], "migrations/one.sql");
    assert!(!env.root.join("must-not-run").exists());
    env.assert_no_sessions(2);
}

#[cfg(unix)]
#[test]
fn fixed_events_run_concurrently_merge_in_order_and_deduplicate_commands() {
    let command = "cat >/dev/null; printf 'started\n' >> started; while [ $(wc -l < started) -lt 2 ]; do sleep 0.01; done; sleep 0.1; printf '{\"additional_context\":\"first\"}'";
    let env = Env::new(
        json!({"concurrency":2,"Notification":[{"matcher":"alert","hooks":[
            {"type":"command","command":command,"id":"first","timeout":3},
            {"type":"command","command":"cat >/dev/null; printf 'started\n' >> started; while [ $(wc -l < started) -lt 2 ]; do sleep 0.01; done; printf '{\"additional_context\":\"second\"}'","id":"second","timeout":3},
            {"type":"command","command":command,"id":"duplicate"}
        ]}]}),
    );
    let path = env.payload(json!({"notification_type":"alert"}));
    let output = env.run(&[
        "hooks",
        "test",
        "Notification",
        "--payload",
        path.to_str().unwrap(),
        "--format",
        "json",
    ]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value = body(&output);
    assert_eq!(value["decision"]["additional_context"], "first\nsecond");
    assert_eq!(value["results"][0]["hook_id"], "first");
    assert_eq!(value["results"][1]["hook_id"], "second");
    assert_eq!(value["results"][2]["outcome"], "skipped");
    env.assert_no_sessions(2);
}

#[cfg(unix)]
#[test]
fn file_changed_subject_and_paths_and_nonmatching_conditions_select_only_expected_handlers() {
    let env = Env::new(json!({"FileChanged":[
        {"matcher":"src/*.rs","paths":["src/**"],"hooks":[{"type":"command","command":"printf '{\"additional_context\":\"selected\"}'","if":{"field":"change","matches":"^edit$"}}]},
        {"matcher":"src/*.rs","hooks":[{"type":"command","command":"touch must-not-run","if":{"field":"change","matches":"^remove$"}}]}
    ]}));
    let path = env.payload(json!({"file_path":"src/main.rs","change":"edit"}));
    let output = env.run(&[
        "hooks",
        "test",
        "FileChanged",
        "--payload",
        path.to_str().unwrap(),
        "--format",
        "json",
    ]);
    assert!(output.status.success());
    assert_eq!(body(&output)["results"].as_array().unwrap().len(), 1);
    assert!(!env.root.join("must-not-run").exists());
    env.assert_no_sessions(1);
}

#[test]
fn withheld_and_loaded_unapproved_checkout_hooks_never_execute() {
    let env = Env::new(json!({}));
    let repo = env.root.join("repo");
    std::fs::create_dir_all(repo.join(".git")).unwrap();
    std::fs::write(repo.join("cyber.jsonc"), json!({"hooks":{"Stop":[{"hooks":[{"type":"command","command":"echo untrusted > effect"}]}]}}).to_string()).unwrap();
    let cwd = repo.to_str().unwrap();
    let withheld = env.run(&["hooks", "test", "Stop", "--cwd", cwd, "--format", "json"]);
    assert!(body(&withheld)["results"].as_array().unwrap().is_empty());
    assert!(
        !body(&withheld)["withheld_definitions"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    let report = body(&env.run(&["trust", "inspect", "--cwd", cwd, "--format", "json"]));
    assert!(
        env.run(&[
            "trust",
            "approve",
            "--cwd",
            cwd,
            "--digest",
            report["digest"].as_str().unwrap()
        ])
        .status
        .success()
    );
    let loaded = body(&env.run(&["hooks", "test", "Stop", "--cwd", cwd, "--format", "json"]));
    assert_eq!(loaded["results"][0]["outcome"], "skipped");
    assert_eq!(loaded["results"][0]["diagnostic"], "untrusted hook skipped");
    assert!(!repo.join("effect").exists());
    env.assert_no_sessions(0);
}

#[cfg(unix)]
fn wait_markers(env: &Env, names: &[&str]) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while names.iter().any(|name| !env.root.join(name).exists()) {
        assert!(std::time::Instant::now() < deadline, "hook did not start");
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

#[cfg(unix)]
#[test]
fn ctrl_c_drains_owned_siblings_and_prevents_queued_native_effects() {
    let env = Env::new(json!({"concurrency":2,"Notification":[{"hooks":[
        {"type":"command","command":"printf ready > one; (sleep 1; touch escaped-one)& wait","timeout":5},
        {"type":"command","command":"printf ready > two; (sleep 1; touch escaped-two)& wait","timeout":5},
        {"type":"command","command":"touch queued-effect"}
    ]}]}));
    let child = env
        .command(&["hooks", "test", "Notification", "--format", "json"])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    wait_markers(&env, &["one", "two"]);
    assert!(
        Command::new("kill")
            .args(["-INT", &child.id().to_string()])
            .status()
            .unwrap()
            .success()
    );
    let output = child.wait_with_output().unwrap();
    assert_eq!(output.status.code(), Some(1));
    let value = body(&output);
    assert_eq!(value["complete"], false);
    assert_eq!(value["results"].as_array().unwrap().len(), 3);
    for result in value["results"].as_array().unwrap() {
        assert_eq!(result["acknowledged"], true);
        assert_eq!(result["must_stop"], true);
    }
    std::thread::sleep(std::time::Duration::from_millis(1100));
    for name in ["queued-effect", "escaped-one", "escaped-two"] {
        assert!(!env.root.join(name).exists(), "{name}");
    }
    env.assert_no_sessions(2);
}

#[cfg(unix)]
#[test]
fn concurrency_limit_refills_a_finished_slot_while_an_earlier_handler_is_blocked() {
    let env = Env::new(json!({"concurrency":2,"Notification":[{"hooks":[
        {"type":"command","command":"touch slow; while [ ! -f release-slow ]; do sleep 0.01; done; touch slow-done","timeout":5},
        {"type":"command","command":"touch fast; while [ ! -f release-fast ]; do sleep 0.01; done","timeout":5},
        {"type":"command","command":"touch third; while [ ! -f release-third ]; do sleep 0.01; done","timeout":5},
        {"type":"command","command":"touch tail","timeout":5}
    ]}]}));
    let child = env
        .command(&["hooks", "test", "Notification", "--format", "json"])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    wait_markers(&env, &["slow", "fast"]);
    assert_absent(&env, &["third", "tail"]);
    std::fs::write(env.root.join("release-fast"), "release").unwrap();
    wait_markers(&env, &["third"]);
    assert_absent(&env, &["tail", "slow-done"]);
    std::fs::write(env.root.join("release-third"), "release").unwrap();
    wait_markers(&env, &["tail"]);
    assert_absent(&env, &["slow-done"]);
    std::fs::write(env.root.join("release-slow"), "release").unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(body(&output)["results"].as_array().unwrap().len(), 4);
    env.assert_no_sessions(4);
}

#[cfg(unix)]
fn assert_absent(env: &Env, names: &[&str]) {
    for name in names {
        assert!(!env.root.join(name).exists(), "{name}");
    }
}

#[tokio::test]
async fn http_hook_cli_posts_synthetic_event_and_prints_decision_without_sessions() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/policy", listener.local_addr().unwrap());
    let served = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut bytes = [0; 8192];
        assert!(socket.read(&mut bytes).await.unwrap() > 0);
        let body = r#"{"decision":"deny","reason":"remote CLI policy"}"#;
        socket
            .write_all(
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
                .as_bytes(),
            )
            .await
            .unwrap();
    });
    let env = Env::new(json!({"PreToolUse":[{"hooks":[{"type":"http","url":url,"id":"policy"}]}]}));
    let output = tokio::task::spawn_blocking(move || {
        let output = env.run(&["hooks", "test", "PreToolUse", "--format", "json"]);
        env.assert_no_sessions(1);
        output
    })
    .await
    .unwrap();
    served.await.unwrap();
    assert!(output.status.success());
    assert_eq!(body(&output)["results"][0]["kind"], "http");
    assert_eq!(body(&output)["decision"]["decision"], "deny");
    assert_eq!(
        body(&output)["decision"]["reason"],
        "policy: remote CLI policy"
    );
}
