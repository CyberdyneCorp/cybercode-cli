//! `/btw` (`tui` → Subagent threads and side chat, `session-runtime` → Side chat) and the
//! client side of `notify`/`send_file` (`background-tasks`) against a production App: the
//! overlay asks through the runner's own `perform`, Esc discards the answer, `i` inserts
//! it, and the main Session's history never changes.

use std::time::Duration;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use futures::StreamExt;
use serde_json::{Value, json};

use crate::app::{Action, App, Overlay};
use crate::screens::{Opts, Server, TITLE, render};

const SIDE_ANSWER: &str = "--frozen fails instead of updating the lockfile.";

fn reply(body: &Value) -> Value {
    let text = body.to_string();
    if text.contains("Write a short title") {
        return json!({ "content": TITLE });
    }
    if text.contains("side question") {
        return json!({ "content": SIDE_ANSWER });
    }
    let last = body["messages"]
        .as_array()
        .and_then(|m| m.last())
        .cloned()
        .unwrap_or_default();
    if last["role"] != "tool" && last["content"].to_string().contains("tell me when done") {
        let notify = json!({"title": "Build finished", "body": "All tests passed"}).to_string();
        let send = json!({"paths": ["report.txt"], "caption": "The report"}).to_string();
        return json!({ "tool_calls": [
            {"index": 0, "id": "call_notify", "type": "function", "function": {"name": "notify", "arguments": notify}},
            {"index": 1, "id": "call_send", "type": "function", "function": {"name": "send_file", "arguments": send}}] });
    }
    json!({ "content": "Done." })
}

async fn server() -> Server {
    Server::start_reply(reply, json!({ "main": {} }), json!({}), false).await
}

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn typed(app: &mut App, text: &str) -> Vec<Action> {
    for c in text.chars() {
        app.on_key(key(KeyCode::Char(c)));
    }
    app.on_key(key(KeyCode::Enter))
}

/// Ask through the runner's own `perform` and `apply`.
async fn ask(server: &Server, app: &mut App, question: &str) {
    let actions = typed(app, &format!("/btw {question}"));
    assert!(
        matches!(actions.as_slice(), [Action::SideChat { question: q, .. }] if q == question),
        "{actions:?}"
    );
    let screen = render(app, 120, 40, &Opts::default());
    assert!(
        screen.contains("Thinking…") && screen.contains("Side chat"),
        "{screen}"
    );
    for action in actions {
        let msg = crate::perform::perform(&server.client, &app.session, action).await;
        crate::runner::apply(app, msg, &mut Default::default());
    }
}

async fn messages(server: &Server, id: &str) -> Value {
    server
        .client
        .get(&format!("/sessions/{id}/messages?limit=200"))
        .await
        .unwrap()["data"]
        .clone()
}

#[tokio::test]
async fn btw_overlay_discards_on_close_and_insert_answer_inserts() {
    let server = server().await;
    let id = server.answered("How do I install?").await;
    let mut app = server.app(&id).await;
    let before = messages(&server, &id).await;

    ask(&server, &mut app, "what does the --frozen flag do?").await;
    let screen = render(&mut app, 120, 40, &Opts::default());
    assert!(screen.contains(SIDE_ANSWER), "{screen}");
    assert!(screen.contains("i Insert answer · Esc discard"), "{screen}");
    assert!(
        screen.contains("You: what does the --frozen flag do?"),
        "{screen}"
    );
    // Esc discards: nothing reaches the composer or the Session.
    app.on_key(key(KeyCode::Esc));
    assert!(matches!(app.overlay, Overlay::None));
    assert!(app.composer.is_empty());
    assert_eq!(messages(&server, &id).await, before);

    // `/side` is the alias; `i` inserts the answer for the user to edit and send.
    ask(&server, &mut app, "and --locked?").await;
    app.on_key(key(KeyCode::Char('i')));
    assert!(matches!(app.overlay, Overlay::None));
    assert_eq!(app.composer.text(), SIDE_ANSWER);
    assert_eq!(messages(&server, &id).await, before);
    assert!(!server.host.runtime.is_running(&id));

    // A bare `/btw` only explains itself.
    app.composer.set_text("");
    assert!(typed(&mut app, "/btw").is_empty());
    assert!(matches!(app.overlay, Overlay::None));
    assert_eq!(app.toast.as_ref().unwrap().0, "Usage: /btw <question>");
    let alias = typed(&mut app, "/side quick one");
    assert!(matches!(alias.as_slice(), [Action::SideChat { .. }]));
}

#[test]
fn btw_is_offered_in_the_palette_and_help() {
    let mut app = App::new(Default::default(), Vec::new(), "cyber");
    let entries = crate::palette::entries(&app.keymap, &[]);
    assert!(entries.iter().any(|e| e.name == "/btw"), "command");
    app.overlay = Overlay::Help;
    let screen = render(&mut app, 120, 48, &Opts::default());
    assert!(screen.contains("/btw <question> · /side"), "{screen}");
}

#[tokio::test]
async fn notify_and_send_file_reach_the_tui_as_a_notification_and_a_toast() {
    let server = server().await;
    let id = server.session().await;
    let mut app = server.app(&id).await;
    let mut events = Box::pin(server.client.events(true).await.unwrap());
    std::fs::write(server.root().join("report.txt"), "all green\n").unwrap();
    server.prompt(&id, "build it and tell me when done").await;
    let mut seen = (false, false);
    while seen != (true, true) {
        let event = tokio::time::timeout(Duration::from_secs(20), events.next())
            .await
            .expect("the deliveries arrive")
            .unwrap();
        crate::runner::server_event(&mut app, &event);
        if event.kind.starts_with("session.notification.sent.") {
            seen.0 = true;
            assert!(
                app.attention_log
                    .iter()
                    .any(|s| s.contains("Build finished: All tests passed")),
                "{:?}",
                app.attention_log
            );
        }
        if event.kind.starts_with("session.files.sent.") {
            seen.1 = true;
            let toast = &app.toast.as_ref().unwrap().0;
            assert!(
                toast.starts_with("The report · Received report.txt (10 B) at "),
                "{toast}"
            );
        }
    }
}
