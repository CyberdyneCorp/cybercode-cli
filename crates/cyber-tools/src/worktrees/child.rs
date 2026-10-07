//! Child-owned checkouts share native creation, setup, activity and removal boundaries.
use super::*;
use cyber_core::worktrees::{Branch, CheckoutActivity, Cleanup};
use cyber_server::runtime::CreateSession;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::sync::Weak;

#[derive(Clone)]
pub(crate) struct ChildWorktree {
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
        Name::parse(child_name).map_err(io::Error::other)?;
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
                self.create_child_checkout(inv, cancel.child_token(), &worktree_request, &branch),
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
        let session = runtime
            .create_session(request)
            .await
            .map_err(io::Error::other)?;
        let mut setup_inv = inv.clone();
        setup_inv.session_id = session.id;
        setup_inv.directory = session.directory;
        setup_inv.agent = session.agent;
        setup_inv.mode = session.mode;
        setup_inv.rules = session.rules;
        let setup = self
            .setup_worktree_session_authorized(
                &setup_inv,
                cancel,
                &repository,
                &managed,
                SetupAdmission {
                    explicit: true,
                    recipe: Some(&recipe),
                },
            )
            .await?;
        if let SetupOutcome::Failed { index, code } = setup {
            return Err(io::Error::other(format!(
                "Isolated child setup command {index} failed with exit code {code:?}; checkout retained at {}",
                managed.path.display()
            )));
        }
        Ok(ChildWorktree {
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
    ) -> io::Result<(Repository, Managed, SetupRecipe)> {
        let ctx = Ctx {
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
        authorize_worktree(
            &ctx,
            false,
            Request {
                action: "worktree".into(),
                resources: vec![request.name.as_str().into()],
                ..Default::default()
            },
            json!({"operation":"create","branch":branch}),
        )
        .await
        .map_err(tool_error)?;
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
        let managed = retry_repository_busy(&ctx.cancel, || {
            repository.create_on_branch(
                &execution,
                &settings,
                &request.data,
                &request.project_id,
                Branch {
                    name: &request.name,
                    reference: branch,
                },
            )
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

impl ChildWorktree {
    pub(crate) async fn retained(
        host: &BuiltinHost,
        inv: &Invocation,
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
            || changes.ahead != 0
            || self.cleanup == Cleanup::Keep
            || cleanup != Cleanup::Auto
        {
            return Ok(());
        }
        authorize_worktree(
            &ctx,
            true,
            Request {
                action: "worktree".into(),
                resources: vec![self.managed.name.clone()],
                ..Default::default()
            },
            json!({"operation":"cleanup","path":self.managed.path}),
        )
        .await
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
