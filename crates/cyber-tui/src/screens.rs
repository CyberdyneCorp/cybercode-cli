//! Screen snapshots (`tui` → Theme tokens, Color capability and NO_COLOR): the real
//! `view::draw` on a `TestBackend`, fed from a production App over HTTP with a local
//! scripted model. Snapshots hold the symbols of every row plus a style legend for
//! chosen rows.

#[path = "../../cyber-app/tests/support/scripted_model.rs"]
pub(crate) mod scripted_model;

use std::fmt::Write as _;
use std::time::Duration;

use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::style::{Color, Modifier};
use serde_json::{Value, json};

use crate::app::App;
use crate::model::Session;

/// What a snapshot renders with.
pub struct Opts {
    pub theme: &'static str,
    /// Render as under `NO_COLOR` (overrides `depth`).
    pub no_color: bool,
    pub depth: crate::theme::Depth,
    /// Terminal background is light (an OSC 11 white reply).
    pub light: bool,
    pub reduced_motion: bool,
    /// Milliseconds after start that every frame is drawn at.
    pub clock_ms: u64,
    /// Rows containing one of these texts get their style runs written below the symbols.
    pub legend: &'static [&'static str],
}

impl Default for Opts {
    fn default() -> Self {
        Self {
            theme: "cyber",
            no_color: false,
            depth: crate::theme::Depth::TrueColor,
            light: false,
            reduced_motion: true,
            clock_ms: 0,
            legend: &[],
        }
    }
}

/// Draw `app` at `width`×`height` and return the buffer.
pub fn buffer(app: &mut App, width: u16, height: u16, opts: &Opts) -> Buffer {
    crate::highlight::preload();
    let appearance = crate::theme::Appearance {
        depth: if opts.no_color {
            crate::theme::Depth::None
        } else {
            opts.depth
        },
        detected_light: Some(opts.light),
        ..Default::default()
    };
    app.set_appearance(app.themes.clone(), appearance);
    app.set_theme(opts.theme);
    app.reduced_motion = opts.reduced_motion;
    app.clock = Some(app.started() + Duration::from_millis(opts.clock_ms));
    app.progress_sync();
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal.draw(|f| crate::view::draw(f, app)).unwrap();
    terminal.backend().buffer().clone()
}

/// Symbols of every row, then `row N: start-end style` runs for the legend rows.
pub fn render(app: &mut App, width: u16, height: u16, opts: &Opts) -> String {
    let buf = buffer(app, width, height, opts);
    let rows: Vec<String> = (0..height)
        .map(|y| (0..width).map(|x| buf[(x, y)].symbol()).collect())
        .collect();
    let mut out = String::new();
    for row in &rows {
        writeln!(out, "{}", row.trim_end()).unwrap();
    }
    for (y, row) in rows.iter().enumerate() {
        if opts.legend.iter().any(|needle| row.contains(needle)) {
            out.push_str(&legend(&buf, y as u16));
        }
    }
    out
}

fn legend(buf: &Buffer, y: u16) -> String {
    let mut out = String::new();
    let mut x = 0;
    while x < buf.area.width {
        let style = describe(&buf[(x, y)]);
        let start = x;
        while x < buf.area.width && describe(&buf[(x, y)]) == style {
            x += 1;
        }
        if !style.is_empty() {
            writeln!(out, "row {y}: {start}-{} {style}", x - 1).unwrap();
        }
    }
    out
}

fn describe(cell: &ratatui::buffer::Cell) -> String {
    let mut parts = Vec::new();
    if cell.fg != Color::Reset {
        parts.push(format!("fg={}", color(cell.fg)));
    }
    if cell.bg != Color::Reset {
        parts.push(format!("bg={}", color(cell.bg)));
    }
    for (flag, name) in [
        (Modifier::BOLD, "bold"),
        (Modifier::ITALIC, "italic"),
        (Modifier::UNDERLINED, "underline"),
        (Modifier::REVERSED, "reverse"),
        (Modifier::DIM, "dim"),
    ] {
        if cell.modifier.contains(flag) {
            parts.push(name.to_string());
        }
    }
    parts.join(" ")
}

fn color(c: Color) -> String {
    match c {
        Color::Rgb(r, g, b) => format!("#{r:02x}{g:02x}{b:02x}"),
        Color::Indexed(i) => format!("@{i}"),
        other => format!("{other:?}").to_lowercase(),
    }
}

const ANSWER: &str = "## Plan\n\nThis keeps the cache warm:\n\n1. **Memoization:** results are cached\n- avoids `recomputation`\n\n```rust\nfn fib(n: u64) -> u64 { n }\n```\n";
pub(crate) const TITLE: &str = "Theme baseline";
/// The recap the scripted model writes.
pub const RECAP: &str = "I explained how to speed up fib with memoization and a cache. Next I'll add a benchmark that compares the iterative version with the recursive one before changing the code.";

/// The answer from the reference screen: a Python block, then a numbered list with bold
/// terms and bullets.
const REFERENCE: &str = "```python\ndef fib(n):\n    # Build up solution iteratively\n    dp = [0, 1]\n    for i in range(2, n + 1):\n        dp.append(dp[i - 1] + dp[i - 2])\n    return dp[n]\n```\n\nThis code demonstrates dynamic optimization principles:\n\n1. **Overlapping Subproblems:** Both examples break down problems into smaller, repeated subproblems\n2. **Optimal Substructure:** Solutions are built from optimal solutions of subproblems\n3. **Memoization:** Results are cached to avoid recomputation\n\nKey benefits of this approach, see [the guide](https://example.com/dp):\n\n- Time complexity improvement from exponential to polynomial\n- Elimination of redundant computations\n";

/// A six-column table that needs about 140 columns, and a fence in an unknown language.
const TABLE: &str = "| Column one | Column two | Column three | Column four | Column five | Column six |\n|---|---|---|---|---|---|\n| the first value cell | the second value one | the third value cell | the fourth value one | the fifth value cell | the sixth value cell |\n\n```klingon\nqapla' batlh\n```\n";

/// A `read` of `fib.py`, then a short answer once the tool result is in.
fn read_tool(body: &Value) -> Value {
    if body.to_string().contains("\"role\":\"tool\"") {
        return json!({ "content": "Read it." });
    }
    json!({ "tool_calls": [{ "index": 0, "id": "call_read", "type": "function",
        "function": { "name": "read", "arguments": "{\"path\":\"fib.py\"}" } }] })
}

/// A `write` of `notes.md`, then a short answer once the tool result is in.
fn write_tool(body: &Value) -> Value {
    if body.to_string().contains("\"role\":\"tool\"") {
        return json!({ "content": "Wrote it." });
    }
    let arguments = json!({ "path": "notes.md", "content": "# Notes\none\ntwo\n" }).to_string();
    json!({ "tool_calls": [{ "index": 0, "id": "call_write", "type": "function",
        "function": { "name": "write", "arguments": arguments } }] })
}

/// The answer for a prompt, chosen by a keyword in it.
pub(crate) fn reply(body: &Value) -> Value {
    let text = body.to_string();
    if text.contains("Write a short title") {
        return json!({ "content": TITLE });
    }
    if text.contains("summarize a coding session") {
        return json!({ "content": RECAP });
    }
    if text.contains("PLAN LIMITS") {
        // Plan windows in the response headers (`observability-costs` → Provider-reported
        // limits): 85% of the 5-hour window, 30% of the weekly one.
        return json!({ "content": ANSWER, "headers": {
            "anthropic-ratelimit-unified-5h-utilization": "0.85",
            "anthropic-ratelimit-unified-5h-reset": "1792130400",
            "anthropic-ratelimit-unified-7d-utilization": "0.3",
            "anthropic-ratelimit-unified-7d-reset": "1792728000",
        } });
    }
    let answer = if text.contains("REFERENCE") {
        REFERENCE
    } else if text.contains("TABLE") {
        TABLE
    } else if text.contains("READ") {
        return read_tool(body);
    } else if text.contains("WRITE") {
        return write_tool(body);
    } else if text.contains("FAIL") {
        // Before HOLD: a later prompt's history holds the earlier ones.
        return json!({ "fail": 400 });
    } else if text.contains("HOLD") {
        return json!({ "hold": true });
    } else {
        ANSWER
    };
    json!({ "content": answer })
}

/// A production App on a TCP listener with the scripted model as `test/main`.
pub struct Server {
    temp: tempfile::TempDir,
    pub host: cyber_app::App,
    pub client: cyber_client::Client,
    pub model: scripted_model::ScriptedModel,
}

impl Server {
    pub async fn start() -> Self {
        Self::start_with(json!({})).await
    }

    /// A server whose `test/main` model has this configuration (`name`, `limits`,
    /// `cost`).
    pub async fn start_with(main: Value) -> Self {
        Self::start_config(main, json!({}), false).await
    }

    /// [`Server::start_with`], with `extra` merged into the global configuration (MCP
    /// servers, sandbox) and file snapshots on or off.
    pub async fn start_config(main: Value, extra: Value, snapshots: bool) -> Self {
        Self::start_models(json!({ "main": main }), extra, snapshots).await
    }

    /// [`Server::start_config`] with every model of provider `test` (`main`, `small`).
    pub async fn start_models(models: Value, extra: Value, snapshots: bool) -> Self {
        Self::start_reply(reply, models, extra, snapshots).await
    }

    /// [`Server::start_models`] whose scripted model answers with `answer`.
    pub async fn start_reply(
        answer: scripted_model::Reply,
        models: Value,
        extra: Value,
        snapshots: bool,
    ) -> Self {
        use cyber_core::paths::{DatabaseLocation, Paths};
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let paths = Paths {
            data: root.join("data"),
            config: root.join("config"),
            state: root.join("state"),
            cache: root.join("cache"),
            tmp: root.join("tmp"),
        };
        paths.ensure().unwrap();
        let model = scripted_model::ScriptedModel::start(answer);
        let mut config = model.config();
        config["providers"]["test"]["models"] = models;
        if let (Some(config), Value::Object(extra)) = (config.as_object_mut(), extra) {
            config.extend(extra);
        }
        std::fs::write(paths.config.join("cyber.json"), config.to_string()).unwrap();
        let host = cyber_app::App::build(cyber_app::AppOptions {
            paths,
            home: root.join("home"),
            database: DatabaseLocation::Memory,
            default_directory: root.clone(),
            sandbox_policy: None,
            snapshots,
            interactive: true,
            password: Some("tui-screens".into()),
            mcp_credentials: None,
            remote_skill_fetcher: None,
            // Slot counts in snapshots must not depend on the runner's cores.
            workflow_cores: Some(16),
            // Screens open `bypass` Sessions without testing the eligibility gate, also on
            // Windows, where no OS sandbox admits them.
            isolation: Some(cyber_app::IsolationOptions {
                dangerously_bypass: true,
                container: false,
            }),
        })
        .await
        .unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(cyber_server::http::serve_tcp(
            host.router(),
            listener,
            std::future::pending::<()>(),
        ));
        let client = cyber_client::Client::http(&base, Some("tui-screens".into()))
            .at(&root.display().to_string());
        Self {
            temp,
            host,
            client,
            model,
        }
    }

    /// The canonical temporary directory the Sessions run in.
    pub fn root(&self) -> std::path::PathBuf {
        self.temp.path().canonicalize().unwrap()
    }

    /// A Session that answered one prompt and got its generated title.
    pub async fn answered_session(&self) -> String {
        self.answered("How do I speed up fib?").await
    }

    pub async fn session(&self) -> String {
        let created = self
            .client
            .post(
                "/sessions",
                json!({ "model": "test/main", "mode": "default" }),
            )
            .await
            .unwrap();
        created["data"]["id"].as_str().unwrap().to_owned()
    }

    /// A Session in `directory` with `mode`.
    #[cfg(unix)]
    pub async fn session_in(&self, directory: &std::path::Path, mode: &str) -> String {
        let created = self
            .client
            .at(&directory.display().to_string())
            .post("/sessions", json!({ "model": "test/main", "mode": mode }))
            .await
            .unwrap();
        created["data"]["id"].as_str().unwrap().to_owned()
    }

    /// Wait until `id` answered and got its generated title.
    pub async fn wait_answered(&self, id: &str) {
        for _ in 0..1000 {
            let session = self.client.get(&format!("/sessions/{id}")).await.unwrap();
            if !self.host.runtime.is_running(id) && session["data"]["title"] == TITLE {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("{id} did not answer");
    }

    pub async fn prompt(&self, id: &str, text: &str) {
        self.client
            .post(
                &format!("/sessions/{id}/prompt"),
                json!({ "parts": [{ "type": "text", "text": text }] }),
            )
            .await
            .unwrap();
    }

    /// A Session that answered `prompt` and got its generated title.
    pub async fn answered(&self, prompt: &str) -> String {
        let id = self.session().await;
        self.prompt(&id, prompt).await;
        self.wait_answered(&id).await;
        id
    }

    /// The TUI state the runner builds for `id`, through `perform::snapshot` and the
    /// runner's own `apply`.
    pub async fn app(&self, id: &str) -> App {
        let mut app = self.load(id).await;
        stabilize(&mut app);
        app
    }

    /// [`Server::app`] with the server's own directory and step times.
    pub async fn load(&self, id: &str) -> App {
        let mut app = App::new(Session::default(), Vec::new(), "cyber");
        app.session.id = id.to_owned();
        let msg = crate::perform::snapshot(&self.client, id).await;
        let follow = crate::runner::apply(&mut app, msg, &mut Default::default());
        self.perform(&mut app, follow).await;
        app
    }

    /// Run the sidebar loads `actions` asks for through `perform` and the runner's
    /// `apply`, as the runner does, until nothing follows.
    pub async fn perform(&self, app: &mut App, mut actions: Vec<crate::app::Action>) {
        while let Some(action) = actions.pop() {
            assert!(
                matches!(
                    action,
                    crate::app::Action::LoadSidebar(_)
                        | crate::app::Action::ReconnectMcp(_)
                        | crate::app::Action::Workflow(crate::workflows::Request::Watch { .. })
                ),
                "{action:?}"
            );
            let msg = crate::perform::perform(&self.client, &app.session, action).await;
            actions.extend(crate::runner::apply(app, msg, &mut Default::default()));
        }
    }
}

/// 14:02:00 UTC on 2026-10-16, minus 11.3 s.
pub const STARTED_MS: i64 = 1_792_159_320_000 - 11_300;

/// What differs between runs: the temporary directory, and step times that come from
/// the wall clock. Every step that has times gets 11.3 s ending at 14:02 UTC, and clock
/// times use UTC.
pub fn stabilize(app: &mut App) {
    app.session.directory = "/Users/leo/work/app".into();
    app.home = Some("/Users/leo".into());
    if app.sidebar.version.is_some() {
        app.sidebar.version = Some("1.0.0".into());
    }
    app.utc_offset_minutes = Some(0);
    for item in &mut app.items {
        if let crate::model::Item::Assistant { step, .. } = item {
            if step.started_ms.is_some() {
                step.started_ms = Some(STARTED_MS);
            }
            if step.ended_ms.is_some() {
                step.ended_ms = Some(STARTED_MS + 11_300);
            }
        }
    }
}

#[tokio::test]
async fn baseline_screens_from_the_production_server() {
    let server = Server::start().await;
    let id = server.answered_session().await;
    let mut app = server.app(&id).await;
    for (width, height) in [(60, 20), (80, 24), (120, 40), (160, 48)] {
        let opts = Opts {
            legend: &[" cyber ", "Memoization", "fn fib", "Plan"],
            ..Opts::default()
        };
        insta::assert_snapshot!(
            format!("transcript_{width}x{height}"),
            render(&mut app, width, height, &opts)
        );
    }
}

pub fn cells(buf: &Buffer) -> impl Iterator<Item = &ratatui::buffer::Cell> {
    buf.content.iter()
}

pub fn row_of(buf: &Buffer, needle: &str) -> u16 {
    (0..buf.area.height)
        .find(|&y| {
            (0..buf.area.width)
                .map(|x| buf[(x, y)].symbol())
                .collect::<String>()
                .contains(needle)
        })
        .unwrap_or_else(|| panic!("{needle:?} is not on screen"))
}

#[tokio::test]
async fn no_color_draws_no_cell_with_a_color_and_selection_in_reverse_video() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    let server = Server::start().await;
    let id = server.answered_session().await;
    let mut app = server.app(&id).await;
    for c in "/theme".chars() {
        app.on_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
    }
    app.completion = None;
    app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    let opts = Opts {
        no_color: true,
        ..Opts::default()
    };
    let buf = buffer(&mut app, 80, 24, &opts);
    for cell in cells(&buf) {
        assert_eq!((cell.fg, cell.bg), (Color::Reset, Color::Reset), "{cell:?}");
    }
    let selected = row_of(&buf, "❯ cyber");
    let x = (0..80)
        .find(|&x| buf[(x, selected)].symbol() == "❯")
        .unwrap();
    assert!(buf[(x, selected)].modifier.contains(Modifier::REVERSED));
    insta::assert_snapshot!(
        "theme_picker_no_color_80x24",
        render(&mut app, 80, 24, &opts)
    );
}

#[tokio::test]
async fn a_256_color_terminal_gets_only_xterm_indexes() {
    let server = Server::start().await;
    let id = server.answered_session().await;
    let mut app = server.app(&id).await;
    let opts = Opts {
        depth: crate::theme::Depth::Ansi256,
        ..Opts::default()
    };
    let buf = buffer(&mut app, 80, 24, &opts);
    for cell in cells(&buf) {
        for c in [cell.fg, cell.bg] {
            assert!(
                matches!(c, Color::Indexed(16..=255) | Color::Reset),
                "{cell:?}"
            );
        }
    }
    assert_eq!(buf[(0, 0)].bg, Color::Indexed(233));
}

/// The OSC 11 reply arrives through the key stream as crossterm parses it.
fn osc11_reply(body: &str) -> Vec<crossterm::event::KeyEvent> {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    let mut keys = vec![KeyEvent::new(KeyCode::Char(']'), KeyModifiers::ALT)];
    keys.extend(
        body.chars()
            .map(|c| KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)),
    );
    keys.push(KeyEvent::new(KeyCode::Char('\\'), KeyModifiers::ALT));
    keys
}

#[tokio::test]
async fn a_white_osc11_reply_switches_cyber_to_its_light_variant() {
    let server = Server::start().await;
    let id = server.answered_session().await;
    let mut app = server.app(&id).await;
    app.clock = Some(app.started());
    app.background_probe = Some(crate::theme::detect::Osc11Reply::new(app.started()));
    assert!(!app.theme.light, "the first frame is dark");
    for key in osc11_reply("11;rgb:ffff/ffff/ffff") {
        assert_eq!(
            app.probe_key(key),
            None,
            "reply keys never reach the composer"
        );
    }
    assert!(app.background_probe.is_none());
    assert!(app.theme.light);
    assert_eq!(app.composer.text(), "");
    let mut terminal = Terminal::new(TestBackend::new(120, 40)).unwrap();
    terminal.draw(|f| crate::view::draw(f, &app)).unwrap();
    let buf = terminal.backend().buffer();
    let answer = buf[(4, row_of(buf, "results are cached"))].clone();
    assert_eq!(answer.bg, Color::Rgb(255, 255, 255));
    let ratio = crate::theme::color::contrast(answer.fg, answer.bg).unwrap();
    assert!(ratio >= 4.5, "{ratio}");
    let typed = crossterm::event::KeyEvent::new(
        crossterm::event::KeyCode::Char('x'),
        crossterm::event::KeyModifiers::NONE,
    );
    assert_eq!(
        app.probe_key(typed),
        Some(typed),
        "keys pass once the reply is in"
    );
}

#[tokio::test]
async fn light_theme_and_overlays_use_tokens() {
    let server = Server::start().await;
    let id = server.answered_session().await;
    let mut app = server.app(&id).await;
    let opts = Opts {
        theme: "github-light",
        legend: &[" cyber ", "Memoization"],
        ..Opts::default()
    };
    insta::assert_snapshot!(
        "transcript_github_light_120x40",
        render(&mut app, 120, 40, &opts)
    );
    app.requests = vec![crate::model::Request::Permission {
        id: "per_1".into(),
        session_id: id.clone(),
        origin: None,
        member: None,
        routed_to: Vec::new(),
        action: "edit".into(),
        resources: vec!["src/fib.rs".into()],
        patterns: vec!["src/fib.rs".into()],
        metadata: json!({ "diff": "@@ -1 +1 @@\n-fn fib() {}\n+fn fib(n: u64) {}" }),
    }];
    app.sync_overlay();
    let opts = Opts {
        legend: &["-fn fib", "+fn fib", "Allow once"],
        ..Opts::default()
    };
    let shown = render(&mut app, 120, 40, &opts);
    let t = &app.theme;
    let removed = format!("bg={}", color(t.diff_removed_bg));
    let added = format!("bg={}", color(t.diff_added_bg));
    assert!(
        shown.contains(&removed) && shown.contains(&added),
        "{shown}"
    );
    insta::assert_snapshot!("permission_diff_120x40", shown);
}

#[tokio::test]
async fn the_running_spinner_animates_unless_motion_is_reduced() {
    let server = Server::start().await;
    let id = server.answered_session().await;
    let mut app = server.app(&id).await;
    app.session.running = true;
    let still = Opts::default();
    let header = |app: &mut App, opts: &Opts| {
        let buf = buffer(app, 100, 30, opts);
        let y = row_of(&buf, " running");
        (0..100)
            .map(|x| buf[(x, y)].symbol().to_string())
            .collect::<String>()
    };
    assert!(header(&mut app, &still).ends_with("… running "));
    let moving = |clock_ms| Opts {
        reduced_motion: false,
        clock_ms,
        ..Opts::default()
    };
    assert!(header(&mut app, &moving(0)).ends_with("⠋ running "));
    // The header follows the progress line's frames.
    app.clock = Some(app.started() + Duration::from_millis(84));
    assert!(app.advance_frame());
    assert!(header(&mut app, &moving(84)).ends_with("⠙ running "));
}

/// Load `id` the way the runner does and draw it as it is, without `Opts`.
async fn show(server: &Server, id: &str, app: &mut App) -> Buffer {
    let msg = crate::perform::snapshot(&server.client, id).await;
    crate::runner::apply(app, msg, &mut Default::default());
    let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
    terminal.draw(|f| crate::view::draw(f, app)).unwrap();
    terminal.backend().buffer().clone()
}

/// The runner's own startup (`runner::start`) reads `tui.jsonc`, custom themes and the
/// environment, then shows a Session the production server answered.
#[tokio::test]
async fn startup_reads_tui_jsonc_custom_themes_and_no_color() {
    let server = Server::start().await;
    let id = server.answered_session().await;
    let root = server.temp.path().canonicalize().unwrap();
    let config = root.join("config");
    std::fs::create_dir_all(config.join("themes")).unwrap();
    std::fs::write(
        config.join("themes/corp.json"),
        r##"{"theme":{"text":"#f0f0f0","background":"#101010","accent":"#ff8800"}}"##,
    )
    .unwrap();
    std::fs::write(
        config.join("tui.jsonc"),
        "{ \"theme\": \"corp\" } // pick it",
    )
    .unwrap();
    let opts = crate::TuiOptions {
        client: server.client.clone(),
        directory: root.display().to_string(),
        state_dir: root.join("state"),
        config_dir: config.clone(),
        start: crate::Start::Resume(id.clone()),
        fork: false,
        model: None,
        agent: None,
        mode: None,
        prompt: None,
    };
    let store = crate::store::LocalStore::new(&opts.state_dir);
    let env = |name: &str| (name == "COLORTERM").then(|| "truecolor".to_owned());
    let (mut app, _) = crate::runner::start(&server.client, &store, &opts, &env)
        .await
        .unwrap();
    let buf = show(&server, &id, &mut app).await;
    assert_eq!(app.theme.name, "corp");
    assert_eq!(
        buf[(1, row_of(&buf, " cyber "))].fg,
        Color::Rgb(0xff, 0x88, 0x00)
    );
    assert_eq!(buf[(0, 0)].bg, Color::Rgb(0x10, 0x10, 0x10));
    assert!(row_of(&buf, "Memoization") > 0);

    let no_color = |name: &str| (name == "NO_COLOR").then(|| "1".to_owned());
    let (mut app, _) = crate::runner::start(&server.client, &store, &opts, &no_color)
        .await
        .unwrap();
    let buf = show(&server, &id, &mut app).await;
    assert!(app.theme.monochrome);
    for cell in cells(&buf) {
        assert_eq!((cell.fg, cell.bg), (Color::Reset, Color::Reset), "{cell:?}");
    }
}
