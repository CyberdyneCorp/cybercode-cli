//! Owned sandboxed Git creation and journaled setup for managed worktree Sessions.

mod child;
mod inspection;
mod recovery;
pub(crate) use child::ChildWorktree;

use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{ExitStatus, Output, Stdio};
use std::sync::Mutex;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use std::time::Duration;

use cyber_core::worktrees::{
    GitExecution, GitFuture, ListedWorktree, Managed, Name, Repository, Settings, SetupEvent,
    SetupExecution, SetupFuture, SetupOutcome, SetupSink, SetupStream, WorktreeStatus,
};
use cyber_server::runtime::{Asker, CreateSession, Invocation, SessionInfo};
use cyber_server::worktrees::{CommandDecision, CommandResult, SetupJournal};
use tokio::io::AsyncReadExt;
use tokio_util::sync::CancellationToken;

use crate::host::{BuiltinHost, Ctx};
use crate::permissions::Request;
use crate::tools::process::Process;

/// Inputs for creating a new Session in a managed worktree.
pub struct WorktreeSessionRequest {
    pub session: CreateSession,
    pub data: PathBuf,
    pub project_id: String,
    pub name: Name,
}

/// A setup failure retains both the Session and its owned worktree.
pub struct WorktreeSession {
    pub session: SessionInfo,
    pub managed: Managed,
    pub setup: Result<SetupOutcome, String>,
}

pub struct WorktreeListing {
    pub ownership: ListedWorktree,
    pub status: Option<WorktreeStatus>,
}

struct SetupRecipe {
    settings: Settings,
    credentials: Vec<String>,
}

struct SetupAdmission<'a> {
    explicit: bool,
    user_requested: bool,
    recipe: Option<&'a SetupRecipe>,
    recovery: Option<&'a cyber_server::worktrees::SetupRecoveryRequest>,
}

impl BuiltinHost {
    pub(crate) async fn claim_worktree_location(
        &self,
        info: &SessionInfo,
        creating: bool,
        cancel: CancellationToken,
    ) -> Result<cyber_server::runtime::LocationLease, String> {
        use cyber_server::runtime::LocationLease;
        let locations = Repository::managed_locations_at(Path::new(&info.directory))
            .map_err(|error| error.to_string())?;
        let Some((_, managed)) = locations.first() else {
            if info.worktree_id.is_some() {
                return Err("Managed checkout ownership is missing; recovery is required".into());
            }
            return Ok(LocationLease::unmanaged());
        };
        match info.worktree_id.as_deref() {
            Some(id) if id == managed.id => {}
            None if creating => {}
            _ => return Err(
                "Managed checkout creation identity changed or is unbound; recovery is required"
                    .into(),
            ),
        }
        let inv = Invocation {
            session_id: info.id.clone(),
            directory: info.directory.clone(),
            agent: info.agent.clone(),
            mode: info.mode.clone(),
            rules: info.rules.clone(),
            message_id: String::new(),
            call_id: cyber_core::ids::new_id("call"),
            operation_key: String::new(),
            name: "worktree".into(),
            input: serde_json::Value::Null,
            attempt: 1,
            asker: Asker::detached(),
        };
        let ctx = Ctx {
            hook_decision: None,
            host: self,
            inv: &inv,
            policy: self.policy(&inv).await?,
            location: PathBuf::from(&info.directory),
            cancel,
        };
        let execution = GitPort {
            ctx: &ctx,
            writable: Some(Vec::new()),
            credentials: &[],
        };
        let id = managed.id.clone();
        let mut leases = Vec::with_capacity(locations.len());
        for (repository, managed) in locations {
            leases.push(
                retry_repository_busy(&ctx.cancel, || {
                    repository.claim(&execution, &managed, &info.id)
                })
                .await
                .map_err(|error| error.to_string())?,
            );
        }
        Ok(LocationLease::managed(id, Box::new(WorktreeLease(leases))))
    }

    pub async fn list_worktrees(
        &self,
        directory: &Path,
        cancel: CancellationToken,
    ) -> io::Result<Vec<WorktreeListing>> {
        let runtime = self
            .runtime()
            .ok_or_else(|| io::Error::other("Runtime is not attached"))?;
        let id = cyber_core::ids::new_id("call");
        let inv = Invocation {
            session_id: cyber_core::ids::new_id("ses"),
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
        };
        runtime
            .own_worktree_setup(cancel.clone(), self.list_worktrees_owned(&inv, cancel))
            .await
    }

    async fn list_worktrees_owned(
        &self,
        inv: &Invocation,
        cancel: CancellationToken,
    ) -> io::Result<Vec<WorktreeListing>> {
        let ctx = Ctx {
            hook_decision: None,
            host: self,
            inv,
            policy: self.policy(inv).await.map_err(io::Error::other)?,
            location: Path::new(&inv.directory).canonicalize()?,
            cancel,
        };
        authorize_worktree(
            &ctx,
            true,
            Request {
                action: "worktree".into(),
                resources: vec![ctx.location.display().to_string()],
                ..Default::default()
            },
            serde_json::json!({"operation":"list"}),
        )
        .await
        .map_err(tool_error)?;
        let execution = GitPort {
            ctx: &ctx,
            writable: Some(Vec::new()),
            credentials: &[],
        };
        let repository = Repository::discover(&execution, &ctx.location).await?;
        let mut result = Vec::new();
        for mut ownership in repository.list(&execution).await? {
            let status = if let ListedWorktree::Ready(managed) = &ownership {
                match repository.status(&execution, managed).await {
                    Ok(status) => Some(status),
                    Err(error)
                        if matches!(
                            error.kind(),
                            io::ErrorKind::Interrupted
                                | io::ErrorKind::WouldBlock
                                | io::ErrorKind::PermissionDenied
                        ) =>
                    {
                        return Err(error);
                    }
                    Err(error) => {
                        ownership = ListedWorktree::Invalid {
                            name: managed.name.clone(),
                            error: error.to_string(),
                        };
                        None
                    }
                }
            } else {
                None
            };
            result.push(WorktreeListing { ownership, status });
        }
        Ok(result)
    }

    /// Create through owned sandboxed Git, then attach a fresh Session and run setup.
    /// Existing Session Location changes are handled by the separate enter/exit flow.
    pub async fn create_worktree_session(
        &self,
        inv: &Invocation,
        cancel: CancellationToken,
        request: WorktreeSessionRequest,
    ) -> io::Result<WorktreeSession> {
        if request.session.id.is_some() {
            return Err(io::Error::other(
                "Worktree creation requires a fresh Session ID",
            ));
        }
        let runtime = self
            .runtime()
            .ok_or_else(|| io::Error::other("Runtime is not attached"))?;
        let source = runtime
            .state(&inv.session_id)
            .await
            .map_err(io::Error::other)?
            .info;
        if Path::new(&source.directory).canonicalize()?
            != Path::new(&inv.directory).canonicalize()?
        {
            return Err(io::Error::other(
                "Creation invocation differs from Session Location",
            ));
        }
        let mut creation_inv = inv.clone();
        creation_inv.mode = source.mode;
        creation_inv.rules = source.rules;
        creation_inv.agent = source.agent;
        self.create_worktree_session_owned(&runtime, &creation_inv, cancel, request, false)
            .await
    }

    /// An authenticated client explicitly requests isolation for a new Session.
    /// This authorizes this creation/setup operation, without changing its Session rules.
    pub async fn start_worktree_session(
        &self,
        directory: &Path,
        call_id: String,
        cancel: CancellationToken,
        request: WorktreeSessionRequest,
    ) -> io::Result<WorktreeSession> {
        if request.session.id.is_some() || call_id.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "A fresh Session and creation call ID are required",
            ));
        }
        let runtime = self
            .runtime()
            .ok_or_else(|| io::Error::other("Runtime is not attached"))?;
        let inv = Invocation {
            session_id: cyber_core::ids::new_id("ses"),
            directory: directory.canonicalize()?.display().to_string(),
            agent: request
                .session
                .agent
                .clone()
                .unwrap_or_else(|| "build".into()),
            mode: request
                .session
                .mode
                .clone()
                .unwrap_or_else(|| "default".into()),
            rules: request.session.rules.clone().unwrap_or_default(),
            message_id: cyber_core::ids::new_id("msg"),
            operation_key: call_id.clone(),
            call_id,
            name: "worktree".into(),
            input: serde_json::Value::Null,
            attempt: 1,
            asker: Asker::detached(),
        };
        self.create_worktree_session_owned(&runtime, &inv, cancel, request, true)
            .await
    }

    async fn create_worktree_session_owned(
        &self,
        runtime: &cyber_server::runtime::Runtime,
        inv: &Invocation,
        cancel: CancellationToken,
        request: WorktreeSessionRequest,
        explicit: bool,
    ) -> io::Result<WorktreeSession> {
        let owned_cancel = cancel.child_token();
        let (repository, managed, recipe) = runtime
            .own_worktree_setup(
                owned_cancel.clone(),
                self.create_worktree(inv, owned_cancel, &request, explicit),
            )
            .await?;
        if cancel.is_cancelled() {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "Creation cancelled before Session admission",
            ));
        }
        let mut session_request = request.session;
        session_request.directory = managed.path.display().to_string();
        session_request.worktree_id = Some(managed.id.clone());
        if explicit {
            session_request.id = Some(inv.session_id.clone());
        }
        let session = runtime
            .create_session(session_request)
            .await
            .map_err(io::Error::other)?;
        let mut setup_inv = inv.clone();
        setup_inv.session_id = session.id.clone();
        setup_inv.directory = session.directory.clone();
        setup_inv.mode = session.mode.clone();
        setup_inv.agent = session.agent.clone();
        setup_inv.rules = session.rules.clone();
        let setup = self
            .setup_worktree_session_authorized(
                &setup_inv,
                cancel,
                &repository,
                &managed,
                SetupAdmission {
                    user_requested: false,
                    explicit: true,
                    recipe: Some(&recipe),
                    recovery: None,
                },
            )
            .await
            .map_err(|error| error.to_string());
        Ok(WorktreeSession {
            session,
            managed,
            setup,
        })
    }

    async fn create_worktree(
        &self,
        inv: &Invocation,
        cancel: CancellationToken,
        request: &WorktreeSessionRequest,
        explicit: bool,
    ) -> io::Result<(Repository, Managed, SetupRecipe)> {
        let location = Path::new(&inv.directory).canonicalize()?;
        let ctx = Ctx {
            hook_decision: None,
            host: self,
            inv,
            policy: self.policy(inv).await.map_err(io::Error::other)?,
            location,
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
                "Read-only sandbox refuses worktree creation",
            ));
        }
        let settings = Settings::from_config(&config).map_err(io::Error::other)?;
        authorize_worktree(
            &ctx,
            explicit,
            Request {
                action: "worktree".into(),
                resources: vec![request.name.as_str().into()],
                ..Request::default()
            },
            serde_json::json!({"operation": "create", "name": request.name.as_str()}),
        )
        .await
        .map_err(tool_error)?;
        if ctx.cancel.is_cancelled() {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "Creation cancelled",
            ));
        }
        let discovery = GitPort {
            ctx: &ctx,
            writable: Some(Vec::new()),
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
        let managed = repository
            .create(
                &execution,
                &settings,
                &request.data,
                &request.project_id,
                &request.name,
            )
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

    /// Lifecycle entry point delivering setup progress to attached Session clients.
    pub async fn setup_worktree_session(
        &self,
        inv: &Invocation,
        cancel: CancellationToken,
        repository: &Repository,
        managed: &Managed,
    ) -> io::Result<SetupOutcome> {
        self.setup_worktree_session_authorized(
            inv,
            cancel,
            repository,
            managed,
            SetupAdmission {
                user_requested: false,
                explicit: false,
                recipe: None,
                recovery: None,
            },
        )
        .await
    }

    async fn setup_worktree_session_authorized(
        &self,
        inv: &Invocation,
        cancel: CancellationToken,
        repository: &Repository,
        managed: &Managed,
        admission: SetupAdmission<'_>,
    ) -> io::Result<SetupOutcome> {
        let runtime = self
            .runtime()
            .ok_or_else(|| io::Error::other("Runtime is not attached for setup output"))?;
        let sink = runtime
            .worktree_setup_sink(&inv.session_id, &inv.call_id, managed)
            .await
            .map_err(io::Error::other)?;
        let owned_cancel = cancel.child_token();
        let result = runtime
            .own_session_worktree_setup(
                &inv.session_id,
                owned_cancel.clone(),
                self.setup_worktree_authorized(
                    inv,
                    owned_cancel,
                    repository,
                    managed,
                    &sink,
                    admission,
                ),
            )
            .await;
        if let Err(error) = &result {
            let _ = sink.failed(&error.to_string());
        }
        result
    }

    /// The lifecycle owner supplies the Session's streaming sink and settles this
    /// explicit setup attempt durably; interrupted commands must not be blindly retried.
    pub async fn setup_worktree(
        &self,
        inv: &Invocation,
        cancel: CancellationToken,
        repository: &Repository,
        managed: &Managed,
        sink: &dyn SetupSink,
    ) -> io::Result<SetupOutcome> {
        self.setup_worktree_authorized(
            inv,
            cancel,
            repository,
            managed,
            sink,
            SetupAdmission {
                user_requested: false,
                explicit: false,
                recipe: None,
                recovery: None,
            },
        )
        .await
    }

    async fn setup_worktree_authorized(
        &self,
        inv: &Invocation,
        cancel: CancellationToken,
        repository: &Repository,
        managed: &Managed,
        sink: &dyn SetupSink,
        admission: SetupAdmission<'_>,
    ) -> io::Result<SetupOutcome> {
        let authority = self
            .runtime()
            .map(|runtime| runtime.capture_child_admission(&inv.session_id))
            .transpose()
            .map_err(io::Error::other)?;
        if Path::new(&inv.directory).canonicalize()? != managed.path {
            return Err(io::Error::other(
                "Session is not located in the owned worktree",
            ));
        }
        let ctx = Ctx {
            hook_decision: None,
            host: self,
            inv,
            policy: self.policy(inv).await.map_err(io::Error::other)?,
            location: managed.path.clone(),
            cancel,
        };
        let settings = match admission.recipe {
            Some(recipe) => recipe.settings.clone(),
            None => {
                let (config, _) = (self.opts.config)(&ctx.location).map_err(io::Error::other)?;
                Settings::from_config(&config).map_err(io::Error::other)?
            }
        };
        let request = Request {
            action: "worktree".into(),
            resources: vec![managed.name.clone()],
            read_only: settings.setup.is_empty(),
            mutates: if settings.setup.is_empty() {
                Vec::new()
            } else {
                vec![managed.path.clone()]
            },
            ..Request::default()
        };
        if admission.user_requested {
            authorize_owned_worktree(&ctx, &request).map_err(tool_error)?;
        } else {
            authorize_worktree(
                &ctx,
                admission.explicit,
                request,
                serde_json::json!({"operation": "setup", "path": managed.path}),
            )
            .await
            .map_err(tool_error)?;
        }
        if admission.recovery.is_some() && !settings.setup.is_empty() {
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
                    "Read-only sandbox refuses setup recovery",
                ));
            }
        }
        let execution = Execution {
            authority,
            ctx: &ctx,
            journal: SetupJournal::new(
                Arc::clone(&self.opts.store),
                managed,
                &settings.setup,
                &inv.session_id,
            )?,
            next_command: AtomicUsize::new(0),
            credentials: admission
                .recipe
                .map(|recipe| recipe.credentials.clone())
                .unwrap_or_default(),
        };
        execution.journal.validate()?;
        if let Some(review) = admission.recovery {
            execution
                .journal
                .check_review(review.revision, &review.digest)?;
            if let Some(index) = review.retry_index {
                execution.journal.retry_failed(
                    review.revision,
                    &review.digest,
                    index,
                    &review.reason,
                )?;
            }
        }
        retry_repository_busy(&ctx.cancel, || {
            repository.setup(&execution, &execution, managed, &settings, sink)
        })
        .await
    }
}

struct Execution<'a> {
    authority: Option<cyber_server::runtime::AdmissionAuthority>,
    ctx: &'a Ctx<'a>,
    journal: SetupJournal,
    next_command: AtomicUsize,
    credentials: Vec<String>,
}

impl SetupExecution for Execution<'_> {
    fn run<'a>(
        &'a self,
        directory: &'a Path,
        command: &'a str,
        sink: &'a dyn SetupSink,
    ) -> SetupFuture<'a> {
        Box::pin(async move {
            let index = self.next_command.fetch_add(1, Ordering::Relaxed);
            let attempt = self.journal.start_attempt(index)?;
            if let CommandDecision::Recorded(result) = attempt.decision {
                return match result {
                    CommandResult::Exited { code } => Ok(code),
                    CommandResult::Failed { message }
                    | CommandResult::NotDispatched { message } => Err(io::Error::other(message)),
                };
            }
            let result = self.execute_setup(directory, command, sink).await;
            let saved = match &result {
                Ok(code) => CommandResult::Exited { code: *code },
                Err(SetupFailure::Preparation(error)) => CommandResult::NotDispatched {
                    message: error.to_string(),
                },
                Err(SetupFailure::Execution(error)) => CommandResult::Failed {
                    message: error.to_string(),
                },
            };
            self.journal.finish_attempt(
                index,
                attempt
                    .started_revision
                    .ok_or_else(|| io::Error::other("Missing setup dispatch revision"))?,
                saved,
            )?;
            result.map_err(|failure| match failure {
                SetupFailure::Preparation(error) | SetupFailure::Execution(error) => error,
            })
        })
    }
}

enum SetupFailure {
    Preparation(io::Error),
    Execution(io::Error),
}

impl Execution<'_> {
    async fn execute_setup(
        &self,
        directory: &Path,
        command: &str,
        sink: &dyn SetupSink,
    ) -> Result<Option<i32>, SetupFailure> {
        self.verify_authority().map_err(SetupFailure::Preparation)?;
        let prepared = self
            .prepare_setup(command)
            .await
            .map_err(SetupFailure::Preparation)?;
        self.verify_authority().map_err(SetupFailure::Preparation)?;
        let status = run(self.ctx, prepared, directory, sink)
            .await
            .map_err(SetupFailure::Execution)?;
        Ok(status.code())
    }

    fn verify_authority(&self) -> io::Result<()> {
        if let Some(authority) = &self.authority {
            let runtime = self
                .ctx
                .host
                .runtime()
                .ok_or_else(|| io::Error::other("Setup runtime stopped"))?;
            authority
                .verify(&runtime, &self.ctx.inv.session_id)
                .map_err(io::Error::other)?;
        }
        Ok(())
    }

    async fn prepare_setup(&self, command: &str) -> io::Result<crate::sandboxing::Prepared> {
        #[cfg(not(windows))]
        let prepared = crate::sandboxing::prepare_worktree_command(
            self.ctx,
            &crate::tools::bash::shell(&self.ctx.host.opts.shell),
            &["-c".into(), command.into()],
            &self.credentials,
        )
        .await;
        #[cfg(windows)]
        let prepared = {
            let program = crate::tools::powershell::installed(&self.ctx.location)
                .ok_or_else(|| io::Error::other("PowerShell is required for Windows setup"))?;
            crate::sandboxing::prepare_worktree_command(
                self.ctx,
                &program.display().to_string(),
                &crate::tools::powershell::arguments(command),
                &self.credentials,
            )
            .await
        };
        prepared.map_err(tool_error)
    }
}

impl GitExecution for Execution<'_> {
    fn inspect<'a>(
        &'a self,
        target: &'a cyber_core::worktrees::inspection::Target,
    ) -> cyber_core::worktrees::InspectionFuture<'a> {
        Box::pin(async move {
            GitPort {
                ctx: self.ctx,
                writable: None,
                credentials: &self.credentials,
            }
            .inspect(target)
            .await
        })
    }

    fn run<'a>(&'a self, directory: &'a Path, args: &'a [OsString]) -> GitFuture<'a> {
        Box::pin(async move {
            GitPort {
                ctx: self.ctx,
                writable: None,
                credentials: &self.credentials,
            }
            .run(directory, args)
            .await
        })
    }
}

struct GitPort<'a> {
    ctx: &'a Ctx<'a>,
    writable: Option<Vec<PathBuf>>,
    credentials: &'a [String],
}

struct WorktreeLease(Vec<cyber_core::worktrees::CheckoutLease>);

impl cyber_server::runtime::LocationGuard for WorktreeLease {
    fn settle(self: Box<Self>) -> Result<(), String> {
        for lease in self.0 {
            lease.settle().map_err(|error| error.to_string())?;
        }
        Ok(())
    }

    fn settle_retained(self: Box<Self>) -> Result<Box<dyn Send>, String> {
        let leases = self
            .0
            .into_iter()
            .map(|lease| lease.settle_retained().map_err(|error| error.to_string()))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Box::new(leases))
    }
}

impl GitExecution for GitPort<'_> {
    fn inspect<'a>(
        &'a self,
        target: &'a cyber_core::worktrees::inspection::Target,
    ) -> cyber_core::worktrees::InspectionFuture<'a> {
        Box::pin(inspection::run(self, target))
    }

    fn run<'a>(&'a self, directory: &'a Path, args: &'a [OsString]) -> GitFuture<'a> {
        Box::pin(async move {
            let mut arguments = vec![
                "-c".into(),
                "core.hooksPath=".into(),
                "-c".into(),
                "core.fsmonitor=false".into(),
            ];
            for arg in args {
                arguments.push(
                    arg.to_str()
                        .ok_or_else(|| io::Error::other("Git argument is not Unicode"))?
                        .into(),
                );
            }
            let mut prepared = match &self.writable {
                Some(roots) => {
                    crate::sandboxing::prepare_worktree_git(self.ctx, &arguments, roots).await
                }
                None => {
                    crate::sandboxing::prepare_worktree_command(
                        self.ctx,
                        "git",
                        &arguments,
                        self.credentials,
                    )
                    .await
                }
            }
            .map_err(tool_error)?;
            prepared
                .env
                .retain(|(key, _)| !key.to_ascii_uppercase().starts_with("GIT_"));
            prepared.env.extend([
                ("GIT_CONFIG_NOSYSTEM".into(), "1".into()),
                (
                    "GIT_CONFIG_GLOBAL".into(),
                    if cfg!(windows) { "NUL" } else { "/dev/null" }.into(),
                ),
            ]);
            let capture = Capture::default();
            let status = run(self.ctx, prepared, directory, &capture).await?;
            let (stdout, stderr) = capture
                .0
                .into_inner()
                .map_err(|_| io::Error::other("Git capture poisoned"))?;
            Ok(Output {
                status,
                stdout,
                stderr,
            })
        })
    }
}

#[derive(Default)]
struct Capture(Mutex<(Vec<u8>, Vec<u8>)>);

impl SetupSink for Capture {
    fn emit(&self, event: SetupEvent<'_>) -> io::Result<()> {
        if let SetupEvent::Output { stream, bytes } = event {
            let mut capture = self
                .0
                .lock()
                .map_err(|_| io::Error::other("Git capture poisoned"))?;
            if capture.0.len() + capture.1.len() + bytes.len() > 1024 * 1024 {
                return Err(io::Error::other("Git verification output exceeds 1 MiB"));
            }
            match stream {
                SetupStream::Stdout => capture.0.extend_from_slice(bytes),
                SetupStream::Stderr => capture.1.extend_from_slice(bytes),
            }
        }
        Ok(())
    }
}

async fn run(
    ctx: &Ctx<'_>,
    mut prepared: crate::sandboxing::Prepared,
    directory: &Path,
    sink: &dyn SetupSink,
) -> io::Result<ExitStatus> {
    if ctx.cancel.is_cancelled() {
        return Err(io::Error::new(
            io::ErrorKind::Interrupted,
            "Setup cancelled",
        ));
    }
    let mut child = Process::spawn(
        &prepared.program,
        &prepared.args,
        ctx.host.opts.sandbox_helper.as_deref(),
        |command| {
            command
                .env_clear()
                .envs(prepared.env.iter().map(|(k, v)| (k, v)))
                .current_dir(directory)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .kill_on_drop(true)
                .env("CYBER", "1")
                .env("CYBER_SESSION_ID", &ctx.inv.session_id)
                .env("CYBER_PROJECT_DIR", &ctx.location);
        },
    )
    .await?;
    let stdout = child
        .stdout()
        .ok_or_else(|| io::Error::other("Missing setup stdout"))?;
    let stderr = child
        .stderr()
        .ok_or_else(|| io::Error::other("Missing setup stderr"))?;
    let streams = async {
        tokio::try_join!(
            pump(stdout, SetupStream::Stdout, sink),
            pump(stderr, SetupStream::Stderr, sink)
        )?;
        Ok::<_, io::Error>(())
    };
    let result = {
        let work = async {
            let (status, ()) = tokio::try_join!(child.wait_tree(), streams)?;
            Ok::<_, io::Error>(status)
        };
        tokio::pin!(work);
        let deadline =
            tokio::time::sleep(Duration::from_millis(crate::tools::bash::MAX_TIMEOUT_MS));
        tokio::pin!(deadline);
        loop {
            tokio::select! {
                result = &mut work => break result,
                _ = ctx.cancel.cancelled() => break Err(io::Error::new(io::ErrorKind::Interrupted, "Setup cancelled")),
                _ = &mut deadline => break Err(io::Error::new(io::ErrorKind::TimedOut, "Setup timed out")),
                Some((host, reply)) = next_ask(&mut prepared.asks) => { let _ = reply.send(crate::sandboxing::answer(ctx, host).await); }
            }
        }
    };
    if result.is_err() {
        child.terminate();
        let _ = child.wait().await;
    }
    result
}

async fn next_ask(
    asks: &mut Option<tokio::sync::mpsc::Receiver<crate::sandboxing::NetworkAsk>>,
) -> Option<crate::sandboxing::NetworkAsk> {
    match asks {
        Some(receiver) => receiver.recv().await,
        None => std::future::pending().await,
    }
}

async fn pump(
    mut stream: impl tokio::io::AsyncRead + Unpin,
    channel: SetupStream,
    sink: &dyn SetupSink,
) -> io::Result<()> {
    let mut bytes = [0; 8192];
    loop {
        let count = stream.read(&mut bytes).await?;
        if count == 0 {
            return Ok(());
        }
        sink.emit(SetupEvent::Output {
            stream: channel,
            bytes: &bytes[..count],
        })?;
    }
}

fn tool_error(error: crate::tools::ToolError) -> io::Error {
    match error {
        crate::tools::ToolError::Failed(message) => io::Error::other(message),
        crate::tools::ToolError::Aborted => {
            io::Error::new(io::ErrorKind::Interrupted, "Setup cancelled")
        }
    }
}

async fn authorize_worktree(
    ctx: &Ctx<'_>,
    explicit: bool,
    request: Request,
    metadata: serde_json::Value,
) -> Result<(), crate::tools::ToolError> {
    if explicit {
        let mut approved = ctx.policy.clone();
        if approved.mode != crate::permissions::Mode::Plan {
            approved.mode = crate::permissions::Mode::Bypass;
        }
        match approved.decide(&request) {
            crate::permissions::Decision::Allow | crate::permissions::Decision::Ask => {
                return Ok(());
            }
            crate::permissions::Decision::Deny(reason) => {
                return Err(crate::tools::ToolError::Failed(format!(
                    "Permission denied: {reason}"
                )));
            }
        }
    }
    let resources = request.resources.clone();
    ctx.authorize(request, resources, metadata).await
}

// Only the manager's lock-entry refusal is retryable; later/unknown outcomes are not.
async fn retry_repository_busy<T, F, Fut>(
    cancel: &CancellationToken,
    mut operation: F,
) -> io::Result<T>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = io::Result<T>>,
{
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        if cancel.is_cancelled() {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "Worktree operation cancelled",
            ));
        }
        let result = operation().await;
        match result {
            Err(error)
                if error.kind() == io::ErrorKind::WouldBlock
                    && error.to_string() == "Worktree repository is busy"
                    && tokio::time::Instant::now() < deadline => {}
            result => return result,
        }
        tokio::select! {
            biased;
            _ = cancel.cancelled() => return Err(io::Error::new(io::ErrorKind::Interrupted, "Worktree operation cancelled")),
            _ = tokio::time::sleep(Duration::from_millis(10)) => {}
        }
    }
}

/// User-approved checkout lifecycle authority does not widen model-tool permissions.
fn authorize_owned_worktree(
    ctx: &Ctx<'_>,
    request: &Request,
) -> Result<(), crate::tools::ToolError> {
    match ctx.policy.user_delegation(request) {
        crate::permissions::Decision::Deny(reason) => Err(crate::tools::ToolError::Failed(
            format!("Permission denied: {reason}"),
        )),
        _ => Ok(()),
    }
}
