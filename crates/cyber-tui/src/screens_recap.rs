//! The Session recap line (`tui` → Session recap line), drawn by the real `view::draw`
//! from recaps a production App generated with a local scripted model, requested and
//! applied through the runner's own `perform` and `apply`.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::style::Modifier;

use crate::app::{Action, App, Overlay};
use crate::screens::{Opts, RECAP, Server, buffer, render, row_of};

const HOUR: i64 = 3_600_000;

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
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

/// The Session's messages read again, as when it is opened, `hours` after its last run.
async fn reopen(server: &Server, app: &mut App, hours: i64) -> Vec<Action> {
    app.recap.opened = None;
    app.recap.clock_ms = Some(now_ms() + hours * HOUR);
    let msg = crate::perform::snapshot(&server.client, &app.session.id).await;
    crate::runner::apply(app, msg, &mut Default::default())
        .into_iter()
        .filter(|a| {
            !matches!(
                a,
                Action::LoadSidebar(_) | Action::Workflow(crate::workflows::Request::Watch { .. })
            )
        })
        .collect()
}

async fn run(server: &Server, app: &mut App, actions: Vec<Action>) {
    for action in actions {
        let msg = crate::perform::perform_owned(&server.client, &app.session, action, None).await;
        crate::runner::apply(app, msg, &mut Default::default());
    }
}

fn recap_calls(server: &Server) -> usize {
    server
        .model
        .requests()
        .iter()
        .filter(|r| r.to_string().contains("summarize a coding session"))
        .count()
}

#[tokio::test]
async fn recap_on_return_wraps_on_top_and_hides_on_the_next_prompt() {
    let server = Server::start().await;
    let id = server.answered_session().await;
    // Just answered: nothing is asked for.
    let mut app = server.app(&id).await;
    assert!(reopen(&server, &mut app, 0).await.is_empty());
    // Three hours later the Session is opened again.
    let actions = reopen(&server, &mut app, 3).await;
    assert_eq!(
        actions,
        [Action::Recap {
            session_id: id.clone(),
            manual: false
        }]
    );
    run(&server, &mut app, actions).await;
    assert_eq!(recap_calls(&server), 1);
    assert_eq!(app.recap.shown.as_ref().unwrap().text, RECAP);
    crate::screens::stabilize(&mut app);

    let opts = Opts {
        legend: &["※ recap:", "────"],
        ..Opts::default()
    };
    let buf = buffer(&mut app, 100, 30, &opts);
    let y = row_of(&buf, "※ recap:");
    assert_eq!(y, 1, "below the one-row header");
    let x = (0..100).find(|x| buf[(*x, y + 2)].symbol() == "─").unwrap();
    assert_eq!(buf[(x, y + 2)].fg, app.theme.border_active);
    let text_x = (0..100).find(|x| buf[(*x, y)].symbol() == "I").unwrap();
    assert_eq!(buf[(text_x, y)].fg, app.theme.text_muted);
    assert!(buf[(text_x, y)].modifier.contains(Modifier::ITALIC));
    // At 60 columns two rows are not enough: the second ends with `…`.
    let narrow = buffer(&mut app, 60, 20, &opts);
    let second: String = (0..60).map(|x| narrow[(x, y + 1)].symbol()).collect();
    assert!(second.trim_end().ends_with('…'), "{second}");
    assert!((1..59).all(|x| narrow[(x, y + 2)].symbol() == "─"));
    insta::assert_snapshot!("recap_100x30", render(&mut app, 100, 30, &opts));
    let plain = Opts {
        no_color: true,
        legend: &["※ recap:"],
        ..Opts::default()
    };
    insta::assert_snapshot!("recap_no_color_100x30", render(&mut app, 100, 30, &plain));

    // The Settings panel keeps it on top.
    typed(&mut app, "/status");
    assert!(matches!(app.overlay, Overlay::Settings));
    let buf = buffer(&mut app, 100, 30, &Opts::default());
    assert_eq!(row_of(&buf, "※ recap:"), 0);
    assert!(row_of(&buf, "Settings") > 2);
    app.on_key(key(KeyCode::Esc));

    // Reopening again is cached on the server: no second model call.
    let actions = reopen(&server, &mut app, 3).await;
    run(&server, &mut app, actions).await;
    assert_eq!(recap_calls(&server), 1);
    // Esc dismisses it; the next prompt hides it.
    assert!(app.on_key(key(KeyCode::Esc)).is_empty());
    assert!(app.recap.shown.is_none());
    let actions = typed(&mut app, "/recap");
    run(&server, &mut app, actions).await;
    assert!(app.recap.shown.is_some());
    let actions = typed(&mut app, "next step");
    assert!(matches!(&actions[..], [Action::Prompt { .. }]));
    assert!(app.recap.shown.is_none());
}

#[tokio::test]
async fn off_makes_no_request_but_slash_recap_still_asks() {
    let server = Server::start().await;
    let id = server.answered_session().await;
    let mut app = server.app(&id).await;
    app.prefs.set("recap", "off", crate::prefs::Origin::Saved);
    assert!(reopen(&server, &mut app, 3).await.is_empty());
    app.recap_focus_lost();
    app.recap.clock_ms = Some(now_ms() + 6 * HOUR);
    assert!(app.recap_focus_gained().is_empty());
    assert_eq!(recap_calls(&server), 0);
    assert!(app.recap.shown.is_none());

    let actions = typed(&mut app, "/recap");
    assert_eq!(
        actions,
        [Action::Recap {
            session_id: id,
            manual: true
        }]
    );
    run(&server, &mut app, actions).await;
    assert_eq!(recap_calls(&server), 1);
    assert!(app.recap.shown.is_some());

    // `/recap` before any finished Turn says so in a toast.
    let empty = server.session().await;
    let mut app = server.app(&empty).await;
    let actions = typed(&mut app, "/recap");
    run(&server, &mut app, actions).await;
    assert!(app.recap.shown.is_none());
    let toast = &app.toast.as_ref().expect("a toast").0;
    assert!(toast.contains("nothing to recap"), "{toast}");
}
