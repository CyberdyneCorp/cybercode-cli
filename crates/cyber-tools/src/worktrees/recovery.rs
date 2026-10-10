//! Explicit parent-authorized recovery of settled child setup failures.
use super::*;
use cyber_server::worktrees::{ChildSetupInspection, SetupRecoveryRequest};

struct RecoveryContext {
    inv: Invocation,
    child: SessionInfo,
    managed: Managed,
    recipe: SetupRecipe,
    repository: Repository,
    source_read_only: bool,
}

impl BuiltinHost {
    async fn child_setup_context(
        &self,
        parent: &str,
        reference: &str,
        cancel: CancellationToken,
    ) -> io::Result<RecoveryContext> {
        let runtime = self
            .runtime()
            .ok_or_else(|| io::Error::other("Runtime is not attached"))?;
        let child = runtime
            .resolve_subagent(parent, reference)
            .await
            .map_err(io::Error::other)?;
        let state = runtime.state(&child.id).await.map_err(io::Error::other)?;
        let managed = state
            .child_worktree()
            .cloned()
            .ok_or_else(|| io::Error::other("Child has no isolated checkout binding"))?;
        let source = runtime.state(parent).await.map_err(io::Error::other)?.info;
        let call_id = cyber_core::ids::new_id("call");
        let inv = Invocation {
            registration: None,
            session_id: source.id,
            directory: source.directory,
            agent: source.agent,
            mode: source.mode,
            rules: source.rules,
            message_id: String::new(),
            operation_key: call_id.clone(),
            call_id,
            name: "worktree".into(),
            input: serde_json::Value::Null,
            attempt: 1,
            asker: Asker::detached(),
        };
        let ctx = Ctx {
            skill_paths: Default::default(),
            compiler_feedback: Default::default(),
            hook_decision: None,
            host: self,
            inv: &inv,
            policy: self.policy(&inv).await.map_err(io::Error::other)?,
            location: Path::new(&inv.directory).canonicalize()?,
            cancel,
        };
        authorize_owned_worktree(
            &ctx,
            &Request {
                action: "worktree".into(),
                resources: vec![managed.name.clone()],
                read_only: true,
                ..Default::default()
            },
        )
        .map_err(tool_error)?;
        let (config, sources) = (self.opts.config)(&ctx.location).map_err(io::Error::other)?;
        let sandbox = cyber_sandbox::SandboxConfig::resolve(
            &config,
            &sources,
            self.opts.sandbox_policy.as_deref(),
            &self.opts.home,
        );
        let source_read_only = sandbox.policy == cyber_sandbox::Policy::ReadOnly;
        let recipe = SetupRecipe {
            settings: Settings::from_config(&config).map_err(io::Error::other)?,
            credentials: crate::sandboxing::credential_env_names(&config),
        };
        let execution = GitPort {
            ctx: &ctx,
            writable: Some(Vec::new()),
            credentials: &[],
        };
        let repository = Repository::discover(&execution, &ctx.location).await?;
        if repository.common_dir != managed.common_dir {
            return Err(io::Error::other(
                "Source repository differs from child ownership",
            ));
        }
        Ok(RecoveryContext {
            inv,
            child,
            managed,
            recipe,
            repository,
            source_read_only,
        })
    }

    pub async fn inspect_child_worktree_setup(
        &self,
        parent: &str,
        reference: &str,
        cancel: CancellationToken,
    ) -> io::Result<ChildSetupInspection> {
        let runtime = self
            .runtime()
            .ok_or_else(|| io::Error::other("Runtime is not attached"))?;
        runtime
            .own_worktree_setup(cancel.clone(), async {
                let context = self.child_setup_context(parent, reference, cancel).await?;
                let state = runtime
                    .state(&context.child.id)
                    .await
                    .map_err(io::Error::other)?;
                let journal = SetupJournal::new(
                    Arc::clone(&self.opts.store),
                    &context.managed,
                    &context.recipe.settings.setup,
                    &context.child.id,
                )?
                .snapshot()?;
                Ok(ChildSetupInspection {
                    session_id: context.child.id,
                    worktree_id: context.managed.id,
                    setup_pending: state.child_worktree_setup_pending(),
                    journal,
                })
            })
            .await
    }

    /// The client must explicitly submit the journal revision/digest it reviewed.
    /// Unknown activity/command outcomes remain fenced; no inference starts here.
    pub async fn recover_child_worktree_setup(
        &self,
        parent: &str,
        reference: &str,
        review: SetupRecoveryRequest,
        cancel: CancellationToken,
    ) -> io::Result<WorktreeSession> {
        Box::pin(self.recover_child_setup_owned(parent, reference, review, cancel)).await
    }

    async fn recover_child_setup_owned(
        &self,
        parent: &str,
        reference: &str,
        review: SetupRecoveryRequest,
        cancel: CancellationToken,
    ) -> io::Result<WorktreeSession> {
        if review.reason.trim().is_empty() || review.reason.len() > 1024 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Recovery reason must contain 1-1024 bytes",
            ));
        }
        let runtime = self
            .runtime()
            .ok_or_else(|| io::Error::other("Runtime is not attached"))?;
        let context = Box::pin(self.child_setup_context(parent, reference, cancel.clone())).await?;
        if context.source_read_only && !context.recipe.settings.setup.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "Read-only source sandbox refuses setup recovery",
            ));
        }
        let owner = runtime
            .claim_child_execution(parent, &context.child.id)
            .map_err(io::Error::other)?;
        runtime
            .validate_child_setup_recovery(&owner, &context.managed)
            .await
            .map_err(io::Error::other)?;
        let mut child_inv = context.inv.clone();
        child_inv.session_id = context.child.id.clone();
        child_inv.directory = context.child.directory.clone();
        child_inv.agent = context.child.agent.clone();
        child_inv.mode = context.child.mode.clone();
        child_inv.rules = context.child.rules.clone();
        let child_ctx = Ctx {
            skill_paths: Default::default(),
            compiler_feedback: Default::default(),
            hook_decision: None,
            host: self,
            inv: &child_inv,
            policy: self.policy(&child_inv).await.map_err(io::Error::other)?,
            location: context.managed.path.clone(),
            cancel: cancel.clone(),
        };
        authorize_owned_worktree(
            &child_ctx,
            &Request {
                action: "worktree".into(),
                resources: vec![context.managed.name.clone()],
                read_only: context.recipe.settings.setup.is_empty(),
                mutates: vec![context.managed.path.clone()],
                ..Default::default()
            },
        )
        .map_err(tool_error)?;
        let mut setup_inv = context.inv;
        setup_inv.session_id = context.child.id.clone();
        setup_inv.directory = context.child.directory.clone();
        let setup = owner
            .run(
                cancel.clone(),
                self.setup_worktree_session_authorized(
                    &setup_inv,
                    cancel,
                    &context.repository,
                    &context.managed,
                    SetupAdmission {
                        explicit: true,
                        user_requested: true,
                        recipe: Some(&context.recipe),
                        recovery: Some(&review),
                    },
                ),
            )
            .await?;
        if matches!(setup, SetupOutcome::Completed) {
            runtime
                .complete_child_worktree_setup(&context.child.id, &context.managed)
                .await
                .map_err(io::Error::other)?;
        }
        Ok(WorktreeSession {
            session: context.child,
            managed: context.managed,
            setup: Ok(setup),
        })
    }
}
