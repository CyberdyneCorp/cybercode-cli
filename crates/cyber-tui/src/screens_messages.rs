//! Cross-session message surfaces (`tui` → Cross-session message surfaces), drawn by the
//! real `view::draw` from a production App over HTTP: messages sent through
//! `POST /messaging/send`, a reply sent by the scripted model's `send_message` call, an
//! idle notice from `POST /messaging/watch`, messages held by the Session's inbound
//! override, and the TUI's reads and actions performed through `perform` and the
//! runner's `apply`.

use std::time::Duration;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use serde_json::{Value, json};

use crate::app::{Action, App, Overlay};
use crate::screens::{Opts, Server, render};
use crate::screens_workflows::{key, press, text};

const QUESTION: &str = "which port does the API use?";
const REPLY: &str = "API moved to port 8080; restart your dev server.";
const NIGHTLY: &str = "The nightly build on desk is red; can you check the migration order?";
const SCHEMA: &str = "Please review the schema change before the next deploy.";

/// B answers the question with `send_message` to A; every other prompt gets a short
/// answer, so the idle notice says `Noted.`.
fn reply(body: &Value) -> Value {
    if body.to_string().contains("Write a short title") {
        return json!({ "content": "Messages" });
    }
    let last = body["messages"]
        .as_array()
        .and_then(|m| m.last())
        .map(Value::to_string)
        .unwrap_or_default();
    if last.contains("\"role\":\"tool\"") {
        return json!({ "content": "Sent." });
    }
    if last.contains(QUESTION) {
        let arguments = json!({ "to": "frontend", "text": REPLY }).to_string();
        return json!({ "tool_calls": [{ "index": 0, "id": "call_send", "type": "function",
            "function": { "name": "send_message", "arguments": arguments } }] });
    }
    json!({ "content": "Noted." })
}

async fn session(server: &Server, title: &str) -> String {
    let body = json!({ "model": "test/main", "mode": "default", "title": title });
    let created = server.client.post("/sessions", body).await.unwrap();
    created["data"]["id"].as_str().unwrap().to_owned()
}

async fn send(server: &Server, id: &str, from: &str, to: &str, text: &str) -> Value {
    let body = json!({ "id": id, "from": from, "to": to, "text": text });
    server.client.post("/messaging/send", body).await.unwrap()["data"].take()
}

/// Wait until `id` is idle with `n` assistant messages reading `answer`.
async fn answered(server: &Server, id: &str, answer: &str, n: usize) {
    for _ in 0..3000 {
        let path = format!("/sessions/{id}/messages?order=asc&limit=200");
        let page = server.client.get(&path).await.unwrap();
        let count = page["data"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|m| m["kind"] == "assistant" && m["text"] == answer)
            .count();
        if count >= n && !server.host.runtime.is_running(id) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("{id} did not answer {answer:?} {n} times");
}

/// Perform `actions` as the runner does, until nothing follows.
async fn drive(server: &Server, app: &mut App, mut actions: Vec<Action>) {
    while !actions.is_empty() {
        let action = actions.remove(0);
        if matches!(action, Action::LoadSidebar(_)) {
            continue;
        }
        let msg = crate::perform::perform_owned(&server.client, &app.session, action, None).await;
        actions.extend(crate::runner::apply(app, msg, &mut Default::default()));
    }
}

/// 14:00 UTC on 2026-10-16 plus `minutes`.
fn at(minutes: i64) -> i64 {
    1_792_159_200_000 + minutes * 60_000
}

/// Fixed times for the records and the notice, in the order they happened.
fn stabilize(app: &mut App) {
    crate::screens::stabilize(app);
    for record in &mut app.messaging.records {
        let minutes = match record.message_id.as_str() {
            "msgx_question" => 2,
            "msgx_nightly" => 5,
            "msgx_schema" => 6,
            _ => 3,
        };
        record.created_ms = at(minutes);
        record.updated_ms = at(minutes);
    }
    for item in &mut app.items {
        if let crate::model::Item::User { id, text, .. } = item
            && id.starts_with("msgx_watch_")
        {
            let start = text.find("finished_at=\"").unwrap() + 13;
            let end = start + text[start..].find('"').unwrap();
            text.replace_range(start..end, "2026-10-16T14:04:00.000Z");
            *id = "msgx_watch_01JA2B".into();
        }
    }
}

/// The Session IDs and the reply's message ID, which differ between runs.
struct Ids {
    a: String,
    b: String,
    sent: String,
}

/// `text` with every run-dependent ID (whole, or cut to its last characters as the
/// views show it) replaced by a fixed one of the same width.
fn redact(text: &str, ids: &Ids) -> String {
    let mut out = text.to_owned();
    for (id, fixed) in [
        (&ids.a, "AAAAAAAAAAAAAAAAAAAAAAAAAA"),
        (&ids.b, "BBBBBBBBBBBBBBBBBBBBBBBBBB"),
        (&ids.sent, "5d1e5d1e5d1e5d1e5d1e5d1e5d1e5d1e5d1e5d1e"),
    ] {
        let (prefix, rest) = id.split_once('_').unwrap();
        let fixed: String = fixed.chars().take(rest.chars().count()).collect();
        out = out.replace(id.as_str(), &format!("{prefix}_{fixed}"));
        for keep in [6, 4] {
            let tail: String = rest.chars().skip(rest.chars().count() - keep).collect();
            let short: String = fixed.chars().take(keep).collect();
            out = out.replace(&format!("{prefix}_{tail}"), &format!("{prefix}_{short}"));
        }
    }
    out
}

fn shot(app: &mut App, ids: &Ids) -> String {
    let saved = (app.messaging.records.clone(), app.items.clone());
    stabilize(app);
    let shown = redact(&render(app, 100, 40, &Opts::default()), ids);
    app.messaging.records = saved.0;
    app.items = saved.1;
    shown
}

fn screen(app: &mut App) -> String {
    text(&crate::screens::buffer(app, 100, 40, &Opts::default()))
}

fn held(app: &App) -> Vec<String> {
    app.messaging
        .held(&app.queued)
        .iter()
        .map(|(row, _)| row.message_id.clone())
        .collect()
}

#[tokio::test]
async fn messages_render_and_held_messages_are_released_from_the_tui() {
    let server = Server::start_reply(reply, json!({ "main": {} }), json!({}), false).await;
    let a = session(&server, "frontend").await;
    let b = session(&server, "api-refactor").await;

    // A asks idle B, which wakes and answers with `send_message` to A's address.
    let sent = send(&server, "msgx_question", &a, "api-refactor", QUESTION).await;
    assert_eq!(sent["outcome"], "delivered", "{sent}");
    answered(&server, &b, "Sent.", 1).await;
    answered(&server, &a, "Noted.", 1).await;
    // B watches A, which is idle: the notice arrives at once and B reads it.
    let watch = json!({ "from": b, "target": "frontend" });
    server.client.post("/messaging/watch", watch).await.unwrap();
    answered(&server, &b, "Noted.", 1).await;
    // B now holds what arrives.
    let hold = json!({ "messaging": { "inbound": "hold" } });
    server
        .client
        .patch(&format!("/sessions/{b}"), hold)
        .await
        .unwrap();
    for (id, text) in [("msgx_nightly", NIGHTLY), ("msgx_schema", SCHEMA)] {
        let held = send(&server, id, &a, "api-refactor", text).await;
        assert_eq!(held["outcome"], "held", "{held}");
    }

    let mut app = server.load(&b).await;
    let sent = app
        .messaging
        .records
        .iter()
        .find(|r| !r.incoming)
        .expect("B's reply is recorded")
        .message_id
        .clone();
    let ids = Ids {
        a: a.clone(),
        b: b.clone(),
        sent,
    };
    assert_eq!(
        app.messaging.records.len(),
        4,
        "{:?}",
        app.messaging.records
    );
    assert_eq!(held(&app), ["msgx_nightly", "msgx_schema"]);

    // The received block, the sent call, the notice, the badge and the held card.
    let screen_text = screen(&mut app);
    for needle in [
        "✉ frontend · ses_",
        "untrusted",
        "│ which port does the API use?",
        "● send_message → frontend",
        "delivered · msgx_",
        "│ API moved to port 8080",
        "◷ frontend is idle",
        "\"Noted.\"",
        "Held message",
        "2 of 2",
        "✉ from frontend",
        "Please review the schema change",
        "✉ 2 held",
    ] {
        assert!(screen_text.contains(needle), "{needle:?}:\n{screen_text}");
    }
    assert!(
        !screen_text.contains("<cross_session_message"),
        "the block is drawn, not its markup:\n{screen_text}"
    );
    insta::assert_snapshot!("messages_transcript_100x40", shot(&mut app, &ids));

    // `/messages`, from the palette: no key by default.
    assert_eq!(app.keymap.label("global", "messages_view"), None);
    app.on_key(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::CONTROL));
    for c in "messages".chars() {
        key(&mut app, c);
    }
    let actions = press(&mut app, KeyCode::Enter);
    assert!(matches!(app.overlay, Overlay::Messages), "{actions:?}");
    drive(&server, &mut app, actions).await;
    let rows = app.messaging.rows(&app.items, &app.queued);
    assert_eq!(rows.len(), 5, "three in, one out and the notice");
    press(&mut app, KeyCode::Enter);
    insta::assert_snapshot!("messages_view_100x40", shot(&mut app, &ids));
    // The held filter lists the two held messages.
    for _ in 0..3 {
        press(&mut app, KeyCode::Tab);
    }
    let view = screen(&mut app);
    assert!(view.contains("[held]"), "{view}");
    assert_eq!(app.messaging.rows(&app.items, &app.queued).len(), 2);
    press(&mut app, KeyCode::Tab);
    press(&mut app, KeyCode::Esc);
    assert!(matches!(app.overlay, Overlay::None));

    // Typing goes to the composer even with a card shown; Tab on an empty composer
    // selects the card.
    key(&mut app, 'r');
    assert_eq!(app.composer.text(), "r");
    assert!(!app.messaging.card_focus);
    press(&mut app, KeyCode::Backspace);
    assert!(press(&mut app, KeyCode::Tab).is_empty());
    assert!(app.messaging.card_focus);
    insta::assert_snapshot!("messages_card_selected_100x40", shot(&mut app, &ids));

    // `r` releases the newest held message: the card moves to the other one, the badge
    // drops by one, and the released message reaches the model.
    let actions = key(&mut app, 'r');
    assert_eq!(
        actions,
        vec![Action::ReleaseHeld("msgx_schema".into()), Action::Refresh]
    );
    assert_eq!(held(&app), ["msgx_nightly"]);
    let after = screen(&mut app);
    assert!(
        after.contains("✉ 1 held") && !after.contains("2 of 2"),
        "{after}"
    );
    drive(&server, &mut app, actions).await;
    answered(&server, &b, "Noted.", 2).await;
    let request = server.model.requests().last().unwrap().to_string();
    assert!(
        request.contains("Please review the schema change"),
        "{request}"
    );
    assert!(!request.contains("nightly build"), "the other stays held");
    drive(&server, &mut app, vec![Action::Refresh]).await;
    assert_eq!(held(&app), ["msgx_nightly"]);
    let released = app.messaging.received("msgx_schema").unwrap();
    assert_eq!(released.outcome, "released");
    let after = screen(&mut app);
    assert!(after.contains("✉ 1 held"), "{after}");
    assert!(
        after.contains("│ Please review the schema change"),
        "{after}"
    );

    // `d` drops the last one, the card still selected; the card and the badge go.
    assert!(app.messaging.card_focus);
    let actions = key(&mut app, 'd');
    assert_eq!(
        actions,
        vec![Action::RemoveQueued("msgx_nightly".into()), Action::Refresh]
    );
    drive(&server, &mut app, actions).await;
    assert!(held(&app).is_empty() && !app.messaging.card_focus);
    let after = screen(&mut app);
    assert!(
        !after.contains("Held message") && !after.contains("✉ 1 held"),
        "{after}"
    );
    assert_eq!(
        app.messaging.received("msgx_nightly").unwrap().outcome,
        "dropped"
    );

    // `a` clears a `non_interactive` refusal.
    let refuse = json!({ "messaging": { "inbound": "refuse", "reason": "non_interactive" } });
    server
        .client
        .patch(&format!("/sessions/{b}"), refuse)
        .await
        .unwrap();
    drive(&server, &mut app, vec![Action::Refresh]).await;
    let actions = app.on_key(KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE));
    assert!(actions.is_empty());
    for c in "messages".chars() {
        key(&mut app, c);
    }
    app.completion = None;
    let actions = press(&mut app, KeyCode::Enter);
    drive(&server, &mut app, actions).await;
    assert!(screen(&mut app).contains("a accept messages"));
    let actions = key(&mut app, 'a');
    assert_eq!(actions, vec![Action::AcceptMessages, Action::Refresh]);
    drive(&server, &mut app, actions).await;
    let session = server.client.get(&format!("/sessions/{b}")).await.unwrap();
    assert!(session["data"]["messaging"].is_null(), "{session}");
    assert!(!screen(&mut app).contains("a accept messages"));
}
