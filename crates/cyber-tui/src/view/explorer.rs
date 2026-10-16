//! The file explorer (`tui` → File explorer): a header with the project, branch and
//! changed-file count, the filter line, the tree with box-drawing guides and fold
//! markers, the preview beside it (or instead of it below 100 columns), and the key
//! row from the binding table.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph};

use super::layout::truncate_end;
use crate::app::App;
use crate::explorer::{Preview, Read, Row, SIDE_BY_SIDE};
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
    let row = |y: u16, height: u16| Rect {
        y: area.y + y,
        height,
        ..area
    };
    f.render_widget(Paragraph::new(header(app)), row(0, 1));
    f.render_widget(Paragraph::new(filter_line(app)), row(1, 1));
    let body = row(2, area.height - 3);
    f.render_widget(
        Paragraph::new(keys(app, area.width)),
        row(area.height - 1, 1),
    );
    match &e.preview {
        Some(preview) if e.preview_focused => draw_preview(f, app, preview, body),
        Some(preview) if area.width >= SIDE_BY_SIDE => {
            let tree_width = (body.width * 2 / 5).max(30);
            draw_tree(
                f,
                app,
                Rect {
                    width: tree_width,
                    ..body
                },
            );
            let side = Rect {
                x: body.x + tree_width + 1,
                width: body.width.saturating_sub(tree_width + 1),
                ..body
            };
            draw_preview(f, app, preview, side);
        }
        _ => draw_tree(f, app, body),
    }
}

/// ` app :main · 3 changed`.
fn header(app: &App) -> Line<'static> {
    let t = &app.theme;
    let directory = crate::sidebar::display_path(&app.session.directory);
    let project = directory
        .trim_end_matches(['/', '\\'])
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or_default()
        .to_owned();
    let mut spans = vec![Span::styled(
        format!(" {project}"),
        Style::default().fg(t.text).add_modifier(Modifier::BOLD),
    )];
    if let Some(branch) = app
        .sidebar
        .vcs
        .ready()
        .and_then(|git| git.as_ref())
        .and_then(|git| git.branch.clone())
    {
        spans.push(Span::styled(
            format!(":{branch}"),
            Style::default().fg(t.accent),
        ));
    }
    if let Some(changed) = app.explorer.changed {
        spans.push(Span::styled(
            format!(" · {changed} changed"),
            Style::default().fg(t.text_muted),
        ));
    }
    Line::from(spans)
}

fn filter_line(app: &App) -> Line<'static> {
    let t = &app.theme;
    let e = &app.explorer;
    if e.filter.is_empty() && !e.filtering {
        return Line::from(Span::styled(
            " / filter",
            Style::default().fg(t.text_subtle),
        ));
    }
    let cursor = if e.filtering { "▏" } else { "" };
    Line::from(vec![
        Span::styled(" / ", Style::default().fg(t.accent)),
        Span::styled(
            format!("{}{cursor}", crate::sidebar::clean(&e.filter)),
            Style::default().fg(t.text),
        ),
    ])
}

/// The explorer keys from the binding table, with the hidden count.
fn keys(app: &App, width: u16) -> Line<'static> {
    let actions = [
        "explorer_open",
        "explorer_filter",
        "explorer_changed",
        "explorer_hidden",
        "explorer_mention",
        "explorer_copy",
        "explorer_refresh",
        "explorer_close",
    ];
    let mut parts: Vec<String> = actions
        .iter()
        .filter_map(|action| app.keymap.hint("explorer", action))
        .map(|(key, what)| format!("{key} {what}"))
        .collect();
    let hidden = app.explorer.hidden_count();
    if hidden > 0 && !app.explorer.hidden {
        parts.push(format!("{hidden} hidden"));
    }
    let text = truncate_end(&format!(" {}", parts.join(" · ")), width as usize);
    Line::from(Span::styled(
        text,
        Style::default().fg(app.theme.text_muted),
    ))
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
    let top = e.selected.saturating_sub(height.saturating_sub(1));
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
    let n = &row.node;
    let marker = match (n.dir, app.explorer.expanded.contains(&n.path)) {
        (true, true) => "▾ ",
        (true, false) => "▸ ",
        _ => "",
    };
    let mut name = format!("{}{marker}{}", row.guides, n.name);
    if let Some(target) = &n.target {
        name.push_str(&format!(" → {target}"));
    }
    let note = match n.read {
        Read::Ask if !n.dir => "  ⊘ ask to read",
        Read::Deny => "  ⊘ denied",
        _ => "",
    };
    let dim = n.read != Read::Allow || n.ignored;
    let base = if selected {
        t.selected()
    } else if dim {
        Style::default().fg(t.text_subtle)
    } else {
        Style::default().fg(t.text)
    };
    let letter = n.git.map_or(Span::raw("  "), |c| {
        Span::styled(format!("{c} "), Style::default().fg(git_color(t, c)))
    });
    let room = width.saturating_sub(3 + note.chars().count());
    Line::from(vec![
        Span::raw(" "),
        letter,
        Span::styled(truncate_end(&name, room), base),
        Span::styled(note, Style::default().fg(t.text_subtle)),
    ])
}

/// `1.2 KiB`.
fn size(bytes: u64) -> String {
    match bytes {
        0..1024 => format!("{bytes} B"),
        1024..1_048_576 => format!("{:.1} KiB", bytes as f64 / 1024.0),
        _ => format!("{:.1} MiB", bytes as f64 / 1_048_576.0),
    }
}

fn draw_preview(f: &mut Frame, app: &App, preview: &Preview, area: Rect) {
    let t = &app.theme;
    let changed = app.changed_lines(&preview.path);
    let mut head = vec![crate::sidebar::clean(&preview.path)];
    head.extend(preview.language.clone());
    match &preview.lines {
        Some(_) => head.push(format!("{} lines", preview.total_lines)),
        None => head.push(format!("binary · {}", size(preview.size))),
    }
    if let Some((_, added, removed)) = &changed {
        head.push(format!("+{added} −{removed}"));
    }
    let title = truncate_end(&format!(" {}", head.join(" · ")), area.width as usize);
    f.render_widget(
        Paragraph::new(Span::styled(
            title,
            Style::default().fg(t.text).add_modifier(Modifier::BOLD),
        )),
        Rect { height: 1, ..area },
    );
    let Some(lines) = &preview.lines else {
        return;
    };
    let body = Rect {
        y: area.y + 1,
        height: area.height.saturating_sub(1),
        ..area
    };
    let marks = changed.map(|(lines, _, _)| lines).unwrap_or_default();
    let styled = highlighted(app, preview, lines);
    let digits = lines.len().max(1).to_string().len();
    let mut out: Vec<Line> = lines
        .iter()
        .enumerate()
        .skip(preview.scroll as usize)
        .take(body.height as usize)
        .map(|(i, text)| {
            let gutter = if marks.contains(&(i + 1)) {
                Span::styled("▎", Style::default().fg(t.diff_added))
            } else {
                Span::raw(" ")
            };
            let mut spans = vec![
                Span::styled(
                    format!("{:>digits$} ", i + 1),
                    Style::default().fg(t.text_subtle),
                ),
                gutter,
            ];
            match styled.as_ref().and_then(|s| s.get(i)) {
                Some(runs) => spans.extend(runs.iter().map(|(s, c)| Span::styled(c.clone(), *s))),
                None => spans.push(Span::styled(text.clone(), Style::default().fg(t.text))),
            }
            Line::from(spans)
        })
        .collect();
    if preview.truncated && out.len() < body.height as usize {
        out.push(Line::from(Span::styled(
            format!(
                " … truncated: {} of {} lines shown",
                lines.len(),
                preview.total_lines
            ),
            Style::default().fg(t.warning),
        )));
    }
    f.render_widget(Paragraph::new(out), body);
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
