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
                requests.push((header, body));
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
                head.starts_with("POST /api/v1/sessions/ses_1/subtask "),
                "{head}"
            );
            assert_eq!(body, &json!({"agent":target,"prompt":"find retry logic"}));
            assert!(matches!(result, Msg::Toast(_)));
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
