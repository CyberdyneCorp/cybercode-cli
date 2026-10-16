//! Markdown and syntax-highlighting screens (`tui` → Markdown rendering,
//! Syntax-highlighted code blocks), drawn by the real `view::draw` from Sessions a
//! production App answered with a local scripted model.

use std::time::Duration;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use futures::StreamExt;
use ratatui::buffer::Buffer;
use ratatui::style::{Color, Modifier};
use serde_json::json;

use crate::app::{Action, App};
use crate::model::{Item, Tool};
use crate::screens::{Opts, Server, buffer, cells, render, row_of};

/// The cell where `needle` starts on its row.
fn cell<'b>(buf: &'b Buffer, needle: &str) -> &'b ratatui::buffer::Cell {
    let y = row_of(buf, needle);
    let row: Vec<&str> = (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect();
    let first = needle.chars().next().unwrap().to_string();
    let x = (0..row.len())
        .find(|&x| row[x] == first && row[x..].concat().starts_with(needle))
        .unwrap();
    &buf[(x as u16, y)]
}

fn screen_text(buf: &Buffer) -> String {
    (0..buf.area.height)
        .map(|y| {
            (0..buf.area.width)
                .map(|x| buf[(x, y)].symbol())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn press(app: &mut App, code: KeyCode, modifiers: KeyModifiers) -> Vec<Action> {
    app.on_key(KeyEvent::new(code, modifiers))
}

#[tokio::test]
async fn the_reference_answer_renders_as_markdown_with_highlighted_python() {
    let server = Server::start().await;
    let id = server.answered("REFERENCE answer please").await;
    let mut app = server.app(&id).await;
    for (width, height) in [(80, 40), (120, 40)] {
        let opts = Opts {
            legend: &["# Build", "for i in", "1. Overlapping", "• Time"],
            ..Opts::default()
        };
        insta::assert_snapshot!(
            format!("reference_{width}x{height}"),
            render(&mut app, width, height, &opts)
        );
    }
    let buf = buffer(&mut app, 120, 40, &Opts::default());
    let t = app.theme.clone();
    let comment = cell(&buf, "# Build up");
    assert_eq!(comment.fg, t.syntax_comment);
    assert!(comment.modifier.contains(Modifier::ITALIC));
    assert_eq!(comment.bg, t.surface);
    assert_eq!(cell(&buf, "for i in").fg, t.syntax_keyword);
    assert_eq!(cell(&buf, "range(").fg, t.syntax_function);
    assert_eq!(cell(&buf, "python").fg, t.text_subtle);
    let term = cell(&buf, "Overlapping Subproblems:");
    assert!(term.modifier.contains(Modifier::BOLD));
    assert_eq!(cell(&buf, "1. Overlapping").fg, t.text_muted);
    let shown = screen_text(&buf);
    assert!(!shown.contains("**") && !shown.contains("```"), "{shown}");
    assert!(shown.contains("  • Time complexity"));
    assert!(shown.contains("the guide (https://example.com/dp)"));

    let opts = Opts {
        no_color: true,
        ..Opts::default()
    };
    let buf = buffer(&mut app, 80, 40, &opts);
    for c in cells(&buf) {
        assert_eq!((c.fg, c.bg), (Color::Reset, Color::Reset), "{c:?}");
    }
    assert!(
        cell(&buf, "Overlapping Subproblems:")
            .modifier
            .contains(Modifier::BOLD)
    );
    insta::assert_snapshot!("reference_no_color_80x40", render(&mut app, 80, 40, &opts));
}

#[tokio::test]
async fn a_wide_table_falls_back_and_an_unknown_language_renders_plain() {
    let server = Server::start().await;
    let id = server.answered("TABLE please").await;
    let mut app = server.app(&id).await;
    let opts = Opts {
        legend: &["qapla"],
        ..Opts::default()
    };
    let shown = render(&mut app, 80, 24, &opts);
    assert!(
        shown.contains("  Column one: the first value cell"),
        "{shown}"
    );
    assert!(
        shown.contains("  Column six: the sixth value cell"),
        "{shown}"
    );
    assert!(!shown.contains('┬'), "{shown}");
    insta::assert_snapshot!("table_fallback_80x24", shown);
    let buf = buffer(&mut app, 80, 24, &Opts::default());
    assert_eq!(cell(&buf, "qapla' batlh").fg, app.theme.md_code);
    assert_eq!(cell(&buf, "klingon").fg, app.theme.text_subtle);

    // The whole width for the transcript: the table needs about 140 columns.
    app.sidebar.preference = crate::sidebar::Preference::Hide;
    let wide = screen_text(&buffer(&mut app, 160, 24, &Opts::default()));
    assert!(
        wide.contains("┬") && wide.contains("│ the first value cell │"),
        "{wide}"
    );
}

/// The production server's own text deltas, delivered in pieces around the closing
/// fence: the block stays `md_code` while it is open, highlights when the fence arrives,
/// and never changes its row count, through to the durable message after reload.
#[tokio::test]
async fn a_streaming_code_block_stays_plain_until_its_fence_closes() {
    crate::highlight::preload();
    let server = Server::start().await;
    let id = server.session().await;
    let mut app = server.app(&id).await;
    let mut events = Box::pin(server.client.events(false).await.unwrap());
    server.prompt(&id, "How do I speed up fib?").await;
    let mut deltas = Vec::new();
    loop {
        let event = tokio::time::timeout(Duration::from_secs(10), events.next())
            .await
            .unwrap()
            .unwrap();
        if event.session_id.as_deref() != Some(id.as_str()) {
            continue;
        }
        if event.kind == "session.text.delta" {
            deltas.push(event);
        } else if event.kind == "session.idle" {
            break;
        } else {
            app.on_server_event(&event);
        }
    }
    let text: String = deltas
        .iter()
        .map(|e| e.data["text"].as_str().unwrap())
        .collect();
    let close = text.rfind("```").unwrap();
    let template = deltas[0].clone();
    let deliver = |app: &mut App, piece: &str| {
        let mut event = template.clone();
        event.data["text"] = json!(piece);
        app.on_server_event(&event);
        crate::view::transcript(app, 79)
    };
    let style = |r: &crate::markdown::Rendered, needle: &str| {
        r.lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .find(|s| s.content.starts_with(needle))
            .unwrap()
            .style
    };
    let t = app.theme.clone();
    let open = deliver(&mut app, &text[..close]);
    assert_eq!(style(&open, "fn fib").fg, Some(t.md_code));
    for _ in 0..2 {
        let partial = deliver(&mut app, "`");
        assert_eq!(partial.lines.len(), open.lines.len());
        assert_eq!(style(&partial, "fn fib").fg, Some(t.md_code));
    }
    let closed = deliver(&mut app, "`");
    assert_eq!(closed.lines.len(), open.lines.len());
    assert_eq!(style(&closed, "fn").fg, Some(t.syntax_keyword));
    let finished = deliver(&mut app, &text[close + 3..]);
    assert_eq!(finished.lines.len(), open.lines.len());

    let msg = crate::perform::snapshot(&server.client, &id).await;
    crate::runner::apply(&mut app, msg, &mut Default::default());
    let durable = crate::view::transcript(&app, 79);
    let rows = |r: &crate::markdown::Rendered| -> Vec<String> {
        r.lines.iter().map(|l| l.to_string()).collect()
    };
    let streamed = rows(&finished);
    let reloaded = rows(&durable);
    let body = &streamed[..streamed.len() - 1];
    assert!(
        reloaded.windows(body.len()).any(|w| w == body),
        "{streamed:#?}\n{reloaded:#?}"
    );
    assert_eq!(style(&durable, "fn").fg, Some(t.syntax_keyword));
}

#[tokio::test]
async fn ctrl_x_y_selects_a_code_block_and_y_copies_it_without_padding() {
    let server = Server::start().await;
    let id = server.answered("REFERENCE answer please").await;
    let mut app = server.app(&id).await;
    assert!(press(&mut app, KeyCode::Char('x'), KeyModifiers::CONTROL).is_empty());
    assert!(press(&mut app, KeyCode::Char('y'), KeyModifiers::NONE).is_empty());
    assert_eq!(app.code_select, Some(0));
    let buf = buffer(&mut app, 120, 40, &Opts::default());
    let marker = cell(&buf, "› python");
    assert_eq!(marker.bg, app.theme.selection);
    let code = "def fib(n):\n    # Build up solution iteratively\n    dp = [0, 1]\n    for i in range(2, n + 1):\n        dp.append(dp[i - 1] + dp[i - 2])\n    return dp[n]";
    assert_eq!(
        press(&mut app, KeyCode::Char('y'), KeyModifiers::NONE),
        vec![Action::Copy(code.into())]
    );
    assert_eq!(app.code_select, None);
    assert_eq!(app.composer.text(), "", "y never reaches the composer");
    assert!(!screen_text(&buffer(&mut app, 120, 40, &Opts::default())).contains("› python"));
    assert_eq!(app.copy_last_code_block(), vec![Action::Copy(code.into())]);

    // Esc leaves the selection; other keys leave it and type.
    press(&mut app, KeyCode::Char('x'), KeyModifiers::CONTROL);
    press(&mut app, KeyCode::Char('y'), KeyModifiers::NONE);
    press(&mut app, KeyCode::Char('h'), KeyModifiers::NONE);
    assert_eq!(
        (app.code_select, app.composer.text()),
        (None, "h".to_owned())
    );

    // Without any code block there is nothing to select.
    let plain = server.answered("TABLE please").await;
    let mut other = server.app(&plain).await;
    other.items.retain(|i| !matches!(i, Item::Assistant { .. }));
    press(&mut other, KeyCode::Char('x'), KeyModifiers::CONTROL);
    press(&mut other, KeyCode::Char('y'), KeyModifiers::NONE);
    assert_eq!(other.code_select, None);
    assert_eq!(
        other.toast.as_ref().map(|t| t.0.as_str()),
        Some("No code block to copy")
    );
}

#[tokio::test]
async fn links_become_osc8_hyperlinks_where_the_terminal_supports_them() {
    let server = Server::start().await;
    let id = server.answered("REFERENCE answer please").await;
    let mut app = server.app(&id).await;
    app.hyperlinks = true;
    let buf = buffer(&mut app, 120, 40, &Opts::default());
    let shown = screen_text(&buf);
    assert!(!shown.contains("(https://example.com/dp)"), "{shown}");
    let links = app.take_screen_links();
    assert_eq!(links.len(), 1, "{links:?}");
    assert_eq!(
        (links[0].text.as_str(), links[0].url.as_str()),
        ("the guide", "https://example.com/dp")
    );
    assert_eq!(buf[(links[0].x, links[0].y)].symbol(), "t");
    let mut out = Vec::new();
    crate::osc::write_hyperlinks(&mut out, &buf, &links).unwrap();
    let written = String::from_utf8(out).unwrap();
    assert!(written.contains("\x1b]8;;https://example.com/dp\x1b\\the guide\x1b]8;;\x1b\\"));
}

#[tokio::test]
async fn read_and_write_tool_cards_highlight_by_file_extension() {
    let server = Server::start().await;
    std::fs::write(
        server.root().join("fib.py"),
        "# fib\ndef fib(n):\n    return n\n",
    )
    .unwrap();
    let id = server.answered("READ fib.py").await;
    let mut app = server.app(&id).await;
    let buf = buffer(&mut app, 120, 40, &Opts::default());
    let t = app.theme.clone();
    let shown = screen_text(&buf);
    assert!(shown.contains("│ 1 # fib"), "{shown}");
    assert_eq!(cell(&buf, "# fib").fg, t.syntax_comment);
    assert_eq!(cell(&buf, "def fib").fg, t.syntax_keyword);
    assert_eq!(cell(&buf, "1 # fib").fg, t.text_subtle);

    let write = Tool {
        call_id: "call_write".into(),
        name: "write".into(),
        input: json!({ "path": "src/lib.rs", "content": "fn main() {}\n" }),
        status: "ok".into(),
        output: "Created src/lib.rs".into(),
    };
    if let Some(Item::Assistant { tools, .. }) = app
        .items
        .iter_mut()
        .rev()
        .find(|i| matches!(i, Item::Assistant { .. }))
    {
        tools.push(write);
    }
    let buf = buffer(&mut app, 120, 40, &Opts::default());
    assert_eq!(cell(&buf, "Created src/lib.rs").fg, t.text_muted);
    assert_eq!(cell(&buf, "fn main").fg, t.syntax_keyword);
}
