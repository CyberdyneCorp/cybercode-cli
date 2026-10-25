//! Explicit owner publication; the integration script has no publication call.
//! An uncertain mutating command is never repeated merely because a response was lost.
use super::workflow_integration::{hash, text};
use super::{BuiltinHost, Ctx};
use crate::tools::{ToolError, failed};
use serde_json::{Value, json};

fn field<'a>(input: &'a Value, name: &str) -> Result<&'a str, ToolError> {
    input[name]
        .as_str()
        .filter(|value| !value.is_empty() && !value.contains('\0'))
        .ok_or_else(|| failed(format!("missing publication {name}")))
}

impl BuiltinHost {
    pub(super) async fn publish_integration(
        &self,
        ctx: &Ctx<'_>,
        run: &str,
        args: &Value,
    ) -> Result<Value, ToolError> {
        let runtime = self
            .runtime()
            .ok_or_else(|| failed("runtime unavailable"))?;
        let workflow = runtime
            .workflow_run(run)
            .map_err(|e| failed(e.to_string()))?
            .ok_or_else(|| failed("integration run missing"))?;
        if workflow.status != cyber_server::runtime::WorkflowRunStatus::Completed {
            return Err(failed(
                "publication requires a completed integration workflow",
            ));
        }
        if workflow
            .result
            .as_ref()
            .is_none_or(|report| report["review_status"] != "reviewed")
            && ctx.inv.input["acknowledge_review_unavailable"] != true
        {
            return Err(failed(
                "independent review is unavailable; explicitly acknowledge that limitation before publication",
            ));
        }
        let manifest:cyber_workflow::integration::Manifest=serde_json::from_value(json!({"target_branch":args["target_branch"],"contributions":args["contributions"],"checks":args["checks"],"reviewer_model":args["reviewer_model"]})).map_err(|e|failed(e.to_string()))?;
        let owner = cyber_server::runtime::TurnContext {
            session_id: ctx.inv.session_id.clone(),
            directory: ctx.inv.directory.clone(),
            agent: ctx.inv.agent.clone(),
            mode: ctx.inv.mode.clone(),
            prefers_apply_patch: false,
            rules: ctx.inv.rules.clone(),
            worktree: Default::default(),
        };
        for contribution in &manifest.contributions {
            self.authorize_integration_reference(&owner, contribution.hub.as_ref())
                .await
                .map_err(failed)?;
        }
        let evidence = self.check_integration(ctx, run, args, false).await?;
        if evidence["eligible"] != true {
            return Err(failed(
                "integration source or target is stale, or checks did not pass",
            ));
        }
        let operation = field(&ctx.inv.input, "operation_id")?;
        if operation.len() > 128
            || !operation
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"_-".contains(&byte))
        {
            return Err(failed("invalid publication operation_id"));
        }
        let remote = field(&ctx.inv.input, "remote")?;
        if remote.len() > 128
            || remote.starts_with('-')
            || !remote
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"_.-".contains(&byte))
        {
            return Err(failed(
                "publication remote must be a configured Git remote name",
            ));
        }
        let repository = field(&ctx.inv.input, "repository")?;
        let title = field(&ctx.inv.input, "title")?;
        let body = ctx.inv.input["body"]
            .as_str()
            .ok_or_else(|| failed("missing PR body"))?;
        if title.len() > 256 || body.len() > 64 * 1024 {
            return Err(failed("publication title/body exceeds its limit"));
        }
        let root = std::path::Path::new(&ctx.inv.directory);
        let git = crate::worktrees::sandboxed_git(ctx, None);
        super::workflow_integration::reject_repository_drivers(&git, root)
            .await
            .map_err(failed)?;
        let urls = text(
            &git,
            root,
            &["remote", "get-url", "--push", "--all", remote],
        )
        .await
        .map_err(failed)?;
        let destinations = urls.lines().collect::<Vec<_>>();
        if destinations.len() != 1 {
            return Err(failed(
                "publication requires exactly one reviewed push destination",
            ));
        }
        let slug = github_destination(destinations[0])?;
        if repository
            .trim_start_matches("github.com/")
            .to_ascii_lowercase()
            != slug
        {
            return Err(failed(
                "publication repository differs from the configured push destination",
            ));
        }
        let branch = evidence["branch"]
            .as_str()
            .ok_or_else(|| failed("missing integration branch"))?;
        let commit = evidence["combined_commit"]
            .as_str()
            .ok_or_else(|| failed("missing integration commit"))?;
        let target = evidence["target_branch"]
            .as_str()
            .ok_or_else(|| failed("missing target branch"))?;
        let definition = hash(
            &json!({"repository":slug,"remote":remote,"branch":branch,"commit":commit,"target":target,"title":title,"body":body,"source_sha256":evidence["source_sha256"]}),
        );
        let key = format!("integration_publication_{run}");
        let mut records = self.integration_record(&key)?;
        if let Some(first) = records.first() {
            if first.data["definition_sha256"] != definition {
                return Err(failed(
                    "publication destination/content changed; inspect the original attempt receipt",
                ));
            }
            if let Some(published) = records
                .iter()
                .find(|record| record.data["phase"] == "published")
            {
                return Ok(published.data.clone());
            }
            if let Some(found) = self
                .find_integration_pr(ctx, &slug, branch, commit, target)
                .await?
            {
                let receipt = json!({"phase":"published","operation_id":operation,"definition_sha256":definition,"repository":slug,"branch":branch,"commit":commit,"pull_request":found,"reconciled":true});
                self.integration_append(
                    &key,
                    records.last().map_or(-1, |event| event.seq),
                    receipt.clone(),
                )?;
                return Ok(receipt);
            }
            return Err(failed(
                "publication has a prior native intent without a matching remote PR; its outcome is unresolved and will not be repeated automatically",
            ));
        }
        self.integration_append(&key,-1,json!({"phase":"intent","operation_id":operation,"definition_sha256":definition,"repository":slug,"branch":branch,"commit":commit,"settled":false}))?;
        let push = cyber_workflow::solve::Check {
            argv: vec![
                "git".into(),
                "-c".into(),
                "core.hooksPath=/dev/null".into(),
                "-c".into(),
                "push.followTags=false".into(),
                "-c".into(),
                "protocol.ext.allow=never".into(),
                "-c".into(),
                "core.sshCommand=ssh".into(),
                "-c".into(),
                "credential.helper=".into(),
                "-c".into(),
                "credential.helper=!gh auth git-credential".into(),
                "-c".into(),
                format!("remote.{remote}.mirror=false"),
                "push".into(),
                "--porcelain".into(),
                "--no-verify".into(),
                destinations[0].into(),
                format!("{commit}:refs/heads/{branch}"),
            ],
            timeout_seconds: 120,
        };
        self.ensure_publication_live(args)?;
        let pushed = crate::tools::solve_check::publish(ctx, &push, Some(args)).await?;
        self.integration_append(&key,0,json!({"phase":"push","operation_id":operation,"definition_sha256":definition,"outcome":pushed}))?;
        if pushed["outcome"] != "passed" || pushed["settled"] != true {
            return Err(failed(
                "publication push did not settle successfully; inspect its durable receipt before any retry",
            ));
        }
        self.ensure_publication_live(args)?;
        if let Some(found) = self
            .find_integration_pr(ctx, &slug, branch, commit, target)
            .await?
        {
            let receipt = json!({"phase":"published","operation_id":operation,"definition_sha256":definition,"repository":slug,"branch":branch,"commit":commit,"pull_request":found,"reconciled":true});
            self.integration_append(&key, 1, receipt.clone())?;
            return Ok(receipt);
        }
        self.ensure_publication_live(args)?;
        self.integration_append(&key,1,json!({"phase":"create_intent","operation_id":operation,"definition_sha256":definition,"settled":false}))?;
        let temp = crate::sandboxing::session_tmp(ctx)?;
        let body_file = crate::worktrees::OwnedFile::create(
            &temp,
            "integration-pr-body",
            "md",
            body.as_bytes(),
        )
        .map_err(|_| failed("could not create private owner PR body"))?;
        let body_path = &body_file.0;
        let create = cyber_workflow::solve::Check {
            argv: vec![
                "gh".into(),
                "pr".into(),
                "create".into(),
                "--repo".into(),
                format!("github.com/{slug}"),
                "--base".into(),
                target.into(),
                "--head".into(),
                branch.into(),
                "--title".into(),
                title.into(),
                "--body-file".into(),
                body_path.display().to_string(),
            ],
            timeout_seconds: 120,
        };
        let result = crate::tools::solve_check::publish(ctx, &create, Some(args)).await?;
        self.integration_append(&key,2,json!({"phase":"create_settlement","operation_id":operation,"definition_sha256":definition,"outcome":result}))?;
        records = self.integration_record(&key)?;
        let Some(found) = self
            .find_integration_pr(ctx, &slug, branch, commit, target)
            .await?
        else {
            return Err(failed(
                "PR publication is unresolved; reconcile the exact repository/head before another publication",
            ));
        };
        let receipt = json!({"phase":"published","operation_id":operation,"definition_sha256":definition,"repository":slug,"branch":branch,"commit":commit,"pull_request":found,"reconciled":false});
        self.integration_append(
            &key,
            records.last().map_or(-1, |event| event.seq),
            receipt.clone(),
        )?;
        Ok(receipt)
    }

    pub(crate) fn ensure_publication_live(&self, args: &Value) -> Result<(), ToolError> {
        if let Some(id) = args["integration_admission"]["batch_id"].as_str() {
            let batch = self
                .runtime()
                .ok_or_else(|| failed("runtime unavailable"))?
                .integration_batch(id)
                .map_err(|error| failed(error.to_string()))?;
            if batch.status != "ready"
                || Some(u64::from(batch.attempt))
                    != args["integration_admission"]["attempt"].as_u64()
            {
                return Err(failed(
                    "publication attempt was cancelled or replaced before the next native phase",
                ));
            }
        }
        Ok(())
    }
    async fn find_integration_pr(
        &self,
        ctx: &Ctx<'_>,
        repository: &str,
        branch: &str,
        commit: &str,
        target: &str,
    ) -> Result<Option<Value>, ToolError> {
        let check = cyber_workflow::solve::Check {
            argv: vec![
                "gh".into(),
                "pr".into(),
                "list".into(),
                "--repo".into(),
                format!("github.com/{repository}"),
                "--head".into(),
                branch.into(),
                "--state".into(),
                "all".into(),
                "--json".into(),
                "number,url,headRefOid,headRefName,baseRefName,isCrossRepository".into(),
            ],
            timeout_seconds: 60,
        };
        let result = crate::tools::solve_check::publish(ctx, &check, None).await?;
        if result["outcome"] != "passed"
            || result["settled"] != true
            || result["log_truncated"] == true
        {
            return Err(failed("cannot establish publication state from GitHub"));
        }
        let rows: Vec<Value> = serde_json::from_str(result["log"].as_str().unwrap_or_default())
            .map_err(|_| failed("invalid GitHub PR reconciliation response"))?;
        matching_pr(rows, branch, commit, target)
    }
}

fn matching_pr(
    rows: Vec<Value>,
    branch: &str,
    commit: &str,
    target: &str,
) -> Result<Option<Value>, ToolError> {
    let matching = rows
        .into_iter()
        .filter(|row| {
            row["headRefName"] == branch
                && row["headRefOid"] == commit
                && row["baseRefName"] == target
                && row["isCrossRepository"] == false
        })
        .collect::<Vec<_>>();
    if matching.len() > 1 {
        return Err(failed(
            "multiple PRs match this integration attempt; owner reconciliation is required",
        ));
    }
    Ok(matching.into_iter().next())
}

fn github_destination(raw: &str) -> Result<String, ToolError> {
    let normalized = raw
        .strip_prefix("git@github.com:")
        .map(|path| format!("ssh://git@github.com/{path}"))
        .unwrap_or_else(|| raw.to_string());
    let url = url::Url::parse(&normalized)
        .map_err(|_| failed("publication requires a reviewed GitHub HTTPS/SSH repository"))?;
    if url.host_str() != Some("github.com")
        || !matches!(url.scheme(), "https" | "ssh")
        || url.port().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.password().is_some()
        || (url.scheme() == "https" && !url.username().is_empty())
    {
        return Err(failed(
            "publication destination must be credential-free standard GitHub HTTPS/SSH",
        ));
    }
    let path = url
        .path()
        .trim_matches('/')
        .trim_end_matches(".git")
        .to_ascii_lowercase();
    if path.split('/').count() != 2
        || path.split('/').any(|part| {
            part.is_empty()
                || part.starts_with('-')
                || !part
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"_.-".contains(&byte))
        })
    {
        return Err(failed("invalid GitHub repository identity"));
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn publication_destinations_are_exact_credential_free_github_repositories() {
        for url in [
            "https://github.com/Team/Repo.git",
            "git@github.com:Team/Repo.git",
            "ssh://git@github.com/Team/Repo.git",
        ] {
            assert_eq!(github_destination(url).unwrap(), "team/repo");
        }
        for url in [
            "https://token@github.com/team/repo",
            "https://github.com.evil.invalid/team/repo",
            "http://github.com/team/repo",
            "https://github.com:8443/team/repo",
            "https://github.com/team/repo?token=secret",
            "ext::helper",
            "https://github.com/team/repo/extra",
        ] {
            assert!(github_destination(url).is_err(), "{url}");
        }
    }
    #[test]
    fn lost_ack_reconciliation_requires_exact_same_repository_head_commit_and_target() {
        let exact = json!({"number":7,"headRefName":"cyber/integration","headRefOid":"abc","baseRefName":"main","isCrossRepository":false});
        assert_eq!(
            matching_pr(vec![exact.clone()], "cyber/integration", "abc", "main").unwrap(),
            Some(exact.clone())
        );
        assert!(
            matching_pr(
                vec![exact.clone()],
                "cyber/integration",
                "different",
                "main"
            )
            .unwrap()
            .is_none()
        );
        assert!(
            matching_pr(vec![exact.clone()], "cyber/integration", "abc", "release")
                .unwrap()
                .is_none()
        );
        let mut fork = exact.clone();
        fork["isCrossRepository"] = true.into();
        assert!(
            matching_pr(vec![fork], "cyber/integration", "abc", "main")
                .unwrap()
                .is_none()
        );
        assert!(
            matching_pr(
                vec![exact.clone(), exact],
                "cyber/integration",
                "abc",
                "main"
            )
            .is_err()
        );
    }
}
