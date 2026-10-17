//! The progress line above the composer (`tui` → Nyan progress indicator): a rainbow
//! trail of `▀` half-blocks and the cat, then the activity label. Unknown progress fills
//! the width and scrolls one cell per frame; known progress draws the fraction and the
//! rest as `·`. `NO_COLOR` draws `=-`, reduced motion the static cat, and screen readers
//! get a sentence that changes at most every ten seconds.

use std::time::Duration;

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::app::App;
use crate::progress::{Activity, SPOKEN_STEP_SECS, Spinner};

const CAT: &str = "~[,,_,,]:3";
const STILL_CAT: &str = "[,,_,,]:3";
/// Every row of the tall cat is this wide.
pub(crate) const CAT_WIDTH: usize = 10;
const TALL_CAT: [&str; 4] = [" ,------,", " | /\\_/\\", "~|_( ^ .^)", "  \"\"  \"\""];
/// The tall indicator's rows: three for the six bands, one for the wave's offset.
const TALL_ROWS: u16 = 4;
/// `nyan-tall` needs a terminal at least this tall.
const TALL_FROM: u16 = 30;
/// Red, orange, yellow, green, blue, purple.
const RAINBOW: [Color; 6] = [
    Color::Rgb(0xff, 0x3b, 0x3b),
    Color::Rgb(0xff, 0x9a, 0x1f),
    Color::Rgb(0xff, 0xe1, 0x4d),
    Color::Rgb(0x4d, 0xdb, 0x6b),
    Color::Rgb(0x3a, 0xa0, 0xff),
    Color::Rgb(0x9b, 0x5c, 0xff),
];
const PINK: Color = Color::Rgb(0xff, 0x9a, 0xd5);
const PINK_ON_LIGHT: Color = Color::Rgb(0xc2, 0x18, 0x7a);
const BRAILLE: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
const DOTS: [&str; 4] = ["·  ", "·· ", "···", "   "];
/// The longest workflow or transfer label shown.
const MAX_LABEL: usize = 40;

/// The progress line is drawn: something is working and `tui.spinner` is not `off`.
pub fn shown(app: &App) -> bool {
    app.activity().is_some() && app.spinner() != Spinner::Off
}

/// Rows the indicator takes on a terminal `rows` tall.
pub fn height(app: &App, rows: u16) -> u16 {
    if !shown(app) {
        0
    } else if app.spinner() == Spinner::NyanTall && tall(app, rows) {
        TALL_ROWS
    } else {
        1
    }
}

/// `nyan-tall` draws its 4 rows only with color and motion, on 30 or more rows.
fn tall(app: &App, rows: u16) -> bool {
    rows >= TALL_FROM && !app.reduced_motion && !app.theme.monochrome && !app.screen_reader
}

pub fn draw(f: &mut Frame, app: &App, area: Rect) {
    let activity = app.activity().filter(|_| shown(app));
    let visible = area.height > 0 && area.width > 0 && activity.is_some();
    app.progress.set_on_screen(visible);
    let Some(activity) = activity.filter(|_| visible) else {
        return;
    };
    let width = area.width as usize;
    let lines = if app.screen_reader {
        vec![Line::from(Span::styled(
            spoken(activity),
            Style::default().fg(app.theme.text),
        ))]
    } else {
        match app.spinner() {
            Spinner::Braille | Spinner::Dots => vec![glyph_line(app, activity)],
            _ if app.reduced_motion => vec![still_line(app, activity)],
            Spinner::NyanTall if area.height >= TALL_ROWS => tall_lines(app, activity, width),
            _ => vec![slim_line(app, activity, width)],
        }
    };
    f.render_widget(Paragraph::new(lines), area);
}

/// `Working, 20 seconds` in steps of ten seconds, then what changed the label.
pub fn spoken(activity: Activity<'_>) -> String {
    let working = |elapsed: Duration| {
        let step = elapsed.as_secs() / SPOKEN_STEP_SECS * SPOKEN_STEP_SECS;
        if step == 0 {
            "Working".to_owned()
        } else {
            format!("Working, {step} seconds")
        }
    };
    let percent = activity
        .known()
        .map(|(done, total)| format!(", {}%", done * 100 / total))
        .unwrap_or_default();
    match activity {
        Activity::Turn(elapsed) => working(elapsed),
        Activity::Compaction(elapsed) => format!("{}, compacting", working(elapsed)),
        Activity::Workflow(w) => format!(
            "Working, {}, {} of {} agents{percent}",
            label_text(&w.label),
            w.done,
            w.total
        ),
        Activity::Transfer(t) => format!(
            "Working, {}, {}{percent}",
            label_text(&t.label),
            transferred(t.done, t.total)
        ),
    }
}

/// `12.4s`, `2m 05s`.
fn elapsed(d: Duration) -> String {
    let tenths = d.as_millis() / 100;
    if tenths < 600 {
        format!("{}.{}s", tenths / 10, tenths % 10)
    } else {
        let secs = d.as_secs();
        format!("{}m {:02}s", secs / 60, secs % 60)
    }
}

/// `950 B`, `3.2 MB`.
fn bytes(n: u64) -> String {
    let mut value = n as f64;
    for unit in ["B", "KB", "MB", "GB"] {
        if value < 1000.0 || unit == "GB" {
            return if unit == "B" {
                format!("{n} B")
            } else {
                format!("{value:.1} {unit}")
            };
        }
        value /= 1000.0;
    }
    unreachable!("the loop returns at GB")
}

/// `3.2 MB/10.0 MB`, or what moved so far when the size is unknown.
fn transferred(done: u64, total: u64) -> String {
    if total == 0 {
        bytes(done)
    } else {
        format!("{}/{}", bytes(done.min(total)), bytes(total))
    }
}

/// Untrusted names on one line, cut to [`MAX_LABEL`] columns.
fn label_text(text: &str) -> String {
    super::layout::truncate_end(&crate::sidebar::clean(text), MAX_LABEL)
}

/// `Thinking · 12.4s · esc interrupt`, `compacting · 3.1s`, `fanout 7/10 agents · 70%`.
fn label(app: &App, activity: Activity<'_>) -> Vec<Span<'static>> {
    let t = &app.theme;
    let muted = Style::default().fg(t.text_muted);
    let strong = |color: Color| Style::default().fg(color).add_modifier(Modifier::BOLD);
    let percent = activity
        .known()
        .map(|(done, total)| Span::styled(format!(" · {}%", done * 100 / total), muted));
    let mut spans = match activity {
        Activity::Turn(d) => vec![
            Span::styled("Thinking", strong(t.text)),
            Span::styled(format!(" · {}", elapsed(d)), muted),
        ],
        Activity::Compaction(d) => vec![
            Span::styled("compacting", strong(t.warning)),
            Span::styled(format!(" · {}", elapsed(d)), muted),
        ],
        Activity::Workflow(w) => vec![
            Span::styled(label_text(&w.label), strong(t.text)),
            Span::styled(
                format!(" {}/{} agents", w.done.min(w.total), w.total),
                Style::default().fg(t.success),
            ),
        ],
        Activity::Transfer(x) => vec![
            Span::styled(label_text(&x.label), strong(t.text)),
            Span::styled(
                format!(" {}", transferred(x.done, x.total)),
                Style::default().fg(t.success),
            ),
        ],
    };
    spans.extend(percent);
    if app.session.running && activity.known().is_none() {
        spans.push(Span::styled(" · ", muted));
        spans.push(Span::styled("esc", strong(t.accent)));
        spans.push(Span::styled(" interrupt", muted));
    }
    spans
}

fn spans_width(spans: &[Span<'_>]) -> usize {
    spans.iter().map(Span::width).sum()
}

fn cat_style(app: &App) -> Style {
    let pink = if app.theme.light { PINK_ON_LIGHT } else { PINK };
    Style::default()
        .fg(crate::theme::color::reduce(pink, app.appearance.depth))
        .add_modifier(Modifier::BOLD)
}

fn band(app: &App, k: usize) -> Color {
    crate::theme::color::reduce(RAINBOW[k % RAINBOW.len()], app.appearance.depth)
}

/// The band of cell `i` at `frame`: the hues move one cell toward the cat per frame.
fn hue(i: usize, frame: u64) -> usize {
    (i as i64 - (frame % RAINBOW.len() as u64) as i64).rem_euclid(RAINBOW.len() as i64) as usize
}

/// `n` trail cells: two adjacent bands per `▀`, or `=-` without color.
fn trail(app: &App, n: usize) -> Vec<Span<'static>> {
    trail_at(app, n, app.progress.frame)
}

/// [`trail`] at `frame`.
fn trail_at(app: &App, n: usize, frame: u64) -> Vec<Span<'static>> {
    if app.theme.monochrome {
        let text: String = (0..n)
            .map(|i| {
                if (i as u64 + frame).is_multiple_of(2) {
                    '='
                } else {
                    '-'
                }
            })
            .collect();
        return vec![Span::raw(text)];
    }
    (0..n)
        .map(|i| {
            let k = hue(i, frame);
            Span::styled("▀", Style::default().fg(band(app, k)).bg(band(app, k + 1)))
        })
        .collect()
}

/// Trail cells and the `·` rest for a bar of `room` cells besides the cat.
fn split(activity: Activity<'_>, room: usize) -> (usize, usize) {
    match activity.fraction() {
        None => (room, 0),
        Some(fraction) => {
            let filled = ((fraction * room as f64).round() as usize).min(room);
            (filled, room - filled)
        }
    }
}

/// A Workflow monitor bar of `width` cells in known-progress mode: the trail over
/// `done` of `total`, with the cat at its head when the bar has room for it, and the rest
/// as `·`. `frame` moves the hues; a row that does not animate passes 0.
pub fn bar(app: &App, done: u64, total: u64, width: usize, frame: u64) -> Vec<Span<'static>> {
    let fraction = if total == 0 {
        0.0
    } else {
        done.min(total) as f64 / total as f64
    };
    let cat = width >= CAT_WIDTH + 10;
    let room = if cat { width - CAT_WIDTH } else { width };
    let filled = ((fraction * room as f64).round() as usize).min(room);
    let mut spans = trail_at(app, filled, frame);
    if cat {
        spans.push(Span::styled(CAT, cat_style(app)));
    }
    if room > filled {
        spans.push(rest(app, room - filled));
    }
    spans
}

fn rest(app: &App, n: usize) -> Span<'static> {
    Span::styled("·".repeat(n), Style::default().fg(app.theme.text_subtle))
}

fn slim_line(app: &App, activity: Activity<'_>, width: usize) -> Line<'static> {
    let label = label(app, activity);
    // A space, the bar, two spaces, the label and a space at the end.
    let room = width.saturating_sub(1 + CAT_WIDTH + 2 + spans_width(&label) + 1);
    let mut spans = vec![Span::raw(" ")];
    if room > 0 {
        let (filled, empty) = split(activity, room);
        spans.extend(trail(app, filled));
        spans.push(Span::styled(CAT, cat_style(app)));
        if empty > 0 {
            spans.push(rest(app, empty));
        }
        spans.push(Span::raw("  "));
    }
    spans.extend(label);
    Line::from(spans)
}

/// Reduced motion: the cat without its trail, and the label.
fn still_line(app: &App, activity: Activity<'_>) -> Line<'static> {
    let mut spans = vec![
        Span::raw(" "),
        Span::styled(STILL_CAT, cat_style(app)),
        Span::raw("  "),
    ];
    spans.extend(label(app, activity));
    Line::from(spans)
}

/// `braille` and `dots`: one glyph per frame, `…` under reduced motion.
fn glyph_line(app: &App, activity: Activity<'_>) -> Line<'static> {
    let frame = app.progress.frame as usize;
    let glyph = match app.spinner() {
        _ if app.reduced_motion => "…",
        Spinner::Dots => DOTS[frame % DOTS.len()],
        _ => BRAILLE[frame % BRAILLE.len()],
    };
    let mut spans = vec![
        Span::raw(" "),
        Span::styled(glyph, Style::default().fg(app.theme.accent)),
        Span::raw(" "),
    ];
    spans.extend(label(app, activity));
    Line::from(spans)
}

/// One column of the tall trail: six bands over eight half-rows, every other pair of
/// columns half a row lower.
fn tall_cell(app: &App, column: usize, row: usize) -> Span<'static> {
    let low = (column as u64 + app.progress.frame) / 2 % 2 == 1;
    let background = app.theme.background;
    let half = |h: usize| {
        let index = if low { h.checked_sub(1) } else { Some(h) };
        index
            .filter(|&b| b < RAINBOW.len())
            .map_or(background, |b| band(app, b))
    };
    let (top, bottom) = (half(2 * row), half(2 * row + 1));
    if top == background && bottom == background {
        return Span::raw(" ");
    }
    Span::styled("▀", Style::default().fg(top).bg(bottom))
}

/// `nyan-tall`: the wave, the 4-row cat, and the label on the second row.
fn tall_lines(app: &App, activity: Activity<'_>, width: usize) -> Vec<Line<'static>> {
    let label = label(app, activity);
    let room = width.saturating_sub(1 + CAT_WIDTH + 2 + spans_width(&label) + 1);
    let (filled, empty) = split(activity, room);
    (0..TALL_ROWS as usize)
        .map(|row| {
            let mut spans = vec![Span::raw(" ")];
            spans.extend((0..filled).map(|column| tall_cell(app, column, row)));
            spans.push(Span::styled(
                format!("{:<CAT_WIDTH$}", TALL_CAT[row]),
                cat_style(app),
            ));
            if row == 1 {
                if empty > 0 {
                    spans.push(rest(app, empty));
                }
                spans.push(Span::raw("  "));
                spans.extend(label.clone());
            }
            Line::from(spans)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn elapsed_bytes_and_hues() {
        assert_eq!(elapsed(Duration::from_millis(12_440)), "12.4s");
        assert_eq!(elapsed(Duration::from_millis(125_000)), "2m 05s");
        assert_eq!(bytes(950), "950 B");
        assert_eq!(bytes(3_200_000), "3.2 MB");
        assert_eq!(transferred(12, 0), "12 B");
        // The hues move one cell per frame.
        assert_eq!(hue(1, 1), hue(0, 0));
        assert_eq!(hue(0, 6), hue(0, 0));
    }

    #[test]
    fn screen_readers_hear_ten_second_steps() {
        let turn = |ms| spoken(Activity::Turn(Duration::from_millis(ms)));
        assert_eq!(turn(9_900), "Working");
        assert_eq!(turn(12_400), "Working, 10 seconds");
        assert_eq!(turn(19_999), "Working, 10 seconds");
        assert_eq!(turn(20_000), "Working, 20 seconds");
        let w = crate::progress::WorkflowProgress {
            label: "fan\u{1b}[2Jout".into(),
            done: 7,
            total: 10,
        };
        assert_eq!(
            spoken(Activity::Workflow(&w)),
            "Working, fan[2Jout, 7 of 10 agents, 70%"
        );
    }
}
