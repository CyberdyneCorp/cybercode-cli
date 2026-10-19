//! Behavior of the state machine and what the screen shows, on an in-memory terminal.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use serde_json::json;

use crate::app::{Action, App, Overlay, PermStep};
use crate::model::{Item, QuestionSpec, Queued, Request, Session, Tool};

pub(crate) fn session(running: bool) -> Session {
    Session {
        id: "ses_1".into(),
        title: "Fix tests".into(),
        directory: "/repo".into(),
        model: "openai/m".into(),
        agent: "build".into(),
        mode: "default".into(),
        running,
        ..Session::default()
    }
}

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn with(code: KeyCode, m: KeyModifiers) -> KeyEvent {
    KeyEvent::new(code, m)
}

fn typed(app: &mut App, text: &str) -> Vec<Action> {
    text.chars()
        .flat_map(|c| app.on_key(key(KeyCode::Char(c))))
        .collect()
}

fn screen(app: &App) -> String {
    let mut t = Terminal::new(TestBackend::new(100, 30)).unwrap();
    t.draw(|f| crate::view::draw(f, app)).unwrap();
    let buf = t.backend().buffer().clone();
    (0..buf.area.height)
        .map(|y| {
            (0..buf.area.width)
                .map(|x| buf[(x, y)].symbol().to_string())
                .collect::<String>()
                .trim_end()
                .to_string()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn enter_steers_while_running_and_tab_queues() {
    let mut idle = App::new(session(false), Vec::new(), "cyber");
    typed(&mut idle, "hello");
    assert_eq!(
        idle.on_key(key(KeyCode::Enter)),
        vec![Action::Prompt {
            text: "hello".into(),
            delivery: "steer"
        }]
    );
    let mut busy = App::new(session(true), Vec::new(), "cyber");
    typed(&mut busy, "also this");
    assert_eq!(
        busy.on_key(key(KeyCode::Tab)),
        vec![Action::Prompt {
            text: "also this".into(),
            delivery: "queue"
        }]
    );
    assert_eq!(busy.on_key(key(KeyCode::Esc)), vec![Action::Interrupt]);
}

#[test]
fn shift_enter_inserts_a_newline_and_shell_and_slash_inputs_route() {
    let mut app = App::new(session(false), Vec::new(), "cyber");
    typed(&mut app, "a");
    app.on_key(with(KeyCode::Enter, KeyModifiers::SHIFT));
    typed(&mut app, "b");
    assert_eq!(app.composer.text(), "a\nb");
    app.composer.clear();
    typed(&mut app, "!cargo test");
    assert_eq!(
        app.on_key(key(KeyCode::Enter)),
        vec![Action::Shell("cargo test".into())]
    );
    typed(&mut app, "/release-notes v1.2");
    app.completion = None;
    assert_eq!(
        app.on_key(key(KeyCode::Enter)),
        vec![Action::Command {
            name: "release-notes".into(),
            arguments: "v1.2".into(),
            id: None,
        }]
    );
    typed(&mut app, "/mode plan");
    app.completion = None;
    assert_eq!(
        app.on_key(key(KeyCode::Enter)),
        vec![Action::SwitchMode("plan".into())]
    );
}

#[test]
fn at_mentions_ask_for_files_and_complete() {
    let mut app = App::new(session(false), Vec::new(), "cyber");
    let actions = typed(&mut app, "see @ma");
    assert!(actions.contains(&Action::FindFiles("ma".into())));
    app.files = vec!["src/main.rs".into(), "README.md".into()];
    typed(&mut app, "i");
    let c = app.completion.as_ref().unwrap();
    assert_eq!(c.items[0].label, "src/main.rs");
    app.on_key(key(KeyCode::Tab));
    assert_eq!(app.composer.text(), "see @src/main.rs ");
}

#[test]
fn enter_runs_a_fully_typed_command_and_completes_a_partial_one() {
    let mut app = App::new(session(false), Vec::new(), "cyber");
    app.commands = ["rename", "release-notes"]
        .map(|name| crate::model::Choice {
            key: name.into(),
            label: name.into(),
            detail: String::new(),
        })
        .to_vec();
    typed(&mut app, "/rena");
    assert!(app.completion.is_some());
    assert!(app.on_key(key(KeyCode::Enter)).is_empty());
    assert_eq!(app.composer.text(), "/rename ");
    assert!(matches!(app.overlay, Overlay::None));

    app.composer.clear();
    typed(&mut app, "/rename");
    assert_eq!(app.completion.as_ref().unwrap().items[0].key, "rename");
    app.on_key(key(KeyCode::Enter));
    assert!(app.composer.is_empty());
    assert!(matches!(app.overlay, Overlay::Rename(_)));
}

#[test]
fn the_footer_shows_the_location_with_home_as_tilde() {
    let mut app = App::new(session(false), Vec::new(), "cyber");
    app.session.directory = "/Users/dev/work/app".into();
    app.home = Some("/Users/dev".into());
    let footer = screen(&app).lines().last().unwrap_or_default().to_owned();
    assert!(footer.starts_with(" ~/work/app"), "{footer}");
}

fn permission() -> Request {
    Request::Permission {
        id: "per_1".into(),
        session_id: "ses_1".into(),
        origin: None,
        routed_to: Vec::new(),
        action: "bash".into(),
        resources: vec!["rm -rf build".into()],
        patterns: vec!["rm *".into()],
        metadata: json!({}),
    }
}

#[test]
fn permission_prompt_confirms_always_and_collects_feedback() {
    let mut app = App::new(session(true), Vec::new(), "cyber");
    app.requests = vec![permission()];
    app.sync_overlay();
    assert_eq!(app.overlay, Overlay::Permission(PermStep::Choose(0)));
    assert!(screen(&app).contains("Permission: bash"));
    app.on_key(key(KeyCode::Char('a')));
    assert!(
        screen(&app).contains("bash rm *"),
        "patterns are listed before saving"
    );
    let actions = app.on_key(key(KeyCode::Enter));
    assert_eq!(
        actions,
        vec![Action::Reply {
            request: "per_1".into(),
            body: json!({ "reply": "always" })
        }]
    );
    assert_eq!(app.overlay, Overlay::None);

    app.requests = vec![permission()];
    app.sync_overlay();
    app.on_key(key(KeyCode::Char('n')));
    typed(&mut app, "use make clean");
    let actions = app.on_key(key(KeyCode::Enter));
    assert_eq!(
        actions,
        vec![Action::Reply {
            request: "per_1".into(),
            body: json!({ "reply": "reject", "message": "use make clean" })
        }]
    );
}

#[test]
fn questions_submit_on_selection_or_dismiss_with_esc() {
    let q = QuestionSpec {
        question: "Database?".into(),
        header: "DB".into(),
        options: vec![
            ("postgres".into(), String::new()),
            ("sqlite".into(), String::new()),
        ],
        multi: false,
        custom: true,
    };
    let req = Request::Question {
        id: "que_1".into(),
        session_id: "ses_1".into(),
        origin: None,
        routed_to: Vec::new(),
        questions: vec![q],
    };
    let mut app = App::new(session(true), Vec::new(), "cyber");
    app.requests = vec![req.clone()];
    app.sync_overlay();
    assert!(screen(&app).contains("Type your own answer"));
    app.on_key(key(KeyCode::Down));
    let actions = app.on_key(key(KeyCode::Enter));
    assert_eq!(
        actions,
        vec![Action::Answer {
            request: "que_1".into(),
            body: json!({ "answers": [["sqlite"]] })
        }]
    );
    app.requests = vec![req];
    app.sync_overlay();
    assert_eq!(
        app.on_key(key(KeyCode::Esc)),
        vec![Action::Answer {
            request: "que_1".into(),
            body: json!({})
        }]
    );
}

#[test]
fn messages_render_text_tools_and_collapsed_output() {
    let mut app = App::new(session(false), Vec::new(), "cyber");
    let output = (1..=40)
        .map(|i| format!("line {i}"))
        .collect::<Vec<_>>()
        .join("\n");
    app.items = vec![
        Item::User {
            id: "m1".into(),
            text: "run the tests".into(),
            promoted_ms: None,
        },
        Item::Assistant {
            id: "m2".into(),
            text: "All **good**: `cargo test` passed.".into(),
            reasoning: "think".into(),
            tools: vec![Tool {
                call_id: "c1".into(),
                name: "bash".into(),
                input: json!({"command": "cargo test"}),
                status: "ok".into(),
                output,
            }],
            error: None,
            step: Default::default(),
        },
    ];
    let s = screen(&app);
    assert!(s.contains("› run the tests"), "{s}");
    assert!(s.contains("⏺ bash cargo test  completed"), "{s}");
    assert!(s.contains("30 lines hidden"), "{s}");
    assert!(s.contains("line 40") && !s.contains("line 30\n"), "{s}");
    assert!(s.contains("thinking (Ctrl+T to show)"));
    assert!(s.contains("All good: cargo test passed."));
    app.on_key(with(KeyCode::Char('o'), KeyModifiers::CONTROL));
    assert!(!screen(&app).contains("lines hidden"));
}

#[test]
fn streaming_text_shows_until_the_message_is_durable() {
    let mut app = App::new(session(true), Vec::new(), "cyber");
    app.on_event(
        "session.text.delta",
        &json!({ "session_id": "ses_1", "message_id": "m9", "text": "Hel" }),
    );
    app.on_event(
        "session.text.delta",
        &json!({ "session_id": "ses_1", "message_id": "m9", "text": "lo" }),
    );
    assert!(screen(&app).contains("Hello"));
    assert_eq!(
        app.on_event("session.step.ended.1", &json!({})),
        vec![Action::Refresh]
    );
    assert_eq!(
        app.on_event("session.idle", &json!({})),
        vec![
            Action::Refresh,
            Action::LoadSidebar(crate::sidebar::Part::Lsp),
            Action::LoadSidebar(crate::sidebar::Part::Vcs),
        ]
    );
    assert!(!app.session.running);
}

/// A refresh during a step brings the in-progress assistant message with no text yet;
/// its streamed text must show once, inside that message, not again after it.
#[test]
fn streaming_text_shows_once_when_its_message_is_listed_without_text() {
    let mut app = App::new(session(true), Vec::new(), "cyber");
    app.items = vec![Item::Assistant {
        id: "m9".into(),
        text: String::new(),
        reasoning: String::new(),
        tools: Vec::new(),
        error: None,
        step: Default::default(),
    }];
    app.on_event(
        "session.text.delta",
        &json!({ "session_id": "ses_1", "message_id": "m9", "text": "Streamed once" }),
    );
    let s = screen(&app);
    assert_eq!(s.matches("Streamed once").count(), 1, "{s}");
    app.streaming
        .insert("m9".into(), "```\nonly block\n```\n".into());
    assert_eq!(app.code_blocks(), vec!["only block".to_string()]);
}

#[test]
fn queued_messages_are_listed_and_can_be_taken_back() {
    let mut app = App::new(session(true), Vec::new(), "cyber");
    app.queued = vec![Queued {
        message_id: "m5".into(),
        text: "then update docs".into(),
        delivery: "queue".into(),
        held: false,
    }];
    assert!(screen(&app).contains("then update docs"));
    let actions = app.on_key(with(KeyCode::Up, KeyModifiers::ALT));
    assert_eq!(actions, vec![Action::RemoveQueued("m5".into())]);
    assert_eq!(app.composer.text(), "then update docs");
}

/// Alt+Up takes back the user's own queued input, never a received message: taking
/// one back would drop another Session's message and paste its untrusted block into
/// the composer.
#[test]
fn taking_back_skips_received_messages() {
    let mut app = App::new(session(true), Vec::new(), "cyber");
    let row = |id: &str, text: &str| Queued {
        message_id: id.into(),
        text: text.into(),
        delivery: "queue".into(),
        held: false,
    };
    app.queued = vec![
        row("m5", "then update docs"),
        row("msgx_q", "<cross_session_message>"),
    ];
    app.messaging.records = vec![crate::messaging::Record {
        message_id: "msgx_q".into(),
        incoming: true,
        ..Default::default()
    }];
    let actions = app.on_key(with(KeyCode::Up, KeyModifiers::ALT));
    assert_eq!(actions, vec![Action::RemoveQueued("m5".into())]);
    assert_eq!(app.composer.text(), "then update docs");
    assert_eq!(app.queued.len(), 1);
    assert!(app.on_key(with(KeyCode::Up, KeyModifiers::ALT)).is_empty());
    assert_eq!(app.queued[0].message_id, "msgx_q");
}

#[test]
fn pickers_filter_and_choose() {
    let mut app = App::new(session(false), Vec::new(), "cyber");
    let items = ["openai/gpt-6-luna", "anthropic/claude-x"]
        .iter()
        .map(|m| crate::model::Choice {
            key: (*m).into(),
            label: (*m).into(),
            detail: String::new(),
        })
        .collect();
    app.open_picker(crate::app::PickerKind::Models, "Models", items);
    typed(&mut app, "claude");
    assert_eq!(
        app.on_key(key(KeyCode::Enter)),
        vec![Action::SwitchModel("anthropic/claude-x".into())]
    );
    // `<leader>s` opens the theme switcher; `<leader>t` cycles themes.
    app.on_key(with(KeyCode::Char('x'), KeyModifiers::CONTROL));
    app.on_key(key(KeyCode::Char('s')));
    typed(&mut app, "nord");
    assert_eq!(
        app.on_key(key(KeyCode::Enter)),
        vec![Action::SaveTheme("nord".into())]
    );
    assert_eq!(app.theme.name, "nord");
}

#[test]
fn shift_tab_cycles_modes_and_ctrl_c_quits_when_empty() {
    let mut app = App::new(session(false), Vec::new(), "cyber");
    assert_eq!(
        app.on_key(key(KeyCode::BackTab)),
        vec![Action::SwitchMode("accept-edits".into())]
    );
    typed(&mut app, "x");
    assert!(
        app.on_key(with(KeyCode::Char('c'), KeyModifiers::CONTROL))
            .is_empty()
    );
    assert_eq!(
        app.on_key(with(KeyCode::Char('c'), KeyModifiers::CONTROL)),
        vec![Action::Quit]
    );
}

#[test]
fn mode_cycle_includes_auto_and_excludes_unattended_modes() {
    for (current, next) in [
        ("default", "accept-edits"),
        ("accept-edits", "plan"),
        ("plan", "auto"),
        ("auto", "default"),
        ("bypass", "default"),
        ("dont-ask", "default"),
    ] {
        let mut s = session(false);
        s.mode = current.into();
        let mut app = App::new(s, Vec::new(), "cyber");
        assert_eq!(
            app.on_key(key(KeyCode::BackTab)),
            vec![Action::SwitchMode(next.into())]
        );
    }
}

#[test]
fn pending_mode_keeps_the_effective_mode_visible_and_cycles_the_selection() {
    let s = Session::parse(
        &json!({"mode":"plan", "effective_mode":"default", "pending_mode":"plan", "status":"running"}),
    );
    let mut app = App::new(s, Vec::new(), "cyber");
    assert!(screen(&app).contains("default → plan (pending)"));
    assert!(screen(&app).contains("esc interrupt"));
    assert_eq!(
        app.on_key(key(KeyCode::BackTab)),
        vec![Action::SwitchMode("auto".into())]
    );
    app.set_session(Session::parse(
        &json!({"mode":"auto", "effective_mode":"plan", "pending_mode":"auto", "status":"running"}),
    ));
    assert!(screen(&app).contains("⏸ plan → auto (pending)"));
    app.set_session(Session::parse(
        &json!({"mode":"auto", "effective_mode":"auto", "pending_mode":null, "status":"running"}),
    ));
    let visible = screen(&app);
    assert!(!visible.contains("(pending)"));
    assert!(!visible.contains("(switching)"));
    assert_eq!(
        Session::parse(&json!({"mode":"auto"})).mode_label(),
        "auto",
        "older servers retain a mode indicator"
    );
}

#[test]
fn bypass_requires_confirmation_from_slash_command_and_picker() {
    let mut app = App::new(session(false), Vec::new(), "cyber");
    typed(&mut app, "/mode bypass");
    assert!(app.on_key(key(KeyCode::Enter)).is_empty());
    assert!(matches!(app.overlay, Overlay::ConfirmBypass));
    assert!(screen(&app).contains("without normal permission prompts"));
    assert!(
        app.on_key(key(KeyCode::Enter)).is_empty(),
        "Enter does not accidentally confirm"
    );
    assert!(app.on_key(key(KeyCode::Esc)).is_empty());
    assert_eq!(app.session.mode, "default");
    typed(&mut app, "/mode");
    app.on_key(key(KeyCode::Enter));
    let Overlay::Picker(picker) = &mut app.overlay else {
        panic!("mode picker")
    };
    picker.selected = picker
        .filtered
        .iter()
        .position(|i| picker.items[*i].key == "bypass")
        .unwrap();
    assert!(app.on_key(key(KeyCode::Enter)).is_empty());
    assert!(matches!(app.overlay, Overlay::ConfirmBypass));
    assert_eq!(
        app.on_key(key(KeyCode::Char('y'))),
        vec![Action::SwitchMode("bypass".into())]
    );
}

#[test]
fn rapid_cycle_tracks_latest_selection_until_acknowledged() {
    let mut app = App::new(session(true), Vec::new(), "cyber");
    assert_eq!(
        app.on_key(key(KeyCode::BackTab)),
        vec![Action::SwitchMode("accept-edits".into())]
    );
    assert_eq!(
        app.on_key(key(KeyCode::BackTab)),
        vec![Action::SwitchMode("plan".into())]
    );
    assert!(screen(&app).contains("default → plan (switching)"));
    let mut intermediate = session(true);
    intermediate.mode = "accept-edits".into();
    intermediate.effective_mode = "default".into();
    intermediate.pending_mode = Some("accept-edits".into());
    app.set_session(intermediate);
    assert!(screen(&app).contains("default → plan (switching)"));
    let mut accepted = session(true);
    accepted.mode = "plan".into();
    accepted.effective_mode = "default".into();
    accepted.pending_mode = Some("plan".into());
    app.set_session(accepted);
    assert!(screen(&app).contains("default → plan (pending)"));
}

#[test]
fn the_session_picker_renames_archives_and_confirms_deletes() {
    let mut app = App::new(session(false), Vec::new(), "cyber");
    let items = vec![crate::model::Choice {
        key: "ses_2".into(),
        label: "Old".into(),
        detail: String::new(),
    }];
    app.open_picker(crate::app::PickerKind::Sessions, "Sessions", items);
    assert!(screen(&app).contains("^R rename"));
    app.on_key(with(KeyCode::Char('r'), KeyModifiers::CONTROL));
    for _ in 0..3 {
        app.on_key(key(KeyCode::Backspace));
    }
    typed(&mut app, "New name");
    assert_eq!(
        app.on_key(key(KeyCode::Enter)),
        vec![Action::Rename {
            id: "ses_2".into(),
            title: "New name".into()
        }]
    );
    assert_eq!(
        app.on_key(with(KeyCode::Char('a'), KeyModifiers::CONTROL)),
        vec![Action::Archive("ses_2".into())]
    );
    app.on_key(with(KeyCode::Char('d'), KeyModifiers::CONTROL));
    assert!(screen(&app).contains("y to confirm"));
    assert!(
        app.on_key(key(KeyCode::Char('n'))).is_empty(),
        "anything but y cancels"
    );
    app.on_key(with(KeyCode::Char('d'), KeyModifiers::CONTROL));
    assert_eq!(
        app.on_key(key(KeyCode::Char('y'))),
        vec![Action::Delete("ses_2".into())]
    );
}

#[test]
fn critical_permission_warning_is_visible_in_the_error_color() {
    let mut app = App::new(session(true), Vec::new(), "cyber");
    let mut request = permission();
    if let Request::Permission { metadata, .. } = &mut request {
        *metadata = json!({"warning": "Danger: this command can remove the repository.", "severity": "danger"});
    }
    app.requests = vec![request];
    app.sync_overlay();
    assert!(screen(&app).contains("Danger: this command can remove the repository."));
    let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
    terminal.draw(|f| crate::view::draw(f, &app)).unwrap();
    let buf = terminal.backend().buffer();
    let warning = buf
        .content
        .iter()
        .find(|cell| cell.symbol() == "D" && cell.fg == app.theme.error);
    assert!(
        warning.is_some(),
        "danger warning must use the theme's error color"
    );
    assert!(screen(&app).contains("Allow once"));
}

#[test]
fn approve_command_and_confirmation_show_exact_call_without_permanent_approval() {
    let mut app = App::new(session(false), Vec::new(), "cyber");
    typed(&mut app, "/approve");
    app.completion = None;
    assert_eq!(app.on_key(key(KeyCode::Enter)), vec![Action::ApproveAuto]);
    let mut request = permission();
    if let Request::Permission {
        action,
        metadata,
        patterns,
        ..
    } = &mut request
    {
        *action = "auto_override".into();
        patterns.clear();
        *metadata = json!({"requires_confirmation":true,"classifier_reason":"Remote push requires review","tool":"bash","input":{"command":"git push origin feature/x"}});
    }
    app.requests = vec![request];
    app.sync_overlay();
    let rendered = screen(&app);
    assert!(rendered.contains("Remote push requires review"));
    assert!(rendered.contains("git push origin feature/x"));
    assert!(rendered.contains("Confirm replay once"));
    assert!(!rendered.contains("Allow always"));
    assert!(app.on_key(key(KeyCode::Char('a'))).is_empty());
    let actions = app.on_key(key(KeyCode::Char('y')));
    assert!(matches!(&actions[..],[Action::Reply { body,.. }] if body["reply"] == "once"));
}

#[test]
fn long_override_details_scroll_while_confirmation_controls_stay_visible() {
    let mut app = App::new(session(false), Vec::new(), "cyber");
    let mut request = permission();
    if let Request::Permission {
        action, metadata, ..
    } = &mut request
    {
        *action = "auto_override".into();
        *metadata = json!({"classifier_reason":"Review command","tool":"bash","input":{"command":"long argument ".repeat(300)}});
    }
    app.requests = vec![request];
    app.sync_overlay();
    assert!(screen(&app).contains("Confirm replay once"));
    app.on_key(key(KeyCode::PageDown));
    assert!(app.permission_scroll > 0);
    let rendered = screen(&app);
    assert!(rendered.contains("Confirm replay once"));
    assert!(rendered.contains("Cancel (n)"));
    app.on_key(key(KeyCode::Home));
    assert_eq!(app.permission_scroll, 0);
}

#[test]
fn routed_child_approval_shows_its_name_and_replies_from_the_parent() {
    let mut app = App::new(session(true), Vec::new(), "cyber");
    let request = Request::parse(&json!({
        "id": "per_child", "session_id": "ses_child", "kind": "permission",
        "action": "bash", "resources": ["npm test"], "always_patterns": [], "metadata": {},
        "origin": { "title": "Review changes (@general)", "agent": "general", "directory": "/worktree" },
        "routed_to": [{ "session_id": "ses_1", "directory": "/repo" }]
    })).unwrap();
    app.requests = vec![request];
    app.sync_overlay();
    assert!(matches!(app.overlay, Overlay::Permission(_)));
    assert!(screen(&app).contains("Review changes (@general)"));
    assert_eq!(
        app.on_key(key(KeyCode::Enter)),
        vec![Action::Reply {
            request: "per_child".into(),
            body: json!({"reply": "once"})
        }]
    );
}

#[test]
fn routed_child_question_shows_its_name_and_unrelated_requests_stay_hidden() {
    let mut app = App::new(session(true), Vec::new(), "cyber");
    let mut data = json!({
        "id": "que_child", "session_id": "ses_child", "kind": "question",
        "questions": [{ "question": "Which storage?", "header": "Storage", "options": [{ "label": "sqlite" }] }],
        "origin": { "title": "Inspect storage (@explore)", "agent": "explore", "directory": "/worktree" },
        "routed_to": [{ "session_id": "ses_other", "directory": "/elsewhere" }]
    });
    app.requests = vec![Request::parse(&data).unwrap()];
    app.sync_overlay();
    assert!(app.active_request().is_none());
    data["routed_to"][0]["session_id"] = json!("ses_1");
    app.requests = vec![Request::parse(&data).unwrap()];
    app.sync_overlay();
    assert!(matches!(app.overlay, Overlay::Question(_)));
    assert!(screen(&app).contains("Inspect storage (@explore)"));
}

#[test]
fn routed_child_prompt_refresh_replaces_a_settled_request_form() {
    let mut app = App::new(session(true), Vec::new(), "cyber");
    let permission = json!({
        "id": "per_child", "session_id": "ses_child", "kind": "permission", "action": "bash",
        "resources": ["npm test"], "always_patterns": [], "metadata": {},
        "origin": { "title": "Review changes (@general)" },
        "routed_to": [{ "session_id": "ses_1" }]
    });
    app.requests = vec![Request::parse(&permission).unwrap()];
    app.sync_overlay();
    let question = json!({
        "id": "que_child", "session_id": "ses_child", "kind": "question",
        "questions": [{ "question": "Which storage?", "header": "Storage", "options": [{ "label": "sqlite" }] }],
        "origin": { "title": "Inspect storage (@explore)" },
        "routed_to": [{ "session_id": "ses_1" }]
    });
    // A snapshot after another client answers the permission contains the next request.
    app.requests = vec![Request::parse(&question).unwrap()];
    app.sync_overlay();
    assert!(matches!(app.overlay, Overlay::Question(_)));
    assert!(screen(&app).contains("Which storage?"));
}

#[test]
fn tasks_picker_opens_children_and_stops_selected_jobs() {
    use crate::app::PickerKind;
    let mut app = App::new(session(false), vec![], "cyber");
    typed(&mut app, "/tasks");
    assert!(matches!(
        app.on_key(key(KeyCode::Enter)).as_slice(),
        [Action::LoadTasks]
    ));
    let items = vec![crate::model::Choice {
        key: "job_1".into(),
        label: "scan (running)".into(),
        detail: "subagent · 3s".into(),
    }];
    app.open_picker(PickerKind::Tasks, "Tasks", items.clone());
    assert!(screen(&app).contains("scan (running)"));
    assert!(
        matches!(app.on_key(with(KeyCode::Char('s'),KeyModifiers::CONTROL)).as_slice(),[Action::StopTask(id)] if id=="job_1")
    );
    app.open_picker(PickerKind::Tasks, "Tasks", items);
    assert!(
        matches!(app.on_key(key(KeyCode::Enter)).as_slice(),[Action::OpenTask(id)] if id=="job_1")
    );
}

#[test]
fn stop_all_tasks_requires_confirmation() {
    let mut app = App::new(session(false), vec![], "cyber");
    typed(&mut app, "/stop");
    assert!(app.on_key(key(KeyCode::Enter)).is_empty());
    assert_eq!(app.overlay, Overlay::ConfirmStopTasks);
    assert!(screen(&app).contains("Stop all running background tasks"));
    assert!(app.on_key(key(KeyCode::Esc)).is_empty());
    typed(&mut app, "/stop");
    app.on_key(key(KeyCode::Enter));
    assert!(matches!(
        app.on_key(key(KeyCode::Char('y'))).as_slice(),
        [Action::StopTasks]
    ));
}

#[test]
fn subtask_requires_a_prompt_and_keeps_the_parent_open() {
    for running in [false, true] {
        let mut app = App::new(session(running), Vec::new(), "cyber");
        typed(&mut app, "/subtask try another approach");
        assert_eq!(
            app.on_key(key(KeyCode::Enter)),
            vec![Action::Subtask("try another approach".into())]
        );
        assert_eq!(app.session.id, "ses_1");
        typed(&mut app, "/subtask");
        assert!(app.on_key(key(KeyCode::Enter)).is_empty());
        assert!(app.toast.as_ref().unwrap().0.contains("Usage: /subtask"));
    }
}

#[tokio::test]
async fn located_actions_use_current_session_directory_instead_of_initial_client_scope() {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let server = std::thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        socket
            .set_read_timeout(Some(std::time::Duration::from_secs(5)))
            .unwrap();
        let mut head = Vec::new();
        let mut bytes = [0; 1024];
        while !head.windows(4).any(|window| window == b"\r\n\r\n") {
            let count = socket.read(&mut bytes).unwrap();
            assert!(count > 0);
            head.extend_from_slice(&bytes[..count]);
        }
        socket
            .write_all(
                b"HTTP/1.1 200 OK\r\nContent-Length: 11\r\nConnection: close\r\n\r\n{\"data\":[]}",
            )
            .unwrap();
        String::from_utf8(head).unwrap()
    });
    let client = cyber_client::Client::http(&base, None).at("/repo/old");
    let mut active = session(false);
    active.directory = "/repo/new".into();
    crate::perform::perform(&client, &active, Action::FindFiles("config".into()))
        .await
        .unwrap();
    let request = server.join().unwrap().to_ascii_lowercase();
    assert!(
        request.contains("x-cyber-directory: /repo/new"),
        "{request}"
    );
    assert!(!request.contains("/repo/old"));
}

#[test]
fn worktree_rebound_updates_active_directory_before_a_snapshot_refresh() {
    let mut app = App::new(session(false), Vec::new(), "cyber");
    assert_eq!(
        app.on_event(
            "session.worktree.rebound.1",
            &json!({"from":{"path":"/repo"},"to":{"path":"/repo/new"}})
        ),
        vec![Action::Refresh]
    );
    assert_eq!(app.session.directory, "/repo/new");
}

#[test]
fn rebound_cursor_rejects_stale_or_malformed_directory_changes() {
    let mut app = App::new(session(false), Vec::new(), "cyber");
    app.session.seq = 3;
    let mut event = cyber_client::Event {
        kind: "session.worktree.rebound.1".into(),
        data: json!({"to":{"path":"/repo/new"}}),
        seq: Some(9),
        session_id: Some(app.session.id.clone()),
    };
    assert_eq!(app.on_server_event(&event), vec![Action::Refresh]);
    assert_eq!(app.session.directory, "/repo/new");
    assert_eq!(app.session.seq, 9);
    event.seq = Some(8);
    event.data = json!({"to":{"path":"/repo/old"}});
    assert!(app.on_server_event(&event).is_empty());
    assert_eq!(app.session.directory, "/repo/new");
    event.seq = None;
    assert_eq!(app.on_server_event(&event), vec![Action::Refresh]);
    assert_eq!(app.session.directory, "/repo/new");
    event.seq = Some(10);
    event.data = json!({"to":{"path":null}});
    assert_eq!(app.on_server_event(&event), vec![Action::Refresh]);
    assert_eq!(app.session.directory, "/repo/new");
    assert_eq!(app.session.seq, 9);
}

#[test]
fn leading_agent_completions_coexist_with_files_and_clear_after_relocation() {
    use crate::model::Choice;
    let mut app = App::new(session(false), Vec::new(), "cyber");
    let actions = typed(&mut app, "@ex");
    assert!(actions.contains(&Action::LoadAgents));
    app.files = vec!["example.rs".into()];
    app.set_agents(vec![Choice {
        key: "explore".into(),
        label: "explore".into(),
        detail: "agent · read only".into(),
    }]);
    let completion = app.completion.as_ref().unwrap();
    assert!(
        completion
            .items
            .iter()
            .any(|choice| choice.key == "explore")
    );
    assert!(
        completion
            .items
            .iter()
            .any(|choice| choice.key == "example.rs")
    );
    assert!(screen(&app).contains("read only"));
    app.composer.set_text("see @ex");
    let actions = typed(&mut app, "p");
    assert!(!actions.contains(&Action::LoadAgents));
    assert!(
        app.completion
            .as_ref()
            .unwrap()
            .items
            .iter()
            .all(|choice| choice.key != "explore")
    );
    let mut relocated = session(false);
    relocated.directory = "/new".into();
    app.set_session(relocated);
    assert!(app.agents.is_empty());
    assert!(app.completion.is_none());
    app.composer.set_text("@rev");
    app.set_agents(vec![Choice {
        key: "review team".into(),
        label: "review team".into(),
        detail: "agent · review".into(),
    }]);
    app.on_key(key(KeyCode::Tab));
    assert_eq!(app.composer.text(), "@\"review team\" ");
}

#[tokio::test]
async fn leading_mentions_use_named_subtasks_while_file_and_embedded_text_use_prompts() {
    use crate::perform::{Msg, perform};
    use std::io::{Read, Write};
    for (text, lookup, target, delivery) in [
        ("@explore find retry logic", true, Some("explore"), "steer"),
        ("@explore find retry logic", true, Some("explore"), "queue"),
        ("@README.md inspect", true, None, "steer"),
        ("look at @explore", false, None, "steer"),
        ("\"@explore\" inspect", false, None, "steer"),
    ] {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let mut requests = Vec::new();
            let responses = if delivery == "queue" {
                vec![json!({"data":[{"name":"explore","mode":"subagent"}]})]
            } else if lookup {
                vec![
                    json!({"data":[{"name":"explore","mode":"subagent","description":"read only"},{"name":"build","mode":"primary"}]}),
                    json!({"data":{"name":"explore","id":"job_test"}}),
                ]
            } else {
                vec![json!({"data":{}})]
            };
            for response in responses {
                let (mut socket, _) = listener.accept().unwrap();
                socket
                    .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                    .unwrap();
                let mut request = Vec::new();
                let mut buffer = [0; 2048];
                let boundary = loop {
                    if let Some(position) = request.windows(4).position(|part| part == b"\r\n\r\n")
                    {
                        break position + 4;
                    }
                    let n = socket.read(&mut buffer).unwrap();
                    assert!(n > 0);
                    request.extend_from_slice(&buffer[..n]);
                };
                let header = String::from_utf8(request[..boundary].to_vec()).unwrap();
                let length = header
                    .lines()
                    .find_map(|line| {
                        line.to_ascii_lowercase()
                            .strip_prefix("content-length: ")
                            .and_then(|value| value.parse::<usize>().ok())
                    })
                    .unwrap_or(0);
                while request.len() < boundary + length {
                    let n = socket.read(&mut buffer).unwrap();
                    assert!(n > 0);
                    request.extend_from_slice(&buffer[..n]);
                }
                let body = if length == 0 {
                    json!(null)
                } else {
                    serde_json::from_slice(&request[boundary..boundary + length]).unwrap()
                };
                requests.push((header.clone(), body));
                let response = if header.starts_with("POST /api/v1/sessions/ses_1/delegations/") {
                    let id = header
                        .lines()
                        .next()
                        .unwrap()
                        .split_whitespace()
                        .nth(1)
                        .unwrap()
                        .rsplit('/')
                        .next()
                        .unwrap();
                    json!({"data":{"id":id,"session_id":"ses_1","phase":"launching","status":"admitted","job_id":"job_test"}})
                } else {
                    response
                };
                let body = response.to_string();
                socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).as_bytes()).unwrap();
            }
            requests
        });
        let client = cyber_client::Client::http(&base, None).at("/old");
        let result = perform(
            &client,
            &session(false),
            Action::Prompt {
                text: text.into(),
                delivery,
            },
        )
        .await;
        let requests = server.join().unwrap();
        if delivery == "queue" {
            assert!(result.unwrap_err().contains("Enter"));
            assert_eq!(requests.len(), 1);
            assert!(requests[0].0.starts_with("GET /api/v1/agents "));
            continue;
        }
        let result = result.unwrap();
        assert_eq!(requests.len(), if lookup { 2 } else { 1 });
        let (head, body) = requests.last().unwrap();
        assert!(
            head.to_ascii_lowercase()
                .contains("x-cyber-directory: /repo")
        );
        if let Some(target) = target {
            assert!(
                head.starts_with("POST /api/v1/sessions/ses_1/delegations/op_"),
                "{head}"
            );
            assert_eq!(body, &json!({"agent":target,"prompt":"find retry logic"}));
            assert!(matches!(
                result,
                Msg::AdmissionUpdated { result: Ok(_), .. }
            ));
        } else {
            assert!(
                head.starts_with("POST /api/v1/sessions/ses_1/prompt "),
                "{head}"
            );
            assert_eq!(body["parts"][0]["text"], text);
            assert_eq!(body["delivery"], "steer");
        }
    }
}

#[test]
fn pending_admission_escape_and_picker_actions_preserve_source_after_session_switch() {
    let mut app = App::new(session(true), vec![], "cyber");
    let request = crate::admissions::Request {
        id: "op_queued".into(),
        source: "ses_1".into(),
        directory: "/repo".into(),
    };
    app.admissions.register(request.clone());
    assert_eq!(app.on_key(key(KeyCode::Esc)), vec![request.action(true)]);
    typed(&mut app, "/admissions");
    app.on_key(key(KeyCode::Enter));
    assert!(
        matches!(&app.overlay,Overlay::Picker(picker) if picker.kind==crate::app::PickerKind::Admissions && picker.items.len()==1)
    );
    assert_eq!(
        app.on_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL)),
        vec![request.action(true)]
    );
    app.overlay = Overlay::None;
    let mut other = session(false);
    other.id = "ses_other".into();
    other.directory = "/other".into();
    app.set_session(other);
    assert!(app.on_key(key(KeyCode::Esc)).is_empty());
    assert_eq!(app.admissions.0["op_queued"].request, request);
}

#[test]
fn admission_picker_updates_acknowledgement_without_changing_selected_request() {
    let mut app = App::new(session(false), vec![], "cyber");
    for id in ["op_a", "op_b"] {
        app.admissions.register(crate::admissions::Request {
            id: id.into(),
            source: "ses_1".into(),
            directory: "/repo".into(),
        });
    }
    typed(&mut app, "/admissions");
    app.on_key(key(KeyCode::Enter));
    app.on_key(key(KeyCode::Down));
    let selected = app.admissions.0["op_b"].request.clone();
    let dir = tempfile::tempdir().unwrap();
    app.admissions.update(
        selected,
        Ok(json!({"id":"op_b","session_id":"ses_1","phase":"reserved","status":"cancelled"})),
        true,
        &crate::store::LocalStore::new(dir.path()),
    );
    app.refresh_admissions();
    let Overlay::Picker(picker) = &app.overlay else {
        panic!("Expected admissions picker")
    };
    assert_eq!(picker.current().unwrap().key, "op_b");
    assert!(picker.current().unwrap().label.starts_with("cancelled"));
}

fn cost_fixture() -> serde_json::Value {
    json!({"scope":"session","id":"ses_1",
        "own":{"tokens":{"input":100,"output":20,"reasoning":3,"cache_read":200,"cache_write":50},"cost":0.25,"unpriced_steps":0,"total_tokens":373,"usage_complete":true,"token_classes_complete":true},
        "descendants":{"tokens":{"input":400,"output":80,"reasoning":7,"cache_read":300,"cache_write":150},"cost":0.75,"unpriced_steps":0,"total_tokens":937,"usage_complete":true,"token_classes_complete":true},
        "total":{"tokens":{"input":500,"output":100,"reasoning":10,"cache_read":500,"cache_write":200},"cost":1.0,"unpriced_steps":0,"total_tokens":1310,"usage_complete":true,"token_classes_complete":true}})
}

/// The Usage tab's `Total cost:` value.
fn total_cost(app: &App) -> String {
    let shown = screen(app);
    shown
        .lines()
        .find_map(|line| line.trim().strip_prefix("Total cost:"))
        .map(|value| value.trim().to_owned())
        .unwrap_or_default()
}

fn apply_cost(app: &mut App, data: &serde_json::Value) {
    let result = crate::settings::cost::Cost::parse_report(data, &app.session.id);
    app.cost.apply(&app.session.id, app.cost.generation, result);
}

#[test]
fn cost_command_refreshes_and_renders_all_classes_for_the_subtree() {
    let mut app = App::new(session(false), Vec::new(), "cyber");
    typed(&mut app, "/cost");
    assert_eq!(
        app.on_key(key(KeyCode::Enter)),
        vec![
            Action::LoadCost { generation: 1 },
            Action::LoadLimits { generation: 1 },
            Action::LoadInsights {
                generation: 1,
                window: "24h"
            }
        ]
    );
    apply_cost(&mut app, &cost_fixture());
    let rendered = screen(&app);
    assert!(matches!(app.overlay, Overlay::Settings));
    assert_eq!(app.settings.tab, crate::settings::Tab::Usage);
    for text in [
        "Session",
        "Input",
        "Output",
        "Reasoning",
        "Cache read",
        "Cache write",
        "500",
        "100",
        "10",
        "200",
        "41.7%",
    ] {
        assert!(rendered.contains(text), "missing {text}: {rendered}");
    }
    assert_eq!(total_cost(&app), "$1.00");
    assert!(app.on_key(key(KeyCode::Esc)).is_empty());
    assert!(matches!(app.overlay, Overlay::None));
}

#[test]
fn cost_command_discloses_unpriced_and_incomplete_attribution() {
    let mut data = cost_fixture();
    data["own"]["unpriced_steps"] = json!(1);
    data["total"]["unpriced_steps"] = json!(1);
    for scope in ["descendants", "total"] {
        data[scope]["usage_complete"] = json!(false);
        data[scope]["token_classes_complete"] = json!(false);
    }
    let mut app = App::new(session(false), Vec::new(), "cyber");
    typed(&mut app, "/cost");
    app.on_key(key(KeyCode::Enter));
    apply_cost(&mut app, &data);
    let rendered = screen(&app);
    assert!(rendered.contains("unpriced"));
    assert!(rendered.contains("incomplete"));
    assert!(rendered.contains("Cache hit rate: unknown"));
    assert_eq!(total_cost(&app), "≥ $1.00 · 1 unpriced calls");
}

#[test]
fn cost_refresh_replaces_the_snapshot_and_preserves_it_on_failure() {
    let mut app = App::new(session(false), Vec::new(), "cyber");
    typed(&mut app, "/cost");
    app.on_key(key(KeyCode::Enter));
    apply_cost(&mut app, &cost_fixture());
    assert_eq!(
        app.on_key(key(KeyCode::Char('r'))),
        vec![
            Action::LoadCost { generation: 2 },
            Action::LoadLimits { generation: 2 },
            Action::LoadInsights {
                generation: 2,
                window: "24h"
            }
        ]
    );
    let mut next = cost_fixture();
    next["descendants"]["cost"] = json!(1.75);
    next["total"]["cost"] = json!(2.0);
    apply_cost(&mut app, &next);
    assert_eq!(total_cost(&app), "$2.00");
    // `R`, the overlay's refresh key, still refreshes for one release.
    assert_eq!(
        app.on_key(key(KeyCode::Char('R'))),
        vec![
            Action::LoadCost { generation: 3 },
            Action::LoadLimits { generation: 3 },
            Action::LoadInsights {
                generation: 3,
                window: "24h"
            }
        ]
    );
    next["descendants"]["tokens"]["input"] = json!(-1);
    apply_cost(&mut app, &next);
    let rendered = screen(&app);
    assert_eq!(total_cost(&app), "$2.00");
    assert!(rendered.contains("Could not load usage"), "{rendered}");
}

#[test]
fn cost_zero_prompt_and_invalid_classes_have_distinct_displays() {
    let mut data = cost_fixture();
    for scope in ["own", "descendants", "total"] {
        for class in ["input", "output", "reasoning", "cache_read", "cache_write"] {
            data[scope]["tokens"][class] = json!(0);
        }
        data[scope]["total_tokens"] = json!(0);
    }
    let mut app = App::new(session(false), Vec::new(), "cyber");
    typed(&mut app, "/cost");
    app.on_key(key(KeyCode::Enter));
    apply_cost(&mut app, &data);
    assert!(screen(&app).contains("no prompt tokens"));
    data["descendants"]["tokens"]["input"] = json!(-1);
    assert!(crate::settings::cost::Cost::parse_report(&data, "ses_1").is_err());
}

#[tokio::test]
async fn cost_command_requests_atomic_usage_instead_of_conversation_history() {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let server = std::thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        socket
            .set_read_timeout(Some(std::time::Duration::from_secs(5)))
            .unwrap();
        let mut request = Vec::new();
        let mut buffer = [0; 2048];
        while !request.windows(4).any(|v| v == b"\r\n\r\n") {
            let n = socket.read(&mut buffer).unwrap();
            assert!(n > 0);
            request.extend_from_slice(&buffer[..n]);
        }
        let header = String::from_utf8(request).unwrap();
        let (status, body) = if header.starts_with("GET /api/v1/usage?") {
            ("200 OK", json!({"data":cost_fixture()}).to_string())
        } else {
            (
                "404 Not Found",
                json!({"_tag":"SessionNotFoundError","message":"wrong endpoint"}).to_string(),
            )
        };
        write!(socket,"HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).unwrap();
        header
    });
    let client = cyber_client::Client::http(&base, None);
    let mut app = App::new(session(false), Vec::new(), "cyber");
    typed(&mut app, "/cost");
    let action = app.on_key(key(KeyCode::Enter)).remove(0);
    let result = crate::perform::perform(&client, &app.session, action).await;
    let request = server.join().unwrap();
    assert!(
        request.starts_with("GET /api/v1/usage?scope=session&id=ses_1 "),
        "{request}"
    );
    let crate::perform::Msg::Cost {
        session_id,
        generation,
        result,
    } = result.unwrap()
    else {
        panic!("expected usage snapshot")
    };
    app.cost.apply(&session_id, generation, result);
    assert_eq!(total_cost(&app), "$1.00");
}

#[test]
fn cost_snapshot_refuses_foreign_missing_and_inconsistent_reports() {
    let mut rounded = cost_fixture();
    rounded["own"]["cost"] = json!(0.1);
    rounded["descendants"]["cost"] = json!(0.2);
    rounded["total"]["cost"] = json!(0.3);
    assert!(crate::settings::cost::Cost::parse_report(&rounded, "ses_1").is_ok());

    for (path, value) in [
        (vec!["id"], json!("ses_foreign")),
        (vec!["scope"], json!("project")),
        (vec!["own", "tokens", "input"], json!(null)),
        (vec!["descendants", "usage_complete"], json!(null)),
        (vec!["total", "cost"], json!(9.0)),
        (vec!["total", "tokens", "input"], json!(501)),
        (vec!["total", "unpriced_steps"], json!(1)),
        (vec!["descendants", "total_tokens"], json!(0)),
    ] {
        let mut report = cost_fixture();
        let mut field = &mut report;
        for part in path {
            field = &mut field[part];
        }
        *field = value;
        assert!(crate::settings::cost::Cost::parse_report(&report, "ses_1").is_err());
    }
}

#[test]
fn cost_refresh_ignores_superseded_responses_and_invalidates_on_session_switch() {
    let mut app = App::new(session(false), Vec::new(), "cyber");
    typed(&mut app, "/cost");
    app.on_key(key(KeyCode::Enter));
    let first = app.cost.generation;
    app.on_key(key(KeyCode::Char('r')));
    app.cost.apply(
        "ses_1",
        first,
        crate::settings::cost::Cost::parse_report(&cost_fixture(), "ses_1"),
    );
    assert_eq!(total_cost(&app), "");
    apply_cost(&mut app, &cost_fixture());
    let mut stale = session(false);
    stale.cost = 99.99;
    app.set_session(stale);
    assert_eq!(total_cost(&app), "$1.00");
    let mut other = session(false);
    other.id = "ses_other".into();
    app.set_session(other);
    assert!(matches!(app.overlay, Overlay::None));
    app.set_session(session(false));
    typed(&mut app, "/cost");
    app.on_key(key(KeyCode::Enter));
    app.cost.apply(
        "ses_1",
        first,
        crate::settings::cost::Cost::parse_report(&cost_fixture(), "ses_1"),
    );
    assert_eq!(total_cost(&app), "");
}

#[test]
fn child_thread_commands_open_existing_threads_and_preserve_steer_delivery() {
    use crate::app::PickerKind;
    for command in ["/agent", "/subagents"] {
        let mut app = App::new(session(false), Vec::new(), "cyber");
        typed(&mut app, command);
        assert_eq!(app.on_key(key(KeyCode::Enter)), vec![Action::LoadChildren]);
    }
    let mut app = App::new(session(true), Vec::new(), "cyber");
    app.open_picker(
        PickerKind::Children,
        "Subagents",
        vec![crate::model::Choice {
            key: "ses_child".into(),
            label: "review (running)".into(),
            detail: "/worktree".into(),
        }],
    );
    assert_eq!(
        app.on_key(with(KeyCode::Char('r'), KeyModifiers::CONTROL)),
        vec![Action::LoadChildren]
    );
    assert_eq!(
        app.on_key(key(KeyCode::Enter)),
        vec![Action::Open("ses_child".into())]
    );
    app.set_session(Session {
        id: "ses_child".into(),
        parent_id: Some("ses_1".into()),
        directory: "/worktree".into(),
        running: true,
        ..session(true)
    });
    typed(&mut app, "inspect this too");
    assert_eq!(
        app.on_key(key(KeyCode::Enter)),
        vec![Action::Prompt {
            text: "inspect this too".into(),
            delivery: "steer"
        }]
    );
    assert_eq!(app.session.id, "ses_child");
    assert_eq!(app.session.directory, "/worktree");
}

fn hook_history_fixture() -> serde_json::Value {
    let row = |id: &str, status: &str| {
        let mut row = json!({"id":id,"session_id":"ses_1","hook_id":"guard\u{1b}[31m","event":"PreToolUse","status":status,"started_ms":1,
        "duration_ms":null,"outcome":null,"acknowledged":null,"must_stop":false,"call_id":"call_1","tool_name":"write","io":{"stdout":"private hook IO"}});
        if status == "unknown" {
            row["duration_ms"] = json!(2);
            row["outcome"] = json!("error");
            row["acknowledged"] = json!(false);
            row["must_stop"] = json!(true);
        }
        row
    };
    json!({"data":[row("hke_running","running"),row("hke_unknown","unknown")],"cursor":{"next":"1:ses_1:hke_next"}})
}
fn open_hook_history(app: &mut App) -> u64 {
    typed(app, "/hooks history");
    let actions = app.on_key(key(KeyCode::Enter));
    let [
        Action::LoadHookHistory {
            generation,
            cursor: None,
        },
    ] = actions.as_slice()
    else {
        panic!("expected receipt request")
    };
    *generation
}
#[test]
fn hook_history_renders_observations_without_raw_io_or_control_characters() {
    let mut app = App::new(session(false), Vec::new(), "cyber");
    let generation = open_hook_history(&mut app);
    assert!(matches!(app.overlay, Overlay::HookHistory));
    assert!(screen(&app).contains("Loading hook receipts"));
    app.hooks.apply(
        generation,
        crate::hooks::Page::parse(&hook_history_fixture(), "ses_1"),
    );
    let rendered = screen(&app);
    assert!(rendered.contains("live state unverified"));
    assert!(rendered.contains("recovery required"));
    assert!(rendered.contains("call_1") && rendered.contains("write"));
    assert!(!rendered.contains("private hook IO") && !rendered.contains('\u{1b}'));
    assert!(rendered.contains("\\u{1b}"));
    assert!(!format!("{:?}", app.hooks).contains("private hook IO"));
    assert!(rendered.contains("must stop: yes"));
}
#[test]
fn hook_history_pages_refreshes_scrolls_and_dismisses() {
    let mut app = App::new(session(false), Vec::new(), "cyber");
    let generation = open_hook_history(&mut app);
    app.hooks.apply(
        generation,
        crate::hooks::Page::parse(&hook_history_fixture(), "ses_1"),
    );
    app.on_key(key(KeyCode::Down));
    assert_eq!(app.hooks.scroll, 1);
    app.on_key(key(KeyCode::Up));
    assert_eq!(app.hooks.scroll, 0);
    let next = app.on_key(key(KeyCode::Char('n')));
    let [
        Action::LoadHookHistory {
            generation: next_generation,
            cursor: Some(cursor),
        },
    ] = next.as_slice()
    else {
        panic!("expected next page")
    };
    assert_eq!(cursor, "1:ses_1:hke_next");
    assert_ne!(generation, *next_generation);
    app.hooks.apply(
        *next_generation,
        crate::hooks::Page::parse(&json!({"data":[],"cursor":{"next":null}}), "ses_1"),
    );
    assert!(screen(&app).contains("No recorded hook executions"));
    assert!(app.on_key(key(KeyCode::Char('n'))).is_empty());
    assert!(matches!(
        app.on_key(key(KeyCode::Char('r'))).as_slice(),
        [Action::LoadHookHistory { cursor: None, .. }]
    ));
    assert!(screen(&app).contains("Loading hook receipts"));
    app.on_key(key(KeyCode::Esc));
    assert_eq!(app.overlay, Overlay::None);
}
#[test]
fn hook_history_refuses_foreign_and_malformed_pages_and_ignores_old_generations() {
    let mut foreign = hook_history_fixture();
    foreign["data"][0]["session_id"] = json!("ses_other");
    assert!(crate::hooks::Page::parse(&foreign, "ses_1").is_err());
    foreign = hook_history_fixture();
    foreign["data"][0]["status"] = json!("live");
    assert!(crate::hooks::Page::parse(&foreign, "ses_1").is_err());
    assert!(crate::hooks::Page::parse(&json!({"data":{}}), "ses_1").is_err());
    let mut app = App::new(session(false), Vec::new(), "cyber");
    let old = open_hook_history(&mut app);
    let refresh = app.on_key(key(KeyCode::Char('r')));
    let [Action::LoadHookHistory { generation, .. }] = refresh.as_slice() else {
        panic!("expected refresh")
    };
    app.hooks.apply(
        old,
        crate::hooks::Page::parse(&hook_history_fixture(), "ses_1"),
    );
    assert!(screen(&app).contains("Loading hook receipts"));
    app.hooks.apply(*generation, Err("unavailable".into()));
    assert!(screen(&app).contains("Could not load receipts: unavailable"));
    app.on_key(key(KeyCode::Esc));
    let generation = open_hook_history(&mut app);
    let mut other = session(false);
    other.id = "ses_other".into();
    app.set_session(other);
    assert_eq!(app.overlay, Overlay::None);
    app.hooks.apply(
        generation,
        crate::hooks::Page::parse(&hook_history_fixture(), "ses_1"),
    );
    assert!(!app.hooks.lines().join(" ").contains("hke_running"));
}
#[tokio::test]
async fn hook_history_uses_authenticated_receipt_api_and_encodes_next_cursor() {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let client = cyber_client::Client::http(
        &format!("http://{}", listener.local_addr().unwrap()),
        Some("pw".into()),
    );
    let server = std::thread::spawn(move || {
        let mut headers = Vec::new();
        for page in [
            hook_history_fixture(),
            json!({"data":[],"cursor":{"next":null}}),
        ] {
            let (mut socket, _) = listener.accept().unwrap();
            socket
                .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                .unwrap();
            let mut bytes = Vec::new();
            let mut buffer = [0; 2048];
            while !bytes.windows(4).any(|v| v == b"\r\n\r\n") {
                let n = socket.read(&mut buffer).unwrap();
                assert!(n > 0);
                bytes.extend_from_slice(&buffer[..n]);
            }
            headers.push(String::from_utf8(bytes).unwrap());
            let body = page.to_string();
            write!(socket,"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).unwrap();
        }
        headers
    });
    let mut app = App::new(session(false), Vec::new(), "cyber");
    let generation = open_hook_history(&mut app);
    let action = Action::LoadHookHistory {
        generation,
        cursor: None,
    };
    let crate::perform::Msg::HookHistory {
        session_id,
        generation,
        result,
    } = crate::perform::perform(&client, &app.session, action)
        .await
        .unwrap()
    else {
        panic!("expected receipt response")
    };
    assert_eq!(session_id, app.session.id);
    app.hooks.apply(generation, result);
    assert!(screen(&app).contains("hke_running"));
    let action = app.on_key(key(KeyCode::Char('n'))).remove(0);
    let crate::perform::Msg::HookHistory {
        generation, result, ..
    } = crate::perform::perform(&client, &app.session, action)
        .await
        .unwrap()
    else {
        panic!("expected next receipt response")
    };
    app.hooks.apply(generation, result);
    assert!(screen(&app).contains("No recorded hook executions"));
    let headers = server.join().unwrap();
    assert!(headers[0].starts_with("GET /api/v1/sessions/ses_1/hook-executions?limit=50 "));
    assert!(headers[1].starts_with(
        "GET /api/v1/sessions/ses_1/hook-executions?limit=50&cursor=1%3Ases_1%3Ahke_next "
    ));
    assert!(headers.iter().all(|header| {
        header.to_ascii_lowercase().contains(
            "authorization: basic Y3liZXI6cHc="
                .to_ascii_lowercase()
                .as_str(),
        )
    }));
}

fn hook_definitions_fixture(trusted: bool) -> serde_json::Value {
    serde_json::json!({"data":{"hooks":[{
        "event":"PreToolUse","scope":"project","source":"/repo/cyber.jsonc","pointer":"#/hooks/PreToolUse/0/hooks/0",
        "matcher":"write","paths":["src/**"],"handler":{"type":"http","url":"https://example.test/hook","headers":{"Authorization":"private-token"}},
        "digest":format!("sha256:{}","a".repeat(64)),"trusted":trusted,"sandbox_required":true
    }],"withheld_definitions":[],"checkout_trusted":true},"location":{"directory":"/repo"}})
}
fn open_hook_definitions(app: &mut App) -> u64 {
    typed(app, "/hooks");
    let actions = app.on_key(key(KeyCode::Enter));
    let [Action::LoadHookDefinitions { generation }] = actions.as_slice() else {
        panic!("expected catalog request")
    };
    *generation
}
#[test]
fn hook_definitions_review_requires_confirmation_for_exact_digest_and_masks_credentials() {
    let mut app = App::new(session(false), Vec::new(), "cyber");
    let generation = open_hook_definitions(&mut app);
    app.hook_definitions.apply(
        generation,
        crate::hook_definitions::Catalog::parse(&hook_definitions_fixture(false)),
    );
    let rendered = screen(&app);
    assert!(rendered.contains("PreToolUse") && rendered.contains("src/**"));
    assert!(rendered.contains("sandbox required: true"));
    assert!(
        !rendered.contains("private-token")
            && !format!("{:?}", app.hook_definitions).contains("private-token")
    );
    assert!(app.on_key(key(KeyCode::Char('y'))).is_empty());
    assert!(app.on_key(key(KeyCode::Char('t'))).is_empty());
    assert!(
        app.hook_definitions
            .lines()
            .join(" ")
            .contains("Approve this exact digest?")
    );
    app.on_key(key(KeyCode::Char('n')));
    assert!(app.on_key(key(KeyCode::Char('y'))).is_empty());
    app.on_key(key(KeyCode::Char('t')));
    let actions = app.on_key(key(KeyCode::Char('y')));
    let [
        Action::ChangeHookTrust {
            generation,
            digest,
            approve: true,
        },
    ] = actions.as_slice()
    else {
        panic!("expected confirmed approval")
    };
    assert_eq!(digest, &format!("sha256:{}", "a".repeat(64)));
    assert!(app.on_key(key(KeyCode::Char('y'))).is_empty());
    app.hook_definitions.apply(
        *generation,
        crate::hook_definitions::Catalog::parse(&hook_definitions_fixture(true)),
    );
    app.on_key(key(KeyCode::Char('u')));
    assert!(matches!(
        app.on_key(key(KeyCode::Char('y'))).as_slice(),
        [Action::ChangeHookTrust { approve: false, .. }]
    ));
}
#[test]
fn hook_definitions_withheld_global_and_untrusted_checkouts_cannot_be_approved() {
    for (scope, trusted_checkout) in [("global", true), ("project", false)] {
        let mut value = hook_definitions_fixture(false);
        value["data"]["hooks"][0]["scope"] = serde_json::json!(scope);
        value["data"]["checkout_trusted"] = serde_json::json!(trusted_checkout);
        value["data"]["withheld_definitions"] = serde_json::json!(["/repo/cyber.jsonc\u{1b}"]);
        let mut app = App::new(session(false), Vec::new(), "cyber");
        let generation = open_hook_definitions(&mut app);
        app.hook_definitions
            .apply(generation, crate::hook_definitions::Catalog::parse(&value));
        app.on_key(key(KeyCode::Char('t')));
        assert!(app.on_key(key(KeyCode::Char('y'))).is_empty());
        let rendered = screen(&app);
        assert!(!rendered.contains('\u{1b}'));
        assert!(rendered.contains("Withheld:"));
    }
}
#[test]
fn hook_definitions_refresh_dismissal_and_location_switch_invalidate_confirmation() {
    let mut app = App::new(session(false), Vec::new(), "cyber");
    let old = open_hook_definitions(&mut app);
    app.hook_definitions.apply(
        old,
        crate::hook_definitions::Catalog::parse(&hook_definitions_fixture(false)),
    );
    app.on_key(key(KeyCode::Char('t')));
    let actions = app.on_key(key(KeyCode::Char('r')));
    let [Action::LoadHookDefinitions { generation }] = actions.as_slice() else {
        panic!("expected refresh")
    };
    app.hook_definitions.apply(
        old,
        crate::hook_definitions::Catalog::parse(&hook_definitions_fixture(true)),
    );
    assert!(
        !app.hook_definitions
            .lines()
            .join(" ")
            .contains("Trusted: true")
    );
    assert!(app.on_key(key(KeyCode::Char('y'))).is_empty());
    app.hook_definitions.apply(
        *generation,
        crate::hook_definitions::Catalog::parse(&hook_definitions_fixture(false)),
    );
    app.on_key(key(KeyCode::Char('t')));
    app.on_key(key(KeyCode::Esc));
    assert!(matches!(app.overlay, Overlay::HookDefinitions));
    app.on_key(key(KeyCode::Esc));
    assert!(matches!(app.overlay, Overlay::None));
    app.hook_definitions.apply(
        *generation,
        crate::hook_definitions::Catalog::parse(&hook_definitions_fixture(true)),
    );
    let generation = open_hook_definitions(&mut app);
    app.hook_definitions.apply(
        generation,
        crate::hook_definitions::Catalog::parse(&hook_definitions_fixture(false)),
    );
    app.on_key(key(KeyCode::Char('t')));
    let mut rebound = app.session.clone();
    rebound.directory = "/other".into();
    app.set_session(rebound);
    assert!(matches!(app.overlay, Overlay::None));
    assert!(app.hook_definitions.change().is_none());
    let mut malformed = hook_definitions_fixture(false);
    malformed["data"]["hooks"][0]["digest"] = serde_json::json!("unknown");
    assert!(crate::hook_definitions::Catalog::parse(&malformed).is_err());
}
#[tokio::test]
async fn hook_definitions_http_uses_authenticated_location_and_refreshes_after_mutations() {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let client = cyber_client::Client::http(
        &format!("http://{}", listener.local_addr().unwrap()),
        Some("pw".into()),
    );
    let digest = format!("sha256:{}", "a".repeat(64));
    let responses = [
        hook_definitions_fixture(false),
        serde_json::json!({"data":{"digest":digest}}),
        hook_definitions_fixture(true),
        serde_json::json!({"data":{"digest":digest,"revoked":true}}),
        hook_definitions_fixture(false),
    ];
    let server = std::thread::spawn(move || {
        let mut requests = Vec::new();
        for response in responses {
            let (mut socket, _) = listener.accept().unwrap();
            socket
                .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                .unwrap();
            let mut bytes = Vec::new();
            let mut buffer = [0; 2048];
            loop {
                let n = socket.read(&mut buffer).unwrap();
                assert!(n > 0);
                bytes.extend_from_slice(&buffer[..n]);
                if let Some(end) = bytes.windows(4).position(|v| v == b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&bytes[..end]);
                    let length = headers
                        .lines()
                        .find_map(|line| {
                            line.to_lowercase()
                                .strip_prefix("content-length:")
                                .map(|v| v.trim().parse::<usize>().unwrap())
                        })
                        .unwrap_or(0);
                    if bytes.len() >= end + 4 + length {
                        break;
                    }
                }
            }
            requests.push(String::from_utf8(bytes).unwrap());
            let body = response.to_string();
            write!(socket,"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).unwrap();
        }
        requests
    });
    let mut app = App::new(session(false), Vec::new(), "cyber");
    app.session.directory = "/repo space".into();
    let generation = open_hook_definitions(&mut app);
    let msg = crate::perform::perform(
        &client,
        &app.session,
        Action::LoadHookDefinitions { generation },
    )
    .await
    .unwrap();
    let crate::perform::Msg::HookDefinitions { result, .. } = msg else {
        panic!("expected catalog")
    };
    app.hook_definitions.apply(generation, result);
    for (control, approve) in [('t', true), ('u', false)] {
        app.on_key(key(KeyCode::Char(control)));
        let action = app.on_key(key(KeyCode::Char('y'))).pop().unwrap();
        assert!(matches!(&action, Action::ChangeHookTrust { approve:value,.. } if *value==approve));
        let msg = crate::perform::perform(&client, &app.session, action)
            .await
            .unwrap();
        let crate::perform::Msg::HookDefinitions {
            generation,
            result,
            directory,
            ..
        } = msg
        else {
            panic!("expected refreshed catalog")
        };
        assert_eq!(directory, "/repo space");
        app.hook_definitions.apply(generation, result);
    }
    let requests = server.join().unwrap();
    for request in &requests {
        assert!(
            request
                .to_lowercase()
                .contains("authorization: basic y3lizxi6chc=")
        );
        assert!(
            request
                .to_lowercase()
                .contains("x-cyber-directory: /repo%20space")
        );
    }
    assert!(requests[0].starts_with("GET /api/v1/hooks "));
    assert!(requests[1].starts_with("POST /api/v1/hooks/trust "));
    assert!(requests[3].starts_with("POST /api/v1/hooks/untrust "));
    for i in [1, 3] {
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(
                requests[i].split("\r\n\r\n").nth(1).unwrap()
            )
            .unwrap(),
            serde_json::json!({"digest":digest})
        );
    }
}

#[test]
fn hook_definitions_selection_is_frozen_during_exact_digest_confirmation() {
    let mut value = hook_definitions_fixture(false);
    let mut second = value["data"]["hooks"][0].clone();
    second["digest"] = serde_json::json!(format!("sha256:{}", "b".repeat(64)));
    second["source"] = serde_json::json!("/repo/.cyber/local.jsonc");
    value["data"]["hooks"].as_array_mut().unwrap().push(second);
    let mut app = App::new(session(false), Vec::new(), "cyber");
    let generation = open_hook_definitions(&mut app);
    app.hook_definitions
        .apply(generation, crate::hook_definitions::Catalog::parse(&value));
    app.on_key(key(KeyCode::Down));
    assert!(
        app.hook_definitions
            .lines()
            .join(" ")
            .contains("Definition 2/2")
    );
    app.on_key(key(KeyCode::PageDown));
    assert!(app.hook_definitions.scroll > 0);
    app.on_key(key(KeyCode::Char('t')));
    app.on_key(key(KeyCode::Up));
    let action = app.on_key(key(KeyCode::Char('y')));
    let [
        Action::ChangeHookTrust {
            generation,
            digest,
            approve: true,
        },
    ] = action.as_slice()
    else {
        panic!("expected selected digest approval")
    };
    assert_eq!(digest, &format!("sha256:{}", "b".repeat(64)));
    app.hook_definitions
        .apply(*generation, Err("Digest no longer current".into()));
    assert!(
        app.hook_definitions
            .lines()
            .join(" ")
            .contains("Digest no longer current")
    );
    app.on_key(key(KeyCode::Char('r')));
    assert!(
        app.hook_definitions
            .lines()
            .join(" ")
            .contains("Loading current")
    );
}

#[test]
fn hook_definitions_worktree_rebound_event_clears_review_and_pending_approval() {
    let mut app = App::new(session(false), Vec::new(), "cyber");
    let generation = open_hook_definitions(&mut app);
    app.hook_definitions.apply(
        generation,
        crate::hook_definitions::Catalog::parse(&hook_definitions_fixture(false)),
    );
    app.on_key(key(KeyCode::Char('t')));
    let actions = app.on_event(
        "session.worktree.rebound.1",
        &serde_json::json!({"to":{"path":"/new checkout"}}),
    );
    assert!(matches!(actions.as_slice(), [Action::Refresh]));
    assert_eq!(app.session.directory, "/new checkout");
    assert!(matches!(app.overlay, Overlay::None));
    assert!(app.hook_definitions.change().is_none());
    app.hook_definitions.apply(
        generation,
        crate::hook_definitions::Catalog::parse(&hook_definitions_fixture(true)),
    );
    assert!(
        !app.hook_definitions
            .lines()
            .join(" ")
            .contains("Trusted: true")
    );
}

#[test]
fn hook_definitions_show_withheld_raw_sections_without_activation_or_approval() {
    let mut value = hook_definitions_fixture(false);
    value["data"]["hooks"] = serde_json::json!([]);
    value["data"]["checkout_trusted"] = serde_json::json!(false);
    value["data"]["withheld_hooks"] = serde_json::json!([{
        "scope":"local","source":"/repo/.cyber/cyber.local.jsonc","pointer":"/profiles/review~1a/hooks",
        "value":{"InvalidEvent":{"command":"{env:SECRET}\u{1b}","headers":{"Authorization":"private-token"}}}
    }]);
    let mut app = App::new(session(false), Vec::new(), "cyber");
    let generation = open_hook_definitions(&mut app);
    app.hook_definitions
        .apply(generation, crate::hook_definitions::Catalog::parse(&value));
    let lines = app.hook_definitions.lines().join(" ");
    assert!(lines.contains("literal/inactive") && lines.contains("{env:SECRET}"));
    assert!(!lines.contains("private-token") && !lines.contains('\u{1b}'));
    assert!(lines.contains("review~1a"));
    assert!(!format!("{:?}", app.hook_definitions).contains("private-token"));
    app.on_key(key(KeyCode::Char('t')));
    assert!(app.on_key(key(KeyCode::Char('y'))).is_empty());
}

#[test]
fn hook_definitions_last_run_distinguishes_unknown_and_running_without_raw_io() {
    for (status, label) in [
        ("unknown", "recovery required"),
        ("running", "live state unverified"),
        ("completed", "completed"),
    ] {
        let mut value = hook_definitions_fixture(false);
        value["data"]["hooks"][0]["last_run"] = serde_json::json!({"id":"hke_latest","session_id":"ses_previous","directory":"/repo\u{1b}","started_ms":42,"duration_ms":1,"status":status,"outcome":"error","acknowledged":false,"must_stop":true,"io":{"stdout":"private output"},"decision":{"reason":"private reason"}});
        let mut app = App::new(session(false), Vec::new(), "cyber");
        let generation = open_hook_definitions(&mut app);
        app.hook_definitions
            .apply(generation, crate::hook_definitions::Catalog::parse(&value));
        let lines = app.hook_definitions.lines().join(" ");
        assert!(lines.contains(label) && lines.contains("hke_latest"));
        assert!(lines.contains("must stop: true"));
        assert!(!lines.contains("private") && !lines.contains('\u{1b}'));
        assert!(!format!("{:?}", app.hook_definitions).contains("private"));
    }
}

fn open_memory(app: &mut App, global: bool) -> crate::memory::Request {
    typed(app, if global { "/memory global" } else { "/memory" });
    memory_request(app.on_key(key(KeyCode::Enter)))
}

fn memory_panel_catalog() -> (App, crate::memory::Request) {
    let mut app = App::new(session(false), Vec::new(), "cyber");
    let request = open_memory(&mut app, true);
    app.memory.apply(
        &request,
        Ok(crate::memory::Data::Catalog(
            cyber_core::memory::MemoryCatalog {
                memories: vec![memory_panel_note().metadata],
                invalid: vec![cyber_core::memory::InvalidMemory {
                    filename: "bad.md".into(),
                    diagnostic: "bad\u{1b}[31m".into(),
                }],
            },
        )),
    );
    (app, request)
}
fn memory_panel_note() -> cyber_core::memory::MemoryDocument {
    cyber_core::memory::MemoryDocument {
        metadata: cyber_core::memory::MemoryMetadata {
            name: "policy".into(),
            description: "Policy".into(),
            kind: cyber_core::memory::MemoryType::Reference,
        },
        body: "Private fact\n\u{1b}[31m terminal control".into(),
    }
}

#[test]
fn memory_panel_reads_selected_notes_and_sanitizes_control_text() {
    use crate::memory::{Data, Operation, Scope};
    let (mut app, request) = memory_panel_catalog();
    assert_eq!(request.scope, Scope::Global);
    assert_eq!(request.directory, "/repo");
    assert!(screen(&app).contains("policy"));
    assert!(app.on_key(key(KeyCode::Char('d'))).is_empty());
    assert!(app.on_key(key(KeyCode::Char('y'))).is_empty());
    let read = memory_request(app.on_key(key(KeyCode::Enter)));
    assert!(matches!(read.operation, Operation::Read(_)));
    app.memory.apply(&read, Ok(Data::Note(memory_panel_note())));
    assert!(screen(&app).contains("Private fact"));
    assert!(!app.memory.lines().join("\n").contains('\u{1b}'));
}

#[test]
fn memory_panel_deletion_requires_confirmation_and_dismissal_fences_late_responses() {
    use crate::memory::{Data, Operation};
    let (mut app, _) = memory_panel_catalog();
    let read = memory_request(app.on_key(key(KeyCode::Enter)));
    app.memory.apply(&read, Ok(Data::Note(memory_panel_note())));
    app.on_key(key(KeyCode::Char('d')));
    assert!(screen(&app).contains("Delete policy from global memory?"));
    assert!(app.on_key(key(KeyCode::Char('g'))).is_empty());
    app.on_key(key(KeyCode::Esc));
    assert!(matches!(app.overlay, Overlay::Memory));
    app.on_key(key(KeyCode::Char('d')));
    let delete = memory_request(app.on_key(key(KeyCode::Char('y'))));
    assert!(
        matches!(&delete.operation, Operation::Delete { name, key } if name == "policy" && !key.is_empty())
    );
    assert!(app.on_key(key(KeyCode::Char('y'))).is_empty());
    assert!(app.on_key(key(KeyCode::Char('g'))).is_empty());
    app.on_key(key(KeyCode::Esc));
    assert!(matches!(app.overlay, Overlay::None));
    app.memory
        .apply(&read, Ok(Data::Catalog(Default::default())));
    assert!(!app.memory.lines().join("\n").contains("Private fact"));
}

#[test]
fn memory_scope_refresh_and_location_changes_drop_old_reviews() {
    use crate::memory::{Data, Scope};
    let mut app = App::new(session(false), Vec::new(), "cyber");
    let original = open_memory(&mut app, false);
    app.memory
        .apply(&original, Ok(Data::Catalog(Default::default())));
    let actions = app.on_key(key(KeyCode::Char('g')));
    let [Action::Memory(global)] = actions.as_slice() else {
        panic!("scope")
    };
    assert_eq!(global.scope, Scope::Global);
    app.memory
        .apply(&original, Ok(Data::Catalog(Default::default())));
    assert!(screen(&app).contains("Loading"));
    app.memory.apply(global, Err("Unavailable".into()));
    assert!(!screen(&app).contains("Loading"));
    let mut changed = app.session.clone();
    changed.directory = "/other".into();
    app.set_session(changed);
    assert!(matches!(app.overlay, Overlay::None));
    assert!(!app.memory.lines().join("\n").contains("Unavailable"));
}

#[cfg(unix)]
#[tokio::test]
async fn memory_panel_recovery_and_deletion_use_real_api_ownership_and_retained_receipts() {
    use cyber_core::paths::{DatabaseLocation, Paths};
    use cyber_server::runtime::{MemoryAdmission, MemoryWrite};
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().canonicalize().unwrap();
    let paths = Paths {
        data: directory.join("data"),
        config: directory.join("config"),
        state: directory.join("state"),
        cache: directory.join("cache"),
        tmp: directory.join("tmp"),
    };
    paths.ensure().unwrap();
    let host = cyber_app::App::build(cyber_app::AppOptions {
        paths: paths.clone(),
        home: directory.join("home"),
        database: DatabaseLocation::File(directory.join("events.db")),
        default_directory: directory.clone(),
        sandbox_policy: None,
        snapshots: false,
        interactive: true,
        password: Some("tui-memory-password".into()),
        mcp_credentials: None,
        remote_skill_fetcher: None,
        workflow_cores: None,
        isolation: None,
    })
    .await
    .unwrap();
    let content = "---\nname: policy\ndescription: Policy\ntype: reference\n---\nProposed fact\n";
    let memory = cyber_core::memory::MemoryStore::open(&paths.data, "global").unwrap();
    {
        let mut scope = memory.claim().unwrap();
        let MemoryAdmission::Owned(mut owner) = host
            .runtime
            .admit_memory_write(MemoryWrite {
                directory: &directory,
                project_id: "global",
                name: "policy",
                deleted: false,
                identity: Some("tui-review-write"),
                content,
                http_hash: None,
            })
            .unwrap()
        else {
            panic!("owner")
        };
        let prepared = scope.prepare_write(content).unwrap();
        owner
            .bind_journal(prepared.journal_identity().unwrap())
            .unwrap();
        drop(prepared);
        drop(owner);
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let client = cyber_client::Client::http(
        &format!("http://{}", listener.local_addr().unwrap()),
        Some("tui-memory-password".into()),
    );
    let (stop, stopped) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(cyber_server::http::serve_tcp(
        cyber_server::http::router(host.state.clone()),
        listener,
        async {
            let _ = stopped.await;
        },
    ));
    let mut ui_session = session(false);
    ui_session.directory = directory.display().to_string();
    let mut app = App::new(ui_session, Vec::new(), "cyber");
    let mut persistence =
        crate::memory::persistence::Persistence::load(&paths.state, &mut app.memory);
    let request = open_memory(&mut app, true);
    apply_memory_response(&mut app, crate::memory::perform(&client, request).await);
    assert!(screen(&app).contains("Unavailable"));
    let action = app.on_key(key(KeyCode::Char('v'))).remove(0);
    perform_memory_action(&client, &mut app, action, &mut persistence).await;
    assert!(screen(&app).contains("Proposed fact"));
    app.on_key(key(KeyCode::Char('c')));
    assert!(screen(&app).contains("Apply this reviewed recovery?"));
    assert!(app.on_key(key(KeyCode::Enter)).is_empty());
    let action = app.on_key(key(KeyCode::Char('y'))).remove(0);
    perform_memory_action(&client, &mut app, action, &mut persistence).await;
    assert!(screen(&app).contains("Recovered policy"));
    let action = app.on_key(key(KeyCode::Char('k'))).remove(0);
    perform_memory_action(&client, &mut app, action, &mut persistence).await;
    assert!(screen(&app).contains("Acknowledged receipt"));
    let action = app.on_key(key(KeyCode::Char('r'))).remove(0);
    perform_memory_action(&client, &mut app, action, &mut persistence).await;
    let action = app.on_key(key(KeyCode::Enter)).remove(0);
    perform_memory_action(&client, &mut app, action, &mut persistence).await;
    assert!(screen(&app).contains("Proposed fact"));
    let action = app.on_key(key(KeyCode::Char('e'))).remove(0);
    perform_memory_action(&client, &mut app, action, &mut persistence).await;
    app.on_paste("Edited Unicode 🦀 fact");
    app.on_key(memory_control('s'));
    app.on_key(key(KeyCode::Char('n')));
    assert_eq!(
        memory.claim().unwrap().read("policy").unwrap().body,
        "Proposed fact"
    );
    app.on_key(memory_control('s'));
    let action = app.on_key(key(KeyCode::Char('y'))).remove(0);
    discard_memory_ack(&client, &mut app, action, &mut persistence).await;
    assert!(screen(&app).contains("Unavailable"));
    drop(persistence);
    app = App::new(app.session.clone(), Vec::new(), "cyber");
    persistence = crate::memory::persistence::Persistence::load(&paths.state, &mut app.memory);
    let request = open_memory(&mut app, true);
    apply_memory_response(&mut app, crate::memory::perform(&client, request).await);
    let action = app.on_key(key(KeyCode::Char('k'))).remove(0);
    perform_memory_action(&client, &mut app, action, &mut persistence).await;
    assert!(
        screen(&app).contains("Acknowledged save of policy"),
        "{}",
        app.memory.lines().join("\n")
    );
    let checkpoint: serde_json::Value = serde_json::from_slice(
        &std::fs::read(paths.state.join("memory-client/state.json")).unwrap(),
    )
    .unwrap();
    assert!(checkpoint["draft"].is_null());
    assert_eq!(checkpoint["intents"].as_array().unwrap().len(), 1);
    assert!(
        memory
            .claim()
            .unwrap()
            .read("policy")
            .unwrap()
            .body
            .contains("Edited Unicode 🦀 fact")
    );
    let action = app.on_key(key(KeyCode::Char('r'))).remove(0);
    perform_memory_action(&client, &mut app, action, &mut persistence).await;
    let action = app.on_key(key(KeyCode::Enter)).remove(0);
    perform_memory_action(&client, &mut app, action, &mut persistence).await;
    let action = app.on_key(key(KeyCode::Char('e'))).remove(0);
    perform_memory_action(&client, &mut app, action, &mut persistence).await;
    app.on_paste("Stale local draft");
    memory
        .claim()
        .unwrap()
        .write(
            "---\nname: policy\ndescription: Policy\ntype: reference\n---\n\nExternal user edit\n",
        )
        .unwrap();
    app.on_key(memory_control('s'));
    let action = app.on_key(key(KeyCode::Char('y'))).remove(0);
    perform_memory_action(&client, &mut app, action, &mut persistence).await;
    assert_eq!(
        memory.claim().unwrap().read("policy").unwrap().body,
        "External user edit"
    );
    assert!(app.memory.lines().join("\n").contains("Stale local draft"));
    app.on_key(key(KeyCode::Esc));
    app.on_key(key(KeyCode::Char('x')));
    app.on_key(key(KeyCode::Char('y')));
    let action = app.on_key(key(KeyCode::Char('r'))).remove(0);
    perform_memory_action(&client, &mut app, action, &mut persistence).await;
    let action = app.on_key(key(KeyCode::Enter)).remove(0);
    perform_memory_action(&client, &mut app, action, &mut persistence).await;
    app.on_key(key(KeyCode::Char('d')));
    app.on_key(key(KeyCode::Char('n')));
    assert!(memory.claim().unwrap().read("policy").is_ok());
    app.on_key(key(KeyCode::Char('d')));
    let action = app.on_key(key(KeyCode::Char('y'))).remove(0);
    discard_memory_ack(&client, &mut app, action, &mut persistence).await;
    drop(persistence);
    app = App::new(app.session.clone(), Vec::new(), "cyber");
    persistence = crate::memory::persistence::Persistence::load(&paths.state, &mut app.memory);
    let request = open_memory(&mut app, true);
    apply_memory_response(&mut app, crate::memory::perform(&client, request).await);
    let action = app.on_key(key(KeyCode::Char('k'))).remove(0);
    perform_memory_action(&client, &mut app, action, &mut persistence).await;
    assert!(screen(&app).contains("Acknowledged deletion of policy"));
    assert!(memory.claim().unwrap().read("policy").is_err());
    stop.send(()).unwrap();
    server.await.unwrap().unwrap();
    host.runtime.shutdown().await;
}

#[cfg(unix)]
fn apply_memory_response(app: &mut App, message: crate::perform::Msg) {
    let crate::perform::Msg::Memory { request, result } = message else {
        panic!("memory reply")
    };
    app.memory.apply(&request, result);
}

fn memory_request(actions: Vec<Action>) -> crate::memory::Request {
    let Action::Memory(request) = actions.into_iter().next().unwrap() else {
        panic!("memory action")
    };
    request
}
#[cfg(unix)]
async fn perform_memory_action(
    client: &cyber_client::Client,
    app: &mut App,
    action: Action,
    persistence: &mut crate::memory::persistence::Persistence,
) {
    let action = persistence.gate(app, vec![action]).remove(0);
    let message = crate::perform::perform(client, &app.session, action)
        .await
        .unwrap();
    apply_memory_response(app, message);
    assert!(persistence.gate(app, Vec::new()).is_empty());
}

#[cfg(unix)]
async fn discard_memory_ack(
    client: &cyber_client::Client,
    app: &mut App,
    action: Action,
    persistence: &mut crate::memory::persistence::Persistence,
) {
    let action = persistence.gate(app, vec![action]).remove(0);
    let message = crate::perform::perform(client, &app.session, action)
        .await
        .unwrap();
    let crate::perform::Msg::Memory { request, result } = message else {
        panic!("memory response");
    };
    assert!(matches!(result, Ok(crate::memory::Data::Change(_))));
    app.memory
        .apply(&request, Err("Simulated lost acknowledgement".into()));
    assert!(persistence.gate(app, Vec::new()).is_empty());
}

fn memory_control(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
}

fn memory_panel_edit() -> App {
    use crate::memory::Data;
    let (mut app, _) = memory_panel_catalog();
    let read = memory_request(app.on_key(key(KeyCode::Enter)));
    app.memory.apply(&read, Ok(Data::Note(memory_panel_note())));
    let review = memory_request(app.on_key(key(KeyCode::Char('e'))));
    app.memory.apply(
        &review,
        Ok(Data::EditReview(cyber_core::memory::MemoryEditReview {
            name: "policy".into(),
            original: Some(
                "---\nname: policy\ndescription: Policy\ntype: reference\n---\n\nFact\n".into(),
            ),
            fingerprint: "a".repeat(64),
        })),
    );
    app
}

#[test]
fn memory_editor_requires_explicit_confirmation_and_retains_failed_drafts() {
    use crate::memory::Operation;
    let mut app = memory_panel_edit();
    app.on_paste("Unicode 🦀\r\nupdated");
    assert!(app.on_key(memory_control('s')).is_empty());
    assert!(screen(&app).contains("Save policy"));
    assert!(app.on_key(key(KeyCode::Enter)).is_empty());
    assert!(app.on_key(key(KeyCode::Char('n'))).is_empty());
    assert!(app.memory.lines().join("\n").contains("updated"));
    app.on_key(memory_control('s'));
    let save = memory_request(app.on_key(key(KeyCode::Char('y'))));
    let Operation::Save(snapshot) = &save.operation else {
        panic!("save action")
    };
    assert!(snapshot.content.ends_with("Unicode 🦀\nupdated"));
    assert_eq!(snapshot.fingerprint, "a".repeat(64));
    app.on_paste("ignored while pending");
    app.memory.apply(&save, Err("stale review".into()));
    assert!(app.memory.lines().join("\n").contains("updated"));
    assert!(app.memory.lines().join("\n").contains(&snapshot.key));
    app.on_key(memory_control('s'));
    assert!(app.on_key(key(KeyCode::Char('y'))).is_empty());
}

#[test]
fn memory_editor_fresh_review_preserves_draft_and_requires_new_confirmation() {
    use crate::memory::Operation;
    let mut app = memory_panel_edit();
    app.on_paste("updated");
    app.on_key(memory_control('s'));
    let save = memory_request(app.on_key(key(KeyCode::Char('y'))));
    app.memory.apply(&save, Err("stale review".into()));
    let review = memory_request(app.on_key(memory_control('r')));
    app.memory.apply(
        &review,
        Ok(crate::memory::Data::EditReview(
            cyber_core::memory::MemoryEditReview {
                name: "policy".into(),
                original: Some("Current user fact".into()),
                fingerprint: "b".repeat(64),
            },
        )),
    );
    assert!(app.memory.lines().join("\n").contains("Current user fact"));
    assert!(app.memory.lines().join("\n").contains("updated"));
    app.on_key(memory_control('s'));
    let save = memory_request(app.on_key(key(KeyCode::Char('y'))));
    assert!(
        matches!(save.operation, Operation::Save(ref snapshot) if snapshot.fingerprint == "b".repeat(64))
    );
}

#[test]
fn memory_drafts_survive_dismissal_and_location_changes_without_foreign_save_authority() {
    let mut app = memory_panel_edit();
    app.on_paste("retained fact");
    app.on_key(key(KeyCode::Esc));
    app.on_key(key(KeyCode::Esc));
    assert!(matches!(app.overlay, Overlay::None));
    let request = open_memory(&mut app, true);
    app.memory.apply(
        &request,
        Ok(crate::memory::Data::Catalog(Default::default())),
    );
    assert!(app.on_key(key(KeyCode::Char('e'))).is_empty());
    assert!(app.memory.lines().join("\n").contains("retained fact"));
    let mut foreign = app.session.clone();
    foreign.directory = "/foreign".into();
    app.set_session(foreign);
    let request = open_memory(&mut app, true);
    app.memory.apply(
        &request,
        Ok(crate::memory::Data::Catalog(Default::default())),
    );
    app.on_key(key(KeyCode::Char('e')));
    assert!(app.on_key(memory_control('s')).is_empty());
    assert!(app.on_key(key(KeyCode::Char('y'))).is_empty());
    assert!(
        app.memory
            .lines()
            .join("\n")
            .contains("Draft retained for another")
    );
    app.on_key(key(KeyCode::Char('x')));
    app.on_key(key(KeyCode::Char('n')));
    assert!(
        app.memory
            .lines()
            .join("\n")
            .contains("Retained global draft")
    );
    app.on_key(key(KeyCode::Char('x')));
    app.on_key(key(KeyCode::Char('y')));
    assert!(
        !app.memory
            .lines()
            .join("\n")
            .contains("Retained global draft")
    );
}

#[test]
fn admissions_picker_reconciliation_is_explicit_and_does_not_repeat_busy_work() {
    let mut app = App::new(session(false), vec![], "cyber");
    let request = crate::admissions::Request {
        id: "op_reconcile".into(),
        source: "ses_1".into(),
        directory: "/repo".into(),
    };
    app.admissions.register(request.clone());
    typed(&mut app, "/admissions");
    app.on_key(key(KeyCode::Enter));
    let recover = with(KeyCode::Char('r'), KeyModifiers::CONTROL);
    assert!(app.on_key(recover).is_empty());
    let entry = app.admissions.0.get_mut(&request.id).unwrap();
    entry.status = "unknown".into();
    entry.busy = false;
    assert_eq!(
        app.on_key(recover),
        vec![Action::ReconcileAdmission(request.clone())]
    );
    app.admissions.0.get_mut(&request.id).unwrap().busy = true;
    assert!(app.on_key(recover).is_empty());
    app.admissions.0.get_mut(&request.id).unwrap().busy = false;
    assert!(screen(&app).contains("Ctrl+R reconcile"));
    let mut other = session(false);
    other.id = "ses_other".into();
    other.directory = "/other".into();
    app.set_session(other);
    assert_eq!(
        app.on_key(recover),
        vec![Action::ReconcileAdmission(request)]
    );
}

#[test]
fn review_validates_the_bundled_target_locally_and_routes_valid_targets() {
    let mut app = App::new(session(false), vec![], "cyber");
    typed(&mut app, "/review HEAD@{1}");
    app.completion = None;
    assert!(app.on_key(key(KeyCode::Enter)).is_empty());
    let toast = &app.toast.as_ref().unwrap().0;
    assert!(
        toast.contains("review") && toast.contains("HEAD@{1}"),
        "{toast}"
    );
    let delegation = submit_review(&mut app, "main..HEAD");
    assert!(delegation.starts_with("op_skill_"), "{delegation}");
    // An overriding `review` skill or command receives its raw arguments.
    app.review.set_bundled(false);
    typed(&mut app, "/review HEAD@{1}");
    app.completion = None;
    assert_eq!(
        app.on_key(key(KeyCode::Enter)),
        vec![Action::Command {
            name: "review".into(),
            arguments: "HEAD@{1}".into(),
            id: None,
        }]
    );
}

/// Submits a bundled `/review` and returns its reviewer delegation ID, derived
/// from the client-chosen message ID as the server derives it.
fn submit_review(app: &mut App, arguments: &str) -> String {
    let command = if arguments.is_empty() {
        "/review".to_string()
    } else {
        format!("/review {arguments}")
    };
    typed(app, &command);
    app.completion = None;
    let actions = app.on_key(key(KeyCode::Enter));
    let [
        Action::Command {
            name,
            arguments: posted,
            id: Some(message),
        },
    ] = actions.as_slice()
    else {
        panic!("expected one review command with an ID, got {actions:?}");
    };
    assert_eq!((name.as_str(), posted.as_str()), ("review", arguments));
    assert!(message.starts_with("msg_"), "{message}");
    cyber_server::runtime::Runtime::skill_command_delegation_id("ses_1", message)
}

/// The delegation lookups among an event's actions (a Job event also refreshes).
fn lookups(actions: Vec<Action>) -> Vec<Action> {
    actions
        .into_iter()
        .filter(|action| matches!(action, Action::ReviewDelegation { .. }))
        .collect()
}

fn delegation_record(id: &str, status: &str, job: Option<&str>) -> serde_json::Value {
    let phase = if job.is_some() {
        "launching"
    } else {
        "reserved"
    };
    json!({"id": id, "session_id": "ses_1", "status": status, "phase": phase,
        "job_id": job, "error": job.is_none().then_some("Permission denied: agent reviewer")})
}

/// Delivers `job.ended.1` for a Job while the review's Job is unknown and
/// answers the resulting delegation lookup with `review_job`.
fn end_job_before_lookup(
    app: &mut App,
    job: &serde_json::Value,
    delegation: &str,
    review_job: &str,
) {
    let lookup = Action::ReviewDelegation {
        session: "ses_1".into(),
        delegation: delegation.into(),
    };
    assert_eq!(lookups(app.on_event("job.ended.1", job)), vec![lookup]);
    app.review_delegation(&delegation_record(delegation, "admitted", Some(review_job)));
}

fn review_job(id: &str, status: &str, result: serde_json::Value) -> serde_json::Value {
    json!({"id": id, "session_id": "ses_1", "child_id": "ses_child", "name": "review",
        "status": status, "result": result, "error": null})
}

#[test]
fn completed_review_job_renders_findings_most_severe_first_with_controls_escaped() {
    let mut app = App::new(session(false), vec![], "cyber");
    let delegation = submit_review(&mut app, "");
    let finding = |file: &str, line: u64, severity: &str, summary: &str| {
        json!({"file": file, "line": line, "severity": severity, "summary": summary,
            "failure_scenario": "input -> wrong output"})
    };
    let result = json!({"text": "done", "result": {"findings": [
        finding("b.rs", 3, "low", "minor"),
        finding("z.rs", 9, "critical", "\u{1b}[31mred alert"),
        finding("a.rs", 20, "high", "second"),
        finding("a.rs", 4, "high", "first"),
    ], "limitations": [".env withheld by read policy"]}});
    end_job_before_lookup(
        &mut app,
        &review_job("job_1", "completed", result),
        &delegation,
        "job_1",
    );
    assert!(matches!(app.overlay, Overlay::Findings));
    let shown = screen(&app);
    let at = |needle: &str| {
        shown
            .find(needle)
            .unwrap_or_else(|| panic!("{needle} missing:\n{shown}"))
    };
    assert!(at("z.rs:9") < at("a.rs:4"));
    assert!(at("a.rs:4") < at("a.rs:20"));
    assert!(at("a.rs:20") < at("b.rs:3"));
    assert!(!shown.contains('\u{1b}'));
    assert!(shown.contains("\\u{1b}[31mred alert"), "{shown}");
    assert!(shown.contains(".env withheld"));
    app.on_key(key(KeyCode::Esc));
    assert_eq!(app.overlay, Overlay::None);
    // The same Job is not shown twice, for example after an event replay.
    let replay = lookups(app.on_event(
        "job.ended.1",
        &review_job(
            "job_1",
            "completed",
            json!({"result": {"findings": [], "limitations": []}}),
        ),
    ));
    assert!(replay.is_empty());
    assert_eq!(app.overlay, Overlay::None);
}

#[test]
fn failed_or_invalid_review_job_shows_the_job_error() {
    let mut app = App::new(session(false), vec![], "cyber");
    let delegation = submit_review(&mut app, "HEAD~1");
    let mut job = review_job("job_2", "error", serde_json::Value::Null);
    job["error"] = json!("SchemaMismatch: result does not match the schema");
    end_job_before_lookup(&mut app, &job, &delegation, "job_2");
    assert_eq!(app.overlay, Overlay::None);
    assert!(app.toast.as_ref().unwrap().0.contains("SchemaMismatch"));
    let delegation = submit_review(&mut app, "");
    let invalid = json!({"result": {"findings": [{"file": "/etc/passwd", "line": 1,
        "severity": "high", "summary": "s", "failure_scenario": "f"}], "limitations": []}});
    end_job_before_lookup(
        &mut app,
        &review_job("job_3", "completed", invalid),
        &delegation,
        "job_3",
    );
    assert_eq!(app.overlay, Overlay::None);
    assert!(
        app.toast
            .as_ref()
            .unwrap()
            .0
            .contains("Invalid review findings")
    );
}

/// Regression: any Job that ended while a review was pending was reported as
/// the review, and cleared it so the real reviewer's failure was dropped.
#[test]
fn unrelated_jobs_ending_during_a_review_are_not_the_review() {
    let mut app = App::new(session(false), vec![], "cyber");
    let findings = json!({"result": {"findings": [], "limitations": []}});
    // No review pending: even a findings-shaped result opens nothing.
    let ended = app.on_event(
        "job.ended.1",
        &review_job("job_0", "completed", findings.clone()),
    );
    assert!(lookups(ended).is_empty());
    assert_eq!(app.overlay, Overlay::None);
    let delegation = submit_review(&mut app, "");
    let mut explore = review_job("job_explore", "error", serde_json::Value::Null);
    explore["name"] = json!("explore");
    explore["error"] = json!("explore failed");
    end_job_before_lookup(&mut app, &explore, &delegation, "job_review");
    let other = review_job("job_other", "completed", json!({"result": {"answer": 42}}));
    assert!(lookups(app.on_event("job.ended.1", &other)).is_empty());
    assert!(app.toast.is_none());
    assert_eq!(app.overlay, Overlay::None);
    let mut reviewer = review_job("job_review", "error", serde_json::Value::Null);
    reviewer["error"] = json!("reviewer crashed");
    assert!(lookups(app.on_event("job.ended.1", &reviewer)).is_empty());
    let toast = &app.toast.as_ref().unwrap().0;
    assert!(toast.contains("review failed: reviewer crashed"), "{toast}");
    // A review Job that ends before its lookup completes is shown once known.
    app.toast = None;
    let delegation = submit_review(&mut app, "");
    end_job_before_lookup(
        &mut app,
        &review_job("job_fast", "completed", findings),
        &delegation,
        "job_fast",
    );
    assert!(matches!(app.overlay, Overlay::Findings));
}

/// Set by the reviewer's first request; the unrelated explore child waits for it.
static REVIEW_STARTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// Set once the explore Job's end reached the TUI; the reviewer waits for it.
static EXPLORE_SEEN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Wait (bounded) until `flag` is set; the scripted model answers on its own thread.
fn wait_flag(flag: &std::sync::atomic::AtomicBool) {
    for _ in 0..3000 {
        if flag.load(std::sync::atomic::Ordering::SeqCst) {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

/// The parent starts a background `explore` child; the reviewer reads `a.rs`, then returns
/// findings only after the explore Job's end reached the TUI, so that Job ends during the
/// review.
fn review_reply(body: &serde_json::Value) -> serde_json::Value {
    let text = body.to_string();
    let tool_result = text.contains("\"role\":\"tool\"");
    if text.contains("Write a short title") {
        return json!({"content":"Review title"});
    }
    if text.contains("review-target") {
        REVIEW_STARTED.store(true, std::sync::atomic::Ordering::SeqCst);
        if !tool_result {
            return json!({"tool_calls":[{"index":0,"id":"call_review_read","type":"function",
                "function":{"name":"read","arguments":"{\"path\":\"a.rs\"}"}}]});
        }
        wait_flag(&EXPLORE_SEEN);
        let result = json!({"findings":[{"file":"a.rs","line":1,"severity":"high",
            "summary":"REAL-APP-FINDING","failure_scenario":"changed line breaks callers"}],
            "limitations":[]});
        return json!({"tool_calls":[{"index":0,"id":"call_review_result","type":"function",
            "function":{"name":"return_result","arguments":result.to_string()}}]});
    }
    if text.contains("EXPLORE-TASK") && !text.contains("SPAWN-EXPLORE") {
        wait_flag(&REVIEW_STARTED);
        return json!({"content":"explored"});
    }
    if text.contains("SPAWN-EXPLORE") && !tool_result {
        let arguments = json!({"agent":"explore","prompt":"EXPLORE-TASK","background":true});
        return json!({"tool_calls":[{"index":0,"id":"call_spawn","type":"function",
            "function":{"name":"agent","arguments":arguments.to_string()}}]});
    }
    json!({"content":"noted"})
}

fn git_in(repo: &std::path::Path, args: &[&str]) {
    let status = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .status()
        .unwrap();
    assert!(status.success(), "git {args:?}");
}

/// `vcs-integration` → Local review command, through a production App over HTTP: the
/// TUI's bundled `/review` posts the command, follows the real event stream, looks up
/// the reviewer delegation and renders the reviewer Job's findings, while an unrelated
/// background Job of the same Session that ends during the review is ignored.
#[tokio::test]
async fn review_against_app_server_renders_findings_and_ignores_unrelated_jobs() {
    use crate::screens::Server;
    use futures::StreamExt;
    let mut extra = json!({"permissions":{"agent":"allow","read":"allow"}});
    if cfg!(windows) {
        // The review engine reads the checkout under the sandbox, which Windows cannot
        // enforce, so the opt-out must be explicit there.
        extra["sandbox"] = json!({"policy":"full-access"});
    }
    let server = Server::start_reply(review_reply, json!({"main":{}}), extra, false).await;
    let repo = server.root().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    git_in(&repo, &["init", "-q"]);
    git_in(&repo, &["config", "user.email", "t@example.com"]);
    git_in(&repo, &["config", "user.name", "T"]);
    std::fs::write(repo.join("a.rs"), "fn a() {}\n").unwrap();
    git_in(&repo, &["add", "a.rs"]);
    git_in(&repo, &["commit", "-q", "-m", "init"]);
    std::fs::write(repo.join("a.rs"), "fn a(x: u8) {}\n").unwrap();
    let client = server.client.at(&repo.display().to_string());
    let created = client
        .post("/sessions", json!({"model":"test/main","mode":"default"}))
        .await
        .unwrap();
    let id = created["data"]["id"].as_str().unwrap().to_owned();
    let mut events = Box::pin(client.events(false).await.unwrap());
    client
        .post(
            &format!("/sessions/{id}/prompt"),
            json!({"parts":[{"type":"text","text":"SPAWN-EXPLORE"}]}),
        )
        .await
        .unwrap();
    // The explore Job runs in the background; the parent Turn ends.
    let explore_job = tokio::time::timeout(std::time::Duration::from_secs(30), async {
        loop {
            let jobs = client.get(&format!("/jobs?session_id={id}")).await.unwrap();
            if let Some(job) = jobs["data"].as_array().and_then(|jobs| jobs.first()) {
                break job["id"].as_str().unwrap().to_owned();
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("explore Job");
    while server.host.runtime.is_running(&id) {
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    let ui = Session {
        id: id.clone(),
        directory: repo.display().to_string(),
        ..Session::default()
    };
    let mut app = App::new(ui, Vec::new(), "cyber");
    typed(&mut app, "/review");
    app.completion = None;
    let mut actions = app.on_key(key(KeyCode::Enter));
    assert!(
        matches!(actions.as_slice(), [Action::Command { name, id: Some(_), .. }] if name == "review"),
        "{actions:?}"
    );
    let mut explore_ended_during_review = false;
    tokio::time::timeout(std::time::Duration::from_secs(60), async {
        loop {
            while let Some(action) = actions.pop() {
                if matches!(
                    action,
                    Action::Command { .. } | Action::ReviewDelegation { .. }
                ) {
                    let msg = crate::perform::perform(&client, &app.session, action).await;
                    actions.extend(crate::runner::apply(&mut app, msg, &mut Default::default()));
                }
            }
            if matches!(app.overlay, Overlay::Findings) {
                return;
            }
            let event = events.next().await.expect("event stream");
            if event.kind == "job.ended.1" && event.data["id"] == explore_job.as_str() {
                assert!(!matches!(app.overlay, Overlay::Findings));
                explore_ended_during_review = true;
            }
            actions = app.on_event(&event.kind, &event.data);
            if explore_ended_during_review {
                EXPLORE_SEEN.store(true, std::sync::atomic::Ordering::SeqCst);
            }
        }
    })
    .await
    .expect("the review findings were never shown");
    assert!(explore_ended_during_review);
    let shown = screen(&app);
    assert!(shown.contains("REAL-APP-FINDING"), "{shown}");
    assert!(shown.contains("a.rs:1"), "{shown}");
    assert!(
        app.toast
            .as_ref()
            .is_none_or(|(toast, ..)| !toast.contains("review failed")),
        "{:?}",
        app.toast
    );
    server.host.runtime.shutdown().await;
}

/// The small model drafts commit and PR text, adding a trailer it must not keep.
fn git_text_reply(body: &serde_json::Value) -> serde_json::Value {
    let text = body.to_string();
    if text.contains("Draft a pull request") {
        return json!({"content":"Greet users\n\nAdds `greet`.\nCo-authored-by: M <m@example.com>"});
    }
    if text.contains("Conventional Commits") {
        return json!({"content":"feat: greet users\n\nCo-authored-by: M <m@example.com>"});
    }
    json!({"content":"ok"})
}

/// `vcs-integration` → Commit and PR text generation, through a production App over HTTP:
/// `/commit` prefills a reviewable `git commit` with the small model's message and no
/// co-author trailer; `/pr-description` prefills `gh pr create` for `base..HEAD`.
#[tokio::test]
async fn commit_and_pr_description_prefill_commands_from_the_app_server() {
    use crate::screens::Server;
    let server = Server::start_reply(git_text_reply, json!({"main":{}}), json!({}), false).await;
    let repo = server.root().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    git_in(&repo, &["init", "-q", "-b", "main"]);
    git_in(&repo, &["config", "user.email", "t@example.com"]);
    git_in(&repo, &["config", "user.name", "T"]);
    std::fs::write(repo.join("a.rs"), "fn a() {}\n").unwrap();
    git_in(&repo, &["add", "a.rs"]);
    git_in(&repo, &["commit", "-q", "-m", "init"]);
    git_in(&repo, &["checkout", "-q", "-b", "feature"]);
    std::fs::write(repo.join("a.rs"), "fn greet() {}\n").unwrap();
    git_in(&repo, &["commit", "-q", "-am", "feat: greet"]);
    std::fs::write(repo.join("b.rs"), "fn b() {}\n").unwrap();
    git_in(&repo, &["add", "b.rs"]);
    let client = server.client.at(&repo.display().to_string());
    let ui = Session {
        id: "ses_git".into(),
        directory: repo.display().to_string(),
        ..Session::default()
    };
    let mut app = App::new(ui, Vec::new(), "cyber");
    for (command, expected) in [
        ("/commit", "!git commit -m 'feat: greet users'"),
        (
            "/pr-description main",
            "!gh pr create --base 'main' --title 'Greet users' --body 'Adds `greet`.'",
        ),
    ] {
        typed(&mut app, command);
        app.completion = None;
        let actions = app.on_key(key(KeyCode::Enter));
        let [action @ Action::GitText(_)] = actions.as_slice() else {
            panic!("{command}: {actions:?}")
        };
        let msg = crate::perform::perform(&client, &app.session, action.clone()).await;
        crate::runner::apply(&mut app, msg, &mut Default::default());
        assert_eq!(app.composer.text(), expected);
        assert!(!app.composer.text().contains("Co-authored-by"));
        app.composer.set_text("");
    }
    server.host.runtime.shutdown().await;
}

/// Regression: a reviewer fork that failed before its Job existed left the
/// review pending forever.
#[test]
fn a_review_fork_that_failed_before_its_job_reports_the_delegation_error() {
    let mut app = App::new(session(false), vec![], "cyber");
    let delegation = submit_review(&mut app, "");
    let mut job = review_job("job_x", "completed", serde_json::Value::Null);
    job["name"] = json!("other");
    app.on_event("job.ended.1", &job);
    app.review_delegation(&delegation_record(&delegation, "failed", None));
    let toast = &app.toast.as_ref().unwrap().0;
    assert!(
        toast.contains("review failed: Permission denied: agent reviewer"),
        "{toast}"
    );
    app.toast = None;
    assert!(lookups(app.on_event("job.ended.1", &job)).is_empty());
    assert!(app.toast.is_none());
}

/// Regression: a reviewer fork refused before its Job existed (a delegation
/// admission denial) ends no Job; the server's handback admission must still
/// trigger the delegation lookup that reports the error.
#[test]
fn a_review_fork_handback_admission_looks_up_the_failed_delegation() {
    let mut app = App::new(session(false), vec![], "cyber");
    let delegation = submit_review(&mut app, "");
    let unrelated = json!({"message_id": "msg_skill_handback_op_skill_other", "parts": []});
    assert!(lookups(app.on_event("session.prompt.admitted.1", &unrelated)).is_empty());
    let handback = json!({"message_id": format!("msg_skill_handback_{delegation}"), "parts": []});
    let lookup = Action::ReviewDelegation {
        session: "ses_1".into(),
        delegation: delegation.clone(),
    };
    assert_eq!(
        lookups(app.on_event("session.prompt.admitted.1", &handback)),
        vec![lookup]
    );
    app.review_delegation(&delegation_record(&delegation, "failed", None));
    let toast = &app.toast.as_ref().unwrap().0;
    assert!(
        toast.contains("review failed: Permission denied: agent reviewer"),
        "{toast}"
    );
    app.toast = None;
    assert!(lookups(app.on_event("session.prompt.admitted.1", &handback)).is_empty());
}

/// A local HTTP server that answers one request with `status` and `body`
/// and returns the request it received.
fn one_response(
    status: &'static str,
    body: &'static str,
) -> (String, std::thread::JoinHandle<String>) {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let server = std::thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        socket
            .set_read_timeout(Some(std::time::Duration::from_secs(5)))
            .unwrap();
        let mut request = Vec::new();
        let mut bytes = [0; 4096];
        let complete = |request: &[u8]| {
            let text = String::from_utf8_lossy(request).to_ascii_lowercase();
            let Some((head, body)) = text.split_once("\r\n\r\n") else {
                return false;
            };
            let length = head
                .lines()
                .find_map(|line| line.strip_prefix("content-length:"))
                .map_or(0, |value| value.trim().parse::<usize>().unwrap());
            body.len() >= length
        };
        while !complete(&request) {
            let count = socket.read(&mut bytes).unwrap();
            assert!(count > 0);
            request.extend_from_slice(&bytes[..count]);
        }
        socket
            .write_all(
                format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
                .as_bytes(),
            )
            .unwrap();
        String::from_utf8(request).unwrap()
    });
    (base, server)
}

/// Regression: a review post failing with a status other than 403/422 left
/// the review pending, so later unrelated Job failures were shown as it.
#[tokio::test]
async fn a_failed_review_post_is_a_refusal() {
    let (base, server) = one_response(
        "503 Service Unavailable",
        r#"{"_tag":"UnavailableError","message":"Review helper unavailable"}"#,
    );
    let client = cyber_client::Client::http(&base, None);
    let mut app = App::new(session(false), vec![], "cyber");
    typed(&mut app, "/review");
    app.completion = None;
    let [action] = <[Action; 1]>::try_from(app.on_key(key(KeyCode::Enter))).unwrap();
    let message = crate::perform::perform(&client, &app.session, action)
        .await
        .unwrap();
    server.join().unwrap();
    let crate::perform::Msg::Refused { command, message } = message else {
        panic!("expected a refusal, got {message:?}");
    };
    app.refused(&command, &message);
    assert!(
        app.toast
            .as_ref()
            .unwrap()
            .0
            .contains("Review helper unavailable")
    );
    app.toast = None;
    let mut job = review_job("job_9", "error", serde_json::Value::Null);
    job["error"] = json!("unrelated");
    assert!(lookups(app.on_event("job.ended.1", &job)).is_empty());
    assert!(app.toast.is_none());
}

#[tokio::test]
async fn review_target_error_shows_the_server_message_without_a_task() {
    let (base, server) = one_response(
        "422 Unprocessable Entity",
        r#"{"_tag":"ReviewTargetError","code":"empty_diff","message":"No changes to review"}"#,
    );
    let client = cyber_client::Client::http(&base, None);
    let mut app = App::new(session(false), vec![], "cyber");
    typed(&mut app, "/review");
    app.completion = None;
    let actions = app.on_key(key(KeyCode::Enter));
    let [action] = <[Action; 1]>::try_from(actions).unwrap();
    let message = crate::perform::perform(&client, &app.session, action)
        .await
        .unwrap();
    let request = server.join().unwrap();
    assert!(
        request.starts_with("POST /api/v1/sessions/ses_1/command"),
        "{request}"
    );
    assert!(request.contains(r#""id":"msg_"#), "{request}");
    let crate::perform::Msg::Refused { command, message } = message else {
        panic!("expected a refusal, got {message:?}");
    };
    app.refused(&command, &message);
    let shown = screen(&app);
    assert!(shown.contains("No changes to review"), "{shown}");
    assert!(!shown.contains("ReviewTargetError"));
    assert!(app.items.is_empty());
    assert_eq!(app.overlay, Overlay::None);
    // No reviewer Job will end, so a later unrelated failure is not reported as the review.
    let mut job = review_job("job_9", "error", serde_json::Value::Null);
    job["error"] = json!("unrelated");
    app.toast = None;
    assert!(lookups(app.on_event("job.ended.1", &job)).is_empty());
    assert!(app.toast.is_none());
}

#[test]
fn hook_definitions_review_shows_managed_policy_skips() {
    let mut app = App::new(session(false), Vec::new(), "cyber");
    let generation = open_hook_definitions(&mut app);
    let mut catalog = hook_definitions_fixture(true);
    catalog["data"]["hooks"][0]["skipped"] = serde_json::json!("managed-only policy");
    app.hook_definitions.apply(
        generation,
        crate::hook_definitions::Catalog::parse(&catalog),
    );
    assert!(screen(&app).contains("Skipped by managed-only policy (never runs)"));
}

/// A changed project hook reaches the TUI from a real App as the review hint.
#[cfg(unix)]
#[tokio::test]
async fn hook_notice_event_shows_untrusted_review_hint() {
    use cyber_core::paths::{DatabaseLocation, Paths};
    use futures::StreamExt;
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().canonicalize().unwrap();
    let repo = directory.join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    let paths = Paths {
        data: directory.join("data"),
        config: directory.join("config"),
        state: directory.join("state"),
        cache: directory.join("cache"),
        tmp: directory.join("tmp"),
    };
    paths.ensure().unwrap();
    std::fs::write(
        paths.config.join("cyber.json"),
        json!({"providers":{"test":{"api":{"type":"openai-compatible","url":"http://127.0.0.1:9/v1","settings":{"auth":"none"}},"models":{"main":{}}}}})
            .to_string(),
    )
    .unwrap();
    let hook = json!({"type":"command","id":"lint","command":"cat >/dev/null"});
    std::fs::write(
        repo.join("cyber.jsonc"),
        json!({"hooks":{"UserPromptSubmit":[{"hooks":[hook]}]}}).to_string(),
    )
    .unwrap();
    // The checkout configuration is approved; the hook definition never was.
    let home = directory.join("home");
    let report = cyber_core::config::trust_report(&cyber_core::config::LoadRequest {
        location: &repo,
        paths: &paths,
        home: &home,
        env: &cyber_core::env::ProcessEnv,
        profile: None,
        overrides: &[],
        flags: Default::default(),
    })
    .unwrap();
    cyber_core::trust::TrustStore::new(paths.trust_file())
        .approve(&report.checkout_root, report.digest.as_deref().unwrap())
        .unwrap();
    let host = cyber_app::App::build(cyber_app::AppOptions {
        paths: paths.clone(),
        home,
        database: DatabaseLocation::File(directory.join("events.db")),
        default_directory: repo.clone(),
        sandbox_policy: None,
        snapshots: false,
        interactive: true,
        password: Some("tui-hook-password".into()),
        mcp_credentials: None,
        remote_skill_fetcher: None,
        workflow_cores: None,
        isolation: None,
    })
    .await
    .unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let client = cyber_client::Client::http(
        &format!("http://{}", listener.local_addr().unwrap()),
        Some("tui-hook-password".into()),
    )
    .at(&repo.display().to_string());
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let server = tokio::spawn(cyber_server::http::serve_tcp(
        cyber_server::http::router(host.state.clone()),
        listener,
        async {
            let _ = stopped.await;
        },
    ));
    let mut events = Box::pin(client.events(false).await.unwrap());
    let created = client
        .post("/sessions", json!({"model":"test/main"}))
        .await
        .unwrap();
    let id = created["data"]["id"].as_str().unwrap().to_owned();
    client
        .post(
            &format!("/sessions/{id}/prompt"),
            json!({"parts":[{"type":"text","text":"hello"}]}),
        )
        .await
        .unwrap();
    let notice = tokio::time::timeout(std::time::Duration::from_secs(20), async {
        loop {
            let event = events.next().await.expect("event stream");
            if event.kind == "session.hook.notice" {
                return event;
            }
        }
    })
    .await
    .expect("hook notice");
    let mut ui_session = session(false);
    ui_session.id = id;
    ui_session.directory = repo.display().to_string();
    let mut app = App::new(ui_session, Vec::new(), "cyber");
    app.on_event(&notice.kind, &notice.data);
    let toast = &app.toast.as_ref().unwrap().0;
    assert!(
        toast.contains("1 untrusted hook changed \u{2014} review with /hooks"),
        "{toast}"
    );
    assert!(
        screen(&app).contains("review with /hooks"),
        "{}",
        screen(&app)
    );
    let _ = stop.send(());
    server.await.unwrap().unwrap();
    host.runtime.shutdown().await;
}

/// `/memory readonly|off|on` from the TUI persist the toggle through a real App, and each
/// Session's next turn offers the matching memory tool and memory context.
#[tokio::test]
async fn memory_readonly_off_on_commands_change_tool_and_context() {
    use crate::screens::Server;
    let server = Server::start().await;
    let id = server.answered("first question").await;
    let mut app = server.load(&id).await;

    async fn toggle(server: &Server, app: &mut App, command: &str) -> String {
        typed(app, command);
        let actions = app.on_key(key(KeyCode::Enter));
        let [action @ Action::SetMemory(_)] = actions.as_slice() else {
            panic!("{actions:?}")
        };
        let msg = crate::perform::perform(&server.client, &app.session, action.clone()).await;
        let follow = crate::runner::apply(app, msg, &mut Default::default());
        assert!(follow.is_empty(), "{follow:?}");
        app.toast.as_ref().unwrap().0.clone()
    }
    async fn turn(server: &Server, id: &str, marker: &str) -> serde_json::Value {
        server.prompt(id, marker).await;
        for _ in 0..2000 {
            let request = server
                .model
                .requests()
                .into_iter()
                .rev()
                .find(|body| body["messages"].to_string().contains(marker));
            if let Some(request) = request
                && !server.host.runtime.is_running(id)
            {
                return request;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        panic!("no model request for {marker}")
    }
    fn memory_operations(request: &serde_json::Value) -> Option<serde_json::Value> {
        request["tools"]
            .as_array()
            .unwrap()
            .iter()
            .find_map(|tool| {
                (tool["function"]["name"] == "memory").then(|| {
                    tool["function"]["parameters"]["properties"]["operation"]["enum"].clone()
                })
            })
    }

    let before = turn(&server, &id, "second question").await;
    assert_eq!(
        memory_operations(&before),
        Some(json!(["list", "read", "write", "update", "delete"]))
    );
    assert!(before.to_string().contains("Save durable user preferences"));

    assert_eq!(
        toggle(&server, &mut app, "/memory readonly").await,
        "Memory read-only from the next turn"
    );
    let config: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(server.root().join("config/cyber.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(config["memory"], json!({"enabled":true,"generate":false}));
    assert!(config["providers"]["test"].is_object(), "{config}");
    let readonly = turn(&server, &id, "third question").await;
    assert_eq!(memory_operations(&readonly), Some(json!(["list", "read"])));
    let text = readonly["messages"].to_string();
    assert!(text.contains("Memory is read-only"), "{text}");

    assert_eq!(
        toggle(&server, &mut app, "/memory off").await,
        "Memory off from the next turn"
    );
    let off = turn(&server, &id, "fourth question").await;
    assert_eq!(memory_operations(&off), None);
    assert!(
        off["messages"]
            .to_string()
            .contains("core/memory context no longer applies")
    );

    assert_eq!(
        toggle(&server, &mut app, "/memory on").await,
        "Memory on from the next turn"
    );
    let on = turn(&server, &id, "fifth question").await;
    assert_eq!(
        memory_operations(&on),
        Some(json!(["list", "read", "write", "update", "delete"]))
    );
    let messages = on["messages"].to_string();
    let restored = messages.rfind("Save durable user preferences").unwrap();
    assert!(restored > messages.find("fourth question").unwrap());

    typed(&mut app, "/memory sometimes");
    assert!(app.on_key(key(KeyCode::Enter)).is_empty());
    assert!(
        app.toast
            .as_ref()
            .unwrap()
            .0
            .contains("/memory on|off|readonly")
    );
}
