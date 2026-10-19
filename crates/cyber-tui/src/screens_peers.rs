//! Messages between machines in the TUI (`tui` → Cross-session message surfaces;
//! `cross-session-messaging` → Direct peer transport), drawn by the real `view::draw` from
//! two production Apps over HTTP on this host: `laptop` pairs with `desk` (a grant on
//! desk, a pinned remote with its token on laptop), sends to a Session on desk through
//! desk's TLS peer listener, and both TUIs show it.

use std::time::Duration;

use crossterm::event::KeyCode;
use serde_json::{Value, json};

use crate::app::{Action, App};
use crate::screens::{Opts, Server, render};
use crate::screens_workflows::{key, press, text};

const NIGHTLY: &str = "The nightly build on desk is red; can you check the migration order?";

/// Every Turn answers `Noted.`.
fn reply(body: &Value) -> Value {
    if body.to_string().contains("Write a short title") {
        return json!({ "content": "Peers" });
    }
    json!({ "content": "Noted." })
}

/// A machine: a production App with `peers` (and anything else) in its user config.
async fn machine(config: Value) -> Server {
    Server::start_reply(reply, json!({ "main": {} }), config, false).await
}

async fn session(server: &Server, title: &str) -> String {
    let body = json!({ "model": "test/main", "mode": "default", "title": title });
    let created = server.client.post("/sessions", body).await.unwrap();
    created["data"]["id"].as_str().unwrap().to_owned()
}

/// desk's peer listener with TLS on a loopback port, as `peers.listen` opens it; its URL
/// and fingerprint.
async fn listen(desk: &Server) -> (String, String) {
    use cyber_server::http::peer::{self, PeerInfo, tls};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let names = tls::SubjectNames {
        machine: "desk".into(),
        address,
    };
    let state = desk.root().join("state").join(tls::DIR);
    let identity = tls::PeerIdentity::load_or_create(&state, &names).unwrap();
    let info = PeerInfo {
        machine: "desk".into(),
        fingerprint: identity.fingerprint().to_string(),
        share_paths: false,
        attach_wait: peer::ATTACH_WAIT,
    };
    tokio::spawn(cyber_server::http::serve_peer(
        peer::router(desk.host.state.clone(), info),
        listener,
        Some(identity.server_config().unwrap()),
        std::future::pending::<()>(),
    ));
    (
        format!("https://{address}"),
        identity.fingerprint().to_string(),
    )
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}

/// Perform `actions` as the runner does, until nothing follows.
async fn drive(server: &Server, app: &mut App, mut actions: Vec<Action>) {
    while !actions.is_empty() {
        let action = actions.remove(0);
        let msg = crate::perform::perform_owned(&server.client, &app.session, action, None).await;
        actions.extend(crate::runner::apply(app, msg, &mut Default::default()));
    }
}

/// 14:05 UTC on 2026-10-16, for every record.
const AT: i64 = 1_792_159_500_000;

/// `text` with the run-dependent Session IDs (whole, or cut to their last six characters
/// as the views show them) replaced by fixed ones of the same width.
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

fn shot(app: &mut App, width: u16, ids: &[(&str, &str)]) -> String {
    crate::screens::stabilize(app);
    for record in &mut app.messaging.records {
        record.created_ms = AT;
        record.updated_ms = AT;
    }
    redact(&render(app, width, 30, &Opts::default()), ids)
}

fn screen(app: &mut App, width: u16) -> String {
    text(&crate::screens::buffer(app, width, 30, &Opts::default()))
}

/// Wait until `id` is idle with an assistant message reading `Noted.`.
async fn answered(server: &Server, id: &str) {
    for _ in 0..3000 {
        let path = format!("/sessions/{id}/messages?order=asc&limit=200");
        let page = server.client.get(&path).await.unwrap();
        let noted = page["data"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|m| m["kind"] == "assistant" && m["text"] == "Noted.");
        if noted && !server.host.runtime.is_running(id) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("{id} did not answer");
}

/// Task 10.1: a message from laptop's `review` to `migration@desk` is held on desk. desk's
/// TUI shows the held card "from another machine" with the sender named after the grant
/// (`review@laptop`), its `/messages` view names the machine in its peer column, and once
/// the user releases it the transcript shows `from review@laptop`. laptop's TUI shows the
/// sent message under machine `desk`, released after its outcome tracker polls desk, and
/// its sidebar has a Peers line (`desk ●  nas ○`) that a machine without remotes lacks.
#[tokio::test]
async fn messages_between_machines_name_the_machine() {
    use cyber_server::runtime::peers::{NewGrant, grants};
    let desk = machine(json!({ "peers": { "machine": "desk" } })).await;
    let (url, fingerprint) = listen(&desk).await;
    let grant = NewGrant {
        name: "laptop".into(),
        scopes: vec![],
        expires_ms: None,
    };
    let token = grants::create(&desk.host.store, grant, now_ms())
        .unwrap()
        .token;
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
    let migration = session(&desk, "migration").await;
    let review = session(&laptop, "review").await;
    let ids = [
        (migration.as_str(), "MIGRATION"),
        (review.as_str(), "REVIEW"),
    ];

    let body =
        json!({ "id": "msgx_nightly", "from": review, "to": "migration@desk", "text": NIGHTLY });
    let sent = laptop.client.post("/messaging/send", body).await.unwrap();
    assert_eq!(sent["data"]["outcome"], "held", "{sent}");

    // desk: the held card names the other machine and the sender after the grant.
    let mut app = desk.load(&migration).await;
    let record = app
        .messaging
        .received("msgx_nightly")
        .expect("desk records it");
    assert_eq!(record.machine(), Some("laptop"));
    let shown = screen(&mut app, 100);
    for needle in [
        "Held message from another machine",
        "✉ from review@laptop",
        "The nightly build on desk is red",
        "✉ 1 held",
    ] {
        assert!(shown.contains(needle), "{needle:?}:\n{shown}");
    }
    assert!(!shown.contains("Peers"), "desk has no remotes:\n{shown}");
    insta::assert_snapshot!("peers_held_card_100x30", shot(&mut app, 100, &ids));

    // desk's `/messages`: the peer column names laptop.
    let actions = app.open_messages();
    drive(&desk, &mut app, actions).await;
    press(&mut app, KeyCode::Enter);
    let view = screen(&mut app, 100);
    let row = view
        .lines()
        .find(|l| l.contains("review@laptop"))
        .expect(&view);
    assert!(row.contains(" laptop ") && row.contains("held"), "{row}");
    assert!(view.contains(&format!("on laptop {review}")), "{view}");
    insta::assert_snapshot!("peers_messages_view_100x30", shot(&mut app, 100, &ids));
    press(&mut app, KeyCode::Esc);

    // desk's user releases it from the card; the transcript names the machine.
    assert!(press(&mut app, KeyCode::Tab).is_empty());
    let actions = key(&mut app, 'r');
    assert_eq!(actions[0], Action::ReleaseHeld("msgx_nightly".into()));
    drive(&desk, &mut app, actions).await;
    answered(&desk, &migration).await;
    drive(&desk, &mut app, vec![Action::Refresh]).await;
    let shown = screen(&mut app, 100);
    assert!(
        shown.contains("✉ from review@laptop · ses_") && shown.contains("untrusted"),
        "{shown}"
    );
    assert!(!shown.contains("Held message"), "{shown}");

    // laptop: its tracker learns the release; the sent row is on machine desk.
    let tracked = laptop
        .host
        .runtime
        .messaging()
        .track_outcomes(now_ms() + 60_000)
        .await
        .unwrap();
    assert_eq!(tracked.changed, 1, "{tracked:?}");
    let mut app = laptop.load(&review).await;
    let record = app
        .messaging
        .sent("msgx_nightly")
        .expect("laptop records it");
    assert_eq!(
        (record.machine(), record.outcome.as_str()),
        (Some("desk"), "released")
    );
    let names: Vec<(String, bool)> = app
        .sidebar
        .peers
        .ready()
        .expect("GET /peers/remotes answered")
        .iter()
        .map(|p| (p.name.clone(), p.online))
        .collect();
    assert_eq!(
        names,
        [("desk".to_owned(), true), ("nas".to_owned(), false)]
    );
    let docked = screen(&mut app, 140);
    assert!(docked.contains("Peers  desk ●  nas ○"), "{docked}");
    assert!(docked.contains("MCP"), "the MCP section stays:\n{docked}");
    insta::assert_snapshot!("peers_sender_sidebar_140x30", shot(&mut app, 140, &ids));
    let actions = app.open_messages();
    drive(&laptop, &mut app, actions).await;
    let view = screen(&mut app, 140);
    let row = view
        .lines()
        .find(|l| l.contains("migration@desk"))
        .expect(&view);
    assert!(row.contains(" desk ") && row.contains("released"), "{row}");
}
