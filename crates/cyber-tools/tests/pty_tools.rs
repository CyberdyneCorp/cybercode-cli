//! `pty_start`, `pty_write` and `pty_read` through real dispatch and a runtime-owned PTY
//! Job (`background-tasks` → Model interaction with PTYs, PTY sessions). Unix only: the
//! tools are absent on Windows.
#![cfg(unix)]
mod support;
use std::time::{Duration, Instant};

use cyber_server::runtime::{Job, JobStatus, PendingKind, PermissionReply, ToolHost, ToolOutcome};
use serde_json::{Value, json};
use support::flow::{Flow, call as call_step, text};
use tokio_util::sync::CancellationToken;

fn flow(config: Value) -> Flow {
    let flow = Flow::new(Vec::new(), false);
    flow.f.set_config(config);
    flow
}

fn allowed() -> Value {
    json!({"permissions": {"bash": "allow", "external_directory": "allow"}})
}

async fn call(flow: &Flow, session: &str, name: &str, input: Value) -> ToolOutcome {
    let mut inv = flow.f.invocation("default", name, input);
    inv.session_id = session.into();
    inv.call_id = cyber_core::ids::new_id("call");
    flow.f.host.execute(inv, CancellationToken::new()).await
}

async fn json_of(flow: &Flow, session: &str, name: &str, input: Value) -> Value {
    let output = support::ok(call(flow, session, name, input).await);
    serde_json::from_str(&output).expect(&output)
}

/// Read until `want` shows up in the output, as a model polling with pty_read would.
async fn read_until(flow: &Flow, session: &str, job: &str, want: &str) -> String {
    let mut seen = String::new();
    let deadline = Instant::now() + Duration::from_secs(30);
    while !seen.contains(want) {
        assert!(Instant::now() < deadline, "no {want:?} in {seen:?}");
        let read = json_of(
            flow,
            session,
            "pty_read",
            json!({"job_id": job, "wait_ms": 2000}),
        )
        .await;
        seen.push_str(read["output"].as_str().unwrap());
    }
    seen
}

async fn settled(flow: &Flow, id: &str) -> Job {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let job = flow.runtime.job(id).unwrap();
        if job.status != JobStatus::Running {
            return job;
        }
        assert!(Instant::now() < deadline, "the PTY Job did not end");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test]
async fn pty_drives_python_repl_print_2_plus_2() {
    // An interactive interpreter is unresolved source to the removal guard, so the model's
    // pty_start asks even under an allow rule; the person approves it once.
    let start = json!({"command": "python3 -q", "description": "REPL"});
    let flow = Flow::new(
        vec![call_step("c1", "pty_start", start), text("started")],
        true,
    );
    flow.f.set_config(allowed());
    let session = flow.session("default").await;
    flow.prompt(&session, "open a python REPL").await;
    let request = flow.pending(&session).await;
    let PendingKind::Permission(ask) = &request.kind else {
        panic!("expected a permission request");
    };
    assert_eq!(ask.action, "bash");
    flow.runtime
        .reply_permission(&request.id, PermissionReply::Once)
        .await
        .unwrap();
    flow.settle(&session).await;
    let output = flow.output(&session, "c1").await;
    let started: Value = serde_json::from_str(&output).expect(&output);
    let id = started["job_id"].as_str().unwrap().to_string();
    let mut stable = started.clone();
    stable["job_id"] = json!("<job>");
    stable["output_path"] = json!("<data>/jobs/<job>.log");
    support::golden(&flow.f, "pty_start", &stable.to_string());
    let job = flow.runtime.job(&id).unwrap();
    assert_eq!((job.kind.as_str(), job.status), ("pty", JobStatus::Running));
    assert_eq!(job.command.as_deref(), Some("python3 -q"));
    read_until(&flow, &session, &id, ">>> ").await;

    let wrote = json_of(
        &flow,
        &session,
        "pty_write",
        json!({"job_id": id, "input": "print(2+2)\n"}),
    )
    .await;
    assert_eq!(wrote["written"], 11);
    let mut stable = wrote.clone();
    stable["job_id"] = json!("<job>");
    support::golden(&flow.f, "pty_write", &stable.to_string());
    let output = read_until(&flow, &session, &id, "4\n").await;
    assert!(output.contains("print(2+2)\n4\n"), "{output:?}");

    // The terminal is announced to the program.
    let env = "import os; print(os.environ['TERM'], os.environ['CYBER_TERMINAL'], os.isatty(0))\n";
    support::ok(
        call(
            &flow,
            &session,
            "pty_write",
            json!({"job_id": id, "input": env}),
        )
        .await,
    );
    read_until(&flow, &session, &id, "xterm-256color 1 True").await;
    let screen = json_of(
        &flow,
        &session,
        "pty_read",
        json!({"job_id": id, "mode": "screen", "wait_ms": 0}),
    )
    .await;
    assert!(
        screen["screen"]
            .as_str()
            .unwrap()
            .contains(">>> print(2+2)\n4"),
        "{screen}"
    );

    // Another Session cannot reach this PTY.
    let other = flow.session("default").await;
    let refused = support::failed(call(&flow, &other, "pty_read", json!({"job_id": id})).await);
    assert_eq!(refused, format!("No PTY {id} in this session"));

    // ctrl-c interrupts a running statement; the exit code is the program's.
    let busy = json!({"job_id": id, "input": "import time; time.sleep(60)\n"});
    support::ok(call(&flow, &session, "pty_write", busy).await);
    tokio::time::sleep(Duration::from_millis(300)).await;
    support::ok(
        call(
            &flow,
            &session,
            "pty_write",
            json!({"job_id": id, "keys": ["ctrl-c"]}),
        )
        .await,
    );
    read_until(&flow, &session, &id, "KeyboardInterrupt").await;
    support::ok(
        call(
            &flow,
            &session,
            "pty_write",
            json!({"job_id": id, "input": "exit(0)\n"}),
        )
        .await,
    );
    let ended = settled(&flow, &id).await;
    assert_eq!(
        (ended.status, ended.exit_code),
        (JobStatus::Completed, Some(0)),
        "{ended:?}"
    );
    let gone = support::failed(call(&flow, &session, "pty_read", json!({"job_id": id})).await);
    assert!(gone.contains("has exited"), "{gone}");
    let log = std::fs::read_to_string(started["output_path"].as_str().unwrap()).unwrap();
    assert!(log.contains("print(2+2)"), "{log}");
}

#[tokio::test]
async fn pty_start_needs_bash_permission_and_ends_on_eof_or_task_stop() {
    let denied = flow(json!({"permissions": {"bash": {"*": "allow", "python3*": "deny"}}}));
    let session = denied.session("default").await;
    let refused = support::failed(
        call(
            &denied,
            &session,
            "pty_start",
            json!({"command": "python3"}),
        )
        .await,
    );
    assert!(
        refused.contains("denied") || refused.contains("Denied"),
        "{refused}"
    );
    assert!(denied.runtime.jobs(Some(&session)).unwrap().is_empty());

    let hidden = flow(json!({"permissions": {"bash": "deny"}}));
    let names = hidden.f.tool_names("default", false);
    assert!(
        !names.iter().any(|name| name.starts_with("pty_")),
        "{names:?}"
    );

    let flow = flow(allowed());
    let session = flow.session("default").await;
    let started = json_of(&flow, &session, "pty_start", json!({"command": "cat"})).await;
    let id = started["job_id"].as_str().unwrap();
    support::ok(
        call(
            &flow,
            &session,
            "pty_write",
            json!({"job_id": id, "input": "echo me\n"}),
        )
        .await,
    );
    read_until(&flow, &session, id, "echo me\necho me").await;
    let again = json!({"job_id": id, "cursor": 0, "wait_ms": 0});
    let mut read = json_of(&flow, &session, "pty_read", again).await;
    read["job_id"] = json!("<job>");
    support::golden(&flow.f, "pty_read", &read.to_string());
    support::ok(
        call(
            &flow,
            &session,
            "pty_write",
            json!({"job_id": id, "keys": ["ctrl-d"]}),
        )
        .await,
    );
    let ended = settled(&flow, id).await;
    assert_eq!(
        (ended.status, ended.exit_code),
        (JobStatus::Completed, Some(0))
    );
    assert!(flow.runtime.pty(id).is_none());

    let started = json_of(&flow, &session, "pty_start", json!({"command": "cat"})).await;
    let id = started["job_id"].as_str().unwrap();
    support::ok(call(&flow, &session, "task_stop", json!({"job_id": id})).await);
    assert_eq!(settled(&flow, id).await.status, JobStatus::Cancelled);
    assert!(flow.runtime.pty(id).is_none());
}

async fn call_in(flow: &Flow, session: &str, mode: &str, name: &str, input: Value) -> ToolOutcome {
    let mut inv = flow.f.invocation(mode, name, input);
    inv.session_id = session.into();
    inv.call_id = cyber_core::ids::new_id("call");
    flow.f.host.execute(inv, CancellationToken::new()).await
}

/// Regression: a line typed into a shell PTY is authorized as a `bash` command, so a
/// `git push` deny rule refuses it (also when typed in pieces) and nothing is written.
#[tokio::test]
async fn pty_write_into_a_shell_applies_command_deny_rules() {
    let flow = flow(
        json!({"permissions": {"bash": {"*": "allow", "git push*": "deny"}, "external_directory": "allow"}}),
    );
    let session = flow.session("default").await;
    let started = json_of(&flow, &session, "pty_start", json!({})).await;
    let id = started["job_id"].as_str().unwrap();
    let write = |input: &str| json!({"job_id": id, "input": input});
    let refused =
        support::failed(call(&flow, &session, "pty_write", write("git push --force\r")).await);
    assert!(refused.to_lowercase().contains("denied"), "{refused}");
    support::ok(call(&flow, &session, "pty_write", write("git pu")).await);
    let refused = support::failed(call(&flow, &session, "pty_write", write("sh\r")).await);
    assert!(refused.to_lowercase().contains("denied"), "{refused}");
    let history = support::failed(
        call(
            &flow,
            &session,
            "pty_write",
            json!({"job_id": id, "keys": ["up", "enter"]}),
        )
        .await,
    );
    assert!(history.contains("typed lines only"), "{history}");
    support::ok(
        call(
            &flow,
            &session,
            "pty_write",
            write("\u{15}echo typed-$((40+2))\r"),
        )
        .await,
    );
    read_until(&flow, &session, id, "typed-42").await;
    let log = std::fs::read_to_string(started["output_path"].as_str().unwrap()).unwrap();
    assert!(!log.contains("push"), "{log}");
    support::ok(call(&flow, &session, "task_stop", json!({"job_id": id})).await);
}

/// Regression (`permissions-modes` → bypass mode): the critical-path removal guard
/// refuses `rm -rf` of the checkout typed into a shell PTY, as it does for `bash`.
#[tokio::test]
async fn bypass_shell_pty_refuses_rm_rf_of_the_git_toplevel() {
    let flow = flow(allowed());
    flow.f.write("keep.txt", "preserve me");
    let session = flow.session("bypass").await;
    let started = serde_json::from_str::<Value>(&support::ok(
        call_in(&flow, &session, "bypass", "pty_start", json!({})).await,
    ))
    .unwrap();
    let id = started["job_id"].as_str().unwrap();
    let input = json!({"job_id": id, "input": "rm -rf \"$(git rev-parse --show-toplevel)\"\r"});
    let refused = support::failed(call_in(&flow, &session, "bypass", "pty_write", input).await);
    assert!(refused.starts_with("Refused: removal"), "{refused}");
    assert_eq!(flow.f.read("keep.txt"), "preserve me");
    support::ok(
        call_in(
            &flow,
            &session,
            "bypass",
            "task_stop",
            json!({"job_id": id}),
        )
        .await,
    );
}

/// Regression (`permissions-modes` → bypass mode): a line continued with a backslash is
/// authorized joined to the next one, as the shell runs it, so `rm -rf \` + `.` meets the
/// critical-path removal guard, in one write or two, and nothing reaches the shell.
#[tokio::test]
async fn bypass_shell_pty_joins_continued_lines_before_the_removal_guard() {
    let flow = flow(allowed());
    flow.f.write("keep.txt", "preserve me");
    let session = flow.session("bypass").await;
    let started = serde_json::from_str::<Value>(&support::ok(
        call_in(&flow, &session, "bypass", "pty_start", json!({})).await,
    ))
    .unwrap();
    let id = started["job_id"].as_str().unwrap();
    let write = |input: &str| json!({"job_id": id, "input": input});
    let refused = support::failed(
        call_in(
            &flow,
            &session,
            "bypass",
            "pty_write",
            write("rm -rf \\\r.\r"),
        )
        .await,
    );
    assert!(refused.starts_with("Refused: removal"), "{refused}");
    support::ok(call_in(&flow, &session, "bypass", "pty_write", write("rm -rf \\\r")).await);
    let refused =
        support::failed(call_in(&flow, &session, "bypass", "pty_write", write(".\r")).await);
    assert!(refused.starts_with("Refused: removal"), "{refused}");
    // The shell still reads on: finish the command harmlessly.
    support::ok(
        call_in(
            &flow,
            &session,
            "bypass",
            "pty_write",
            write("missing.txt\r"),
        )
        .await,
    );
    // An open quote the shell reads on through must close in the same write.
    let open =
        support::failed(call_in(&flow, &session, "bypass", "pty_write", write("echo \"a\r")).await);
    assert!(open.contains("open"), "{open}");
    let alias = support::failed(
        call_in(
            &flow,
            &session,
            "bypass",
            "pty_write",
            write("alias x='rm -rf .'\r"),
        )
        .await,
    );
    assert!(alias.contains("Aliases"), "{alias}");
    support::ok(
        call_in(
            &flow,
            &session,
            "bypass",
            "pty_write",
            write("echo \"joined-\r$((40+2))\"\r"),
        )
        .await,
    );
    read_until(&flow, &session, id, "joined-\n42").await;
    assert_eq!(flow.f.read("keep.txt"), "preserve me");
    let log = std::fs::read_to_string(started["output_path"].as_str().unwrap()).unwrap();
    assert!(!log.contains("alias"), "{log}");
    support::ok(
        call_in(
            &flow,
            &session,
            "bypass",
            "task_stop",
            json!({"job_id": id}),
        )
        .await,
    );
}

/// Regression: `git \` + `push` is the command `git push`, so a `git push*` deny rule
/// refuses it.
#[tokio::test]
async fn pty_write_joins_continued_lines_before_deny_rules() {
    let flow = flow(
        json!({"permissions": {"bash": {"*": "allow", "git push*": "deny"}, "external_directory": "allow"}}),
    );
    let session = flow.session("default").await;
    let started = json_of(&flow, &session, "pty_start", json!({})).await;
    let id = started["job_id"].as_str().unwrap();
    let input = json!({"job_id": id, "input": "git \\\rpush --force\r"});
    let refused = support::failed(call(&flow, &session, "pty_write", input).await);
    assert!(refused.to_lowercase().contains("denied"), "{refused}");
    let log = std::fs::read_to_string(started["output_path"].as_str().unwrap()).unwrap();
    assert!(!log.contains("push"), "{log}");
    support::ok(call(&flow, &session, "task_stop", json!({"job_id": id})).await);
}
