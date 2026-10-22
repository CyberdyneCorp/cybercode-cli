//! Goals (`goals`): the `goals` settings, the Goal status machine, condition rules and the
//! `/goal` grammar, shared by the server, the TUI and `cyber exec`.
//!
//! ```text
//! /goal                         show the active Goal, the queue and the last verdict
//! /goal [--check "<cmd>"] <condition>      set the active Goal (clears the current one)
//! /goal add [--check "<cmd>"] <condition>  queue a Goal (activates when none is active)
//! /goal list | pause | resume | skip | next | clear [<gol_id>|all] | edit [<gol_id>]
//! /goal fork [--check "<cmd>"] <condition>  pursue it in a new background Session
//! /goal use <template> [text]               set the Goal from `goals.templates.<template>`
//! ```
use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::budget::Budget;

const KEY: &str = "goals";
/// The longest condition, in characters.
pub const MAX_CONDITION: usize = 4000;
/// The message for a longer condition (`goals` → Goal model).
pub const CONDITION_TOO_LONG: &str =
    "Goal conditions are limited to 4000 characters; put details in a file and reference it";
/// `budgets.goal.max_turns` when no budget sets it.
pub const DEFAULT_MAX_TURNS: u64 = 50;
/// Verdict reasons are cut to this many characters.
pub const MAX_REASON: usize = 300;

/// A Goal's status (`goals` → Goal model).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum GoalStatus {
    Pending,
    Active,
    Paused,
    Met,
    Impossible,
    Cleared,
    BudgetExceeded,
}

impl GoalStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Active => "active",
            Self::Paused => "paused",
            Self::Met => "met",
            Self::Impossible => "impossible",
            Self::Cleared => "cleared",
            Self::BudgetExceeded => "budget_exceeded",
        }
    }

    /// No transition leaves a terminal status.
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Met | Self::Impossible | Self::Cleared | Self::BudgetExceeded
        )
    }

    /// Whether `self → next` is a legal transition.
    pub fn can_become(self, next: Self) -> bool {
        use GoalStatus::*;
        matches!(
            (self, next),
            (Pending, Active | Cleared)
                | (Active, Paused | Met | Impossible | Cleared | BudgetExceeded)
                | (Paused, Active | Cleared)
        )
    }
}

/// An evaluator verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    Met,
    NotMet,
    Impossible,
}

/// Refuse an empty or over-long condition.
pub fn check_condition(condition: &str) -> Result<(), String> {
    let condition = condition.trim();
    if condition.is_empty() {
        return Err("A goal needs a condition".into());
    }
    if condition.chars().count() > MAX_CONDITION {
        return Err(CONDITION_TOO_LONG.into());
    }
    Ok(())
}

/// What happens to the queue after an `impossible` Goal.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum OnImpossible {
    #[default]
    Stop,
    Continue,
}

/// Where `/goal fork` runs its Session.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum ForkIsolation {
    #[default]
    Worktree,
    None,
}

/// `goals.templates.<name>`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GoalTemplate {
    pub condition: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub check: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget: Option<Budget>,
}

impl GoalTemplate {
    /// The condition with `$ARGUMENTS` replaced by `args`; without the placeholder,
    /// non-empty `args` are appended on a new line.
    pub fn condition_for(&self, args: &str) -> String {
        let args = args.trim();
        if self.condition.contains("$ARGUMENTS") {
            return self.condition.replace("$ARGUMENTS", args).trim().to_owned();
        }
        if args.is_empty() {
            return self.condition.trim().to_owned();
        }
        format!("{}\n{args}", self.condition.trim())
    }
}

/// The resolved `goals` key.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct GoalSettings {
    pub max_queue: u64,
    pub check_timeout_seconds: u64,
    pub on_impossible: OnImpossible,
    pub fork_isolation: ForkIsolation,
    pub unattended_ask_seconds: u64,
    pub evaluator_max_input_tokens: u64,
    pub templates: BTreeMap<String, GoalTemplate>,
}

impl Default for GoalSettings {
    fn default() -> Self {
        Self {
            max_queue: 20,
            check_timeout_seconds: 600,
            on_impossible: OnImpossible::Stop,
            fork_isolation: ForkIsolation::Worktree,
            unattended_ask_seconds: 300,
            evaluator_max_input_tokens: 8000,
            templates: BTreeMap::new(),
        }
    }
}

impl GoalSettings {
    /// The `goals` key of `config`; an invalid field is an error naming it.
    pub fn from_config(config: &Value) -> Result<Self, String> {
        let mut settings = Self::default();
        let Some(goals) = config.get(KEY).filter(|v| !v.is_null()) else {
            return Ok(settings);
        };
        let map = goals
            .as_object()
            .ok_or_else(|| format!("{KEY}: expected an object"))?;
        for (field, value) in map {
            settings.apply(field, value)?;
        }
        Ok(settings)
    }

    fn apply(&mut self, field: &str, value: &Value) -> Result<(), String> {
        let ranged = |min: u64, max: u64| {
            value
                .as_u64()
                .filter(|n| (min..=max).contains(n))
                .ok_or_else(|| {
                    format!("{KEY}.{field}: expected a whole number from {min} to {max}")
                })
        };
        match field {
            "max_queue" => self.max_queue = ranged(1, 200)?,
            "check_timeout_seconds" => self.check_timeout_seconds = ranged(1, 86_400)?,
            "unattended_ask_seconds" => self.unattended_ask_seconds = ranged(1, 86_400)?,
            "evaluator_max_input_tokens" => {
                self.evaluator_max_input_tokens = ranged(1000, 200_000)?;
            }
            "on_impossible" => self.on_impossible = enum_value(field, value)?,
            "fork_isolation" => self.fork_isolation = enum_value(field, value)?,
            "templates" => self.templates = templates(value)?,
            other => return Err(format!("{KEY}.{other}: unknown setting")),
        }
        Ok(())
    }
}

fn enum_value<T: serde::de::DeserializeOwned>(field: &str, value: &Value) -> Result<T, String> {
    serde_json::from_value(value.clone()).map_err(|e| format!("{KEY}.{field}: {e}"))
}

fn templates(value: &Value) -> Result<BTreeMap<String, GoalTemplate>, String> {
    let parsed: BTreeMap<String, GoalTemplate> =
        serde_json::from_value(value.clone()).map_err(|e| format!("{KEY}.templates: {e}"))?;
    for (name, template) in &parsed {
        check_condition(&template.condition)
            .map_err(|e| format!("{KEY}.templates.{name}.condition: {e}"))?;
        if let Some(budget) = &template.budget {
            budget
                .validate()
                .map_err(|e| format!("{KEY}.templates.{name}.budget.{e}"))?;
        }
    }
    Ok(parsed)
}

/// The Budget a Goal enforces: its own, else `budgets.goal`, with `max_turns` defaulting
/// to [`DEFAULT_MAX_TURNS`].
pub fn effective_budget(own: Option<Budget>, configured: Option<Budget>) -> Budget {
    let mut budget = own.or(configured).unwrap_or_default();
    budget.max_turns.get_or_insert(DEFAULT_MAX_TURNS);
    budget
}

/// A parsed `/goal` command.
#[derive(Debug, Clone, PartialEq)]
pub enum GoalCommand {
    Show,
    List,
    Set {
        condition: String,
        check: Option<String>,
    },
    Add {
        condition: String,
        check: Option<String>,
    },
    /// `/goal edit [id]`: put the condition in the composer.
    Edit(Option<String>),
    /// `/goal edit <id> <condition>`: replace the condition.
    EditCondition {
        goal_id: String,
        condition: String,
    },
    Pause,
    Resume,
    Skip,
    /// `None` clears every Goal of the Session.
    Clear(Option<String>),
    /// `/goal fork`: a new background Session pursues this Goal.
    Fork {
        condition: String,
        check: Option<String>,
    },
    /// `/goal use <template> [text]`.
    Use {
        template: String,
        args: String,
    },
}

/// Parse the arguments of `/goal` (everything after the command name).
pub fn parse(args: &str) -> Result<GoalCommand, String> {
    let args = args.trim();
    let (first, rest) = split_word(args);
    let rest = rest.trim();
    let id = |rest: &str| -> Result<Option<String>, String> {
        match rest {
            "" => Ok(None),
            id if is_goal_id(id) => Ok(Some(id.to_owned())),
            other => Err(format!("Unknown goal {other}")),
        }
    };
    match first {
        "" => Ok(GoalCommand::Show),
        "list" if rest.is_empty() => Ok(GoalCommand::List),
        "pause" if rest.is_empty() => Ok(GoalCommand::Pause),
        "resume" if rest.is_empty() => Ok(GoalCommand::Resume),
        "skip" | "next" if rest.is_empty() => Ok(GoalCommand::Skip),
        "clear" if rest == "all" => Ok(GoalCommand::Clear(None)),
        "clear" => match id(rest)? {
            Some(id) => Ok(GoalCommand::Clear(Some(id))),
            None => Ok(GoalCommand::Clear(None)),
        },
        "edit" => {
            let (target, text) = split_word(rest);
            let text = text.trim();
            if text.is_empty() {
                return id(target).map(GoalCommand::Edit);
            }
            let goal_id = id(target)?.ok_or("Unknown goal")?;
            check_condition(text)?;
            Ok(GoalCommand::EditCondition {
                goal_id,
                condition: text.to_owned(),
            })
        }
        "fork" => {
            let (condition, check) = condition_and_check(rest)?;
            Ok(GoalCommand::Fork { condition, check })
        }
        "use" => {
            let (template, args) = split_word(rest);
            if template.is_empty() {
                return Err("/goal use needs a template name".into());
            }
            Ok(GoalCommand::Use {
                template: template.to_owned(),
                args: args.trim().to_owned(),
            })
        }
        "add" => {
            let (condition, check) = condition_and_check(rest)?;
            Ok(GoalCommand::Add { condition, check })
        }
        _ => {
            let (condition, check) = condition_and_check(args)?;
            Ok(GoalCommand::Set { condition, check })
        }
    }
}

/// `[--check "<cmd>"] <condition>`.
fn condition_and_check(text: &str) -> Result<(String, Option<String>), String> {
    let mut rest = text.trim();
    let mut check = None;
    if let Some(after) = rest.strip_prefix("--check") {
        let (value, after) =
            take_value(after).ok_or("--check needs a command, quoted when it has spaces")?;
        if value.trim().is_empty() {
            return Err("--check needs a command".into());
        }
        check = Some(value);
        rest = after.trim();
    }
    check_condition(rest)?;
    Ok((rest.to_owned(), check))
}

/// Whether `text` has the shape of a Goal ID.
pub fn is_goal_id(text: &str) -> bool {
    text.strip_prefix("gol_").is_some_and(|rest| {
        !rest.is_empty() && rest.len() <= 64 && rest.bytes().all(|b| b.is_ascii_alphanumeric())
    })
}

fn split_word(text: &str) -> (&str, &str) {
    let text = text.trim_start();
    match text.find(char::is_whitespace) {
        Some(end) => (&text[..end], &text[end..]),
        None => (text, ""),
    }
}

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

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn transitions_follow_the_goal_model() {
        use GoalStatus::*;
        assert!(Pending.can_become(Active));
        assert!(Active.can_become(Paused) && Paused.can_become(Active));
        assert!(Active.can_become(BudgetExceeded));
        for terminal in [Met, Impossible, Cleared, BudgetExceeded] {
            assert!(terminal.is_terminal());
            for next in [Pending, Active, Paused, Met, Cleared] {
                assert!(!terminal.can_become(next), "{terminal:?} -> {next:?}");
            }
        }
        assert!(!Pending.can_become(Met));
        assert!(!Paused.can_become(Met));
    }

    #[test]
    fn conditions_are_bounded() {
        assert_eq!(
            check_condition(&"x".repeat(5000)).unwrap_err(),
            CONDITION_TOO_LONG
        );
        assert!(check_condition(&"x".repeat(4000)).is_ok());
        assert!(check_condition("  ").is_err());
    }

    #[test]
    fn the_goal_grammar() {
        assert_eq!(parse("").unwrap(), GoalCommand::Show);
        assert_eq!(
            parse("all call sites compile").unwrap(),
            GoalCommand::Set {
                condition: "all call sites compile".into(),
                check: None
            }
        );
        assert_eq!(
            parse(r#"--check "npm test" all lint errors fixed"#).unwrap(),
            GoalCommand::Set {
                condition: "all lint errors fixed".into(),
                check: Some("npm test".into())
            }
        );
        assert_eq!(
            parse("add --check make B done").unwrap(),
            GoalCommand::Add {
                condition: "B done".into(),
                check: Some("make".into())
            }
        );
    }

    #[test]
    fn the_goal_management_grammar() {
        assert_eq!(parse("next").unwrap(), GoalCommand::Skip);
        assert_eq!(parse("clear all").unwrap(), GoalCommand::Clear(None));
        assert_eq!(
            parse("clear gol_01AB").unwrap(),
            GoalCommand::Clear(Some("gol_01AB".into()))
        );
        assert_eq!(parse("clear nope").unwrap_err(), "Unknown goal nope");
        assert_eq!(parse("edit").unwrap(), GoalCommand::Edit(None));
        assert_eq!(
            parse("edit gol_01AB tests and docs pass").unwrap(),
            GoalCommand::EditCondition {
                goal_id: "gol_01AB".into(),
                condition: "tests and docs pass".into()
            }
        );
        assert_eq!(parse("edit nope text").unwrap_err(), "Unknown goal nope");
        assert_eq!(
            parse("fork --check 'cargo test' deps upgraded").unwrap(),
            GoalCommand::Fork {
                condition: "deps upgraded".into(),
                check: Some("cargo test".into())
            }
        );
        assert_eq!(
            parse("use green auth module refactored").unwrap(),
            GoalCommand::Use {
                template: "green".into(),
                args: "auth module refactored".into()
            }
        );
        assert!(parse("use").is_err());
        assert_eq!(
            parse("--check").unwrap_err(),
            "--check needs a command, quoted when it has spaces"
        );
        // "list" followed by text is a condition.
        assert!(matches!(
            parse("list of bugs is empty").unwrap(),
            GoalCommand::Set { .. }
        ));
    }

    #[test]
    fn settings_defaults_ranges_and_templates() {
        assert_eq!(
            GoalSettings::from_config(&Value::Null).unwrap(),
            GoalSettings::default()
        );
        let s = GoalSettings::from_config(
            &json!({"goals": {"max_queue": 5, "on_impossible": "continue",
            "templates": {"green": {"condition": "tests pass", "check": "cargo test"}}}}),
        )
        .unwrap();
        assert_eq!((s.max_queue, s.on_impossible), (5, OnImpossible::Continue));
        assert_eq!(s.templates["green"].check.as_deref(), Some("cargo test"));
        assert_eq!(
            GoalSettings::from_config(&json!({"goals": {"max_queue": 0}})).unwrap_err(),
            "goals.max_queue: expected a whole number from 1 to 200"
        );
        assert!(
            GoalSettings::from_config(&json!({"goals": {"evaluator_max_input_tokens": 10}}))
                .is_err()
        );
        assert!(GoalSettings::from_config(&json!({"goals": {"bogus": 1}})).is_err());
    }

    #[test]
    fn templates_substitute_their_arguments() {
        let green = GoalTemplate {
            condition: "$ARGUMENTS and all tests pass".into(),
            check: Some("just test".into()),
            budget: None,
        };
        assert_eq!(
            green.condition_for("auth module refactored"),
            "auth module refactored and all tests pass"
        );
        let plain = GoalTemplate {
            condition: "lint is clean".into(),
            ..green
        };
        assert_eq!(plain.condition_for(""), "lint is clean");
        assert_eq!(
            plain.condition_for("in crates/"),
            "lint is clean\nin crates/"
        );
    }

    #[test]
    fn budgets_default_to_fifty_turns() {
        assert_eq!(effective_budget(None, None).max_turns, Some(50));
        let own = Budget {
            max_turns: Some(3),
            ..Budget::default()
        };
        assert_eq!(effective_budget(Some(own), None).max_turns, Some(3));
        let configured = Budget {
            max_cost_usd: Some(1.0),
            ..Budget::default()
        };
        let b = effective_budget(None, Some(configured));
        assert_eq!((b.max_turns, b.max_cost_usd), (Some(50), Some(1.0)));
    }
}
