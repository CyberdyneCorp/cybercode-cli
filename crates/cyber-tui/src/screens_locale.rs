//! The interface in each supported language (`localization` → Interface translation)
//! against a production App: translated chrome, unchanged command names and keys.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use cyber_core::locale::Locale;
use serde_json::json;

use crate::app::{Action, App};
use crate::screens::{Opts, Server, render};

fn press(app: &mut App, code: KeyCode) -> Vec<Action> {
    app.on_key(KeyEvent::new(code, KeyModifiers::NONE))
}

fn permission(app: &mut App, session_id: &str) {
    app.requests = vec![crate::model::Request::Permission {
        id: "per_1".into(),
        session_id: session_id.into(),
        origin: None,
        member: None,
        routed_to: Vec::new(),
        action: "bash".into(),
        resources: vec!["cargo test".into()],
        patterns: vec!["cargo test".into()],
        metadata: json!({}),
    }];
    app.sync_overlay();
}

#[tokio::test]
async fn welcome_palette_and_permission_in_each_language() {
    let server = Server::start().await;
    let id = server.session().await;
    for (locale, allow) in [
        (Locale::PtBr, "Permitir uma vez (y)"),
        (Locale::Es, "Permitir una vez (y)"),
        (Locale::Fr, "Autoriser une fois (y)"),
    ] {
        let mut app = server.app(&id).await;
        app.locale = locale;
        // The default title carries the creation time.
        app.session.title = "New session".into();
        let tag = locale.tag();
        let welcome = render(&mut app, 100, 30, &Opts::default());
        assert!(!welcome.contains("Ask anything"), "{tag}: {welcome}");
        assert!(
            welcome
                .lines()
                .all(|l| !l.contains("~/work/app") || l.contains(" ~/work/app")),
            "{tag}: every label is followed by a space: {welcome}"
        );
        insta::assert_snapshot!(format!("locale_welcome_{tag}"), welcome);

        app.open_palette();
        let palette = render(&mut app, 100, 30, &Opts::default());
        assert!(palette.contains("/model"), "{tag}: command names stay");
        insta::assert_snapshot!(format!("locale_palette_{tag}"), palette);
        press(&mut app, KeyCode::Esc);

        permission(&mut app, &id);
        let prompt = render(&mut app, 100, 30, &Opts::default());
        assert!(prompt.contains(allow), "{tag}: {prompt}");
        insta::assert_snapshot!(format!("locale_permission_{tag}"), prompt);
        let actions = press(&mut app, KeyCode::Char('y'));
        assert!(
            matches!(actions.as_slice(), [Action::Reply { .. }]),
            "{tag}: y still allows once: {actions:?}"
        );
    }
}

#[tokio::test]
async fn english_is_unchanged_and_palette_search_finds_commands_by_name() {
    let server = Server::start().await;
    let id = server.session().await;
    let mut app = server.app(&id).await;
    let english = render(&mut app, 100, 30, &Opts::default());
    assert!(english.contains("Ask anything"), "{english}");

    app.locale = Locale::Es;
    let entries = crate::palette::entries(&app.keymap, &[], app.locale);
    let model = entries.iter().find(|e| e.name == "/model").unwrap();
    assert_eq!(model.description, "cambiar modelo");
    let settings = entries.iter().find(|e| e.name == "Ajustes");
    assert!(settings.is_some(), "UI actions are translated");
}

#[tokio::test]
async fn the_history_search_line_is_translated() {
    let server = Server::start().await;
    let id = server.session().await;
    let mut app = server.app(&id).await;
    app.locale = Locale::PtBr;
    app.composer = crate::composer::Composer::new(vec!["run the linter".into()]);
    app.on_key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL));
    for c in "zzz".chars() {
        press(&mut app, KeyCode::Char(c));
    }
    let screen = render(&mut app, 100, 30, &Opts::default());
    assert!(screen.contains("busca: zzz  nenhum resultado"), "{screen}");
    assert!(screen.contains("ctrl+r anterior"), "{screen}");
}
