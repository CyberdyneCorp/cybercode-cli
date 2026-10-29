//! Managed worktree removal, top-level exit cleanup and the `enter_worktree` /
//! `exit_worktree` tools (`worktrees` → Worktree management commands, Cleanup on
//! exit, Enter and exit tools).
use super::*;
use cyber_core::worktrees::CheckoutActivity;
use cyber_server::runtime::WorktreeChange;
use serde_json::{Value, json};

use crate::tools::{ToolError, failed};

/// The outcome of a Session's exit cleanup.
pub struct SessionCleanup {
    pub managed: Managed,
    pub kept: bool,
    pub reason: Option<String>,
}

/// A detached invocation of a user's management request at `directory`.
fn management_invocation(directory: &Path, session_id: String) -> io::Result<Invocation> {
    let id = cyber_core::ids::new_id("call");
    Ok(Invocation {
        tool_output: Default::default(),
        registration: None,
        session_id,
        directory: directory.canonicalize()?.display().to_string(),
        agent: "build".into(),
        mode: "default".into(),
        rules: serde_json::Value::Null,
        message_id: cyber_core::ids::new_id("msg"),
        operation_key: id.clone(),
        call_id: id,
        name: "worktree".into(),
        input: serde_json::Value::Null,
        attempt: 1,
        asker: Asker::detached(),
    })
}

fn not_found(name: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::NotFound,
        format!("No managed worktree named {name}"),
    )
}

impl BuiltinHost {
    /// Remove a managed worktree and its branch. A dirty checkout needs `force`; a
    /// checkout that is the Location of a running Session is refused either way. One
    /// that an idle Session entered with `enter_worktree` and has not exited needs
    /// `force`, which first returns that Session to the Location it entered from.
    /// `WorktreeRemove` then runs as a Session the checkout belonged to, if one has a
    /// Location left ([`Self::removal_owner`]).
    pub async fn remove_worktree(
        &self,
        directory: &Path,
        name: &Name,
        force: bool,
        cancel: CancellationToken,
    ) -> io::Result<Managed> {
        let runtime = self
            .runtime()
            .ok_or_else(|| io::Error::other("Runtime is not attached"))?;
        let inv = management_invocation(directory, cyber_core::ids::new_id("ses"))?;
        let (managed, owner) = runtime
            .own_worktree_setup(
                cancel.clone(),
                self.remove_worktree_owned(&inv, name, force, cancel.clone()),
            )
            .await?;
        runtime.publish_worktree(WorktreeChange::Removed, directory, &managed, None);
        // After shutdown admission is released, as for an isolated child. Hook events
        // carry a Session's identity and Location, so without such an owner none runs.
        match owner {
            Some(owner) => {
                let mut hook_inv = inv;
                hook_inv.session_id = owner.id;
                hook_inv.agent = owner.agent;
                hook_inv.mode = owner.mode;
                hook_inv.rules = owner.rules;
                self.worktree_remove_hooks(&hook_inv, &managed, cancel)
                    .await;
            }
            None => cyber_core::log::debug(
                "hooks",
                "WorktreeRemove skipped: no Session of the removed worktree has a Location left",
                json!({"name": managed.name}),
            ),
        }
        Ok(managed)
    }

    /// The Session `WorktreeRemove` runs as for a user's removal, one whose Location
    /// outlives the checkout: a Session that entered it (only `force` removes it, after
    /// returning that Session), else the parent of an isolated child in it; live
    /// Sessions before archived ones. A Session started in the checkout has no Location
    /// left to run hooks in.
    async fn removal_owner(
        runtime: &cyber_server::runtime::Runtime,
        managed: &Managed,
    ) -> io::Result<Option<cyber_server::runtime::SessionInfo>> {
        let mut bound: Vec<_> = runtime
            .bound_sessions(managed)
            .await
            .map_err(io::Error::other)?
            .into_iter()
            .filter(|info| info.worktree_origin.is_some() || info.parent_id.is_some())
            .collect();
        bound.sort_by_key(|info| (info.archived, info.worktree_origin.is_none()));
        let Some(first) = bound.into_iter().next() else {
            return Ok(None);
        };
        match (&first.worktree_origin, &first.parent_id) {
            (None, Some(parent)) => Ok(runtime.state(parent).await.ok().map(|state| state.info)),
            _ => Ok(Some(first)),
        }
    }

    async fn remove_worktree_owned(
        &self,
        inv: &Invocation,
        name: &Name,
        force: bool,
        cancel: CancellationToken,
    ) -> io::Result<(Managed, Option<cyber_server::runtime::SessionInfo>)> {
        let ctx = self.worktree_ctx(inv, cancel).await?;
        self.worktree_config(&ctx)?;
        authorize_worktree(
            &ctx,
            true,
            Request {
                action: "worktree".into(),
                resources: vec![name.as_str().into()],
                ..Request::default()
            },
            json!({"operation": "remove", "name": name.as_str(), "force": force}),
        )
        .await
        .map_err(tool_error)?;
        let discovery = GitPort {
            ctx: &ctx,
            writable: Some(Vec::new()),
            credentials: &[],
            read_only: Vec::new(),
        };
        let repository = Repository::discover(&discovery, &ctx.location).await?;
        let managed = retry_repository_busy(&ctx.cancel, || repository.list(&discovery))
            .await?
            .into_iter()
            .find_map(|entry| match entry {
                ListedWorktree::Ready(managed) if managed.name == name.as_str() => {
                    Some(Ok(managed))
                }
                ListedWorktree::Pending(managed) if managed.name == name.as_str() => Some(Err(
                    io::Error::other("Worktree creation is incomplete; recovery is required"),
                )),
                ListedWorktree::Invalid {
                    name: listed,
                    error,
                } if listed == name.as_str() => Some(Err(io::Error::other(error))),
                _ => None,
            })
            .ok_or_else(|| not_found(name.as_str()))??;
        let runtime = self
            .runtime()
            .ok_or_else(|| io::Error::other("Runtime is not attached"))?;
        let owner = Self::removal_owner(&runtime, &managed).await?;
        self.release_entered(&runtime, &managed, force).await?;
        self.require_owned_storage(&ctx, &repository, &managed)?;
        // The checkout is deleted in-process; Git only prunes its registration.
        let execution = GitPort {
            ctx: &ctx,
            writable: Some(vec![repository.common_dir.clone()]),
            credentials: &[],
            read_only: Vec::new(),
        };
        retry_repository_busy(&ctx.cancel, || {
            repository.remove(&execution, &CheckoutActivity, &managed, force)
        })
        .await?;
        Ok((managed, owner))
    }

    /// Refuse a checkout that Sessions entered and have not exited, or with `force`
    /// return each, idle, to the Location it entered from; a running one refuses.
    async fn release_entered(
        &self,
        runtime: &cyber_server::runtime::Runtime,
        managed: &Managed,
        force: bool,
    ) -> io::Result<()> {
        let entered = runtime
            .entered_sessions(managed)
            .await
            .map_err(io::Error::other)?;
        if !force && let Some(session) = entered.first() {
            return Err(io::Error::other(format!(
                "Worktree in use by {session}, which entered it; exit the worktree first or pass --force"
            )));
        }
        for session in entered {
            if runtime.is_running(&session) {
                return Err(io::Error::other(format!(
                    "Worktree in use by {session}, which entered it and is running"
                )));
            }
            runtime
                .exit_idle_worktree(&session)
                .await
                .map_err(io::Error::other)?;
            self.clear_session_reads(&session);
        }
        Ok(())
    }

    /// The exit cleanup of a top-level Session started in a managed worktree, asked
    /// from `directory`, a Location of the same repository whose configuration
    /// decides `worktrees.cleanup`. A clean checkout is removed with its branch;
    /// otherwise both are kept and the reason is reported.
    pub async fn cleanup_session_worktree(
        &self,
        directory: &Path,
        session_id: &str,
        cancel: CancellationToken,
    ) -> io::Result<SessionCleanup> {
        let runtime = self
            .runtime()
            .ok_or_else(|| io::Error::other("Runtime is not attached"))?;
        let info = runtime
            .state(session_id)
            .await
            .map_err(io::Error::other)?
            .info;
        let worktree_id = match (&info.parent_id, &info.worktree_origin, &info.worktree_id) {
            (None, None, Some(id)) => id.clone(),
            (None, Some(_), Some(_)) => {
                return self.cleanup_entered_session(directory, info, cancel).await;
            }
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "Only a top-level Session started in or entered into a managed worktree has exit cleanup",
                ));
            }
        };
        if runtime.is_running(session_id) {
            return Err(io::Error::other("The Session is still running"));
        }
        let (repository, managed) = Repository::managed_at(Path::new(&info.directory))?
            .filter(|(_, managed)| managed.id == worktree_id)
            .ok_or_else(|| {
                io::Error::other("The Session's worktree ownership changed; recovery is required")
            })?;
        let mut inv = management_invocation(directory, info.id.clone())?;
        inv.agent = info.agent;
        inv.mode = info.mode;
        inv.rules = info.rules;
        self.check_same_repository(&inv, &managed, cancel.clone())
            .await?;
        let (config, _) =
            (self.opts.config)(Path::new(&inv.directory)).map_err(io::Error::other)?;
        let cleanup = Settings::from_config(&config)
            .map_err(io::Error::other)?
            .cleanup;
        let report = ChildWorktree::top_level(self, inv, repository, managed.clone(), cleanup)
            .report(cancel)
            .await;
        Ok(cleanup_outcome(managed, &report))
    }

    /// The exit cleanup of a top-level Session that ends in a worktree it entered: it
    /// returns, idle, to the Location it entered from, then a clean checkout it created
    /// is removed as by `exit_worktree`; one it found existing is kept. A checkout that
    /// is already gone only returns the Session.
    async fn cleanup_entered_session(
        &self,
        directory: &Path,
        info: cyber_server::runtime::SessionInfo,
        cancel: CancellationToken,
    ) -> io::Result<SessionCleanup> {
        let runtime = self
            .runtime()
            .ok_or_else(|| io::Error::other("Runtime is not attached"))?;
        if runtime.is_running(&info.id) {
            return Err(io::Error::other("The Session is still running"));
        }
        let (managed, exit) = runtime
            .exit_idle_worktree(&info.id)
            .await
            .map_err(io::Error::other)?
            .ok_or_else(|| io::Error::other("The Session did not enter a managed worktree"))?;
        self.clear_session_reads(&info.id);
        let kept = |managed: Managed, reason: &str| {
            runtime.publish_worktree(WorktreeChange::Kept, directory, &managed, Some(&info.id));
            SessionCleanup {
                managed,
                kept: true,
                reason: Some(reason.into()),
            }
        };
        if !exit.created {
            return Ok(kept(managed, "entered an existing worktree"));
        }
        if !managed.path.try_exists()? {
            return Ok(SessionCleanup {
                managed,
                kept: false,
                reason: Some("The worktree was already removed".into()),
            });
        }
        let Some((repository, current)) =
            Repository::managed_at(&managed.path)?.filter(|(_, current)| current.id == managed.id)
        else {
            return Ok(kept(
                managed,
                "The worktree's ownership changed; recovery is required",
            ));
        };
        let mut inv = management_invocation(directory, info.id.clone())?;
        inv.agent = info.agent;
        inv.mode = info.mode;
        inv.rules = info.rules;
        self.check_same_repository(&inv, &current, cancel.clone())
            .await?;
        let (config, _) =
            (self.opts.config)(Path::new(&inv.directory)).map_err(io::Error::other)?;
        let cleanup = Settings::from_config(&config)
            .map_err(io::Error::other)?
            .cleanup;
        let report = ChildWorktree::top_level(self, inv, repository, current.clone(), cleanup)
            .report(cancel)
            .await;
        Ok(cleanup_outcome(current, &report))
    }

    async fn check_same_repository(
        &self,
        inv: &Invocation,
        managed: &Managed,
        cancel: CancellationToken,
    ) -> io::Result<()> {
        let ctx = self.worktree_ctx(inv, cancel).await?;
        let discovery = GitPort {
            ctx: &ctx,
            writable: Some(Vec::new()),
            credentials: &[],
            read_only: Vec::new(),
        };
        let repository = Repository::discover(&discovery, &ctx.location).await?;
        if repository.common_dir != managed.common_dir {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Cleanup must be asked from a Location of the worktree's repository",
            ));
        }
        Ok(())
    }

    /// `enter_worktree { name?, path? }`: switch into an existing managed worktree of
    /// this repository or create one, then move the Session there.
    pub(crate) async fn enter_worktree_tool(&self, ctx: &Ctx<'_>) -> Result<String, ToolError> {
        let runtime = self.runtime().ok_or_else(|| failed("Runtime stopped"))?;
        let input = &ctx.inv.input;
        let name = crate::tools::text(input, "name");
        let path = crate::tools::text(input, "path");
        if !name.is_empty() && !path.is_empty() {
            return Err(failed("Pass name or path, not both"));
        }
        // Refused before any checkout, permission, hook or setup side effect.
        runtime
            .check_enterable(&ctx.inv.session_id, &ctx.inv.call_id)
            .await
            .map_err(|error| failed(error.to_string()))?;
        let (managed, created, setup) = if path.is_empty() {
            let name = if name.is_empty() {
                Name::generate()
            } else {
                Name::parse(name).map_err(failed)?
            };
            self.enter_by_name(ctx, &runtime, &name).await?
        } else {
            (self.existing_at(ctx, Path::new(path)).await?, false, None)
        };
        runtime
            .enter_worktree(
                &ctx.inv.session_id,
                &ctx.inv.call_id,
                &managed,
                created,
                ctx.cancel.clone(),
            )
            .await
            .map_err(|error| failed(error.to_string()))?;
        self.clear_session_reads(&ctx.inv.session_id);
        let mut result = json!({
            "name": managed.name, "path": managed.path, "branch": managed.branch,
            "created": created, "returned_to_on_exit": ctx.inv.directory,
        });
        if let Some(setup) = setup {
            result["setup_output"] = json!(setup);
        }
        Ok(result.to_string())
    }

    async fn enter_by_name(
        &self,
        ctx: &Ctx<'_>,
        runtime: &cyber_server::runtime::Runtime,
        name: &Name,
    ) -> Result<(Managed, bool, Option<String>), ToolError> {
        let discovery = GitPort {
            ctx,
            writable: Some(Vec::new()),
            credentials: &[],
            read_only: Vec::new(),
        };
        let repository = Repository::discover(&discovery, &ctx.location)
            .await
            .map_err(io_failed)?;
        let existing = retry_repository_busy(&ctx.cancel, || repository.list(&discovery))
            .await
            .map_err(io_failed)?
            .into_iter()
            .find_map(|entry| match entry {
                ListedWorktree::Ready(managed) if managed.name == name.as_str() => {
                    Some(Ok(managed.path))
                }
                ListedWorktree::Pending(managed) if managed.name == name.as_str() => Some(Err(
                    failed("Worktree creation is incomplete; recovery is required"),
                )),
                ListedWorktree::Invalid {
                    name: listed,
                    error,
                } if listed == name.as_str() => Some(Err(failed(error))),
                _ => None,
            })
            .transpose()?;
        if let Some(path) = existing {
            return Ok((self.existing_at(ctx, &path).await?, false, None));
        }
        let (managed, setup) = self.create_entered(ctx, runtime, name).await?;
        Ok((managed, true, Some(setup)))
    }

    /// A ready managed worktree root of the Session's repository.
    async fn existing_at(&self, ctx: &Ctx<'_>, path: &Path) -> Result<Managed, ToolError> {
        let path = ctx.location.join(path).canonicalize().map_err(io_failed)?;
        let (_, managed) = Repository::managed_at(&path)
            .map_err(io_failed)?
            .filter(|(_, managed)| managed.path == path && managed.ready)
            .ok_or_else(|| failed("Not the root of a ready managed worktree"))?;
        ctx.authorize(
            Request {
                action: "worktree".into(),
                resources: vec![managed.name.clone()],
                read_only: true,
                ..Request::default()
            },
            vec![managed.name.clone()],
            json!({"operation": "enter", "path": managed.path, "branch": managed.branch}),
        )
        .await?;
        self.check_same_repository(ctx.inv, &managed, ctx.cancel.clone())
            .await
            .map_err(io_failed)?;
        Ok(managed)
    }

    /// Create the checkout with the Session's permission and hooks, then run setup
    /// before the Session moves, so its output is part of this call's result.
    async fn create_entered(
        &self,
        ctx: &Ctx<'_>,
        runtime: &cyber_server::runtime::Runtime,
        name: &Name,
    ) -> Result<(Managed, String), ToolError> {
        let request = WorktreeSessionRequest {
            session: CreateSession::default(),
            data: self
                .opts
                .tool_output_dir
                .parent()
                .ok_or_else(|| failed("Missing data directory"))?
                .to_owned(),
            project_id: cyber_core::project::identify(&ctx.location).id,
            name: name.clone(),
        };
        self.admit_worktree_creation(ctx.inv, ctx.cancel.clone(), &request, false)
            .await
            .map_err(io_failed)?;
        let (repository, managed, recipe) = runtime
            .own_worktree_setup(
                ctx.cancel.clone(),
                self.create_worktree(ctx.inv, ctx.cancel.clone(), &request, None),
            )
            .await
            .map_err(io_failed)?;
        runtime.publish_worktree(
            WorktreeChange::Created,
            &ctx.location,
            &managed,
            Some(&ctx.inv.session_id),
        );
        let mut setup_inv = ctx.inv.clone();
        setup_inv.directory = managed.path.display().to_string();
        let capture = Capture {
            limit: 1 << 20,
            output: Mutex::default(),
        };
        let outcome = runtime
            .own_worktree_setup(
                ctx.cancel.clone(),
                self.setup_worktree_authorized(
                    &setup_inv,
                    ctx.cancel.clone(),
                    &repository,
                    &managed,
                    &capture,
                    SetupAdmission {
                        explicit: true,
                        user_requested: false,
                        recipe: Some(&recipe),
                        recovery: None,
                    },
                ),
            )
            .await
            .map_err(io_failed)?;
        let output = capture
            .output
            .into_inner()
            .map(|(stdout, stderr)| {
                format!(
                    "{}{}",
                    String::from_utf8_lossy(&stdout),
                    String::from_utf8_lossy(&stderr)
                )
            })
            .unwrap_or_default();
        if let SetupOutcome::Failed { index, code } = outcome {
            return Err(failed(format!(
                "Worktree setup command {index} failed with exit code {code:?}; checkout retained at {}\n{output}",
                managed.path.display()
            )));
        }
        Ok((managed, output))
    }

    /// `exit_worktree { keep? }`: return to the Location the Session entered from.
    /// Unless `keep`, a clean checkout this Session created is then removed with its
    /// branch; an existing worktree it entered is always kept.
    pub(crate) async fn exit_worktree_tool(&self, ctx: &Ctx<'_>) -> Result<String, ToolError> {
        let runtime = self.runtime().ok_or_else(|| failed("Runtime stopped"))?;
        let keep = ctx
            .inv
            .input
            .get("keep")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let info = runtime
            .state(&ctx.inv.session_id)
            .await
            .map_err(|error| failed(error.to_string()))?
            .info;
        let (repository, managed) = Repository::managed_at(Path::new(&info.directory))
            .map_err(io_failed)?
            .filter(|(_, managed)| info.worktree_id.as_deref() == Some(managed.id.as_str()))
            .ok_or_else(|| failed("The Session did not enter a managed worktree"))?;
        ctx.authorize(
            Request {
                action: "worktree".into(),
                resources: vec![managed.name.clone()],
                read_only: true,
                ..Request::default()
            },
            vec![managed.name.clone()],
            json!({"operation": "exit", "path": managed.path, "keep": keep}),
        )
        .await?;
        let exit = runtime
            .exit_worktree(
                &ctx.inv.session_id,
                &ctx.inv.call_id,
                &managed,
                ctx.cancel.clone(),
            )
            .await
            .map_err(|error| failed(error.to_string()))?;
        self.clear_session_reads(&ctx.inv.session_id);
        let origin = exit.origin;
        let mut inv = ctx.inv.clone();
        inv.directory = origin.clone();
        let kept_reason = match (keep, exit.created) {
            (true, _) => Some("keep was requested"),
            (false, false) => Some("entered an existing worktree"),
            (false, true) => None,
        };
        let mut result = if let Some(reason) = kept_reason {
            runtime.publish_worktree(
                WorktreeChange::Kept,
                Path::new(&origin),
                &managed,
                Some(&ctx.inv.session_id),
            );
            json!({"path": managed.path, "branch": managed.branch, "kept": true, "cleanup_reason": reason})
        } else {
            let (config, _) = (self.opts.config)(Path::new(&origin)).map_err(failed)?;
            let cleanup = Settings::from_config(&config).map_err(failed)?.cleanup;
            ChildWorktree::top_level(self, inv, repository, managed, cleanup)
                .report(ctx.cancel.clone())
                .await
        };
        result["returned_to"] = json!(origin);
        Ok(result.to_string())
    }
}

fn io_failed(error: io::Error) -> ToolError {
    if error.kind() == io::ErrorKind::Interrupted {
        return ToolError::Aborted;
    }
    failed(error.to_string())
}

fn cleanup_outcome(managed: Managed, report: &Value) -> SessionCleanup {
    let kept = report["kept"] != json!(false);
    let changes = &report["changes"];
    let changed = changes["dirty"] == json!(true)
        || changes["ahead"].as_u64().is_some_and(|ahead| ahead > 0)
        || changes["files"]
            .as_array()
            .is_some_and(|files| !files.is_empty());
    let reason = report["cleanup_error"]
        .as_str()
        .or_else(|| report["cleanup_reason"].as_str())
        .map(str::to_owned)
        .or_else(|| {
            kept.then(|| {
                if changed {
                    "The worktree has uncommitted changes or commits beyond its base".into()
                } else {
                    "worktrees.cleanup is keep".into()
                }
            })
        });
    SessionCleanup {
        managed,
        kept,
        reason,
    }
}
