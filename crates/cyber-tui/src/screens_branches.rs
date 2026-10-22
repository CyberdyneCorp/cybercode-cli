//! `/tree` and `/label` (`tui` → Branch tree view) against a production App: the tree
//! lists every branch's user messages, Enter checks one out through the runner's own
//! `perform`, `c` restores its code with the diff shown, `l` and `/label` label, the
//! header names the active branch, and a busy Session keeps its branch.

use std::time::Duration;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use serde_json::{Value, json};

use crate::app::{Action, App, Overlay};
use crate::branches::{Request, short_id};
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

/// Perform `actions` as the runner does, following up until nothing is left.
async fn run(server: &Server, app: &mut App, mut actions: Vec<Action>) {
    while let Some(action) = actions.pop() {
        let msg = crate::perform::perform(&server.client, &app.session, action).await;
        actions.extend(crate::runner::apply(app, msg, &mut Default::default()));
    }
}

/// Type a slash command and perform what it asks for.
async fn command(server: &Server, app: &mut App, line: &str) {
    let actions = typed(app, line);
    run(server, app, actions).await;
}

async fn messages(server: &Server, id: &str) -> Vec<Value> {
    let page = server
        .client
        .get(&format!("/sessions/{id}/messages?order=asc&limit=200"))
        .await
        .unwrap();
    page["data"].as_array().unwrap().clone()
}

async fn user_ids(server: &Server, id: &str) -> Vec<String> {
    let listed = messages(server, id).await;
    let users = listed.iter().filter(|m| m["kind"] == "user");
    users
        .map(|m| m["id"].as_str().unwrap().to_owned())
        .collect()
}

/// Send `text` and wait until its Turn has answered.
async fn turn(server: &Server, id: &str, text: &str) {
    let before = user_ids(server, id).await.len();
    server.prompt(id, text).await;
    for _ in 0..1000 {
        let listed = messages(server, id).await;
        let users = listed.iter().filter(|m| m["kind"] == "user").count();
        let last = listed.last().cloned().unwrap_or_default();
        let answered = last["kind"] == "assistant" && last["finished"] == true;
        if users == before + 1 && answered && !server.host.runtime.is_running(id) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("{text} was not answered");
}

async fn rewind(server: &Server, id: &str, message: &str, target: &str) {
    let body = json!({ "message_id": message, "target": target });
    server
        .client
        .post(&format!("/sessions/{id}/rewind"), body)
        .await
        .unwrap();
}

/// User messages one, two, three; a conversation rewind to two; then two-b. Returns
/// the Session and the user message IDs in that order.
async fn two_branches(server: &Server) -> (String, Vec<String>) {
    let id = server.session().await;
    for text in ["one", "two", "three"] {
        turn(server, &id, text).await;
    }
    let first = user_ids(server, &id).await;
    rewind(server, &id, &first[1], "conversation").await;
    turn(server, &id, "two-b").await;
    let twob = user_ids(server, &id).await[1].clone();
    let mut ids = first;
    ids.push(twob);
    (id, ids)
}

/// The scenario "Switch branches from the tree" and the header's branch tag.
#[tokio::test]
async fn tree_checks_out_a_branch_and_the_header_names_it() {
    let server = Server::start().await;
    let (id, ids) = two_branches(&server).await;
    let mut app = server.app(&id).await;
    let screen = render(&mut app, 100, 30, &Opts::default());
    let header = format!("branch {} (2 of 2)", short_id(&ids[3]));
    assert!(screen.contains(&header), "{screen}");

    let actions = typed(&mut app, "/tree");
    assert_eq!(actions, [Action::Branch(Request::Load { open: true })]);
    run(&server, &mut app, actions).await;
    assert_eq!(app.overlay, Overlay::Tree);
    let screen = render(&mut app, 120, 40, &Opts::default());
    for expected in [
        "Session tree",
        "● one  (2 branches)",
        "○ two",
        "○ three",
        "● two-b  (active)",
        "Enter checkout · c restore code: off · l label",
    ] {
        assert!(screen.contains(expected), "{expected}: {screen}");
    }

    // Up from two-b selects three; Enter checks it out.
    app.on_key(key(KeyCode::Up));
    let actions = app.on_key(key(KeyCode::Enter));
    assert_eq!(
        actions,
        [Action::Branch(Request::Checkout {
            message_id: ids[2].clone(),
            code: false,
        })]
    );
    run(&server, &mut app, actions).await;
    assert_eq!(app.overlay, Overlay::None);
    let screen = render(&mut app, 100, 30, &Opts::default());
    let header = format!("branch {} (1 of 2)", short_id(&ids[2]));
    assert!(screen.contains(&header), "{screen}");
    assert!(
        screen.contains("three") && !screen.contains("two-b"),
        "{screen}"
    );
    assert_eq!(user_ids(&server, &id).await, ids[..3]);

    // `l` labels the selected message; the header shows the branch's label.
    command(&server, &mut app, "/tree").await;
    app.on_key(key(KeyCode::Up));
    app.on_key(key(KeyCode::Char('l')));
    let screen = render(&mut app, 120, 40, &Opts::default());
    assert!(
        screen.contains("Label: ▏  Enter save · Esc cancel"),
        "{screen}"
    );
    let actions = typed(&mut app, "fix-a");
    assert_eq!(
        actions,
        [Action::Branch(Request::Label {
            message_id: Some(ids[1].clone()),
            name: "fix-a".into(),
        })]
    );
    run(&server, &mut app, actions).await;
    let screen = render(&mut app, 120, 40, &Opts::default());
    assert!(
        screen.contains("○ two  [fix-a]") || screen.contains("● two  [fix-a]"),
        "{screen}"
    );
    app.on_key(key(KeyCode::Esc));
    let screen = render(&mut app, 100, 30, &Opts::default());
    assert!(screen.contains("branch fix-a (1 of 2)"), "{screen}");

    // `/label` labels the active tip; `/label --clear` removes a label.
    command(&server, &mut app, "/label here").await;
    let state = server.host.runtime.state(&id).await.unwrap();
    assert_eq!(
        state.branches().labels()["here"],
        state.active_tip().unwrap()
    );
    command(&server, &mut app, "/label --clear fix-a").await;
    let state = server.host.runtime.state(&id).await.unwrap();
    assert!(!state.branches().labels().contains_key("fix-a"));
    assert!(typed(&mut app, "/label has space").is_empty());
    assert!(typed(&mut app, "/label").is_empty());
}

/// `c` restores the branch's code with the checkout and shows the restored diff.
#[tokio::test]
async fn tree_restores_code_with_c_and_shows_the_diff() {
    let server = Server::start_config(json!({}), json!({}), true).await;
    // A worktree of its own: the server's data directory is not part of it.
    let root = server.root().join("work");
    std::fs::create_dir_all(&root).unwrap();
    let git = std::process::Command::new("git")
        .args(["init", "-q", "-b", "main"])
        .current_dir(&root)
        .status()
        .unwrap();
    assert!(git.success());
    let created = server
        .client
        .at(&root.display().to_string())
        .post(
            "/sessions",
            json!({ "model": "test/main", "mode": "accept-edits" }),
        )
        .await
        .unwrap();
    let id = created["data"]["id"].as_str().unwrap().to_owned();
    turn(&server, &id, "WRITE notes").await;
    let written = user_ids(&server, &id).await[0].clone();
    assert!(root.join("notes.md").exists());
    rewind(&server, &id, &written, "both").await;
    assert!(!root.join("notes.md").exists());
    turn(&server, &id, "plain").await;

    let mut app = server.app(&id).await;
    command(&server, &mut app, "/tree").await;
    app.on_key(key(KeyCode::Up));
    app.on_key(key(KeyCode::Char('c')));
    let screen = render(&mut app, 120, 40, &Opts::default());
    assert!(screen.contains("c restore code: on"), "{screen}");
    let actions = app.on_key(key(KeyCode::Enter));
    assert_eq!(
        actions,
        [Action::Branch(Request::Checkout {
            message_id: written.clone(),
            code: true,
        })]
    );
    run(&server, &mut app, actions).await;
    assert!(root.join("notes.md").exists());
    let Overlay::TaskLog { title, text, .. } = &app.overlay else {
        panic!("{:?}", app.overlay);
    };
    assert!(title.starts_with("Restored 1 file for"), "{title}");
    assert!(text.contains("+# Notes"), "{text}");
    assert_eq!(user_ids(&server, &id).await, [written]);
}

/// The scenario "Busy Session": Enter while a Turn runs reports that the Session must
/// be idle, and a checkout the server refuses leaves the branch as it was.
#[tokio::test]
async fn tree_refuses_to_switch_while_a_turn_runs() {
    let server = Server::start().await;
    let (id, ids) = two_branches(&server).await;
    let mut app = server.app(&id).await;
    command(&server, &mut app, "/tree").await;
    server.prompt(&id, "HOLD on").await;
    while !server.host.runtime.is_running(&id) {
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    app.session.running = true;
    app.on_key(key(KeyCode::Up));
    assert!(app.on_key(key(KeyCode::Enter)).is_empty());
    let toast = app
        .toast
        .as_ref()
        .map(|(t, _)| t.clone())
        .unwrap_or_default();
    assert!(toast.contains("must be idle"), "{toast}");

    // A client that missed the start still gets the server's refusal.
    app.session.running = false;
    let actions = app.on_key(key(KeyCode::Enter));
    run(&server, &mut app, actions).await;
    let toast = app
        .toast
        .as_ref()
        .map(|(t, _)| t.clone())
        .unwrap_or_default();
    assert!(toast.contains("SessionBusyError"), "{toast}");
    let state = server.host.runtime.state(&id).await.unwrap();
    assert!(state.entries.iter().any(|e| e.id() == ids[3]));
    server.model.release();
}

#[test]
fn tree_and_label_are_offered_in_the_palette_and_help() {
    let mut app = App::new(Default::default(), Vec::new(), "cyber");
    let entries = crate::palette::entries(&app.keymap, &[]);
    for name in ["Session tree", "Label branch", "/tree", "/label"] {
        assert!(entries.iter().any(|e| e.name == name), "{name}");
    }
    app.overlay = Overlay::Help;
    let screen = render(&mut app, 120, 48, &Opts::default());
    assert!(screen.contains("/tree · /label <name>"), "{screen}");
}
