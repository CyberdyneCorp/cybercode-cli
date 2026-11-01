//! The file explorer (`tui` → File explorer), drawn by the real `view::draw` from a
//! production App's file routes: a git project, a Turn that wrote a file (the Session
//! diff gives the changed-line gutter), and an `.env` behind the default `read` ask rule.

use std::time::Duration;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::buffer::Buffer;
use serde_json::{Value, json};

use crate::app::{Action, App, Overlay, PermStep};
use crate::screens::{Opts, Server, buffer, render, row_of, stabilize};

fn press(app: &mut App, code: KeyCode) -> Vec<Action> {
    app.on_key(KeyEvent::new(code, KeyModifiers::NONE))
}

fn text(buf: &Buffer) -> String {
    (0..buf.area.height)
        .map(|y| {
            (0..buf.area.width)
                .map(|x| buf[(x, y)].symbol())
                .collect::<String>()
                + "\n"
        })
        .collect()
}

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

const MARKDOWN_RS: &str = "//! Markdown.\n\npub fn render(text: &str) -> usize {\n    // count the lines\n    text.lines().count()\n}\n";

/// A production App with snapshots on, and a Session in a git project whose Turn wrote
/// `notes.md`.
struct Fixture {
    server: Server,
    work: std::path::PathBuf,
    id: String,
}

impl Fixture {
    async fn new() -> Self {
        let server = Server::start_config(json!({}), json!({}), true).await;
        let work = server.root().join("work");
        let src = work.join("crates/cyber-tui/src");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::create_dir_all(work.join("target/debug")).unwrap();
        std::fs::write(work.join(".gitignore"), "target/\n").unwrap();
        std::fs::write(src.join("markdown.rs"), "//! Markdown.\n").unwrap();
        std::fs::write(src.join("app.rs"), "pub struct App;\n").unwrap();
        std::fs::write(work.join("README.md"), "# Work\n").unwrap();
        git(&work, &["init", "-q", "-b", "main"]);
        git(&work, &["add", "."]);
        git(&work, &["commit", "-q", "-m", "start"]);
        std::fs::write(src.join("markdown.rs"), MARKDOWN_RS).unwrap();
        std::fs::write(work.join(".env"), "TOKEN=secret\n").unwrap();
        let created = server
            .client
            .at(&work.display().to_string())
            .post(
                "/sessions",
                json!({ "model": "test/main", "mode": "accept-edits" }),
            )
            .await
            .unwrap();
        let id = created["data"]["id"].as_str().unwrap().to_owned();
        server.prompt(&id, "WRITE the notes").await;
        server.wait_answered(&id).await;
        Self { server, work, id }
    }

    async fn app(&self) -> App {
        let mut app = self.server.load(&self.id).await;
        stabilize(&mut app);
        // The real project, so replies match the Session's directory; the footer leaves
        // the temporary path out.
        app.session.directory = self.work.display().to_string();
        app.statusline_fields = vec!["context".into(), "cost".into()];
        app
    }

    /// Perform `actions` in the project as the runner does, until nothing follows.
    async fn run(&self, app: &mut App, mut actions: Vec<Action>) {
        while let Some(action) = actions.pop() {
            let msg = self.perform(app, action).await;
            actions.extend(crate::runner::apply(app, msg, &mut Default::default()));
        }
    }

    async fn perform(&self, app: &App, action: Action) -> Result<crate::perform::Msg, String> {
        crate::perform::perform_owned(&self.server.client, &app.session, action, None).await
    }
}

/// Select the row whose path is `path`.
fn select(app: &mut App, path: &str) {
    let index = app
        .explorer
        .rows()
        .iter()
        .position(|r| r.node.path == path)
        .unwrap_or_else(|| panic!("{path} in {:?}", app.explorer.rows()));
    app.explorer.selected = index;
}

/// Open the explorer and expand `crates/cyber-tui/src`.
async fn browse(f: &Fixture, app: &mut App) {
    let actions = crate::app::App::open_explorer(app);
    f.run(app, actions).await;
    for dir in ["crates", "crates/cyber-tui", "crates/cyber-tui/src"] {
        select(app, dir);
        let actions = press(app, KeyCode::Enter);
        f.run(app, actions).await;
    }
}

#[tokio::test]
async fn browse_and_preview_at_150x36_and_80x30_and_without_color() {
    let f = Fixture::new().await;
    let mut app = f.app().await;
    crate::highlight::preload();
    browse(&f, &mut app).await;
    let opts = Opts {
        legend: &["markdown.rs", "crates"],
        ..Opts::default()
    };
    let shown = render(&mut app, 150, 36, &opts);
    // Root rows have no guide; nested rows get one `│ ` per level before the marker.
    assert!(shown.contains("│ ▾ cyber-tui/ 1"), "{shown}");
    assert!(shown.contains("│ │ ▾ src/ 2"), "{shown}");
    let crates = shown
        .lines()
        .find(|l| l.contains("▾ crates/"))
        .unwrap_or_else(|| panic!("{shown}"));
    assert!(
        !crates.contains("├") && crates.trim_end().ends_with('M'),
        "{crates}"
    );
    assert!(shown.contains("EXPLORER"), "{shown}");
    assert!(shown.contains("⌕ filter files…"), "{shown}");
    assert!(
        !shown.contains("⊘ ask to read"),
        "dot files are hidden by default"
    );

    // The written file: a preview beside the tree with the Session diff's gutter.
    select(&mut app, "notes.md");
    let actions = press(&mut app, KeyCode::Enter);
    assert_eq!(actions, vec![Action::LoadPreview("notes.md".into())]);
    f.run(&mut app, actions).await;
    // Markdown is rendered by default; `m` shows the source with the gutter.
    let buf = buffer(&mut app, 150, 36, &Opts::default());
    let header = row_of(&buf, "notes.md   Markdown · 3 lines");
    assert!(text(&buf).contains("+3 −0"), "{}", text(&buf));
    assert!(text(&buf).contains(" rendered  m raw"), "{}", text(&buf));
    assert!(!text(&buf).contains("# Notes"), "{}", text(&buf));
    assert!(row_of(&buf, "Notes") > header);
    insta::assert_snapshot!("explorer_rendered_150x36", render(&mut app, 150, 36, &opts));
    press(&mut app, KeyCode::Char('m'));
    let buf = buffer(&mut app, 150, 36, &Opts::default());
    let gutter = row_of(&buf, "1▎│ # Notes");
    assert!(gutter > header);
    let x = text(&buf)
        .lines()
        .nth(gutter as usize)
        .unwrap()
        .find("Notes")
        .unwrap();
    let x = text(&buf).lines().nth(gutter as usize).unwrap()[..x]
        .chars()
        .count() as u16;
    assert_eq!(buf[(x, gutter)].fg, app.theme.md_heading);
    insta::assert_snapshot!("explorer_150x36", render(&mut app, 150, 36, &opts));
    let plain = Opts {
        no_color: true,
        ..Opts::default()
    };
    insta::assert_snapshot!(
        "explorer_no_color_150x36",
        render(&mut app, 150, 36, &plain)
    );

    // Below 100 columns Enter replaces the tree with the preview, and Esc returns.
    app.explorer.preview = None;
    buffer(&mut app, 80, 30, &Opts::default());
    select(&mut app, "crates/cyber-tui/src/markdown.rs");
    let actions = press(&mut app, KeyCode::Enter);
    f.run(&mut app, actions).await;
    assert!(app.explorer.preview_focused);
    let opts = Opts {
        legend: &["fn render"],
        ..Opts::default()
    };
    let shown = render(&mut app, 80, 30, &opts);
    assert!(shown.contains("Rust · 6 lines"), "{shown}");
    assert!(!shown.contains("README.md"), "{shown}");
    insta::assert_snapshot!("explorer_preview_80x30", shown);
    let buf = buffer(&mut app, 80, 30, &Opts::default());
    let y = row_of(&buf, "pub fn render");
    let x = text(&buf)
        .lines()
        .nth(y as usize)
        .unwrap()
        .find("pub")
        .unwrap();
    assert_eq!(buf[(x as u16, y)].fg, app.theme.syntax_keyword);
    press(&mut app, KeyCode::Esc);
    assert!(!app.explorer.preview_focused);
    insta::assert_snapshot!(
        "explorer_tree_80x30",
        render(&mut app, 80, 30, &Opts::default())
    );
    press(&mut app, KeyCode::Esc);
    assert!(matches!(app.overlay, Overlay::None));
}

#[tokio::test]
async fn hidden_rows_filter_changed_only_and_expansion_surviving_a_refresh() {
    let f = Fixture::new().await;
    let mut app = f.app().await;
    browse(&f, &mut app).await;
    assert!(
        app.explorer.hidden_count() >= 2,
        "{:?}",
        app.explorer.levels
    );
    // `.` shows `.env` (dimmed, ask to read) and the ignored `target`.
    let actions = press(&mut app, KeyCode::Char('.'));
    f.run(&mut app, actions).await;
    let shown = text(&buffer(&mut app, 150, 36, &Opts::default()));
    assert!(shown.contains(".env  ⊘ ask to read"), "{shown}");
    assert!(shown.contains("target"), "{shown}");
    assert!(app.explorer.expanded.contains("crates/cyber-tui/src"));

    // A refresh keeps the expansion; `g` keeps only changed rows.
    std::fs::write(
        f.work.join("crates/cyber-tui/src/app.rs"),
        "pub struct App2;\n",
    )
    .unwrap();
    let actions = press(&mut app, KeyCode::Char('r'));
    f.run(&mut app, actions).await;
    assert!(app.explorer.expanded.contains("crates/cyber-tui/src"));
    let app_row = app
        .explorer
        .rows()
        .into_iter()
        .find(|r| r.node.path == "crates/cyber-tui/src/app.rs")
        .unwrap();
    assert_eq!(app_row.node.git, Some('M'));
    press(&mut app, KeyCode::Char('g'));
    let names: Vec<String> = app
        .explorer
        .rows()
        .iter()
        .map(|r| r.node.path.clone())
        .collect();
    assert!(!names.iter().any(|n| n == "README.md"), "{names:?}");
    assert!(
        names
            .iter()
            .any(|n| n == "crates/cyber-tui/src/markdown.rs")
    );
    press(&mut app, KeyCode::Char('g'));

    // `/` filters across loaded and server-searched paths.
    press(&mut app, KeyCode::Char('/'));
    let mut actions = Vec::new();
    for c in "mdrs".chars() {
        actions = press(&mut app, KeyCode::Char(c));
    }
    assert_eq!(actions, vec![Action::SearchFiles("mdrs".into())]);
    f.run(&mut app, actions).await;
    assert_eq!(
        app.explorer.current().unwrap().node.path,
        "crates/cyber-tui/src/markdown.rs"
    );
    press(&mut app, KeyCode::Esc);
    assert!(app.explorer.filter.is_empty());

    // Screen readers get each row as a sentence.
    app.screen_reader = true;
    let shown = text(&buffer(&mut app, 150, 36, &Opts::default()));
    assert!(
        shown.contains("markdown.rs, file, modified, level 4"),
        "{shown}"
    );
}

#[tokio::test]
async fn the_mention_key_puts_the_path_in_the_composer_and_closes() {
    let f = Fixture::new().await;
    let mut app = f.app().await;
    browse(&f, &mut app).await;
    select(&mut app, "crates/cyber-tui/src/app.rs");
    press(&mut app, KeyCode::Char('@'));
    assert_eq!(app.composer.text(), "@crates/cyber-tui/src/app.rs ");
    assert!(matches!(app.overlay, Overlay::None));
    // `y` copies the project-relative path.
    let actions = crate::app::App::open_explorer(&mut app);
    f.run(&mut app, actions).await;
    select(&mut app, "README.md");
    assert_eq!(
        press(&mut app, KeyCode::Char('y')),
        vec![Action::Copy("README.md".into())]
    );
}

/// Regression: the explorer mentioned project-root-relative paths, but `@` mentions
/// resolve against the Session's directory, so from a Location below the git root a
/// mention named the wrong file.
#[tokio::test]
async fn a_mention_from_a_location_below_the_project_root_is_relative_to_it() {
    let f = Fixture::new().await;
    let location = f.work.join("crates/cyber-tui");
    let created = f
        .server
        .client
        .at(&location.display().to_string())
        .post(
            "/sessions",
            json!({ "model": "test/main", "mode": "default" }),
        )
        .await
        .unwrap();
    let id = created["data"]["id"].as_str().unwrap().to_owned();
    // The project root lies outside the Location, so listing it asks
    // `external_directory`; approve every request once.
    let approver = {
        let (client, id) = (f.server.client.clone(), id.clone());
        tokio::spawn(async move {
            loop {
                let list = client
                    .get(&format!("/permissions/requests?session_id={id}"))
                    .await
                    .unwrap();
                for request in list["data"].as_array().into_iter().flatten() {
                    let rid = request["id"].as_str().unwrap();
                    let _ = client
                        .post(
                            &format!("/sessions/{id}/permissions/{rid}/reply"),
                            json!({ "reply": "once" }),
                        )
                        .await;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
    };
    let mut app = f.server.load(&id).await;
    stabilize(&mut app);
    app.session.directory = location.display().to_string();
    browse(&f, &mut app).await;
    assert_eq!(
        app.explorer.root.as_deref(),
        Some(f.work.display().to_string().as_str())
    );
    select(&mut app, "crates/cyber-tui/src/app.rs");
    press(&mut app, KeyCode::Char('@'));
    assert_eq!(app.composer.text(), "@src/app.rs ");
    let attached = crate::perform::parts(&app.composer.text(), &app.session.directory);
    assert_eq!(attached.len(), 2, "the mention names a file: {attached:?}");
    assert!(
        attached[1]["text"]
            .as_str()
            .unwrap()
            .contains("pub struct App;"),
        "{attached:?}"
    );

    // A file outside the Location is mentioned by its absolute path.
    app.composer.clear();
    let actions = crate::app::App::open_explorer(&mut app);
    f.run(&mut app, actions).await;
    select(&mut app, "README.md");
    press(&mut app, KeyCode::Char('@'));
    let readme = crate::sidebar::display_path(&f.work.join("README.md").display().to_string())
        .replace('\\', "/");
    assert_eq!(app.composer.text(), format!("@{readme} "));
    let attached = crate::perform::parts(&app.composer.text(), &app.session.directory);
    assert!(
        attached[1]["text"].as_str().unwrap().contains("# Work"),
        "{attached:?}"
    );
    approver.abort();
}

/// The pending permission requests of `id`.
async fn pending(f: &Fixture) -> Vec<Value> {
    let list = f
        .server
        .client
        .get(&format!("/permissions/requests?session_id={}", f.id))
        .await
        .unwrap();
    list["data"].as_array().cloned().unwrap_or_default()
}

async fn wait_asked(f: &Fixture) {
    for _ in 0..500 {
        if !pending(f).await.is_empty() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("no permission request");
}

#[tokio::test]
async fn an_ask_file_fetches_nothing_before_approval_and_a_denial_shows_nothing() {
    let f = Fixture::new().await;
    let mut app = f.app().await;
    let actions = crate::app::App::open_explorer(&mut app);
    f.run(&mut app, actions).await;
    let actions = press(&mut app, KeyCode::Char('.'));
    f.run(&mut app, actions).await;
    select(&mut app, ".env");
    assert_eq!(
        app.explorer.current().unwrap().node.read,
        crate::explorer::Read::Ask
    );

    for approve in [false, true] {
        let actions = press(&mut app, KeyCode::Enter);
        let [Action::LoadPreview(path)] = &actions[..] else {
            panic!("{actions:?}")
        };
        assert_eq!(path, ".env");
        // The content request waits on the server for the approval.
        let session = app.session.clone();
        let client = f.server.client.clone();
        let action = actions[0].clone();
        let fetch = tokio::spawn(async move {
            crate::perform::perform_owned(&client, &session, action, None).await
        });
        wait_asked(&f).await;
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(!fetch.is_finished(), "no content before the reply");
        // The runner's refresh shows the normal permission prompt over the explorer.
        let msg = crate::perform::snapshot(&f.server.client, &f.id).await;
        crate::runner::apply(&mut app, msg, &mut Default::default());
        assert!(matches!(app.overlay, Overlay::Permission(_)));
        let key = if approve {
            KeyCode::Char('y')
        } else {
            KeyCode::Esc
        };
        let reply = press(&mut app, key);
        assert!(matches!(&reply[..], [Action::Reply { .. }]), "{reply:?}");
        assert!(
            matches!(app.overlay, Overlay::Files),
            "back to the explorer"
        );
        for action in reply {
            f.perform(&app, action).await.unwrap();
        }
        let msg = fetch.await.unwrap();
        crate::runner::apply(&mut app, msg, &mut Default::default());
        let preview = app.explorer.preview.as_ref();
        if approve {
            assert_eq!(
                preview.and_then(|p| p.lines.clone()),
                Some(vec!["TOKEN=secret".to_owned()])
            );
        } else {
            assert!(preview.is_none());
            let shown = text(&buffer(&mut app, 150, 36, &Opts::default()));
            assert!(!shown.contains("TOKEN"), "{shown}");
        }
    }
}

/// Wide enough for every footer chip, `esc close` last.
const WIDE: u16 = 160;

/// The column and row where `needle` starts on screen.
fn cell_of(buf: &Buffer, needle: &str) -> (u16, u16) {
    let y = row_of(buf, needle);
    let row: Vec<&str> = (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect();
    let x = (0..row.len())
        .find(|&x| row[x..].concat().starts_with(needle))
        .unwrap_or_else(|| panic!("{needle:?}"));
    (x as u16, y)
}

fn mouse(app: &mut App, kind: MouseEventKind, (column, row): (u16, u16)) -> Vec<Action> {
    app.on_mouse(MouseEvent {
        kind,
        column,
        row,
        modifiers: KeyModifiers::NONE,
    })
}

/// Draw the screen as the runner does before each event, then click `needle` at
/// `ms` on the test clock.
fn click_text(app: &mut App, needle: &str, ms: u64) -> Vec<Action> {
    let at = cell_of(&buffer(app, WIDE, 36, &Opts::default()), needle);
    app.clock = Some(app.started() + Duration::from_millis(ms));
    mouse(app, MouseEventKind::Down(MouseButton::Left), at)
}

fn selected_path(app: &App) -> String {
    app.explorer.current().unwrap().node.path
}

#[tokio::test]
async fn tab_focuses_the_preview_and_esc_returns_to_the_tree() {
    let f = Fixture::new().await;
    let mut app = f.app().await;
    browse(&f, &mut app).await;
    select(&mut app, "crates/cyber-tui/src/markdown.rs");
    buffer(&mut app, 120, 36, &Opts::default());
    let actions = press(&mut app, KeyCode::Enter);
    f.run(&mut app, actions).await;
    assert!(
        !app.explorer.preview_focused,
        "the tree keeps focus at 120 columns"
    );
    let title = |app: &mut App, needle: &str| {
        let buf = buffer(app, 120, 36, &Opts::default());
        buf[cell_of(&buf, needle)].fg
    };
    assert_eq!(title(&mut app, "EXPLORER"), app.theme.accent);
    assert_eq!(title(&mut app, "markdown.rs   Rust"), app.theme.text);

    press(&mut app, KeyCode::Tab);
    assert!(app.explorer.preview_focused);
    assert_eq!(title(&mut app, "EXPLORER"), app.theme.text);
    assert_eq!(title(&mut app, "markdown.rs   Rust"), app.theme.accent);
    // Down scrolls the preview, beside the tree, and the tree selection stays.
    press(&mut app, KeyCode::Down);
    assert_eq!(app.explorer.preview.as_ref().unwrap().scroll, 1);
    assert_eq!(selected_path(&app), "crates/cyber-tui/src/markdown.rs");
    let opts = Opts {
        legend: &["EXPLORER"],
        ..Opts::default()
    };
    let shown = render(&mut app, 120, 36, &opts);
    assert!(
        shown.contains("README.md"),
        "the tree stays beside it: {shown}"
    );
    assert!(shown.contains(" tab  focus "), "{shown}");
    insta::assert_snapshot!("explorer_preview_focused_120x36", shown);

    press(&mut app, KeyCode::Esc);
    assert!(!app.explorer.preview_focused);
    assert!(
        matches!(app.overlay, Overlay::Files),
        "Esc only left the preview"
    );
    press(&mut app, KeyCode::Down);
    assert_eq!(app.explorer.preview.as_ref().unwrap().scroll, 1);
    assert_eq!(
        selected_path(&app),
        "README.md",
        "Down moves the tree again"
    );

    // A click in the preview focuses it, and a click in the tree pane returns.
    click_text(&mut app, "pub fn render", 0);
    assert!(app.explorer.preview_focused);
    click_text(&mut app, "filter files", 1_000);
    assert!(!app.explorer.preview_focused);
    assert_eq!(selected_path(&app), "README.md");
}

#[tokio::test]
async fn clicks_unfold_open_and_toggle() {
    let f = Fixture::new().await;
    let mut app = f.app().await;
    let actions = App::open_explorer(&mut app);
    f.run(&mut app, actions).await;

    // A folded directory unfolds, loading its level from the server.
    let actions = click_text(&mut app, "▸ crates/", 0);
    assert_eq!(
        actions,
        vec![Action::LoadDir {
            path: "crates".into(),
            hidden: false
        }]
    );
    f.run(&mut app, actions).await;
    assert!(app.explorer.expanded.contains("crates"));
    let shown = text(&buffer(&mut app, WIDE, 36, &Opts::default()));
    assert!(shown.contains("│ ▸ cyber-tui/"), "{shown}");

    // A file is selected and its preview opens beside the tree.
    let actions = click_text(&mut app, "README.md", 1_000);
    assert_eq!(actions, vec![Action::LoadPreview("README.md".into())]);
    f.run(&mut app, actions).await;
    assert_eq!(selected_path(&app), "README.md");
    assert_eq!(app.explorer.preview.as_ref().unwrap().path, "README.md");
    assert!(!app.explorer.preview_focused);
    let shown = text(&buffer(&mut app, WIDE, 36, &Opts::default()));
    assert!(shown.contains(" rendered  m raw"), "{shown}");

    // A double click on a directory unfolds it once.
    let actions = click_text(&mut app, "▸ cyber-tui/", 2_000);
    f.run(&mut app, actions).await;
    assert!(app.explorer.expanded.contains("crates/cyber-tui"));
    assert!(click_text(&mut app, "▾ cyber-tui/", 2_300).is_empty());
    assert!(
        app.explorer.expanded.contains("crates/cyber-tui"),
        "still unfolded"
    );
    // A later click folds it again.
    click_text(&mut app, "▾ cyber-tui/", 3_000);
    assert!(!app.explorer.expanded.contains("crates/cyber-tui"));

    // The header chip switches the Markdown preview to its source.
    click_text(&mut app, " rendered ", 4_000);
    assert!(app.explorer.raw);
    let shown = text(&buffer(&mut app, WIDE, 36, &Opts::default()));
    assert!(shown.contains(" raw  m rendered"), "{shown}");
    assert!(shown.contains("1 │ # Work"), "{shown}");

    // The footer's `esc close` chip closes the explorer.
    click_text(&mut app, "esc  close", 5_000);
    assert!(matches!(app.overlay, Overlay::None));
}

#[tokio::test]
async fn a_double_click_in_a_scrolled_tree_unfolds_only_the_clicked_directory() {
    let f = Fixture::new().await;
    for i in 0..40 {
        let dir = f.work.join(format!("d{i:02}"));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("f.txt"), "f\n").unwrap();
    }
    let mut app = f.app().await;
    let actions = App::open_explorer(&mut app);
    f.run(&mut app, actions).await;
    // Scrolled to the bottom: the top rows show a directory far below the first.
    select(&mut app, "README.md");
    let shown = text(&buffer(&mut app, WIDE, 36, &Opts::default()));
    assert!(!shown.contains("▸ d00/"), "scrolled: {shown}");
    let top = (0..40)
        .map(|i| format!("d{i:02}"))
        .find(|dir| shown.contains(&format!("▸ {dir}/")))
        .unwrap();

    let at = cell_of(
        &buffer(&mut app, WIDE, 36, &Opts::default()),
        &format!("▸ {top}/"),
    );
    let left = MouseEventKind::Down(MouseButton::Left);
    app.clock = Some(app.started());
    let actions = mouse(&mut app, left, at);
    assert_eq!(
        actions,
        vec![Action::LoadDir {
            path: top.clone(),
            hidden: false
        }]
    );
    f.run(&mut app, actions).await;
    // The second press, at the same cell, redrawn as the runner does.
    let buf = buffer(&mut app, WIDE, 36, &Opts::default());
    assert_eq!(cell_of(&buf, &format!("▾ {top}/")), at, "the row stays put");
    app.clock = Some(app.started() + Duration::from_millis(300));
    assert!(mouse(&mut app, left, at).is_empty());
    assert_eq!(
        app.explorer.expanded.iter().collect::<Vec<_>>(),
        vec![&top],
        "only the clicked directory unfolded"
    );
    assert_eq!(selected_path(&app), top);
}

#[tokio::test]
async fn a_click_on_an_ask_file_raises_the_permission_request() {
    let f = Fixture::new().await;
    let mut app = f.app().await;
    let actions = App::open_explorer(&mut app);
    f.run(&mut app, actions).await;
    let actions = press(&mut app, KeyCode::Char('.'));
    f.run(&mut app, actions).await;

    let actions = click_text(&mut app, ".env", 0);
    assert_eq!(selected_path(&app), ".env");
    let [Action::LoadPreview(path)] = &actions[..] else {
        panic!("{actions:?}")
    };
    assert_eq!(path, ".env");
    let session = app.session.clone();
    let client = f.server.client.clone();
    let action = actions[0].clone();
    let fetch = tokio::spawn(async move {
        crate::perform::perform_owned(&client, &session, action, None).await
    });
    wait_asked(&f).await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(!fetch.is_finished(), "no content before the reply");
    let msg = crate::perform::snapshot(&f.server.client, &f.id).await;
    crate::runner::apply(&mut app, msg, &mut Default::default());
    assert!(matches!(app.overlay, Overlay::Permission(_)));
    // The tree under the prompt takes no clicks.
    let buf = buffer(&mut app, WIDE, 36, &Opts::default());
    app.clock = Some(app.started() + Duration::from_millis(5_000));
    for _ in 0..2 {
        assert!(
            mouse(
                &mut app,
                MouseEventKind::Down(MouseButton::Left),
                (2, buf.area.height - 1)
            )
            .is_empty()
        );
    }
    assert!(matches!(app.overlay, Overlay::Permission(_)));
    let reply = press(&mut app, KeyCode::Esc);
    for action in reply {
        f.perform(&app, action).await.unwrap();
    }
    let msg = fetch.await.unwrap();
    crate::runner::apply(&mut app, msg, &mut Default::default());
    assert!(app.explorer.preview.is_none());
    let shown = text(&buffer(&mut app, WIDE, 36, &Opts::default()));
    assert!(!shown.contains("TOKEN"), "{shown}");
}

#[tokio::test]
async fn a_triple_click_on_an_ask_file_never_answers_the_prompt_it_raised() {
    let f = Fixture::new().await;
    let mut app = f.app().await;
    let actions = App::open_explorer(&mut app);
    f.run(&mut app, actions).await;
    let actions = press(&mut app, KeyCode::Char('.'));
    f.run(&mut app, actions).await;

    let actions = click_text(&mut app, ".env", 0);
    let [action @ Action::LoadPreview(_)] = &actions[..] else {
        panic!("{actions:?}")
    };
    let session = app.session.clone();
    let client = f.server.client.clone();
    let action = action.clone();
    let fetch = tokio::spawn(async move {
        crate::perform::perform_owned(&client, &session, action, None).await
    });
    wait_asked(&f).await;
    let msg = crate::perform::snapshot(&f.server.client, &f.id).await;
    crate::runner::apply(&mut app, msg, &mut Default::default());
    assert!(matches!(
        app.overlay,
        Overlay::Permission(PermStep::Choose(0))
    ));
    // The rest of the burst lands on the prompt the first press raised.
    for ms in [150, 300] {
        assert!(
            click_text(&mut app, "Allow once (y)", ms).is_empty(),
            "{ms}"
        );
    }
    assert!(matches!(
        app.overlay,
        Overlay::Permission(PermStep::Choose(0))
    ));
    assert_eq!(pending(&f).await.len(), 1, "no reply was sent");

    // A deliberate double click after a pause answers it.
    assert!(click_text(&mut app, "Allow once (y)", 1_000).is_empty());
    let reply = click_text(&mut app, "Allow once (y)", 1_200);
    assert!(matches!(&reply[..], [Action::Reply { .. }]), "{reply:?}");
    for action in reply {
        f.perform(&app, action).await.unwrap();
    }
    let msg = fetch.await.unwrap();
    crate::runner::apply(&mut app, msg, &mut Default::default());
    assert_eq!(
        app.explorer.preview.as_ref().and_then(|p| p.lines.clone()),
        Some(vec!["TOKEN=secret".to_owned()])
    );
}

#[tokio::test]
async fn the_wheel_scrolls_the_pane_under_the_pointer() {
    let f = Fixture::new().await;
    let long: String = (1..=200).map(|n| format!("line {n}\n")).collect();
    std::fs::write(f.work.join("long.txt"), long).unwrap();
    let mut app = f.app().await;
    let actions = App::open_explorer(&mut app);
    f.run(&mut app, actions).await;
    let actions = click_text(&mut app, "long.txt", 0);
    f.run(&mut app, actions).await;
    let buf = buffer(&mut app, WIDE, 36, &Opts::default());
    let preview = cell_of(&buf, "line 1");
    let tree = cell_of(&buf, "README.md");
    let scroll = |app: &App| app.explorer.preview.as_ref().unwrap().scroll;

    let long = app.explorer.selected;
    mouse(&mut app, MouseEventKind::ScrollDown, preview);
    assert_eq!(scroll(&app), super::mouse::WHEEL_LINES);
    assert_eq!(selected_path(&app), "long.txt", "the tree selection stays");
    mouse(&mut app, MouseEventKind::ScrollDown, tree);
    assert_eq!(app.explorer.selected, long + 1, "the next row");
    assert_eq!(scroll(&app), super::mouse::WHEEL_LINES, "the preview stays");
    let buf = buffer(&mut app, WIDE, 36, &Opts::default());
    assert_eq!(row_of(&buf, "4 │ line 4"), preview.1, "the first body row");

    // Whichever pane has focus.
    press(&mut app, KeyCode::Tab);
    assert!(app.explorer.preview_focused);
    mouse(&mut app, MouseEventKind::ScrollUp, tree);
    assert_eq!(selected_path(&app), "long.txt");
    mouse(&mut app, MouseEventKind::ScrollUp, preview);
    assert_eq!(scroll(&app), 0);
    assert_eq!(app.scroll, 0, "the transcript under the explorer stays");
}

#[tokio::test]
async fn hex_workspace_reads_real_binary_bytes_navigates_searches_and_never_starts_inference() {
    let f = Fixture::new().await;
    let bytes: Vec<u8> = (0..=255).collect();
    std::fs::write(f.work.join("fixture.bin"), &bytes).unwrap();
    let mut app = f.app().await;
    let before = f
        .server
        .client
        .get(&format!("/sessions/{}/messages", app.session.id))
        .await
        .unwrap();
    let actions = app.open_hex("fixture.bin");
    f.run(&mut app, actions).await;
    assert!(matches!(app.overlay, Overlay::Hex));
    assert_eq!(app.hex.bytes, bytes);
    assert_eq!(app.hex.document.as_ref().unwrap().length, 256);
    for (name, width, height, opts) in [
        ("hex_wide", 140, 36, Opts::default()),
        ("hex_narrow", 60, 28, Opts::default()),
        (
            "hex_light",
            100,
            30,
            Opts {
                light: true,
                ..Opts::default()
            },
        ),
        (
            "hex_no_color",
            100,
            30,
            Opts {
                no_color: true,
                ..Opts::default()
            },
        ),
    ] {
        let shown = render(&mut app, width, height, &opts);
        assert!(shown.contains("LIVE VIEW"));
        assert!(shown.contains("INSPECTOR LE"));
        assert!(!shown.contains("\u{1b}"));
        let revision = app
            .hex
            .document
            .as_ref()
            .unwrap()
            .revision
            .chars()
            .take(12)
            .collect::<String>();
        insta::assert_snapshot!(name, shown.replace(&revision, "obs_revision"));
    }
    press(&mut app, KeyCode::Char('g'));
    for c in "0x10".chars() {
        press(&mut app, KeyCode::Char(c));
    }
    let actions = press(&mut app, KeyCode::Enter);
    f.run(&mut app, actions).await;
    assert_eq!(app.hex.cursor, 16);
    assert_eq!(
        press(&mut app, KeyCode::Char('c')),
        vec![Action::Copy("10".into())]
    );
    press(&mut app, KeyCode::Char('b'));
    for c in "header".chars() {
        press(&mut app, KeyCode::Char(c));
    }
    press(&mut app, KeyCode::Enter);
    assert_eq!(app.hex.bookmarks, [("header".into(), 16)]);
    press(&mut app, KeyCode::Char('/'));
    for c in "202122".chars() {
        press(&mut app, KeyCode::Char(c));
    }
    let actions = press(&mut app, KeyCode::Enter);
    f.run(&mut app, actions).await;
    assert_eq!(app.hex.cursor, 32);
    assert_eq!(app.hex.found, [32]);
    assert_eq!(std::fs::read(f.work.join("fixture.bin")).unwrap(), bytes);
    let after = f
        .server
        .client
        .get(&format!("/sessions/{}/messages", app.session.id))
        .await
        .unwrap();
    assert_eq!(
        before["data"], after["data"],
        "manual byte operations do not start inference"
    );
    let doc = app.hex.document.as_ref().unwrap().id.clone();
    let actions = press(&mut app, KeyCode::Esc);
    f.run(&mut app, actions).await;
    assert!(app.hex.document.is_none());
    let error = f
        .server
        .client
        .at(&app.session.directory)
        .get(&format!(
            "/files/binary/documents/{doc}/bytes?session_id={}&revision=x",
            app.session.id
        ))
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        cyber_client::ClientError::Api { status: 404, .. }
    ));
}

#[tokio::test]
async fn hex_stale_open_is_closed_and_session_switch_forgets_private_bytes() {
    let f = Fixture::new().await;
    std::fs::write(f.work.join("fixture.bin"), [0, 255, 27]).unwrap();
    let mut app = f.app().await;
    let mut actions = app.open_hex("fixture.bin");
    let pending = f.perform(&app, actions.pop().unwrap()).await;
    press(&mut app, KeyCode::Esc);
    let cleanup = crate::runner::apply(&mut app, pending, &mut Default::default());
    assert!(matches!(
        &cleanup[..],
        [Action::Hex(crate::hex::Request::Close { .. })]
    ));
    f.run(&mut app, cleanup).await;
    assert!(app.hex.bytes.is_empty());
    let actions = app.open_hex("fixture.bin");
    f.run(&mut app, actions).await;
    let old = app.session.clone();
    let mut other = old.clone();
    other.id = "ses_other".into();
    app.set_session(other);
    assert!(app.hex.bytes.is_empty());
    assert!(app.hex.document.is_none());
    assert!(matches!(app.overlay, Overlay::None));
    let requests = app.hex.pending_closes.drain(..).map(Action::Hex).collect();
    f.run(&mut app, requests).await;
    app.set_session(old);
    for _ in 0..5 {
        let actions = app.open_hex("fixture.bin");
        f.run(&mut app, actions).await;
        assert!(app.hex.error.is_none());
        let actions = press(&mut app, KeyCode::Esc);
        f.run(&mut app, actions).await;
    }
}
