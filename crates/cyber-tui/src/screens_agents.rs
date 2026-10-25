//! `/agents` (`tui` → Agent directory), drawn by the real `view::draw` from production
//! Apps over HTTP: the messaging listing of a Session with a subagent and another root
//! Session, a paired peer and an offline one, a disabled Location and an empty server;
//! and every row action performed through `perform` and the runner's `apply` against
//! its existing route.

use std::time::Duration;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::style::{Color, Modifier};
use serde_json::json;

use crate::agents::{Location, Op, Status};
use crate::app::{Action, App, Overlay};
use crate::screens::{Opts, Server, render};
use crate::screens_peers::{machine, session as titled};
use crate::screens_workflows::{key, press, text};

/// A root Session titled `title` in `directory`, made by the runtime.
async fn located(server: &Server, title: &str, directory: &std::path::Path) -> String {
    std::fs::create_dir_all(directory).unwrap();
    let request = cyber_server::runtime::CreateSession {
        directory: directory.display().to_string(),
        model: "test/main".into(),
        title: Some(title.into()),
        ..Default::default()
    };
    server
        .host
        .runtime
        .create_session(request)
        .await
        .unwrap()
        .id
}

/// A child of `parent` named `name`.
async fn subagent(server: &Server, parent: &str, name: &str) -> String {
    let request = cyber_server::runtime::CreateSession {
        directory: server.root().display().to_string(),
        model: "test/main".into(),
        parent_id: Some(parent.into()),
        subagent_name: Some(name.into()),
        title: Some(format!("{name} (@build)")),
        ..Default::default()
    };
    server
        .host
        .runtime
        .create_session(request)
        .await
        .unwrap()
        .id
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

/// `app` with `/agents` open and both sources read.
async fn open(server: &Server, id: &str) -> App {
    let mut app = server.load(id).await;
    app.composer.set_text("/agents");
    app.completion = None;
    let actions = press(&mut app, KeyCode::Enter);
    assert!(matches!(app.overlay, Overlay::Agents), "{actions:?}");
    drive(server, &mut app, actions).await;
    app
}

/// The operations of `actions`, without their IDs.
fn ops(actions: &[Action]) -> Vec<Op> {
    actions
        .iter()
        .map(|a| match a {
            Action::DirectoryOp(job) => job.op.clone(),
            other => panic!("{other:?}"),
        })
        .collect()
}

fn ctrl(app: &mut App, c: char) -> Vec<Action> {
    app.on_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL))
}

fn typed(app: &mut App, line: &str) {
    for c in line.chars() {
        key(app, c);
    }
}

fn screen(app: &mut App, width: u16, height: u16) -> String {
    text(&crate::screens::buffer(
        app,
        width,
        height,
        &Opts::default(),
    ))
}

fn toast(app: &App) -> String {
    app.toast.as_ref().map(|t| t.0.clone()).unwrap_or_default()
}

/// The screen line holding `needle`.
fn line_of(screen: &str, needle: &str) -> String {
    screen
        .lines()
        .find(|l| l.contains(needle))
        .unwrap_or_else(|| panic!("{needle:?} is not on screen:\n{screen}"))
        .to_owned()
}

/// What differs between runs: the temporary directories rows show.
fn stabilize(app: &mut App, root: &std::path::Path) {
    crate::screens::stabilize(app);
    let root = root.display().to_string();
    for row in &mut app.directory.messaging.rows {
        if let Location::Local(directory) = &mut row.location {
            *directory = directory.replace(&root, "/Users/leo/work/app");
        }
    }
}

/// Scenario "Local Sessions and a subagent": `tests` is a subagent and `migration` a
/// Session in another directory; the attached Session is not listed. Drawn dark, light
/// and without color, where no cell has a color and the selection is reverse video.
#[tokio::test]
async fn agents_lists_local_sessions_and_subagents() {
    let server = machine(json!({})).await;
    let review = titled(&server, "review").await;
    located(
        &server,
        "migration",
        &server.root().join("services").join("db"),
    )
    .await;
    subagent(&server, &review, "tests").await;
    let mut app = open(&server, &review).await;
    assert_eq!(app.directory.messaging.status, Status::Ready);
    let rows: Vec<(String, String)> = app
        .directory
        .visible()
        .iter()
        .map(|e| (e.address.clone(), e.kind.label().to_owned()))
        .collect();
    assert_eq!(
        rows,
        [
            ("tests".to_owned(), "subagent".to_owned()),
            ("migration".to_owned(), "session".to_owned())
        ]
    );
    assert!(
        app.directory
            .visible()
            .iter()
            .all(|e| e.session_id.as_deref() != Some(&review))
    );
    stabilize(&mut app, &server.root());
    for (name, opts) in [
        (
            "agents_dark_160x48",
            Opts {
                legend: &["○ tests"],
                ..Opts::default()
            },
        ),
        (
            "agents_light_160x48",
            Opts {
                light: true,
                legend: &["○ tests"],
                ..Opts::default()
            },
        ),
        (
            "agents_no_color_160x48",
            Opts {
                no_color: true,
                ..Opts::default()
            },
        ),
    ] {
        let shown = render(&mut app, 160, 48, &opts);
        for needle in ["Agents · review", "This server", "[all]", "esc clear/close"] {
            assert!(shown.contains(needle), "{needle:?}:\n{shown}");
        }
        let migration = line_of(&shown, "○ migration");
        // The server reports the directory with native separators.
        let location = format!("~/work/app{0}services{0}db", std::path::MAIN_SEPARATOR);
        for column in ["session", "idle", "open", &location] {
            assert!(migration.contains(column), "{column:?}: {migration}");
        }
        if cfg!(not(windows)) {
            insta::assert_snapshot!(name, shown);
        }
    }
    let opts = Opts {
        no_color: true,
        ..Opts::default()
    };
    let buf = crate::screens::buffer(&mut app, 160, 48, &opts);
    for cell in crate::screens::cells(&buf) {
        assert_eq!((cell.fg, cell.bg), (Color::Reset, Color::Reset), "{cell:?}");
    }
    let y = crate::screens::row_of(&buf, "○ tests");
    let x = (0..160).find(|&x| buf[(x, y)].symbol() == "t").unwrap();
    assert!(buf[(x, y)].modifier.contains(Modifier::REVERSED));
}

/// Scenario "Message from the directory": `migr` filters to `migration`, Enter opens
/// the editor and Enter sends from the attached Session through `POST /messaging/send`.
#[tokio::test]
async fn agents_message_from_directory() {
    let server = machine(json!({})).await;
    let review = titled(&server, "review").await;
    let migration = titled(&server, "migration").await;
    subagent(&server, &review, "tests").await;
    let mut app = open(&server, &review).await;
    typed(&mut app, "migr");
    assert_eq!(app.directory.visible().len(), 1);
    assert!(press(&mut app, KeyCode::Enter).is_empty());
    assert!(screen(&mut app, 120, 30).contains("Message to migration"));
    app.on_paste("schema is merged");
    let actions = press(&mut app, KeyCode::Enter);
    assert_eq!(
        ops(&actions),
        [Op::Send {
            to: "migration".into(),
            text: "schema is merged".into(),
            notify: false,
        }]
    );
    drive(&server, &mut app, actions).await;
    let outcome = toast(&app);
    assert!(
        outcome.starts_with("delivered to migration · msgx_"),
        "{outcome}"
    );
    assert!(app.directory.editor.is_none());
    let names: Vec<String> = app
        .directory
        .visible()
        .iter()
        .map(|e| e.address.clone())
        .collect();
    assert_eq!(
        names,
        ["migration"],
        "the filter still applies after the refresh"
    );
    let rows = server.host.runtime.message_rows(&migration).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].peer_session_id.as_deref(), Some(review.as_str()));
}

/// Ctrl+W watches through `POST /messaging/watch`; Ctrl+G opens `/messages` on its held
/// filter at the newest message held from that row.
#[tokio::test]
async fn agents_watch_and_held_jump() {
    let server = machine(json!({})).await;
    let review = titled(&server, "review").await;
    let migration = titled(&server, "migration").await;
    let hold = json!({ "messaging": { "inbound": "hold" } });
    server
        .client
        .patch(&format!("/sessions/{review}"), hold)
        .await
        .unwrap();
    let body =
        json!({ "id": "msgx_held_one", "from": migration, "to": "review", "text": "ready?" });
    let sent = server.client.post("/messaging/send", body).await.unwrap();
    assert_eq!(sent["data"]["outcome"], "held", "{sent}");
    let mut app = open(&server, &review).await;
    typed(&mut app, "migration");
    let actions = ctrl(&mut app, 'w');
    assert_eq!(
        ops(&actions),
        [Op::Watch {
            target: "migration".into()
        }]
    );
    drive(&server, &mut app, actions).await;
    let outcome = toast(&app);
    assert!(
        outcome.starts_with("Watching migration until "),
        "{outcome}"
    );
    // The server recorded review's watch on migration (already idle, so it fired).
    let watches = server.host.runtime.watch_rows(&review).unwrap();
    let watched: Vec<(&str, &str)> = watches
        .iter()
        .map(|w| (w.target_address.as_str(), w.target_id.as_str()))
        .collect();
    assert_eq!(watched, [("migration", migration.as_str())]);
    let actions = ctrl(&mut app, 'g');
    assert_eq!(actions, [Action::Refresh]);
    drive(&server, &mut app, actions).await;
    assert!(matches!(app.overlay, Overlay::Messages));
    assert_eq!(app.messaging.filter, crate::messaging::Filter::Held);
    assert_eq!(app.messaging.selected.as_deref(), Some("msgx_held_one"));
    assert!(screen(&mut app, 120, 30).contains("[held]"));
}

/// Scenario "Rename a local Session": a Session listed by its ID (its title is shared,
/// so no name addresses it) gets a title through the rename route and is then listed by
/// it.
#[tokio::test]
async fn agents_rename_local_session() {
    let server = machine(json!({})).await;
    let review = titled(&server, "review").await;
    let untitled = titled(&server, "scratch").await;
    titled(&server, "scratch").await;
    let mut app = open(&server, &review).await;
    typed(&mut app, &untitled);
    assert_eq!(app.directory.current().unwrap().address, untitled);
    assert!(ctrl(&mut app, 'r').is_empty());
    assert!(screen(&mut app, 120, 30).contains(&format!("Rename {untitled}")));
    // The editor starts from the current title.
    assert_eq!(app.directory.editor.as_ref().unwrap().input, "scratch");
    ctrl(&mut app, 'u');
    typed(&mut app, "api-server");
    let actions = press(&mut app, KeyCode::Enter);
    assert_eq!(
        ops(&actions),
        [Op::Rename {
            id: untitled.clone(),
            title: "api-server".into()
        }]
    );
    drive(&server, &mut app, actions).await;
    assert_eq!(toast(&app), "Renamed to api-server");
    press(&mut app, KeyCode::Esc);
    let row = app
        .directory
        .visible()
        .into_iter()
        .find(|e| e.session_id.as_deref() == Some(&untitled))
        .cloned()
        .unwrap();
    assert_eq!(row.address, "api-server");
    let shown = screen(&mut app, 120, 30);
    assert!(line_of(&shown, "api-server").contains("session"), "{shown}");
}

/// Scenarios "Offline peer" and "Crafted peer name": laptop pairs with desk (a grant
/// on desk, a pinned remote on laptop) and has a dead remote `nas`. `*@nas` is listed
/// offline and closed and Enter on it opens no editor; desk's Session whose title holds
/// a bidi override is shown without it, and a message to it goes to its Session ID.
#[tokio::test]
async fn agents_offline_peer_and_crafted_name() {
    use cyber_server::runtime::peers::{NewGrant, grants};
    let desk = machine(json!({ "peers": { "machine": "desk" } })).await;
    let (url, fingerprint) = crate::screens_peers::listen(&desk).await;
    let grant = NewGrant {
        name: "laptop".into(),
        scopes: vec![],
        expires_ms: None,
    };
    let now = crate::screens_peers::now_ms();
    let token = grants::create(&desk.host.store, grant, now).unwrap().token;
    let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let dead = format!("https://{}", closed.local_addr().unwrap());
    drop(closed);
    let laptop = machine(json!({
        "peers": { "machine": "laptop", "remotes": [
            { "name": "desk", "url": url, "fingerprint": fingerprint, "token": token },
            { "name": "nas", "url": dead, "fingerprint": fingerprint, "token": "cyp_pgrx_unused" },
        ]},
        "permissions": { "message.send": { "*@desk": "allow" } },
    }))
    .await;
    let crafted = titled(&desk, "mig\u{202e}ration").await;
    let review = titled(&laptop, "review").await;
    let mut app = open(&laptop, &review).await;

    let shown = screen(&mut app, 160, 40);
    assert!(shown.contains("Peers"), "{shown}");
    let offline = line_of(&shown, "*@nas");
    for column in ["peer", "offline", "closed", "peer:nas"] {
        assert!(offline.contains(column), "{column:?}: {offline}");
    }
    assert!(!shown.contains('\u{202e}'), "{shown}");
    let row = line_of(&shown, "migration@desk");
    assert!(row.contains("peer:desk"), "{row}");

    typed(&mut app, "*@nas");
    press(&mut app, KeyCode::Enter);
    assert!(app.directory.editor.is_none());
    assert!(toast(&app).contains("cannot receive"), "{}", toast(&app));
    press(&mut app, KeyCode::Esc);

    typed(&mut app, "migration@desk");
    let entry = app.directory.current().cloned().unwrap();
    assert_eq!(
        entry.target.as_deref(),
        Some(format!("{crafted}@desk").as_str())
    );
    assert_eq!(entry.session_id, None, "a peer row never opens as local");
    assert!(ctrl(&mut app, 'o').is_empty());
    assert!(toast(&app).contains(&format!("cyber attach {crafted}@desk")));
    ctrl(&mut app, 'r');
    assert!(app.directory.editor.is_none());
    press(&mut app, KeyCode::Enter);
    app.on_paste("nightly is red");
    let actions = press(&mut app, KeyCode::Enter);
    assert_eq!(
        ops(&actions),
        [Op::Send {
            to: format!("{crafted}@desk"),
            text: "nightly is red".into(),
            notify: false,
        }]
    );
    drive(&laptop, &mut app, actions).await;
    let outcome = toast(&app);
    assert!(
        outcome.contains(&format!("{crafted}@desk · msgx_")),
        "{outcome}"
    );
    let received = desk.host.runtime.message_rows(&crafted).unwrap();
    assert_eq!(received.len(), 1, "desk records it");
}

/// Messaging disabled in the Location: the listing still answers, every row closed,
/// and Enter says why instead of opening the editor; a server answering 503 (as the
/// send route does) is drawn as disabled, without an empty-state claim.
#[tokio::test]
async fn agents_messaging_disabled() {
    let server = machine(json!({ "messaging": { "enabled": false } })).await;
    let review = titled(&server, "review").await;
    titled(&server, "migration").await;
    let mut app = open(&server, &review).await;
    let shown = screen(&mut app, 120, 30);
    assert!(line_of(&shown, "migration").contains("closed"), "{shown}");
    press(&mut app, KeyCode::Enter);
    assert!(app.directory.editor.is_none());
    assert!(toast(&app).contains("cannot receive"), "{}", toast(&app));
    let actions = app.agents_request();
    let Action::Directory(request) = &actions[0] else {
        panic!("{actions:?}");
    };
    let fetched = crate::agents::Fetched {
        status: Status::Disabled,
        ..Default::default()
    };
    let generation = request.generation;
    app.agents_reply(
        &review,
        generation,
        crate::agents::Source::Messaging,
        Ok(fetched),
    );
    let hub = crate::agents::Fetched {
        status: Status::Inactive,
        ..Default::default()
    };
    app.agents_reply(&review, generation, crate::agents::Source::Hub, Ok(hub));
    let shown = screen(&mut app, 120, 30);
    assert!(
        shown.contains("messaging is disabled here (messaging.enabled)"),
        "{shown}"
    );
    assert!(!shown.contains("No other Session"), "{shown}");
}

/// A server with only the attached Session: the empty state names it and points to
/// peer pairing and `/collab`; Esc closes, and the palette lists `/agents` with no key.
#[tokio::test]
async fn agents_empty() {
    let server = machine(json!({})).await;
    let review = titled(&server, "review").await;
    let mut app = open(&server, &review).await;
    assert!(app.directory.empty());
    let shown = screen(&mut app, 120, 30);
    for needle in [
        "No other Session can be reached from review.",
        "cyber peers add",
        "/collab",
    ] {
        assert!(shown.contains(needle), "{needle:?}:\n{shown}");
    }
    assert!(press(&mut app, KeyCode::Enter).is_empty());
    assert_eq!(toast(&app), "No row selected");
    press(&mut app, KeyCode::Esc);
    assert!(matches!(app.overlay, Overlay::None));
    assert_eq!(app.keymap.label("global", "agents_view"), None);
    app.on_key(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::CONTROL));
    typed(&mut app, "agents");
    let actions = press(&mut app, KeyCode::Enter);
    assert!(matches!(app.overlay, Overlay::Agents), "{actions:?}");
    // The poll waits while both sources load, then every ten seconds.
    assert!(app.agents_poll().is_empty());
    drive(&server, &mut app, actions).await;
    assert!(app.agents_poll().is_empty());
    tokio::time::sleep(Duration::from_millis(10)).await;
    assert!(app.agents_poll().is_empty(), "not due yet");
}

/// Scenario "Hub agent": collaboration is enabled, the Session is bound to a project
/// and another Session of the checkout is published as the agent `reviewer`; Project
/// hub lists it as `hub agent` at `hub:<project>`, Enter sends a direct hub message,
/// Ctrl+W and Ctrl+R say why not. The attached Session, published as `self`, is not
/// listed.
#[tokio::test]
#[cfg_attr(
    not(target_os = "linux"),
    ignore = "uses Linux temporary protected files; avoids native vault writes"
)]
async fn agents_hub_agent_row() {
    let hub = crate::screens_collaboration::Bound::start().await;
    let server = &hub.server;
    let reviewer = server.session().await;
    for (session, name) in [(&reviewer, "reviewer"), (&hub.session, "self")] {
        server
            .client
            .post(
                "/collaboration/owner/publish",
                json!({"operation_id": format!("cop_agents_{name}"), "session_id": session,
                    "project": hub.project, "name": name}),
            )
            .await
            .unwrap();
    }
    let mut app = open(server, &hub.session).await;
    assert!(
        app.directory.hub.rows.iter().all(|e| e.address != "self"),
        "{:?}",
        app.directory.hub.rows
    );
    assert_eq!(
        app.directory.hub.status,
        Status::Ready,
        "{:?}",
        app.directory.hub
    );
    typed(&mut app, "reviewer");
    let entry = app
        .directory
        .current()
        .cloned()
        .expect("the hub lists reviewer");
    assert_eq!(entry.kind.label(), "hub agent");
    assert_eq!(entry.location.label(), format!("hub:{}", hub.project));
    let shown = screen(&mut app, 140, 32);
    // The filter line also shows "reviewer", so match the row by its glyph.
    assert!(
        line_of(&shown, "● reviewer").contains("hub agent"),
        "{shown}"
    );
    assert!(shown.contains("Project hub"), "{shown}");
    for (c, reason) in [('w', "watch does not apply"), ('r', "cannot be renamed")] {
        assert!(ctrl(&mut app, c).is_empty());
        assert!(toast(&app).contains(reason), "{c}: {}", toast(&app));
    }
    press(&mut app, KeyCode::Enter);
    app.on_paste("please review");
    let actions = press(&mut app, KeyCode::Enter);
    assert!(
        matches!(&ops(&actions)[..], [Op::HubSend { .. }]),
        "{actions:?}"
    );
    drive(server, &mut app, actions).await;
    assert!(toast(&app).starts_with("Hub message"), "{}", toast(&app));
    assert!(!server.host.runtime.is_running(&hub.session));
    assert!(!server.host.runtime.is_running(&reviewer));
}
