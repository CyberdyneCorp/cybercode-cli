//! `/sounds` (`tui` → Sound settings command): one row per setting and per event, the
//! selected row marked `›`, every state a word, then the player status and the keys.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};

use super::layout::truncate_end;
use super::overlays::{centered, frame};
use crate::app::App;
use crate::sound::overlay::{ROWS, Row, SoundsView};
use crate::sound::{Choice, Pack, Settings, When};

const KEYS: &str = "↑↓ select · space toggle · ←→ change · p preview · esc close";
const LABEL: usize = 18;

pub fn draw(f: &mut Frame, app: &App, view: &SoundsView, area: Rect) {
    let t = &app.theme;
    let rect = centered(area, 84, ROWS.len() as u16 + 7);
    let width = rect.width.saturating_sub(2) as usize;
    let chosen = &app.sounds.chosen;
    let effective = app.sounds.settings();
    let mut lines: Vec<Line> = ROWS
        .iter()
        .enumerate()
        .map(|(i, &row)| {
            let selected = i == view.selected;
            let marker = if selected { "› " } else { "  " };
            let text = format!(
                "{marker}{:<LABEL$}{}",
                label(row),
                value(row, chosen, &effective)
            );
            let text = format!("{:<width$}", truncate_end(&text, width));
            let style = if selected {
                t.selected()
            } else {
                Style::default().fg(t.text)
            };
            Line::from(Span::styled(text, style))
        })
        .collect();
    lines.push(Line::default());
    let status = app.sounds.status();
    let color = if app.sounds.bell_reason().is_some() {
        t.warning
    } else {
        t.text_muted
    };
    lines.push(Line::from(Span::styled(
        truncate_end(&format!("  {status}"), width),
        Style::default().fg(color),
    )));
    if effective.when == When::Unfocused {
        lines.push(Line::from(Span::styled(
            truncate_end("  alerts sound while the terminal is unfocused", width),
            Style::default().fg(t.text_muted),
        )));
    } else {
        lines.push(Line::default());
    }
    lines.push(Line::default());
    lines.push(Line::from(Span::styled(
        truncate_end(&format!("  {KEYS}"), width),
        Style::default().fg(t.text_muted),
    )));
    f.render_widget(Clear, rect);
    f.render_widget(Paragraph::new(lines).block(frame(app, "Sounds")), rect);
}

fn label(row: Row) -> &'static str {
    match row {
        Row::Enabled => "Sounds",
        Row::Pack => "Pack",
        Row::Volume => "Volume",
        Row::When => "When",
        Row::Event(alert) => alert.name(),
    }
}

/// The row's state in words: what `/sounds` chose, and what a project turns down.
fn value(row: Row, chosen: &Settings, effective: &Settings) -> String {
    let project = |limited: bool| {
        if limited {
            " · off in this project"
        } else {
            ""
        }
    };
    match row {
        Row::Enabled => format!(
            "{}{}",
            on_off(chosen.enabled),
            project(chosen.enabled && !effective.enabled)
        ),
        Row::Pack => chips(Pack::ALL.map(Pack::name).as_slice(), chosen.pack.name()),
        Row::Volume => {
            let limited = if effective.volume < chosen.volume {
                format!(" · {} in this project", effective.volume)
            } else {
                String::new()
            };
            format!("{:<5}{}{limited}", chosen.volume, meter(chosen.volume))
        }
        Row::When => chips(When::ALL.map(When::name).as_slice(), chosen.when.name()),
        Row::Event(alert) => {
            let state = event(chosen.event(alert));
            let limited = project(chosen.event(alert).on() && !effective.event(alert).on());
            format!("{:<14}{}{limited}", state, alert.describe())
        }
    }
}

fn on_off(on: bool) -> &'static str {
    if on { "on" } else { "off" }
}

/// `on`, `off`, `on · soft` (another pack's sound) or `on · done.wav` (a file).
fn event(choice: &Choice) -> String {
    match choice {
        Choice::On => "on".into(),
        Choice::Off => "off".into(),
        Choice::Pack(pack) => format!("on · {}", pack.name()),
        Choice::File(path) => format!(
            "on · {}",
            path.file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default()
        ),
    }
}

/// `[cyber]  soft   retro  bell`: the current value in brackets.
fn chips(all: &[&str], current: &str) -> String {
    all.iter()
        .map(|name| {
            if *name == current {
                format!("[{name}]")
            } else {
                format!(" {name} ")
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Ten cells, one per step of 10.
fn meter(volume: u8) -> String {
    let filled = usize::from(volume / 10);
    format!("{}{}", "■".repeat(filled), "·".repeat(10 - filled))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sound::Alert;

    #[test]
    fn states_are_words() {
        let mut chosen = Settings::default();
        *chosen.event_mut(Alert::Done) = Choice::File("/s/done.wav".into());
        *chosen.event_mut(Alert::Error) = Choice::Pack(Pack::Soft);
        let mut effective = chosen.clone();
        effective.enabled = false;
        *effective.event_mut(Alert::Message) = Choice::Off;
        effective.volume = 30;
        assert_eq!(
            value(Row::Enabled, &chosen, &effective),
            "on · off in this project"
        );
        assert_eq!(
            value(Row::Pack, &chosen, &effective),
            "[cyber]  soft   retro   bell "
        );
        assert_eq!(
            value(Row::Volume, &chosen, &effective),
            "60   ■■■■■■···· · 30 in this project"
        );
        assert!(value(Row::Event(Alert::Done), &chosen, &effective).starts_with("on · done.wav"));
        assert!(value(Row::Event(Alert::Error), &chosen, &effective).starts_with("on · soft"));
        assert!(
            value(Row::Event(Alert::Message), &chosen, &effective)
                .ends_with("a message arrived or is held · off in this project")
        );
    }
}
