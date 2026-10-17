//! The progress line above the composer (`tui` → Nyan progress indicator): a rainbow
//! trail and the cat, then the activity label. The trail is a smooth hue gradient that
//! flows with the frame, every cell a `▀` or `▄` half block chosen by a travelling wave,
//! fading toward the background at its tail. Unknown progress runs the cat across the bar
//! like a comet and wraps; known progress draws the fraction and the rest as `·`. While
//! the user is asked something the cat naps and nothing moves. `NO_COLOR` draws `=-`,
//! reduced motion the static cat, and screen readers get a sentence that changes at most
//! every ten seconds.

use std::time::Duration;

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::app::App;
use crate::progress::{Activity, SPOKEN_STEP_SECS, Spinner, TurnPhase};

/// The cat without its tail glyph, which alternates per frame (see [`TAILS`]).
const CAT_BODY: &str = "[,,_,,]:3";
const TAILS: [char; 4] = ['~', '-', '~', '='];
const STILL_CAT: &str = "[,,_,,]:3";
const NAPPING: &str = "zZ";
/// The sparkle ahead of the running cat, shown on half of the frames.
const SPARKLE: &str = "✦";
/// Every row of the tall cat is this wide; the slim cat is its tail plus [`CAT_BODY`].
pub(crate) const CAT_WIDTH: usize = 10;
const TALL_CAT: [&str; 4] = [" ,------,", " | /\\_/\\", "~|_( ^ .^)", "  \"\"  \"\""];
/// The tall indicator's rows: six band half-rows plus the wave's half-row offset.
const TALL_ROWS: u16 = 4;
/// `nyan-tall` needs a terminal at least this tall.
const TALL_FROM: u16 = 30;
/// Hue change per cell along the trail and per frame, in turns of the color wheel.
const HUE_PER_CELL: f64 = 0.035;
const HUE_PER_FRAME: f64 = 0.06;
/// The tall trail's bands are this far apart in hue.
const HUE_PER_BAND: f64 = 0.12;
/// The wave: its phase moves this many cells per frame, with this many radians per cell.
const WAVE_SPEED: f64 = 0.9;
const WAVE_SCALE: f64 = 0.55;
/// The faded tail keeps this share of its color.
const TAIL_FLOOR: f64 = 0.25;
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
    rows >= TALL_FROM
        && !app.reduced_motion
        && !app.theme.monochrome
        && !app.screen_reader
        && !app.waiting_on_user()
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
            _ if app.reduced_motion || app.waiting_on_user() => vec![still_line(app, activity)],
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
        Activity::Turn(d) => turn_label(app, d),
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
    if app.session.running && activity.known().is_none() && !app.waiting_on_user() {
        spans.push(Span::styled(" · ", muted));
        spans.push(Span::styled("esc", strong(t.accent)));
        spans.push(Span::styled(" interrupt", muted));
    }
    spans
}

/// `Thinking · 12.4s`, `Running bash cargo test · 3.0s`, or `Waiting for approval ·
/// bash ps -axo … · 12.0s · y allow · n reject`.
fn turn_label(app: &App, d: Duration) -> Vec<Span<'static>> {
    let t = &app.theme;
    let muted = Style::default().fg(t.text_muted);
    let strong = |color: Color| Style::default().fg(color).add_modifier(Modifier::BOLD);
    let time = Span::styled(format!(" · {}", elapsed(d)), muted);
    match app.turn_phase() {
        TurnPhase::Thinking => vec![Span::styled("Thinking", strong(t.text)), time],
        TurnPhase::Running(tool) => vec![
            Span::styled("Running", strong(t.text)),
            Span::styled(format!(" {}", label_text(&tool)), muted),
            time,
        ],
        TurnPhase::Waiting { permission, about } => {
            let what = if permission {
                "Waiting for approval"
            } else {
                "Waiting for your answer"
            };
            let mut spans = vec![Span::styled(what, strong(t.warning))];
            if !about.is_empty() {
                spans.push(Span::styled(format!(" · {}", label_text(&about)), muted));
            }
            spans.push(time);
            if permission {
                for (key, word) in [("y", "allow"), ("n", "reject")] {
                    spans.push(Span::styled(" · ", muted));
                    spans.push(Span::styled(key, strong(t.accent)));
                    spans.push(Span::styled(format!(" {word}"), muted));
                }
            }
            spans
        }
    }
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

/// The background the trail fades into and is drawn on; `None` for the terminal's own.
fn backdrop(app: &App) -> Option<(u8, u8, u8)> {
    match app.theme.background {
        Color::Rgb(r, g, b) => Some((r, g, b)),
        _ => None,
    }
}

/// HSV with full value to RGB; `h` in turns.
fn hsv(h: f64, s: f64) -> (u8, u8, u8) {
    let h = h.rem_euclid(1.0) * 6.0;
    let f = h - h.floor();
    let (p, q, t) = (1.0 - s, 1.0 - s * f, 1.0 - s * (1.0 - f));
    let (r, g, b) = match h as u8 {
        0 => (1.0, t, p),
        1 => (q, 1.0, p),
        2 => (p, 1.0, t),
        3 => (p, q, 1.0),
        4 => (t, p, 1.0),
        _ => (1.0, p, q),
    };
    let byte = |v: f64| (v * 255.0).round() as u8;
    (byte(r), byte(g), byte(b))
}

/// `color` faded toward the backdrop, keeping `keep` (0–1) of it.
fn fade(app: &App, color: (u8, u8, u8), keep: f64) -> Color {
    let (r, g, b) = match backdrop(app) {
        Some((br, bg, bb)) => {
            let mix =
                |c: u8, back: u8| (back as f64 + (c as f64 - back as f64) * keep).round() as u8;
            (mix(color.0, br), mix(color.1, bg), mix(color.2, bb))
        }
        None => color,
    };
    crate::theme::color::reduce(Color::Rgb(r, g, b), app.appearance.depth)
}

/// The color of trail cell `i` of `n` (0 is the tail) at `frame`: the hue gradient flows
/// toward the cat, and the tail fades.
fn trail_color(app: &App, i: usize, n: usize, frame: u64, band: usize) -> Color {
    let hue = i as f64 * HUE_PER_CELL - frame as f64 * HUE_PER_FRAME + band as f64 * HUE_PER_BAND;
    let along = if n > 1 {
        i as f64 / (n - 1) as f64
    } else {
        1.0
    };
    fade(app, hsv(hue, 0.85), TAIL_FLOOR + (1.0 - TAIL_FLOOR) * along)
}

/// The travelling wave: cell `i` sits in the upper half (`true`) or the lower half.
fn crest(i: usize, frame: u64) -> bool {
    ((i as f64 - frame as f64 * WAVE_SPEED) * WAVE_SCALE).sin() > 0.0
}

/// One half-block cell: `▀` or `▄` in `color`, on the backdrop.
fn half(app: &App, upper: bool, color: Color) -> Span<'static> {
    let mut style = Style::default().fg(color);
    if let Some((r, g, b)) = backdrop(app) {
        style = style.bg(crate::theme::color::reduce(
            Color::Rgb(r, g, b),
            app.appearance.depth,
        ));
    }
    Span::styled(if upper { "▀" } else { "▄" }, style)
}

/// `n` trail cells at the current frame.
fn trail(app: &App, n: usize) -> Vec<Span<'static>> {
    trail_at(app, n, app.progress.frame)
}

/// [`trail`] at `frame`; `=-` without color, shifting with the frame.
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
        .map(|i| half(app, crest(i, frame), trail_color(app, i, n, frame, 0)))
        .collect()
}

/// The running cat: its tail glyph alternates per frame.
fn cat(app: &App, frame: u64) -> Span<'static> {
    let tail = TAILS[(frame % TAILS.len() as u64) as usize];
    Span::styled(format!("{tail}{CAT_BODY}"), cat_style(app))
}

/// The sparkle cell ahead of the cat: on for three frames, off for three.
fn sparkle(app: &App, frame: u64) -> Span<'static> {
    if frame % 6 < 3 {
        let color = fade(app, hsv(frame as f64 * HUE_PER_FRAME, 0.6), 1.0);
        Span::styled(
            SPARKLE,
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        )
    } else {
        Span::raw(" ")
    }
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

/// Unknown progress: where the comet is in a bar of `room` cells at `frame`, as
/// `(lead, trail)`: blank cells before the trail, and the trail's length. The cat runs
/// one and a half cells per frame, its trail grows up to two thirds of the bar, and it
/// wraps back to the start.
fn comet(room: usize, frame: u64) -> (usize, usize) {
    if room == 0 {
        return (0, 0);
    }
    let head = (frame as usize * 3 / 2) % (room + 1);
    let longest = (room * 2 / 3).max(1);
    let trail = head.min(longest);
    (head - trail, trail)
}

/// A Workflow monitor bar of `width` cells in known-progress mode: the trail over
/// `done` of `total`, with the cat at its head when the bar has room for it, and the rest
/// as `·`. `frame` moves the trail; a row that does not animate passes 0.
pub fn bar(app: &App, done: u64, total: u64, width: usize, frame: u64) -> Vec<Span<'static>> {
    let fraction = if total == 0 {
        0.0
    } else {
        done.min(total) as f64 / total as f64
    };
    let with_cat = width >= CAT_WIDTH + 10;
    let room = if with_cat { width - CAT_WIDTH } else { width };
    let filled = ((fraction * room as f64).round() as usize).min(room);
    let mut spans = trail_at(app, filled, frame);
    if with_cat {
        spans.push(cat(app, frame));
    }
    if room > filled {
        spans.push(rest(app, room - filled));
    }
    spans
}

fn rest(app: &App, n: usize) -> Span<'static> {
    Span::styled("·".repeat(n), Style::default().fg(app.theme.text_subtle))
}

/// The bar of `room` cells, the cat and the sparkle cell, for the slim line.
fn slim_bar(app: &App, activity: Activity<'_>, room: usize) -> Vec<Span<'static>> {
    let frame = app.progress.frame;
    let mut spans = Vec::new();
    let tail_room = match activity.fraction() {
        Some(_) => {
            let (filled, empty) = split(activity, room);
            spans.extend(trail(app, filled));
            spans.push(cat(app, frame));
            spans.push(sparkle(app, frame));
            if empty > 0 {
                spans.push(rest(app, empty));
            }
            return spans;
        }
        None => comet(room, frame),
    };
    let (lead, length) = tail_room;
    if lead > 0 {
        spans.push(Span::raw(" ".repeat(lead)));
    }
    spans.extend(trail(app, length));
    spans.push(cat(app, frame));
    spans.push(sparkle(app, frame));
    let after = room - lead - length;
    if after > 0 {
        spans.push(Span::raw(" ".repeat(after)));
    }
    spans
}

fn slim_line(app: &App, activity: Activity<'_>, width: usize) -> Line<'static> {
    if app.waiting_on_user() {
        return still_line(app, activity);
    }
    let label = label(app, activity);
    // A space, the bar, the cat and its sparkle, two spaces, the label and a space.
    let room = width.saturating_sub(1 + CAT_WIDTH + 1 + 2 + spans_width(&label) + 1);
    let mut spans = vec![Span::raw(" ")];
    if room > 0 {
        spans.extend(slim_bar(app, activity, room));
        spans.push(Span::raw("  "));
    }
    spans.extend(label);
    Line::from(spans)
}

/// Reduced motion, or waiting on the user: the cat without its trail (napping while the
/// user is asked), and the label.
fn still_line(app: &App, activity: Activity<'_>) -> Line<'static> {
    let mut spans = vec![Span::raw(" "), Span::styled(STILL_CAT, cat_style(app))];
    if app.waiting_on_user() {
        spans.push(Span::styled(
            format!(" {NAPPING}"),
            Style::default()
                .fg(app.theme.text_muted)
                .add_modifier(Modifier::ITALIC),
        ));
    }
    spans.push(Span::raw("  "));
    spans.extend(label(app, activity));
    Line::from(spans)
}

/// `braille` and `dots`: one glyph per frame, `…` under reduced motion.
fn glyph_line(app: &App, activity: Activity<'_>) -> Line<'static> {
    let frame = app.progress.frame as usize;
    let glyph = match app.spinner() {
        _ if app.reduced_motion || app.waiting_on_user() => "…",
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

/// One cell of the tall trail: six bands, one per half-row, each a step around the hue
/// wheel from the column's flowing hue; the wave lowers every crest column by half a row.
fn tall_cell(app: &App, column: usize, columns: usize, row: usize) -> Span<'static> {
    let frame = app.progress.frame;
    let offset = usize::from(!crest(column, frame));
    // Half-rows 0..8; the bands take 1..7 or, lowered, 2..8.
    let band_at = |h: usize| h.checked_sub(1 + offset).filter(|&b| b < 6);
    let color = |b: usize| trail_color(app, column, columns, frame, b);
    match (band_at(2 * row), band_at(2 * row + 1)) {
        (None, None) => Span::raw(" "),
        (Some(top), Some(bottom)) => {
            Span::styled("▀", Style::default().fg(color(top)).bg(color(bottom)))
        }
        (Some(top), None) => half(app, true, color(top)),
        (None, Some(bottom)) => half(app, false, color(bottom)),
    }
}

/// `nyan-tall`: the wave, the 4-row cat, and the label on the second row.
fn tall_lines(app: &App, activity: Activity<'_>, width: usize) -> Vec<Line<'static>> {
    let label = label(app, activity);
    let room = width.saturating_sub(1 + CAT_WIDTH + 2 + spans_width(&label) + 1);
    let (lead, filled, empty) = match activity.fraction() {
        Some(_) => {
            let (filled, empty) = split(activity, room);
            (0, filled, empty)
        }
        None => {
            let (lead, length) = comet(room, app.progress.frame);
            (lead, length, room - lead - length)
        }
    };
    let blank = activity.fraction().is_none();
    (0..TALL_ROWS as usize)
        .map(|row| {
            let mut spans = vec![Span::raw(" ".repeat(1 + lead))];
            spans.extend((0..filled).map(|column| tall_cell(app, column, filled, row)));
            spans.push(Span::styled(
                format!("{:<CAT_WIDTH$}", TALL_CAT[row]),
                cat_style(app),
            ));
            if blank && empty > 0 {
                spans.push(Span::raw(" ".repeat(empty)));
            }
            if row == 1 {
                if !blank && empty > 0 {
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
    }

    #[test]
    fn the_comet_runs_grows_and_wraps() {
        let room = 30;
        assert_eq!(comet(room, 0), (0, 0));
        // One and a half cells per frame; the trail follows the head.
        assert_eq!(comet(room, 4), (0, 6));
        // The trail stops growing at two thirds of the bar and the comet moves on.
        assert_eq!(comet(room, 18), (7, 20));
        let (lead, trail) = comet(room, 20);
        assert_eq!(lead + trail, 30, "the head reaches the end");
        assert_eq!(comet(room, 21), (0, 0), "then wraps to the start");
        assert_eq!(comet(0, 9), (0, 0));
    }

    #[test]
    fn the_hue_flows_and_the_wave_travels() {
        // A crest moves one cell forward in about a frame (0.9 cells per frame).
        let crests = |frame| (0..40).filter(|&i| crest(i, frame)).collect::<Vec<_>>();
        assert_ne!(crests(0), crests(1));
        assert_eq!(hsv(0.0, 1.0), (255, 0, 0));
        assert_eq!(hsv(1.0 / 3.0, 1.0), (0, 255, 0));
        assert_eq!(hsv(1.0, 1.0), hsv(0.0, 1.0), "the wheel wraps");
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
