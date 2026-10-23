//! What a Workflow Run needs before it is recorded (`workflows` → Bundled workflow
//! contracts): its `args` checked against `meta.args`, then the host preparation a module
//! names in `meta.prepare`:
//!
//! - `review_target` resolves `args.target` as `/review` does, under the reviewer's own
//!   permissions, and passes the diff as `args.diff`;
//! - `migrate_permissions` checks that the owner's rules let an edit-capable agent edit the
//!   selected files and run the check commands without asking, when no one can answer;
//! - `research_sources` records whether the reviewer may use the web tools here, as
//!   `args.web_permitted`.

use std::path::Path;

use cyber_server::runtime::workflow_start::{self, StartError};
use cyber_server::runtime::{CommandPlanError, TurnContext};
use cyber_workflow::WorkflowModule;
use serde_json::{Value, json};

use super::BuiltinHost;
use super::isolation::PermissionTest;
use crate::permissions::{Decision, Request};

/// Why a run's arguments were refused before the run exists.
#[derive(Debug)]
pub enum PrepareError {
    /// The arguments or the module are invalid (`ExecutionFailure: args.<path> …`).
    Start(StartError),
    /// The review target could not be resolved (`ReviewTargetError`).
    Review(cyber_core::review::TargetError),
    /// The owner's rules do not allow what the run needs.
    Forbidden(String),
    /// Preparation failed for another reason.
    Unavailable(String),
}

impl std::fmt::Display for PrepareError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Start(error) => write!(f, "{error}"),
            Self::Review(error) => {
                write!(f, "ReviewTargetError ({:?}): {}", error.code, error.message)
            }
            Self::Forbidden(message) | Self::Unavailable(message) => f.write_str(message),
        }
    }
}

impl From<CommandPlanError> for PrepareError {
    fn from(error: CommandPlanError) -> Self {
        match error {
            CommandPlanError::Review(error) => Self::Review(error),
            CommandPlanError::Forbidden(message) => Self::Forbidden(message),
            other => Self::Unavailable(other.to_string()),
        }
    }
}

impl BuiltinHost {
    /// Validate `args` and apply the module's `meta.prepare`; returns the args the run
    /// starts with. `interactive` is whether someone can answer the run's agents' asks.
    pub async fn prepare_workflow_args(
        &self,
        owner: &TurnContext,
        module: &WorkflowModule,
        args: Value,
        interactive: bool,
    ) -> Result<Value, PrepareError> {
        workflow_start::validate_args(module, &args).map_err(PrepareError::Start)?;
        match module.meta.raw.get("prepare").and_then(Value::as_str) {
            Some("review_target") => self.prepare_review(owner, args).await,
            Some("migrate_permissions") if !interactive => {
                self.check_migrate(owner, &args).await?;
                Ok(args)
            }
            Some("research_sources") => self.prepare_research(owner, args, interactive).await,
            _ => Ok(args),
        }
    }

    async fn prepare_review(
        &self,
        owner: &TurnContext,
        mut args: Value,
    ) -> Result<Value, PrepareError> {
        let target = args
            .get("target")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let inv =
            super::command_skills::command_invocation(owner, "workflow", json!({"name": "review"}));
        let policy = self.policy(&inv).await.map_err(PrepareError::Unavailable)?;
        args["diff"] = self.review_diff(&inv, policy, &target).await?;
        Ok(args)
    }

    /// Every edit and check command a non-interactive `migrate` needs must be allowed for
    /// the `general` agent under the owner's rules; the refusal names each missing rule.
    async fn check_migrate(&self, owner: &TurnContext, args: &Value) -> Result<(), PrepareError> {
        let location = Path::new(&owner.directory);
        let inv = super::command_skills::command_invocation(
            owner,
            "workflow",
            json!({"name": "migrate"}),
        );
        let policy = self
            .policy_for(&inv, Some("general"))
            .await
            .map_err(PrepareError::Unavailable)?;
        let mut missing = Vec::new();
        for (tool, resource, command) in migrate_requests(args) {
            let test = PermissionTest {
                tool: Some(tool.into()),
                resource: resource.clone(),
                command: command.clone(),
                ..PermissionTest::default()
            };
            let request = self
                .test_request(location, &test)
                .map_err(PrepareError::Unavailable)?;
            if !matches!(policy.decide(&request), Decision::Allow) {
                missing.push(format!(
                    "{tool} {}",
                    command.or(resource).unwrap_or_default()
                ));
            }
        }
        if missing.is_empty() {
            return Ok(());
        }
        Err(PrepareError::Forbidden(format!(
            "Permission denied: migrate runs without anyone to approve its agents, and no rule allows: {}. Add allow rules for them (for example `\"permissions\": {{\"edit\": {{\"src/**\": \"allow\"}}}}`) or start it from an attended Session.",
            missing.join("; ")
        )))
    }

    async fn prepare_research(
        &self,
        owner: &TurnContext,
        mut args: Value,
        interactive: bool,
    ) -> Result<Value, PrepareError> {
        let inv = super::command_skills::command_invocation(
            owner,
            "workflow",
            json!({"name": "research"}),
        );
        let reader = self
            .policy_for(&inv, Some("reviewer"))
            .await
            .map_err(PrepareError::Unavailable)?;
        let usable = |action: &str, resource: &str| match reader.decide(&Request {
            tool: Some(action.into()),
            action: action.into(),
            resources: vec![resource.into()],
            read_only: true,
            ..Request::default()
        }) {
            Decision::Allow => true,
            Decision::Ask => interactive,
            Decision::Deny(_) => false,
        };
        let permitted = usable("websearch", "*") && usable("webfetch", "https://example.com/");
        args["web_permitted"] = permitted.into();
        Ok(args)
    }
}

/// `(tool, resource, command)` of every permission `migrate` needs: an edit of each listed
/// file (or of the `pattern`), and each check command.
fn migrate_requests(args: &Value) -> Vec<(&'static str, Option<String>, Option<String>)> {
    let strings = |key: &str| -> Vec<String> {
        args.get(key)
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default()
    };
    let mut paths = strings("files");
    if paths.is_empty()
        && let Some(pattern) = args.get("pattern").and_then(Value::as_str)
    {
        paths.push(pattern.to_owned());
    }
    let edits = paths.into_iter().map(|path| ("edit", Some(path), None));
    let checks = strings("check")
        .into_iter()
        .map(|command| ("bash", None, Some(command)));
    edits.chain(checks).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrate_needs_an_edit_per_file_or_the_pattern_and_each_check() {
        let requests =
            migrate_requests(&json!({"files": ["a.rs", "b.rs"], "check": ["cargo check"]}));
        assert_eq!(
            requests,
            [
                ("edit", Some("a.rs".into()), None),
                ("edit", Some("b.rs".into()), None),
                ("bash", None, Some("cargo check".into())),
            ]
        );
        assert_eq!(
            migrate_requests(&json!({"pattern": "src/**/*.rs"})),
            [("edit", Some("src/**/*.rs".into()), None)]
        );
    }
}
