//! Ctrl+R prompt history search (`tui` → Prompt history search) against a production
//! App: the composer shows the selected entry with the query marked, Enter leaves it
//! for editing without submitting, and Esc restores the draft.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::app::{Action, App};
use crate::screens::{Opts, Server, render};

fn press(app: &mut App, code: KeyCode, modifiers: KeyModifiers) -> Vec<Action> {
    app.on_key(KeyEvent::new(code, modifiers))
}

fn typed(app: &mut App, text: &str) {
    for c in text.chars() {
        press(app, KeyCode::Char(c), KeyModifiers::NONE);
    }
}

fn ctrl(app: &mut App, c: char) -> Vec<Action> {
    press(app, KeyCode::Char(c), KeyModifiers::CONTROL)
}

fn with_history(app: &mut App, entries: &[&str]) {
    app.composer =
        crate::composer::Composer::new(entries.iter().map(|e| (*e).to_string()).collect());
}

#[tokio::test]
async fn search_finds_marks_and_recalls_an_older_prompt() {
    let server = Server::start().await;
    let id = server.answered_session().await;
    let mut app = server.app(&id).await;
    with_history(
        &mut app,
        &["run the tests", "fix the login bug", "run the linter"],
    );
    typed(&mut app, "half-written idea");

    ctrl(&mut app, 'r');
    typed(&mut app, "run");
    for (name, opts) in [
        ("dark", Opts::default()),
        (
            "no_color",
            Opts {
                no_color: true,
                ..Opts::default()
            },
        ),
    ] {
        let screen = render(
            &mut app,
            100,
            30,
            &Opts {
                legend: &["┃ run the linter"],
                ..opts
            },
        );
        assert!(screen.contains("search: run  1/2"), "{screen}");
        insta::assert_snapshot!(format!("history_search_{name}"), screen);
    }

    ctrl(&mut app, 'r');
    let actions = press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
    assert!(actions.is_empty(), "accepting never submits: {actions:?}");
    assert!(app.history_search.is_none());
    assert_eq!(app.composer.text(), "run the tests");
    assert_eq!(app.composer.cursor(), (0, 13), "the cursor is at the end");
}

#[tokio::test]
async fn cancel_and_no_match_restore_the_draft() {
    let server = Server::start().await;
    let id = server.answered_session().await;
    let mut app = server.app(&id).await;
    with_history(&mut app, &["run the linter"]);
    typed(&mut app, "half-written idea");

    ctrl(&mut app, 'r');
    typed(&mut app, "lint");
    press(&mut app, KeyCode::Esc, KeyModifiers::NONE);
    assert_eq!(app.composer.text(), "half-written idea");

    ctrl(&mut app, 'r');
    typed(&mut app, "zzz");
    let screen = render(&mut app, 100, 30, &Opts::default());
    assert!(screen.contains("search: zzz  no match"), "{screen}");
    assert!(screen.contains("┃ half-written idea"), "{screen}");
    press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
    assert_eq!(app.composer.text(), "half-written idea");

    ctrl(&mut app, 'r');
    ctrl(&mut app, 'g');
    assert!(app.history_search.is_none(), "ctrl+g cancels");
    assert_eq!(app.composer.text(), "half-written idea");
}

#[tokio::test]
async fn a_disabled_search_binding_leaves_ctrl_r_inert() {
    let server = Server::start().await;
    let id = server.answered_session().await;
    let mut app = server.app(&id).await;
    let mut warnings = Vec::new();
    app.keymap = crate::keys::Keymap::load(
        Some(&serde_json::json!({ "composer": { "history_search": "none" } })),
        &mut warnings,
    );
    assert!(warnings.is_empty(), "{warnings:?}");
    with_history(&mut app, &["run the linter"]);
    ctrl(&mut app, 'r');
    assert!(app.history_search.is_none());
    assert_eq!(app.composer.text(), "");
}
