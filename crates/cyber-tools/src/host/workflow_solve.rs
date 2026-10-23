//! Read-only solve admission. Captured input is not verification evidence: running
//! candidates/checks and the authoritative report use separate host paths.
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::Path;

use cyber_core::worktrees::{GitExecution, Repository};
use cyber_server::runtime::workflow_start::{self, StartError};
use cyber_server::runtime::{ModelResolver, TurnContext};
use cyber_workflow::{Diagnostic, DiagnosticKind, solve};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use super::{BuiltinHost, Ctx, PrepareError};
use crate::permissions::{Decision, Request};

fn invalid(message: impl Into<String>) -> PrepareError {
    PrepareError::Start(StartError::Module(Diagnostic::execution(message)))
}

fn unavailable_model(message: impl Into<String>) -> PrepareError {
    PrepareError::Start(StartError::Module(Diagnostic::new(
        DiagnosticKind::ModelUnavailable,
        message,
    )))
}

impl BuiltinHost {
    /// Narrow solve children even when the owner permits full-access commands.
    pub(crate) fn solve_isolation(&self, session: &str) -> Result<bool, String> {
        let Some(runtime) = self.runtime() else {
            return Ok(false);
        };
        let Some(id) = self.process_workflow_run(session) else {
            return Ok(false);
        };
        let run = runtime
            .workflow_run(&id)
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "workflow authority disappeared".to_owned())?;
        Ok(run.origin == json!({"kind":"bundled","name":"solve"})
            && cyber_workflow::bundled::module("solve")
                .is_some_and(|module| module.digest == run.digest))
    }

    pub(super) async fn prepare_solve(
        &self,
        owner: &TurnContext,
        value: Value,
    ) -> Result<Value, PrepareError> {
        let args = solve::Args::parse(value).map_err(invalid)?;
        let (config, _) = (self.opts.config)(Path::new(&owner.directory)).map_err(invalid)?;
        let settings = workflow_start::location(&config).map_err(invalid)?;
        let runtime = self
            .runtime()
            .ok_or_else(|| invalid("solve admission requires the runtime"))?;
        let owner_model = runtime
            .state(&owner.session_id)
            .await
            .map_err(|e| invalid(e.to_string()))?
            .info
            .model;
        let models = self
            .opts
            .models
            .as_deref()
            .ok_or_else(|| unavailable_model("solve requires model resolution"))?;
        // Resolve every solver and the judge before inspecting Git or creating children.
        let candidates: Vec<Value> = args.candidates.iter().map(|candidate| {
            let model = resolved_model(models, &settings.tiers, &owner_model, &candidate.model)?;
            Ok(json!({"label":candidate.label, "requested_model":candidate.model, "model":model}))
        }).collect::<Result<_, PrepareError>>()?;
        let judge = resolved_model(models, &settings.tiers, &owner_model, &args.judge_model)?;
        let base = self.solve_base(owner).await?;
        let distinct: std::collections::BTreeSet<&str> = candidates
            .iter()
            .filter_map(|c| c["model"].as_str())
            .map(|reference| reference.split('#').next().unwrap_or(reference))
            .collect();
        let mut prepared = serde_json::to_value(&args).map_err(|e| invalid(e.to_string()))?;
        prepared["solve_admission"] = json!({
            "base": base,
            "candidates": candidates,
            "judge_model":judge,
            "distinct_solver_models":distinct.len(),
        });
        Ok(prepared)
    }

    async fn solve_base(&self, owner: &TurnContext) -> Result<Value, PrepareError> {
        let inv =
            super::command_skills::command_invocation(owner, "workflow", json!({"name":"solve"}));
        let policy = self.policy(&inv).await.map_err(PrepareError::Unavailable)?;
        let ctx = Ctx {
            skill_paths: Default::default(),
            loaded_skill: Default::default(),
            compiler_feedback: Default::default(),
            hook_decision: None,
            denial: Default::default(),
            host: self,
            policy,
            location: Path::new(&owner.directory).to_path_buf(),
            inv: &inv,
            cancel: CancellationToken::new(),
        };
        check_read(&ctx, &ctx.location)?;
        let git = crate::worktrees::sandboxed_git(&ctx, None);
        let repo = Repository::discover(&git, &ctx.location)
            .await
            .map_err(|e| invalid(format!("solve requires an inspectable Git checkout: {e}")))?;
        check_read(&ctx, &repo.root)?;
        let before = git_text(
            &git,
            &repo.root,
            &["rev-parse", "--verify", "HEAD^{commit}"],
        )
        .await?;
        let diff = super::review::solve_diff(&ctx).await.map_err(|error| {
            invalid(format!(
                "solve read-only checkout inspection failed: {error}"
            ))
        })?;
        if diff.root != repo.root || diff.base.as_deref() != Some(before.trim()) {
            return Err(invalid(
                "solve repository identity or base changed during inspection",
            ));
        }
        if !diff.limitations.is_empty() {
            return Err(invalid(
                "solve could not establish a complete clean checkout inspection",
            ));
        }
        if !diff.files.is_empty() {
            return Err(invalid(
                "solve requires a clean working tree: commit tracked changes and non-ignored untracked files, or choose an isolated clean checkout; no files were stashed",
            ));
        }
        let after = git_text(
            &git,
            &repo.root,
            &["rev-parse", "--verify", "HEAD^{commit}"],
        )
        .await?;
        if before != after {
            return Err(invalid(
                "solve base changed during inspection; retry from a stable checkout",
            ));
        }
        let commit = before.trim();
        if !matches!(commit.len(), 40 | 64) || !commit.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(invalid("solve could not verify a committed Git HEAD"));
        }
        Ok(json!({"commit":commit, "directory":repo.root, "common_dir":repo.common_dir}))
    }
}

fn check_read(ctx: &Ctx<'_>, root: &Path) -> Result<(), PrepareError> {
    let action = if root.starts_with(&ctx.location) {
        "read"
    } else {
        "external_directory"
    };
    let request = Request {
        tool: Some("read".into()),
        action: action.into(),
        resources: vec![format!("{}/*", crate::permissions::slash(root))],
        read_only: true,
        ..Default::default()
    };
    if let Decision::Deny(reason) = ctx.policy.user_delegation(&request) {
        return Err(PrepareError::Forbidden(format!(
            "Permission denied: {reason}"
        )));
    }
    Ok(())
}

async fn git_text(
    git: &dyn GitExecution,
    directory: &Path,
    args: &[&str],
) -> Result<String, PrepareError> {
    let argv: Vec<OsString> = std::iter::once("--no-optional-locks")
        .chain(args.iter().copied())
        .map(OsString::from)
        .collect();
    let output = git
        .run(directory, &argv)
        .await
        .map_err(|e| invalid(format!("solve Git inspection unavailable: {e}")))?;
    if !output.status.success() {
        return Err(invalid(
            "solve requires a Git checkout with a committed HEAD; Git inspection failed",
        ));
    }
    String::from_utf8(output.stdout)
        .map_err(|_| invalid("solve Git inspection returned non-Unicode metadata"))
}

fn resolved_model(
    models: &dyn ModelResolver,
    tiers: &BTreeMap<String, Option<String>>,
    owner: &str,
    requested: &str,
) -> Result<String, PrepareError> {
    let reference = if requested.contains('/') {
        requested
    } else {
        tiers
            .get(requested)
            .ok_or_else(|| unavailable_model(format!("Unknown solve model tier: {requested}")))?
            .as_deref()
            .unwrap_or(owner)
    };
    let resolved = models
        .resolve(reference)
        .map_err(|_| unavailable_model(format!("Solve model unavailable: {requested}")))?;
    let mut effective = format!("{}/{}", resolved.provider, resolved.model);
    if let Some((_, variant)) = reference.split_once('#') {
        effective.push('#');
        effective.push_str(variant);
    }
    Ok(effective)
}
