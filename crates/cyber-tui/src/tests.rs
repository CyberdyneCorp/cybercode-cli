//! Behavior of the state machine and what the screen shows, on an in-memory terminal.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use serde_json::json;

use crate::app::{Action, App, Overlay, PermStep};
use crate::model::{Item, QuestionSpec, Queued, Request, Session, Tool};

fn session(running: bool) -> Session {
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
            arguments: "v1.2".into()
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
        vec![Action::Refresh]
    );
    assert!(!app.session.running);
}

#[test]
fn queued_messages_are_listed_and_can_be_taken_back() {
    let mut app = App::new(session(true), Vec::new(), "cyber");
    app.queued = vec![Queued {
        message_id: "m5".into(),
        text: "then update docs".into(),
        delivery: "queue".into(),
    }];
    assert!(screen(&app).contains("then update docs"));
    let actions = app.on_key(with(KeyCode::Up, KeyModifiers::ALT));
    assert_eq!(actions, vec![Action::RemoveQueued("m5".into())]);
    assert_eq!(app.composer.text(), "then update docs");
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
    app.on_key(with(KeyCode::Char('x'), KeyModifiers::CONTROL));
    app.on_key(key(KeyCode::Char('t')));
    typed(&mut app, "nord");
    assert_eq!(
        app.on_key(key(KeyCode::Enter)),
        vec![Action::SaveTheme("nord")]
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
    assert!(screen(&app).contains("Esc to interrupt"));
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

fn apply_cost(app: &mut App, data: &serde_json::Value) {
    let result = crate::cost::Cost::parse_report(data, &app.session.id);
    app.cost.apply(&app.session.id, app.cost.generation, result);
}

#[test]
fn cost_command_refreshes_and_renders_all_classes_for_the_subtree() {
    let mut app = App::new(session(false), Vec::new(), "cyber");
    typed(&mut app, "/cost");
    assert_eq!(
        app.on_key(key(KeyCode::Enter)),
        vec![Action::LoadCost { generation: 1 }]
    );
    apply_cost(&mut app, &cost_fixture());
    let rendered = screen(&app);
    for text in [
        "Session and descendants",
        "Input",
        "Output",
        "Reasoning",
        "Cache read",
        "Cache write",
        "500",
        "100",
        "10",
        "200",
        "$1.0000",
        "41.7%",
    ] {
        assert!(rendered.contains(text), "missing {text}: {rendered}");
    }
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
    assert!(!rendered.contains("Total cost: $1.0000"));
}

#[test]
fn cost_refresh_replaces_the_snapshot_and_preserves_it_on_failure() {
    let mut app = App::new(session(false), Vec::new(), "cyber");
    typed(&mut app, "/cost");
    app.on_key(key(KeyCode::Enter));
    apply_cost(&mut app, &cost_fixture());
    assert_eq!(
        app.on_key(key(KeyCode::Char('r'))),
        vec![Action::LoadCost { generation: 2 }]
    );
    let mut next = cost_fixture();
    next["descendants"]["cost"] = json!(1.75);
    next["total"]["cost"] = json!(2.0);
    apply_cost(&mut app, &next);
    assert!(screen(&app).contains("Total cost: $2.0000"));
    app.on_key(key(KeyCode::Char('r')));
    next["descendants"]["tokens"]["input"] = json!(-1);
    apply_cost(&mut app, &next);
    let rendered = screen(&app);
    assert!(rendered.contains("Total cost: $2.0000"));
    assert!(rendered.contains("Refresh failed"));
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
    assert!(crate::cost::Cost::parse_report(&data, "ses_1").is_err());
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
    assert!(screen(&app).contains("Total cost: $1.0000"));
}

#[test]
fn cost_snapshot_refuses_foreign_missing_and_inconsistent_reports() {
    let mut rounded = cost_fixture();
    rounded["own"]["cost"] = json!(0.1);
    rounded["descendants"]["cost"] = json!(0.2);
    rounded["total"]["cost"] = json!(0.3);
    assert!(crate::cost::Cost::parse_report(&rounded, "ses_1").is_ok());

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
        assert!(crate::cost::Cost::parse_report(&report, "ses_1").is_err());
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
        crate::cost::Cost::parse_report(&cost_fixture(), "ses_1"),
    );
    assert!(!screen(&app).contains("Total cost: $1.0000"));
    apply_cost(&mut app, &cost_fixture());
    let mut stale = session(false);
    stale.cost = 99.99;
    app.set_session(stale);
    assert!(screen(&app).contains("Total cost: $1.0000"));
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
        crate::cost::Cost::parse_report(&cost_fixture(), "ses_1"),
    );
    assert!(!screen(&app).contains("Total cost: $1.0000"));
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
