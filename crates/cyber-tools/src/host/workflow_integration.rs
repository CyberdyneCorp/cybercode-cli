//! Trusted integration host protocol: source composition and checks never come
//! from model assertions, and contributed configuration never selects the policy.
use std::{
    collections::BTreeMap,
    ffi::OsString,
    path::{Path, PathBuf},
};

use cyber_core::worktrees::{GitExecution, Managed, Name, Repository, Settings};
use cyber_server::runtime::{Invocation, TurnContext};
use cyber_store::{Expected, NewEvent};
use cyber_workflow::integration::Manifest;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::{BuiltinHost, Ctx, PrepareError};
use crate::tools::{ToolError, failed};

const EVENT: &str = "workflow.integration.evidence.1";
struct CheckSource<'a> {
    run: &'a str,
    base: &'a str,
    source: &'a str,
    execute: bool,
}
type Configuration = (Value, BTreeMap<String, String>);
tokio::task_local! { static PINNED: (String, Configuration, PathBuf); }
tokio::task_local! { static NATIVE_GIT: bool; }

pub(crate) fn native_git_authority(ctx: &Ctx<'_>) -> bool {
    isolated(ctx) || NATIVE_GIT.try_with(|enabled| *enabled).unwrap_or(false)
}

pub(crate) fn configuration(ctx: &Ctx<'_>) -> Option<Result<Configuration, String>> {
    PINNED
        .try_with(|(session, config, _)| {
            (session == &ctx.inv.session_id).then(|| Ok(config.clone()))
        })
        .ok()
        .flatten()
}
pub(crate) fn temporary(ctx: &Ctx<'_>) -> Option<PathBuf> {
    PINNED
        .try_with(|(session, _, path)| (session == &ctx.inv.session_id).then(|| path.clone()))
        .ok()
        .flatten()
}
pub(crate) fn isolated(ctx: &Ctx<'_>) -> bool {
    configuration(ctx).is_some()
}
fn invalid(message: impl Into<String>) -> PrepareError {
    PrepareError::Unavailable(message.into())
}
pub(super) fn hash(value: &impl serde::Serialize) -> String {
    format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(value).expect("serializable host evidence"))
    )
}
fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

impl BuiltinHost {
    pub async fn integration_owner_operation(
        &self,
        owner: &TurnContext,
        run: &str,
        operation: &str,
        input: Value,
    ) -> Result<Value, String> {
        if !matches!(operation, "report" | "publish") {
            return Err("unsupported owner integration operation".into());
        }
        let mut invocation = super::command_skills::command_invocation(owner, "bash", input);
        invocation.input["operation"] = operation.into();
        let context = self.integration_context(&invocation).await?;
        self.integration_tool(&context, run)
            .await
            .map_err(|error| match error {
                ToolError::Failed(message) => message,
                ToolError::Aborted => {
                    "integration operation interrupted; inspect its durable receipt".into()
                }
            })
    }

    pub(super) async fn prepare_integration(
        &self,
        owner: &TurnContext,
        value: Value,
    ) -> Result<Value, PrepareError> {
        NATIVE_GIT
            .scope(true, self.prepare_integration_manifest(owner, value))
            .await
    }

    async fn prepare_integration_manifest(
        &self,
        owner: &TurnContext,
        value: Value,
    ) -> Result<Value, PrepareError> {
        let manifest: Manifest =
            serde_json::from_value(value).map_err(|error| invalid(error.to_string()))?;
        let order = manifest.validate().map_err(invalid)?;
        let inv = super::command_skills::command_invocation(
            owner,
            "workflow",
            json!({"name":"integrate"}),
        );
        let ctx = self.integration_context(&inv).await.map_err(invalid)?;
        let git = crate::worktrees::sandboxed_git(&ctx, None);
        super::workflow_solve::check_read(&ctx, &ctx.location)?;
        reject_repository_drivers(&git, Path::new(&owner.directory))
            .await
            .map_err(invalid)?;
        let base = self.solve_base(owner).await?;
        let directory = Path::new(
            base["directory"]
                .as_str()
                .ok_or_else(|| invalid("missing repository root"))?,
        );
        let branch = match &manifest.target_branch {
            Some(branch) => branch.trim_start_matches("refs/heads/").to_string(),
            None => text(
                &git,
                directory,
                &["symbolic-ref", "--quiet", "--short", "HEAD"],
            )
            .await
            .map_err(invalid)?
            .trim()
            .to_string(),
        };
        text(
            &git,
            directory,
            &["check-ref-format", &format!("refs/heads/{branch}")],
        )
        .await
        .map_err(invalid)?;
        let target = text(
            &git,
            directory,
            &[
                "rev-parse",
                "--verify",
                &format!("refs/heads/{branch}^{{commit}}"),
            ],
        )
        .await
        .map_err(invalid)?;
        if target.trim() != base["commit"].as_str().unwrap_or_default() {
            return Err(invalid(
                "integration target must match this clean checkout's HEAD; choose a clean target checkout",
            ));
        }
        if directory.join(".worktreeinclude").exists() {
            return Err(invalid(
                "integration preparation does not copy .worktreeinclude data; use a clean checkout without that file",
            ));
        }
        for contribution in &manifest.contributions {
            self.authorize_integration_hub(owner, contribution.hub.as_ref())
                .await?;
            validate_contribution(
                &git,
                directory,
                base["commit"].as_str().unwrap_or_default(),
                &contribution.commit,
            )
            .await
            .map_err(invalid)?;
        }
        let (config, _) = (self.opts.config)(Path::new(&owner.directory)).map_err(invalid)?;
        let settings = cyber_server::runtime::workflow_start::location(&config).map_err(invalid)?;
        let owner_model = self
            .runtime()
            .ok_or_else(|| invalid("runtime unavailable"))?
            .state(&owner.session_id)
            .await
            .map_err(|e| invalid(e.to_string()))?
            .info
            .model;
        let reviewer = self.opts.models.as_deref().and_then(|models| {
            super::workflow_solve::resolved_model(
                models,
                &settings.tiers,
                &owner_model,
                &manifest.reviewer_model,
            )
            .ok()
        });
        let mut args = serde_json::to_value(&manifest).map_err(|e| invalid(e.to_string()))?;
        args["integration_admission"] = json!({"base":base,"target_branch":branch,"order":order,"reviewer_model":reviewer,"manifest_sha256":hash(&manifest),"owner_configuration_sha256":hash(&config)});
        Ok(args)
    }

    pub(super) async fn authorize_integration_reference(
        &self,
        owner: &TurnContext,
        reference: Option<&cyber_workflow::integration::HubReference>,
    ) -> Result<(), String> {
        self.authorize_integration_hub(owner, reference)
            .await
            .map_err(|error| error.to_string())
    }

    async fn authorize_integration_hub(
        &self,
        owner: &TurnContext,
        reference: Option<&cyber_workflow::integration::HubReference>,
    ) -> Result<(), PrepareError> {
        let Some(reference) = reference else {
            return Ok(());
        };
        let service = self.collaboration.get().ok_or_else(|| {
            invalid("hub-linked integration requires live collaboration authorization")
        })?;
        service.call("integration.authorize".into(), json!({"session_id":owner.session_id,"project":reference.project,"task":reference.task,"agent":reference.agent})).await.map_err(|error| invalid(error.body.message))?;
        Ok(())
    }

    async fn integration_context<'a>(&'a self, inv: &'a Invocation) -> Result<Ctx<'a>, String> {
        Ok(Ctx {
            skill_paths: Default::default(),
            loaded_skill: Default::default(),
            compiler_feedback: Default::default(),
            hook_decision: None,
            denial: Default::default(),
            host: self,
            policy: self.policy(inv).await?,
            location: PathBuf::from(&inv.directory),
            inv,
            cancel: tokio_util::sync::CancellationToken::new(),
        })
    }

    pub(super) async fn integration_tool(
        &self,
        ctx: &Ctx<'_>,
        run_id: &str,
    ) -> Result<Value, ToolError> {
        let runtime = self
            .runtime()
            .ok_or_else(|| failed("runtime unavailable"))?;
        let run = runtime
            .workflow_run(run_id)
            .map_err(|e| failed(e.to_string()))?
            .ok_or_else(|| failed("integration run disappeared"))?;
        if run.origin != json!({"kind":"bundled","name":"integrate"})
            || !cyber_workflow::bundled::module("integrate")
                .is_some_and(|module| module.digest == run.digest)
            || run.session_id != ctx.inv.session_id
        {
            return Err(failed("untrusted integration host request"));
        }
        if run.revision != 1 {
            return Err(failed(
                "integration attempts are immutable; submit/run a fresh attempt instead of changing a workflow revision",
            ));
        }
        if let Some(batch) = run.args["integration_admission"]["batch_id"].as_str()
            && runtime
                .integration_batch(batch)
                .map_err(|error| failed(error.to_string()))?
                .status
                == "cancelled"
        {
            return Err(failed("integration batch was cancelled"));
        }
        let config = (self.opts.config)(Path::new(&run.directory)).map_err(failed)?;
        if run.args["integration_admission"]["owner_configuration_sha256"] != hash(&config.0) {
            return Err(failed(
                "owner configuration changed; submit a fresh integration batch",
            ));
        }
        let operation = ctx.inv.input["operation"]
            .as_str()
            .ok_or_else(|| failed("missing integration operation"))?;
        let mut invocation = ctx.inv.clone();
        if let Some(cap) = run
            .budget
            .as_ref()
            .and_then(|budget| budget.budget.max_wall_seconds)
        {
            let elapsed = runtime
                .workflow_run_summary(run_id)
                .map_err(|error| failed(error.to_string()))?
                .map_or(0, |summary| summary.elapsed_ms);
            let remaining = ((cap * 1000.0) as u64).saturating_sub(elapsed);
            invocation.input["deadline_ms"] = now().saturating_add(remaining as i64).into();
        }
        let context = Ctx {
            skill_paths: Default::default(),
            loaded_skill: Default::default(),
            compiler_feedback: Default::default(),
            hook_decision: None,
            denial: Default::default(),
            host: self,
            policy: ctx.policy.clone(),
            location: ctx.location.clone(),
            inv: &invocation,
            cancel: ctx.cancel.clone(),
        };
        let ctx = &context;
        let temporary = self.opts.temp_dir.join(format!("integration-{run_id}"));
        PINNED
            .scope((ctx.inv.session_id.clone(), config, temporary), async {
                match operation {
                    "prepare" => self.compose_integration(ctx, run_id, &run.args).await,
                    "verify" => self.check_integration(ctx, run_id, &run.args, true).await,
                    "report" => self.integration_report(ctx, run_id, &run.args).await,
                    "publish" => self.publish_integration(ctx, run_id, &run.args).await,
                    _ => Err(failed("unknown integration host operation")),
                }
            })
            .await
    }

    async fn integration_report(
        &self,
        ctx: &Ctx<'_>,
        run: &str,
        args: &Value,
    ) -> Result<Value, ToolError> {
        let mut report = match self.check_integration(ctx, run, args, false).await {
            Ok(report) => report,
            Err(error) => {
                json!({"status":"unknown","eligible":false,"run_id":run,"limitation":match error { ToolError::Failed(message)=>message,ToolError::Aborted=>"report interrupted".into() },"checks":self.recorded_integration_checks(run,args)?,"preparation":self.integration_record(&format!("integration_prepare_{run}"))?})
            }
        };
        report["cost"] = self.integration_cost(run)?;
        Ok(report)
    }
    fn integration_cost(&self, run: &str) -> Result<Value, ToolError> {
        let summary = self
            .runtime()
            .ok_or_else(|| failed("runtime unavailable"))?
            .workflow_run_summary(run)
            .map_err(|error| failed(error.to_string()))?
            .ok_or_else(|| failed("run summary unavailable"))?;
        let id = run.to_string();
        let (unpriced,complete)=self.opts.store.read(move|db| {
            db.query_row("SELECT EXISTS(SELECT 1 FROM session WHERE workflow_run_id=?1 AND (unpriced_steps>0 OR children_unpriced_steps>0)), NOT EXISTS(SELECT 1 FROM session WHERE workflow_run_id=?1 AND children_usage_complete=0) AND NOT EXISTS(SELECT 1 FROM event WHERE aggregate_id=?1 AND type='workflow.agent.settled.1' AND json_extract(data,'$.status')!='completed')",[id],|row|Ok((row.get::<_,bool>(0)?,row.get::<_,bool>(1)?))).map_err(Into::into)
        }).map_err(|error|failed(error.to_string()))?;
        let complete = complete && summary.agents.total == summary.agents.completed;
        Ok(
            json!({"actual_priced_usd":summary.cost_usd,"tokens":summary.tokens,"estimated_usd":null,"estimate_unavailable":"No provider-independent cost prediction is available","unpriced":unpriced,"usage_complete":complete,"cost_complete":complete && !unpriced}),
        )
    }

    fn recorded_integration_checks(
        &self,
        run: &str,
        args: &Value,
    ) -> Result<Vec<Value>, ToolError> {
        let checks: Vec<cyber_workflow::solve::Check> =
            serde_json::from_value(args["checks"].clone())
                .map_err(|error| failed(error.to_string()))?;
        checks
            .iter()
            .enumerate()
            .map(|(index, check)| {
                let key = format!("integration_check_{run}_{index}");
                let records = self.integration_record(&key)?;
                Ok(records
                    .get(1)
                    .or(records.first())
                    .map(|record| check_preview(&record.data))
                    .unwrap_or_else(
                        || json!({"id":key,"argv":check.argv,"outcome":"not_run","settled":false}),
                    ))
            })
            .collect()
    }
    pub(super) fn integration_record(
        &self,
        id: &str,
    ) -> Result<Vec<cyber_store::StoredEvent>, ToolError> {
        self.opts
            .store
            .read_events(id, -1, 500)
            .map(|page| page.events)
            .map_err(|e| failed(e.to_string()))
    }
    pub(super) fn integration_append(
        &self,
        id: &str,
        seq: i64,
        value: Value,
    ) -> Result<(), ToolError> {
        self.opts
            .store
            .append(id, Expected::Seq(seq), vec![NewEvent::new(EVENT, value)])
            .map(|_| ())
            .map_err(|e| failed(e.to_string()))
    }

    async fn compose_integration(
        &self,
        ctx: &Ctx<'_>,
        run: &str,
        args: &Value,
    ) -> Result<Value, ToolError> {
        let key = format!("integration_prepare_{run}");
        let existing = self.integration_record(&key)?;
        if !existing.is_empty() {
            return existing.get(1).map(|event| event.data.clone()).ok_or_else(|| failed("integration preparation has an unresolved native intent; reconcile it before a new attempt"));
        }
        let admission = &args["integration_admission"];
        let base = admission["base"]["commit"]
            .as_str()
            .ok_or_else(|| failed("missing integration base"))?;
        let root = PathBuf::from(
            admission["base"]["directory"]
                .as_str()
                .ok_or_else(|| failed("missing integration repository"))?,
        );
        let git = crate::worktrees::sandboxed_git(ctx, None);
        let target = text(
            &git,
            &root,
            &[
                "rev-parse",
                "--verify",
                &format!(
                    "refs/heads/{}^{{commit}}",
                    admission["target_branch"].as_str().unwrap_or_default()
                ),
            ],
        )
        .await
        .map_err(failed)?;
        if target.trim() != base {
            return Err(failed(
                "integration target advanced; submit a fresh attempt",
            ));
        }
        let repo = Repository::discover(&git, &root)
            .await
            .map_err(|e| failed(e.to_string()))?;
        let name = Name::parse(&format!(
            "integration-{}",
            run.trim_start_matches("run_").to_ascii_lowercase()
        ))
        .map_err(failed)?;
        crate::worktrees::authorize_worktree(
            ctx,
            true,
            crate::permissions::Request {
                tool: Some("worktree".into()),
                action: "worktree".into(),
                resources: vec![name.as_str().into()],
                ..Default::default()
            },
            json!({"operation":"integration_create","run_id":run,"name":name.as_str()}),
        )
        .await?;
        let settings = Settings {
            base: base.into(),
            branch_prefix: "cyber/integrations/".into(),
            ..Default::default()
        };
        let data = self
            .opts
            .tool_output_dir
            .parent()
            .ok_or_else(|| failed("missing managed data directory"))?;
        let project = format!("prj_{}", &hash(&repo.common_dir)[..16]);
        self.integration_append(&key,-1,json!({"run_id":run,"base":base,"manifest_sha256":admission["manifest_sha256"],"started_ms":now(),"settled":false}))?;
        let target = repo
            .target(&settings, data, &project, &name)
            .map_err(|e| failed(e.to_string()))?;
        let parent = target
            .parent()
            .ok_or_else(|| failed("missing managed storage parent"))?
            .to_path_buf();
        let creation_git =
            crate::worktrees::sandboxed_git(ctx, Some(vec![parent, repo.common_dir.clone()]));
        let managed = repo
            .create(&creation_git, &settings, data, &project, &name)
            .await
            .map_err(|e| {
                failed(format!(
                    "integration worktree preparation is unresolved: {e}"
                ))
            })?;
        let lease = repo
            .claim_lsp(
                &creation_git,
                &managed,
                &format!("lsp_integration_{}", run.trim_start_matches("run_")),
            )
            .await
            .map_err(|error| failed(error.to_string()))?;
        let writable = vec![managed.path.clone(), repo.common_dir.clone()];
        let git = crate::worktrees::sandboxed_git(ctx, Some(writable));
        let contributions = args["contributions"]
            .as_array()
            .ok_or_else(|| failed("missing contributions"))?;
        let order = admission["order"]
            .as_array()
            .ok_or_else(|| failed("missing contribution order"))?;
        let mut combined = base.to_string();
        let mut prefixes = Vec::new();
        for index in order {
            if ctx.cancel.is_cancelled() {
                return Err(ToolError::Aborted);
            }
            let contribution = contributions
                .get(
                    index
                        .as_u64()
                        .ok_or_else(|| failed("invalid contribution order"))?
                        as usize,
                )
                .ok_or_else(|| failed("invalid contribution order"))?;
            let commit = contribution["commit"]
                .as_str()
                .ok_or_else(|| failed("missing contribution commit"))?;
            let output = git
                .run(
                    &root,
                    &argv(&[
                        "merge-tree",
                        "--write-tree",
                        "--name-only",
                        &combined,
                        commit,
                    ]),
                )
                .await
                .map_err(|e| failed(e.to_string()))?;
            if !output.status.success() {
                let report = json!({"status":"conflicted","eligible":false,"run_id":run,"base":base,"managed":managed,"prefixes":prefixes,"conflict_contribution":contribution["label"],"conflict_output":String::from_utf8_lossy(&output.stdout).chars().take(16384).collect::<String>(),"conflict_output_truncated":output.stdout.len()>16384,"settled":true});
                self.integration_append(&key, 0, report.clone())?;
                lease.settle().map_err(|error| failed(error.to_string()))?;
                return Ok(report);
            }
            let output = String::from_utf8(output.stdout)
                .map_err(|_| failed("non-Unicode merge-tree result"))?;
            let tree = output.lines().next().unwrap_or_default();
            if !cyber_workflow::integration::commit(tree) {
                return Err(failed("invalid merge tree identity"));
            }
            combined = text(
                &git,
                &root,
                &[
                    "-c",
                    "user.name=Cyber Code",
                    "-c",
                    "user.email=integration@localhost",
                    "-c",
                    "commit.gpgsign=false",
                    "commit-tree",
                    tree,
                    "-p",
                    &combined,
                    "-p",
                    commit,
                    "-m",
                    "Integrate verified contribution",
                ],
            )
            .await
            .map_err(failed)?
            .trim()
            .to_string();
            prefixes.push(json!({"label":contribution["label"],"contribution_commit":commit,"combined_commit":combined}));
        }
        text(&git, &managed.path, &["reset", "--hard", &combined])
            .await
            .map_err(failed)?;
        let report = json!({"status":"checking","eligible":false,"run_id":run,"base":base,"combined_commit":combined,"managed":managed,"prefixes":prefixes,"settled":true});
        self.integration_append(&key, 0, report.clone())?;
        lease.settle().map_err(|error| failed(error.to_string()))?;
        Ok(report)
    }

    pub(super) async fn check_integration(
        &self,
        ctx: &Ctx<'_>,
        run: &str,
        args: &Value,
        execute: bool,
    ) -> Result<Value, ToolError> {
        let prepared = self
            .integration_record(&format!("integration_prepare_{run}"))?
            .get(1)
            .map(|event| event.data.clone())
            .ok_or_else(|| failed("integration preparation is unresolved"))?;
        if prepared["status"] == "conflicted" {
            return Ok(prepared);
        }
        let managed: Managed = serde_json::from_value(prepared["managed"].clone())
            .map_err(|e| failed(e.to_string()))?;
        let lease_repository = Repository {
            root: managed.path.clone(),
            common_dir: managed.common_dir.clone(),
        };
        let lease_git = crate::worktrees::sandboxed_git(ctx, Some(Vec::new()));
        let lease = lease_repository
            .claim_lsp(
                &lease_git,
                &managed,
                &format!("lsp_integration_{}", run.trim_start_matches("run_")),
            )
            .await
            .map_err(|error| failed(error.to_string()))?;
        let base = prepared["base"]
            .as_str()
            .ok_or_else(|| failed("missing base"))?;
        let mut policy = ctx.policy.clone();
        policy.location = managed.path.clone();
        let check_ctx = Ctx {
            skill_paths: Default::default(),
            loaded_skill: Default::default(),
            compiler_feedback: Default::default(),
            hook_decision: None,
            denial: Default::default(),
            host: self,
            policy,
            location: managed.path.clone(),
            inv: ctx.inv,
            cancel: ctx.cancel.clone(),
        };
        let snapshot = super::review::solve_snapshot(&check_ctx, base)
            .await
            .map_err(|e| failed(e.to_string()))?;
        if !snapshot.limitations.is_empty() {
            return Err(failed("combined source snapshot is incomplete"));
        }
        let source = snapshot
            .source_sha256
            .as_deref()
            .ok_or_else(|| failed("combined source fingerprint unavailable"))?;
        let identity = format!("integration_source_{run}");
        let stored = self.integration_record(&identity)?;
        if let Some(first) = stored.first() {
            if first.data["source_sha256"] != source {
                lease.settle().map_err(|error| failed(error.to_string()))?;
                return Ok(
                    json!({"status":"stale","eligible":false,"limitation":"combined source changed after submission","source_sha256":source,"base":base,"combined_commit":prepared["combined_commit"],"verified_source_sha256":first.data["source_sha256"],"patch":first.data["patch"],"patch_sha256":first.data["patch_sha256"],"checks":self.recorded_integration_checks(run,args)?}),
                );
            }
        } else {
            self.integration_append(&identity,-1,json!({"run_id":run,"source_sha256":source,"combined_commit":prepared["combined_commit"],"manifest_sha256":args["integration_admission"]["manifest_sha256"],"patch_sha256":snapshot.patch_sha256,"patch":snapshot.patch()}))?;
        }
        let checks: Vec<cyber_workflow::solve::Check> =
            serde_json::from_value(args["checks"].clone()).map_err(|e| failed(e.to_string()))?;
        let evidence = self
            .integration_checks(
                &check_ctx,
                &checks,
                CheckSource {
                    run,
                    base,
                    source,
                    execute,
                },
            )
            .await?;
        let after = super::review::solve_snapshot(&check_ctx, base)
            .await
            .map_err(|e| failed(e.to_string()))?;
        let git = crate::worktrees::sandboxed_git(ctx, None);
        let root = Path::new(
            args["integration_admission"]["base"]["directory"]
                .as_str()
                .ok_or_else(|| failed("missing owner root"))?,
        );
        let tip = text(
            &git,
            root,
            &[
                "rev-parse",
                "--verify",
                &format!(
                    "refs/heads/{}^{{commit}}",
                    args["integration_admission"]["target_branch"]
                        .as_str()
                        .unwrap_or_default()
                ),
            ],
        )
        .await
        .map_err(failed)?;
        let local_tip = text(
            &git,
            &managed.path,
            &[
                "rev-parse",
                "--verify",
                &format!("refs/heads/{}^{{commit}}", managed.branch),
            ],
        )
        .await
        .map_err(failed)?;
        let stale = tip.trim() != base
            || after.source_sha256.as_deref() != Some(source)
            || local_tip.trim() != prepared["combined_commit"].as_str().unwrap_or_default();
        let eligible = !stale
            && evidence.len() == checks.len()
            && evidence.iter().all(|record| {
                record["outcome"] == "passed"
                    && record["settled"] == true
                    && record["source_unchanged"] == true
            });
        if evidence.iter().all(|record| record["settled"] == true) {
            lease.settle().map_err(|error| failed(error.to_string()))?;
        }
        let evidence: Vec<Value> = evidence.iter().map(check_preview).collect();
        Ok(
            json!({"status":if stale{"stale"}else if eligible{"ready"}else{"failed"},"eligible":eligible,"run_id":run,"base":base,"target_branch":args["integration_admission"]["target_branch"],"current_target_commit":tip.trim(),"combined_commit":prepared["combined_commit"],"branch":managed.branch,"worktree":managed.path,"source_sha256":source,"patch_sha256":snapshot.patch_sha256,"patch":snapshot.patch(),"prefixes":prepared["prefixes"],"contributions":args["contributions"],"checks":evidence,"limitations":["environment identity uses local toolchain and configured sandbox; no reproducible-container claim"],"merged":false,"published":false}),
        )
    }
    async fn integration_checks(
        &self,
        check_ctx: &Ctx<'_>,
        checks: &[cyber_workflow::solve::Check],
        input: CheckSource<'_>,
    ) -> Result<Vec<Value>, ToolError> {
        let CheckSource {
            run,
            base,
            source,
            execute,
        } = input;
        let mut evidence = Vec::new();
        for (index, check) in checks.iter().enumerate() {
            let key = format!("integration_check_{run}_{index}");
            let records = self.integration_record(&key)?;
            if let Some(record) = records.get(1) {
                if record.data["source_sha256"] != source
                    || record.data["definition_sha256"] != hash(check)
                    || record.data["run_id"] != run
                {
                    return Err(failed(
                        "integration check evidence does not match this immutable attempt",
                    ));
                }
                evidence.push(record.data.clone());
                continue;
            }
            if !records.is_empty() || !execute {
                evidence
                    .push(json!({"id":key,"argv":check.argv,"outcome":"unknown","settled":false}));
                continue;
            }
            if evidence.iter().any(|record| record["settled"] != true) {
                evidence
                    .push(json!({"id":key,"argv":check.argv,"outcome":"not_run","settled":false}));
                continue;
            }
            let current = super::review::solve_snapshot(check_ctx, base)
                .await
                .map_err(|e| failed(e.to_string()))?;
            if current.source_sha256.as_deref() != Some(source) {
                evidence.push(json!({"id":key,"argv":check.argv,"outcome":"not_run","settled":true,"limitation":"source changed before check"}));
                continue;
            }
            let mut record = json!({"id":key,"run_id":run,"source_sha256":source,"definition_sha256":hash(check),"argv":check.argv,"started_ms":now(),"outcome":"unknown","settled":false});
            self.integration_append(&key, -1, record.clone())?;
            let outcome = match crate::tools::solve_check::run(check_ctx, check).await {
                Ok(value) => value,
                Err(ToolError::Aborted) => json!({"outcome":"cancelled","settled":false}),
                Err(ToolError::Failed(error)) => {
                    json!({"outcome":"blocked","settled":true,"limitation":error})
                }
            };
            if let Some(values) = outcome.as_object() {
                for (key, value) in values {
                    record[key] = value.clone();
                }
            }
            let after = super::review::solve_snapshot(check_ctx, base).await;
            let unchanged = after
                .as_ref()
                .is_ok_and(|snapshot| snapshot.source_sha256.as_deref() == Some(source));
            record["source_unchanged"] = unchanged.into();
            record["ended_ms"] = now().into();
            if !unchanged {
                record["outcome"] = "failed".into();
                record["limitation"] = "verification changed the submitted source".into();
            }
            self.integration_append(&key, 0, record.clone())?;
            evidence.push(record);
        }
        Ok(evidence)
    }
}

fn check_preview(record: &Value) -> Value {
    let mut result = record.clone();
    if let Some(log) = record["log"].as_str().filter(|log| log.len() > 16_384) {
        let mut end = 16_384;
        while !log.is_char_boundary(end) {
            end -= 1;
        }
        result["log"] = log[..end].into();
        result["log_truncated"] = true.into();
        result["full_evidence_aggregate"] = record["id"].clone();
    }
    result
}

fn argv(args: &[&str]) -> Vec<OsString> {
    args.iter().map(OsString::from).collect()
}
pub(super) async fn text(
    git: &dyn GitExecution,
    directory: &Path,
    args: &[&str],
) -> Result<String, String> {
    let output = git
        .run(directory, &argv(args))
        .await
        .map_err(|e| e.to_string())?;
    if !output.status.success() {
        return Err(format!(
            "integration Git operation {} failed (exit {:?})",
            args.first().unwrap_or(&""),
            output.status.code()
        ));
    }
    String::from_utf8(output.stdout).map_err(|_| "Git metadata is not Unicode".into())
}
pub(super) async fn reject_repository_drivers(
    git: &dyn GitExecution,
    root: &Path,
) -> Result<(), String> {
    let output = git
        .run(
            root,
            &argv(&[
                "config",
                "--local",
                "--no-includes",
                "--name-only",
                "--get-regexp",
                "^(filter\\.|merge\\..*\\.driver$|include\\.|includeIf\\.|includeif\\.|url\\.|extensions\\.(partialclone|worktreeconfig)$|remote\\..*\\.promisor$)",
            ]),
        )
        .await
        .map_err(|e| e.to_string())?;
    if output.status.success() && !output.stdout.is_empty() {
        return Err("integration refuses repository-local external filters, merge drivers, URL rewrites, partial-clone fetchers, worktree-specific or included configuration".into());
    }
    if output.status.code() != Some(1) && !output.status.success() {
        return Err("cannot inspect repository-local execution settings".into());
    }
    Ok(())
}
async fn validate_contribution(
    git: &dyn GitExecution,
    root: &Path,
    base: &str,
    commit: &str,
) -> Result<(), String> {
    text(
        git,
        root,
        &["rev-parse", "--verify", &format!("{commit}^{{commit}}")],
    )
    .await?;
    text(git, root, &["merge-base", base, commit]).await?;
    let tree = text(git, root, &["ls-tree", "-r", commit]).await?;
    if tree.lines().any(|entry| entry.starts_with("160000 ")) {
        return Err("integration does not execute or materialize submodule contributions".into());
    }
    let paths = text(
        git,
        root,
        &[
            "diff-tree",
            "--no-commit-id",
            "--name-only",
            "-r",
            "-z",
            base,
            commit,
        ],
    )
    .await?;
    if paths.split('\0').any(protected_configuration) {
        return Err("contributions cannot replace verification policy, hooks, plugins or worktree include configuration".into());
    }
    Ok(())
}
fn protected_configuration(path: &str) -> bool {
    let path = path.to_ascii_lowercase();
    matches!(
        path.as_str(),
        "cyber.json" | "cyber.jsonc" | ".cyber" | ".git" | ".gitmodules" | ".worktreeinclude"
    ) || path.starts_with(".cyber/")
        || path.starts_with(".git/")
}
