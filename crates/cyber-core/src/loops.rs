//! The `/loop` grammar and loop settings (`loops-scheduling`), shared by the TUI and the
//! server so both accept and reject exactly the same commands.
//!
//! ```text
//! /loop [<interval>] [--max N] [--for <dur>] [--until "<cmd>"] [--overlap skip|coalesce]
//!       [--missed skip|run-once] [--max-cost USD] [--max-tokens N] [--max-turns N]
//!       [--timeout <dur>] <prompt or /command>
//! /loop list
//! /loop stop [<loop_id>|all]
//! ```
//!
//! Flags are read only before the prompt: everything from the first token that is not a
//! flag is the prompt, verbatim, so a prompt may itself contain `--`.
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::budget::Budget;

/// The shortest interval a loop may use.
pub const MIN_INTERVAL_SECONDS: u64 = 60;
/// `schedule_wakeup` delays are clamped to this range.
pub const WAKEUP_RANGE: (u64, u64) = (60, 3600);
/// Prompts and `--until` commands longer than this are refused.
pub const MAX_TEXT: usize = 4000;

/// What to do with a tick that arrives while the previous iteration still runs.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum Overlap {
    #[default]
    Skip,
    Coalesce,
}

/// What to do with ticks missed while the Session was not running.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "kebab-case")]
pub enum Missed {
    #[default]
    Skip,
    RunOnce,
}

/// A loop to create. `interval_seconds: None` makes it self-paced.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LoopSpec {
    pub prompt: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interval_seconds: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_iterations: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub for_seconds: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub until: Option<String>,
    #[serde(default)]
    pub overlap: Overlap,
    #[serde(default)]
    pub missed: Missed,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget: Option<Budget>,
}

impl LoopSpec {
    pub fn validate(&self) -> Result<(), String> {
        let prompt = self.prompt.trim();
        if prompt.is_empty() {
            return Err("Loop prompt is empty".into());
        }
        if prompt.chars().count() > MAX_TEXT {
            return Err(format!("Loop prompt is longer than {MAX_TEXT} characters"));
        }
        if self
            .interval_seconds
            .is_some_and(|s| s < MIN_INTERVAL_SECONDS)
        {
            return Err(format!(
                "Loop interval must be at least {MIN_INTERVAL_SECONDS}s"
            ));
        }
        if self.max_iterations == Some(0) {
            return Err("--max must be at least 1".into());
        }
        if self.for_seconds == Some(0) {
            return Err("--for must be longer than 0s".into());
        }
        if let Some(until) = &self.until
            && (until.trim().is_empty() || until.chars().count() > MAX_TEXT)
        {
            return Err(format!(
                "--until needs a command of 1 to {MAX_TEXT} characters"
            ));
        }
        if let Some(budget) = &self.budget {
            budget.validate()?;
        }
        Ok(())
    }
}

/// A parsed `/loop` command.
#[derive(Debug, Clone, PartialEq)]
pub enum LoopCommand {
    Create(LoopSpec),
    List,
    /// `None` stops every loop of the Session.
    Stop(Option<String>),
}

/// Parse the arguments of `/loop` (everything after the command name).
pub fn parse(args: &str) -> Result<LoopCommand, String> {
    let args = args.trim();
    let (first, rest) = split_word(args);
    match first {
        "" => Err("Usage: /loop [interval] <prompt> | /loop list | /loop stop [id|all]".into()),
        "list" if rest.trim().is_empty() => Ok(LoopCommand::List),
        "stop" => match rest.trim() {
            "" | "all" => Ok(LoopCommand::Stop(None)),
            id if is_loop_id(id) => Ok(LoopCommand::Stop(Some(id.to_owned()))),
            other => Err(format!("Unknown loop {other}")),
        },
        _ => create(args).map(LoopCommand::Create),
    }
}

fn create(args: &str) -> Result<LoopSpec, String> {
    let mut spec = LoopSpec::default();
    let mut rest = args;
    let (first, after) = split_word(rest);
    if let Some(seconds) = duration(first) {
        spec.interval_seconds = Some(seconds);
        rest = after;
    } else if looks_like_duration(first) {
        return Err(format!(
            "Invalid loop interval {first}: use Ns, Nm, Nh or Nd"
        ));
    }
    loop {
        let (word, after) = split_word(rest);
        let Some(flag) = word.strip_prefix("--").filter(|f| !f.is_empty()) else {
            break;
        };
        let (value, after) = take_value(after).ok_or_else(|| format!("--{flag} needs a value"))?;
        apply_flag(&mut spec, flag, &value)?;
        rest = after;
    }
    spec.prompt = rest.trim().to_owned();
    spec.validate()?;
    Ok(spec)
}

fn apply_flag(spec: &mut LoopSpec, flag: &str, value: &str) -> Result<(), String> {
    let budget = || spec_budget(spec);
    match flag {
        "max" => spec.max_iterations = Some(count(flag, value)?),
        "for" => spec.for_seconds = Some(seconds(flag, value)?),
        "until" => spec.until = Some(value.to_owned()),
        "overlap" => {
            spec.overlap = match value {
                "skip" => Overlap::Skip,
                "coalesce" => Overlap::Coalesce,
                _ => return Err("--overlap is skip or coalesce".into()),
            }
        }
        "missed" => {
            spec.missed = match value {
                "skip" => Missed::Skip,
                "run-once" => Missed::RunOnce,
                _ => return Err("--missed is skip or run-once".into()),
            }
        }
        "max-cost" => {
            let usd = value
                .parse::<f64>()
                .ok()
                .filter(|v| v.is_finite() && *v > 0.0)
                .ok_or("--max-cost needs a positive amount in USD")?;
            spec.budget = Some(Budget {
                max_cost_usd: Some(usd),
                ..budget()
            });
        }
        "max-tokens" => {
            let tokens = count(flag, value)?;
            spec.budget = Some(Budget {
                max_tokens: Some(tokens),
                ..budget()
            });
        }
        "max-turns" => {
            let turns = count(flag, value)?;
            spec.budget = Some(Budget {
                max_turns: Some(turns),
                ..budget()
            });
        }
        "timeout" => {
            let wall = seconds(flag, value)? as f64;
            spec.budget = Some(Budget {
                max_wall_seconds: Some(wall),
                ..budget()
            });
        }
        other => return Err(format!("Unknown /loop option --{other}")),
    }
    Ok(())
}

fn spec_budget(spec: &LoopSpec) -> Budget {
    spec.budget.clone().unwrap_or_default()
}

fn count(flag: &str, value: &str) -> Result<u64, String> {
    value
        .parse::<u64>()
        .ok()
        .filter(|n| *n > 0)
        .ok_or_else(|| format!("--{flag} needs a positive whole number"))
}

fn seconds(flag: &str, value: &str) -> Result<u64, String> {
    duration(value).ok_or_else(|| format!("--{flag} needs a duration such as 30m or 2h"))
}

/// `Ns`, `Nm`, `Nh` or `Nd` in seconds.
pub fn duration(text: &str) -> Option<u64> {
    let unit = text.chars().last()?;
    let multiplier = match unit {
        's' => 1,
        'm' => 60,
        'h' => 3600,
        'd' => 86_400,
        _ => return None,
    };
    let digits = &text[..text.len() - 1];
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    digits.parse::<u64>().ok()?.checked_mul(multiplier)
}

/// A token that starts like a duration (`5x`, `10`), so it is reported, not used as prompt.
fn looks_like_duration(word: &str) -> bool {
    word.len() <= 8
        && word.bytes().next().is_some_and(|b| b.is_ascii_digit())
        && word.bytes().all(|b| b.is_ascii_alphanumeric())
        && word.bytes().any(|b| b.is_ascii_alphabetic())
}

/// Whether `text` has the shape of a loop ID.
pub fn is_loop_id(text: &str) -> bool {
    text.strip_prefix("lop_").is_some_and(|rest| {
        !rest.is_empty() && rest.len() <= 64 && rest.bytes().all(|b| b.is_ascii_alphanumeric())
    })
}

/// The first whitespace-separated word and the rest.
fn split_word(text: &str) -> (&str, &str) {
    let text = text.trim_start();
    match text.find(char::is_whitespace) {
        Some(end) => (&text[..end], &text[end..]),
        None => (text, ""),
    }
}

/// A flag value: one word, or a single- or double-quoted string.
fn take_value(text: &str) -> Option<(String, &str)> {
    let text = text.trim_start();
    let quote = text.chars().next().filter(|c| *c == '"' || *c == '\'');
    let Some(quote) = quote else {
        let (word, rest) = split_word(text);
        return (!word.is_empty()).then(|| (word.to_owned(), rest));
    };
    let body = &text[1..];
    let end = body.find(quote)?;
    Some((body[..end].to_owned(), &body[end + 1..]))
}

/// Loop limits and defaults from the `loops` config key.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LoopSettings {
    pub max_iterations: u64,
    pub max_per_session: u64,
    pub until_timeout_seconds: u64,
}

impl Default for LoopSettings {
    fn default() -> Self {
        Self {
            max_iterations: 500,
            max_per_session: 50,
            until_timeout_seconds: 60,
        }
    }
}

impl LoopSettings {
    pub fn from_config(config: &Value) -> Self {
        let defaults = Self::default();
        let loops = config.get("loops");
        let number = |key: &str, default: u64| {
            loops
                .and_then(|l| l.get(key))
                .and_then(Value::as_u64)
                .filter(|n| *n > 0)
                .unwrap_or(default)
        };
        Self {
            max_iterations: number("max_iterations", defaults.max_iterations),
            max_per_session: number("max_per_session", defaults.max_per_session),
            until_timeout_seconds: number("until_timeout_seconds", defaults.until_timeout_seconds),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(args: &str) -> LoopSpec {
        match parse(args).unwrap() {
            LoopCommand::Create(spec) => spec,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn interval_and_prompt() {
        let s = spec("5m check the deploy status");
        assert_eq!(s.interval_seconds, Some(300));
        assert_eq!(s.prompt, "check the deploy status");
        assert_eq!(spec("1h /review").prompt, "/review");
    }

    #[test]
    fn self_paced_without_interval() {
        let s = spec("watch CI until green");
        assert_eq!(s.interval_seconds, None);
        assert_eq!(s.prompt, "watch CI until green");
    }

    #[test]
    fn short_and_malformed_intervals_are_refused() {
        assert_eq!(
            parse("10s ping").unwrap_err(),
            "Loop interval must be at least 60s"
        );
        assert!(
            parse("5x ping")
                .unwrap_err()
                .contains("Invalid loop interval")
        );
        assert_eq!(spec("60s ping").interval_seconds, Some(60));
    }

    #[test]
    fn flags_before_the_prompt_only() {
        let s = spec(
            "2m --max 10 --for 2h --until \"curl -sf https://x/health\" --overlap coalesce \
             --missed run-once --max-cost 2.5 --max-turns 30 --timeout 1d run tests -- --verbose",
        );
        assert_eq!(s.interval_seconds, Some(120));
        assert_eq!(s.max_iterations, Some(10));
        assert_eq!(s.for_seconds, Some(7200));
        assert_eq!(s.until.as_deref(), Some("curl -sf https://x/health"));
        assert_eq!(s.overlap, Overlap::Coalesce);
        assert_eq!(s.missed, Missed::RunOnce);
        let budget = s.budget.unwrap();
        assert_eq!(budget.max_cost_usd, Some(2.5));
        assert_eq!(budget.max_turns, Some(30));
        assert_eq!(budget.max_wall_seconds, Some(86_400.0));
        assert_eq!(s.prompt, "run tests -- --verbose");
    }

    #[test]
    fn bad_flags_are_reported() {
        assert_eq!(
            parse("5m --max 0 x").unwrap_err(),
            "--max needs a positive whole number"
        );
        assert_eq!(
            parse("5m --bogus 1 x").unwrap_err(),
            "Unknown /loop option --bogus"
        );
        assert_eq!(parse("5m --until").unwrap_err(), "--until needs a value");
        assert_eq!(
            parse("5m --until \"open x").unwrap_err(),
            "--until needs a value"
        );
        assert_eq!(
            parse("5m --overlap later x").unwrap_err(),
            "--overlap is skip or coalesce"
        );
        assert_eq!(parse("5m --max 3").unwrap_err(), "Loop prompt is empty");
        assert!(parse("").is_err());
    }

    #[test]
    fn list_and_stop() {
        assert_eq!(parse("list").unwrap(), LoopCommand::List);
        assert_eq!(parse("stop").unwrap(), LoopCommand::Stop(None));
        assert_eq!(parse("stop all").unwrap(), LoopCommand::Stop(None));
        assert_eq!(
            parse("stop lop_01ABC").unwrap(),
            LoopCommand::Stop(Some("lop_01ABC".into()))
        );
        assert_eq!(parse("stop nope").unwrap_err(), "Unknown loop nope");
        // "list" followed by text is a prompt, not the subcommand.
        assert_eq!(spec("list the open PRs").prompt, "list the open PRs");
    }

    #[test]
    fn durations() {
        assert_eq!(duration("90s"), Some(90));
        assert_eq!(duration("2d"), Some(172_800));
        assert_eq!(duration("m"), None);
        assert_eq!(duration("1.5h"), None);
    }

    #[test]
    fn settings_defaults_and_overrides() {
        assert_eq!(
            LoopSettings::from_config(&Value::Null),
            LoopSettings::default()
        );
        let s = LoopSettings::from_config(&serde_json::json!({"loops": {"max_iterations": 7}}));
        assert_eq!(s.max_iterations, 7);
        assert_eq!(s.max_per_session, 50);
    }
}
