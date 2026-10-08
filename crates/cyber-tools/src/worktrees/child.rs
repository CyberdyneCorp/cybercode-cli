//! Child-owned checkouts share native creation, setup, activity and removal boundaries.
use super::*;
use cyber_core::worktrees::{Branch, CheckoutActivity, Cleanup};
use cyber_server::runtime::CreateSession;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::sync::Weak;

#[derive(Clone)]
pub(crate) struct ChildWorktree {
    child_id: String,
    host: Weak<BuiltinHost>,
    invocation: Invocation,
    repository: Repository,
    managed: Managed,
    cleanup: Cleanup,
}

impl BuiltinHost {
    pub(crate) async fn create_child_worktree(
        &self,
        inv: &Invocation,
        cancel: CancellationToken,
        request: CreateSession,
        user_requested: bool,
    ) -> io::Result<ChildWorktree> {
        let runtime = self
            .runtime()
            .ok_or_else(|| io::Error::other("Runtime is not attached"))?;
        let parent = request
            .parent_id
            .as_deref()
            .filter(|parent| *parent == inv.session_id)
            .ok_or_else(|| io::Error::other("Isolated child must belong to its caller"))?;
        let id = request
            .id
            .as_deref()
            .ok_or_else(|| io::Error::other("Isolated child requires an owned Session ID"))?;
        let child_name = request
            .subagent_name
            .as_deref()
            .ok_or_else(|| io::Error::other("Isolated child requires a reserved name"))?;
        let name = Name::parse(&format!(
            "child-{}",
            id.strip_prefix("ses_").unwrap_or(id).to_ascii_lowercase()
        ))
        .map_err(io::Error::other)?;
        let parent_short = format!("{:x}", Sha256::digest(parent.as_bytes()));
        let branch = format!("cyber/{}/{child_name}", &parent_short[..12]);
        let worktree_request = WorktreeSessionRequest {
            project_id: cyber_core::project::identify(Path::new(&inv.directory)).id,
            data: self
                .opts
                .tool_output_dir
                .parent()
                .ok_or_else(|| io::Error::other("Missing data directory"))?
                .to_owned(),
            name,
            session: request,
        };
        let lock = self.path_lock(
            &worktree_request
                .data
                .join("child-lifecycle")
                .join(&worktree_request.project_id),
        );
        let _lifecycle = tokio::select! {
            biased;
            _ = cancel.cancelled() => return Err(io::Error::new(io::ErrorKind::Interrupted, "Isolated creation cancelled")),
            guard = lock.lock() => guard,
        };
        let (repository, managed, recipe) = runtime
            .own_worktree_setup(
                cancel.child_token(),
                self.create_child_checkout(
                    inv,
                    cancel.child_token(),
                    &worktree_request,
                    &branch,
                    user_requested,
                    None,
                ),
            )
            .await?;
        if cancel.is_cancelled() {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "Isolated creation cancelled",
            ));
        }
        let mut request = worktree_request.session;
        request.directory = managed.path.display().to_string();
        request.worktree_id = Some(managed.id.clone());
        request.child_worktree = Some(managed.clone());
        request.child_worktree_setup_pending = true;
        let session = runtime
            .create_session(request)
            .await
            .map_err(io::Error::other)?;
        let mut setup_inv = inv.clone();
        setup_inv.session_id = session.id.clone();
        setup_inv.directory = session.directory;
        let setup = self
            .setup_worktree_session_authorized(
                &setup_inv,
                cancel,
                &repository,
                &managed,
                SetupAdmission {
                    user_requested,
                    explicit: true,
                    recipe: Some(&recipe),
                    recovery: None,
                },
            )
            .await?;
        if let SetupOutcome::Failed { index, code } = setup {
            return Err(io::Error::other(format!(
                "Isolated child setup command {index} failed with exit code {code:?}; checkout retained at {}",
                managed.path.display()
            )));
        }
        runtime
            .complete_child_worktree_setup(&session.id, &managed)
            .await
            .map_err(io::Error::other)?;
        Ok(ChildWorktree {
            child_id: session.id,
            host: self.weak.clone(),
            invocation: inv.clone(),
            repository,
            managed,
            cleanup: recipe.settings.cleanup,
        })
    }

    async fn create_child_checkout(
        &self,
        inv: &Invocation,
        cancel: CancellationToken,
        request: &WorktreeSessionRequest,
        branch: &str,
        user_requested: bool,
        removed: Option<&Managed>,
    ) -> io::Result<(Repository, Managed, SetupRecipe)> {
        let ctx = Ctx {
            hook_decision: None,
            host: self,
            inv,
            policy: self.policy(inv).await.map_err(io::Error::other)?,
            location: Path::new(&inv.directory).canonicalize()?,
            cancel,
        };
        let (config, sources) = (self.opts.config)(&ctx.location).map_err(io::Error::other)?;
        let sandbox = cyber_sandbox::SandboxConfig::resolve(
            &config,
            &sources,
            self.opts.sandbox_policy.as_deref(),
            &self.opts.home,
        );
        if sandbox.policy == cyber_sandbox::Policy::ReadOnly {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "Read-only sandbox refuses isolated creation",
            ));
        }
        let admission = Request {
            action: "worktree".into(),
            resources: vec![request.name.as_str().into()],
            ..Default::default()
        };
        if user_requested {
            authorize_owned_worktree(&ctx, &admission).map_err(tool_error)?;
        } else {
            authorize_worktree(
                &ctx,
                false,
                admission,
                json!({"operation":"create","branch":branch}),
            )
            .await
            .map_err(tool_error)?;
        }
        let current = self.policy(inv).await.map_err(io::Error::other)?;
        if let crate::permissions::Decision::Deny(reason) = current.user_delegation(&Request {
            action: "worktree".into(),
            resources: vec![request.name.as_str().into()],
            ..Default::default()
        }) {
            return Err(io::Error::new(io::ErrorKind::PermissionDenied, reason));
        }
        let settings = Settings::from_config(&config).map_err(io::Error::other)?;
        let discovery = GitPort {
            ctx: &ctx,
            writable: Some(vec![]),
            credentials: &[],
        };
        let repository = Repository::discover(&discovery, &ctx.location).await?;
        let target =
            repository.target(&settings, &request.data, &request.project_id, &request.name)?;
        let execution = GitPort {
            ctx: &ctx,
            writable: Some(vec![repository.common_dir.clone(), target]),
            credentials: &[],
        };
        let managed = retry_repository_busy(&ctx.cancel, || async {
            let runtime = self
                .runtime()
                .ok_or_else(|| io::Error::other("Missing runtime"))?;
            let authority = match &request.session.admission_authority {
                Some(authority) => authority.clone(),
                None => runtime
                    .capture_child_admission(&inv.session_id)
                    .map_err(io::Error::other)?,
            };
            authority
                .verify(&runtime, &inv.session_id)
                .map_err(io::Error::other)?;
            if let Some(removed) = removed {
                repository
                    .recreate_removed(
                        &execution,
                        &CheckoutActivity,
                        &settings,
                        &request.data,
                        &request.project_id,
                        removed,
                    )
                    .await
            } else {
                repository
                    .create_on_branch(
                        &execution,
                        &settings,
                        &request.data,
                        &request.project_id,
                        Branch {
                            name: &request.name,
                            reference: branch,
                        },
                    )
                    .await
            }
        })
        .await?;
        Ok((
            repository,
            managed,
            SetupRecipe {
                settings,
                credentials: crate::sandboxing::credential_env_names(&config),
            },
        ))
    }
}

impl BuiltinHost {
    pub(crate) async fn resume_child_worktree(
        &self,
        inv: &Invocation,
        cancel: CancellationToken,
        owner: &cyber_server::runtime::ChildExecution,
        child: &cyber_server::runtime::SessionInfo,
        removed: Managed,
    ) -> io::Result<ChildWorktree> {
        self.resume_child_worktree_authorized(inv, cancel, owner, child, removed, false)
            .await
    }

    pub(crate) async fn resume_child_worktree_authorized(
        &self,
        inv: &Invocation,
        cancel: CancellationToken,
        owner: &cyber_server::runtime::ChildExecution,
        child: &cyber_server::runtime::SessionInfo,
        removed: Managed,
        user_requested: bool,
    ) -> io::Result<ChildWorktree> {
        let runtime = self
            .runtime()
            .ok_or_else(|| io::Error::other("Runtime stopped"))?;
        owner.verify(&runtime).map_err(io::Error::other)?;
        if runtime
            .state(&child.id)
            .await
            .map_err(io::Error::other)?
            .child_worktree_setup_pending()
        {
            return Err(io::Error::other(
                "Isolated child setup is incomplete; recovery is required",
            ));
        }
        if removed.path.try_exists()? {
            return ChildWorktree::retained(self, inv, &child.id, removed).await;
        }
        let request = WorktreeSessionRequest {
            project_id: cyber_core::project::identify(Path::new(&inv.directory)).id,
            data: self
                .opts
                .tool_output_dir
                .parent()
                .ok_or_else(|| io::Error::other("Missing data directory"))?
                .to_owned(),
            name: Name::parse(&removed.name).map_err(io::Error::other)?,
            session: CreateSession::default(),
        };
        let lock = self.path_lock(
            &request
                .data
                .join("child-lifecycle")
                .join(&request.project_id),
        );
        let _lifecycle = tokio::select! { biased;
            _ = cancel.cancelled() => return Err(io::Error::new(io::ErrorKind::Interrupted, "Isolated recreation cancelled")),
            guard = lock.lock() => guard,
        };
        let (repository, managed, recipe) = runtime
            .own_worktree_setup(
                cancel.clone(),
                self.create_child_checkout(
                    inv,
                    cancel.clone(),
                    &request,
                    &removed.branch,
                    user_requested,
                    Some(&removed),
                ),
            )
            .await?;
        runtime
            .rebind_child_worktree(owner, &removed, &managed)
            .await
            .map_err(io::Error::other)?;
        self.clear_session_reads(&child.id);
        let mut setup_inv = inv.clone();
        setup_inv.session_id = child.id.clone();
        setup_inv.directory = managed.path.display().to_string();
        let setup = self
            .setup_worktree_session_authorized(
                &setup_inv,
                cancel,
                &repository,
                &managed,
                SetupAdmission {
                    user_requested,
                    explicit: true,
                    recipe: Some(&recipe),
                    recovery: None,
                },
            )
            .await?;
        if let SetupOutcome::Failed { index, code } = setup {
            return Err(io::Error::other(format!(
                "Isolated child setup command {index} failed with exit code {code:?}; checkout retained at {}",
                managed.path.display()
            )));
        }
        runtime
            .complete_child_worktree_setup(&child.id, &managed)
            .await
            .map_err(io::Error::other)?;
        Ok(ChildWorktree {
            child_id: child.id.clone(),
            host: self.weak.clone(),
            invocation: inv.clone(),
            repository,
            managed,
            cleanup: recipe.settings.cleanup,
        })
    }
}

impl ChildWorktree {
    pub(crate) async fn retained(
        host: &BuiltinHost,
        inv: &Invocation,
        child_id: &str,
        managed: Managed,
    ) -> io::Result<Self> {
        if !managed.path.exists() {
            return Err(io::Error::other(
                "Isolated child worktree was removed; start a fresh child",
            ));
        }
        let (repository, current) = Repository::managed_at(&managed.path)?
            .ok_or_else(|| io::Error::other("Isolated child ownership is missing"))?;
        if current != managed {
            return Err(io::Error::other(
                "Isolated child ownership changed; recovery is required",
            ));
        }
        let (config, _) =
            (host.opts.config)(Path::new(&inv.directory)).map_err(io::Error::other)?;
        Ok(Self {
            child_id: child_id.into(),
            host: host.weak.clone(),
            invocation: inv.clone(),
            repository,
            managed,
            cleanup: Settings::from_config(&config)
                .map_err(io::Error::other)?
                .cleanup,
        })
    }

    pub(crate) async fn report(&self, cancel: CancellationToken) -> Value {
        let mut result = json!({"id":self.managed.id,"path":self.managed.path,"branch":self.managed.branch,"kept":true});
        let outcome = match self.host.upgrade().and_then(|host| host.runtime()) {
            Some(runtime) => {
                runtime
                    .own_worktree_setup(cancel.clone(), self.report_owned(cancel, &mut result))
                    .await
            }
            None => Err(io::Error::other(
                "Runtime stopped before worktree reporting",
            )),
        };
        if let Err(error) = outcome {
            result["cleanup_error"] = json!(error.to_string());
            result["kept"] = Value::Null;
        }
        result
    }

    async fn confirm_cleanup(
        &self,
        host: &BuiltinHost,
        cleanup: Cleanup,
        cancel: &CancellationToken,
    ) -> io::Result<bool> {
        if cleanup != Cleanup::Ask {
            return Ok(true);
        }
        let runtime = host
            .runtime()
            .ok_or_else(|| io::Error::other("Runtime stopped"))?;
        let asker = runtime
            .operation_asker(&self.child_id)
            .await
            .map_err(io::Error::other)?;
        let ask = cyber_server::runtime::PermissionAsk {
            action: "worktree".into(),
            resources: vec![
                format!("Remove clean worktree: {}", self.managed.path.display()),
                format!("Branch: {}", self.managed.branch),
            ],
            always_patterns: Vec::new(),
            metadata: json!({"operation":"cleanup", "requires_confirmation":true, "path":self.managed.path, "branch":self.managed.branch}),
        };
        let reply = tokio::select! {
            biased;
            _ = cancel.cancelled() => return Ok(false),
            reply = asker.permission(ask) => reply,
        };
        Ok(matches!(
            reply,
            cyber_server::runtime::PermissionReply::Once
                | cyber_server::runtime::PermissionReply::Always
        ))
    }

    async fn report_owned(&self, cancel: CancellationToken, result: &mut Value) -> io::Result<()> {
        let host = self
            .host
            .upgrade()
            .ok_or_else(|| io::Error::other("Tool host stopped"))?;
        let data = host
            .opts
            .tool_output_dir
            .parent()
            .ok_or_else(|| io::Error::other("Missing data directory"))?;
        let project = cyber_core::project::identify(Path::new(&self.invocation.directory));
        let lock = host.path_lock(&data.join("child-lifecycle").join(project.id));
        let _lifecycle = tokio::select! {
            biased;
            _ = cancel.cancelled() => return Err(io::Error::new(io::ErrorKind::Interrupted, "Worktree reporting cancelled")),
            guard = lock.lock() => guard,
        };
        let ctx = Ctx {
            hook_decision: None,
            host: &host,
            inv: &self.invocation,
            policy: host
                .policy(&self.invocation)
                .await
                .map_err(io::Error::other)?,
            location: PathBuf::from(&self.invocation.directory),
            cancel,
        };
        let execution = GitPort {
            ctx: &ctx,
            writable: Some(vec![
                self.repository.common_dir.clone(),
                self.managed.path.clone(),
            ]),
            credentials: &[],
        };
        let changes = retry_repository_busy(&ctx.cancel, || {
            self.repository.changes(&execution, &self.managed)
        })
        .await?;
        result["changes"] = json!(changes);
        let (config, _) = (host.opts.config)(&ctx.location).map_err(io::Error::other)?;
        let cleanup = Settings::from_config(&config)
            .map_err(io::Error::other)?
            .cleanup;
        if changes.dirty
            || !changes.files.is_empty()
            || changes.ahead != 0
            || self.cleanup == Cleanup::Keep
            || cleanup == Cleanup::Keep
        {
            return Ok(());
        }
        drop(_lifecycle);
        if !self.confirm_cleanup(&host, cleanup, &ctx.cancel).await? {
            result["cleanup_reason"] = json!("Cleanup was not approved");
            return Ok(());
        }
        let _lifecycle = tokio::select! {
            biased;
            _ = ctx.cancel.cancelled() => return Err(io::Error::new(io::ErrorKind::Interrupted, "Cleanup cancelled")),
            guard = lock.lock() => guard,
        };
        let (config, _) = (host.opts.config)(&ctx.location).map_err(io::Error::other)?;
        let current = Settings::from_config(&config)
            .map_err(io::Error::other)?
            .cleanup;
        if current == Cleanup::Keep || (current == Cleanup::Ask && cleanup != Cleanup::Ask) {
            result["cleanup_reason"] = json!("Cleanup policy changed");
            return Ok(());
        }
        let ctx = Ctx {
            hook_decision: None,
            policy: host
                .policy(&self.invocation)
                .await
                .map_err(io::Error::other)?,
            ..ctx
        };
        let execution = GitPort {
            ctx: &ctx,
            writable: Some(vec![
                self.repository.common_dir.clone(),
                self.managed.path.clone(),
            ]),
            credentials: &[],
        };
        let changes = retry_repository_busy(&ctx.cancel, || {
            self.repository.changes(&execution, &self.managed)
        })
        .await?;
        result["changes"] = json!(changes);
        if changes.dirty || !changes.files.is_empty() || changes.ahead != 0 {
            result["cleanup_reason"] = json!("Checkout changed while cleanup was pending");
            return Ok(());
        }
        authorize_owned_worktree(
            &ctx,
            &Request {
                action: "worktree".into(),
                resources: vec![self.managed.name.clone()],
                ..Default::default()
            },
        )
        .map_err(tool_error)?;
        retry_repository_busy(&ctx.cancel, || {
            self.repository
                .remove(&execution, &CheckoutActivity, &self.managed, false)
        })
        .await?;
        result["kept"] = json!(false);
        Ok(())
    }
}

impl cyber_server::runtime::ChildContinuation for ChildWorktree {
    fn settle(
        self: Box<Self>,
        completed: bool,
        cancel: CancellationToken,
    ) -> futures::future::BoxFuture<'static, Result<Option<Value>, String>> {
        Box::pin(async move {
            if !completed {
                return Ok(None);
            }
            let report = self.report(cancel).await;
            if let Some(error) = report["cleanup_error"].as_str() {
                return Err(error.to_owned());
            }
            Ok(Some(report))
        })
    }
}
