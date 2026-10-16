//! The Stats tab's activity heatmap (`tui` → Settings Stats tab): one column per week,
//! one row per weekday (Sunday first), newest week on the right, month labels above,
//! `Mon`/`Wed`/`Fri` row labels, and cells at quartile levels of the active days'
//! tokens. Each level has its own glyph, so the grid also reads without color.

use jiff::ToSpan;
use jiff::civil::Date;
use ratatui::style::Style;
use ratatui::text::{Line, Span};

use super::stats::Day;
use crate::theme::Tokens;

/// The most weeks drawn.
pub const MAX_WEEKS: usize = 53;
/// The weekday label column (`Mon `).
pub const LABEL: usize = 4;
/// Level glyphs: no Turn, then the four quartiles.
pub const GLYPHS: [&str; 5] = ["·", "░", "▒", "▓", "█"];
const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

pub fn month(date: Date) -> &'static str {
    MONTHS[(date.month() - 1) as usize]
}

/// The level (0–4) of every day: 0 without Turns, else its quartile among active days.
pub fn levels(days: &[(Date, &Day)]) -> Vec<u8> {
    let mut active: Vec<u64> = days
        .iter()
        .filter(|(_, d)| d.turns > 0)
        .map(|(_, d)| d.total_tokens)
        .collect();
    active.sort_unstable();
    let at = |q: usize| active.get((active.len() * q / 4).min(active.len().saturating_sub(1)));
    let cuts = [at(1), at(2), at(3)].map(|v| v.copied().unwrap_or(0));
    days.iter()
        .map(|(_, d)| match d.total_tokens {
            _ if d.turns == 0 => 0,
            t if t < cuts[0] => 1,
            t if t < cuts[1] => 2,
            t if t < cuts[2] => 3,
            _ => 4,
        })
        .collect()
}

fn style(t: &Tokens, level: u8) -> Style {
    match level {
        0 => Style::default().fg(t.text_subtle).bg(t.heat_0),
        1 => Style::default().fg(t.heat_1),
        2 => Style::default().fg(t.heat_2),
        3 => Style::default().fg(t.heat_3),
        _ => Style::default().fg(t.heat_4),
    }
}

/// The weeks that fit in `width`, at most 53, always covering `period` days when they
/// fit at all.
pub fn weeks(width: usize, last: Date, period: usize) -> usize {
    let lead = last.weekday().to_sunday_zero_offset() as usize;
    let needed = (period + 6 - lead).div_ceil(7).max(1);
    needed.min(MAX_WEEKS).min(width.saturating_sub(LABEL))
}

/// The month label row: each month's name above the first week holding its 1st, the
/// first column labelled too when the next label leaves room.
fn months(starts: &[Date]) -> String {
    let mut row = vec![' '; starts.len() + 3];
    let mut free = 0;
    for (i, start) in starts.iter().enumerate() {
        let end = start.checked_add(6.days()).unwrap_or(*start);
        let first = end.day() <= 7;
        let leading = i == 0
            && !starts
                .iter()
                .skip(1)
                .take(4)
                .any(|s| s.checked_add(6.days()).is_ok_and(|e| e.day() <= 7));
        if (first || leading) && i >= free {
            let name = month(if first { end } else { *start });
            for (j, c) in name.chars().enumerate() {
                row[i + j] = c;
            }
            free = i + 4;
        }
    }
    row.into_iter().collect::<String>().trim_end().to_owned()
}

/// The grid: the month row, seven weekday rows and the legend row. Days outside the
/// period stay blank.
pub fn lines(t: &Tokens, days: &[(Date, &Day)], width: usize) -> Vec<Line<'static>> {
    let (Some((first, _)), Some((last, _))) = (days.first(), days.last()) else {
        return Vec::new();
    };
    let n = weeks(width, *last, days.len());
    if n == 0 {
        return Vec::new();
    }
    let lead = last.weekday().to_sunday_zero_offset() as i64;
    let Ok(origin) = last.checked_sub((lead + 7 * (n as i64 - 1)).days()) else {
        return Vec::new();
    };
    let starts: Vec<Date> = origin.series(1.week()).take(n).collect();
    let levels = levels(days);
    let level = |date: Date| {
        (date >= *first && date <= *last)
            .then(|| days.iter().position(|(d, _)| *d == date))
            .flatten()
            .map(|i| levels[i])
    };
    let mut out = vec![Line::from(Span::styled(
        format!("{:LABEL$}{}", "", months(&starts)),
        Style::default().fg(t.text),
    ))];
    for weekday in 0..7 {
        let label = match weekday {
            1 => "Mon",
            3 => "Wed",
            5 => "Fri",
            _ => "",
        };
        let mut spans = vec![Span::styled(
            format!("{label:LABEL$}"),
            Style::default().fg(t.text),
        )];
        for start in &starts {
            let date = start.checked_add(weekday.days()).unwrap_or(*start);
            spans.push(match level(date) {
                Some(l) => Span::styled(GLYPHS[l as usize], style(t, l)),
                None => Span::raw(" "),
            });
        }
        out.push(Line::from(spans));
    }
    out.push(Line::default());
    out.push(legend(t));
    out
}

/// `Less ░ ▒ ▓ █ More`.
pub fn legend(t: &Tokens) -> Line<'static> {
    let mut spans = vec![Span::styled(
        format!("{:LABEL$}Less", ""),
        Style::default().fg(t.text),
    )];
    for level in 1..=4u8 {
        spans.push(Span::raw(" "));
        spans.push(Span::styled(GLYPHS[level as usize], style(t, level)));
    }
    spans.push(Span::styled(" More", Style::default().fg(t.text)));
    Line::from(spans)
}

#[cfg(test)]
mod tests {
    use super::*;
    use jiff::civil::date;

    fn day(turns: u64, total_tokens: u64) -> Day {
        Day {
            date: String::new(),
            total_tokens,
            turns,
        }
    }

    #[test]
    fn levels_are_quartiles_of_active_days() {
        let days = [
            day(0, 0),
            day(1, 10),
            day(1, 20),
            day(1, 30),
            day(1, 40),
            day(0, 99),
        ];
        let d = date(2026, 10, 16);
        let pairs: Vec<_> = days.iter().map(|x| (d, x)).collect();
        assert_eq!(levels(&pairs), [0, 1, 2, 3, 4, 0]);
        // One active day is the top level.
        assert_eq!(levels(&pairs[..2]), [0, 4]);
    }

    #[test]
    fn weeks_cover_the_period_and_fit_the_width() {
        // Friday 2026-10-16: a 7-day period spans two weeks (Sat..Fri).
        let friday = date(2026, 10, 16);
        assert_eq!(weeks(80, friday, 7), 2);
        assert_eq!(weeks(80, friday, 1), 1);
        assert_eq!(weeks(200, friday, 371), 53);
        assert_eq!(weeks(40, friday, 371), 36);
        assert_eq!(weeks(3, friday, 30), 0);
    }

    #[test]
    fn month_labels_sit_above_the_week_of_the_first() {
        let starts: Vec<Date> = date(2026, 9, 6).series(1.week()).take(9).collect();
        // Weeks start Sep 6, 13, 20, 27 (holds Oct 1), Oct 4, ..., Nov 1. `Sep` would
        // collide with `Oct`, so the first column stays unlabelled.
        assert_eq!(months(&starts), "   Oct  Nov");
        // Without a 1st in the next four weeks, the first column names its month.
        assert_eq!(months(&starts[..3]), "Sep");
    }
}
