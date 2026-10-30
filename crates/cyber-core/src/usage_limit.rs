//! Usage limit waits (`session-runtime` → Usage limit wait, `configuration` → Usage limit
//! settings): the `usage_limit` settings, and the pure decision whether a Turn that hit a
//! subscription usage limit waits for the reset or fails.

use serde::{Deserialize, Serialize};
use serde_json::Value;

const KEY: &str = "usage_limit";
/// `usage_limit.max_wait` when unset: 6 hours.
pub const DEFAULT_MAX_WAIT_SECONDS: u64 = 6 * 3600;
/// The longest `usage_limit.max_wait`: 7 days.
pub const MAX_WAIT_LIMIT_SECONDS: u64 = 7 * 86_400;
/// Time added after the provider's reset, so the retry does not race the reset.
pub const MARGIN_MS: i64 = 10_000;
/// The spread of resume times of runs that hit the same reset.
pub const JITTER_MS: i64 = 30_000;

/// Which Sessions wait for a usage limit to reset.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WaitMode {
    /// Only Sessions with no interactive owner.
    #[default]
    Unattended,
    Always,
    Never,
}

/// The `usage_limit` key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageLimitSettings {
    pub mode: WaitMode,
    pub max_wait_seconds: u64,
}

impl Default for UsageLimitSettings {
    fn default() -> Self {
        Self {
            mode: WaitMode::default(),
            max_wait_seconds: DEFAULT_MAX_WAIT_SECONDS,
        }
    }
}

impl UsageLimitSettings {
    /// The `usage_limit` key of `config`; an invalid field is an error naming it.
    pub fn from_config(config: &Value) -> Result<Self, String> {
        let mut settings = Self::default();
        let Some(value) = config.get(KEY).filter(|v| !v.is_null()) else {
            return Ok(settings);
        };
        let map = value
            .as_object()
            .ok_or_else(|| format!("{KEY}: expected an object"))?;
        for (field, value) in map {
            match field.as_str() {
                "mode" => {
                    settings.mode = serde_json::from_value(value.clone()).map_err(|_| {
                        format!("{KEY}.mode: expected one of unattended, always, never")
                    })?;
                }
                "max_wait" => settings.max_wait_seconds = max_wait(value)?,
                other => return Err(format!("{KEY}.{other}: unknown setting")),
            }
        }
        Ok(settings)
    }
}

fn max_wait(value: &Value) -> Result<u64, String> {
    value
        .as_str()
        .and_then(crate::loops::duration)
        .filter(|seconds| (1..=MAX_WAIT_LIMIT_SECONDS).contains(seconds))
        .ok_or_else(|| {
            format!("{KEY}.max_wait: expected a duration from 1s to 7d, such as 30m or 6h")
        })
}

/// What a wait decision reads; times are Unix milliseconds.
#[derive(Debug, Clone, Default)]
pub struct WaitFacts {
    /// The Session has no interactive owner.
    pub unattended: bool,
    /// When the limit resets; `None` when the provider did not say.
    pub resets_at_ms: Option<i64>,
    pub now_ms: i64,
    /// When the first wait of this Turn started; `max_wait` counts from it.
    pub since_ms: Option<i64>,
    /// The earliest wall deadline of the Session's budgets, Goal or loop.
    pub deadline_ms: Option<i64>,
    /// This Session's jitter, from [`jitter_ms`].
    pub jitter_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WaitPlan {
    Wait { resume_at_ms: i64 },
    Fail(NoWait),
}

/// Why a Turn that hit a usage limit does not wait.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NoWait {
    /// `usage_limit.mode: never`.
    Disabled,
    /// An interactive Session under `mode: unattended`: the user decides.
    Interactive,
    UnknownReset,
    BeyondMaxWait {
        max_wait_seconds: u64,
    },
    /// Resuming at `resume_at_ms` would pass the wall deadline.
    PastDeadline {
        resume_at_ms: i64,
        deadline_ms: i64,
    },
}

impl NoWait {
    /// What the failure message adds; none for an interactive Session, whose client
    /// offers to resume at the reset instead.
    pub fn note(&self) -> Option<String> {
        Some(match self {
            Self::Interactive => return None,
            Self::Disabled => "not waiting: usage_limit.mode is never".into(),
            Self::UnknownReset => {
                "not waiting: the provider did not say when the limit resets".into()
            }
            Self::BeyondMaxWait { max_wait_seconds } => format!(
                "not waiting: the reset is more than usage_limit.max_wait ({}) away",
                duration_text(*max_wait_seconds)
            ),
            Self::PastDeadline { .. } => {
                "not waiting: the reset falls after the run's wall-clock deadline".into()
            }
        })
    }
}

/// Wait or fail (`session-runtime` → Usage limit wait), in order: the mode, the
/// Session's context, a known reset, `max_wait` from the first wait, then the deadline.
pub fn plan(settings: &UsageLimitSettings, facts: &WaitFacts) -> WaitPlan {
    match settings.mode {
        WaitMode::Never => return WaitPlan::Fail(NoWait::Disabled),
        WaitMode::Unattended if !facts.unattended => {
            return WaitPlan::Fail(NoWait::Interactive);
        }
        WaitMode::Unattended | WaitMode::Always => {}
    }
    let Some(resets_at) = facts.resets_at_ms else {
        return WaitPlan::Fail(NoWait::UnknownReset);
    };
    let since = facts.since_ms.unwrap_or(facts.now_ms);
    let max_wait =
        i64::try_from(settings.max_wait_seconds.saturating_mul(1000)).unwrap_or(i64::MAX);
    if resets_at.saturating_sub(since) > max_wait {
        return WaitPlan::Fail(NoWait::BeyondMaxWait {
            max_wait_seconds: settings.max_wait_seconds,
        });
    }
    let resume_at_ms = resets_at
        .max(facts.now_ms)
        .saturating_add(MARGIN_MS + facts.jitter_ms.clamp(0, JITTER_MS - 1));
    match facts.deadline_ms {
        Some(deadline_ms) if resume_at_ms >= deadline_ms => WaitPlan::Fail(NoWait::PastDeadline {
            resume_at_ms,
            deadline_ms,
        }),
        _ => WaitPlan::Wait { resume_at_ms },
    }
}

/// A jitter in `0..JITTER_MS`, stable for one Session and reset (FNV-1a), so many runs
/// hitting one reset spread out and a restart keeps the same resume time.
pub fn jitter_ms(session_id: &str, resets_at_ms: i64) -> i64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in session_id.bytes().chain(resets_at_ms.to_le_bytes()) {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    (hash % JITTER_MS as u64) as i64
}

/// `6h`, `1h 30m`, `45m`, `30s`.
pub fn duration_text(seconds: u64) -> String {
    let (hours, minutes, rest) = (seconds / 3600, seconds % 3600 / 60, seconds % 60);
    let parts: Vec<String> = [(hours, "h"), (minutes, "m"), (rest, "s")]
        .into_iter()
        .filter(|(n, _)| *n > 0)
        .map(|(n, unit)| format!("{n}{unit}"))
        .collect();
    if parts.is_empty() {
        "0s".into()
    } else {
        parts.join(" ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const NOW: i64 = 1_000_000_000;
    const MIN: i64 = 60_000;

    fn settings(mode: WaitMode, max_wait_seconds: u64) -> UsageLimitSettings {
        UsageLimitSettings {
            mode,
            max_wait_seconds,
        }
    }

    fn facts(unattended: bool, resets_in: Option<i64>) -> WaitFacts {
        WaitFacts {
            unattended,
            resets_at_ms: resets_in.map(|ms| NOW + ms),
            now_ms: NOW,
            since_ms: None,
            deadline_ms: None,
            jitter_ms: 5_000,
        }
    }

    #[test]
    fn settings_default_and_parse() {
        assert_eq!(
            UsageLimitSettings::from_config(&json!({})).unwrap(),
            settings(WaitMode::Unattended, 6 * 3600)
        );
        let parsed = UsageLimitSettings::from_config(
            &json!({"usage_limit": {"mode": "always", "max_wait": "30m"}}),
        )
        .unwrap();
        assert_eq!(parsed, settings(WaitMode::Always, 1800));
        let never =
            UsageLimitSettings::from_config(&json!({"usage_limit": {"mode": "never"}})).unwrap();
        assert_eq!(never.mode, WaitMode::Never);
    }

    #[test]
    fn invalid_settings_name_the_field() {
        let error = |value: Value| {
            UsageLimitSettings::from_config(&json!({ "usage_limit": value })).unwrap_err()
        };
        assert_eq!(error(json!(true)), "usage_limit: expected an object");
        assert_eq!(
            error(json!({"mode": "sometimes"})),
            "usage_limit.mode: expected one of unattended, always, never"
        );
        let duration = "usage_limit.max_wait: expected a duration from 1s to 7d, such as 30m or 6h";
        for bad in [
            json!("2 weeks"),
            json!(3600),
            json!("0s"),
            json!("8d"),
            json!(""),
        ] {
            assert_eq!(error(json!({ "max_wait": bad })), duration);
        }
        assert_eq!(
            error(json!({"retries": 2})),
            "usage_limit.retries: unknown setting"
        );
        assert!(
            UsageLimitSettings::from_config(&json!({"usage_limit": {"max_wait": "7d"}})).is_ok()
        );
    }

    /// Mode × context: only `always` and an unattended Session under `unattended` wait.
    #[test]
    fn mode_and_context() {
        let cases = [
            (WaitMode::Unattended, true, true),
            (WaitMode::Unattended, false, false),
            (WaitMode::Always, true, true),
            (WaitMode::Always, false, true),
            (WaitMode::Never, true, false),
            (WaitMode::Never, false, false),
        ];
        for (mode, unattended, waits) in cases {
            let planned = plan(&settings(mode, 3600), &facts(unattended, Some(20 * MIN)));
            assert_eq!(
                matches!(planned, WaitPlan::Wait { .. }),
                waits,
                "{mode:?} {unattended}"
            );
        }
        let interactive = plan(
            &settings(WaitMode::Unattended, 3600),
            &facts(false, Some(MIN)),
        );
        assert_eq!(interactive, WaitPlan::Fail(NoWait::Interactive));
        assert_eq!(NoWait::Interactive.note(), None);
        let never = plan(&settings(WaitMode::Never, 3600), &facts(true, Some(MIN)));
        assert_eq!(never, WaitPlan::Fail(NoWait::Disabled));
    }

    #[test]
    fn resume_adds_margin_and_jitter_after_the_reset() {
        let planned = plan(
            &settings(WaitMode::Always, 3600),
            &facts(true, Some(20 * MIN)),
        );
        assert_eq!(
            planned,
            WaitPlan::Wait {
                resume_at_ms: NOW + 20 * MIN + MARGIN_MS + 5_000
            }
        );
        // A reset already past resumes from now; the jitter stays in range.
        let mut past = facts(true, Some(-MIN));
        past.jitter_ms = 99_999;
        assert_eq!(
            plan(&settings(WaitMode::Always, 3600), &past),
            WaitPlan::Wait {
                resume_at_ms: NOW + MARGIN_MS + JITTER_MS - 1
            }
        );
    }

    #[test]
    fn unknown_reset_never_waits() {
        let planned = plan(&settings(WaitMode::Always, 3600), &facts(true, None));
        assert_eq!(planned, WaitPlan::Fail(NoWait::UnknownReset));
        assert_eq!(
            NoWait::UnknownReset.note().unwrap(),
            "not waiting: the provider did not say when the limit resets"
        );
    }

    /// `max_wait` bounds the reset, counted from the first wait of the Turn.
    #[test]
    fn max_wait_counts_from_the_first_wait() {
        let s = settings(WaitMode::Unattended, 6 * 3600);
        assert!(matches!(
            plan(&s, &facts(true, Some(6 * 60 * MIN))),
            WaitPlan::Wait { .. }
        ));
        let far = plan(&s, &facts(true, Some(7 * 60 * MIN)));
        assert_eq!(
            far,
            WaitPlan::Fail(NoWait::BeyondMaxWait {
                max_wait_seconds: 6 * 3600
            })
        );
        assert_eq!(
            NoWait::BeyondMaxWait {
                max_wait_seconds: 6 * 3600
            }
            .note()
            .unwrap(),
            "not waiting: the reset is more than usage_limit.max_wait (6h) away"
        );
        // A second limit 2 h after a first wait that began 5 h ago passes the 6 h bound.
        let mut again = facts(true, Some(2 * 60 * MIN));
        again.since_ms = Some(NOW - 5 * 60 * MIN);
        assert!(matches!(
            plan(&s, &again),
            WaitPlan::Fail(NoWait::BeyondMaxWait { .. })
        ));
        again.since_ms = Some(NOW - 3 * 60 * MIN);
        assert!(matches!(plan(&s, &again), WaitPlan::Wait { .. }));
    }

    /// A deadline before the planned resume wins, even in `always`.
    #[test]
    fn deadline_before_resume_fails() {
        let s = settings(WaitMode::Always, 6 * 3600);
        let mut f = facts(true, Some(20 * MIN));
        f.deadline_ms = Some(NOW + 10 * MIN);
        assert_eq!(
            plan(&s, &f),
            WaitPlan::Fail(NoWait::PastDeadline {
                resume_at_ms: NOW + 20 * MIN + MARGIN_MS + 5_000,
                deadline_ms: NOW + 10 * MIN
            })
        );
        // The margin counts: a deadline right after the reset still wins.
        f.deadline_ms = Some(NOW + 20 * MIN + 1);
        assert!(matches!(
            plan(&s, &f),
            WaitPlan::Fail(NoWait::PastDeadline { .. })
        ));
        f.deadline_ms = Some(NOW + 30 * MIN);
        assert!(matches!(plan(&s, &f), WaitPlan::Wait { .. }));
        // Interactive and unknown reset are decided before the deadline.
        f.unattended = false;
        assert_eq!(
            plan(&settings(WaitMode::Unattended, 3600), &f),
            WaitPlan::Fail(NoWait::Interactive)
        );
    }

    #[test]
    fn jitter_is_stable_spread_and_in_range() {
        assert_eq!(jitter_ms("ses_a", 7), jitter_ms("ses_a", 7));
        let spread: std::collections::BTreeSet<i64> =
            (0..50).map(|n| jitter_ms(&format!("ses_{n}"), 7)).collect();
        assert!(spread.len() > 40, "{spread:?}");
        assert!(spread.iter().all(|j| (0..JITTER_MS).contains(j)));
    }

    #[test]
    fn durations_read_naturally() {
        assert_eq!(duration_text(6 * 3600), "6h");
        assert_eq!(duration_text(5400), "1h 30m");
        assert_eq!(duration_text(45), "45s");
        assert_eq!(duration_text(0), "0s");
    }
}
