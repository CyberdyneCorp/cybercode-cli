//! Real CLI observations of durable receipts without launch or recovery side effects.
#[path = "../../cyber-server/tests/support/mod.rs"]
mod support;
use cyber_core::{
    config::{self, LoadRequest},
    hooks::{HookCatalog, HookEvent, HookIdentity, HookLocation, HookOutcome},
    paths::Paths,
};
use cyber_server::runtime::{HookExecutionIo, HookExecutionResult};
use serde_json::{Value, json};
use std::process::{Command, Output};
use support::{Harness, Setup};

fn invoke(h: &Harness, args: &[&str]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_cyber"));
    command
        .args(["hooks", "history"])
        .args(args)
        .env_clear()
        .env("HOME", h.dir.path())
        .env("CYBER_HOME", h.dir.path().join("cli-home"))
        .env("CYBER_DB", h.dir.path().join("cyber.db"))
        .current_dir(&h.repo);
    #[cfg(windows)]
    if let Some(system) = std::env::var_os("SystemRoot") {
        command.env("SystemRoot", system);
    }
    command.output().unwrap()
}
fn page(h: &Harness, args: &[&str]) -> Value {
    let mut args = args.to_vec();
    args.extend(["--format", "json"]);
    let output = invoke(h, &args);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}
fn count(h: &Harness) -> i64 {
    h.store
        .read(|conn| Ok(conn.query_row("SELECT count(*) FROM event", [], |row| row.get(0))?))
        .unwrap()
}
fn result() -> HookExecutionResult {
    HookExecutionResult {
        outcome: HookOutcome::Ok,
        decision: Default::default(),
        acknowledged: true,
        must_stop: false,
        io: Some(HookExecutionIo {
            stdin: "private input".into(),
            stdout: "private output".into(),
            stderr: "private error".into(),
            truncated: false,
        }),
    }
}
#[tokio::test]
async fn history_pages_receipts_without_loading_config_or_reconciling_owners() {
    let h = Harness::new(Setup::default());
    let session = h.session().await;
    let other = h.session().await;
    let empty = h.session().await;
    let env = std::collections::HashMap::from([(
        "CYBER_HOME".into(),
        h.dir.path().join("definitions").display().to_string(),
    )]);
    let paths = Paths::resolve(&env, h.dir.path());
    paths.ensure().unwrap();
    std::fs::write(paths.config.join("cyber.jsonc"),json!({"hooks":{"PreToolUse":[{"hooks":[{"type":"command","command":"echo never-launched","id":"guard"}]}]}}).to_string()).unwrap();
    let resolved = config::load(&LoadRequest {
        location: &h.repo,
        paths: &paths,
        env: &env,
        home: h.dir.path(),
        profile: None,
        overrides: &[],
        flags: json!({}),
    })
    .unwrap();
    let definition = HookCatalog::from_config(&resolved)
        .unwrap()
        .definitions
        .remove(0);
    let event = |id: &str| {
        HookEvent::new(
            "PreToolUse",
            HookIdentity {
                session_id: id.into(),
                location: HookLocation { directory: h.repo.clone(), workspace: None },
                project_id: "global".into(), agent: "build".into(), mode: "default".into(),
            },
            1,
            json!({"call_id":"call_history","tool_name":"read","tool_input":{"private":"private input"}})
                .as_object().unwrap().clone(),
        ).unwrap()
    };
    let masked = h
        .runtime
        .start_hook_execution(&event(&session), &definition, false)
        .await
        .unwrap()
        .finish(result())
        .unwrap();
    let opted = h
        .runtime
        .start_hook_execution(&event(&session), &definition, true)
        .await
        .unwrap()
        .finish(result())
        .unwrap();
    let unknown = h
        .runtime
        .start_hook_execution(&event(&session), &definition, false)
        .await
        .unwrap();
    let unknown_id = unknown.record().id.clone();
    drop(unknown);
    let running = h
        .runtime
        .start_hook_execution(&event(&session), &definition, false)
        .await
        .unwrap();
    let running_id = running.record().id.clone();
    h.runtime
        .start_hook_execution(&event(&other), &definition, false)
        .await
        .unwrap()
        .finish(result())
        .unwrap();
    let cli_paths = Paths::resolve(
        &std::collections::HashMap::from([(
            "CYBER_HOME".into(),
            h.dir.path().join("cli-home").display().to_string(),
        )]),
        h.dir.path(),
    );
    cli_paths.ensure().unwrap();
    std::fs::write(
        cli_paths.config.join("cyber.jsonc"),
        "invalid configuration",
    )
    .unwrap();
    let before = count(&h);
    let empty_page = page(&h, &["--session", &empty]);
    assert_eq!(empty_page["data"], json!([]));
    assert!(empty_page["cursor"]["next"].is_null());
    let mut all = Vec::new();
    let mut cursor: Option<String> = None;
    let mut first_cursor = String::new();
    loop {
        let mut args = vec!["--session", session.as_str(), "--limit", "1"];
        if let Some(cursor) = &cursor {
            args.extend(["--cursor", cursor]);
        }
        let value = page(&h, &args);
        all.extend(value["data"].as_array().unwrap().clone());
        cursor = value["cursor"]["next"].as_str().map(str::to_owned);
        if first_cursor.is_empty() {
            first_cursor = cursor.clone().unwrap();
        }
        if cursor.is_none() {
            break;
        }
        assert!(all.len() < 5);
    }
    assert_eq!(all.len(), 4);
    assert!(all.windows(2).all(|rows| (
        rows[0]["started_ms"].as_i64().unwrap(),
        rows[0]["id"].as_str().unwrap()
    ) > (
        rows[1]["started_ms"].as_i64().unwrap(),
        rows[1]["id"].as_str().unwrap()
    )));
    let by_id = |id: &str| all.iter().find(|row| row["id"] == id).unwrap();
    assert!(by_id(&masked.id).get("io").is_none());
    assert_eq!(by_id(&masked.id)["call_id"], "call_history");
    assert_eq!(by_id(&opted.id)["io"]["stdout"], "private output");
    assert_eq!(by_id(&unknown_id)["status"], "unknown");
    assert_eq!(by_id(&running_id)["status"], "running");
    assert!(all.iter().all(|row| row["session_id"] == session));
    let text = invoke(&h, &["--session", &session]);
    assert!(text.status.success());
    let text = String::from_utf8(text.stdout).unwrap();
    assert!(text.contains("live state unverified") && text.contains("recovery required"));
    assert!(!text.contains("private output"));
    for args in [
        vec!["--session", &session, "--cursor", "malformed"],
        vec!["--session", &other, "--cursor", &first_cursor],
        vec!["--session", &session, "--limit", "0"],
        vec!["--session", &session, "--limit", "501"],
    ] {
        let output = invoke(&h, &args);
        assert_eq!(output.status.code(), Some(2));
        assert!(output.stdout.is_empty());
    }
    let missing = invoke(&h, &["--session", "ses_missing"]);
    assert_eq!(missing.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&missing.stderr).contains("SessionNotFoundError"));
    assert_eq!(
        count(&h),
        before,
        "history must not commit recovery or run hooks"
    );
    assert!(h.models.requests("test/main").is_empty());
    running.finish(result()).unwrap();
}
