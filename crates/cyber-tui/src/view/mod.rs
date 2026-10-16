//! Drawing (`tui` → Message and part rendering, Permission prompt, Question prompt).

mod composer;
mod explorer;
pub(crate) mod footer;
pub mod layout;
mod messages;
mod overlays;
mod settings;
mod sidebar;
mod themes;
pub mod welcome;
mod wrap;

#[cfg(test)]
pub(crate) use messages::transcript;
#[cfg(test)]
pub(crate) use settings::lines as settings_lines;

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};

use crate::app::{App, Overlay};

pub fn draw(f: &mut Frame, app: &App) {
    let area = f.area();
    f.render_widget(Block::default().style(base(app)), area);
    let plan = layout::plan(
        area,
        &layout::Inputs {
            docked: app.sidebar.docked(area.width),
            drawer: app.sidebar.drawer,
            screen_reader: app.screen_reader,
            queued: app.queued.len() as u16,
            composer: composer::height(app),
        },
    );
    app.frame_width
        .store(area.width, std::sync::atomic::Ordering::Relaxed);
    if plan.too_small {
        draw_too_small(f, app, area);
        return;
    }
    if matches!(app.overlay, Overlay::Settings | Overlay::Files) {
        // Full screen over the transcript, composer and sidebar; the footer stays.
        app.set_screen_links(Vec::new());
        let panel = recap(
            f,
            app,
            Rect {
                height: area.height.saturating_sub(plan.footer.height),
                ..area
            },
        );
        if matches!(app.overlay, Overlay::Files) {
            explorer::draw(f, app, panel);
        } else {
            settings::draw(f, app, panel);
        }
        footer::draw(f, app, plan.footer, plan.compact_footer);
        return;
    }
    if plan.header.height > 0 {
        draw_header(f, app, plan.header);
    }
    let transcript = recap(f, app, plan.transcript);
    if welcome::shown(app) {
        app.set_screen_links(Vec::new());
        welcome::draw(f, app, transcript);
    } else {
        messages::draw(f, app, transcript);
    }
    draw_queued(f, app, plan.queued);
    composer::draw(f, app, plan.composer);
    footer::draw(f, app, plan.footer, plan.compact_footer);
    if let Some(rect) = plan.sidebar {
        sidebar::draw(f, app, rect);
    }
    if let Some(c) = &app.completion {
        overlays::completion(f, app, c, plan.composer);
    }
    match &app.overlay {
        Overlay::None | Overlay::Settings | Overlay::Files => {}
        Overlay::Themes(s) => themes::draw(f, app, s, area),
        Overlay::Picker(p) => overlays::picker(f, app, p, area),
        Overlay::Palette(p) => overlays::palette(f, app, p, area),
        Overlay::Permission(step) => overlays::permission(f, app, step, area),
        Overlay::Question(form) => overlays::question(f, app, form, area),
        Overlay::HookDefinitions => overlays::hook_definitions(f, app, area),
        Overlay::HookHistory => overlays::hook_history(f, app, area),
        Overlay::Memory => overlays::memory(f, app, area),
        Overlay::Help => overlays::help(f, app, area),
        Overlay::Findings => overlays::findings(f, app, area),
        Overlay::FileDiff { index, scroll } => overlays::file_diff(f, app, *index, *scroll, area),
        Overlay::ConfirmStopTasks => overlays::confirm_stop_tasks(f, app, area),
        Overlay::ConfirmBypass => overlays::confirm_bypass(f, app, area),
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

/// A braille spinner frame every 250 ms (the redraw tick), or a static `…` under
/// reduced motion.
fn spinner(app: &App) -> &'static str {
    const FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
    if app.reduced_motion {
        return "…";
    }
    FRAMES[(app.uptime().as_millis() / 250) as usize % FRAMES.len()]
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
    let room = (area.width as usize).saturating_sub(state.chars().count() + 10);
    let title = layout::truncate_end(&crate::sidebar::clean(&app.session.title), room);
    let line = Line::from(vec![
        Span::styled(
            " cyber ",
            Style::default().fg(t.accent).add_modifier(Modifier::BOLD),
        ),
        Span::styled(format!("· {title}"), Style::default().fg(t.text)),
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

fn draw_queued(f: &mut Frame, app: &App, area: Rect) {
    let lines: Vec<Line> = app
        .queued
        .iter()
        .take(area.height as usize)
        .map(|q| {
            let first = q.text.lines().next().unwrap_or_default();
            Line::from(vec![
                Span::styled(
                    format!(" ⏳ {} ", q.delivery),
                    Style::default().fg(app.theme.warning),
                ),
                Span::raw(first.to_string()),
            ])
        })
        .collect();
    f.render_widget(Paragraph::new(lines), area);
}
