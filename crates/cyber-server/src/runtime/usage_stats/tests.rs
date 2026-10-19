use cyber_llm::Usage;
use jiff::civil::date;
use jiff::tz::TimeZone;

use super::aggregate::{Row, Span, When, Window, aggregate, streaks};
use super::{Period, plausible_zone, resolve_zone};

const HOUR: i64 = 3_600_000;

fn ms(zone: &str, y: i16, m: i8, d: i8, h: i8) -> i64 {
    date(y, m, d)
        .at(h, 0, 0, 0)
        .in_tz(zone)
        .unwrap()
        .timestamp()
        .as_millisecond()
}

fn row(when: i64, root: &str, model: &str, tokens: u64) -> Row {
    Row {
        when: When::Bucket(when),
        root: root.into(),
        provider: "test".into(),
        model: model.into(),
        directory: "/repo".into(),
        steps: 1,
        turns: 1,
        unpriced: 0,
        tokens: Usage {
            input: tokens,
            output: 0,
            reasoning: 0,
            cache_read: 0,
            cache_write: 0,
        },
        cost: 0.5,
        origin: "user".into(),
    }
}

fn window(zone: &str, period: Period, now: i64) -> Window {
    Window::new(TimeZone::get(zone).unwrap(), period, now).unwrap()
}

#[test]
fn periods_parse_within_bounds() {
    assert_eq!(Period::parse("all"), Some(Period::All));
    assert_eq!(Period::parse("7d"), Some(Period::Days(7)));
    assert_eq!(Period::parse("371d"), Some(Period::Days(371)));
    for bad in ["0d", "372d", "d", "+7d", "7", "-1d", "1000d", "7 d", ""] {
        assert_eq!(Period::parse(bad), None, "{bad}");
    }
}

#[test]
fn zone_names_are_checked_before_any_lookup() {
    assert!(plausible_zone("America/New_York"));
    assert!(plausible_zone("Etc/GMT+5"));
    for bad in [
        "../../etc/passwd",
        "/etc/localtime",
        "Europe/\u{1b}[31m",
        "",
    ] {
        assert!(!plausible_zone(bad), "{bad:?}");
    }
    // An unknown or unsafe name falls back to the server's zone.
    let (_, name) = resolve_zone(Some("../../etc/passwd"));
    let (_, system) = resolve_zone(None);
    assert_eq!(name, system);
    assert_eq!(resolve_zone(Some("Asia/Tokyo")).1, "Asia/Tokyo");
}

#[test]
fn streaks_count_back_from_today_or_yesterday() {
    assert_eq!(streaks(&[]), (0, 0));
    assert_eq!(streaks(&[false, false]), (0, 0));
    // 14 active days ending today, after an idle day.
    let mut days = vec![true, true, true, false];
    days.extend([true; 14]);
    assert_eq!(streaks(&days), (14, 14));
    // Today has no Turn yet: the streak through yesterday is still current.
    days.push(false);
    assert_eq!(streaks(&days), (14, 14));
    // Two idle days end it.
    days.push(false);
    assert_eq!(streaks(&days), (14, 0));
    assert_eq!(streaks(&[true, true, true, false, true]), (3, 1));
}

#[test]
fn the_last_14_local_days_are_a_14_day_streak() {
    let zone = "Europe/Lisbon";
    let now = ms(zone, 2026, 10, 16, 12);
    let rows: Vec<Row> = (0..14)
        .map(|d| row(now - d * 24 * HOUR, "ses_a", "m", 10))
        .collect();
    let stats = aggregate(&rows, &[], &window(zone, Period::Days(30), now));
    assert_eq!(stats.days.len(), 30);
    assert_eq!(stats.period_days, 30);
    assert_eq!(stats.current_streak, 14);
    assert_eq!(stats.longest_streak, 14);
    assert_eq!(stats.active_days, 14);
    assert_eq!(stats.days.last().unwrap().date, "2026-10-16");
    assert_eq!(stats.days[0].date, "2026-09-17");
}

#[test]
fn local_midnight_decides_the_day_not_utc() {
    // 23:30 in New York on Oct 15 is already Oct 16 in UTC.
    let zone = "America/New_York";
    let late = ms(zone, 2026, 10, 15, 23) + 30 * 60_000;
    let now = ms(zone, 2026, 10, 16, 9);
    let stats = aggregate(
        &[row(late, "ses_a", "m", 5)],
        &[],
        &window(zone, Period::Days(2), now),
    );
    let dates: Vec<_> = stats
        .days
        .iter()
        .map(|d| (d.date.as_str(), d.turns))
        .collect();
    assert_eq!(dates, [("2026-10-15", 1), ("2026-10-16", 0)]);
    // Today has no Turn, yesterday had one.
    assert_eq!(stats.current_streak, 1);
    assert_eq!(stats.most_active_day.as_deref(), Some("2026-10-15"));
}

#[test]
fn days_stay_contiguous_across_a_dst_change() {
    // Clocks went back on 2026-11-01 in New York: that day has 25 hours.
    let zone = "America/New_York";
    let now = ms(zone, 2026, 11, 3, 12);
    let w = window(zone, Period::Days(5), now);
    assert_eq!(w.since_ms(), Some(ms(zone, 2026, 10, 30, 0)));
    let rows = [
        // 00:30 and 23:30 on the long day are the same local day.
        row(ms(zone, 2026, 11, 1, 0) + 30 * 60_000, "ses_a", "m", 1),
        row(ms(zone, 2026, 11, 1, 23) + 30 * 60_000, "ses_b", "m", 2),
    ];
    let stats = aggregate(&rows, &[], &w);
    let dates: Vec<_> = stats.days.iter().map(|d| d.date.as_str()).collect();
    assert_eq!(
        dates,
        [
            "2026-10-30",
            "2026-10-31",
            "2026-11-01",
            "2026-11-02",
            "2026-11-03"
        ]
    );
    assert_eq!(stats.days[2].total_tokens, 3);
    assert_eq!(stats.days[2].sessions, 2);
    // Clocks went forward on 2026-03-08: a 23-hour day is still one day.
    let spring = window(zone, Period::Days(3), ms(zone, 2026, 3, 9, 12));
    let stats = aggregate(&[], &[], &spring);
    let dates: Vec<_> = stats.days.iter().map(|d| d.date.as_str()).collect();
    assert_eq!(dates, ["2026-03-07", "2026-03-08", "2026-03-09"]);
}

#[test]
fn an_empty_period_has_zero_days_and_no_favorites() {
    let zone = "UTC";
    let now = ms(zone, 2026, 10, 16, 12);
    let stats = aggregate(&[], &[], &window(zone, Period::Days(7), now));
    assert_eq!(stats.days.len(), 7);
    assert!(
        stats
            .days
            .iter()
            .all(|d| d.turns == 0 && d.total_tokens == 0)
    );
    assert_eq!((stats.active_days, stats.period_days), (0, 7));
    assert_eq!((stats.longest_streak, stats.current_streak), (0, 0));
    assert_eq!(stats.totals.cost, None);
    assert!(stats.favorite_model.is_none());
    assert!(stats.most_active_day.is_none());
    assert!(stats.longest_session.is_none());
    assert!(stats.models.is_empty());
    // `all` without any usage is just today.
    let all = aggregate(&[], &[], &window(zone, Period::All, now));
    assert_eq!(all.period_days, 1);
    assert_eq!(all.since_ms, None);
}

#[test]
fn all_time_starts_at_the_first_day_and_returns_at_most_371_days() {
    let zone = "UTC";
    let now = ms(zone, 2026, 10, 16, 12);
    let old = ms(zone, 2024, 10, 16, 12);
    let stats = aggregate(
        &[row(old, "ses_old", "m", 1), row(now, "ses_new", "m", 1)],
        &[],
        &window(zone, Period::All, now),
    );
    assert_eq!(stats.period_days, 731);
    assert_eq!(stats.days.len(), 371);
    assert_eq!(stats.active_days, 2);
    assert_eq!(stats.totals.sessions, 2);
}

#[test]
fn models_favorites_and_the_longest_session() {
    let zone = "UTC";
    let now = ms(zone, 2026, 10, 16, 12);
    let mut unpriced = row(now, "ses_b", "small", 10);
    unpriced.unpriced = 1;
    unpriced.cost = 0.0;
    let mut title = row(now, "ses_a", "small", 0);
    title.turns = 0;
    let rows = [row(now - HOUR, "ses_a", "big", 30), unpriced, title];
    let spans = [
        Span {
            root: "ses_a".into(),
            directory: "/repo".into(),
            first: now - 3 * HOUR,
            last: now,
        },
        Span {
            root: "ses_b".into(),
            directory: "/repo".into(),
            first: now - HOUR,
            last: now,
        },
        Span {
            root: "ses_a".into(),
            directory: "/other".into(),
            first: now - 5 * HOUR,
            last: now - 4 * HOUR,
        },
    ];
    let stats = aggregate(&rows, &spans, &window(zone, Period::Days(7), now));
    let names: Vec<_> = stats.models.iter().map(|m| m.model.as_str()).collect();
    assert_eq!(names, ["big", "small"]);
    assert_eq!(stats.favorite_model.as_ref().unwrap().model, "big");
    assert_eq!(stats.models[0].share, 0.75);
    assert_eq!(stats.models[1].sessions, 2);
    assert_eq!(stats.models[1].turns, 1);
    // `small` has one priced call and one unpriced call.
    assert_eq!(stats.models[1].cost, Some(0.5));
    assert_eq!(stats.totals.unpriced_steps, 1);
    assert_eq!(stats.totals.turns, 2);
    assert_eq!(stats.totals.sessions, 2);
    let longest = stats.longest_session.unwrap();
    assert_eq!(
        (longest.id.as_str(), longest.duration_ms),
        ("ses_a", 5 * HOUR as u64)
    );
}

#[test]
fn folded_days_are_flagged_approximate() {
    let zone = "UTC";
    let now = ms(zone, 2026, 10, 16, 12);
    let mut folded = row(0, "ses_old", "m", 7);
    folded.when = When::Day(date(2026, 10, 10));
    let stats = aggregate(&[folded], &[], &window(zone, Period::Days(7), now));
    assert!(stats.approximate_days);
    assert_eq!(stats.days[0].date, "2026-10-10");
    assert_eq!(stats.days[0].total_tokens, 7);
}

/// Task 9.3: each origin is its own entry with its tokens, most tokens first.
#[test]
fn usage_splits_by_origin() {
    let zone = "UTC";
    let now = ms(zone, 2026, 10, 19, 12);
    let mut rows = vec![row(now - HOUR, "ses_a", "main", 300)];
    for (origin, tokens) in [("session", 200), ("client", 100), ("job", 50)] {
        let mut r = row(now - HOUR, "ses_b", "main", tokens);
        r.origin = origin.into();
        rows.push(r);
    }
    let mut unpriced = row(now - HOUR, "ses_b", "main", 7);
    (unpriced.origin, unpriced.unpriced) = ("mcp".into(), 1);
    rows.push(unpriced);
    let stats = aggregate(&rows, &[], &window(zone, Period::Days(1), now));
    let origins: Vec<(&str, u64, Option<f64>, u64)> = stats
        .origins
        .iter()
        .map(|o| (o.origin.as_str(), o.total_tokens, o.cost, o.sessions))
        .collect();
    assert_eq!(
        origins,
        [
            ("user", 300, Some(0.5), 1),
            ("session", 200, Some(0.5), 1),
            ("client", 100, Some(0.5), 1),
            ("job", 50, Some(0.5), 1),
            ("mcp", 7, None, 1),
        ]
    );
}
