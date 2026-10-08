//! Terminal setup, the event loop and teardown (`tui` → Launch and server transport,
//! Rendering modes and terminal integration).

use std::io::Write;
use std::time::Duration;

use crossterm::event::{Event as TermEvent, EventStream, KeyEventKind};
use crossterm::{event, execute, terminal};
use cyber_client::Client;
use futures::StreamExt;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use serde_json::{Value, json};
use tokio::sync::mpsc;

use crate::app::{Action, App, PickerKind};
use crate::model::Session;
use crate::perform::{self, Msg};
use crate::store::LocalStore;
use crate::{Start, Summary, TuiOptions};

type Term = Terminal<CrosstermBackend<std::io::Stdout>>;

pub async fn run(opts: TuiOptions) -> Result<Summary, String> {
    let client = opts.client.at(&opts.directory);
    let store = LocalStore::new(&opts.state_dir);
    let (session, open_picker) = open(&client, &opts).await?;
    let theme = store
        .get("theme")
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_else(|| "cyber".into());
    let mut app = App::new(session, store.history(&opts.directory), &theme);
    app.composer.set_text(&store.draft(&app.session.id));
    let mut terminal = setup().map_err(|e| e.to_string())?;
    let result = event_loop(&mut terminal, &mut app, &client, &store, &opts, open_picker).await;
    teardown(&mut terminal);
    store.save_draft(&app.session.id, &app.composer.text());
    result?;
    let s = &app.session;
    Ok(Summary {
        session_id: s.id.clone(),
        title: s.title.clone(),
        input_tokens: s.input_tokens,
        output_tokens: s.output_tokens,
        cost: s.cost,
    })
}

/// The Session to show first, and whether to open the picker over it.
async fn open(client: &Client, opts: &TuiOptions) -> Result<(Session, bool), String> {
    let existing = match &opts.start {
        Start::New => None,
        Start::Last | Start::Pick => first_session(client).await?,
        Start::Resume(wanted) => Some(find_session(client, wanted).await?),
    };
    let session = match existing {
        Some(id) if opts.fork => client
            .post(&format!("/sessions/{id}/fork"), json!({}))
            .await
            .map_err(|e| e.to_string())?["data"]
            .clone(),
        Some(id) => client
            .get(&format!("/sessions/{id}"))
            .await
            .map_err(|e| e.to_string())?["data"]
            .clone(),
        None => {
            let body = json!({ "model": opts.model, "agent": opts.agent, "mode": opts.mode });
            client
                .post("/sessions", body)
                .await
                .map_err(|e| e.to_string())?["data"]
                .clone()
        }
    };
    let mut session = Session::parse(&session);
    if let Some(model) = opts.model.as_ref().filter(|m| **m != session.model) {
        session = Session::parse(
            &client
                .post(
                    &format!("/sessions/{}/model", session.id),
                    json!({ "model": model }),
                )
                .await
                .map_err(|e| e.to_string())?["data"],
        );
    }
    Ok((session, opts.start == Start::Pick))
}

async fn first_session(client: &Client) -> Result<Option<String>, String> {
    let list = client
        .get("/sessions?limit=1")
        .await
        .map_err(|e| e.to_string())?;
    Ok(list["data"]["data"][0]["id"].as_str().map(str::to_string))
}

async fn find_session(client: &Client, wanted: &str) -> Result<String, String> {
    if client.get(&format!("/sessions/{wanted}")).await.is_ok() {
        return Ok(wanted.to_string());
    }
    let list = client
        .get("/sessions?limit=200")
        .await
        .map_err(|e| e.to_string())?;
    list["data"]["data"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|s| s["title"] == wanted)
        .and_then(|s| s["id"].as_str().map(str::to_string))
        .ok_or_else(|| format!("Session not found: {wanted}"))
}

fn setup() -> std::io::Result<Term> {
    terminal::enable_raw_mode()?;
    let mut out = std::io::stdout();
    execute!(
        out,
        terminal::EnterAlternateScreen,
        event::EnableBracketedPaste,
        event::EnableFocusChange
    )?;
    if terminal::supports_keyboard_enhancement().unwrap_or(false) {
        let _ = execute!(
            out,
            event::PushKeyboardEnhancementFlags(
                event::KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
            )
        );
    }
    Terminal::new(CrosstermBackend::new(out))
}

fn teardown(terminal: &mut Term) {
    let mut out = std::io::stdout();
    let _ = execute!(
        out,
        event::PopKeyboardEnhancementFlags,
        event::DisableBracketedPaste,
        event::DisableFocusChange,
        terminal::LeaveAlternateScreen
    );
    let _ = terminal::disable_raw_mode();
    let _ = terminal.show_cursor();
}

/// Refreshes are coalesced: one in flight, at most one more queued.
#[derive(Default)]
struct Refresh {
    in_flight: bool,
    again: bool,
    mode_in_flight: bool,
    queued_mode: Option<(String, String)>,
}

async fn event_loop(
    terminal: &mut Term,
    app: &mut App,
    client: &Client,
    store: &LocalStore,
    opts: &TuiOptions,
    open_picker: bool,
) -> Result<(), String> {
    let (tx, mut rx) = mpsc::channel::<Result<Msg, String>>(64);
    let mut keys = EventStream::new();
    let mut server = Box::pin(client.events(true).await.map_err(|e| e.to_string())?);
    let mut refresh = Refresh::default();
    let mut initial = vec![Action::Refresh];
    for request in store.admissions() {
        app.admissions.register(request.clone());
        initial.push(request.action(false));
    }
    if open_picker {
        initial.push(Action::LoadSessions);
    }
    if let Some(prompt) = &opts.prompt {
        initial.push(Action::Prompt {
            text: prompt.clone(),
            delivery: "steer",
        });
    }
    spawn_commands(client, &tx);
    dispatch(app, client, store, &tx, &mut refresh, terminal, initial);
    let mut tick = tokio::time::interval(Duration::from_millis(250));
    while !app.quit {
        set_title(app);
        terminal
            .draw(|f| crate::view::draw(f, app))
            .map_err(|e| e.to_string())?;
        let actions = tokio::select! {
            Some(Ok(ev)) = keys.next() => terminal_event(app, ev),
            Some(ev) = server.next() => {
                if event_is_visible(&app.session.id, &ev) { app.on_server_event(&ev) } else { Vec::new() }
            }
            Some(msg) = rx.recv() => {
                match msg {
                    Ok(Msg::AdmissionStarted(request)) => {app.admissions.register(request); Vec::new()}
                    Ok(Msg::AdmissionUpdated {request,result,stop}) => {
                        let message=app.admissions.update(request,result,stop,store);
                        app.refresh_admissions();
                        app.toast(message);
                        Vec::new()
                    }
                    other=>apply(app,other,&mut refresh),
                }
            },
            _ = tick.tick() => {let actions=app.admissions.poll(); app.refresh_admissions(); actions},
        };
        dispatch(app, client, store, &tx, &mut refresh, terminal, actions);
    }
    Ok(())
}

fn event_is_visible(session_id: &str, event: &cyber_client::Event) -> bool {
    if event.kind == "server.connected" || event.session_id.as_deref() == Some(session_id) {
        return true;
    }
    let request_event =
        event.kind.starts_with("permission.") || event.kind.starts_with("question.");
    request_event
        && event.data["routed_to"].as_array().is_some_and(|routes| {
            routes
                .iter()
                .any(|route| route["session_id"].as_str() == Some(session_id))
        })
}

fn terminal_event(app: &mut App, ev: TermEvent) -> Vec<Action> {
    match ev {
        TermEvent::Key(key) if key.kind != KeyEventKind::Release => app.on_key(key),
        TermEvent::Paste(text) => {
            app.on_paste(&text);
            Vec::new()
        }
        TermEvent::FocusGained => {
            app.focused = true;
            Vec::new()
        }
        TermEvent::FocusLost => {
            app.focused = false;
            Vec::new()
        }
        _ => Vec::new(),
    }
}

fn apply(app: &mut App, msg: Result<Msg, String>, refresh: &mut Refresh) -> Vec<Action> {
    let msg = match msg {
        Ok(msg) => msg,
        Err(e) => {
            refresh.in_flight = false;
            app.toast(e);
            return Vec::new();
        }
    };
    match msg {
        Msg::AdmissionStarted(_) | Msg::AdmissionUpdated { .. } => {}
        Msg::Snapshot {
            session,
            items,
            queued,
            requests,
        } => {
            refresh.in_flight = false;
            if session.id != app.session.id || session.seq < app.session.seq {
                return vec![Action::Refresh];
            }
            app.streaming.retain(|id, _| !items.iter().any(|i| i.id() == id && matches!(i, crate::model::Item::Assistant { text, .. } if !text.is_empty())));
            app.set_session(session);
            app.items = items;
            app.queued = queued;
            app.requests = requests;
            app.sync_overlay();
            if std::mem::take(&mut refresh.again) {
                return vec![Action::Refresh];
            }
        }
        Msg::ModeChanged { session_id, result } => {
            return mode_changed(app, refresh, session_id, result);
        }
        Msg::Cost {
            session_id,
            generation,
            result,
        } => {
            if session_id == app.session.id {
                app.cost.apply(&session_id, generation, result);
            }
        }
        Msg::Tasks { session_id, items } => {
            if session_id == app.session.id {
                app.open_picker(
                    PickerKind::Tasks,
                    "Tasks · Enter opens child · Ctrl+S stops",
                    items,
                );
            }
        }
        Msg::Sessions(items) => app.open_picker(PickerKind::Sessions, "Sessions", items),
        Msg::Models(items) => app.open_picker(PickerKind::Models, "Models", items),
        Msg::Commands(items) => app.commands = items,
        Msg::Files(files) => app.files = files,
        Msg::Agents { directory, items } => {
            if directory == app.session.directory {
                app.set_agents(items);
            }
        }
        Msg::Switched(session) => {
            app.set_session(session);
            app.items.clear();
            app.streaming.clear();
            return vec![Action::Refresh];
        }
        Msg::Toast(text) => app.toast(text),
        Msg::Done => {}
    }
    Vec::new()
}

fn mode_changed(
    app: &mut App,
    refresh: &mut Refresh,
    session_id: String,
    result: Result<crate::model::Session, String>,
) -> Vec<Action> {
    refresh.mode_in_flight = false;
    let queued = refresh.queued_mode.take();
    if session_id == app.session.id {
        match result {
            Ok(session) if session.seq >= app.session.seq => app.set_session(session),
            Ok(_) => {}
            Err(error) => {
                if queued.is_none() {
                    app.mode_selection = None;
                }
                app.toast(error);
            }
        }
    }
    match queued {
        Some((id, mode)) if id == app.session.id => vec![Action::SwitchMode(mode)],
        _ => vec![Action::Refresh],
    }
}

fn spawn_commands(client: &Client, tx: &mpsc::Sender<Result<Msg, String>>) {
    let (client, tx) = (client.clone(), tx.clone());
    tokio::spawn(async move {
        let _ = tx.send(perform::commands(&client).await).await;
    });
}

/// Run actions: local ones here, API calls on tasks that report back through `tx`.
fn dispatch(
    app: &mut App,
    client: &Client,
    store: &LocalStore,
    tx: &mpsc::Sender<Result<Msg, String>>,
    refresh: &mut Refresh,
    terminal: &mut Term,
    actions: Vec<Action>,
) {
    for action in actions {
        match action {
            Action::Quit => app.quit = true,
            Action::SaveTheme(name) => store.set("theme", Value::String(name.into())),
            Action::Editor(text) => {
                let edited = edit_externally(terminal, &text);
                app.composer.set_text(&edited);
            }
            Action::Refresh if refresh.in_flight => refresh.again = true,
            action => {
                if let Action::SwitchMode(mode) = &action {
                    if refresh.mode_in_flight {
                        refresh.queued_mode = Some((app.session.id.clone(), mode.clone()));
                        continue;
                    }
                    refresh.mode_in_flight = true;
                }
                if action == Action::Refresh {
                    refresh.in_flight = true;
                }
                if let Action::Prompt { text, .. } = &action {
                    store.push_history(&app.session.directory, text);
                }
                if let Action::SwitchModel(model) = &action {
                    store.push_recent_model(model);
                }
                if let Action::Admission { request, stop } = &action {
                    if *stop && let Err(error) = store.save_admission(request) {
                        app.toast(format!("Cannot retain cancellation identity: {error}"));
                        continue;
                    }
                    if let Some(entry) = app.admissions.0.get_mut(&request.id) {
                        entry.busy = true;
                        if *stop {
                            if entry.status == "unknown" {
                                entry.stop_at = Some(std::time::Instant::now());
                            } else {
                                entry.stop_at.get_or_insert_with(std::time::Instant::now);
                            }
                        }
                        entry.stopping |= stop;
                    }
                }
                let store = store.clone();
                let (client, tx, session) = (client.clone(), tx.clone(), app.session.clone());
                tokio::spawn(async move {
                    let _ = tx
                        .send(
                            perform::perform_owned(
                                &client,
                                &session,
                                action,
                                Some(&crate::admissions::Context {
                                    store: &store,
                                    tx: &tx,
                                }),
                            )
                            .await,
                        )
                        .await;
                });
            }
        }
    }
}

/// Open the draft in `$VISUAL`/`$EDITOR`, suspending the UI.
fn edit_externally(terminal: &mut Term, text: &str) -> String {
    let editor = std::env::var("VISUAL")
        .or_else(|_| std::env::var("EDITOR"))
        .unwrap_or_else(|_| "vi".into());
    let path = std::env::temp_dir().join(format!("cyber-prompt-{}.md", std::process::id()));
    if std::fs::write(&path, text).is_err() {
        return text.to_string();
    }
    teardown(terminal);
    let status = std::process::Command::new("sh")
        .arg("-c")
        .arg(format!("{editor} \"$1\""))
        .arg("sh")
        .arg(&path)
        .status();
    if let Ok(t) = setup() {
        *terminal = t;
    }
    let _ = terminal.clear();
    let edited = status
        .ok()
        .filter(|s| s.success())
        .and_then(|_| std::fs::read_to_string(&path).ok());
    let _ = std::fs::remove_file(&path);
    edited
        .map(|e| e.trim_end_matches('\n').to_string())
        .unwrap_or_else(|| text.to_string())
}

/// `cyber · <title> · <status>`.
fn set_title(app: &App) {
    let status = if app.session.running {
        "running"
    } else {
        "idle"
    };
    let mut out = std::io::stdout();
    let _ = execute!(
        out,
        terminal::SetTitle(format!("cyber · {} · {status}", app.session.title))
    );
    let _ = out.flush();
}

#[cfg(test)]
mod mode_tests {
    use super::*;

    #[test]
    fn routed_child_request_events_are_visible_without_forwarding_child_text() {
        let mut event = cyber_client::Event {
            kind: "permission.replied.1".into(),
            data: json!({ "routed_to": [{ "session_id": "ses_parent" }] }),
            seq: None,
            session_id: Some("ses_child".into()),
        };
        assert!(event_is_visible("ses_parent", &event));
        assert!(!event_is_visible("ses_other", &event));
        event.kind = "question.asked.1".into();
        assert!(event_is_visible("ses_parent", &event));
        event.kind = "session.text.delta".into();
        assert!(!event_is_visible("ses_parent", &event));
        assert!(event_is_visible("ses_child", &event));
        event.kind = "server.connected".into();
        event.session_id = None;
        assert!(event_is_visible("ses_parent", &event));
    }
    use crate::model::Session;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn app() -> App {
        App::new(
            Session {
                id: "current".into(),
                mode: "default".into(),
                seq: 0,
                ..Session::default()
            },
            Vec::new(),
            "cyber",
        )
    }

    #[test]
    fn latest_selection_follows_the_first_ack_and_rejection_restores_the_server_selection() {
        let mut app = app();
        let key = KeyEvent::new(KeyCode::BackTab, KeyModifiers::NONE);
        app.on_key(key);
        app.on_key(key);
        let mut refresh = Refresh {
            mode_in_flight: true,
            queued_mode: Some(("current".into(), "plan".into())),
            ..Refresh::default()
        };
        let accepted = Session {
            id: "current".into(),
            mode: "accept-edits".into(),
            seq: 1,
            ..Session::default()
        };
        assert_eq!(
            mode_changed(&mut app, &mut refresh, "current".into(), Ok(accepted)),
            vec![Action::SwitchMode("plan".into())]
        );
        assert!(app.mode_label().contains("plan (switching)"));
        mode_changed(
            &mut app,
            &mut refresh,
            "current".into(),
            Err("mode denied".into()),
        );
        assert_eq!(app.mode_label(), "accept-edits");
        assert_eq!(app.on_key(key), vec![Action::SwitchMode("plan".into())]);
    }

    #[test]
    fn old_session_results_cannot_restore_an_old_mode() {
        let mut app = app();
        app.session.mode = "auto".into();
        let mut refresh = Refresh {
            mode_in_flight: true,
            queued_mode: Some(("current".into(), "plan".into())),
            ..Refresh::default()
        };
        let old = Session {
            id: "old-session".into(),
            mode: "bypass".into(),
            seq: 50,
            ..Session::default()
        };
        assert_eq!(
            mode_changed(&mut app, &mut refresh, "old-session".into(), Ok(old)),
            vec![Action::SwitchMode("plan".into())]
        );
        assert_eq!(app.session.mode, "auto");
        assert_eq!(app.session.id, "current");
    }

    #[test]
    fn stale_refresh_does_not_overwrite_a_newer_mode_ack() {
        let mut app = app();
        app.session.seq = 10;
        app.session.mode = "auto".into();
        let mut refresh = Refresh::default();
        for (id, seq) in [("current", 9), ("old-session", 20)] {
            let snapshot = Msg::Snapshot {
                session: Session {
                    id: id.into(),
                    seq,
                    mode: "bypass".into(),
                    ..Session::default()
                },
                items: Vec::new(),
                queued: Vec::new(),
                requests: Vec::new(),
            };
            assert_eq!(
                apply(&mut app, Ok(snapshot), &mut refresh),
                vec![Action::Refresh]
            );
            assert_eq!(app.session.id, "current");
            assert_eq!(app.session.mode, "auto");
        }
    }
    #[test]
    fn a_pre_rebound_snapshot_cannot_restore_the_previous_directory() {
        let mut app = app();
        app.session.directory = "/repo/old".into();
        app.session.seq = 4;
        let event = cyber_client::Event {
            kind: "session.worktree.rebound.1".into(),
            data: json!({"to":{"path":"/repo/new"}}),
            seq: Some(8),
            session_id: Some(app.session.id.clone()),
        };
        app.on_server_event(&event);
        let snapshot = Msg::Snapshot {
            session: Session {
                id: app.session.id.clone(),
                seq: 7,
                directory: "/repo/old".into(),
                ..Session::default()
            },
            items: Vec::new(),
            queued: Vec::new(),
            requests: Vec::new(),
        };
        assert_eq!(
            apply(&mut app, Ok(snapshot), &mut Refresh::default()),
            vec![Action::Refresh]
        );
        assert_eq!(app.session.directory, "/repo/new");
        assert_eq!(app.session.seq, 8);
    }
    #[test]
    fn an_agent_catalogue_reply_from_the_previous_directory_is_ignored() {
        let mut app = app();
        app.session.directory = "/new".into();
        let message = Msg::Agents {
            directory: "/old".into(),
            items: vec![crate::model::Choice {
                key: "old-agent".into(),
                label: "old-agent".into(),
                detail: String::new(),
            }],
        };
        assert!(apply(&mut app, Ok(message), &mut Refresh::default()).is_empty());
        assert!(app.agents.is_empty());
    }
}
