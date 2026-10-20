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
use crate::model::{Choice, Session};
use crate::perform::{self, Msg};
use crate::store::LocalStore;
use crate::{Start, Summary, TuiOptions};

type Term = Terminal<CrosstermBackend<std::io::Stdout>>;

pub async fn run(opts: TuiOptions) -> Result<Summary, String> {
    let client = opts.client.at(&opts.directory);
    let store = LocalStore::new(&opts.state_dir);
    let env = |name: &str| std::env::var(name).ok();
    let (mut app, open_picker) = start(&client, &store, &opts, &env).await?;
    let mut terminal = setup().map_err(|e| e.to_string())?;
    let result = event_loop(&mut terminal, &mut app, &client, &store, &opts, open_picker).await;
    teardown(&mut terminal);
    store.save_draft(&app.session.id, &app.composer.text());
    result?;
    let s = &app.session;
    Ok(Summary {
        session_id: s.id.clone(),
        // Printed to the terminal after the UI closes.
        title: crate::sidebar::clean(&s.title),
        input_tokens: s.input_tokens,
        output_tokens: s.output_tokens,
        cost: s.cost,
    })
}

/// The first Session and the UI state for it, with the appearance from `tui.jsonc`,
/// custom themes, `kv.json` and the environment; and whether to open the picker.
pub(crate) async fn start(
    client: &Client,
    store: &LocalStore,
    opts: &TuiOptions,
    env: &dyn Fn(&str) -> Option<String>,
) -> Result<(App, bool), String> {
    let (session, open_picker) = open(client, opts).await?;
    let dirs = crate::prefs::config_dirs(&opts.config_dir, std::path::Path::new(&opts.directory));
    let plugins = client
        .get("/plugins")
        .await
        .map(|body| body["data"].clone())
        .unwrap_or_default();
    let plugin_themes = crate::plugins::theme_dirs(&plugins);
    let prefs = crate::prefs::load_with_plugins(store, &dirs, &plugin_themes, env);
    let mut app = App::new(session, store.history(&opts.directory), "cyber");
    app.reduced_motion = prefs.reduced_motion;
    app.screen_reader = prefs.screen_reader;
    app.keymap = prefs.keymap;
    if let Some(fields) = prefs.statusline_fields {
        app.statusline_fields = fields;
    }
    app.sidebar.preference = prefs.sidebar;
    app.timestamps = prefs.preferences.get("timestamps") == "on";
    app.show_reasoning = prefs.preferences.get("reasoning") == "expanded";
    app.prefs = prefs.preferences;
    app.welcome = prefs.welcome;
    app.explorer.icons = prefs.explorer_icons;
    app.notifications = prefs.notifications;
    app.home = env("HOME").or_else(|| env("USERPROFILE"));
    app.hyperlinks = crate::osc::hyperlinks(env);
    let warnings = prefs.themes.warnings.clone();
    app.set_appearance(prefs.themes, prefs.appearance);
    app.set_theme(&prefs.theme);
    if let Some(first) = warnings.first() {
        app.toast(first.clone());
    } else if let Some(offer) = crate::import_offer::offer(
        store,
        std::path::Path::new(&opts.directory),
        app.home.as_deref(),
    ) {
        app.toast(offer);
    } else if let Some(offer) = crate::plugins::offer(store, &opts.directory, &plugins) {
        app.toast(offer);
    }
    app.composer.set_text(&store.draft(&app.session.id));
    Ok((app, open_picker))
}

/// The Session to show first, and whether to open the picker over it.
async fn open(client: &Client, opts: &TuiOptions) -> Result<(Session, bool), String> {
    let existing = match &opts.start {
        Start::New => None,
        Start::Last | Start::Pick => first_session(client, "").await?,
        Start::PickPr(pr) => Some(
            first_session(client, &crate::perform::pr_filter(*pr))
                .await?
                .ok_or_else(|| format!("no Session is linked to pull request #{pr}"))?,
        ),
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
    Ok((
        session,
        matches!(opts.start, Start::Pick | Start::PickPr(_)),
    ))
}

/// The most recent Session matching `filter` (extra `&key=value` query terms).
async fn first_session(client: &Client, filter: &str) -> Result<Option<String>, String> {
    let list = client
        .get(&format!("/sessions?limit=1{filter}"))
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
pub(crate) struct Refresh {
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
    let mut memory_persistence =
        crate::memory::persistence::Persistence::load(&opts.state_dir, &mut app.memory);
    let (tx, mut rx) = mpsc::channel::<Result<Msg, String>>(64);
    let mut keys = EventStream::new();
    let mut server = Events {
        stream: client
            .events(true)
            .await
            .map_err(|e| e.to_string())?
            .boxed(),
        open: true,
        ticks: 0,
    };
    let mut team_events = crate::team::Events::default();
    let mut refresh = Refresh::default();
    let initial = initial_actions(app, store, opts, open_picker);
    spawn_commands(client, &tx);
    let initial = memory_persistence.gate(app, initial);
    dispatch(app, client, store, &tx, &mut refresh, terminal, initial);
    let mut tick = tokio::time::interval(Duration::from_millis(250));
    query_background(app);
    while !app.quit {
        set_title(app);
        app.progress_sync();
        let frame = terminal
            .draw(|f| crate::view::draw(f, app))
            .map_err(|e| e.to_string())?;
        let links = app.take_screen_links();
        if app.hyperlinks && matches!(app.overlay, crate::app::Overlay::None) {
            let _ = crate::osc::write_hyperlinks(&mut std::io::stdout(), frame.buffer, &links);
        }
        let actions = tokio::select! {
            Some(Ok(ev)) = keys.next() => terminal_event(app, ev),
            ev = server.next() => match ev {
                Some(ev) => server_event(app, &ev),
                None => {
                    app.sidebar.connection = crate::sidebar::Connection::Reconnecting;
                    Vec::new()
                }
            },
            Some(msg) = rx.recv() => received(app, store, msg, &mut refresh),
            Some(ev) = team_events.next() => app.team_event(&ev),
            () = frame_due(app.next_frame()) => {
                app.advance_frame();
                Vec::new()
            },
            _ = tick.tick() => on_tick(app, client, &mut server, &mut team_events).await,
        };
        let actions = memory_persistence.gate(app, actions);
        dispatch(app, client, store, &tx, &mut refresh, terminal, actions);
    }
    Ok(())
}

/// Every 250 ms: reopen closed event streams, follow the team, poll admissions and the
/// machine sample.
async fn on_tick(
    app: &mut App,
    client: &Client,
    server: &mut Events,
    team_events: &mut crate::team::Events,
) -> Vec<Action> {
    server.tick(app, client).await;
    team_events.sync(app, client).await;
    let mut actions = app.admissions.poll();
    app.refresh_admissions();
    actions.extend(app.machine_poll());
    actions
}

/// Wakes when the activity indicator's next frame is due; never without one.
async fn frame_due(at: Option<std::time::Instant>) {
    match at {
        Some(at) => tokio::time::sleep_until(at.into()).await,
        None => std::future::pending().await,
    }
}

/// The first refresh, stored admissions to reconcile, the picker and the `--prompt`.
fn initial_actions(
    app: &mut App,
    store: &LocalStore,
    opts: &TuiOptions,
    open_picker: bool,
) -> Vec<Action> {
    let mut initial = vec![Action::Refresh];
    for request in store.admissions() {
        app.admissions.register(request.clone());
        initial.push(request.action(false));
    }
    if open_picker {
        initial.push(match opts.start {
            Start::PickPr(pr) => Action::LoadPrSessions(pr),
            _ => Action::LoadSessions,
        });
    }
    if let Some(prompt) = &opts.prompt {
        initial.push(Action::Prompt {
            text: prompt.clone(),
            delivery: "steer",
        });
    }
    initial
}

fn received(
    app: &mut App,
    store: &LocalStore,
    msg: Result<Msg, String>,
    refresh: &mut Refresh,
) -> Vec<Action> {
    match msg {
        Ok(Msg::AdmissionStarted(request)) => {
            app.admissions.register(request);
            Vec::new()
        }
        Ok(Msg::AdmissionUpdated {
            request,
            result,
            stop,
        }) => {
            let message = app.admissions.update(request, result, stop, store);
            app.refresh_admissions();
            app.toast(message);
            Vec::new()
        }
        other => apply(app, other, refresh),
    }
}

/// Ticks (250 ms) between attempts to reopen a closed event stream.
const RECONNECT_TICKS: u32 = 8;

/// The instance event stream, reopened after it ends.
struct Events {
    stream: futures::stream::BoxStream<'static, cyber_client::Event>,
    open: bool,
    ticks: u32,
}

impl Events {
    /// The next event; `None` once when the stream ends, then nothing until it reopens.
    async fn next(&mut self) -> Option<cyber_client::Event> {
        if !self.open {
            return std::future::pending().await;
        }
        let event = self.stream.next().await;
        self.open = event.is_some();
        event
    }

    /// Try to reopen a closed stream every [`RECONNECT_TICKS`] ticks.
    async fn tick(&mut self, app: &mut App, client: &Client) {
        self.ticks = self.ticks.wrapping_add(1);
        if self.open || !self.ticks.is_multiple_of(RECONNECT_TICKS) {
            return;
        }
        if let Some(stream) = reopen_events(app, client).await {
            self.stream = stream;
            self.open = true;
        }
    }
}

/// Reopen the instance event stream. Its `server.connected` event marks the connection
/// up again and re-reads the Session and the sidebar; a failed attempt shows the
/// connection as down until the next one.
async fn reopen_events(
    app: &mut App,
    client: &Client,
) -> Option<futures::stream::BoxStream<'static, cyber_client::Event>> {
    let attempt = tokio::time::timeout(Duration::from_secs(2), client.events(true)).await;
    match attempt {
        Ok(Ok(stream)) => Some(stream.boxed()),
        _ => {
            app.sidebar.connection = crate::sidebar::Connection::Disconnected;
            None
        }
    }
}

pub(crate) fn server_event(app: &mut App, event: &cyber_client::Event) -> Vec<Action> {
    if event.kind.starts_with("workflow.") {
        return app.workflow_event(event);
    }
    if commands_changed(event) {
        return vec![
            Action::LoadCommands,
            Action::LoadSidebar(crate::sidebar::Part::Mcp),
        ];
    }
    if event_is_visible(&app.session.id, event) {
        let actions = app.on_server_event(event);
        reload_sidebar(app, actions)
    } else {
        Vec::new()
    }
}

/// MCP prompt commands appear when a server publishes its prompts, change on
/// `list_changed` and go when it disconnects, so completion refetches `/commands`.
fn commands_changed(event: &cyber_client::Event) -> bool {
    matches!(
        event.kind.as_str(),
        "mcp.tools.changed.1" | "mcp.status.changed.1"
    )
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

/// Ask the terminal for its background color (OSC 11). The reply comes back through
/// the key stream, where [`App::probe_key`] takes it. Only on Unix terminals and in
/// `tui.theme_mode: auto`; elsewhere `COLORFGBG` or dark stands.
fn query_background(app: &mut App) {
    if !cfg!(unix) || app.appearance.mode != crate::theme::ThemeMode::Auto {
        return;
    }
    let mut out = std::io::stdout();
    let sent = out
        .write_all(crate::theme::detect::OSC11_QUERY.as_bytes())
        .and_then(|()| out.flush());
    if sent.is_ok() {
        app.background_probe = Some(crate::theme::detect::Osc11Reply::new(
            std::time::Instant::now(),
        ));
    }
}

fn terminal_event(app: &mut App, ev: TermEvent) -> Vec<Action> {
    match ev {
        TermEvent::Key(key) if key.kind != KeyEventKind::Release => app
            .probe_key(key)
            .map(|key| app.on_key(key))
            .unwrap_or_default(),
        TermEvent::Paste(text) => {
            app.on_paste(&text);
            Vec::new()
        }
        TermEvent::FocusGained => {
            app.focused = true;
            app.recap_focus_gained()
        }
        TermEvent::FocusLost => {
            app.focused = false;
            app.recap_focus_lost();
            Vec::new()
        }
        _ => Vec::new(),
    }
}

/// The `/skills` picker, for the Location the request was made in.
fn open_skills(app: &mut App, directory: &str, result: Result<Vec<Choice>, String>) {
    if directory != app.session.directory {
        return;
    }
    match result {
        Ok(items) => app.open_picker(
            PickerKind::Skills,
            "Skills · Enter starts /name · scope · permission · source · diagnostics",
            items,
        ),
        Err(error) => app.toast(format!("skills: {error}")),
    }
}

pub(crate) fn apply(app: &mut App, msg: Result<Msg, String>, refresh: &mut Refresh) -> Vec<Action> {
    let actions = apply_msg(app, msg, refresh);
    reload_sidebar(app, actions)
}

fn apply_msg(app: &mut App, msg: Result<Msg, String>, refresh: &mut Refresh) -> Vec<Action> {
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
            messages,
            loops,
        } => {
            refresh.in_flight = false;
            if session.id != app.session.id || session.seq < app.session.seq {
                return vec![Action::Refresh];
            }
            app.streaming.retain(|id, _| !items.iter().any(|i| i.id() == id && matches!(i, crate::model::Item::Assistant { text, .. } if !text.is_empty())));
            app.set_session(session);
            app.items = items;
            app.queued = queued;
            app.messaging.set_records(messages);
            app.loops.replace(loops);
            app.requests = requests;
            app.sync_overlay();
            if std::mem::take(&mut refresh.again) {
                return vec![Action::Refresh];
            }
            return app.recap_on_open();
        }
        Msg::ModeChanged { session_id, result } => {
            return mode_changed(app, refresh, session_id, result);
        }
        Msg::Memory { request, result } => {
            if request.session_id == app.session.id
                && request.directory == app.session.directory
                && matches!(app.overlay, crate::app::Overlay::Memory)
            {
                app.memory.apply(&request, result);
            }
        }
        Msg::HookDefinitions {
            session_id,
            directory,
            generation,
            result,
        } => {
            if session_id == app.session.id
                && directory == app.session.directory
                && matches!(app.overlay, crate::app::Overlay::HookDefinitions)
            {
                app.hook_definitions.apply(generation, result);
            }
        }
        Msg::HookHistory {
            session_id,
            generation,
            result,
        } => {
            if session_id == app.session.id
                && matches!(app.overlay, crate::app::Overlay::HookHistory)
            {
                app.hooks.apply(generation, result);
            }
        }
        msg @ (Msg::Cost { .. }
        | Msg::Status { .. }
        | Msg::Stats { .. }
        | Msg::Insights { .. }
        | Msg::Limits { .. }
        | Msg::Recap { .. }) => settings_reply(app, msg),
        msg @ (Msg::Dir { .. } | Msg::FileHits { .. } | Msg::Preview { .. }) => {
            explorer_reply(app, msg)
        }
        Msg::Tasks { session_id, items } => {
            if session_id == app.session.id {
                app.open_or_refresh_picker(
                    PickerKind::Tasks,
                    "Tasks · Enter opens · x stops",
                    items,
                );
            }
        }
        Msg::Loops {
            session_id,
            choices,
        } => app.loops_listed(&session_id, choices),
        Msg::Children { session_id, result } => {
            if session_id == app.session.id {
                match result {
                    Ok(items) => app.open_picker(
                        PickerKind::Children,
                        "Subagents · Enter opens thread · Ctrl+R refreshes",
                        items,
                    ),
                    Err(error) => app.toast(error),
                }
            }
        }
        Msg::Sessions(items) => app.open_picker(PickerKind::Sessions, "Sessions", items),
        Msg::Models(items) => app.open_picker(PickerKind::Models, "Models", items),
        Msg::Skills { directory, result } => open_skills(app, &directory, result),
        Msg::Plugins {
            directory,
            reload,
            result,
        } => crate::plugins::show(app, &directory, reload, result),
        Msg::Commands {
            items,
            palette,
            bundled_review,
        } => {
            app.commands = items;
            app.server_commands = palette;
            app.review.set_bundled(bundled_review);
        }
        Msg::Catalog(catalog) => app.catalog = catalog,
        Msg::Refused { command, message } => app.refused(&command, &message),
        Msg::ReviewDelegation(record) => app.review_delegation(&record),
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
        Msg::Sidebar {
            session_id,
            directory,
            loaded,
        } => {
            if session_id == app.session.id && directory == app.session.directory {
                app.sidebar.apply(loaded);
            }
        }
        Msg::Workflow(reply) => return app.workflow_reply(reply),
        Msg::Team {
            session_id,
            view,
            open,
            toast,
        } => app.team_reply(&session_id, *view, open, toast),
        Msg::Toast(text) => app.toast(text),
        Msg::TaskLog {
            session_id,
            title,
            text,
        } => {
            if session_id == app.session.id {
                app.open_task_log(title, text);
            }
        }
        Msg::SideAnswer {
            session_id,
            question,
            result,
        } => app.side_answer(&session_id, &question, result),
        Msg::Compose(text) => {
            app.composer.set_text(&text);
            app.toast("Review the drafted command, then press Enter to run it");
        }
        Msg::Done => {}
    }
    Vec::new()
}

/// A file explorer reply, for the Session (and directory) it was read for.
fn explorer_reply(app: &mut App, msg: Msg) {
    match msg {
        Msg::Dir {
            session_id,
            directory,
            path,
            result,
        } if session_id == app.session.id && directory == app.session.directory => match result {
            Ok((level, changed)) => app.explorer.apply_level(path, level, changed),
            Err(error) => app.toast(format!("files: {error}")),
        },
        Msg::FileHits {
            session_id,
            query,
            hits,
        } if session_id == app.session.id && query == app.explorer.filter => {
            app.explorer.hits = hits;
        }
        Msg::Preview {
            session_id,
            path,
            result,
        } if session_id == app.session.id => app.apply_preview(&path, result),
        _ => {}
    }
}

/// A Usage or Status tab reply, for the Session it was read for.
fn settings_reply(app: &mut App, msg: Msg) {
    match msg {
        Msg::Cost {
            session_id,
            generation,
            result,
        } if session_id == app.session.id => app.cost.apply(&session_id, generation, result),
        Msg::Status {
            session_id,
            generation,
            status,
        } if session_id == app.session.id => app.settings.status.apply(generation, Ok(*status)),
        Msg::Stats { generation, result } => app.settings.stats.data.apply(generation, result),
        Msg::Insights { generation, result } => {
            app.settings.insights.data.apply(generation, result)
        }
        Msg::Limits { generation, result } => app.settings.limits.data.apply(generation, result),
        Msg::Recap {
            session_id,
            manual,
            result,
        } => app.apply_recap(&session_id, manual, result),
        _ => {}
    }
}

/// Reload every sidebar source after the Session or its directory changed.
fn reload_sidebar(app: &mut App, mut actions: Vec<Action>) -> Vec<Action> {
    if std::mem::take(&mut app.sidebar_stale) {
        actions.extend(crate::sidebar::Part::ALL.map(Action::LoadSidebar));
        actions.extend(app.watch_reload());
    }
    actions
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
        let _ = tx.send(perform::catalog(&client).await).await;
    });
}

/// Save a preference the UI changed to `kv.json`.
pub(crate) fn persist(store: &LocalStore, action: Action) {
    match action {
        Action::SaveTheme(name) => store.save_preference("theme", Value::String(name)),
        Action::SavePreference { key, value } => store.save_preference(key, value),
        _ => {}
    }
}

/// `p` in the theme switcher: `theme` in the project's `.cyber/tui.jsonc`.
pub(crate) fn save_project_theme(app: &mut App, name: &str) {
    let directory = std::path::Path::new(&app.session.directory);
    match crate::prefs::save_project_theme(directory, name) {
        Ok(path) => app.toast(format!("Theme {name} saved to {}", path.display())),
        Err(error) => app.toast(error),
    }
}

fn count(lines: usize) -> String {
    if lines == 1 {
        "1 line".into()
    } else {
        format!("{lines} lines")
    }
}

/// Copy through a native clipboard tool, or OSC 52 on the terminal without one.
fn copy(app: &mut App, code: &str) {
    let env = |name: &str| std::env::var(name).ok();
    let lines = code.lines().count();
    match crate::osc::copy(code, &env, &mut std::io::stdout()) {
        Ok(crate::osc::Copied::Native(tool)) => {
            app.toast(format!("Copied {} ({tool})", count(lines)))
        }
        Ok(crate::osc::Copied::Osc52) => app.toast(format!("Copied {} (OSC 52)", count(lines))),
        Err(error) => app.toast(format!("Copy failed: {error}")),
    }
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
            action @ (Action::SaveTheme(_) | Action::SavePreference { .. }) => {
                persist(store, action)
            }
            Action::SaveProjectTheme(name) => save_project_theme(app, &name),
            Action::Copy(code) => copy(app, &code),
            Action::Editor(text) => {
                let edited = edit_externally(terminal, &text);
                app.composer.set_text(&edited);
            }
            Action::EditFile(path) => edit_file(app, terminal, &path),
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
                if let Err(error) = app.admissions.prepare(&action, store) {
                    app.toast(error);
                    continue;
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

/// Run `$VISUAL`/`$EDITOR` on `path`, suspending the UI; true when it exited cleanly.
/// The path is passed as an argument, never as shell text.
fn run_editor(terminal: &mut Term, path: &std::path::Path) -> bool {
    let editor = std::env::var("VISUAL")
        .or_else(|_| std::env::var("EDITOR"))
        .unwrap_or_else(|_| "vi".into());
    teardown(terminal);
    let status = std::process::Command::new("sh")
        .arg("-c")
        .arg(format!("{editor} \"$1\""))
        .arg("sh")
        .arg(path)
        .status();
    if let Ok(t) = setup() {
        *terminal = t;
    }
    let _ = terminal.clear();
    status.is_ok_and(|s| s.success())
}

/// `s` on a Workflow Run: its script in the editor, when this machine has the file.
fn edit_file(app: &mut App, terminal: &mut Term, path: &str) {
    let path = std::path::Path::new(path);
    if !path.is_file() {
        app.toast(format!(
            "Script not found here: {}",
            crate::sidebar::display_path(&path.display().to_string())
        ));
        return;
    }
    if !run_editor(terminal, path) {
        app.toast("The editor did not exit cleanly");
    }
}

/// Open the draft in `$VISUAL`/`$EDITOR`, suspending the UI.
fn edit_externally(terminal: &mut Term, text: &str) -> String {
    let path = std::env::temp_dir().join(format!("cyber-prompt-{}.md", std::process::id()));
    if std::fs::write(&path, text).is_err() {
        return text.to_string();
    }
    let edited = run_editor(terminal, &path)
        .then(|| std::fs::read_to_string(&path).ok())
        .flatten();
    let _ = std::fs::remove_file(&path);
    edited
        .map(|e| e.trim_end_matches('\n').to_string())
        .unwrap_or_else(|| text.to_string())
}

/// `cyber · <title> · <status>`. The title is server data, often written by a model, and
/// goes into an OSC 0 sequence that ratatui does not filter: a BEL or ESC in it would end
/// the sequence and reach the terminal as its own (an OSC 52 clipboard write, a screen
/// clear), so control characters are removed.
fn window_title(app: &App) -> String {
    let status = if app.session.running {
        "running"
    } else {
        "idle"
    };
    let title = crate::sidebar::clean(&app.session.title);
    format!("cyber · {title} · {status}")
}

fn set_title(app: &App) {
    let mut out = std::io::stdout();
    let _ = execute!(out, terminal::SetTitle(window_title(app)));
    let _ = out.flush();
}

#[cfg(test)]
mod mode_tests {
    use super::*;

    #[test]
    fn window_title_drops_control_characters_from_a_model_written_title() {
        let mut app = app();
        app.session.title = "Evil\x07\x1b]52;c;SGFja2Vk\x07\x1b[2J\u{9b}2Jtitle\nsecond".into();
        let title = window_title(&app);
        assert_eq!(title, "cyber · Evil]52;c;SGFja2Vk[2J2Jtitle · idle");
        assert!(!title.chars().any(char::is_control));
    }

    #[test]
    fn memory_panel_responses_cannot_cross_session_location_or_dismissal() {
        let mut app = app();
        let mut refresh = Refresh::default();
        let request = app.memory.open(crate::memory::Scope::Global, &app.session);
        app.overlay = crate::app::Overlay::Memory;
        for foreign in ["session", "location"] {
            let mut foreign_request = request.clone();
            if foreign == "session" {
                foreign_request.session_id = "previous".into();
            } else {
                foreign_request.directory = "/previous".into();
            }
            apply(
                &mut app,
                Ok(Msg::Memory {
                    request: foreign_request,
                    result: Err("foreign failure".into()),
                }),
                &mut refresh,
            );
            assert!(!app.memory.lines().join(" ").contains("foreign failure"));
        }
        app.overlay = crate::app::Overlay::None;
        apply(
            &mut app,
            Ok(Msg::Memory {
                request: request.clone(),
                result: Err("dismissed failure".into()),
            }),
            &mut refresh,
        );
        assert!(!app.memory.lines().join(" ").contains("dismissed failure"));
        app.overlay = crate::app::Overlay::Memory;
        apply(
            &mut app,
            Ok(Msg::Memory {
                request,
                result: Err("current failure".into()),
            }),
            &mut refresh,
        );
        assert!(app.memory.lines().join(" ").contains("current failure"));
    }

    #[test]
    fn hook_definitions_responses_cannot_cross_session_location_or_dismissal() {
        let mut app = app();
        app.session.directory = "/current".into();
        let mut refresh = Refresh::default();
        let generation = app.hook_definitions.load();
        app.overlay = crate::app::Overlay::HookDefinitions;
        for (id, directory) in [("previous", "/current"), ("current", "/previous")] {
            apply(
                &mut app,
                Ok(Msg::HookDefinitions {
                    session_id: id.into(),
                    directory: directory.into(),
                    generation,
                    result: Err("foreign failure".into()),
                }),
                &mut refresh,
            );
            assert!(
                !app.hook_definitions
                    .lines()
                    .join(" ")
                    .contains("foreign failure")
            );
        }
        app.overlay = crate::app::Overlay::None;
        apply(
            &mut app,
            Ok(Msg::HookDefinitions {
                session_id: "current".into(),
                directory: "/current".into(),
                generation,
                result: Err("dismissed failure".into()),
            }),
            &mut refresh,
        );
        assert!(matches!(app.overlay, crate::app::Overlay::None));
        assert!(
            !app.hook_definitions
                .lines()
                .join(" ")
                .contains("dismissed failure")
        );
        app.overlay = crate::app::Overlay::HookDefinitions;
        apply(
            &mut app,
            Ok(Msg::HookDefinitions {
                session_id: "current".into(),
                directory: "/current".into(),
                generation,
                result: Err("current failure".into()),
            }),
            &mut refresh,
        );
        assert!(
            app.hook_definitions
                .lines()
                .join(" ")
                .contains("current failure")
        );
    }

    #[test]
    fn hook_history_responses_do_not_reopen_dismissed_views_or_cross_sessions() {
        let mut app = app();
        let mut refresh = Refresh::default();
        let generation = app.hooks.load();
        app.overlay = crate::app::Overlay::HookHistory;
        apply(
            &mut app,
            Ok(Msg::HookHistory {
                session_id: "previous".into(),
                generation,
                result: Err("foreign failure".into()),
            }),
            &mut refresh,
        );
        assert!(
            app.hooks
                .lines()
                .join(" ")
                .contains("Loading hook receipts")
        );
        app.overlay = crate::app::Overlay::None;
        apply(
            &mut app,
            Ok(Msg::HookHistory {
                session_id: "current".into(),
                generation,
                result: Err("dismissed failure".into()),
            }),
            &mut refresh,
        );
        assert!(matches!(app.overlay, crate::app::Overlay::None));
        assert!(
            app.hooks
                .lines()
                .join(" ")
                .contains("Loading hook receipts")
        );
        assert!(app.toast.is_none());
    }

    #[test]
    fn child_thread_responses_are_ignored_after_switching_sessions() {
        let mut app = app();
        let mut refresh = Refresh::default();
        for result in [Ok(Vec::new()), Err("old request failed".into())] {
            assert!(
                apply(
                    &mut app,
                    Ok(Msg::Children {
                        session_id: "previous".into(),
                        result,
                    }),
                    &mut refresh
                )
                .is_empty()
            );
            assert!(matches!(app.overlay, crate::app::Overlay::None));
            assert!(app.toast.is_none());
        }
        apply(
            &mut app,
            Ok(Msg::Children {
                session_id: "current".into(),
                result: Err("current request failed".into()),
            }),
            &mut refresh,
        );
        assert_eq!(app.toast.as_ref().unwrap().0, "current request failed");
    }

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
                messages: None,
                requests: Vec::new(),
                loops: None,
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
            messages: None,
            requests: Vec::new(),
            loops: None,
        };
        let actions = apply(&mut app, Ok(snapshot), &mut Refresh::default());
        assert_eq!(actions[0], Action::Refresh);
        assert!(
            actions[1..].iter().all(|a| matches!(
                a,
                Action::LoadSidebar(_) | Action::Workflow(crate::workflows::Request::Watch { .. })
            )),
            "the rebound directory reloads the sidebar: {actions:?}"
        );
        assert_eq!(app.session.directory, "/repo/new");
        assert_eq!(app.session.seq, 8);
    }
    #[test]
    fn an_mcp_prompt_published_after_startup_becomes_a_completion_choice() {
        let mut app = app();
        let choice = |name: &str| crate::model::Choice {
            key: name.into(),
            label: name.into(),
            detail: String::new(),
        };
        apply(
            &mut app,
            Ok(Msg::Commands {
                items: vec![choice("review")],
                palette: Vec::new(),
                bundled_review: false,
            }),
            &mut Refresh::default(),
        );
        // Other Locations' MCP events carry no session, yet each one refetches.
        for kind in ["mcp.status.changed.1", "mcp.tools.changed.1"] {
            let event = cyber_client::Event {
                kind: kind.into(),
                data: serde_json::json!({"name":"gh","prompts":["gh:review_pr"]}),
                seq: None,
                session_id: None,
            };
            assert_eq!(
                server_event(&mut app, &event),
                vec![
                    Action::LoadCommands,
                    Action::LoadSidebar(crate::sidebar::Part::Mcp)
                ]
            );
        }
        apply(
            &mut app,
            Ok(Msg::Commands {
                items: vec![choice("review"), choice("gh:review_pr")],
                palette: Vec::new(),
                bundled_review: false,
            }),
            &mut Refresh::default(),
        );
        for c in "/gh:rev".chars() {
            app.on_key(crossterm::event::KeyEvent::new(
                crossterm::event::KeyCode::Char(c),
                crossterm::event::KeyModifiers::NONE,
            ));
        }
        let completion = app.completion.as_ref().expect("no completion menu");
        assert_eq!(completion.items[0].label, "gh:review_pr");
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

    fn skill_catalog_choices() -> Vec<crate::model::Choice> {
        let catalog = serde_json::json!({"agent":"build","skills":[
            {"name":"deploy","description":"Deploy","source":"/repo/.cyber/skills/deploy","scope":"project",
             "status":"loaded","user_invocable":true,"model_invocable":true,"permission":"deny",
             "diagnostics":["/repo/.cyber/skills/deploy: replaces skill deploy from /g/deploy"]},
            {"name":"broken","description":"","source":"/repo/.cyber/skills/broken/SKILL.md","scope":"project",
             "status":"invalid","user_invocable":false,"model_invocable":false,"permission":null,
             "diagnostics":["/repo/.cyber/skills/broken/SKILL.md: invalid frontmatter\u{1b}[31m"]}
        ]});
        crate::perform::skill_choices(&catalog)
    }

    /// `GET /skills` rows show scope, permission, source and diagnostics; skipped
    /// definitions have no command key.
    #[test]
    fn skill_rows_show_scope_permission_and_diagnostics() {
        let items = skill_catalog_choices();
        assert_eq!(items[0].key, "deploy");
        assert!(
            items[0]
                .detail
                .contains("project · deny · /repo/.cyber/skills/deploy")
        );
        assert!(
            items[0]
                .detail
                .contains("replaces skill deploy from /g/deploy")
        );
        assert_eq!(items[1].key, "");
        assert_eq!(items[1].label, "broken (invalid)");
        assert!(items[1].detail.contains("skipped"));
        assert!(items[1].detail.contains("invalid frontmatter"));
        assert!(!items[1].detail.contains('\u{1b}'));
    }

    /// `/skills` loads the catalog into a picker; Enter on a user-invocable skill starts it.
    #[test]
    fn skills_command_opens_a_picker_that_starts_the_chosen_skill() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let mut app = app();
        let enter = |app: &mut App, code| app.on_key(KeyEvent::new(code, KeyModifiers::NONE));
        for c in "/skills".chars() {
            enter(&mut app, KeyCode::Char(c));
        }
        assert_eq!(enter(&mut app, KeyCode::Enter), vec![Action::LoadSkills]);
        let message = Msg::Skills {
            directory: app.session.directory.clone(),
            result: Ok(skill_catalog_choices()),
        };
        assert!(apply(&mut app, Ok(message), &mut Refresh::default()).is_empty());
        assert!(matches!(
            &app.overlay,
            crate::app::Overlay::Picker(picker) if picker.kind == PickerKind::Skills
        ));
        assert!(enter(&mut app, KeyCode::Enter).is_empty());
        assert_eq!(app.composer.text(), "/deploy ");
    }

    /// A server that answers every request with the organization's bypass refusal, as
    /// `POST /sessions/:id/mode` does when `policy.modes.disable` lists `bypass`.
    async fn refusing_server() -> String {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                let mut buffer = vec![0; 8192];
                let _ = socket.read(&mut buffer).await;
                let body = json!({"_tag": "InvalidRequestError",
                    "message": "bypass mode is disabled by your organization"})
                .to_string();
                let reply = format!(
                    "HTTP/1.1 400 Bad Request\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = socket.write_all(reply.as_bytes()).await;
            }
        });
        base
    }

    /// `tui` → Permission mode indicator and cycling: when org policy disables `bypass`,
    /// the confirmed `/mode bypass` shows the server's reason and the mode is unchanged.
    #[tokio::test]
    async fn mode_bypass_disabled_by_org_policy_keeps_mode() {
        let mut app = app();
        for c in "/mode bypass".chars() {
            app.on_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
        }
        assert!(
            app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
                .is_empty()
        );
        let action = app
            .on_key(KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE))
            .remove(0);
        assert_eq!(action, Action::SwitchMode("bypass".into()));
        let client = cyber_client::Client::http(&refusing_server().await, None);
        let msg = perform::perform_owned(&client, &app.session, action, None).await;
        let mut refresh = Refresh {
            mode_in_flight: true,
            ..Refresh::default()
        };
        apply(&mut app, msg, &mut refresh);
        assert_eq!(app.session.mode, "default");
        assert_eq!(app.mode_label(), "default");
        assert_eq!(
            app.toast.as_ref().map(|(text, _)| text.as_str()),
            Some("bypass mode is disabled by your organization")
        );
    }
}
