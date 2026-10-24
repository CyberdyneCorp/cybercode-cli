//! Mouse clicks (`tui` → Mouse clicks, Permission prompt, Question prompt): the real
//! `view::draw` registers the click targets, and clicks at coordinates found in the
//! drawn buffer go through `App::on_mouse`, against a production App over HTTP with a
//! local scripted model.

use std::time::Duration;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::buffer::Buffer;
use serde_json::{Value, json};

use crate::app::{Action, App, Overlay, PermStep};
use crate::model::Request;
use crate::screens::{Opts, Server, buffer, row_of};

const WIDTH: u16 = 100;
const HEIGHT: u16 = 30;

/// The column and row where `needle` starts on screen.
pub(crate) fn cell_of(buf: &Buffer, needle: &str) -> (u16, u16) {
    let y = row_of(buf, needle);
    let row: Vec<&str> = (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect();
    let x = (0..row.len())
        .find(|&x| row[x..].concat().starts_with(needle))
        .unwrap();
    (x as u16, y)
}

fn draw(app: &mut App) -> Buffer {
    buffer(app, WIDTH, HEIGHT, &Opts::default())
}

pub(crate) fn click(app: &mut App, (column, row): (u16, u16)) -> Vec<Action> {
    app.on_mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column,
        row,
        modifiers: KeyModifiers::NONE,
    })
}

/// Draw the screen as the runner does before each event, then click `needle`.
fn click_text(app: &mut App, needle: &str) -> Vec<Action> {
    let at = cell_of(&draw(app), needle);
    click(app, at)
}

/// Type `line` in the composer and press Enter.
fn typed(app: &mut App, line: &str) -> Vec<Action> {
    app.composer.set_text(line);
    app.completion = None;
    app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
}

/// Perform `actions` as the runner does, until nothing follows.
async fn drive(server: &Server, app: &mut App, mut actions: Vec<Action>) {
    while !actions.is_empty() {
        let action = actions.remove(0);
        let msg = crate::perform::perform_owned(&server.client, &app.session, action, None).await;
        actions.extend(crate::runner::apply(app, msg, &mut Default::default()));
    }
}

fn palette_selected(app: &App) -> usize {
    match &app.overlay {
        Overlay::Palette(p) => p.selected,
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn a_click_outside_the_palette_leaves_it_open() {
    let server = Server::start().await;
    let id = server.answered_session().await;
    let mut app = server.app(&id).await;
    app.open_palette();
    let second = match &app.overlay {
        Overlay::Palette(p) => p.entries[p.filtered[1]].name.clone(),
        other => panic!("{other:?}"),
    };
    assert!(click_text(&mut app, &second).is_empty());
    assert_eq!(palette_selected(&app), 1, "the first click selects");
    let buf = draw(&mut app);
    let palette = row_of(&buf, " Commands ");
    // The transcript beside and above the palette, and the composer below it.
    for at in [(1, palette + 3), (1, 1), (1, HEIGHT - 3)] {
        assert!(click(&mut app, at).is_empty(), "{at:?}");
        assert_eq!(palette_selected(&app), 1, "{at:?}");
    }
    // Inside the palette on no row: the query line.
    assert!(click_text(&mut app, "› ").is_empty());
    assert_eq!(palette_selected(&app), 1);
    assert!(
        click_text(&mut app, &second).is_empty(),
        "a click elsewhere in between disarms the row"
    );
    assert_eq!(palette_selected(&app), 1);
    assert_eq!(app.scroll, 0);
}

#[tokio::test]
async fn a_row_above_the_selection_of_a_scrolled_list_activates_on_a_second_click() {
    let server = Server::start().await;
    let id = server.answered_session().await;
    let mut app = server.app(&id).await;
    app.open_palette();
    let wheel = MouseEvent {
        kind: MouseEventKind::ScrollDown,
        column: WIDTH / 2,
        row: HEIGHT / 2,
        modifiers: KeyModifiers::NONE,
    };
    for _ in 0..20 {
        app.on_mouse(wheel);
        draw(&mut app);
    }
    assert_eq!(
        palette_selected(&app),
        20,
        "scrolled past the palette's room"
    );
    let tenth = match &app.overlay {
        Overlay::Palette(p) => p.entries[p.filtered[10]].name.clone(),
        other => panic!("{other:?}"),
    };
    let at = cell_of(&draw(&mut app), &tenth);
    assert!(click(&mut app, at).is_empty(), "the first click selects");
    assert_eq!(palette_selected(&app), 10);
    assert_eq!(
        cell_of(&draw(&mut app), &tenth),
        at,
        "the window stays: the row is still under the pointer"
    );
    click(&mut app, at);
    assert!(
        !matches!(app.overlay, Overlay::Palette(_)),
        "a second click on the same cell runs it"
    );
}

/// A request of the open Session, as the server lists it.
fn permission(app: &App, action: &str, metadata: Value) -> Request {
    Request::Permission {
        id: format!("per_{action}"),
        session_id: app.session.id.clone(),
        origin: None,
        member: None,
        routed_to: Vec::new(),
        action: action.into(),
        resources: vec!["rm -rf build".into()],
        patterns: vec!["rm *".into()],
        metadata,
    }
}

#[tokio::test]
async fn confirmations_take_no_clicks() {
    let server = Server::start().await;
    let id = server.answered_session().await;
    let mut app = server.app(&id).await;
    for (overlay, text) in [
        (Overlay::ConfirmStopTasks, "y / Enter to stop"),
        (Overlay::ConfirmBypass, "y to enable bypass"),
    ] {
        app.overlay = overlay;
        for _ in 0..2 {
            assert!(click_text(&mut app, text).is_empty(), "{text}");
        }
        assert!(
            matches!(
                app.overlay,
                Overlay::ConfirmStopTasks | Overlay::ConfirmBypass
            ),
            "{text}"
        );
    }

    // The Allow always step saves rules and the feedback is typed: both take keys only.
    app.requests = vec![permission(&app, "bash", json!({}))];
    app.overlay = Overlay::None;
    app.sync_overlay();
    for (step, text) in [
        (PermStep::ConfirmAlways, "bash rm *"),
        (PermStep::Feedback(String::new()), "Tell the model"),
    ] {
        app.overlay = Overlay::Permission(step.clone());
        for _ in 0..2 {
            assert!(click_text(&mut app, text).is_empty(), "{text}");
        }
        assert!(matches!(&app.overlay, Overlay::Permission(s) if *s == step));
    }

    // The blocked-call approval replays a call the classifier refused: keys only.
    app.requests = vec![permission(
        &app,
        "auto_override",
        json!({ "tool": "bash", "input": { "command": "rm -rf build" } }),
    )];
    app.overlay = Overlay::Permission(PermStep::Choose(1));
    for _ in 0..2 {
        assert!(click_text(&mut app, "Confirm replay once").is_empty());
    }
    assert!(matches!(
        app.overlay,
        Overlay::Permission(PermStep::Choose(1))
    ));
}

/// Two `bash` calls, one after the other, then an answer.
fn runs_bash_twice(body: &Value) -> Value {
    let text = body.to_string();
    if text.contains("Write a short title") {
        return json!({ "content": "Clicks" });
    }
    let results = text.matches("\"role\":\"tool\"").count();
    if results >= 2 {
        return json!({ "content": "Done." });
    }
    let arguments = json!({ "command": format!("echo step{results}") }).to_string();
    json!({ "tool_calls": [{ "index": 0, "id": format!("call_{results}"), "type": "function",
        "function": { "name": "bash", "arguments": arguments } }] })
}

/// Load `id` as the runner does until a request other than `previous` is shown.
async fn next_request(server: &Server, app: &mut App, previous: Option<&str>) -> String {
    for _ in 0..1000 {
        let msg = crate::perform::snapshot(&server.client, &app.session.id).await;
        crate::runner::apply(app, msg, &mut Default::default());
        if let Some(id) = app.active_request().map(|r| r.id().to_owned())
            && Some(id.as_str()) != previous
        {
            return id;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("no request after {previous:?}");
}

/// Wait until `id`'s Turn ended with the model's last answer; returns the transcript.
async fn wait_idle(server: &Server, id: &str) -> String {
    let path = format!("/sessions/{id}/messages?order=asc&limit=200");
    for _ in 0..1000 {
        let page = server.client.get(&path).await.unwrap().to_string();
        if !server.host.runtime.is_running(id) && page.contains("Done.") {
            return page;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("{id} did not finish");
}

async fn perform(server: &Server, app: &App, actions: Vec<Action>) {
    for action in actions {
        crate::perform::perform_owned(&server.client, &app.session, action, None)
            .await
            .unwrap();
    }
}

#[tokio::test]
async fn a_single_click_never_answers_a_permission() {
    let server =
        Server::start_reply(runs_bash_twice, json!({ "main": {} }), json!({}), false).await;
    let id = server.session().await;
    let mut app = server.load(&id).await;
    server.prompt(&id, "Run the steps.").await;
    let first = next_request(&server, &mut app, None).await;
    assert!(matches!(
        app.overlay,
        Overlay::Permission(PermStep::Choose(0))
    ));

    // One click selects, on any option; no reply is sent. The clicks are seconds
    // apart, so none is the tail of a multi-click.
    let start = app.started();
    let later = move |app: &mut App, s: u64| app.clock = Some(start + Duration::from_secs(s));
    later(&mut app, 1);
    assert!(click_text(&mut app, "Reject (n)").is_empty());
    assert!(matches!(
        app.overlay,
        Overlay::Permission(PermStep::Choose(2))
    ));
    later(&mut app, 2);
    assert!(click_text(&mut app, "Allow once (y)").is_empty());
    assert!(matches!(
        app.overlay,
        Overlay::Permission(PermStep::Choose(0))
    ));

    // The request is answered elsewhere while `Allow once` is armed, and the next one
    // shows the option at the same place: the click there only selects it.
    let at = cell_of(&draw(&mut app), "Allow once (y)");
    server
        .client
        .post(
            &format!("/sessions/{id}/permissions/{first}/reply"),
            json!({ "reply": "once" }),
        )
        .await
        .unwrap();
    let second = next_request(&server, &mut app, Some(&first)).await;
    assert_eq!(cell_of(&draw(&mut app), "Allow once (y)"), at);
    later(&mut app, 3);
    assert!(click(&mut app, at).is_empty(), "armed for another request");
    assert_eq!(app.active_request().unwrap().id(), second);

    // The second click on the same option replies `once`, as Enter would.
    later(&mut app, 4);
    let reply = click(&mut app, at);
    assert_eq!(
        reply,
        vec![Action::Reply {
            request: second.clone(),
            body: json!({ "reply": "once" }),
        }]
    );
    assert!(matches!(app.overlay, Overlay::None));
    perform(&server, &app, reply).await;
    wait_idle(&server, &id).await;
}

/// One question, then an answer once it is answered.
fn asks_the_database(body: &Value) -> Value {
    let text = body.to_string();
    if text.contains("Write a short title") {
        return json!({ "content": "Clicks" });
    }
    if text.contains("\"role\":\"tool\"") {
        return json!({ "content": "Done." });
    }
    let arguments = json!({ "questions": [{ "question": "Database?", "header": "DB",
        "options": [{ "label": "Postgres", "description": "a server" },
                    { "label": "SQLite", "description": "a file" }] }] });
    json!({ "tool_calls": [{ "index": 0, "id": "call_q", "type": "function",
        "function": { "name": "question", "arguments": arguments.to_string() } }] })
}

#[tokio::test]
async fn a_question_is_answered_by_a_second_click() {
    let server =
        Server::start_reply(asks_the_database, json!({ "main": {} }), json!({}), false).await;
    let id = server.session().await;
    let mut app = server.load(&id).await;
    server.prompt(&id, "Pick a database.").await;
    let request = next_request(&server, &mut app, None).await;
    let cursor = |app: &App| match &app.overlay {
        Overlay::Question(form) => form.cursor,
        other => panic!("{other:?}"),
    };

    assert!(click_text(&mut app, "Database?").is_empty(), "no row");
    assert_eq!(cursor(&app), 0);
    assert!(click_text(&mut app, "SQLite").is_empty(), "one click moves");
    assert_eq!(cursor(&app), 1);
    let answer = click_text(&mut app, "SQLite");
    assert_eq!(
        answer,
        vec![Action::Answer {
            request,
            body: json!({ "answers": [["SQLite"]] }),
        }]
    );
    perform(&server, &app, answer).await;
    assert!(wait_idle(&server, &id).await.contains("SQLite"));
}

fn picker(app: &App) -> &crate::app::Picker {
    match &app.overlay {
        Overlay::Picker(p) => p,
        other => panic!("{other:?}"),
    }
}

/// The label of the listed choice whose key is `key`.
fn label_of(app: &App, key: &str) -> String {
    let p = picker(app);
    let choice = p.items.iter().find(|c| c.key == key);
    choice
        .unwrap_or_else(|| panic!("{key}: {:?}", p.items))
        .label
        .clone()
}

#[tokio::test]
async fn picker_rows_select_then_activate() {
    let server = Server::start_models(json!({ "main": {}, "small": {} }), json!({}), false).await;
    let id = server.answered_session().await;
    let mut app = server.load(&id).await;
    let actions = typed(&mut app, "/model");
    drive(&server, &mut app, actions).await;
    // The catalog also lists the machine's own providers: search for this server's.
    for c in "test/".chars() {
        app.on_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
    }
    let label = label_of(&app, "test/small");
    assert!(
        click_text(&mut app, &label).is_empty(),
        "the first click selects"
    );
    let p = picker(&app);
    assert_eq!(p.items[p.filtered[p.selected]].key, "test/small");
    let switch = click_text(&mut app, &label);
    assert_eq!(switch, vec![Action::SwitchModel("test/small".into())]);
    assert!(matches!(app.overlay, Overlay::None));
    drive(&server, &mut app, switch).await;
    let session = server.client.get(&format!("/sessions/{id}")).await.unwrap();
    assert_eq!(session["data"]["model"], "test/small", "{session}");
}

/// The parent starts one `explore` child; the child answers at once.
#[cfg(unix)]
fn spawns_a_child(body: &Value) -> Value {
    let text = body.to_string();
    if text.contains("Write a short title") {
        return json!({ "content": crate::screens::TITLE });
    }
    if text.contains("\"role\":\"tool\"") {
        return json!({ "content": "Done." });
    }
    if text.contains("CHILD-TASK") {
        return json!({ "content": "explored" });
    }
    let arguments = json!({ "agent": "explore", "prompt": "CHILD-TASK" }).to_string();
    json!({ "tool_calls": [{ "index": 0, "id": "call_spawn", "type": "function",
        "function": { "name": "agent", "arguments": arguments } }] })
}

/// The background command runs `sleep`, as in `screens_tasks`, so it is Unix-only.
#[cfg(unix)]
#[tokio::test]
async fn tasks_and_agent_rows_activate_on_a_second_click() {
    let extra = json!({ "permissions": { "bash": "allow", "agent": "allow" },
        "sandbox": { "policy": "full-access" } });
    let server = Server::start_reply(spawns_a_child, json!({ "main": {} }), extra, false).await;
    let id = server.answered("SPAWN-CHILD").await;
    let mut app = server.load(&id).await;
    let command = "echo bg-line; sleep 300";
    drive(
        &server,
        &mut app,
        vec![Action::BackgroundShell(command.into())],
    )
    .await;
    let mut job = Value::Null;
    for _ in 0..1500 {
        let jobs = server
            .client
            .get(&format!("/jobs?session_id={id}"))
            .await
            .unwrap();
        job = jobs["data"][0].clone();
        let log = job["output_path"].as_str().map(std::fs::read_to_string);
        if log.is_some_and(|log| log.unwrap_or_default().contains("bg-line")) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let job_id = job["id"].as_str().expect("the background Job").to_owned();

    // `/tasks`: the first click selects the command, the second opens its log.
    let actions = typed(&mut app, "/tasks");
    drive(&server, &mut app, actions).await;
    let row = format!("{command} (running)");
    assert!(click_text(&mut app, &row).is_empty());
    assert!(matches!(app.overlay, Overlay::Picker(_)));
    let open = click_text(&mut app, &row);
    assert_eq!(open, vec![Action::OpenTask(job_id.clone())]);
    drive(&server, &mut app, open).await;
    assert!(
        matches!(&app.overlay, Overlay::TaskLog { text, .. } if text.contains("bg-line")),
        "{:?}",
        app.overlay
    );
    app.on_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    let stop = format!("/jobs/{job_id}/stop");
    server.client.post(&stop, json!({})).await.unwrap();

    // `/agent`: the second click on the child opens its thread.
    let children = server
        .client
        .get(&format!("/sessions/{id}/children?limit=10"))
        .await
        .unwrap();
    let child = children["data"]["data"][0]["session"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("{children}"))
        .to_owned();
    let actions = typed(&mut app, "/agent");
    drive(&server, &mut app, actions).await;
    let label = label_of(&app, &child);
    assert!(click_text(&mut app, &label).is_empty());
    assert_eq!(click_text(&mut app, &label), vec![Action::Open(child)]);
}

/// The column and row of the first screen row holding every one of `needles`, at the
/// first of them.
fn cell_with(buf: &Buffer, needles: &[&str]) -> (u16, u16) {
    let text = |y| -> String { (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect() };
    let y = (0..buf.area.height)
        .find(|&y| needles.iter().all(|n| text(y).contains(n)))
        .unwrap_or_else(|| panic!("{needles:?} are not on one row"));
    let row: Vec<&str> = (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect();
    let x = (0..row.len())
        .find(|&x| row[x..].concat().starts_with(needles[0]))
        .unwrap();
    (x as u16, y)
}

#[tokio::test]
async fn messages_team_and_tree_rows_activate_on_a_second_click() {
    let teams = json!({ "features": { "teams": true } });
    let server =
        Server::start_reply(crate::screens::reply, json!({ "main": {} }), teams, false).await;

    // `/tree`: two clicks check a branch out, only while the Session is idle.
    let (id, ids) = crate::screens_branches::two_branches(&server).await;
    let mut app = server.app(&id).await;
    let actions = typed(&mut app, "/tree");
    drive(&server, &mut app, actions).await;
    assert!(click_text(&mut app, "○ three").is_empty());
    assert_eq!(app.branches.selected_node().unwrap().id, ids[2]);
    app.session.running = true;
    assert!(click_text(&mut app, "○ three").is_empty(), "busy");
    assert!(app.toast.as_ref().unwrap().0.contains("must be idle"));
    app.session.running = false;
    assert!(click_text(&mut app, "○ three").is_empty(), "armed again");
    let checkout = click_text(&mut app, "○ three");
    assert_eq!(
        checkout,
        vec![Action::Branch(crate::branches::Request::Checkout {
            message_id: ids[2].clone(),
            code: false,
        })]
    );
    drive(&server, &mut app, checkout).await;
    assert_eq!(app.overlay, Overlay::None);

    // `/messages`: two held messages; the second click on one expands it.
    let new = |title: &str| json!({ "model": "test/main", "mode": "default", "title": title });
    let created = server
        .client
        .post("/sessions", new("frontend"))
        .await
        .unwrap();
    let a = created["data"]["id"].as_str().unwrap().to_owned();
    let created = server
        .client
        .post("/sessions", new("api-refactor"))
        .await
        .unwrap();
    let b = created["data"]["id"].as_str().unwrap().to_owned();
    let hold = json!({ "messaging": { "inbound": "hold" } });
    server
        .client
        .patch(&format!("/sessions/{b}"), hold)
        .await
        .unwrap();
    for (message, text) in [("msgx_first", "first note"), ("msgx_second", "second note")] {
        let body = json!({ "id": message, "from": a, "to": "api-refactor", "text": text });
        server.client.post("/messaging/send", body).await.unwrap();
    }
    let mut app = server.app(&b).await;
    let actions = typed(&mut app, "/messages");
    drive(&server, &mut app, actions).await;
    assert_eq!(app.overlay, Overlay::Messages);
    assert!(click_text(&mut app, "first note").is_empty());
    assert_eq!(app.messaging.selected.as_deref(), Some("msgx_first"));
    assert!(!app.messaging.expanded);
    click_text(&mut app, "first note");
    assert!(app.messaging.expanded);

    // `/team`: the second click on a member opens its Session.
    let definitions = server.root().join("config").join("teams");
    std::fs::create_dir_all(&definitions).unwrap();
    std::fs::write(
        definitions.join("feature.md"),
        crate::screens_team::DEFINITION,
    )
    .unwrap();
    let lead = server.session().await;
    let mut app = server.load(&lead).await;
    let actions = typed(&mut app, "/team start feature");
    drive(&server, &mut app, actions).await;
    assert_eq!(app.overlay, Overlay::Team, "{:?}", app.toast);
    let team = app.sidebar.team.ready().unwrap();
    let ui = team.members.iter().position(|m| m.name == "ui").unwrap();
    let session = team.members[ui].session_id.clone();
    let at = cell_with(&draw(&mut app), &[" ui ", "teammate"]);
    assert!(click(&mut app, at).is_empty());
    assert_eq!(app.team.selected, ui);
    assert_eq!(click(&mut app, at), vec![Action::Open(session)]);
    assert_eq!(app.overlay, Overlay::None);
}

/// The hook lists draw no rows to pick: `/hooks` shows one definition at a time and
/// `/hooks history` is a scrolled log, where Enter closes. A click inside them hits no
/// target, so it neither closes them nor reaches a trust change; the wheel still moves
/// them.
#[tokio::test]
async fn hook_lists_take_no_row_clicks() {
    let server = Server::start().await;
    let id = server.answered_session().await;
    let mut app = server.app(&id).await;
    for (command, text) in [
        ("/hooks", "Hook definitions"),
        ("/hooks history", "Recorded hook"),
    ] {
        let actions = typed(&mut app, command);
        drive(&server, &mut app, actions).await;
        let open = std::mem::discriminant(&app.overlay);
        for _ in 0..2 {
            assert!(click_text(&mut app, text).is_empty(), "{command}");
            assert!(click(&mut app, (1, 1)).is_empty(), "{command}: outside");
        }
        assert_eq!(std::mem::discriminant(&app.overlay), open, "{command}");
        app.on_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    }
    assert_eq!(app.overlay, Overlay::None);
}

/// Reads a 60-line file, then answers.
fn reads_a_long_file(body: &Value) -> Value {
    let text = body.to_string();
    if text.contains("Write a short title") {
        return json!({ "content": crate::screens::TITLE });
    }
    if text.contains("\"role\":\"tool\"") {
        return json!({ "content": "Done." });
    }
    let arguments = json!({ "path": "long.txt" }).to_string();
    json!({ "tool_calls": [{ "index": 0, "id": "call_read", "type": "function",
        "function": { "name": "read", "arguments": arguments } }] })
}

#[tokio::test]
async fn clicking_a_collapsed_tool_output_expands_it() {
    let server =
        Server::start_reply(reads_a_long_file, json!({ "main": {} }), json!({}), false).await;
    let lines: Vec<String> = (1..=60).map(|i| format!("line {i}")).collect();
    std::fs::write(server.root().join("long.txt"), lines.join("\n")).unwrap();
    let id = server.answered("Read long.txt").await;
    let mut app = server.app(&id).await;
    let hidden = "lines hidden (Ctrl+O to expand)";
    assert!(!app.expand_tools);
    // Two presses at the same instant: a double click toggles once.
    let at = cell_of(&draw(&mut app), hidden);
    assert!(click(&mut app, at).is_empty());
    assert!(app.expand_tools, "the click pressed tool_details");
    click(&mut app, at);
    assert!(app.expand_tools);
    assert!(!format!("{:?}", draw(&mut app).content).contains("lines hidden"));

    // The collapsed reasoning row presses reasoning_toggle.
    let Some(crate::model::Item::Assistant { reasoning, .. }) = app.items.last_mut() else {
        panic!("{:?}", app.items.last())
    };
    *reasoning = "weighing the options".into();
    click_text(&mut app, "∴ thinking");
    assert!(app.show_reasoning);
    assert!(row_of(&draw(&mut app), "weighing the options") > 0);

    // Unbound, neither row takes clicks.
    let mut warnings = Vec::new();
    let unbound = json!({ "tool_details": "none", "reasoning_toggle": "none" });
    app.keymap = crate::keys::Keymap::load(Some(&unbound), &mut warnings);
    assert!(warnings.is_empty(), "{warnings:?}");
    app.expand_tools = false;
    app.show_reasoning = false;
    for text in [hidden, "∴ thinking"] {
        assert!(click_text(&mut app, text).is_empty());
    }
    assert!(!app.expand_tools && !app.show_reasoning);
}

#[tokio::test]
async fn a_collapsed_row_click_takes_focus_back_from_the_sidebar_and_expands() {
    let server =
        Server::start_reply(reads_a_long_file, json!({ "main": {} }), json!({}), false).await;
    let lines: Vec<String> = (1..=60).map(|i| format!("line {i}")).collect();
    std::fs::write(server.root().join("long.txt"), lines.join("\n")).unwrap();
    let id = server.answered("Read long.txt").await;
    let mut app = server.app(&id).await;
    let hidden = "lines hidden (Ctrl+O to expand)";
    // Docked at 140 columns, with the focus in the sidebar.
    buffer(&mut app, 140, HEIGHT, &Opts::default());
    app.focus_sidebar();
    assert_eq!(app.sidebar.focus, Some(0));
    let at = cell_of(&buffer(&mut app, 140, HEIGHT, &Opts::default()), hidden);
    assert!(click(&mut app, at).is_empty());
    assert_eq!(app.sidebar.focus, None, "focus is back in the composer");
    assert!(app.expand_tools, "the row pressed tool_details");
}
