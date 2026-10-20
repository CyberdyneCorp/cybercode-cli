//! Catalog and Location lookups for the HTTP API.
mod intelligence;
mod memory;
mod workflows;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use cyber_llm::catalog::{Availability, default_model, recent_models};
use cyber_server::http::remote_tools::RemoteTools;
use cyber_server::http::{AgentInfo, CommandInfo, ModelInfo, Services, SkillCatalog};
use cyber_server::runtime::{CatalogResolver, ToolDef, TurnContext};
use cyber_tools::{BuiltinHost, ConfigFn};
use futures::future::BoxFuture;

pub type HookReviewFn =
    dyn Fn(&Path) -> Result<Vec<cyber_core::config::RawHookSection>, String> + Send + Sync;

/// The defaults and global configuration layers only (`config::load_global`).
pub type GlobalConfigFn = dyn Fn() -> Result<serde_json::Value, String> + Send + Sync;

pub struct AppServices {
    resolver: Arc<CatalogResolver>,
    host: Arc<BuiltinHost>,
    remote: Arc<RemoteTools>,
    config: Arc<ConfigFn>,
    recent_file: PathBuf,
    data: PathBuf,
    cache: PathBuf,
    global_config_dir: PathBuf,
    hook_review: Arc<HookReviewFn>,
    global: Arc<GlobalConfigFn>,
    /// The full resolved configuration (trust and warnings included) of a Location.
    resolved: Arc<cyber_tools::HookConfigFn>,
}

impl AppServices {
    pub fn new(
        resolver: Arc<CatalogResolver>,
        host: Arc<BuiltinHost>,
        remote: Arc<RemoteTools>,
        resolved: Arc<cyber_tools::HookConfigFn>,
        paths: cyber_core::paths::Paths,
        hook_review: Arc<HookReviewFn>,
        global: Arc<GlobalConfigFn>,
    ) -> Self {
        let loader = Arc::clone(&resolved);
        let config: Arc<ConfigFn> = Arc::new(move |location| {
            loader(location).map(|resolved| (resolved.value, resolved.sources))
        });
        Self {
            global,
            resolved,
            resolver,
            host,
            remote,
            config,
            recent_file: paths.state.join("model.json"),
            global_config_dir: paths.config,
            data: paths.data,
            cache: paths.cache,
            hook_review,
        }
    }
}

/// Built-in slash commands handled by clients or the server.
use cyber_core::commands::BUILTIN as BUILTIN_COMMANDS;

impl Services for AppServices {
    fn sandbox_status(&self, location: &Path) -> Option<serde_json::Value> {
        Some(self.host.sandbox_status(location))
    }
    fn permission_test<'a>(
        &'a self,
        location: &'a Path,
        query: serde_json::Value,
    ) -> BoxFuture<'a, Result<serde_json::Value, cyber_server::http::ApiError>> {
        Box::pin(async move {
            let test: cyber_tools::PermissionTest = serde_json::from_value(query)
                .map_err(|error| cyber_server::http::ApiError::invalid(error.to_string()))?;
            let report = self
                .host
                .permission_test(location, &test)
                .await
                .map_err(cyber_server::http::ApiError::invalid)?;
            serde_json::to_value(report)
                .map_err(|error| cyber_server::http::ApiError::unknown(error.to_string()))
        })
    }
    fn transition_lsp(
        &self,
        directory: PathBuf,
        reload: bool,
    ) -> BoxFuture<'_, Result<(), cyber_server::http::ApiError>> {
        Box::pin(async move {
            self.host
                .transition_lsp_location(&directory, reload)
                .await
                .map_err(|_| {
                    cyber_server::http::ApiError::new(
                        axum::http::StatusCode::CONFLICT,
                        "ConflictError",
                        "LSP Location transition is unavailable or unsettled",
                    )
                })
        })
    }
    fn lsp_status(
        &self,
        directory: PathBuf,
    ) -> BoxFuture<'_, Result<Vec<cyber_server::http::LspStatus>, cyber_server::http::ApiError>>
    {
        let host = self.host.clone();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || {
                host.lsp_status(&directory)
                    .map(|rows| {
                        rows.into_iter()
                            .map(|row| cyber_server::http::LspStatus {
                                id: row.id,
                                root: row.root,
                                status: match row.status {
                                    cyber_tools::lsp::ServerState::Starting => {
                                        cyber_server::http::LspState::Starting
                                    }
                                    cyber_tools::lsp::ServerState::Connected => {
                                        cyber_server::http::LspState::Connected
                                    }
                                    cyber_tools::lsp::ServerState::Broken => {
                                        cyber_server::http::LspState::Broken
                                    }
                                },
                            })
                            .collect()
                    })
                    .map_err(|_| cyber_server::http::ApiError::invalid("LSP status is unavailable"))
            })
            .await
            .map_err(|_| {
                cyber_server::http::ApiError::invalid("LSP status observation could not complete")
            })?
        })
    }
    fn formatters(
        &self,
        directory: PathBuf,
    ) -> BoxFuture<'_, Result<Vec<cyber_server::http::FormatterStatus>, cyber_server::http::ApiError>>
    {
        Box::pin(intelligence::formatters(
            self.config.clone(),
            self.cache.clone(),
            directory,
        ))
    }
    fn memory_recovery(
        &self,
        runtime: cyber_server::runtime::Runtime,
        directory: PathBuf,
        scope: cyber_server::http::MemoryScope,
    ) -> BoxFuture<
        '_,
        Result<Option<cyber_server::http::MemoryRecoveryView>, cyber_server::http::ApiError>,
    > {
        Box::pin(memory::recovery(
            self.data.clone(),
            runtime,
            directory,
            scope,
        ))
    }
    fn memory_recover(
        &self,
        runtime: cyber_server::runtime::Runtime,
        confirmation: cyber_server::http::MemoryRecoveryConfirm,
    ) -> BoxFuture<'_, Result<cyber_server::runtime::MemoryChange, cyber_server::http::ApiError>>
    {
        Box::pin(memory::recover(
            self.data.clone(),
            self.config.clone(),
            runtime,
            confirmation,
        ))
    }

    fn memory_edit_review(
        &self,
        directory: PathBuf,
        scope: cyber_server::http::MemoryScope,
        name: String,
    ) -> BoxFuture<'_, Result<cyber_core::memory::MemoryEditReview, cyber_server::http::ApiError>>
    {
        Box::pin(memory::edit_review(
            self.data.clone(),
            directory,
            scope,
            name,
        ))
    }
    fn memory_edit(
        &self,
        runtime: cyber_server::runtime::Runtime,
        edit: cyber_server::http::MemoryEdit,
    ) -> BoxFuture<'_, Result<cyber_server::runtime::MemoryChange, cyber_server::http::ApiError>>
    {
        Box::pin(memory::edit(
            self.data.clone(),
            self.config.clone(),
            runtime,
            edit,
        ))
    }
    fn memory_settings(
        &self,
        directory: PathBuf,
    ) -> BoxFuture<'_, Result<cyber_server::http::MemorySettingsView, cyber_server::http::ApiError>>
    {
        Box::pin(memory::settings(self.config.clone(), directory))
    }
    fn set_memory_settings(
        &self,
        directory: PathBuf,
        state: cyber_core::memory::MemoryState,
    ) -> BoxFuture<'_, Result<cyber_server::http::MemorySettingsView, cyber_server::http::ApiError>>
    {
        Box::pin(memory::set_settings(
            self.config.clone(),
            self.global_config_dir.clone(),
            directory,
            state,
        ))
    }
    fn memory_list(
        &self,
        directory: PathBuf,
        scope: cyber_server::http::MemoryScope,
    ) -> BoxFuture<'_, Result<cyber_core::memory::MemoryCatalog, cyber_server::http::ApiError>>
    {
        Box::pin(memory::list(self.data.clone(), directory, scope))
    }
    fn memory_read(
        &self,
        directory: PathBuf,
        scope: cyber_server::http::MemoryScope,
        name: String,
    ) -> BoxFuture<'_, Result<cyber_core::memory::MemoryDocument, cyber_server::http::ApiError>>
    {
        Box::pin(memory::read(self.data.clone(), directory, scope, name))
    }
    fn mcp_status(
        &self,
        location: &Path,
    ) -> Result<Vec<cyber_server::runtime::McpServerStatus>, cyber_server::http::ApiError> {
        self.host.mcp_status(location).map_err(|error| {
            cyber_core::log::error("mcp", &error, serde_json::json!({"directory":location}));
            cyber_server::http::ApiError::invalid("MCP status configuration is unavailable")
        })
    }
    fn close_mcp(
        &self,
        location: PathBuf,
    ) -> BoxFuture<'_, Result<(), cyber_server::http::ApiError>> {
        Box::pin(async move {
            self.host
                .close_mcp_location(&location)
                .await
                .map_err(|error| {
                    cyber_core::log::error(
                        "mcp",
                        &error,
                        serde_json::json!({"directory":location}),
                    );
                    cyber_server::http::ApiError::conflict(
                        "MCP Location close requires retry or recovery",
                    )
                })
        })
    }

    fn reconnect_mcp(
        &self,
        location: PathBuf,
        name: String,
    ) -> BoxFuture<'_, Result<cyber_server::runtime::McpServerStatus, cyber_server::http::ApiError>>
    {
        use cyber_server::http::ApiError;
        use cyber_tools::mcp::McpReconnectError;
        Box::pin(async move {
            self.host
                .reconnect_mcp(&location, &name)
                .await
                .map_err(|error| match error {
                    McpReconnectError::NotFound(message) => {
                        ApiError::not_found("McpServerNotFoundError", message)
                    }
                    McpReconnectError::Forbidden(message) => ApiError::forbidden(message),
                    McpReconnectError::Conflict(message) => ApiError::conflict(message),
                    McpReconnectError::Unavailable(message) => {
                        cyber_core::log::error(
                            "mcp",
                            &message,
                            serde_json::json!({"directory":location,"server":name}),
                        );
                        ApiError::invalid("MCP reconnect configuration is unavailable")
                    }
                })
        })
    }

    fn plugins(
        &self,
        location: &Path,
    ) -> Result<cyber_core::plugins::PluginList, cyber_server::http::ApiError> {
        self.host
            .plugins(location)
            .map_err(cyber_server::http::ApiError::invalid)
    }

    fn review_hooks(
        &self,
        location: &Path,
    ) -> Result<cyber_core::hooks::HookReview, cyber_server::http::ApiError> {
        let mut review = self
            .host
            .review_hooks(location)
            .map_err(cyber_server::http::ApiError::invalid)?;
        review.withheld_hooks =
            (self.hook_review)(location).map_err(cyber_server::http::ApiError::invalid)?;
        Ok(review)
    }

    fn trust_hook(
        &self,
        location: &Path,
        digest: &str,
    ) -> Result<(), cyber_server::http::ApiError> {
        self.host
            .trust_hook(location, digest)
            .map_err(cyber_server::http::ApiError::invalid)
    }

    fn untrust_hook(
        &self,
        location: &Path,
        digest: &str,
    ) -> Result<bool, cyber_server::http::ApiError> {
        self.host
            .untrust_hook(location, digest)
            .map_err(cyber_server::http::ApiError::invalid)
    }

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
                            created_ms: entry.created_ms,
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
    fn remove_worktree(
        &self,
        directory: PathBuf,
        name: cyber_core::worktrees::Name,
        force: bool,
    ) -> BoxFuture<
        '_,
        Result<cyber_server::http::worktrees::WorktreeInfo, cyber_server::http::ApiError>,
    > {
        Box::pin(async move {
            self.host
                .remove_worktree(
                    &directory,
                    &name,
                    force,
                    tokio_util::sync::CancellationToken::new(),
                )
                .await
                .map(Into::into)
                .map_err(worktree_error)
        })
    }
    fn cleanup_session_worktree(
        &self,
        directory: PathBuf,
        session_id: String,
    ) -> BoxFuture<
        '_,
        Result<cyber_server::http::worktrees::WorktreeCleanup, cyber_server::http::ApiError>,
    > {
        Box::pin(async move {
            let outcome = self
                .host
                .cleanup_session_worktree(
                    &directory,
                    &session_id,
                    tokio_util::sync::CancellationToken::new(),
                )
                .await
                .map_err(worktree_error)?;
            Ok(cyber_server::http::worktrees::WorktreeCleanup {
                worktree: outcome.managed.into(),
                kept: outcome.kept,
                reason: outcome.reason,
            })
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

    /// Read for every MCP serve request, so configuration changes apply to new requests.
    /// An invalid global configuration fails closed.
    fn mcp_serve(
        &self,
    ) -> Result<cyber_core::config::McpServeSettings, cyber_server::http::ApiError> {
        let unavailable = |error: String| {
            cyber_server::http::ApiError::service_unavailable(
                "mcp_serve",
                format!("MCP serve configuration is invalid: {error}"),
            )
        };
        let global = (self.global)().map_err(unavailable)?;
        cyber_core::config::McpServeSettings::from_config(&global).map_err(unavailable)
    }

    /// An invalid configuration (an unknown feature among them) fails the request.
    fn features(
        &self,
        location: &Path,
    ) -> Result<cyber_core::features::Features, cyber_server::http::ApiError> {
        let (config, sources) =
            (self.config)(location).map_err(cyber_server::http::ApiError::invalid)?;
        Ok(cyber_core::features::Features::from_config(
            &config, &sources,
        ))
    }

    /// Project definitions only from a trusted checkout.
    fn team_definitions(
        &self,
        location: &Path,
    ) -> Result<cyber_core::teams::Definitions, cyber_server::http::ApiError> {
        let resolved = (self.resolved)(location).map_err(cyber_server::http::ApiError::invalid)?;
        Ok(cyber_core::teams::definitions(
            location,
            &self.global_config_dir,
            &resolved,
            &cyber_core::env::ProcessEnv,
        ))
    }

    /// Read from the Location's resolved (trust-gated) configuration; an invalid
    /// configuration fails closed.
    fn location_defaults(
        &self,
        location: &Path,
    ) -> Result<cyber_server::http::LocationDefaults, cyber_server::http::ApiError> {
        let (config, _) = (self.config)(location).map_err(cyber_server::http::ApiError::invalid)?;
        let text = |key: &str| {
            config
                .get(key)
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned)
        };
        Ok(cyber_server::http::LocationDefaults {
            mode: text("mode"),
            default_agent: text("default_agent"),
        })
    }

    fn workflow_location(
        &self,
        location: &Path,
    ) -> Result<cyber_server::http::WorkflowLocation, cyber_server::http::ApiError> {
        let (config, _) = (self.config)(location).map_err(cyber_server::http::ApiError::invalid)?;
        workflows::location(&config)
    }

    fn authorize_workflow_run(
        &self,
        owner: &cyber_server::runtime::TurnContext,
        module: &cyber_workflow::WorkflowModule,
    ) -> Result<(), cyber_server::http::ApiError> {
        match self.host.workflow_run_denial(owner, module) {
            Ok(None) => Ok(()),
            Ok(Some(reason)) => {
                Err(cyber_server::http::ApiError::forbidden(reason).with_reason("PermissionDenied"))
            }
            // An unreadable configuration fails closed.
            Err(error) => Err(cyber_server::http::ApiError::invalid(error)),
        }
    }

    fn saved_workflow(
        &self,
        location: &Path,
        name: &str,
    ) -> Result<cyber_workflow::WorkflowModule, cyber_server::http::ApiError> {
        workflows::saved(&self.global_config_dir, location, name)
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
                color: agent.color,
            })
            .collect()
    }

    /// The tools a Turn would offer: built-ins plus client-registered tools.
    fn tools(&self, turn: &TurnContext) -> Vec<ToolDef> {
        crate::host::materialize_tools(&self.host, &self.remote, turn)
    }

    fn commands(&self, location: &Path) -> Vec<CommandInfo> {
        let builtin = BUILTIN_COMMANDS
            .iter()
            .map(|(name, description)| CommandInfo {
                name: (*name).into(),
                description: (*description).into(),
                source: "builtin".into(),
                argument_hint: None,
                namespace: None,
                provenance: None,
            });
        let (bundled, skills): (Vec<_>, Vec<_>) = self
            .host
            .skill_commands(location)
            .into_iter()
            .map(|skill| CommandInfo {
                source: if skill.is_bundled() {
                    "builtin"
                } else {
                    "skill"
                }
                .into(),
                argument_hint: skill
                    .argument_hint
                    .or_else(|| cyber_core::skills::derived_argument_hint(&skill.body)),
                name: skill.name,
                description: skill.description,
                namespace: None,
                provenance: None,
            })
            .partition(|entry| entry.source == "builtin");
        let mut registry = self.host.command_registry(location);
        let bundled = bundled
            .into_iter()
            .filter(|entry| !registry.unavailable.contains(&entry.name));
        let configured = registry.entries.into_iter().map(|(name, command)| {
            let provenance = registry.provenance.remove(&name);
            CommandInfo {
                name,
                description: command.description,
                source: "command".into(),
                argument_hint: command
                    .argument_hint
                    .or_else(|| cyber_core::skills::derived_argument_hint(&command.template)),
                namespace: None,
                provenance,
            }
        });
        // MCP prompts (`/<server>:<prompt>`) replace project commands and yield to skills.
        let prompts = self
            .host
            .mcp_prompt_commands(location)
            .into_iter()
            .map(|command| CommandInfo {
                name: command.name(),
                argument_hint: command.argument_hint(),
                description: command.prompt.description,
                source: "mcp".into(),
                namespace: None,
                provenance: None,
            });
        let mut result = std::collections::BTreeMap::new();
        for mut entry in builtin
            .chain(bundled)
            .chain(configured)
            .chain(prompts)
            .chain(skills)
        {
            if entry.source != "builtin" {
                entry.name = cyber_core::commands::invocation_name(&entry.name);
            }
            entry.namespace = entry
                .name
                .split_once(':')
                .map(|(namespace, _)| namespace.into());
            result.insert(entry.name.clone(), entry);
        }
        result.into_values().collect()
    }

    fn skills(
        &self,
        location: &Path,
        agent: &str,
    ) -> Result<SkillCatalog, cyber_server::http::ApiError> {
        self.host
            .skill_catalog(location, agent)
            .map_err(cyber_server::http::ApiError::invalid)
    }

    fn find_files(&self, location: &Path, query: &str, limit: usize) -> Vec<String> {
        self.host.find_files(location, query, limit)
    }

    fn command_plan<'a>(
        &'a self,
        turn: &'a TurnContext,
        name: &'a str,
        arguments: &'a str,
    ) -> BoxFuture<
        'a,
        Result<Option<cyber_server::runtime::CommandPlan>, cyber_server::runtime::CommandPlanError>,
    > {
        Box::pin(self.host.command_plan(turn, name, arguments))
    }

    fn expand_command(&self, location: &Path, name: &str, arguments: &str) -> Option<String> {
        self.host.expand_command(location, name, arguments)
    }

    fn commit_message<'a>(
        &'a self,
        location: &'a Path,
    ) -> BoxFuture<'a, Result<cyber_server::http::vcs::CommitMessage, cyber_server::http::ApiError>>
    {
        Box::pin(async move {
            let (config, _) =
                (self.config)(location).map_err(cyber_server::http::ApiError::invalid)?;
            let message = crate::git_text::commit_message(&*self.resolver, &config, location)
                .await
                .map_err(git_text_error)?;
            Ok(cyber_server::http::vcs::CommitMessage { message })
        })
    }

    fn pr_description<'a>(
        &'a self,
        location: &'a Path,
        base: Option<String>,
    ) -> BoxFuture<'a, Result<cyber_server::http::vcs::PrDescription, cyber_server::http::ApiError>>
    {
        Box::pin(async move {
            let (config, _) =
                (self.config)(location).map_err(cyber_server::http::ApiError::invalid)?;
            let pr = crate::git_text::pr_description(
                &*self.resolver,
                &config,
                location,
                base.as_deref(),
            )
            .await
            .map_err(git_text_error)?;
            Ok(cyber_server::http::vcs::PrDescription {
                base: pr.base,
                title: pr.title,
                body: pr.body,
            })
        })
    }
}

fn git_text_error(error: crate::git_text::GitTextError) -> cyber_server::http::ApiError {
    use crate::git_text::GitTextError;
    use cyber_server::http::ApiError;
    match error {
        GitTextError::Invalid(message) => ApiError::invalid(message),
        GitTextError::Failed(message) => {
            ApiError::new(axum::http::StatusCode::BAD_GATEWAY, "GitTextError", message)
        }
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
    } else if error.kind() == std::io::ErrorKind::NotFound {
        ApiError::not_found("WorktreeNotFoundError", error.to_string())
    } else {
        ApiError::conflict(error.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::BUILTIN_COMMANDS;

    /// Aliases the TUI accepts that are deliberately not suggested on their own.
    const ALIASES: &[&str] = &["subagents", "models", "sessions", "clear", "quit", "side"];

    /// Every slash command the TUI dispatches itself must be in the catalog that feeds
    /// `/` completion and `GET /commands`, or users can't discover it (`/rename` was missing).
    #[test]
    fn every_tui_slash_command_is_in_the_builtin_catalog() {
        let source = include_str!("../../cyber-tui/src/app.rs");
        let start = source.find("match name {").expect("slash dispatch");
        let body = &source[start..];
        let end = body
            .find("_ => vec![Action::Command")
            .expect("dispatch fallback");
        let mut missing = Vec::new();
        for line in body[..end].lines() {
            let arm = line.trim_start();
            if !arm.starts_with('"') || !arm.contains("=>") {
                continue;
            }
            let pattern = arm.split("=>").next().unwrap_or_default();
            let pattern = pattern.split(" if ").next().unwrap_or_default();
            for name in pattern
                .split('|')
                .filter_map(|p| p.trim().split('"').nth(1))
            {
                let known = BUILTIN_COMMANDS.iter().any(|(n, _)| *n == name);
                if !known && !ALIASES.contains(&name) && name != "review" {
                    missing.push(name.to_owned());
                }
            }
        }
        missing.dedup();
        assert!(
            missing.is_empty(),
            "missing from BUILTIN_COMMANDS: {missing:?}"
        );
    }
}
