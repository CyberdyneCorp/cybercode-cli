//! The `/tree` view (`tui` → Branch tree view): user messages of every branch, the
//! active path highlighted, branch counts at branch points and labels as tags.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};

use super::layout::truncate_end;
use super::overlays::{centered, frame};
use crate::app::App;
use crate::branches::{Row, Tree};
use crate::mouse::List;

pub fn draw(f: &mut Frame, app: &App, area: Rect) {
    let tree = &app.branches;
    let rect = centered(area, 96, 24);
    let width = rect.width.saturating_sub(2) as usize;
    let rows = tree.rows();
    let room = rect.height.saturating_sub(4) as usize;
    let selected = tree.selected..tree.selected + 1;
    let first = app.mouse.window(List::Tree, selected, room, rows.len());
    let mut lines: Vec<Line> = rows
        .iter()
        .enumerate()
        .skip(first)
        .take(room)
        .map(|(pos, row)| row_line(app, tree, row, pos == tree.selected, width))
        .collect();
    // A label being typed takes the keys.
    if tree.label.is_none() {
        for (line, pos) in (first..rows.len()).take(room).enumerate() {
            super::overlays::push_row(app, rect, line, List::Tree, pos);
        }
    }
    if rows.is_empty() {
        lines.push(Line::from(Span::styled(
            "  no messages yet",
            Style::default().fg(app.theme.text_muted),
        )));
    }
    while lines.len() < room {
        lines.push(Line::default());
    }
    lines.push(Line::default());
    lines.push(hint(app, tree));
    f.render_widget(Clear, rect);
    f.render_widget(
        Paragraph::new(lines).block(frame(app, "Session tree")),
        rect,
    );
}

fn row_line(app: &App, tree: &Tree, row: &Row, selected: bool, width: usize) -> Line<'static> {
    let t = &app.theme;
    let node = &tree.nodes[row.index];
    let fold = match (row.children, row.collapsed) {
        (0, _) => ' ',
        (_, true) => '▸',
        (_, false) => '▾',
    };
    let marker = if node.active { '●' } else { '○' };
    let mut tail = String::new();
    if row.children > 1 {
        tail.push_str(&format!("  ({} branches)", row.children));
    }
    for label in &node.labels {
        tail.push_str(&format!("  [{label}]"));
    }
    if node.active && node.tip {
        tail.push_str("  (active)");
    }
    let head = format!(
        "{}{}{fold} {marker} ",
        if selected { "❯ " } else { "  " },
        "  ".repeat(row.depth)
    );
    let room = width.saturating_sub(head.chars().count() + tail.chars().count());
    let text = format!("{head}{}{tail}", truncate_end(&node.preview, room));
    let style = match (selected, node.active) {
        (true, _) => t.selected(),
        (false, true) => Style::default().fg(t.accent).add_modifier(Modifier::BOLD),
        (false, false) => Style::default().fg(t.text),
    };
    Line::from(Span::styled(text, style))
}

fn hint(app: &App, tree: &Tree) -> Line<'static> {
    let t = &app.theme;
    if let Some(label) = &tree.label {
        return Line::from(vec![
            Span::styled(" Label: ", Style::default().fg(t.text_muted)),
            Span::styled(label.clone(), Style::default().fg(t.text)),
            Span::styled("▏", Style::default().fg(t.accent)),
            Span::styled(
                "  Enter save · Esc cancel",
                Style::default().fg(t.text_muted),
            ),
        ]);
    }
    let code = if tree.code { "on" } else { "off" };
    Line::from(Span::styled(
        format!(" Enter checkout · c restore code: {code} · l label · Space fold · Esc close"),
        Style::default().fg(t.text_muted),
    ))
}
