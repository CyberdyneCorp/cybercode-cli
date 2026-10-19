//! Sidebar, responsive layout and welcome screen snapshots (`tui` → Session sidebar,
//! Responsive layout, Welcome screen), drawn from a production App over HTTP.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Color;
use serde_json::json;

use crate::app::{Action, App};
use crate::screens::{Opts, Server, buffer, cells, render, row_of, stabilize};
use crate::sidebar::Source;

fn press(app: &mut App, code: KeyCode, modifiers: KeyModifiers) -> Vec<Action> {
    app.on_key(KeyEvent::new(code, modifiers))
}

fn key(app: &mut App, c: char) -> Vec<Action> {
    press(app, KeyCode::Char(c), KeyModifiers::NONE)
}

/// `<leader>` then `c`.
fn chord(app: &mut App, c: char) -> Vec<Action> {
    assert!(press(app, KeyCode::Char('x'), KeyModifiers::CONTROL).is_empty());
    key(app, c)
}

fn text(buf: &Buffer) -> String {
    (0..buf.area.height)
        .map(|y| {
            (0..buf.area.width)
                .map(|x| buf[(x, y)].symbol())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The symbols of `area` only.
fn region(buf: &Buffer, area: Rect) -> String {
    (area.y..area.bottom())
        .map(|y| {
            (area.x..area.right())
                .map(|x| buf[(x, y)].symbol())
                .collect::<String>()
                .trim_end()
                .to_owned()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[tokio::test]
async fn docked_at_160_hidden_at_100_then_a_drawer_compact_at_50_and_too_small_at_30() {
    let server = Server::start().await;
    let id = server.answered_session().await;
    let mut app = server.app(&id).await;
    assert_eq!(app.sidebar.version.as_deref(), Some("1.0.0"));
    assert_eq!(
        app.sidebar.vcs,
        Source::Ready(None),
        "the temp dir is not git"
    );
    let opts = Opts {
        legend: &["Context", "~/work/app", "Cyber Code"],
        ..Opts::default()
    };
    let docked = render(&mut app, 160, 48, &opts);
    assert!(
        docked.contains("~/work/app\n") || docked.contains("~/work/app  "),
        "{docked}"
    );
    assert!(!docked.contains("~/work/app:"), "no branch outside git");
    insta::assert_snapshot!("sidebar_docked_160x48", docked);

    let hidden = render(&mut app, 100, 30, &Opts::default());
    assert!(!hidden.contains("Context"), "{hidden}");
    assert!(hidden.starts_with(" cyber · Theme baseline"), "{hidden}");
    insta::assert_snapshot!("sidebar_hidden_100x30", hidden);
    assert!(chord(&mut app, 'b').is_empty());
    assert!(app.sidebar.drawer && app.sidebar.focus == Some(0));
    let drawer = render(&mut app, 100, 30, &Opts::default());
    assert!(drawer.contains("Context"), "{drawer}");
    insta::assert_snapshot!("sidebar_drawer_100x30", drawer);
    assert!(press(&mut app, KeyCode::Esc, KeyModifiers::NONE).is_empty());
    assert!(!app.sidebar.drawer && app.sidebar.focus.is_none());

    let compact = render(&mut app, 50, 20, &Opts::default());
    let footer = compact.lines().nth(19).unwrap();
    assert_eq!(footer.trim(), "100 · —", "{compact}");
    insta::assert_snapshot!("compact_footer_50x20", compact);

    let small = render(&mut app, 30, 8, &Opts::default());
    assert!(small.contains("Terminal too small"), "{small}");
    insta::assert_snapshot!("too_small_30x8", small);
}

#[tokio::test]
async fn shrinking_hides_the_docked_sidebar_and_the_toggle_reopens_it_as_a_drawer() {
    let server = Server::start().await;
    let id = server.answered_session().await;
    let mut app = server.app(&id).await;
    let wide = buffer(&mut app, 140, 30, &Opts::default());
    assert!(text(&wide).contains("Context"));
    let narrow = buffer(&mut app, 100, 30, &Opts::default());
    assert!(!text(&narrow).contains("Context"));
    assert!(text(&narrow).contains(" cyber · Theme baseline"));
    chord(&mut app, 'b');
    assert!(text(&buffer(&mut app, 100, 30, &Opts::default())).contains("Context"));
    // Back on a wide terminal the toggle hides the docked sidebar, and `/sidebar` too.
    press(&mut app, KeyCode::Esc, KeyModifiers::NONE);
    buffer(&mut app, 140, 30, &Opts::default());
    chord(&mut app, 'b');
    assert!(!text(&buffer(&mut app, 140, 30, &Opts::default())).contains("Context"));
    for c in "/sidebar".chars() {
        key(&mut app, c);
    }
    app.completion = None;
    press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
    assert!(text(&buffer(&mut app, 140, 30, &Opts::default())).contains("Context"));
}

/// A sections whose route is unavailable (404/503) is omitted, never shown with
/// placeholders; the parent title shows for a child Session.
#[tokio::test]
async fn unavailable_sources_are_omitted() {
    let server = Server::start().await;
    let id = server.answered_session().await;
    let mut app = server.app(&id).await;
    app.sidebar.lsp = Source::Missing;
    app.sidebar.vcs = Source::Missing;
    app.sidebar.parent_title = Some("Parent work".into());
    let shown = text(&buffer(&mut app, 160, 48, &Opts::default()));
    assert!(!shown.contains("LSP"), "{shown}");
    assert!(shown.contains("↳ Parent work"), "{shown}");
    assert!(shown.contains("~/work/app"), "{shown}");
}

/// Sizes from 40×10 to 300×100: the plan's regions stay inside the screen and apart,
/// and drawing never panics, with and without the sidebar, the drawer and the welcome
/// screen.
#[tokio::test]
async fn random_sizes_never_panic_or_overlap() {
    let server = Server::start().await;
    let id = server.answered_session().await;
    let mut app = server.app(&id).await;
    let empty = server.session().await;
    let mut welcome = server.app(&empty).await;
    let mut seed: u64 = 0x5eed;
    let mut next = |n: u64| {
        seed = seed
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (seed >> 33) % n
    };
    for _ in 0..150 {
        let (width, height) = (40 + next(261) as u16, 10 + next(91) as u16);
        let inputs = crate::view::layout::Inputs {
            docked: next(2) == 0,
            drawer: next(2) == 0,
            screen_reader: false,
            queued: next(6) as u16,
            held: [0, crate::view::messaging_card_height()][next(2) as usize],
            progress: [0, 1, 4][next(3) as usize],
            composer: 4 + next(9) as u16,
        };
        let area = Rect::new(0, 0, width, height);
        let plan = crate::view::layout::plan(area, &inputs);
        let mut regions = vec![
            plan.header,
            plan.transcript,
            plan.queued,
            plan.held,
            plan.progress,
            plan.composer,
            plan.footer,
        ];
        if plan.mode == crate::view::layout::SidebarMode::Docked {
            regions.push(plan.sidebar.unwrap());
        }
        for (i, a) in regions.iter().enumerate() {
            assert!(
                area.contains(a.as_position()) || a.area() == 0,
                "{width}×{height} {a:?}"
            );
            assert!(
                a.right() <= width && a.bottom() <= height,
                "{width}×{height} {a:?}"
            );
            for b in &regions[i + 1..] {
                assert!(
                    a.intersection(*b).area() == 0,
                    "{width}×{height}: {a:?} {b:?}"
                );
            }
        }
        if let (crate::view::layout::SidebarMode::Drawer, Some(drawer)) = (plan.mode, plan.sidebar)
        {
            assert_eq!(
                drawer.intersection(plan.transcript),
                drawer,
                "over the transcript"
            );
        }
        app.sidebar.drawer = inputs.drawer;
        app.sidebar.toggled = !inputs.docked;
        for app in [&mut app, &mut welcome] {
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            terminal.draw(|f| crate::view::draw(f, app)).unwrap();
        }
    }
}

#[tokio::test]
async fn the_welcome_screen_at_each_width_and_without_color() {
    let server = Server::start().await;
    let id = server.session().await;
    let mut app = server.app(&id).await;
    assert!(app.items.is_empty());
    // The default title carries the creation time.
    app.session.title = "New session".into();
    let wide = render(&mut app, 120, 34, &Opts::default());
    assert!(
        wide.contains("██████╗██╗   ██╗██████╗ ███████╗██████╗      ██████╗"),
        "{wide}"
    );
    assert!(wide.contains("1.0.0 · local-first coding agent"), "{wide}");
    assert!(wide.contains("Tips") && wide.contains("ctrl+p  "), "{wide}");
    insta::assert_snapshot!("welcome_120x34", wide);
    let compact = render(&mut app, 60, 30, &Opts::default());
    assert!(
        compact.contains(" ██████╗ ██████╗ ██████╗ ███████╗"),
        "{compact}"
    );
    insta::assert_snapshot!("welcome_60x30", compact);
    let tiny = render(&mut app, 40, 20, &Opts::default());
    assert!(tiny.contains("Cyber Code") && !tiny.contains('█'), "{tiny}");
    insta::assert_snapshot!("welcome_40x20", tiny);
    let no_color = Opts {
        no_color: true,
        ..Opts::default()
    };
    let buf = buffer(&mut app, 120, 34, &no_color);
    for cell in cells(&buf) {
        assert_eq!((cell.fg, cell.bg), (Color::Reset, Color::Reset), "{cell:?}");
    }
    insta::assert_snapshot!(
        "welcome_no_color_120x34",
        render(&mut app, 120, 34, &no_color)
    );

    // The logo is a gradient from `accent` to `secondary` with dimmed shadows.
    let buf = buffer(&mut app, 120, 34, &Opts::default());
    let y = row_of(&buf, "██████╗██╗");
    let first = (0..120).find(|&x| buf[(x, y)].symbol() == "█").unwrap();
    let last = (0..120)
        .rev()
        .find(|&x| buf[(x, y)].symbol() == "█")
        .unwrap();
    let distance = |a: Color, b: Color| match (a, b) {
        (Color::Rgb(r1, g1, b1), Color::Rgb(r2, g2, b2)) => {
            let d = |x: u8, y: u8| (i32::from(x) - i32::from(y)).pow(2);
            d(r1, r2) + d(g1, g2) + d(b1, b2)
        }
        _ => panic!("RGB expected"),
    };
    let (start, end) = (buf[(first, y)].fg, buf[(last, y)].fg);
    let (accent, secondary) = (app.theme.accent, app.theme.secondary);
    assert!(
        distance(start, accent) < distance(start, secondary),
        "{start:?}"
    );
    assert!(distance(end, secondary) < distance(end, accent), "{end:?}");
    let shadow = (first..120).find(|&x| buf[(x, y)].symbol() == "╗").unwrap();
    let background = app.theme.background;
    assert!(
        distance(buf[(shadow, y)].fg, background) < distance(buf[(shadow - 1, y)].fg, background),
        "shadow cells are dimmed toward the background"
    );
    let sixteen = Opts {
        depth: crate::theme::Depth::Ansi16,
        ..Opts::default()
    };
    let buf = buffer(&mut app, 120, 34, &sixteen);
    let block = buf[(first, row_of(&buf, "██████╗██╗"))].fg;
    assert_eq!(block, app.theme.accent, "16 colors use accent only");
}

#[tokio::test]
async fn a_rebound_key_shows_in_the_tips_and_off_shows_nothing_and_the_first_prompt_ends_it() {
    let server = Server::start().await;
    let id = server.session().await;
    let mut app = server.app(&id).await;
    let mut warnings = Vec::new();
    app.keymap = crate::keys::Keymap::load(
        Some(&json!({ "command_palette": "ctrl+k", "help": "none" })),
        &mut warnings,
    );
    let shown = render(&mut app, 120, 34, &Opts::default());
    assert!(shown.contains("ctrl+k    command palette"), "{shown}");
    assert!(
        !shown.contains("ctrl+x h"),
        "a disabled key is not a tip: {shown}"
    );

    app.welcome = crate::view::welcome::Welcome::Compact;
    let compact = render(&mut app, 120, 34, &Opts::default());
    assert!(
        !compact.contains('█') && compact.contains("Agent"),
        "{compact}"
    );

    app.welcome = crate::view::welcome::Welcome::Off;
    let off = render(&mut app, 120, 34, &Opts::default());
    assert!(!off.contains("Agent ") && !off.contains("Tips"), "{off}");

    app.welcome = crate::view::welcome::Welcome::Full;
    app.screen_reader = true;
    let reader = render(&mut app, 120, 34, &Opts::default());
    assert!(
        reader.starts_with(" cyber") || reader.contains("Cyber Code 1.0.0\n"),
        "{reader}"
    );
    assert!(
        reader.contains("Agent: Build") && !reader.contains('█'),
        "{reader}"
    );
    app.screen_reader = false;

    server.prompt(&id, "Hello there").await;
    server.wait_answered(&id).await;
    let msg = crate::perform::snapshot(&server.client, &id).await;
    let follow = crate::runner::apply(&mut app, msg, &mut Default::default());
    server.perform(&mut app, follow).await;
    stabilize(&mut app);
    let buf = buffer(&mut app, 120, 34, &Opts::default());
    let shown = region(&buf, Rect::new(0, 0, 84, 34));
    assert!(!shown.contains("Tips") && !shown.contains('█'), "{shown}");
    assert!(
        shown.starts_with("› Hello there"),
        "the transcript starts at the top: {shown}"
    );
}

/// The end-to-end sidebar runs `/usr/bin/python3` MCP stubs, so it is Unix-only.
#[cfg(unix)]
mod end_to_end {
    use super::*;
    use crate::app::Overlay;
    use crate::sidebar::{Connection, Row, Section};

    /// Wait until `GET /mcp` in `directory` reports every `(name, status)`.
    async fn wait_mcp(server: &Server, directory: &std::path::Path, wanted: &[(&str, &str)]) {
        let client = server.client.at(&directory.display().to_string());
        let mut last = serde_json::Value::Null;
        for _ in 0..500 {
            let status = client.get("/mcp").await.unwrap();
            last = status.clone();
            let servers = status["data"].as_array().cloned().unwrap_or_default();
            let seen = |(name, state): &(&str, &str)| {
                servers
                    .iter()
                    .any(|s| s["name"] == *name && s["status"] == *state)
            };
            if wanted.iter().all(seen) {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        panic!("MCP never reached {wanted:?}: {last}");
    }

    /// A stdio MCP server that answers `initialize` and lists no tools.
    const STUB: &str = "import json,sys\nfor line in sys.stdin:\n    req=json.loads(line)\n    if 'id' not in req: continue\n    if req['method']=='initialize': result={'protocolVersion':'2025-11-25','capabilities':{'tools':{}},'serverInfo':{'name':'stub','version':'1'}}\n    else: result={'tools':[]}\n    print(json.dumps({'jsonrpc':'2.0','id':req['id'],'result':result}),flush=True)\n";

    fn git(dir: &std::path::Path, args: &[&str]) {
        let ok = std::process::Command::new("git")
            .args(["-c", "user.name=t", "-c", "user.email=t@t"])
            .args(["-c", "commit.gpgsign=false"])
            .args(args)
            .current_dir(dir)
            .output()
            .unwrap()
            .status
            .success();
        assert!(ok, "git {args:?}");
    }

    /// The sidebar end to end: a production App with a real stdio MCP stub that connects
    /// and one whose command is missing, a git checkout, and a Turn that writes a file. The
    /// sidebar shows both servers with words, the failed one's error, the LSP sentence, the
    /// modified file and `cwd:branch`; `<leader>e` focuses it, Enter opens the file's diff,
    /// and `r` on the failed server reconnects it through the public route.
    #[tokio::test]
    async fn the_sidebar_end_to_end_with_mcp_git_and_a_modified_file() {
        let temp = tempfile::tempdir().unwrap();
        let bin = temp.path().canonicalize().unwrap();
        // A script that does not exist yet: the launch fails and settles `failed`.
        let late = bin.join("context7.py");
        let mcp = json!({
            "sandbox": { "policy": "full-access" },
            "mcp": {
                "context7": { "type": "local", "command": "/usr/bin/python3", "args": ["-u", late] },
                "local": { "type": "local", "command": "/usr/bin/python3", "args": ["-u", "-c", STUB] },
            },
        });
        let server = Server::start_config(json!({}), mcp, true).await;
        let work = server.root().join("work");
        std::fs::create_dir_all(&work).unwrap();
        git(&work, &["init", "-q", "-b", "main"]);
        git(&work, &["commit", "-q", "--allow-empty", "-m", "start"]);
        let id = server.session_in(&work, "accept-edits").await;
        server.prompt(&id, "WRITE the notes").await;
        server.wait_answered(&id).await;
        wait_mcp(
            &server,
            &work,
            &[("local", "connected"), ("context7", "failed")],
        )
        .await;
        let mut app = server.load(&id).await;
        app.home = Some(server.root().display().to_string());
        app.sidebar.connection = Connection::Connected;

        let names: Vec<(&str, &str)> = app
            .sidebar
            .servers()
            .iter()
            .map(|s| (s.name.as_str(), s.word()))
            .collect();
        assert!(names.contains(&("local", "Connected")) && names.contains(&("context7", "Failed")));
        let error = app
            .sidebar
            .servers()
            .iter()
            .find(|s| s.name == "context7")
            .unwrap();
        let error = error
            .error
            .clone()
            .expect("a failed server reports its error");
        assert_eq!(app.sidebar.lsp, Source::Ready(Vec::new()));
        let files: Vec<&str> = app
            .sidebar
            .changes()
            .iter()
            .map(|c| c.file.as_str())
            .collect();
        assert_eq!(files, ["notes.md"]);

        let buf = buffer(&mut app, 160, 48, &Opts::default());
        let shown = text(&buf);
        assert!(shown.contains("● context7 stdio Failed"), "{shown}");
        let first_words: String = error.chars().take(12).collect();
        assert!(shown.contains(&first_words), "{error} in {shown}");
        assert!(
            shown.contains("● local stdio Connected · 0 tools"),
            "{shown}"
        );
        assert!(
            shown.contains("LSPs will activate as files are read"),
            "{shown}"
        );
        assert!(
            shown.contains("notes.md") && shown.contains("+3 -0"),
            "{shown}"
        );
        assert!(shown.contains("~/work:main"), "{shown}");
        let y = row_of(&buf, "● context7");
        let x = (112..160).find(|&x| buf[(x, y)].symbol() == "●").unwrap();
        assert_eq!(buf[(x, y)].fg, app.theme.error);

        // `<leader>e` focuses the sidebar; Enter on the file opens its diff.
        chord(&mut app, 'e');
        let rows = app.sidebar.rows();
        let file = rows.iter().position(|r| matches!(r, Row::File(0))).unwrap();
        app.sidebar.focus = Some(file - 1);
        key(&mut app, 'j');
        assert_eq!(app.sidebar.focused(), Some(Row::File(0)));
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        assert!(matches!(app.overlay, Overlay::FileDiff { index: 0, .. }));
        let diff = text(&buffer(&mut app, 160, 48, &Opts::default()));
        assert!(diff.contains("+# Notes"), "{diff}");
        press(&mut app, KeyCode::Esc, KeyModifiers::NONE);
        assert!(matches!(app.overlay, Overlay::None) && app.sidebar.focus.is_some());

        // Space on the MCP header collapses it.
        app.sidebar.focus = Some(0);
        key(&mut app, ' ');
        assert!(app.sidebar.collapsed.contains(&Section::Mcp));
        assert!(!text(&buffer(&mut app, 160, 48, &Opts::default())).contains("● local"));
        key(&mut app, ' ');

        // `r` on the failed server reconnects it once its command exists.
        let failed = app
            .sidebar
            .rows()
            .iter()
            .position(|r| matches!(r, Row::Mcp(i) if app.sidebar.servers()[*i].name == "context7"))
            .unwrap();
        app.sidebar.focus = Some(failed);
        std::fs::write(&late, STUB).unwrap();
        let actions = key(&mut app, 'r');
        assert_eq!(actions, vec![Action::ReconnectMcp("context7".into())]);
        server.perform(&mut app, actions).await;
        wait_mcp(&server, &work, &[("context7", "connected")]).await;
        server
            .perform(
                &mut app,
                vec![Action::LoadSidebar(crate::sidebar::Part::Mcp)],
            )
            .await;
        let context7 = app
            .sidebar
            .servers()
            .iter()
            .find(|s| s.name == "context7")
            .unwrap();
        assert_eq!(context7.word(), "Connected");
        press(&mut app, KeyCode::Esc, KeyModifiers::NONE);
        assert!(app.sidebar.focus.is_none());

        stabilize(&mut app);
        app.toast = None;
        insta::assert_snapshot!(
            "sidebar_end_to_end_160x48",
            render(&mut app, 160, 48, &Opts::default())
        );
    }
}
