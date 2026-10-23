//! Admission inputs for the verified `solve` workflow. This module does not assert that
//! candidates or their checks ran; that authority belongs to the host's future evidence.
use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Candidate {
    pub label: String,
    pub model: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Check {
    pub argv: Vec<String>,
    #[serde(default = "check_timeout")]
    pub timeout_seconds: u64,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Args {
    pub task: String,
    #[serde(default = "candidates")]
    pub candidates: Vec<Candidate>,
    pub checks: Vec<Check>,
    #[serde(default = "judge_model")]
    pub judge_model: String,
}

fn check_timeout() -> u64 {
    300
}

fn judge_model() -> String {
    "verify".into()
}

fn candidates() -> Vec<Candidate> {
    ["fast", "smart", "verify"]
        .into_iter()
        .map(|name| Candidate {
            label: name.into(),
            model: name.into(),
        })
        .collect()
}

impl Args {
    /// Reject malformed inputs before native inspection or any child admission. Commands
    /// remain argv arrays; nothing here joins, expands or executes a shell command.
    pub fn parse(value: Value) -> Result<Self, String> {
        let args: Self = serde_json::from_value(value).map_err(|e| format!("args: {e}"))?;
        if args.task.trim().is_empty() || args.task.len() > 32768 {
            return Err("args.task: expected 1–32768 UTF-8 bytes of nonblank task text".into());
        }
        args.validate_candidates()?;
        if !(1..=8).contains(&args.checks.len()) {
            return Err("args.checks: expected 1–8 checks".into());
        }
        for (index, check) in args.checks.iter().enumerate() {
            validate_check(check).map_err(|e| format!("args.checks[{index}]: {e}"))?;
        }
        if args.judge_model.trim().is_empty() {
            return Err("args.judge_model: expected a model ref or tier".into());
        }
        Ok(args)
    }

    fn validate_candidates(&self) -> Result<(), String> {
        if !(2..=4).contains(&self.candidates.len()) {
            return Err("args.candidates: expected 2–4 candidates".into());
        }
        let mut labels = BTreeSet::new();
        for (index, candidate) in self.candidates.iter().enumerate() {
            let label = &candidate.label;
            if label.is_empty()
                || label.len() > 48
                || !label
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
            {
                return Err(format!(
                    "args.candidates[{index}].label: expected 1–48 ASCII letters, digits, underscores or hyphens"
                ));
            }
            if !labels.insert(label) {
                return Err(format!(
                    "args.candidates[{index}].label: duplicate label {label:?}"
                ));
            }
            if candidate.model.trim().is_empty() {
                return Err(format!(
                    "args.candidates[{index}].model: expected a model ref or tier"
                ));
            }
        }
        Ok(())
    }
}

fn validate_check(check: &Check) -> Result<(), &'static str> {
    if !(1..=3600).contains(&check.timeout_seconds) {
        return Err("timeout_seconds must be between 1 and 3600");
    }
    if check.argv.is_empty() || check.argv.len() > 256 || check.argv[0].trim().is_empty() {
        return Err("argv requires a nonblank program and at most 256 arguments");
    }
    if check
        .argv
        .iter()
        .any(|arg| arg.contains('\0') || arg.len() > 32768)
        || check.argv.iter().map(String::len).sum::<usize>() > 131072
    {
        return Err(
            "argv must contain no NUL, at most 32768 bytes per argument and at most 131072 bytes total",
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn input() -> Value {
        json!({"task":"fix the parser", "checks":[{"argv":["test", "a file", "$(touch marker)", ""]}]})
    }

    #[test]
    fn defaults_preserve_argv_as_literal_data() {
        let args = Args::parse(input()).unwrap();
        assert_eq!(
            args.candidates
                .iter()
                .map(|c| c.model.as_str())
                .collect::<Vec<_>>(),
            ["fast", "smart", "verify"]
        );
        assert_eq!(args.judge_model, "verify");
        assert_eq!(args.checks[0].timeout_seconds, 300);
        assert_eq!(
            args.checks[0].argv,
            ["test", "a file", "$(touch marker)", ""]
        );
    }

    #[test]
    fn rejects_invalid_or_unbounded_checks() {
        for check in [
            json!({"argv":[]}),
            json!({"argv":[" "]}),
            json!({"argv":["test\u{0000}x"]}),
            json!({"argv":["test"],"timeout_seconds":0}),
            json!({"argv":["test"],"timeout_seconds":3601}),
            json!({"argv":vec!["x";257]}),
            json!({"argv":["x".repeat(32769)]}),
            json!({"argv":vec!["x".repeat(32768);5]}),
        ] {
            let mut value = input();
            value["checks"] = json!([check]);
            assert!(Args::parse(value).is_err());
        }
    }

    #[test]
    fn rejects_invalid_candidates_and_reserved_host_metadata() {
        for candidates in [
            json!([]),
            json!([{"label":"x","model":"p/m"}]),
            json!([{"label":"x","model":"p/m"},{"label":"x","model":"p/m"}]),
            json!([{"label":"bad/name","model":"p/m"},{"label":"b","model":"p/m"}]),
            json!([{"label":"a","model":" "},{"label":"b","model":"p/m"}]),
        ] {
            let mut value = input();
            value["candidates"] = candidates;
            assert!(Args::parse(value).is_err());
        }
        let mut value = input();
        value["solve_admission"] = json!({"base_commit":"forged"});
        assert!(Args::parse(value).is_err());
    }

    #[test]
    fn task_limit_is_bytes_not_unicode_characters() {
        let mut value = input();
        value["task"] = "é".repeat(16385).into();
        assert!(Args::parse(value).is_err());
        let mut value = input();
        value["task"] = " \n".into();
        assert!(Args::parse(value).is_err());
    }
}
