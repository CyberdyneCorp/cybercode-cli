//! The file explorer (`tui` → File explorer): a tree pane (header bar with the project,
//! branch and change state, the filter line, the tree with indent guides, fold markers,
//! kind colors and sizes, and a summary), a divider, the preview pane beside it (or
//! instead of it below 100 columns) with its own header bar, and the key chips from the
//! binding table. Rows, panes and chips register their click targets as they are drawn.

use crossterm::event::KeyEvent;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph};
use unicode_width::UnicodeWidthStr;

use super::layout::truncate_end;
use crate::app::App;
use crate::explorer::{Kind, Node, Preview, Read, Row, SIDE_BY_SIDE};
use crate::mouse::{Act, List, Pane, Target};
use crate::theme::Tokens;

pub fn draw(f: &mut Frame, app: &App, area: Rect) {
    let t = &app.theme;
    f.render_widget(Clear, area);
    f.render_widget(
        Block::default().style(Style::default().fg(t.text).bg(t.background)),
        area,
    );
    if area.width < 10 || area.height < 5 {
        return;
    }
    let e = &app.explorer;
    let footer = Rect {
        y: area.y + area.height - 1,
        height: 1,
        ..area
    };
    f.render_widget(Paragraph::new(keys(app, footer)), footer);
    let body = Rect {
        height: area.height - 1,
        ..area
    };
    match &e.preview {
        Some(preview) if e.preview_focused && area.width < SIDE_BY_SIDE => {
            draw_preview(f, app, preview, body)
        }
        Some(preview) if area.width >= SIDE_BY_SIDE => {
            let tree_width = (body.width * 2 / 5).max(30);
            draw_tree_pane(
                f,
                app,
                Rect {
                    width: tree_width,
                    ..body
                },
            );
            let divider = Rect {
                x: body.x + tree_width,
                width: 1,
                ..body
            };
            let rule = vec![Line::from("│"); divider.height as usize];
            f.render_widget(
                Paragraph::new(rule).style(Style::default().fg(t.border).bg(t.background)),
                divider,
            );
            let side = Rect {
                x: body.x + tree_width + 1,
                width: body.width.saturating_sub(tree_width + 1),
                ..body
            };
            draw_preview(f, app, preview, side);
        }
        _ => draw_tree_pane(f, app, body),
    }
}

fn one_row(area: Rect, y: u16) -> Rect {
    Rect {
        y: area.y + y,
        height: 1,
        ..area
    }
}

/// `spans` with `right` at the right edge, padded with `bg` to `width`.
fn bar(
    left: Vec<Span<'static>>,
    right: Vec<Span<'static>>,
    width: usize,
    bg: Color,
) -> Line<'static> {
    let used: usize = left.iter().chain(&right).map(Span::width).sum();
    let mut spans = left;
    if used < width {
        spans.push(Span::raw(" ".repeat(width - used)));
    }
    spans.extend(right);
    Line::from(spans).style(Style::default().bg(bg))
}

fn draw_tree_pane(f: &mut Frame, app: &App, area: Rect) {
    let t = &app.theme;
    app.mouse.push(area, Target::Pane(Pane::ExplorerTree));
    f.render_widget(
        Block::default().style(Style::default().fg(t.text).bg(t.surface_alt)),
        area,
    );
    let width = area.width as usize;
    f.render_widget(Paragraph::new(header(app, width)), one_row(area, 0));
    f.render_widget(Paragraph::new(filter_line(app)), one_row(area, 1));
    if area.height < 5 {
        return;
    }
    let tree = Rect {
        y: area.y + 3,
        height: area.height - 4,
        ..area
    };
    draw_tree(f, app, tree);
    f.render_widget(
        Paragraph::new(Span::styled(
            truncate_end(&summary(app), width),
            Style::default().fg(t.text_subtle),
        )),
        one_row(area, area.height - 1),
    );
}

/// ` EXPLORER app:main` with `✓ clean` or `3 changed` at the right.
fn header(app: &App, width: usize) -> Line<'static> {
    let t = &app.theme;
    let directory = crate::sidebar::display_path(&app.session.directory);
    let project = directory
        .trim_end_matches(['/', '\\'])
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or_default()
        .to_owned();
    let bold = Modifier::BOLD;
    let mut left = vec![
        Span::styled(
            " EXPLORER ",
            Style::default()
                .fg(title_color(t, !app.explorer.preview_focused))
                .add_modifier(bold),
        ),
        Span::styled(project, Style::default().fg(t.text).add_modifier(bold)),
    ];
    if let Some(branch) = app
        .sidebar
        .vcs
        .ready()
        .and_then(|git| git.as_ref())
        .and_then(|git| git.branch.clone())
    {
        left.push(Span::styled(":", Style::default().fg(t.text_muted)));
        left.push(Span::styled(
            branch,
            Style::default().fg(t.secondary).add_modifier(bold),
        ));
    }
    let right = match app.explorer.changed {
        Some(0) => vec![Span::styled("✓ clean ", Style::default().fg(t.success))],
        Some(n) => vec![Span::styled(
            format!("{n} changed "),
            Style::default().fg(t.warning),
        )],
        None => Vec::new(),
    };
    bar(left, right, width, t.surface)
}

fn filter_line(app: &App) -> Line<'static> {
    let t = &app.theme;
    let e = &app.explorer;
    let glass = Span::styled(" ⌕ ", Style::default().fg(t.text_muted));
    if e.filter.is_empty() && !e.filtering {
        return Line::from(vec![
            glass,
            Span::styled(
                "filter files…",
                Style::default()
                    .fg(t.text_subtle)
                    .add_modifier(Modifier::ITALIC),
            ),
        ]);
    }
    let cursor = if e.filtering { "▏" } else { "" };
    Line::from(vec![
        Span::styled(" ⌕ ", Style::default().fg(t.accent)),
        Span::styled(
            format!("{}{cursor}", crate::sidebar::clean(&e.filter)),
            Style::default().fg(t.text),
        ),
    ])
}

/// `19 shown · 8 hidden · .gitignore`, or the filter's match count.
fn summary(app: &App) -> String {
    let e = &app.explorer;
    let rows = e.rows().len();
    if !e.filter.is_empty() {
        return format!(" {rows} matches");
    }
    let hidden = e.hidden_count();
    let mut parts = vec![format!(" {rows} shown")];
    if e.hidden {
        parts.push("hidden and ignored shown".into());
    } else if hidden > 0 {
        parts.push(format!("{hidden} hidden"));
        parts.push(".gitignore".into());
    }
    if e.changed_only {
        parts.push("changed only".into());
    }
    parts.join(" · ")
}

/// A key label as the chips show it: arrows as glyphs.
fn glyph(label: &str) -> String {
    match label {
        "up" => "↑".into(),
        "down" => "↓".into(),
        "left" => "←".into(),
        "right" => "→".into(),
        other => other.to_owned(),
    }
}

/// A pane header's title: `accent` while the pane has focus.
fn title_color(t: &Tokens, focused: bool) -> Color {
    if focused { t.accent } else { t.text }
}

/// A footer chip: its keys, its label, and the key a click on it presses (none for a
/// pair of arrows).
type Chip = (String, &'static str, Option<KeyEvent>);

/// Chips from the binding table: paired arrows, then single actions.
fn chips(app: &App) -> Vec<Chip> {
    let label = |action: &str| app.keymap.label("explorer", action).map(|l| glyph(&l));
    let mut out = Vec::new();
    for (first, second, what) in [
        ("explorer_previous", "explorer_next", "move"),
        ("explorer_collapse", "explorer_expand", "fold"),
    ] {
        if let (Some(a), Some(b)) = (label(first), label(second)) {
            out.push((format!("{a}{b}"), what, None));
        }
    }
    for (action, what) in [
        ("explorer_open", "open"),
        ("explorer_focus", "focus"),
        ("explorer_filter", "filter"),
        ("explorer_changed", "changed"),
        ("explorer_hidden", "hidden"),
        ("explorer_markdown", "raw/rendered"),
        ("explorer_mention", "mention"),
        ("explorer_copy", "copy path"),
        ("explorer_refresh", "refresh"),
        ("explorer_close", "close"),
    ] {
        if let Some(key) = label(action) {
            out.push((key, what, app.keymap.event("explorer", action)));
        }
    }
    out
}

/// The explorer keys as chips that fit the footer row `area`; a click on a chip
/// presses its key.
fn keys(app: &App, area: Rect) -> Line<'static> {
    let t = &app.theme;
    let chip = if t.monochrome {
        Style::default().add_modifier(Modifier::REVERSED | Modifier::BOLD)
    } else {
        Style::default()
            .fg(t.background)
            .bg(t.text_muted)
            .add_modifier(Modifier::BOLD)
    };
    let mut spans = vec![Span::raw(" ")];
    let mut used = 1;
    for (key, what, event) in chips(app) {
        let size = key.width() + what.width() + 4;
        if used + size > area.width as usize {
            break;
        }
        if let Some(event) = event {
            let x = area.x + used as u16;
            app.mouse
                .push(Rect::new(x, area.y, size as u16, 1), Target::Key(event));
        }
        used += size;
        spans.push(Span::styled(format!(" {key} "), chip));
        spans.push(Span::styled(
            format!(" {what} "),
            Style::default().fg(t.text_muted),
        ));
    }
    Line::from(spans)
}

fn git_color(t: &Tokens, letter: char) -> Color {
    match letter {
        'M' => t.warning,
        'A' | '?' => t.success,
        _ => t.error,
    }
}

fn draw_tree(f: &mut Frame, app: &App, area: Rect) {
    let e = &app.explorer;
    let rows = e.rows();
    if rows.is_empty() {
        let text = if e.levels.is_empty() {
            " Loading…"
        } else {
            " Nothing to show"
        };
        f.render_widget(
            Paragraph::new(Span::styled(
                text,
                Style::default().fg(app.theme.text_muted),
            )),
            area,
        );
        return;
    }
    let height = area.height as usize;
    let selected = e.selected..e.selected + 1;
    let top = app
        .mouse
        .window(List::Explorer, selected, height, rows.len());
    for (y, index) in (top..rows.len().min(top + height)).enumerate() {
        let target = Target::Row {
            list: List::Explorer,
            index,
            act: Act::First,
        };
        app.mouse.push(one_row(area, y as u16), target);
    }
    let lines: Vec<Line> = rows
        .iter()
        .enumerate()
        .skip(top)
        .take(height)
        .map(|(i, row)| tree_row(app, row, i == e.selected, area.width as usize))
        .collect();
    f.render_widget(Paragraph::new(lines), area);
}

/// `<name>, directory|file, <status>, level <n>` for screen readers.
fn spoken(row: &Row) -> String {
    let n = &row.node;
    let status = match n.git {
        Some('M') => "modified",
        Some('A') => "added",
        Some('?') => "untracked",
        Some('D') => "deleted",
        _ => "unchanged",
    };
    let kind = if n.dir { "directory" } else { "file" };
    format!("{}, {kind}, {status}, level {}", n.name, row.depth + 1)
}

fn kind_color(t: &Tokens, kind: Kind) -> Color {
    match kind {
        Kind::Dir => t.file_dir,
        Kind::Doc => t.file_doc,
        Kind::Config => t.file_config,
        Kind::Code => t.file_code,
        Kind::Dim => t.file_dim,
        Kind::Plain => t.text,
    }
}

/// A Nerd Font glyph for `node` when `tui.explorer.icons` is `nerd`.
fn icon(node: &Node, open: bool) -> &'static str {
    if node.dir {
        return if open { "\u{f07c}" } else { "\u{f07b}" };
    }
    let ext = node.name.rsplit_once('.').map_or("", |(_, e)| e);
    match ext.to_ascii_lowercase().as_str() {
        "rs" => "\u{e7a8}",
        "md" | "mdx" | "markdown" => "\u{f48a}",
        "toml" | "json" | "jsonc" | "yaml" | "yml" => "\u{e615}",
        "lock" => "\u{f023}",
        "js" | "mjs" | "cjs" => "\u{e74e}",
        "ts" | "tsx" => "\u{e628}",
        "py" => "\u{e73c}",
        "sh" | "bash" | "zsh" => "\u{f489}",
        _ => "\u{f15b}",
    }
}

/// The row's name style: kind color, dimmed when not readable, bold for directories.
fn name_style(app: &App, node: &Node, selected: bool) -> Style {
    let t = &app.theme;
    let kind = if node.read == Read::Allow {
        crate::explorer::kind(node)
    } else {
        Kind::Dim
    };
    let mut style = Style::default().fg(kind_color(t, kind));
    if node.dir || selected {
        style = style.add_modifier(Modifier::BOLD);
    }
    style
}

/// The left part of a row: accent bar, guides, fold marker, icon, name and extras.
fn row_left(app: &App, row: &Row, selected: bool) -> Vec<Span<'static>> {
    let t = &app.theme;
    let n = &row.node;
    let open = app.explorer.expanded.contains(&n.path);
    let mut spans = vec![if selected {
        Span::styled("▌", Style::default().fg(t.accent))
    } else {
        Span::raw(" ")
    }];
    spans.push(Span::raw(" "));
    spans.push(Span::styled(
        row.guides.clone(),
        Style::default().fg(t.text_subtle),
    ));
    let marker = match (n.dir, open) {
        (true, true) => "▾ ",
        (true, false) => "▸ ",
        _ => "  ",
    };
    spans.push(Span::styled(marker, Style::default().fg(t.text_muted)));
    let style = name_style(app, n, selected);
    if app.explorer.icons == crate::explorer::Icons::Nerd && !t.monochrome {
        spans.push(Span::styled(format!("{} ", icon(n, open)), style));
    }
    let suffix = if n.dir { "/" } else { "" };
    spans.push(Span::styled(format!("{}{suffix}", n.name), style));
    if let Some(count) = app.explorer.child_count(&n.path) {
        spans.push(Span::styled(
            format!(" {count}"),
            Style::default().fg(t.text_muted),
        ));
    }
    if let Some(target) = &n.target {
        spans.push(Span::styled(
            format!(" → {target}"),
            Style::default().fg(t.text_subtle),
        ));
    }
    let note = match n.read {
        Read::Ask if !n.dir => "  ⊘ ask to read",
        Read::Deny => "  ⊘ denied",
        _ => "",
    };
    if !note.is_empty() {
        spans.push(Span::styled(note, Style::default().fg(t.text_subtle)));
    }
    spans
}

/// The right part of a row: the git letter and the size.
fn row_right(app: &App, n: &Node) -> Vec<Span<'static>> {
    let t = &app.theme;
    let mut spans = Vec::new();
    if let Some(c) = n.git {
        spans.push(Span::styled(
            format!("{c} "),
            Style::default()
                .fg(git_color(t, c))
                .add_modifier(Modifier::BOLD),
        ));
    }
    if !n.dir {
        spans.push(Span::styled(
            format!("{:>5} ", compact(n.size)),
            Style::default().fg(t.text_muted),
        ));
    }
    spans
}

/// `left` truncated so `right` fits, padded to `width`.
fn fit(
    mut left: Vec<Span<'static>>,
    right: Vec<Span<'static>>,
    width: usize,
) -> Vec<Span<'static>> {
    let right_width: usize = right.iter().map(Span::width).sum();
    let room = width.saturating_sub(right_width);
    let mut used = 0;
    left.retain_mut(|span| {
        let w = span.width();
        if used >= room {
            return false;
        }
        if used + w > room {
            span.content = truncate_end(&span.content, room - used).into();
        }
        used += span.width();
        true
    });
    if used < room {
        left.push(Span::raw(" ".repeat(room - used)));
    }
    left.extend(right);
    left
}

fn tree_row(app: &App, row: &Row, selected: bool, width: usize) -> Line<'static> {
    let t = &app.theme;
    if app.screen_reader {
        let style = if selected {
            t.selected()
        } else {
            Style::default().fg(t.text)
        };
        return Line::from(Span::styled(truncate_end(&spoken(row), width), style));
    }
    let spans = fit(
        row_left(app, row, selected),
        row_right(app, &row.node),
        width,
    );
    let line = Line::from(spans);
    match (selected, t.monochrome) {
        (true, true) => line.style(Style::default().add_modifier(Modifier::REVERSED)),
        (true, false) => line.style(Style::default().bg(t.selection)),
        _ => line.style(Style::default().bg(t.surface_alt)),
    }
}

/// `1.2K`.
fn compact(bytes: u64) -> String {
    let kib = bytes as f64 / 1024.0;
    match bytes {
        0..1024 => format!("{bytes}B"),
        1024..10_240 => format!("{kib:.1}K"),
        10_240..1_048_576 => format!("{}K", bytes / 1024),
        1_048_576..10_485_760 => format!("{:.1}M", kib / 1024.0),
        _ => format!("{}M", bytes / 1_048_576),
    }
}

/// `markdown` → `Markdown`.
fn title_case(s: &str) -> String {
    let mut chars = s.chars();
    chars
        .next()
        .map(|c| c.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

/// The preview's header bar: breadcrumb, facts, diff summary and the raw/rendered chip.
fn preview_header(app: &App, preview: &Preview, width: usize) -> Line<'static> {
    let t = &app.theme;
    let path = crate::sidebar::clean(&preview.path);
    let mut left = vec![Span::raw(" ")];
    let mut parts = path.split('/').peekable();
    while let Some(part) = parts.next() {
        if parts.peek().is_some() {
            left.push(Span::styled(
                part.to_owned(),
                Style::default().fg(t.text_muted),
            ));
            left.push(Span::styled(" › ", Style::default().fg(t.text_subtle)));
        } else {
            let title = title_color(t, app.explorer.preview_focused);
            left.push(Span::styled(
                part.to_owned(),
                Style::default().fg(title).add_modifier(Modifier::BOLD),
            ));
        }
    }
    let mut facts: Vec<String> = preview.language.iter().map(|l| title_case(l)).collect();
    match &preview.lines {
        Some(_) => facts.push(format!("{} lines", preview.total_lines)),
        None => facts.push("binary".into()),
    }
    facts.push(compact(preview.size));
    left.push(Span::styled(
        format!("   {}", facts.join(" · ")),
        Style::default().fg(t.text_muted),
    ));
    if let Some((_, added, removed)) = app.changed_lines(&preview.path) {
        left.push(Span::styled(
            format!("  +{added}"),
            Style::default().fg(t.diff_added),
        ));
        left.push(Span::styled(
            format!(" −{removed}"),
            Style::default().fg(t.diff_removed),
        ));
    }
    let right = mode_chip(app, preview);
    bar(
        fit(
            left,
            Vec::new(),
            width.saturating_sub(right.iter().map(Span::width).sum()),
        ),
        right,
        width,
        t.surface,
    )
}

/// `rendered  m raw` for Markdown previews.
fn mode_chip(app: &App, preview: &Preview) -> Vec<Span<'static>> {
    let t = &app.theme;
    if preview.lines.is_none() || !crate::explorer::is_markdown(&preview.path) {
        return Vec::new();
    }
    let (now, other) = if app.explorer.raw {
        ("raw", "rendered")
    } else {
        ("rendered", "raw")
    };
    let chip = if t.monochrome {
        Style::default().add_modifier(Modifier::REVERSED | Modifier::BOLD)
    } else {
        Style::default()
            .fg(t.background)
            .bg(t.accent)
            .add_modifier(Modifier::BOLD)
    };
    let key = app
        .keymap
        .label("explorer", "explorer_markdown")
        .unwrap_or_default();
    vec![
        Span::styled(format!(" {now} "), chip),
        Span::styled(
            format!(" {key} "),
            Style::default().fg(t.accent).add_modifier(Modifier::BOLD),
        ),
        Span::styled(format!("{other} "), Style::default().fg(t.text_muted)),
    ]
}

fn draw_preview(f: &mut Frame, app: &App, preview: &Preview, area: Rect) {
    let t = &app.theme;
    app.mouse.push(area, Target::Pane(Pane::ExplorerPreview));
    f.render_widget(
        Paragraph::new(preview_header(app, preview, area.width as usize)),
        one_row(area, 0),
    );
    // The `rendered`/`raw` chip at the header's right presses `explorer_markdown`.
    let chip: u16 = mode_chip(app, preview)
        .iter()
        .map(|s| s.width() as u16)
        .sum();
    if let Some(key) = app.keymap.event("explorer", "explorer_markdown") {
        let x = area.x + area.width.saturating_sub(chip);
        app.mouse.push(
            Rect::new(x, area.y, chip.min(area.width), 1),
            Target::Key(key),
        );
    }
    if preview.lines.is_none() || area.height < 3 || area.width < 8 {
        return;
    }
    let body = Rect {
        x: area.x + 1,
        y: area.y + 2,
        width: area.width.saturating_sub(3),
        height: area.height.saturating_sub(2),
    };
    let rows = preview_rows(app, preview, body.width as usize);
    preview
        .rows
        .store(rows.len(), std::sync::atomic::Ordering::Relaxed);
    let height = body.height as usize;
    let top = (preview.scroll as usize).min(rows.len().saturating_sub(1));
    let shown: Vec<Line> = rows.iter().skip(top).take(height).cloned().collect();
    f.render_widget(Paragraph::new(shown), body);
    if rows.len() > height {
        let track = Rect {
            x: area.x + area.width - 1,
            width: 1,
            ..body
        };
        f.render_widget(Paragraph::new(scrollbar(t, rows.len(), height, top)), track);
    }
}

/// A scrollbar column: `│` track with a `┃` thumb.
fn scrollbar(t: &Tokens, rows: usize, height: usize, top: usize) -> Vec<Line<'static>> {
    let thumb = (height * height / rows).clamp(1, height);
    let room = height - thumb;
    let start = top * room / rows.saturating_sub(height).max(1);
    (0..height)
        .map(|y| {
            if (start..start + thumb).contains(&y) {
                Line::from(Span::styled("┃", Style::default().fg(t.text_muted)))
            } else {
                Line::from(Span::styled("│", Style::default().fg(t.border)))
            }
        })
        .collect()
}

/// Every row of the preview at `width`: a rendered Markdown document, or the source
/// with line numbers, the changed-line mark and soft wrapping under the gutter.
fn preview_rows(app: &App, preview: &Preview, width: usize) -> Vec<Line<'static>> {
    let Some(lines) = &preview.lines else {
        return Vec::new();
    };
    let t = &app.theme;
    let mut rows = if crate::explorer::is_markdown(&preview.path) && !app.explorer.raw {
        let ctx = crate::markdown::Ctx::new(t, width, false);
        app.markdown.message(&lines.join("\n"), &ctx).lines.clone()
    } else {
        source_rows(app, preview, lines, width)
    };
    if preview.truncated {
        rows.push(Line::default());
        rows.push(Line::from(Span::styled(
            format!(
                "… truncated: {} of {} lines shown",
                lines.len(),
                preview.total_lines
            ),
            Style::default().fg(t.warning),
        )));
    }
    rows
}

fn source_rows(app: &App, preview: &Preview, lines: &[String], width: usize) -> Vec<Line<'static>> {
    let t = &app.theme;
    let marks = app
        .changed_lines(&preview.path)
        .map(|(lines, _, _)| lines)
        .unwrap_or_default();
    let styled = highlighted(app, preview, lines);
    let digits = lines.len().max(1).to_string().len();
    let rule = Span::styled("│ ", Style::default().fg(t.border));
    let hang = [Span::raw(" ".repeat(digits + 1)), rule.clone()];
    let mut out = Vec::new();
    for (i, text) in lines.iter().enumerate() {
        let mark = if marks.contains(&(i + 1)) {
            Span::styled("▎", Style::default().fg(t.diff_added))
        } else {
            Span::raw(" ")
        };
        let mut spans = vec![
            Span::styled(
                format!("{:>digits$}", i + 1),
                Style::default().fg(t.text_subtle),
            ),
            mark,
            rule.clone(),
        ];
        match styled.as_ref().and_then(|s| s.get(i)) {
            Some(runs) => spans.extend(runs.iter().map(|(s, c)| Span::styled(c.clone(), *s))),
            None => spans.push(Span::styled(text.clone(), Style::default().fg(t.text))),
        }
        out.extend(super::wrap::wrap_hanging(Line::from(spans), width, &hang));
    }
    out
}

/// The preview's lines colored by scope, cached for the theme; `None` while the syntax
/// set loads, for unknown languages and over the highlighting limits.
fn highlighted(
    app: &App,
    preview: &Preview,
    lines: &[String],
) -> Option<Vec<crate::highlight::Styled>> {
    let mut cache = preview
        .highlighted
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some((theme, styled)) = cache.as_ref()
        && *theme == app.theme.name
    {
        return Some(styled.clone());
    }
    let set = crate::highlight::syntaxes()?;
    let syntax = crate::highlight::for_path(set, &preview.path)?;
    let styled = crate::highlight::highlight(set, syntax, &lines.join("\n"), &app.theme)?;
    *cache = Some((app.theme.name.clone(), styled.clone()));
    Some(styled)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_are_compact() {
        for (bytes, want) in [
            (64, "64B"),
            (1_228, "1.2K"),
            (18_432, "18K"),
            (186_368, "182K"),
            (1_572_864, "1.5M"),
            (52_428_800, "50M"),
        ] {
            assert_eq!(compact(bytes), want, "{bytes}");
        }
    }

    #[test]
    fn the_scrollbar_thumb_moves_with_the_scroll() {
        let t = crate::theme::Catalog::default()
            .get("cyber")
            .resolve(false, crate::theme::Depth::TrueColor);
        let thumb = |top| {
            scrollbar(&t, 100, 10, top)
                .iter()
                .position(|l| l.spans[0].content == "┃")
                .unwrap()
        };
        assert_eq!(thumb(0), 0);
        assert_eq!(thumb(90), 9);
        assert!(thumb(45) > 2 && thumb(45) < 7);
    }
}
