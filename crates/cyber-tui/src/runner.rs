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
                if ev.session_id.as_deref() == Some(app.session.id.as_str()) { app.on_event(&ev.kind, &ev.data) } else { Vec::new() }
            }
            Some(msg) = rx.recv() => apply(app, msg, &mut refresh),
            _ = tick.tick() => Vec::new(),
        };
        dispatch(app, client, store, &tx, &mut refresh, terminal, actions);
    }
    Ok(())
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
        Msg::Snapshot {
            session,
            items,
            queued,
            requests,
        } => {
            refresh.in_flight = false;
            app.streaming.retain(|id, _| !items.iter().any(|i| i.id() == id && matches!(i, crate::model::Item::Assistant { text, .. } if !text.is_empty())));
            app.session = session;
            app.items = items;
            app.queued = queued;
            app.requests = requests;
            app.sync_overlay();
            if std::mem::take(&mut refresh.again) {
                return vec![Action::Refresh];
            }
        }
        Msg::Sessions(items) => app.open_picker(PickerKind::Sessions, "Sessions", items),
        Msg::Models(items) => app.open_picker(PickerKind::Models, "Models", items),
        Msg::Commands(items) => app.commands = items,
        Msg::Files(files) => app.files = files,
        Msg::Switched(session) => {
            app.session = session;
            app.items.clear();
            app.streaming.clear();
            return vec![Action::Refresh];
        }
        Msg::Toast(text) => app.toast(text),
        Msg::Done => {}
    }
    Vec::new()
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
                if action == Action::Refresh {
                    refresh.in_flight = true;
                }
                if let Action::Prompt { text, .. } = &action {
                    store.push_history(&app.session.directory, text);
                }
                if let Action::SwitchModel(model) = &action {
                    store.push_recent_model(model);
                }
                let (client, tx, session) = (client.clone(), tx.clone(), app.session.clone());
                tokio::spawn(async move {
                    let _ = tx
                        .send(perform::perform(&client, &session, action).await)
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
