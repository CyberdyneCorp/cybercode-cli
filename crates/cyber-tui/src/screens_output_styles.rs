//! `/output-style` (`tui` → Output style picker) against a production App: the picker
//! lists the Location's styles through the runner's own `perform` and `apply`, Enter
//! switches the Session from its next Turn, `d` saves the default, Esc changes nothing,
//! and a held project directory is explained.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::app::{Action, App, Overlay};
use crate::output_styles::{Opener, Request, SUBAGENT};
use crate::screens::{Opts, Server, render};

fn press(app: &mut App, code: KeyCode) -> Vec<Action> {
    app.on_key(KeyEvent::new(code, KeyModifiers::NONE))
}

fn typed(app: &mut App, text: &str) -> Vec<Action> {
    for c in text.chars() {
        press(app, KeyCode::Char(c));
    }
    press(app, KeyCode::Enter)
}

/// Run `actions` and what follows them through `perform` and the runner's `apply`.
async fn run(server: &Server, app: &mut App, mut actions: Vec<Action>) {
    while let Some(action) = actions.pop() {
        assert!(matches!(action, Action::OutputStyle(_)), "{action:?}");
        let msg = crate::perform::perform(&server.client, &app.session, action).await;
        actions.extend(crate::runner::apply(app, msg, &mut Default::default()));
    }
}

fn toast(app: &App) -> String {
    app.toast
        .as_ref()
        .map(|(text, _)| text.clone())
        .unwrap_or_default()
}

/// The system text of the model's last request.
fn last_system(server: &Server) -> String {
    let requests = server.model.requests();
    let turn = requests
        .iter()
        .rev()
        .find(|body| body["tools"].is_array())
        .expect("a Turn called the model");
    turn["messages"][0]["content"].to_string()
}

#[tokio::test]
async fn picker_switches_the_session_and_shows_next_turn() {
    let server = Server::start().await;
    let id = server.answered_session().await;
    let mut app = server.load(&id).await;

    let actions = typed(&mut app, "/output-style");
    assert!(
        matches!(
            actions.as_slice(),
            [Action::OutputStyle(Request::Load {
                open: Some(Opener::Command),
                ..
            })]
        ),
        "{actions:?}"
    );
    run(&server, &mut app, actions).await;
    assert!(matches!(app.overlay, Overlay::OutputStyles(_)));
    let shown = render(&mut app, 120, 40, &Opts::default());
    for text in ["Output style", "concise", "explanatory", "learning"] {
        assert!(shown.contains(text), "{text}: {shown}");
    }
    assert!(shown.contains("built-in · current · default"), "{shown}");

    // Esc has no side effects.
    assert!(press(&mut app, KeyCode::Esc).is_empty());
    assert!(matches!(app.overlay, Overlay::None));
    assert_eq!(app.session.output_style, None);

    // The fuzzy filter, then Enter switches the Session.
    let actions = typed(&mut app, "/output-style");
    run(&server, &mut app, actions).await;
    let actions = typed(&mut app, "expl");
    assert!(
        matches!(
            actions.as_slice(),
            [Action::OutputStyle(Request::Switch { style, save: false, .. })] if style == "explanatory"
        ),
        "{actions:?}"
    );
    run(&server, &mut app, actions).await;
    assert_eq!(app.session.output_style.as_deref(), Some("explanatory"));
    let shown = render(&mut app, 120, 40, &Opts::default());
    assert!(
        shown.contains("Output style: explanatory (next turn)"),
        "{shown}"
    );
    let session = server.client.get(&format!("/sessions/{id}")).await.unwrap();
    assert_eq!(session["data"]["output_style"], "explanatory");

    // The next Turn's system prompt carries the style.
    server.prompt(&id, "And now?").await;
    server.wait_answered(&id).await;
    assert!(
        last_system(&server).contains("Output style: explanatory"),
        "{}",
        last_system(&server)
    );

    // `d` saves the highlighted style as the default and switches the Session.
    let actions = typed(&mut app, "/output-style");
    run(&server, &mut app, actions).await;
    press(&mut app, KeyCode::Up);
    let actions = press(&mut app, KeyCode::Char('d'));
    assert!(
        matches!(
            actions.as_slice(),
            [Action::OutputStyle(Request::Switch { style, save: true, .. })] if style == "concise"
        ),
        "{actions:?}"
    );
    run(&server, &mut app, actions).await;
    assert_eq!(app.session.output_style.as_deref(), Some("concise"));
    assert!(
        toast(&app).contains("Saved concise as the default"),
        "{}",
        toast(&app)
    );
    let listing = app.output_styles.listing.as_ref().unwrap();
    assert_eq!(listing.effective, "concise");
    let saved = std::fs::read_to_string(server.root().join("config/cyber.json")).unwrap();
    assert!(saved.contains("\"output_style\""), "{saved}");

    // An unknown name shows the server's reason; a subagent thread refuses the command.
    let actions = typed(&mut app, "/output-style nope");
    run(&server, &mut app, actions).await;
    assert!(
        toast(&app).contains("unknown output style"),
        "{}",
        toast(&app)
    );
    assert_eq!(app.session.output_style.as_deref(), Some("concise"));
    app.session.parent_id = Some("ses_parent".into());
    assert!(typed(&mut app, "/output-style").is_empty());
    assert_eq!(toast(&app), SUBAGENT);
}

/// The text inside the picker's border with its wrapped lines joined: where the held
/// notice wraps depends on the length of the temporary checkout path.
fn panel_text(shown: &str) -> String {
    let rows = shown.lines().filter_map(|line| {
        let (start, end) = (line.find('│')?, line.rfind('│')?);
        (start < end).then(|| line[start + '│'.len_utf8()..end].trim())
    });
    rows.collect::<Vec<_>>().join(" ")
}

#[tokio::test]
async fn held_project_styles_are_explained() {
    let server = Server::start().await;
    let root = server.root();
    // An untrusted checkout with a project style, which the configuration selects.
    std::fs::create_dir_all(root.join(".git")).unwrap();
    let styles = root.join(".cyber/output-styles");
    std::fs::create_dir_all(&styles).unwrap();
    std::fs::write(styles.join("reviewer.md"), "Review every change carefully.").unwrap();
    let path = root.join("config/cyber.json");
    let mut config: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    config["output_style"] = "reviewer".into();
    std::fs::write(&path, config.to_string()).unwrap();
    let id = server.session().await;
    let mut app = server.load(&id).await;

    let actions = typed(&mut app, "/output-style");
    run(&server, &mut app, actions).await;
    let shown = render(&mut app, 120, 40, &Opts::default());
    assert!(
        panel_text(&shown)
            .contains("project output styles are held until this checkout is trusted"),
        "{shown}"
    );
    assert!(!shown.contains("reviewer  "), "{shown}");
    // The configured style is unavailable: said once per Session.
    let notice = toast(&app);
    assert!(
        notice.contains("is held until this checkout is trusted"),
        "{notice}"
    );
    assert!(
        notice.contains("the default output style applies"),
        "{notice}"
    );
    app.toast = None;
    press(&mut app, KeyCode::Esc);
    let actions = app.output_style_check();
    run(&server, &mut app, actions).await;
    assert!(app.toast.is_none(), "{}", toast(&app));
}

/// A Session in another checkout than the TUI's (a worktree, or one opened with
/// `/resume`) lists its own Location's styles: a trusted project style that exists only
/// there is offered, and picking it switches the Session.
#[tokio::test]
async fn the_picker_lists_the_sessions_location() {
    let server = Server::start().await;
    let root = server.root();
    let work = root.join("work");
    std::fs::create_dir_all(work.join(".git")).unwrap();
    let styles = work.join(".cyber/output-styles");
    std::fs::create_dir_all(&styles).unwrap();
    std::fs::write(styles.join("reviewer.md"), "Review every change carefully.").unwrap();
    trust(&root, &work);
    let created = server
        .client
        .at(&work.display().to_string())
        .post(
            "/sessions",
            serde_json::json!({ "model": "test/main", "mode": "default" }),
        )
        .await
        .unwrap();
    let id = created["data"]["id"].as_str().unwrap().to_owned();
    let mut app = server.load(&id).await;
    assert_eq!(app.session.directory, work.display().to_string());

    let actions = typed(&mut app, "/output-style");
    run(&server, &mut app, actions).await;
    let shown = render(&mut app, 120, 40, &Opts::default());
    assert!(
        shown.contains("reviewer") && shown.contains("project"),
        "{shown}"
    );
    let actions = typed(&mut app, "reviewer");
    assert!(
        matches!(
            actions.as_slice(),
            [Action::OutputStyle(Request::Switch { style, .. })] if style == "reviewer"
        ),
        "{actions:?}"
    );
    run(&server, &mut app, actions).await;
    assert_eq!(app.session.output_style.as_deref(), Some("reviewer"));
    let session = server.client.get(&format!("/sessions/{id}")).await.unwrap();
    assert_eq!(session["data"]["output_style"], "reviewer");
}

/// Approve the checkout at `work` in the screen server's trust store.
fn trust(root: &std::path::Path, work: &std::path::Path) {
    let paths = cyber_core::paths::Paths {
        data: root.join("data"),
        config: root.join("config"),
        state: root.join("state"),
        cache: root.join("cache"),
        tmp: root.join("tmp"),
    };
    let report = cyber_core::config::trust_report(&cyber_core::config::LoadRequest {
        location: work,
        paths: &paths,
        home: &root.join("home"),
        env: &cyber_core::env::ProcessEnv,
        profile: None,
        overrides: &[],
        flags: Default::default(),
    })
    .unwrap();
    cyber_core::trust::TrustStore::new(paths.trust_file())
        .approve(&report.checkout_root, report.digest.as_deref().unwrap())
        .unwrap();
}
