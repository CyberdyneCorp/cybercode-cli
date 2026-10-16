//! Provider-reported usage windows (`observability-costs` → Provider-reported limits).
//!
//! An adapter reads the plan and rate-limit headers of every provider response and
//! records the latest reading per provider and window. Readings come only from what the
//! provider sent: nothing here derives, estimates or extrapolates a limit from local
//! usage, and a provider that sends no such headers never appears.
//!
//! Recognized header families:
//! - plan windows `anthropic-ratelimit-unified-<window>-utilization` (a 0–1 fraction) with
//!   `-reset` (Unix seconds), for windows such as `5h`, `7d` and `7d_opus`;
//! - plan windows `x-codex-<primary|secondary>-used-percent` (0–100) with
//!   `-window-minutes` and `-reset-after-seconds` or `-reset-at` (Unix seconds);
//! - rate limits `anthropic-ratelimit-<class>-{limit,remaining,reset}` (RFC 3339 reset)
//!   and `x-ratelimit-{limit,remaining,reset}-<class>` (duration reset such as `6m0s`).

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use reqwest::header::HeaderMap;
use serde::Serialize;

/// The most windows the store holds, so a provider naming ever-new windows cannot grow it.
const MAX_WINDOWS: usize = 64;
/// The longest window name accepted from a header name.
const MAX_KEY: usize = 24;

/// Whether a window is shown in the daily or the weekly view: windows of up to a day
/// (rate limits, a 5-hour plan window) are `daily`, longer ones `weekly`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum WindowPeriod {
    Daily,
    Weekly,
}

/// One window a provider reported in one response.
#[derive(Debug, Clone, PartialEq, Serialize, schemars::JsonSchema)]
pub struct UsageWindow {
    /// Stable window identity within the provider (`5h`, `primary`, `requests`).
    pub window: String,
    /// Display label (`5-hour limit`, `Weekly limit (opus)`, `Requests`).
    pub label: String,
    pub period: WindowPeriod,
    /// Percent of the window used, 0–100.
    pub used_percent: f64,
    /// When the window resets, Unix milliseconds; null when the provider did not say.
    pub resets_ms: Option<i64>,
}

/// The latest reading of one provider window.
#[derive(Debug, Clone, PartialEq, Serialize, schemars::JsonSchema)]
pub struct LimitReading {
    pub provider: String,
    #[serde(flatten)]
    pub window: UsageWindow,
    /// When the response carrying this reading arrived, Unix milliseconds.
    pub observed_ms: i64,
}

/// The latest reading per provider and window, shared by every adapter of a catalog.
#[derive(Clone, Default)]
pub struct LimitStore(Arc<Mutex<BTreeMap<(String, String), LimitReading>>>);

impl LimitStore {
    /// A sink that records the windows of `provider`'s responses here.
    pub fn sink(&self, provider: &str) -> LimitSink {
        LimitSink {
            provider: provider.to_owned(),
            store: self.clone(),
        }
    }

    pub fn record(&self, provider: &str, windows: Vec<UsageWindow>, observed_ms: i64) {
        let Ok(mut map) = self.0.lock() else {
            return;
        };
        for window in windows {
            let key = (provider.to_owned(), window.window.clone());
            if map.len() >= MAX_WINDOWS && !map.contains_key(&key) {
                continue;
            }
            map.insert(
                key,
                LimitReading {
                    provider: provider.to_owned(),
                    window,
                    observed_ms,
                },
            );
        }
    }

    /// Every reading, by provider then window.
    pub fn readings(&self) -> Vec<LimitReading> {
        self.0
            .lock()
            .map(|map| map.values().cloned().collect())
            .unwrap_or_default()
    }
}

/// Where one provider's adapter reports the windows of its responses.
#[derive(Clone)]
pub struct LimitSink {
    provider: String,
    store: LimitStore,
}

impl LimitSink {
    /// Record the windows in a response's headers; headers without any change nothing.
    pub fn observe(&self, headers: &HeaderMap) {
        let now = now_ms();
        let windows = parse(headers, now);
        if !windows.is_empty() {
            self.store.record(&self.provider, windows, now);
        }
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

/// The usage windows in one response's headers; `now_ms` resolves relative resets.
pub fn parse(headers: &HeaderMap, now_ms: i64) -> Vec<UsageWindow> {
    let mut out = unified(headers);
    out.extend(codex(headers, now_ms));
    out.extend(anthropic_rates(headers));
    out.extend(openai_rates(headers, now_ms));
    out
}

fn header<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name)?.to_str().ok().map(str::trim)
}

fn number(headers: &HeaderMap, name: &str) -> Option<f64> {
    header(headers, name)?
        .parse::<f64>()
        .ok()
        .filter(|v| v.is_finite())
}

/// A percent in 0–100, or none for anything a provider could not mean.
fn percent(value: f64) -> Option<f64> {
    (value.is_finite() && (0.0..=1000.0).contains(&value)).then(|| value.min(100.0))
}

/// Unix seconds before the year 2096, as milliseconds.
fn unix_seconds(value: f64) -> Option<i64> {
    let ms = (value * 1000.0).round();
    (ms.is_finite() && ms > 0.0 && ms < 4.0e12).then_some(ms as i64)
}

/// A window name taken from a header name: lowercase letters, digits and `_`.
fn safe_key(key: &str) -> bool {
    !key.is_empty()
        && key.len() <= MAX_KEY
        && key
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

/// `5h` → 300 minutes, `7d` → 10,080 minutes.
fn span_minutes(span: &str) -> Option<u64> {
    let unit = span.chars().last()?;
    let n: u64 = span[..span.len() - 1].parse().ok().filter(|n| *n > 0)?;
    match unit {
        'h' => n.checked_mul(60),
        'd' => n.checked_mul(1440),
        _ => None,
    }
}

/// `5-hour limit`, `Weekly limit`, `3-day limit`, `90-minute limit`.
fn span_label(minutes: u64) -> String {
    match minutes {
        10_080 => "Weekly limit".into(),
        m if m >= 1440 && m % 1440 == 0 => format!("{}-day limit", m / 1440),
        m if m >= 60 && m % 60 == 0 => format!("{}-hour limit", m / 60),
        m => format!("{m}-minute limit"),
    }
}

fn period_of(minutes: u64) -> WindowPeriod {
    if minutes <= 1440 {
        WindowPeriod::Daily
    } else {
        WindowPeriod::Weekly
    }
}

const UNIFIED: &str = "anthropic-ratelimit-unified-";

fn unified(headers: &HeaderMap) -> Vec<UsageWindow> {
    let mut out = Vec::new();
    for name in headers.keys() {
        let Some(key) = name
            .as_str()
            .strip_prefix(UNIFIED)
            .and_then(|rest| rest.strip_suffix("-utilization"))
        else {
            continue;
        };
        if !safe_key(key) {
            continue;
        }
        let (span, model) = key.split_once('_').unwrap_or((key, ""));
        let Some(minutes) = span_minutes(span) else {
            continue;
        };
        let Some(used) = number(headers, name.as_str()).and_then(|f| percent(f * 100.0)) else {
            continue;
        };
        let mut label = span_label(minutes);
        if !model.is_empty() {
            label = format!("{label} ({model})");
        }
        let resets_ms = number(headers, &format!("{UNIFIED}{key}-reset")).and_then(unix_seconds);
        out.push(UsageWindow {
            window: key.to_owned(),
            label,
            period: period_of(minutes),
            used_percent: used,
            resets_ms,
        });
    }
    out
}

fn codex(headers: &HeaderMap, now_ms: i64) -> Vec<UsageWindow> {
    let mut out = Vec::new();
    for (slot, fallback) in [
        ("primary", WindowPeriod::Daily),
        ("secondary", WindowPeriod::Weekly),
    ] {
        let prefix = format!("x-codex-{slot}-");
        let Some(used) = number(headers, &format!("{prefix}used-percent")).and_then(percent) else {
            continue;
        };
        let minutes = number(headers, &format!("{prefix}window-minutes"))
            .filter(|m| *m >= 1.0 && *m <= 525_600.0)
            .map(|m| m as u64);
        let after = number(headers, &format!("{prefix}reset-after-seconds"))
            .filter(|s| *s >= 0.0 && *s <= 3.2e8)
            .map(|s| now_ms + (s * 1000.0) as i64);
        let at = || number(headers, &format!("{prefix}reset-at")).and_then(unix_seconds);
        out.push(UsageWindow {
            window: slot.to_owned(),
            label: minutes.map_or_else(
                || format!("{}{} limit", slot[..1].to_uppercase(), &slot[1..]),
                span_label,
            ),
            period: minutes.map_or(fallback, period_of),
            used_percent: used,
            resets_ms: after.or_else(at),
        });
    }
    out
}

/// The rate-limit classes each family names, with their labels.
const RATE_CLASSES: [(&str, &str); 4] = [
    ("requests", "Requests"),
    ("tokens", "Tokens"),
    ("input-tokens", "Input tokens"),
    ("output-tokens", "Output tokens"),
];

/// Percent used of a `limit` with `remaining` left.
fn rate_used(limit: Option<f64>, remaining: Option<f64>) -> Option<f64> {
    let (limit, remaining) = (limit?, remaining?);
    (limit > 0.0 && remaining >= 0.0)
        .then(|| percent(100.0 * (limit - remaining.min(limit)) / limit))
        .flatten()
}

fn rate_window(class: &str, label: &str, used: f64, resets_ms: Option<i64>) -> UsageWindow {
    UsageWindow {
        window: format!("rate_{}", class.replace('-', "_")),
        label: label.to_owned(),
        period: WindowPeriod::Daily,
        used_percent: used,
        resets_ms,
    }
}

fn anthropic_rates(headers: &HeaderMap) -> Vec<UsageWindow> {
    RATE_CLASSES
        .iter()
        .filter_map(|(class, label)| {
            let name = |part: &str| format!("anthropic-ratelimit-{class}-{part}");
            let used = rate_used(
                number(headers, &name("limit")),
                number(headers, &name("remaining")),
            )?;
            let resets_ms = header(headers, &name("reset"))
                .and_then(|v| v.parse::<jiff::Timestamp>().ok())
                .map(|t| t.as_millisecond());
            Some(rate_window(class, label, used, resets_ms))
        })
        .collect()
}

fn openai_rates(headers: &HeaderMap, now_ms: i64) -> Vec<UsageWindow> {
    RATE_CLASSES
        .iter()
        .filter_map(|(class, label)| {
            let name = |part: &str| format!("x-ratelimit-{part}-{class}");
            let used = rate_used(
                number(headers, &name("limit")),
                number(headers, &name("remaining")),
            )?;
            let resets_ms = header(headers, &name("reset"))
                .and_then(duration_ms)
                .map(|ms| now_ms + ms);
            Some(rate_window(class, label, used, resets_ms))
        })
        .collect()
}

/// `6m0s`, `1.5s`, `20ms`, `1h2m` as milliseconds; anything else is none.
fn duration_ms(text: &str) -> Option<i64> {
    let mut total = 0.0;
    let mut rest = text;
    while !rest.is_empty() {
        let digits = rest
            .find(|c: char| !(c.is_ascii_digit() || c == '.'))
            .unwrap_or(rest.len());
        let value: f64 = rest[..digits].parse().ok()?;
        rest = &rest[digits..];
        let (unit, len) = [
            ("ms", 1.0, 2),
            ("h", 3_600_000.0, 1),
            ("m", 60_000.0, 1),
            ("s", 1000.0, 1),
        ]
        .iter()
        .find(|(u, _, _)| rest.starts_with(u))
        .map(|(_, scale, len)| (*scale, *len))?;
        total += value * unit;
        rest = &rest[len..];
    }
    (total.is_finite() && total > 0.0 && total < 3.2e11).then_some(total as i64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::header::{HeaderName, HeaderValue};

    const NOW: i64 = 1_792_123_200_000;

    fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for (name, value) in pairs {
            map.insert(
                HeaderName::from_bytes(name.as_bytes()).unwrap(),
                HeaderValue::from_str(value).unwrap(),
            );
        }
        map
    }

    #[test]
    fn plan_windows_from_unified_headers() {
        let map = headers(&[
            ("anthropic-ratelimit-unified-5h-utilization", "0.42"),
            ("anthropic-ratelimit-unified-5h-reset", "1792130400"),
            ("anthropic-ratelimit-unified-7d-utilization", "0.9"),
            ("anthropic-ratelimit-unified-7d_opus-utilization", "1.2"),
            ("anthropic-ratelimit-unified-status", "allowed"),
        ]);
        let mut windows = parse(&map, NOW);
        windows.sort_by(|a, b| a.window.cmp(&b.window));
        let five = &windows[0];
        assert_eq!(five.window, "5h");
        assert_eq!(five.label, "5-hour limit");
        assert_eq!(five.period, WindowPeriod::Daily);
        assert!((five.used_percent - 42.0).abs() < 1e-9);
        assert_eq!(five.resets_ms, Some(1_792_130_400_000));
        assert_eq!(windows[1].label, "Weekly limit");
        assert_eq!(windows[1].period, WindowPeriod::Weekly);
        assert_eq!(windows[1].resets_ms, None);
        assert_eq!(windows[2].label, "Weekly limit (opus)");
        assert_eq!(windows[2].used_percent, 100.0, "clamped");
        assert_eq!(windows.len(), 3);
    }

    #[test]
    fn codex_windows_with_relative_resets() {
        let map = headers(&[
            ("x-codex-primary-used-percent", "12.5"),
            ("x-codex-primary-window-minutes", "300"),
            ("x-codex-primary-reset-after-seconds", "60"),
            ("x-codex-secondary-used-percent", "70"),
            ("x-codex-secondary-window-minutes", "10080"),
            ("x-codex-secondary-reset-at", "1792209600"),
        ]);
        let windows = parse(&map, NOW);
        assert_eq!(windows[0].label, "5-hour limit");
        assert_eq!(windows[0].resets_ms, Some(NOW + 60_000));
        assert_eq!(windows[1].label, "Weekly limit");
        assert_eq!(windows[1].period, WindowPeriod::Weekly);
        assert_eq!(windows[1].resets_ms, Some(1_792_209_600_000));
    }

    #[test]
    fn rate_limits_of_both_families() {
        let map = headers(&[
            ("anthropic-ratelimit-requests-limit", "50"),
            ("anthropic-ratelimit-requests-remaining", "40"),
            ("anthropic-ratelimit-requests-reset", "2026-10-16T04:01:00Z"),
            ("x-ratelimit-limit-tokens", "1000"),
            ("x-ratelimit-remaining-tokens", "250"),
            ("x-ratelimit-reset-tokens", "6m0s"),
        ]);
        let windows = parse(&map, NOW);
        assert_eq!(windows.len(), 2);
        assert_eq!(windows[0].window, "rate_requests");
        assert!((windows[0].used_percent - 20.0).abs() < 1e-9);
        assert_eq!(windows[0].resets_ms, Some(NOW + 60_000));
        assert_eq!(windows[1].label, "Tokens");
        assert!((windows[1].used_percent - 75.0).abs() < 1e-9);
        assert_eq!(windows[1].resets_ms, Some(NOW + 360_000));
        assert_eq!(duration_ms("20ms"), Some(20));
        assert_eq!(duration_ms("1h2m1.5s"), Some(3_721_500));
        assert_eq!(duration_ms("soon"), None);
    }

    #[test]
    fn nothing_is_invented_from_absent_or_hostile_headers() {
        assert!(parse(&HeaderMap::new(), NOW).is_empty());
        let map = headers(&[
            ("content-type", "text/event-stream"),
            ("anthropic-ratelimit-unified-5h-utilization", "NaN"),
            ("anthropic-ratelimit-unified-5h.x-utilization", "0.5"),
            ("anthropic-ratelimit-unified-forever-utilization", "0.5"),
            ("anthropic-ratelimit-requests-limit", "0"),
            ("anthropic-ratelimit-requests-remaining", "0"),
            ("x-ratelimit-limit-requests", "100"),
            ("x-codex-primary-used-percent", "-5"),
            ("x-codex-secondary-used-percent", "1e9"),
        ]);
        assert!(parse(&map, NOW).is_empty(), "{:?}", parse(&map, NOW));
    }

    #[test]
    fn the_store_keeps_the_latest_reading_per_provider_and_window() {
        let store = LimitStore::default();
        let window = |used: f64| UsageWindow {
            window: "5h".into(),
            label: "5-hour limit".into(),
            period: WindowPeriod::Daily,
            used_percent: used,
            resets_ms: None,
        };
        store.record("anthropic", vec![window(10.0)], 1);
        store.record("anthropic", vec![window(30.0)], 2);
        store.record("other", vec![window(5.0)], 3);
        let readings = store.readings();
        assert_eq!(readings.len(), 2);
        assert_eq!(readings[0].window.used_percent, 30.0);
        assert_eq!(readings[0].observed_ms, 2);
        assert_eq!(readings[1].provider, "other");
        let sink = store.sink("gateway");
        sink.observe(&headers(&[("content-type", "text/event-stream")]));
        assert_eq!(store.readings().len(), 2, "no headers, no reading");
    }
}
