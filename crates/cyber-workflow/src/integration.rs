//! Owner-selected integration manifests. These types never confer verification
//! authority: only settled host records for the pinned combined source do so.
use std::collections::{BTreeMap, BTreeSet};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct HubReference {
    pub project: String,
    pub task: Option<String>,
    pub agent: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Contribution {
    pub label: String,
    pub commit: String,
    #[serde(default)]
    pub depends_on: Vec<String>,
    pub hub: Option<HubReference>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Check {
    pub argv: Vec<String>,
    #[serde(default = "timeout")]
    pub timeout_seconds: u64,
}
fn timeout() -> u64 {
    300
}
fn reviewer() -> String {
    "verify".into()
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub target_branch: Option<String>,
    pub contributions: Vec<Contribution>,
    pub checks: Vec<Check>,
    #[serde(default = "reviewer")]
    pub reviewer_model: String,
}

impl Manifest {
    pub fn validate(&self) -> Result<Vec<usize>, String> {
        if self.contributions.is_empty() || self.contributions.len() > 20 {
            return Err("an integration requires 1..20 contributions".into());
        }
        if self.checks.is_empty() || self.checks.len() > 8 {
            return Err("an integration requires 1..8 host checks".into());
        }
        for check in &self.checks {
            if check.argv.is_empty()
                || check.argv.len() > 256
                || check
                    .argv
                    .iter()
                    .any(|arg| arg.contains('\0') || arg.len() > 16_384)
                || check.argv[0].is_empty()
                || !(1..=3600).contains(&check.timeout_seconds)
            {
                return Err("invalid literal argv or check timeout (1..3600 seconds)".into());
            }
        }
        if self.reviewer_model.trim().is_empty() {
            return Err("reviewer_model must not be empty".into());
        }
        let mut labels = BTreeMap::new();
        let mut commits = BTreeSet::new();
        for (index, contribution) in self.contributions.iter().enumerate() {
            if !label(&contribution.label) || labels.insert(&contribution.label, index).is_some() {
                return Err(
                    "contribution labels must be unique identifiers of at most 63 bytes".into(),
                );
            }
            if !commit(&contribution.commit)
                || !commits.insert(contribution.commit.to_ascii_lowercase())
            {
                return Err("contributions must pin distinct full Git commit IDs".into());
            }
        }
        let mut complete = BTreeSet::new();
        let mut order = Vec::new();
        while order.len() < self.contributions.len() {
            let next = self
                .contributions
                .iter()
                .enumerate()
                .find(|(index, contribution)| {
                    !complete.contains(index)
                        && contribution.depends_on.iter().all(|name| {
                            labels
                                .get(name)
                                .is_some_and(|dependency| complete.contains(dependency))
                        })
                });
            let Some((index, _)) = next else {
                return Err("contribution dependencies contain a cycle or unknown label".into());
            };
            complete.insert(index);
            order.push(index);
        }
        Ok(order)
    }
}

pub fn commit(value: &str) -> bool {
    matches!(value.len(), 40 | 64) && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}
pub fn label(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 63
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_-".contains(&byte))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn manifest() -> Manifest {
        Manifest {
            target_branch: None,
            contributions: vec![
                Contribution {
                    label: "api".into(),
                    commit: "a".repeat(40),
                    depends_on: vec![],
                    hub: None,
                },
                Contribution {
                    label: "ui".into(),
                    commit: "b".repeat(40),
                    depends_on: vec!["api".into()],
                    hub: None,
                },
            ],
            checks: vec![Check {
                argv: vec!["cargo".into(), "test".into()],
                timeout_seconds: 30,
            }],
            reviewer_model: "verify".into(),
        }
    }
    #[test]
    fn ordering_respects_dependencies_and_preserves_submission_order() {
        let mut input = manifest();
        input.contributions.swap(0, 1);
        assert_eq!(input.validate().unwrap(), vec![1, 0]);
        input.contributions[0].depends_on.clear();
        assert_eq!(input.validate().unwrap(), vec![0, 1]);
    }
    #[test]
    fn cycles_unknown_labels_duplicates_and_nonpinned_commits_are_refused() {
        let mut input = manifest();
        input.contributions[0].depends_on = vec!["ui".into()];
        assert!(input.validate().is_err());
        input.contributions[0].depends_on = vec!["missing".into()];
        assert!(input.validate().is_err());
        input = manifest();
        input.contributions[0].commit = "main".into();
        assert!(input.validate().is_err());
        input = manifest();
        input.contributions[1].commit = input.contributions[0].commit.clone();
        assert!(input.validate().is_err());
    }
    #[test]
    fn literal_argv_limits_and_unknown_fields_fail_before_execution() {
        let mut input = manifest();
        input.checks[0].argv[0] = "bad\0program".into();
        assert!(input.validate().is_err());
        assert!(
            serde_json::from_value::<Manifest>(
                serde_json::json!({"contributions":[],"checks":[],"verified":true})
            )
            .is_err()
        );
    }
}
