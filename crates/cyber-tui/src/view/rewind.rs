use crate::{
    app::App,
    messaging::clean,
    rewind::{Intent, Phase},
};
use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::Line,
    widgets::{Block, Clear, Paragraph, Wrap},
};

pub fn draw(frame: &mut Frame, app: &App, area: Rect) {
    let view = &app.rewind;
    frame.render_widget(Clear, area);
    let block = Block::bordered()
        .title(match view.intent {
            Intent::Menu => " Rewind ",
            Intent::Undo => " Undo · staged until next prompt ",
            Intent::Redo => " Redo / clear staged undo ",
        })
        .style(Style::default().fg(app.theme.text).bg(app.theme.background));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let mut lines = vec![Line::styled(
        format!(
            "{} · {}",
            clean(&app.session.title),
            clean(&app.session.directory)
        ),
        Style::default().fg(app.theme.text_muted),
    )];
    match view.phase {
        Phase::Busy => {
            lines.push(Line::from(
                "Session is busy. I explicitly interrupts this Session; Esc cancels.",
            ));
        }
        Phase::Waiting => {
            lines.push(Line::from(
                "Waiting for acknowledged idle. R refreshes status; Esc closes.",
            ));
        }
        Phase::Loading => {
            lines.push(Line::from(
                "Waiting for the owner server… Esc closes. Failed confirmations require fresh status and review.",
            ));
        }
        Phase::Review => review(app, &mut lines),
        _ => choose(app, &mut lines),
    }
    if let Some(error) = &view.code_error {
        lines.push(Line::styled(
            format!("Code unavailable/conflicting: {}", clean(error)),
            Style::default().fg(app.theme.warning),
        ));
    }
    if let Some(error) = &view.error {
        lines.push(Line::styled(
            clean(error),
            Style::default().fg(app.theme.error),
        ));
    }
    frame.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .scroll((view.scroll, 0)),
        inner,
    );
}
fn choose(app: &App, lines: &mut Vec<Line<'static>>) {
    let view = &app.rewind;
    lines.push(Line::from(format!(
        "Target {:?} · action {:?} · 1 code / 2 conversation / 3 both",
        view.target, view.action
    )));
    lines.push(Line::from(
        "↑/↓ select · Enter preview · U undo · D redo · R reload · Esc close",
    ));
    if view.saved_draft(&app.session).is_some() {
        lines.push(Line::from("P copies the saved original text draft"));
    }
    if view.staged.is_some() {
        lines.push(Line::from(
            "An undo is staged; target stays fixed. Redo clears the last boundary.",
        ));
    }
    let first = view.selected.saturating_sub(4);
    for (i, boundary) in view.boundaries.iter().enumerate().skip(first).take(9) {
        let text = format!(
            "{} {}  {}",
            if i == view.selected { ">" } else { " " },
            clean(&boundary.id),
            clean(&boundary.text)
        );
        lines.push(if i == view.selected {
            Line::styled(text, app.theme.selected())
        } else {
            Line::from(text)
        });
    }
    lines.push(Line::from(
        "Conversation-only rewind remains available when code snapshots are unavailable.",
    ));
    lines.push(Line::from(
        "Remote/database effects and unresolved native outcomes are not undone.",
    ));
}
fn review(app: &App, lines: &mut Vec<Line<'static>>) {
    let Some(preview) = &app.rewind.preview else {
        return;
    };
    lines.push(Line::styled(
        format!(
            "REVIEW {:?} · {:?} · before {}",
            preview.action,
            preview.target,
            clean(&preview.message_id)
        ),
        Style::default()
            .fg(app.theme.accent)
            .add_modifier(Modifier::BOLD),
    ));
    lines.push(Line::from(
        "Enter explicitly confirms · R refreshes preview · PgUp/PgDn scroll · Esc back",
    ));
    lines.push(Line::from(clean(&preview.warning)));
    if preview.action == cyber_server::runtime::RewindAction::Stage
        && let Some(boundary) = app
            .rewind
            .boundaries
            .iter()
            .find(|b| b.id == preview.message_id)
    {
        lines.push(Line::from(format!(
            "Composer after acknowledgement (newer edits stay): {}",
            clean(&boundary.text)
        )));
    }
    if let Some(code) = &preview.code {
        lines.push(Line::from(format!(
            "{} proposed file changes · {} current exclusions",
            code.files.len(),
            code.skipped.len()
        )));
        for file in &code.files {
            lines.push(Line::styled(
                format!(
                    "{} {} (+{} -{})",
                    file.status,
                    clean(&file.file),
                    file.additions,
                    file.deletions
                ),
                Style::default().fg(app.theme.secondary),
            ));
            lines.extend(file.patch.lines().map(|line| Line::from(clean(line))));
        }
        for path in &code.skipped {
            lines.push(Line::from(format!("Excluded: {}", clean(path))));
        }
    } else {
        lines.push(Line::from("Conversation only · no files restored"));
    }
    for effect in &preview.effects {
        lines.push(Line::from(format!(
            "Retained effect {} · {} · {:?}",
            clean(&effect.call_id),
            clean(&effect.tool),
            effect.status
        )));
    }
}
