//! Drawing (`tui` → Message and part rendering, Permission prompt, Question prompt).

mod agents;
mod branches;
mod collaboration;
mod composer;
mod explorer;
pub(crate) mod footer;
pub(crate) mod hex;
pub mod layout;
mod login;
mod messages;
mod messaging;
mod output_styles;
mod overlays;
pub(crate) mod progress;
mod rename;
mod settings;
pub(crate) mod sidebar;
mod sounds;
mod team;
mod themes;
pub mod welcome;
mod workflow_library;
mod workflows;
mod wrap;

#[cfg(test)]
pub(crate) use messages::transcript;
#[cfg(test)]
pub(crate) use settings::lines as settings_lines;

/// The held card's rows, for the layout property test.
#[cfg(test)]
pub(crate) fn messaging_card_height() -> u16 {
    messaging::CARD_HEIGHT
}

use std::ops::Range;

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};

use crate::app::{App, Overlay};
use crate::mouse::{Pane, Target};

pub fn draw(f: &mut Frame, app: &App) {
    let area = f.area();
    f.render_widget(Block::default().style(base(app)), area);
    app.mouse.clear();
    app.overview_visible
        .store(false, std::sync::atomic::Ordering::Relaxed);
    let plan = layout::plan(
        area,
        &layout::Inputs {
            docked: app.sidebar.docked(area.width),
            drawer: app.sidebar.drawer,
            screen_reader: app.screen_reader,
            queued: queued_rows(app).len() as u16,
            held: messaging::card_height(app),
            progress: progress::height(app, area.height),
            composer: composer::height(app),
        },
    );
    app.frame_width
        .store(area.width, std::sync::atomic::Ordering::Relaxed);
    if plan.too_small {
        app.progress.set_on_screen(false);
        draw_too_small(f, app, area);
        return;
    }
    if matches!(
        app.overlay,
        Overlay::Settings
            | Overlay::Files
            | Overlay::Hex
            | Overlay::Workflows
            | Overlay::Messages
            | Overlay::Team
            | Overlay::Collaboration
            | Overlay::Agents
    ) {
        // Full screen over the transcript, composer and sidebar; the footer stays.
        app.set_screen_links(Vec::new());
        app.progress.set_on_screen(false);
        let panel = Rect {
            height: area.height.saturating_sub(plan.footer.height),
            ..area
        };
        let panel = if app.computer.banner(&app.session.id).is_some() {
            banner(f, app, panel)
        } else {
            panel
        };
        let panel = recap(f, app, panel);
        full_screen(f, app, panel);
        footer::draw(f, app, plan.footer, plan.compact_footer);
        return;
    }
    if plan.header.height > 0 {
        draw_header(f, app, plan.header);
    }
    let below_banner = banner(f, app, plan.transcript);
    let transcript = recap(f, app, below_banner);
    // Panes first: the rows drawn on them are targets over them.
    app.mouse.push(transcript, Target::Pane(Pane::Transcript));
    app.mouse.push(plan.composer, Target::Pane(Pane::Composer));
    if welcome::shown(app) {
        app.set_screen_links(Vec::new());
        welcome::draw(f, app, transcript);
    } else {
        messages::draw(f, app, transcript);
    }
    draw_queued(f, app, plan.queued);
    messaging::card(f, app, plan.held);
    progress::draw(f, app, plan.progress);
    composer::draw(f, app, plan.composer);
    footer::draw(f, app, plan.footer, plan.compact_footer);
    if let Some(rect) = plan.sidebar {
        app.mouse.push(rect, Target::Pane(Pane::Sidebar));
        sidebar::draw(f, app, rect);
    }
    if let Some(c) = &app.completion {
        overlays::completion(f, app, c, plan.composer);
    }
    if !matches!(app.overlay, Overlay::None) {
        // Every panel over the main screen is modal: a click beside it does nothing.
        app.mouse.modal(area);
    }
    match &app.overlay {
        Overlay::None
        | Overlay::Settings
        | Overlay::Files
        | Overlay::Hex
        | Overlay::Workflows
        | Overlay::Messages
        | Overlay::Team
        | Overlay::Collaboration
        | Overlay::Agents => {}
        Overlay::Tree => branches::draw(f, app, area),
        Overlay::Themes(s) => themes::draw(f, app, s, area),
        Overlay::Picker(p) => overlays::picker(f, app, p, area),
        Overlay::Palette(p) => overlays::palette(f, app, p, area),
        Overlay::Permission(step) => overlays::permission(f, app, step, area),
        Overlay::Question(form) => overlays::question(f, app, form, area),
        Overlay::HookDefinitions => overlays::hook_definitions(f, app, area),
        Overlay::HookHistory => overlays::hook_history(f, app, area),
        Overlay::Memory => overlays::memory(f, app, area),
        Overlay::Help => overlays::help(f, app, area),
        Overlay::Rename(title) => rename::draw(f, app, title, area),
        Overlay::Login => login::draw(f, app, area),
        Overlay::Findings => overlays::findings(f, app, area),
        Overlay::FileDiff { index, scroll } => overlays::file_diff(f, app, *index, *scroll, area),
        Overlay::TaskLog {
            title,
            text,
            scroll,
        } => overlays::task_log(f, app, title, text, *scroll, area),
        Overlay::SideChat(chat) => overlays::side_chat(f, app, chat, area),
        Overlay::ConfirmStopTasks => overlays::confirm_stop_tasks(f, app, area),
        Overlay::ConfirmExport(request) => overlays::confirm_export(f, app, request, area),
        Overlay::OutputStyles(picker) => output_styles::draw(f, app, picker, area),
        Overlay::Sounds(view) => sounds::draw(f, app, view, area),
        Overlay::ConfirmBypass => overlays::confirm_bypass(f, app, area),
    }
}

/// Filter chips: `[current]` selected, the others muted, e.g. `[all] in  out  held`.
pub(super) fn filter_chips<T: PartialEq + Copy>(
    all: &[T],
    current: T,
    label: impl Fn(T) -> &'static str,
    t: &crate::theme::Tokens,
) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    for &filter in all {
        let label = label(filter);
        if filter == current {
            spans.push(Span::styled(format!("[{label}]"), t.selected()));
        } else {
            spans.push(Span::styled(
                format!(" {label} "),
                Style::default().fg(t.text_muted),
            ));
        }
        spans.push(Span::raw(" "));
    }
    spans
}

/// Key hints joined with ` · `, wrapped to `width`; a wrapped line starts with ` · `.
pub(super) fn key_hints(keys: &[&str], width: usize) -> Vec<Line<'static>> {
    use unicode_width::UnicodeWidthStr;
    let mut lines = vec![String::from(" ")];
    for key in keys {
        let last = lines.last_mut().expect("a line");
        let sep = if last.trim().is_empty() { "" } else { " · " };
        if last.width() + sep.len() + key.width() > width && !last.trim().is_empty() {
            lines.push(format!(" · {key}"));
        } else {
            last.push_str(sep);
            last.push_str(key);
        }
    }
    lines
        .into_iter()
        .map(|l| Line::from(Span::raw(l)))
        .collect()
}

/// Register `target` over the lines `lines` of a paragraph drawn in `body` from its
/// line `skip` on; the part scrolled away or cut off below takes no clicks.
pub(super) fn push_lines(app: &App, body: Rect, lines: Range<usize>, skip: usize, target: Target) {
    let start = lines.start.max(skip) - skip;
    let end = lines.end.saturating_sub(skip);
    if start >= end || start >= body.height as usize {
        return;
    }
    let rect = Rect {
        y: body.y + start as u16,
        height: (end - start).min(body.height as usize) as u16,
        ..body
    };
    app.mouse.push(rect.intersection(body), target);
}

/// The full-screen view the overlay opens: the explorer, the Workflow monitor,
/// `/messages`, the team view or Settings.
fn full_screen(f: &mut Frame, app: &App, panel: Rect) {
    match app.overlay {
        Overlay::Files => explorer::draw(f, app, panel),
        Overlay::Hex => hex::draw(f, app, panel),
        Overlay::Workflows => workflows::draw(f, app, panel),
        Overlay::Messages => messaging::view(f, app, panel),
        Overlay::Team => team::view(f, app, panel),
        Overlay::Collaboration => collaboration::view(f, app, panel),
        Overlay::Agents => agents::view(f, app, panel),
        _ => settings::draw(f, app, panel),
    }
}

/// The unsupported-version warning on the first row of `area`, while the rest keeps at
/// least three rows; returns the area below it.
fn banner(f: &mut Frame, app: &App, area: Rect) -> Rect {
    let computer_banner = app.computer.banner(&app.session.id);
    let Some(text) = computer_banner.as_ref().or(app.update.banner.as_ref()) else {
        return area;
    };
    if area.height < 4 {
        return area;
    }
    let width = area.width.saturating_sub(2);
    let style = Style::default()
        .fg(app.theme.warning)
        .add_modifier(Modifier::BOLD);
    f.render_widget(
        Paragraph::new(Line::from(Span::styled(
            layout::truncate_end(text, width as usize),
            style,
        ))),
        Rect {
            x: area.x + 1,
            width,
            height: 1,
            ..area
        },
    );
    Rect {
        y: area.y + 1,
        height: area.height - 1,
        ..area
    }
}

/// The recap block at the top of `area`, when one is shown and the rest keeps at least
/// three rows; returns the area below it.
fn recap(f: &mut Frame, app: &App, area: Rect) -> Rect {
    let rows = crate::recap::rows(app, area.width.saturating_sub(2) as usize);
    let height = rows.len() as u16;
    if rows.is_empty() || area.height < height + 3 {
        return area;
    }
    f.render_widget(
        Paragraph::new(rows),
        Rect {
            x: area.x + 1,
            width: area.width.saturating_sub(2),
            height,
            ..area
        },
    );
    Rect {
        y: area.y + height,
        height: area.height - height,
        ..area
    }
}

/// `Terminal too small (40×10 minimum)`, centered, cut to the width.
fn draw_too_small(f: &mut Frame, app: &App, area: Rect) {
    app.set_screen_links(Vec::new());
    let text = format!(
        "Terminal too small ({}×{} minimum)",
        layout::MIN_WIDTH,
        layout::MIN_HEIGHT
    );
    let rows = (text.chars().count() as u16)
        .div_ceil(area.width.max(1))
        .max(1);
    let top = area.height.saturating_sub(rows) / 2;
    let rect = Rect {
        y: area.y + top,
        height: area.height - top,
        ..area
    };
    f.render_widget(
        Paragraph::new(Span::styled(text, Style::default().fg(app.theme.warning)))
            .wrap(ratatui::widgets::Wrap { trim: true })
            .alignment(ratatui::layout::Alignment::Center),
        rect,
    );
}

/// The theme's text on its background, for every cell nothing else styles.
fn base(app: &App) -> Style {
    Style::default().fg(app.theme.text).bg(app.theme.background)
}

/// A braille spinner frame per progress frame, or a static `…` under reduced motion.
fn spinner(app: &App) -> &'static str {
    const FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
    if app.reduced_motion {
        return "…";
    }
    FRAMES[app.progress.frame as usize % FRAMES.len()]
}

/// ` cyber · <title>` and the run state when the sidebar is not docked; the model, Mode
/// and usage live in the composer info line and the status footer.
fn draw_header(f: &mut Frame, app: &App, area: Rect) {
    let t = &app.theme;
    let state = if app.session.running {
        format!("{} running ", spinner(app))
    } else {
        "idle ".to_owned()
    };
    let branch = app
        .branches
        .header()
        .map(|b| format!(" · {b}"))
        .unwrap_or_default();
    let room =
        (area.width as usize).saturating_sub(state.chars().count() + branch.chars().count() + 10);
    let title = layout::truncate_end(&crate::sidebar::clean(&app.session.title), room);
    let line = Line::from(vec![
        Span::styled(
            " cyber ",
            Style::default().fg(t.accent).add_modifier(Modifier::BOLD),
        ),
        Span::styled(format!("· {title}"), Style::default().fg(t.text)),
        Span::styled(branch, Style::default().fg(t.text_muted)),
    ]);
    f.render_widget(Paragraph::new(line), area);
    let style = if app.session.running {
        Style::default().fg(t.warning)
    } else {
        Style::default().fg(t.text_muted)
    };
    f.render_widget(
        Paragraph::new(Line::from(Span::styled(state, style)))
            .alignment(ratatui::layout::Alignment::Right),
        area,
    );
}

/// Queued inputs; a received message shows its sender and text, and held messages
/// are drawn as cards instead.
fn queued_rows(app: &App) -> Vec<(String, String)> {
    app.queued
        .iter()
        .filter(|q| !(q.held && app.messaging.is_message(q)))
        .map(|q| match app.messaging.received(&q.message_id) {
            Some(record) => {
                let first = record.text.as_deref().unwrap_or_default();
                let first = crate::messaging::clean(first.lines().next().unwrap_or_default());
                (
                    format!(
                        " ⏳ {} ✉ {} ",
                        q.delivery,
                        crate::messaging::clean(&record.from)
                    ),
                    first,
                )
            }
            None => (
                format!(" ⏳ {} ", q.delivery),
                q.text.lines().next().unwrap_or_default().to_string(),
            ),
        })
        .collect()
}

fn draw_queued(f: &mut Frame, app: &App, area: Rect) {
    let lines: Vec<Line> = queued_rows(app)
        .into_iter()
        .take(area.height as usize)
        .map(|(label, first)| {
            Line::from(vec![
                Span::styled(label, Style::default().fg(app.theme.warning)),
                Span::raw(first),
            ])
        })
        .collect();
    f.render_widget(Paragraph::new(lines), area);
}
