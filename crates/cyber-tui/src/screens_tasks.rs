//! Background task controls against a production App over HTTP (`background-tasks` →
//! Tasks view, Background commands from the user; `tui` → Background tasks view): `/tasks`
//! lists commands and monitors with their last output line, Enter opens a log, `x` stops
//! one; `& cmd` starts a background Job without the model; Ctrl+B moves a running
//! foreground `bash` call to the background.
#![cfg(unix)]

use std::time::Duration;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use serde_json::{Value, json};

use crate::app::{Action, App, Overlay, PickerKind};
use crate::screens::{Opts, Server, render};

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn typed(app: &mut App, text: &str) -> Vec<Action> {
    for c in text.chars() {
        app.on_key(key(KeyCode::Char(c)));
    }
    app.on_key(key(KeyCode::Enter))
}

/// Run `actions` through the runner's own `perform` and `apply`.
async fn perform(server: &Server, app: &mut App, actions: Vec<Action>) {
    for action in actions {
        let msg = crate::perform::perform(&server.client, &app.session, action).await;
        let follow = crate::runner::apply(app, msg, &mut Default::default());
        Box::pin(perform(server, app, follow)).await;
    }
}

fn reply(body: &Value) -> Value {
    if body.to_string().contains("Write a short title") {
        return json!({ "content": crate::screens::TITLE });
    }
    let last = body["messages"]
        .as_array()
        .and_then(|messages| messages.last())
        .cloned()
        .unwrap_or_default();
    let asked = |word: &str| last["role"] == "user" && last["content"].to_string().contains(word);
    let call = |name: &str, arguments: Value| {
        json!({ "tool_calls": [{ "index": 0, "id": format!("call_{name}"), "type": "function",
            "function": { "name": name, "arguments": arguments.to_string() } }] })
    };
    if asked("START-MONITOR") {
        return call("monitor", json!({"path": "app.log", "filter": "ERROR"}));
    }
    if asked("RUN-FOREGROUND") {
        return call(
            "bash",
            json!({"command": "echo fg-started; sleep 300", "timeout_ms": 600000}),
        );
    }
    json!({ "content": "ok" })
}

/// A server whose commands run without prompts and without the sandbox helper.
async fn server() -> Server {
    Server::start_reply(
        reply,
        json!({"main": {}}),
        json!({"permissions": {"bash": "allow"}, "sandbox": {"policy": "full-access"}}),
        false,
    )
    .await
}

async fn eventually<T>(what: &str, mut probe: impl AsyncFnMut() -> Option<T>) -> T {
    for _ in 0..1500 {
        if let Some(value) = probe().await {
            return value;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("timed out waiting for {what}");
}

async fn jobs(server: &Server, id: &str) -> Vec<Value> {
    let listed = server
        .client
        .get(&format!("/jobs?session_id={id}"))
        .await
        .unwrap();
    listed["data"].as_array().cloned().unwrap_or_default()
}

fn log(job: &Value) -> String {
    std::fs::read_to_string(job["output_path"].as_str().unwrap()).unwrap_or_default()
}

async fn stop(server: &Server, job: &str) {
    server
        .client
        .post(&format!("/jobs/{job}/stop"), json!({}))
        .await
        .unwrap();
}

/// `& cmd` starts the command as a background Job of the Session: no model request, no
/// message, and the composer shows the background shell mode.
#[tokio::test]
async fn ampersand_shell_command_starts_background_job_without_model() {
    let server = server().await;
    let id = server.answered("first question").await;
    let mut app = server.load(&id).await;
    let requests = server.model.requests().len();
    let messages = app.items.len();
    for c in "& echo bg-started; sleep 300".chars() {
        app.on_key(key(KeyCode::Char(c)));
    }
    let screen = render(&mut app, 100, 30, &Opts::default());
    assert!(screen.contains("background shell"), "{screen}");
    let actions = app.on_key(key(KeyCode::Enter));
    assert_eq!(
        actions,
        [Action::BackgroundShell("echo bg-started; sleep 300".into())]
    );
    perform(&server, &mut app, actions).await;
    let job = eventually("the background Job", async || {
        jobs(&server, &id).await.into_iter().next()
    })
    .await;
    let job_id = job["id"].as_str().unwrap().to_owned();
    assert_eq!(
        app.toast.as_ref().unwrap().0,
        format!("Started {job_id} in the background · /tasks")
    );
    assert_eq!(job["kind"], "bash");
    assert_eq!(job["status"], "running");
    assert_eq!(job["command"], "echo bg-started; sleep 300");
    eventually("the command's output", async || {
        log(&job).contains("bg-started").then_some(())
    })
    .await;
    assert_eq!(
        server.model.requests().len(),
        requests,
        "the model was asked"
    );
    assert!(!server.host.runtime.is_running(&id));
    perform(&server, &mut app, vec![Action::Refresh]).await;
    assert_eq!(app.items.len(), messages, "a message was recorded");
    stop(&server, &job_id).await;
}

/// `/tasks` lists a background command and a monitor with kind, status and last output
/// line; Enter opens the command's log; `x` stops it, it is listed `cancelled`, and its
/// notice reaches the model.
#[tokio::test]
async fn tasks_stop_with_x_marks_job_cancelled_and_notifies_model() {
    let server = server().await;
    let id = server.answered("START-MONITOR").await;
    let mut app = server.load(&id).await;
    perform(
        &server,
        &mut app,
        vec![Action::BackgroundShell("echo bg-line; sleep 300".into())],
    )
    .await;
    let command = eventually("the command's output", async || {
        let jobs = jobs(&server, &id).await;
        let job = jobs.into_iter().find(|job| job["kind"] == "bash")?;
        log(&job).contains("bg-line").then_some(job)
    })
    .await;
    let command_id = command["id"].as_str().unwrap().to_owned();

    let actions = typed(&mut app, "/tasks");
    assert_eq!(actions, [Action::LoadTasks]);
    perform(&server, &mut app, actions).await;
    let Overlay::Picker(picker) = &app.overlay else {
        panic!("{:?}", app.overlay)
    };
    assert_eq!(picker.kind, PickerKind::Tasks);
    let screen = render(&mut app, 120, 30, &Opts::default());
    assert!(screen.contains("x stops"), "{screen}");
    assert!(screen.contains("tail -F app.log (running)"), "{screen}");
    assert!(screen.contains("monitor · "), "{screen}");
    assert!(
        screen.contains("echo bg-line; sleep 300 (running)"),
        "{screen}"
    );
    assert!(screen.contains("bg-line"), "{screen}");

    let select = |app: &mut App| {
        let Overlay::Picker(picker) = &mut app.overlay else {
            panic!("{:?}", app.overlay)
        };
        picker.selected = picker
            .filtered
            .iter()
            .position(|&i| picker.items[i].key == command_id)
            .unwrap();
    };
    select(&mut app);
    let actions = app.on_key(key(KeyCode::Enter));
    assert_eq!(actions, [Action::OpenTask(command_id.clone())]);
    perform(&server, &mut app, actions).await;
    let Overlay::TaskLog { title, text, .. } = &app.overlay else {
        panic!("{:?}", app.overlay)
    };
    assert!(title.starts_with(&command_id), "{title}");
    assert_eq!(text, "bg-line\n");
    app.on_key(key(KeyCode::Esc));
    assert_eq!(app.overlay, Overlay::None);

    perform(&server, &mut app, vec![Action::LoadTasks]).await;
    select(&mut app);
    let actions = app.on_key(key(KeyCode::Char('x')));
    assert_eq!(actions, [Action::StopTask(command_id.clone())]);
    perform(&server, &mut app, actions).await;
    let job = server
        .client
        .get(&format!("/jobs/{command_id}"))
        .await
        .unwrap();
    assert_eq!(job["data"]["status"], "cancelled");
    let screen = render(&mut app, 120, 30, &Opts::default());
    assert!(
        screen.contains("echo bg-line; sleep 300 (cancelled)"),
        "{screen}"
    );
    eventually("the notice to reach the model", async || {
        server
            .model
            .requests()
            .iter()
            .any(|body| {
                let text = body["messages"].to_string();
                text.contains("Background job ended") && text.contains(&command_id)
            })
            .then_some(())
    })
    .await;
    let monitor = jobs(&server, &id)
        .await
        .into_iter()
        .find(|job| job["kind"] == "monitor")
        .unwrap();
    stop(&server, monitor["id"].as_str().unwrap()).await;
}

/// Ctrl+B while a foreground `bash` call runs: the process keeps running as a
/// background Job, the call settles with the Job's ID and the Turn continues.
#[tokio::test]
async fn ctrl_b_moves_foreground_bash_to_background_job() {
    let server = server().await;
    let id = server.answered("first question").await;
    let mut app = server.load(&id).await;
    let ctrl_b = KeyEvent::new(KeyCode::Char('b'), KeyModifiers::CONTROL);
    assert!(app.on_key(ctrl_b).is_empty(), "nothing runs yet");
    assert_eq!(app.toast.as_ref().unwrap().0, "No command is running");
    server.prompt(&id, "RUN-FOREGROUND").await;
    eventually("the foreground command", async || {
        let state = server.host.runtime.state(&id).await.unwrap();
        state.calls.contains_key("call_bash").then_some(())
    })
    .await;
    perform(&server, &mut app, vec![Action::Refresh]).await;
    assert!(app.session.running);
    let actions = app.on_key(ctrl_b);
    assert_eq!(actions, [Action::MoveToBackground]);
    perform(&server, &mut app, actions).await;
    assert_eq!(
        app.toast.as_ref().unwrap().0,
        "Moved the running command to the background"
    );
    // The call settles with the Job, and the Turn goes on to answer.
    let output: Value = eventually("the call to settle", async || {
        let state = server.host.runtime.state(&id).await.unwrap();
        let output = state.calls["call_bash"].output.clone()?;
        serde_json::from_str(&output).ok()
    })
    .await;
    let job_id = output["job_id"].as_str().unwrap().to_owned();
    eventually("the Turn to continue", async || {
        let seen = server.model.requests().iter().any(|body| {
            let messages = body["messages"].as_array().unwrap();
            messages.last().is_some_and(|last| {
                last["role"] == "tool" && last["content"].to_string().contains(&job_id)
            })
        });
        (seen && !server.host.runtime.is_running(&id)).then_some(())
    })
    .await;
    let job = server.client.get(&format!("/jobs/{job_id}")).await.unwrap();
    assert_eq!(job["data"]["kind"], "bash");
    assert_eq!(job["data"]["status"], "running");
    assert_eq!(job["data"]["command"], "echo fg-started; sleep 300");
    eventually("the output before the move in the Job log", async || {
        log(&job["data"]).contains("fg-started").then_some(())
    })
    .await;
    // Nothing is left to move.
    let refused = server
        .client
        .post(&format!("/sessions/{id}/background"), json!({}))
        .await;
    assert!(refused.is_err(), "{refused:?}");
    stop(&server, &job_id).await;
    let job = server.client.get(&format!("/jobs/{job_id}")).await.unwrap();
    assert_eq!(job["data"]["status"], "cancelled");
}

/// Regression (`tui` → Background tasks view): every `job.*` event (each `job.output.1`
/// chunk) reloads the open `/tasks` list, and the reload reopened the picker, so a
/// running command's output reset the selection to the first row and cleared the filter
/// as the user typed: ↓ and Enter could not reach any other task. A reload now keeps
/// the filter and the selected task.
#[tokio::test]
async fn tasks_reload_keeps_the_filter_and_the_selected_task() {
    let server = server().await;
    let id = server.answered("first question").await;
    let mut app = server.load(&id).await;
    for command in ["echo first-task; sleep 300", "echo second-task; sleep 300"] {
        perform(
            &server,
            &mut app,
            vec![Action::BackgroundShell(command.into())],
        )
        .await;
    }
    eventually("both Jobs", async || {
        (jobs(&server, &id).await.len() == 2).then_some(())
    })
    .await;
    let actions = typed(&mut app, "/tasks");
    perform(&server, &mut app, actions).await;
    let selected = |app: &App| {
        let Overlay::Picker(picker) = &app.overlay else {
            panic!("{:?}", app.overlay)
        };
        assert_eq!(picker.kind, PickerKind::Tasks);
        (
            picker.query.clone(),
            picker.current().map(|c| c.key.clone()),
        )
    };
    app.on_key(key(KeyCode::Down));
    let (_, moved) = selected(&app);
    assert!(moved.is_some());
    // The live reload a job event triggers while the list is open.
    let reload = app.on_event("job.output.1", &json!({"session_id": id}));
    assert!(reload.contains(&Action::LoadTasks), "{reload:?}");
    perform(&server, &mut app, vec![Action::LoadTasks]).await;
    assert_eq!(selected(&app), (String::new(), moved));
    for c in "second-task".chars() {
        app.on_key(key(KeyCode::Char(c)));
    }
    let (query, filtered) = selected(&app);
    perform(&server, &mut app, vec![Action::LoadTasks]).await;
    assert_eq!(selected(&app), (query.clone(), filtered.clone()));
    assert_eq!(query, "second-task");
    let actions = app.on_key(key(KeyCode::Enter));
    assert_eq!(actions, [Action::OpenTask(filtered.unwrap())]);
    for job in jobs(&server, &id).await {
        stop(&server, job["id"].as_str().unwrap()).await;
    }
}

/// `background-tasks` → Tasks view: the Session's Workflow Runs are listed in `/tasks` as
/// `workflow` rows, Enter opens one in the `/workflows` monitor, and `/stop` stops a
/// running run with the Session's Jobs.
#[tokio::test]
async fn tasks_lists_workflow_runs_and_stop_all_stops_them() {
    let server = server().await;
    let id = server.answered("first question").await;
    let script = "export const meta = { name: 'slow', description: 'A long agent' };\nexport default async () => (await agent('RUN-FOREGROUND now')).text;\n";
    let started = server
        .client
        .post(
            "/workflows/runs",
            json!({"session_id": id, "script": script}),
        )
        .await
        .unwrap();
    let run_id = started["data"]["id"].as_str().unwrap().to_owned();
    let path = format!("/workflows/runs/{run_id}");
    eventually("the run's agent", async || {
        let run = server.client.get(&path).await.ok()?;
        (run["data"]["agents"]["running"] == 1).then_some(())
    })
    .await;
    let mut app = server.load(&id).await;
    let actions = typed(&mut app, "/tasks");
    perform(&server, &mut app, actions).await;
    let screen = render(&mut app, 120, 30, &Opts::default());
    assert!(screen.contains("slow (running)"), "{screen}");
    assert!(screen.contains("workflow · "), "{screen}");

    perform(&server, &mut app, vec![Action::StopTasks]).await;
    let run = server.client.get(&path).await.unwrap();
    assert_eq!(run["data"]["status"], "stopped", "{run}");
}
