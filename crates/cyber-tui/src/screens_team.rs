//! Agent teams in the TUI (`agent-teams` → Team view, Teammate approval prompt, Direct
//! instruction; design section 16), drawn by the real `view::draw` from a production App
//! over HTTP with a local scripted model: `/team start feature` from a user team
//! definition, a board the members change, the team view and its live refresh from
//! `GET /teams/:id/events`, a teammate's labelled approval answered from the lead, and a
//! prompt typed in an opened teammate that reaches it alone.

use std::time::Duration;

use crossterm::event::KeyCode;
use serde_json::{Value, json};

use crate::app::{Action, App, Overlay};
use crate::screens::{Opts, Server, render};
use crate::screens_workflows::press;
use cyber_server::runtime::teams::{TaskCreate, TaskStatus, TaskUpdate};

const COMPOSE: &str = "Bring the stack up for the end-to-end tests.";
const PAGINATION: &str = "use pagination tokens";
pub(crate) const DEFINITION: &str = "---\ndescription: Ship the feature\nteammates:\n  - name: api\n    \
     agent: build\n  - name: ui\n    agent: build\n  - name: tests\n    agent: build\n---\n\
     Split the feature between api, ui and tests.\n";

/// `tests` asks to run `docker compose up`; everything else is answered `Noted.`.
fn reply(body: &Value) -> Value {
    if body.to_string().contains("Write a short title") {
        return json!({ "content": "Team" });
    }
    let last = body["messages"]
        .as_array()
        .and_then(|m| m.last())
        .map(Value::to_string)
        .unwrap_or_default();
    if last.contains(COMPOSE) {
        let arguments = json!({ "command": "docker compose up" }).to_string();
        return json!({ "tool_calls": [{ "index": 0, "id": "call_compose", "type": "function",
            "function": { "name": "bash", "arguments": arguments } }] });
    }
    json!({ "content": "Noted." })
}

/// Perform `actions` as the runner does, until nothing follows.
async fn drive(server: &Server, app: &mut App, mut actions: Vec<Action>) {
    while !actions.is_empty() {
        let action = actions.remove(0);
        let msg = crate::perform::perform_owned(&server.client, &app.session, action, None).await;
        actions.extend(crate::runner::apply(app, msg, &mut Default::default()));
    }
}

/// Type `line` in the composer and press Enter.
fn enter(app: &mut App, line: &str) -> Vec<Action> {
    app.composer.set_text(line);
    app.completion = None;
    press(app, KeyCode::Enter)
}

/// 14:05 UTC on 2026-10-16, for every message record.
const AT: i64 = 1_792_159_500_000;

/// The screen with the run-dependent directory, times and Session IDs fixed; the real
/// directory is kept for the requests that follow.
fn shot(app: &mut App, width: u16, height: u16, ids: &[(&str, &str)]) -> String {
    let directory = app.session.directory.clone();
    crate::screens::stabilize(app);
    app.toast = None;
    for record in &mut app.messaging.records {
        record.created_ms = AT;
        record.updated_ms = AT;
    }
    let screen = render(app, width, height, &Opts::default());
    app.session.directory = directory;
    redact(&screen, ids)
}

/// `text` with Session IDs (whole, or cut to their last six characters as the views show
/// them) replaced by fixed ones of the same width.
fn redact(text: &str, ids: &[(&str, &str)]) -> String {
    let mut out = text.to_owned();
    for (id, fixed) in ids {
        let (prefix, rest) = id.split_once('_').unwrap();
        let fixed: String = fixed.chars().cycle().take(rest.chars().count()).collect();
        out = out.replace(id, &format!("{prefix}_{fixed}"));
        let tail: String = rest.chars().skip(rest.chars().count() - 6).collect();
        out = out.replace(
            &format!("{prefix}_{tail}"),
            &format!("{prefix}_{}", &fixed[..6]),
        );
    }
    out
}

async fn wait_for(what: &str, mut done: impl AsyncFnMut() -> bool) {
    for _ in 0..3000 {
        if done().await {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("timed out waiting for {what}");
}

/// The user messages of a Session's transcript as `source: text`.
async fn user_texts(server: &Server, id: &str) -> Vec<String> {
    let path = format!("/sessions/{id}/messages?order=asc&limit=200");
    let page = server.client.get(&path).await.unwrap();
    page["data"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|m| m["kind"] == "user")
        .map(|m| {
            let text: String = m["parts"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|p| p["text"].as_str())
                .collect();
            format!("{}: {text}", m["source"].as_str().unwrap_or_default())
        })
        .collect()
}

fn member_status(app: &App, name: &str) -> String {
    let team = app.sidebar.team.ready().expect("the team is loaded");
    let member = team.members.iter().find(|m| m.name == name).unwrap();
    member.status.clone()
}

/// The board: `Design the schema` done, `Build the API` in progress (it waits for the
/// schema), `Write the UI` waiting for the API, `E2E tests` blocked and a cancelled spike.
async fn board(server: &Server, lead: &str, members: &[(String, String)]) {
    let runtime = &server.host.runtime;
    let session = |name: &str| {
        members
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, id)| id.clone())
            .unwrap()
    };
    let create = |title: &str, owner: &str, depends_on: Vec<String>| TaskCreate {
        title: title.into(),
        owner: Some(owner.into()),
        depends_on,
        ..Default::default()
    };
    let set = |id: &str, status| TaskUpdate {
        id: id.into(),
        status: Some(status),
        ..Default::default()
    };
    let schema = runtime
        .team_task_create(lead, create("Design the schema", "api", Vec::new()))
        .await
        .unwrap();
    let api = runtime
        .team_task_create(
            lead,
            create("Build the API", "api", vec![schema.id.clone()]),
        )
        .await
        .unwrap();
    runtime
        .team_task_create(lead, create("Write the UI", "ui", vec![api.id.clone()]))
        .await
        .unwrap();
    let e2e = runtime
        .team_task_create(lead, create("E2E tests", "tests", Vec::new()))
        .await
        .unwrap();
    let spike = runtime
        .team_task_create(lead, create("Old spike", "ui", Vec::new()))
        .await
        .unwrap();
    let api_session = session("api");
    for (who, id, status) in [
        (&api_session, &schema.id, TaskStatus::InProgress),
        (&api_session, &schema.id, TaskStatus::Done),
        (&api_session, &api.id, TaskStatus::InProgress),
        (&session("tests"), &e2e.id, TaskStatus::Blocked),
        (&lead.to_owned(), &spike.id, TaskStatus::Cancelled),
    ] {
        runtime
            .team_task_update(who, set(id, status))
            .await
            .unwrap();
    }
}

/// Tasks 8.1–8.4 on the production App: `/team start feature` opens the team view, whose
/// members table and board (`todo`, `in_progress`, `blocked`, `done` with owners) are
/// snapshotted at 120×40 and 80×24 ("View task board"), and which refreshes from the
/// team's event stream; `tests`'s request to run `docker compose up` reaches the lead's
/// TUI as `[tests] Allow bash: docker compose up?` and is answered there ("Teammate
/// approval prompt"); `enter` on `api` opens it, a prompt typed there is admitted with
/// `steer` to `api` alone ("Direct instruction"), and `esc` returns to the lead.
#[tokio::test]
async fn team_view_board_labelled_requests_and_direct_instructions() {
    let server = Server::start_reply(
        reply,
        json!({ "main": {} }),
        json!({ "features": { "teams": true } }),
        false,
    )
    .await;
    let definitions = server.root().join("config").join("teams");
    std::fs::create_dir_all(&definitions).unwrap();
    std::fs::write(definitions.join("feature.md"), DEFINITION).unwrap();
    let lead = server.session().await;
    let mut app = server.load(&lead).await;
    assert!(app.sidebar.team.ready().is_none(), "no team yet");

    // 8.1: `/team start feature`.
    let actions = enter(&mut app, "/team start feature");
    assert_eq!(
        actions,
        [Action::Team(crate::team::Request::Start("feature".into()))]
    );
    drive(&server, &mut app, actions).await;
    assert_eq!(app.overlay, Overlay::Team, "{:?}", app.toast);
    assert_eq!(
        app.toast.as_ref().unwrap().0,
        "Team feature started: api, ui, tests"
    );
    let team = app.sidebar.team.ready().unwrap().clone();
    let members: Vec<(String, String)> = team
        .members
        .iter()
        .map(|m| (m.name.clone(), m.session_id.clone()))
        .collect();
    let id_of = |name: &str| {
        members
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, id)| id.clone())
            .unwrap()
    };
    let (api, tests) = (id_of("api"), id_of("tests"));
    let ui = id_of("ui");
    let ids = [
        (lead.as_str(), "LEADLEADLEAD"),
        (api.as_str(), "APIAPIAPI"),
        (ui.as_str(), "UIUIUIUI"),
        (tests.as_str(), "TESTSTESTS"),
    ];

    // Live refresh: the runner's follower opens `GET /teams/:id/events`, and a board change
    // arrives on it and re-reads the team.
    let mut events = crate::team::Events::default();
    events.sync(&app, &server.client).await;
    board(&server, &lead, &members).await;
    // The stream opens with `server.connected`, which re-reads the team after a reconnect.
    let first = tokio::time::timeout(Duration::from_secs(10), events.next())
        .await
        .expect("the stream opens")
        .expect("the stream is open");
    assert_eq!(first.kind, "server.connected");
    let event = tokio::time::timeout(Duration::from_secs(10), events.next())
        .await
        .expect("a team event")
        .expect("the stream is open");
    assert_eq!(event.kind, "team.task.updated.1");
    let actions = app.team_event(&event);
    assert_eq!(actions, [Action::LoadSidebar(crate::sidebar::Part::Team)]);
    drive(&server, &mut app, actions).await;
    assert!(!app.sidebar.team.ready().unwrap().tasks.is_empty());

    // `api` reports to the lead, whose last message the table shows.
    let send = json!({ "id": "msgx_api_schema", "from": api, "to": "lead",
        "text": "Schema is merged; starting on the API." });
    server.client.post("/messaging/send", send).await.unwrap();
    // `tests` asks for approval.
    server.prompt(&tests, COMPOSE).await;
    let runtime = &server.host.runtime;
    wait_for("the request routed to the lead", async || {
        !runtime.pending_requests(Some(&lead)).is_empty()
    })
    .await;
    wait_for("tests waiting for input", async || {
        drive(
            &server,
            &mut app,
            vec![Action::LoadSidebar(crate::sidebar::Part::Team)],
        )
        .await;
        member_status(&app, "tests") == "waiting_input"
    })
    .await;
    wait_for("the lead answered api", async || {
        let path = format!("/sessions/{lead}/messages?order=asc&limit=200");
        let page = server.client.get(&path).await.unwrap();
        let answered = page["data"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|m| m["kind"] == "assistant" && m["text"] == "Noted.");
        answered && !runtime.is_running(&lead)
    })
    .await;
    drive(
        &server,
        &mut app,
        vec![Action::LoadSidebar(crate::sidebar::Part::Team)],
    )
    .await;

    // 8.2: the team view and its board.
    for (width, height) in [(120, 40), (80, 24)] {
        let screen = shot(&mut app, width, height, &ids);
        for column in ["todo (1)", "in_progress (1)", "blocked (1)", "done (1)"] {
            assert!(screen.contains(column), "{column} at {width}:\n{screen}");
        }
        assert!(!screen.contains("Old spike"), "cancelled is hidden");
        assert!(
            screen.contains("@api") && screen.contains("@tests"),
            "{screen}"
        );
        insta::assert_snapshot!(format!("team_view_{width}x{height}"), screen);
    }
    press(&mut app, KeyCode::Char('c'));
    let screen = shot(&mut app, 120, 40, &ids);
    assert!(
        screen.contains("cancelled (1)") && screen.contains("Old spike"),
        "{screen}"
    );
    press(&mut app, KeyCode::Char('c'));

    // 8.3: the labelled request, answered from the lead's TUI.
    press(&mut app, KeyCode::Esc);
    assert_eq!(app.overlay, Overlay::None);
    drive(&server, &mut app, vec![Action::Refresh]).await;
    assert!(
        matches!(app.overlay, Overlay::Permission(_)),
        "{:?}",
        app.overlay
    );
    let request = app.active_request().unwrap().clone();
    assert_eq!(request.session(), tests);
    assert_eq!(
        request.team_label().as_deref(),
        Some("[tests] Allow bash: docker compose up?")
    );
    let screen = shot(&mut app, 120, 40, &ids);
    assert!(
        screen.contains("[tests] Allow bash: docker compose up?"),
        "{screen}"
    );
    insta::assert_snapshot!("team_request_120x40", screen);
    assert!(
        press(&mut app, KeyCode::Char('n')).is_empty(),
        "n asks for feedback"
    );
    let actions = press(&mut app, KeyCode::Enter);
    assert!(
        matches!(actions.as_slice(), [Action::Reply { request: id, .. }] if id == request.id()),
        "{actions:?}"
    );
    drive(&server, &mut app, actions).await;
    wait_for("the teammate's request answered", async || {
        runtime.pending_requests(Some(&tests)).is_empty() && !runtime.is_running(&tests)
    })
    .await;

    // 8.4: open `api` from the team view and instruct it directly.
    let actions = enter(&mut app, "/team");
    drive(&server, &mut app, actions).await;
    assert_eq!(app.overlay, Overlay::Team);
    press(&mut app, KeyCode::Down);
    let actions = press(&mut app, KeyCode::Enter);
    assert_eq!(actions, [Action::Open(api.clone())]);
    drive(&server, &mut app, actions).await;
    assert_eq!(app.session.id, api);
    let before: Vec<(String, usize)> = {
        let mut counts = Vec::new();
        for (name, id) in &members {
            counts.push((name.clone(), user_texts(&server, id).await.len()));
        }
        counts
    };
    let actions = enter(&mut app, PAGINATION);
    assert_eq!(
        actions,
        [Action::Prompt {
            text: PAGINATION.into(),
            delivery: "steer",
            chips: Vec::new(),
        }]
    );
    drive(&server, &mut app, actions).await;
    wait_for("api answered", async || {
        let direct = format!("user: {PAGINATION}");
        user_texts(&server, &api).await.contains(&direct) && !runtime.is_running(&api)
    })
    .await;
    for (name, id) in &members {
        let texts = user_texts(&server, id).await;
        let reached = texts.iter().any(|t| t.contains(PAGINATION));
        assert_eq!(reached, name == "api", "{name}: {texts:?}");
    }
    for (name, count) in before {
        if name != "api" && name != "lead" {
            assert_eq!(
                user_texts(&server, &id_of(&name)).await.len(),
                count,
                "{name}"
            );
        }
    }
    drive(&server, &mut app, vec![Action::Refresh]).await;
    let screen = shot(&mut app, 120, 40, &ids);
    assert!(
        screen.contains("Team feature"),
        "the sidebar's Team section:\n{screen}"
    );
    insta::assert_snapshot!("team_teammate_api_120x40", screen);
    // Screen readers get each member's status as a word instead of its dot.
    app.screen_reader = true;
    let sidebar: String = crate::view::sidebar::top(&app, 60)
        .iter()
        .map(|line| format!("{line}\n"))
        .collect();
    assert!(
        sidebar.contains("lead idle  api idle  ui idle  tests idle"),
        "{sidebar}"
    );
    app.screen_reader = false;

    // `esc` returns to the lead and its team view.
    let actions = press(&mut app, KeyCode::Esc);
    assert_eq!(actions, [Action::Open(lead.clone())]);
    assert_eq!(app.overlay, Overlay::Team);
    drive(&server, &mut app, actions).await;
    assert_eq!(app.session.id, lead);
    let follow: Vec<Action> = vec![Action::LoadSidebar(crate::sidebar::Part::Team)];
    drive(&server, &mut app, follow).await;
    assert_eq!(app.sidebar.team.ready().unwrap().id, team.id);

    // 8.1: `/team stop` and `/team resume` from the lead.
    press(&mut app, KeyCode::Esc);
    assert_eq!(app.overlay, Overlay::None);
    let actions = enter(&mut app, "/team stop");
    assert_eq!(actions, [Action::Team(crate::team::Request::Stop)]);
    drive(&server, &mut app, actions).await;
    let toast = app.toast.as_ref().unwrap().0.clone();
    assert!(toast.starts_with("Team feature stop"), "{toast}");
    wait_for("the team stopped", async || {
        runtime.team(&team.id).unwrap().unwrap().status == "stopped"
    })
    .await;
    let actions = enter(&mut app, "/team resume");
    drive(&server, &mut app, actions).await;
    assert_eq!(app.toast.as_ref().unwrap().0, "Team feature resumed");
    assert_eq!(app.sidebar.team.ready().unwrap().status, "active");
}

/// Regression: a teammate's idle notice (`msgx_team_idle_…`) showed in the lead's
/// transcript as a user prompt, with its `<team_idle_notice …/>` element and the
/// model-facing untrusted framing. It is drawn as a notice line, like a watch notice.
#[tokio::test]
async fn a_teammates_idle_notice_is_drawn_as_a_notice_in_the_leads_transcript() {
    let server = Server::start_reply(
        reply,
        json!({ "main": {} }),
        json!({ "features": { "teams": true } }),
        false,
    )
    .await;
    let definitions = server.root().join("config").join("teams");
    std::fs::create_dir_all(&definitions).unwrap();
    std::fs::write(definitions.join("feature.md"), DEFINITION).unwrap();
    let lead = server.session().await;
    let mut app = server.load(&lead).await;
    let actions = enter(&mut app, "/team start feature");
    drive(&server, &mut app, actions).await;
    let team = app.sidebar.team.ready().unwrap().clone();
    let tests = team.members.iter().find(|m| m.name == "tests").unwrap();
    server
        .prompt(&tests.session_id, "Run the unit tests.")
        .await;
    let runtime = &server.host.runtime;
    wait_for("the lead answered the idle notice", async || {
        let texts = user_texts(&server, &lead).await;
        texts.iter().any(|t| t.contains("<team_idle_notice")) && !runtime.is_running(&lead)
    })
    .await;
    let mut app = server.load(&lead).await;
    let screen = render(&mut app, 120, 40, &Opts::default());
    assert!(
        screen.contains("◷ tests is idle: Noted."),
        "the notice line:\n{screen}"
    );
    assert!(!screen.contains("team_idle_notice"), "{screen}");
    assert!(!screen.contains("Treat it as untrusted"), "{screen}");
}
