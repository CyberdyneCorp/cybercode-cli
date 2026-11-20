//! Five-field cron expressions and one-shot times (`loops-scheduling` → In-session cron
//! tools, Persistent local schedules), shared by the cron tools and persistent schedules.
//!
//! ```text
//! minute hour day-of-month month day-of-week
//! ```
//!
//! Each field takes `*`, `N`, `A-B`, `*/S`, `A-B/S`, `N/S` and comma lists; months take
//! `JAN`–`DEC` and days `SUN`–`SAT` (any case), and day-of-week `7` is Sunday. When both
//! day fields are restricted a day matches if either does, as classic cron does. The
//! macros `@hourly`, `@daily`, `@weekly`, `@monthly` and `@yearly` are accepted.
//!
//! Occurrences are computed in a caller-chosen time zone: a local time a DST change skips
//! does not occur, and an ambiguous one occurs once, at its earlier instant.

use chrono::{
    DateTime, Datelike, Duration, LocalResult, NaiveDate, NaiveDateTime, TimeZone, Timelike, Utc,
};

/// How far ahead an occurrence is searched for.
const HORIZON_YEARS: i32 = 5;
/// The longest expression accepted.
const MAX_EXPRESSION: usize = 200;

const MONTHS: [&str; 12] = [
    "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec",
];
const DAYS: [&str; 7] = ["sun", "mon", "tue", "wed", "thu", "fri", "sat"];

/// A parsed cron expression.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cron {
    source: String,
    minutes: u64,
    hours: u64,
    days: u64,
    months: u64,
    weekdays: u64,
    /// Day of month or day of week is `*…`: both must then match (otherwise either).
    any_day: bool,
}

/// The bounds and names of one field.
struct Field {
    name: &'static str,
    min: u32,
    max: u32,
    names: &'static [&'static str],
    /// The value of the first name (`JAN` is 1, `SUN` is 0).
    first: u32,
}

const MINUTE: Field = Field {
    name: "minute",
    min: 0,
    max: 59,
    names: &[],
    first: 0,
};
const HOUR: Field = Field {
    name: "hour",
    min: 0,
    max: 23,
    names: &[],
    first: 0,
};
const DAY: Field = Field {
    name: "day of month",
    min: 1,
    max: 31,
    names: &[],
    first: 0,
};
const MONTH: Field = Field {
    name: "month",
    min: 1,
    max: 12,
    names: &MONTHS,
    first: 1,
};
const WEEKDAY: Field = Field {
    name: "day of week",
    min: 0,
    max: 7,
    names: &DAYS,
    first: 0,
};

impl Cron {
    /// Parse `expression`; the error is `Invalid cron expression "<expr>": <reason>`.
    pub fn parse(expression: &str) -> Result<Self, String> {
        let invalid = |reason: String| format!("Invalid cron expression {expression:?}: {reason}");
        if expression.len() > MAX_EXPRESSION {
            return Err(invalid(format!("longer than {MAX_EXPRESSION} characters")));
        }
        let text = expression.trim();
        let expanded = macro_expansion(text).unwrap_or(text);
        let fields: Vec<&str> = expanded.split_whitespace().collect();
        let [minute, hour, day, month, weekday] = fields.as_slice() else {
            return Err(invalid(format!(
                "expected 5 fields (minute hour day-of-month month day-of-week), got {}",
                fields.len()
            )));
        };
        let mut weekdays = field(weekday, &WEEKDAY).map_err(invalid)?;
        if weekdays & (1 << 7) != 0 {
            weekdays = (weekdays & !(1 << 7)) | 1;
        }
        let cron = Self {
            source: text.to_owned(),
            minutes: field(minute, &MINUTE).map_err(invalid)?,
            hours: field(hour, &HOUR).map_err(invalid)?,
            days: field(day, &DAY).map_err(invalid)?,
            months: field(month, &MONTH).map_err(invalid)?,
            weekdays,
            any_day: day.starts_with('*') || weekday.starts_with('*'),
        };
        let epoch = Utc.with_ymd_and_hms(2001, 1, 1, 0, 0, 0).single();
        if epoch
            .and_then(|start| cron.next_after(start, &Utc))
            .is_none()
        {
            return Err(invalid("it never matches a date".into()));
        }
        Ok(cron)
    }

    /// The expression as given (a macro stays a macro).
    pub fn as_str(&self) -> &str {
        &self.source
    }

    /// The first whole minute strictly after `after` that matches in `tz`; `None` when
    /// none does within five years.
    pub fn next_after<Tz: TimeZone>(&self, after: DateTime<Utc>, tz: &Tz) -> Option<DateTime<Utc>> {
        let local = after.with_timezone(tz).naive_local();
        let mut candidate = local.with_second(0)?.with_nanosecond(0)? + Duration::minutes(1);
        let limit =
            NaiveDate::from_ymd_opt(candidate.year() + HORIZON_YEARS, candidate.month(), 1)?;
        while candidate.date() <= limit {
            if let Some(next) = self.advance(candidate) {
                candidate = next;
                continue;
            }
            match tz.from_local_datetime(&candidate) {
                LocalResult::Single(time) | LocalResult::Ambiguous(time, _)
                    if time.with_timezone(&Utc) > after =>
                {
                    return Some(time.with_timezone(&Utc));
                }
                // Skipped by a DST change, or the earlier instant of a fold already passed.
                _ => candidate += Duration::minutes(1),
            }
        }
        None
    }

    /// The next candidate after a local time that does not match, `None` when it does.
    fn advance(&self, at: NaiveDateTime) -> Option<NaiveDateTime> {
        let date = at.date();
        if !bit(self.months, at.month()) {
            return first_of_next_month(date)?.and_hms_opt(0, 0, 0);
        }
        if !self.day_matches(date) {
            return date.succ_opt()?.and_hms_opt(0, 0, 0);
        }
        if !bit(self.hours, at.hour()) {
            let hour = at.with_minute(0)?;
            return Some(hour + Duration::hours(1));
        }
        if !bit(self.minutes, at.minute()) {
            return Some(at + Duration::minutes(1));
        }
        None
    }

    fn day_matches(&self, date: NaiveDate) -> bool {
        let day = bit(self.days, date.day());
        let weekday = bit(self.weekdays, date.weekday().num_days_from_sunday());
        if self.any_day {
            day && weekday
        } else {
            day || weekday
        }
    }
}

fn macro_expansion(text: &str) -> Option<&'static str> {
    Some(match text.to_ascii_lowercase().as_str() {
        "@hourly" => "0 * * * *",
        "@daily" | "@midnight" => "0 0 * * *",
        "@weekly" => "0 0 * * 0",
        "@monthly" => "0 0 1 * *",
        "@yearly" | "@annually" => "0 0 1 1 *",
        _ => return None,
    })
}

fn bit(set: u64, value: u32) -> bool {
    set & (1u64 << value) != 0
}

fn first_of_next_month(date: NaiveDate) -> Option<NaiveDate> {
    let (year, month) = match date.month() {
        12 => (date.year() + 1, 1),
        m => (date.year(), m + 1),
    };
    NaiveDate::from_ymd_opt(year, month, 1)
}

/// One field as a bit set of the values it matches.
fn field(text: &str, spec: &Field) -> Result<u64, String> {
    let mut set = 0u64;
    for part in text.split(',') {
        set |= part_bits(part, spec)?;
    }
    Ok(set)
}

fn part_bits(part: &str, spec: &Field) -> Result<u64, String> {
    let (range, step) = match part.split_once('/') {
        Some((range, step)) => {
            let step = step
                .parse::<u32>()
                .ok()
                .filter(|s| *s > 0)
                .ok_or_else(|| format!("bad step {step:?} in the {} field", spec.name))?;
            (range, Some(step))
        }
        None => (part, None),
    };
    let (low, high) = bounds(range, step.is_some(), spec)?;
    let mut set = 0u64;
    let mut value = low;
    while value <= high {
        set |= 1u64 << value;
        value += step.unwrap_or(1);
    }
    Ok(set)
}

/// The inclusive bounds of `*`, `A-B` or `N` (`N/S` runs from `N` to the field's end).
fn bounds(range: &str, stepped: bool, spec: &Field) -> Result<(u32, u32), String> {
    if range == "*" {
        return Ok((spec.min, spec.max));
    }
    if let Some((low, high)) = range.split_once('-') {
        let (low, high) = (value(low, spec)?, value(high, spec)?);
        if low > high {
            return Err(format!(
                "range {range} of the {} field is reversed",
                spec.name
            ));
        }
        return Ok((low, high));
    }
    let single = value(range, spec)?;
    Ok((single, if stepped { spec.max } else { single }))
}

fn value(text: &str, spec: &Field) -> Result<u32, String> {
    let named = spec
        .names
        .iter()
        .position(|name| name.eq_ignore_ascii_case(text))
        .and_then(|index| u32::try_from(index).ok())
        .map(|index| index + spec.first);
    let parsed = named.or_else(|| {
        text.bytes()
            .all(|b| b.is_ascii_digit())
            .then(|| text.parse::<u32>().ok())
            .flatten()
    });
    parsed
        .filter(|v| (spec.min..=spec.max).contains(v))
        .ok_or_else(|| {
            format!(
                "{text:?} is not a {} ({}-{})",
                spec.name, spec.min, spec.max
            )
        })
}

/// When a one-shot task runs: RFC 3339 (`2026-10-03T09:00:00Z`, or with an offset), or a
/// local `YYYY-MM-DDTHH:MM[:SS]` in `tz`. It must lie after `now` and within one year.
pub fn parse_at<Tz: TimeZone>(
    text: &str,
    now: DateTime<Utc>,
    tz: &Tz,
) -> Result<DateTime<Utc>, String> {
    let text = text.trim();
    let time = match DateTime::parse_from_rfc3339(text) {
        Ok(time) => time.with_timezone(&Utc),
        Err(_) => local_time(text, tz)?,
    };
    if time <= now {
        return Err("at must be in the future".into());
    }
    if time > now + Duration::days(366) {
        return Err("at must be within one year".into());
    }
    Ok(time)
}

fn local_time<Tz: TimeZone>(text: &str, tz: &Tz) -> Result<DateTime<Utc>, String> {
    let naive = [
        "%Y-%m-%dT%H:%M:%S",
        "%Y-%m-%dT%H:%M",
        "%Y-%m-%d %H:%M:%S",
        "%Y-%m-%d %H:%M",
    ]
    .iter()
    .find_map(|format| NaiveDateTime::parse_from_str(text, format).ok())
    .ok_or_else(|| {
        format!("at {text:?} is not a time: use 2026-10-03T09:00 or an RFC 3339 timestamp")
    })?;
    match tz.from_local_datetime(&naive) {
        LocalResult::Single(time) | LocalResult::Ambiguous(time, _) => Ok(time.with_timezone(&Utc)),
        LocalResult::None => Err(format!("at {text:?} does not exist in the local time zone")),
    }
}

/// `ms` since the Unix epoch as an RFC 3339 time in the local time zone, to the second.
pub fn local_time_text(ms: i64) -> String {
    DateTime::<Utc>::from_timestamp_millis(ms)
        .map(|time| {
            time.with_timezone(&chrono::Local)
                .to_rfc3339_opts(chrono::SecondsFormat::Secs, false)
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use chrono::FixedOffset;

    use super::*;

    fn utc(text: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(text)
            .unwrap()
            .with_timezone(&Utc)
    }

    fn next(expr: &str, after: &str) -> String {
        Cron::parse(expr)
            .unwrap()
            .next_after(utc(after), &Utc)
            .unwrap()
            .to_rfc3339()
    }

    #[test]
    fn every_minute_and_fixed_times() {
        assert_eq!(
            next("* * * * *", "2026-10-03T09:00:30Z"),
            "2026-10-03T09:01:00+00:00"
        );
        // Strictly after: an exact match moves on.
        assert_eq!(
            next("0 9 * * *", "2026-10-03T09:00:00Z"),
            "2026-10-04T09:00:00+00:00"
        );
        assert_eq!(
            next("0 2 * * *", "2026-10-03T01:59:59Z"),
            "2026-10-03T02:00:00+00:00"
        );
        assert_eq!(
            next("30 23 31 12 *", "2026-01-01T00:00:00Z"),
            "2026-12-31T23:30:00+00:00"
        );
    }

    #[test]
    fn ranges_steps_lists_and_names() {
        assert_eq!(
            next("*/15 * * * *", "2026-10-03T09:01:00Z"),
            "2026-10-03T09:15:00+00:00"
        );
        assert_eq!(
            next("5/20 * * * *", "2026-10-03T09:30:00Z"),
            "2026-10-03T09:45:00+00:00"
        );
        assert_eq!(
            next("0 9-17/4 * * *", "2026-10-03T13:00:00Z"),
            "2026-10-03T17:00:00+00:00"
        );
        assert_eq!(
            next("0 9,18 * * *", "2026-10-03T10:00:00Z"),
            "2026-10-03T18:00:00+00:00"
        );
        // 2026-10-03 is a Saturday: weekdays resume on Monday.
        assert_eq!(
            next("0 9 * * MON-fri", "2026-10-03T10:00:00Z"),
            "2026-10-05T09:00:00+00:00"
        );
        assert_eq!(
            next("0 0 1 jan,JUL *", "2026-02-01T00:00:00Z"),
            "2026-07-01T00:00:00+00:00"
        );
        // 7 is Sunday too.
        assert_eq!(
            next("0 0 * * 7", "2026-10-03T00:00:00Z"),
            "2026-10-04T00:00:00+00:00"
        );
    }

    #[test]
    fn restricted_day_fields_match_either() {
        // The 15th (a Thursday) or any Monday, whichever is first.
        assert_eq!(
            next("0 0 15 * 1", "2026-10-06T00:00:00Z"),
            "2026-10-12T00:00:00+00:00"
        );
        assert_eq!(
            next("0 0 15 * 1", "2026-10-12T00:00:00Z"),
            "2026-10-15T00:00:00+00:00"
        );
        // With a `*` day of week, the day of month alone decides.
        assert_eq!(
            next("0 0 15 * *", "2026-10-06T00:00:00Z"),
            "2026-10-15T00:00:00+00:00"
        );
    }

    #[test]
    fn macros() {
        assert_eq!(
            next("@daily", "2026-10-03T09:00:00Z"),
            "2026-10-04T00:00:00+00:00"
        );
        assert_eq!(
            next("@hourly", "2026-10-03T09:00:00Z"),
            "2026-10-03T10:00:00+00:00"
        );
        assert_eq!(Cron::parse("@weekly").unwrap().as_str(), "@weekly");
    }

    #[test]
    fn errors_name_the_problem() {
        let error = Cron::parse("61 * * * *").unwrap_err();
        assert!(
            error.starts_with("Invalid cron expression \"61 * * * *\": "),
            "{error}"
        );
        assert!(error.contains("minute"), "{error}");
        assert!(
            Cron::parse("* * * *")
                .unwrap_err()
                .contains("expected 5 fields")
        );
        assert!(Cron::parse("*/0 * * * *").unwrap_err().contains("bad step"));
        assert!(Cron::parse("* 5-2 * * *").unwrap_err().contains("reversed"));
        assert!(Cron::parse("* * * foo *").unwrap_err().contains("month"));
        assert!(
            Cron::parse("0 0 30 2 *")
                .unwrap_err()
                .contains("never matches")
        );
        assert!(Cron::parse("0 0 29 2 *").is_ok(), "leap days occur");
    }

    #[test]
    fn local_time_zones_shift_occurrences() {
        let east = FixedOffset::east_opt(2 * 3600).unwrap();
        let cron = Cron::parse("0 9 * * *").unwrap();
        let next = cron.next_after(utc("2026-10-03T06:00:00Z"), &east).unwrap();
        assert_eq!(next.to_rfc3339(), "2026-10-03T07:00:00+00:00");
    }

    /// A zone that skips 02:00–03:00 on 2026-03-29 and repeats 01:00–02:00 on 2026-10-25,
    /// as central Europe does, to test DST without the system zone database.
    #[derive(Clone, Copy, Debug)]
    struct Dst;

    impl Dst {
        fn offset_at_utc(utc: &NaiveDateTime) -> FixedOffset {
            let summer_start = NaiveDate::from_ymd_opt(2026, 3, 29)
                .unwrap()
                .and_hms_opt(1, 0, 0)
                .unwrap();
            let summer_end = NaiveDate::from_ymd_opt(2026, 10, 25)
                .unwrap()
                .and_hms_opt(0, 0, 0)
                .unwrap();
            let hours = if *utc >= summer_start && *utc < summer_end {
                2
            } else {
                1
            };
            FixedOffset::east_opt(hours * 3600).unwrap()
        }
    }

    impl TimeZone for Dst {
        type Offset = FixedOffset;
        fn from_offset(_: &FixedOffset) -> Self {
            Dst
        }
        fn offset_from_local_date(&self, _: &NaiveDate) -> LocalResult<FixedOffset> {
            unimplemented!()
        }
        fn offset_from_local_datetime(&self, local: &NaiveDateTime) -> LocalResult<FixedOffset> {
            let candidates: Vec<FixedOffset> = [1, 2]
                .into_iter()
                .map(|h| FixedOffset::east_opt(h * 3600).unwrap())
                .filter(|offset| {
                    let utc = *local - Duration::seconds(i64::from(offset.local_minus_utc()));
                    Self::offset_at_utc(&utc) == *offset
                })
                .collect();
            match candidates.as_slice() {
                [] => LocalResult::None,
                [one] => LocalResult::Single(*one),
                // The larger offset is the earlier instant.
                [plus_one, plus_two] => LocalResult::Ambiguous(*plus_two, *plus_one),
                _ => unreachable!(),
            }
        }
        fn offset_from_utc_date(&self, _: &NaiveDate) -> FixedOffset {
            unimplemented!()
        }
        fn offset_from_utc_datetime(&self, utc: &NaiveDateTime) -> FixedOffset {
            Self::offset_at_utc(utc)
        }
    }

    #[test]
    fn dst_gaps_are_skipped_and_folds_run_once() {
        let cron = Cron::parse("30 2 * * *").unwrap();
        // 02:30 does not exist on 2026-03-29; the next one is on the 30th (summer time).
        let next = cron.next_after(utc("2026-03-28T23:00:00Z"), &Dst).unwrap();
        assert_eq!(next.to_rfc3339(), "2026-03-30T00:30:00+00:00");
        let fold = Cron::parse("30 1 * * *").unwrap();
        // 01:30 happens twice on 2026-10-25; it runs at the earlier instant only.
        let first = fold.next_after(utc("2026-10-24T22:00:00Z"), &Dst).unwrap();
        assert_eq!(first.to_rfc3339(), "2026-10-24T23:30:00+00:00");
        let after = fold.next_after(first, &Dst).unwrap();
        assert_eq!(after.to_rfc3339(), "2026-10-26T00:30:00+00:00");
    }

    #[test]
    fn one_shot_times() {
        let now = utc("2026-10-02T12:00:00Z");
        let east = FixedOffset::east_opt(3600).unwrap();
        assert_eq!(
            parse_at("2026-10-03T09:00", now, &east)
                .unwrap()
                .to_rfc3339(),
            "2026-10-03T08:00:00+00:00"
        );
        assert_eq!(
            parse_at("2026-10-03T09:00:00Z", now, &east)
                .unwrap()
                .to_rfc3339(),
            "2026-10-03T09:00:00+00:00"
        );
        assert_eq!(
            parse_at("2026-10-01T09:00", now, &Utc).unwrap_err(),
            "at must be in the future"
        );
        assert_eq!(
            parse_at("2028-10-01T09:00", now, &Utc).unwrap_err(),
            "at must be within one year"
        );
        assert!(
            parse_at("tomorrow", now, &Utc)
                .unwrap_err()
                .contains("is not a time")
        );
    }
}
