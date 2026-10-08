//! Catalog and Location lookups for the HTTP API.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use cyber_llm::catalog::{Availability, default_model, recent_models};
use cyber_server::http::remote_tools::RemoteTools;
use cyber_server::http::{AgentInfo, CommandInfo, ModelInfo, Services};
use cyber_server::runtime::{CatalogResolver, ToolDef, ToolHost, TurnContext};
use cyber_tools::{BuiltinHost, ConfigFn};
use futures::future::BoxFuture;

pub struct AppServices {
    resolver: Arc<CatalogResolver>,
    host: Arc<BuiltinHost>,
    remote: Arc<RemoteTools>,
    config: Arc<ConfigFn>,
    recent_file: PathBuf,
    data: PathBuf,
}

impl AppServices {
    pub fn new(
        resolver: Arc<CatalogResolver>,
        host: Arc<BuiltinHost>,
        remote: Arc<RemoteTools>,
        config: Arc<ConfigFn>,
        recent_file: PathBuf,
        data: PathBuf,
    ) -> Self {
        Self {
            resolver,
            host,
            remote,
            config,
            recent_file,
            data,
        }
    }
}

/// Built-in slash commands handled by clients or the server.
const BUILTIN_COMMANDS: &[(&str, &str)] = &[
    ("compact", "Summarize older history to free context"),
    ("model", "Switch the model"),
    ("mode", "Switch the permission mode"),
    (
        "approve",
        "Confirm and replay the last classifier-blocked call once",
    ),
    ("resume", "Open another Session"),
    ("new", "Start a new Session"),
    ("rewind", "Rewind code and conversation to a message"),
    (
        "subtask",
        "Fork a background child with the current context",
    ),
    ("tasks", "List background tasks and open or stop a child"),
    (
        "hooks",
        "Inspect recorded hook executions with /hooks history",
    ),
    ("ps", "List background tasks"),
    ("stop", "Confirm stopping this Session's background tasks"),
    ("help", "Show commands and keys"),
    ("exit", "Quit"),
];

impl Services for AppServices {
    fn inspect_child_setup(
        &self,
        parent: String,
        child: String,
    ) -> BoxFuture<
        '_,
        Result<cyber_server::worktrees::ChildSetupInspection, cyber_server::http::ApiError>,
    > {
        Box::pin(async move {
            self.host
                .inspect_child_worktree_setup(
                    &parent,
                    &child,
                    tokio_util::sync::CancellationToken::new(),
                )
                .await
                .map_err(worktree_error)
        })
    }
    fn recover_child_setup(
        &self,
        parent: String,
        child: String,
        review: cyber_server::worktrees::SetupRecoveryRequest,
    ) -> BoxFuture<
        '_,
        Result<cyber_server::http::worktrees::StartedWorktree, cyber_server::http::ApiError>,
    > {
        Box::pin(async move {
            use cyber_core::worktrees::SetupOutcome;
            use cyber_server::http::worktrees::{SetupStatus, StartedWorktree};
            let result = self
                .host
                .recover_child_worktree_setup(
                    &parent,
                    &child,
                    review,
                    tokio_util::sync::CancellationToken::new(),
                )
                .await
                .map_err(worktree_error)?;
            let setup = match result.setup {
                Ok(SetupOutcome::Completed) => SetupStatus::Completed,
                Ok(SetupOutcome::Failed { index, code }) => SetupStatus::Failed { index, code },
                Err(message) => SetupStatus::Error { message },
            };
            Ok(StartedWorktree {
                worktree: result.managed.into(),
                session: result.session,
                setup,
            })
        })
    }
    fn list_worktrees(
        &self,
        directory: PathBuf,
    ) -> BoxFuture<
        '_,
        Result<Vec<cyber_server::http::worktrees::WorktreeEntry>, cyber_server::http::ApiError>,
    > {
        Box::pin(async move {
            use cyber_core::worktrees::ListedWorktree;
            use cyber_server::http::worktrees::WorktreeEntry;
            let listings = self
                .host
                .list_worktrees(&directory, tokio_util::sync::CancellationToken::new())
                .await
                .map_err(worktree_error)?;
            listings
                .into_iter()
                .map(|entry| match entry.ownership {
                    ListedWorktree::Ready(managed) => {
                        let status = entry.status.ok_or_else(|| {
                            cyber_server::http::ApiError::conflict("Worktree status is unavailable")
                        })?;
                        Ok(WorktreeEntry::Ready {
                            worktree: managed.into(),
                            dirty: status.dirty,
                            ahead: status.ahead,
                            behind: status.behind,
                            sessions: Vec::new(),
                        })
                    }
                    ListedWorktree::Pending(managed) => Ok(WorktreeEntry::Pending {
                        worktree: managed.into(),
                    }),
                    ListedWorktree::Invalid { name, error } => Ok(WorktreeEntry::Invalid {
                        name,
                        message: error,
                    }),
                })
                .collect()
        })
    }
    fn create_worktree(
        &self,
        directory: PathBuf,
        session: cyber_server::runtime::CreateSession,
        name: cyber_core::worktrees::Name,
        call_id: String,
    ) -> BoxFuture<
        '_,
        Result<cyber_server::http::worktrees::StartedWorktree, cyber_server::http::ApiError>,
    > {
        Box::pin(async move {
            use cyber_core::worktrees::SetupOutcome;
            use cyber_server::http::worktrees::{SetupStatus, StartedWorktree};
            let request = cyber_tools::WorktreeSessionRequest {
                project_id: cyber_core::project::identify(&directory).id,
                session,
                data: self.data.clone(),
                name,
            };
            let result = self
                .host
                .start_worktree_session(
                    &directory,
                    call_id,
                    tokio_util::sync::CancellationToken::new(),
                    request,
                )
                .await
                .map_err(worktree_error)?;
            let setup = match result.setup {
                Ok(SetupOutcome::Completed) => SetupStatus::Completed,
                Ok(SetupOutcome::Failed { index, code }) => SetupStatus::Failed { index, code },
                Err(message) => SetupStatus::Error { message },
            };
            Ok(StartedWorktree {
                worktree: result.managed.into(),
                session: result.session,
                setup,
            })
        })
    }

    fn models(&self, _location: &Path) -> BoxFuture<'_, Result<Vec<ModelInfo>, String>> {
        Box::pin(async move {
            let rows = self
                .resolver
                .catalog()
                .list(None)
                .map_err(|e| e.to_string())?;
            Ok(rows
                .into_iter()
                .map(|row| ModelInfo {
                    provider: row.model.provider_id.clone(),
                    name: row.model.name.clone(),
                    available: matches!(row.availability, Availability::Available),
                    context_limit: row.model.limits.context,
                    reasoning: row.model.capabilities.reasoning,
                    id: row.model_ref,
                })
                .collect())
        })
    }

    fn default_model(&self, location: &Path) -> Option<String> {
        let config = (self.config)(location).map(|(v, _)| v).unwrap_or_default();
        let recent = recent_models(&self.recent_file);
        default_model(self.resolver.catalog(), &config, &recent)
            .ok()
            .map(|(r, _)| r.to_string())
    }

    fn agents(&self, location: &Path) -> Vec<AgentInfo> {
        let Ok((config, _)) = (self.config)(location) else {
            return Vec::new();
        };
        let Ok(profiles) = cyber_core::config::resolve_agents(&config) else {
            return Vec::new();
        };
        let mut profiles: Vec<_> = profiles
            .into_values()
            .filter(|agent| !agent.hidden)
            .collect();
        profiles.sort_by(|a, b| (!a.builtin, &a.name).cmp(&(!b.builtin, &b.name)));
        profiles
            .into_iter()
            .map(|agent| AgentInfo {
                name: agent.name,
                description: agent.description,
                mode: agent.mode,
            })
            .collect()
    }

    /// The tools a Turn would offer: built-ins plus client-registered tools.
    fn tools(&self, turn: &TurnContext) -> Vec<ToolDef> {
        let mut defs = self.host.definitions(turn);
        defs.extend(
            self.host
                .filter_agent_tools(turn, self.remote.definitions()),
        );
        defs
    }

    fn commands(&self, location: &Path) -> Vec<CommandInfo> {
        let builtin = BUILTIN_COMMANDS
            .iter()
            .map(|(name, description)| CommandInfo {
                name: (*name).into(),
                description: (*description).into(),
                source: "builtin".into(),
                argument_hint: None,
            });
        let skills =
            self.host
                .skill_commands(location)
                .into_iter()
                .map(|(name, description, hint)| CommandInfo {
                    name,
                    description,
                    source: "skill".into(),
                    argument_hint: hint,
                });
        builtin.chain(skills).collect()
    }

    fn find_files(&self, location: &Path, query: &str, limit: usize) -> Vec<String> {
        self.host.find_files(location, query, limit)
    }

    fn expand_command(&self, location: &Path, name: &str, arguments: &str) -> Option<String> {
        self.host.expand_skill(location, name, arguments)
    }
}

fn worktree_error(error: std::io::Error) -> cyber_server::http::ApiError {
    use cyber_server::http::ApiError;
    if error.kind() == std::io::ErrorKind::PermissionDenied
        || error.to_string().contains("Permission denied")
    {
        ApiError::forbidden(error.to_string())
    } else if error.kind() == std::io::ErrorKind::InvalidInput {
        ApiError::invalid(error.to_string())
    } else {
        ApiError::conflict(error.to_string())
    }
}
