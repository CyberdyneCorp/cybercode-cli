//! Goals in the TUI (`tui` → Goal and loop indicators, `goals` → Goal management
//! commands): the `/goal` panel with its keys, the footer and the sidebar Goal section,
//! drawn by the real `view::draw` from a production App over HTTP, with the Session's
//! `GET .../goals` view as the runner reads it.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use serde_json::{Value, json};

use crate::app::{Action, App, Overlay};
use crate::goals::GoalRequest;
use crate::screens::{Opts, Server, render};

/// A current Goal on turn 3 of 50 with a not-met verdict, and two queued Goals.
fn view() -> Value {
    json!({
        "current": {"goal_id": "gol_01A", "condition": "all call sites compile against the v2 API",
            "status": "active", "turns": 3, "budget": {"max_turns": 50}, "cost_usd": 0.42,
            "eval_count": 3, "eval_tokens": 2400, "eval_cost_usd": 0.01,
            "last_verdict": "not_met", "reason": "2 call sites still use v1"},
        "queue": [
            {"goal_id": "gol_01B", "condition": "cargo test passes", "status": "pending",
                "turns": 0, "budget": {"max_turns": 50}, "cost_usd": 0.0},
            {"goal_id": "gol_01C", "condition": "the changelog lists the API change",
                "status": "pending", "turns": 0, "budget": {"max_turns": 50}, "cost_usd": 0.0}
        ],
        "ended": [
            {"goal_id": "gol_009", "condition": "the parser is split into modules",
                "status": "met", "turns": 5, "budget": {"max_turns": 50}, "cost_usd": 0.9,
                "eval_count": 5, "eval_tokens": 3900, "eval_cost_usd": 0.02,
                "reason": "modules exist and tests pass"}
        ]
    })
}

async fn goal_app() -> App {
    let server = Server::start().await;
    let id = server.answered_session().await;
    let mut app = server.app(&id).await;
    app.goals.replace(Some(view()));
    app
}

fn panel(app: &mut App) {
    let session_id = app.session.id.clone();
    app.goals_listed(&session_id, crate::goals::choices(&view()));
}

fn press(app: &mut App, code: KeyCode, modifiers: KeyModifiers) -> Vec<Action> {
    app.on_key(KeyEvent::new(code, modifiers))
}

#[tokio::test]
async fn the_goal_panel_lists_the_goal_its_queue_and_keys() {
    let mut app = goal_app().await;
    panel(&mut app);
    let screen = render(&mut app, 120, 40, &Opts::default());
    insta::assert_snapshot!("goal_panel_120x40", screen);

    // Shift+↓ on the first queued Goal moves it below the second.
    press(&mut app, KeyCode::Down, KeyModifiers::NONE);
    let actions = press(&mut app, KeyCode::Down, KeyModifiers::SHIFT);
    let session_id = app.session.id.clone();
    assert_eq!(
        actions,
        [Action::Goal(GoalRequest::Reorder {
            session_id: session_id.clone(),
            goal_ids: vec!["gol_01C".into(), "gol_01B".into()],
        })]
    );
    panel(&mut app);
    assert_eq!(
        press(&mut app, KeyCode::Char('p'), KeyModifiers::NONE),
        [Action::Goal(GoalRequest::Pause {
            session_id: session_id.clone()
        })]
    );
    assert!(matches!(app.overlay, Overlay::None));
    panel(&mut app);
    press(&mut app, KeyCode::Down, KeyModifiers::NONE);
    assert_eq!(
        press(&mut app, KeyCode::Char('c'), KeyModifiers::NONE),
        [Action::Goal(GoalRequest::Clear {
            session_id: session_id.clone(),
            goal_id: Some("gol_01B".into()),
        })]
    );
    panel(&mut app);
    assert_eq!(
        press(&mut app, KeyCode::Char('e'), KeyModifiers::NONE),
        [Action::Goal(GoalRequest::Edit {
            session_id: session_id.clone(),
            goal_id: Some("gol_01A".into()),
        })]
    );
    panel(&mut app);
    // The current Goal is not queued, so it cannot move.
    assert!(press(&mut app, KeyCode::Up, KeyModifiers::SHIFT).is_empty());
    assert_eq!(
        press(&mut app, KeyCode::Char('r'), KeyModifiers::NONE),
        [Action::Goal(GoalRequest::Resume { session_id })]
    );
}

#[tokio::test]
async fn the_footer_and_sidebar_show_the_current_goal() {
    let mut app = goal_app().await;
    let screen = render(&mut app, 160, 40, &Opts::default());
    assert!(screen.contains("◎ goal all call sites compile"), "{screen}");
    insta::assert_snapshot!("goal_sidebar_160x40", screen);
    let light = Opts {
        light: true,
        legend: &["◎"],
        ..Opts::default()
    };
    let screen = render(&mut app, 160, 40, &light);
    insta::assert_snapshot!("goal_sidebar_light_160x40", screen);
}
