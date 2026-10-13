//! Prompts of shared connections offered as `/<server>:<prompt>` commands
//! (`mcp` → Prompts as commands).
use std::path::Path;
use std::sync::{Arc, PoisonError};

use serde_json::{Map, Value, json};
use tokio_util::sync::CancellationToken;

use super::{Entry, detached, end_unresolved};
use crate::host::BuiltinHost;
use crate::mcp::{DiscoveredPrompt, McpPromptCommand, PromptMessage};
use crate::tools::ToolError;

/// A server with this name would shadow the `project:` command namespace, so its
/// prompts are never offered.
pub(crate) const RESERVED_SERVER: &str = "project";

impl BuiltinHost {
    /// Prompts of the freshly authorized, connected servers in `directory`, by server name.
    pub fn mcp_prompt_commands(&self, directory: &Path) -> Vec<McpPromptCommand> {
        let Some((directory, resolved)) = self.prompt_location(directory) else {
            return Vec::new();
        };
        let names: Vec<String> = {
            let pool = self.mcp.0.lock().unwrap_or_else(PoisonError::into_inner);
            pool.entries
                .keys()
                .filter(|(location, _)| location == &directory)
                .map(|(_, name)| name.clone())
                .collect()
        };
        let mut commands = Vec::new();
        for name in names {
            let Some(entry) = self.prompt_entry(&directory, &resolved, &name) else {
                continue;
            };
            commands.extend(
                published_prompts(&entry)
                    .into_iter()
                    .map(|prompt| McpPromptCommand {
                        server: name.clone(),
                        prompt,
                    }),
            );
        }
        commands
    }

    /// The connected prompt `prompt` of `server`, if it is offered as a command.
    pub(crate) fn mcp_prompt(
        &self,
        directory: &Path,
        server: &str,
        prompt: &str,
    ) -> Option<DiscoveredPrompt> {
        let (directory, resolved) = self.prompt_location(directory)?;
        let entry = self.prompt_entry(&directory, &resolved, server)?;
        published_prompts(&entry)
            .into_iter()
            .find(|candidate| candidate.name == prompt)
    }

    /// `prompts/get` on the server's shared connection. Authorization and the catalog are
    /// looked up fresh before and after taking the connection, the configured call timeout
    /// bounds it, and a request left unresolved ends the connection as loss. The RPC runs
    /// [`detached`], so a caller dropped mid-request still settles it.
    pub(crate) async fn mcp_get_prompt(
        &self,
        directory: &Path,
        server: &str,
        prompt: &str,
        arguments: &Map<String, Value>,
        cancel: &CancellationToken,
    ) -> Result<Vec<PromptMessage>, ToolError> {
        let host = self
            .weak
            .upgrade()
            .ok_or_else(|| unavailable_prompt(server, prompt))?;
        let (directory, server, prompt) = (
            directory.to_path_buf(),
            server.to_owned(),
            prompt.to_owned(),
        );
        let (arguments, cancel) = (arguments.clone(), cancel.clone());
        detached(async move {
            host.settled_prompt(&directory, &server, &prompt, &arguments, &cancel)
                .await
        })
        .await
    }

    async fn settled_prompt(
        &self,
        directory: &Path,
        server: &str,
        prompt: &str,
        arguments: &Map<String, Value>,
        cancel: &CancellationToken,
    ) -> Result<Vec<PromptMessage>, ToolError> {
        let unavailable = || unavailable_prompt(server, prompt);
        let (canonical, resolved) = self.prompt_location(directory).ok_or_else(unavailable)?;
        let entry = self
            .prompt_entry(&canonical, &resolved, server)
            .ok_or_else(unavailable)?;
        let mut guard = tokio::select! {
            guard = entry.server.lock() => guard,
            _ = cancel.cancelled() => return Err(ToolError::Aborted),
            _ = entry.cancel.cancelled() => return Err(ToolError::Aborted),
        };
        let current = self.prompt_entry(&canonical, &resolved, server);
        if !current.is_some_and(|current| Arc::ptr_eq(&current, &entry)) {
            return Err(unavailable());
        }
        let owner = guard.as_mut().ok_or_else(unavailable)?;
        let timeout = self.mcp_call_timeout(&canonical, server)?;
        let result = tokio::select! {
            result = owner.get_prompt(prompt, arguments, timeout) => {
                result.map_err(|error| ToolError::Failed(error.to_string()))
            }
            _ = cancel.cancelled() => Err(ToolError::Aborted),
            _ = entry.cancel.cancelled() => Err(ToolError::Aborted),
        };
        end_unresolved(&entry, owner).await;
        result
    }

    fn prompt_location(
        &self,
        directory: &Path,
    ) -> Option<(std::path::PathBuf, cyber_core::config::Resolved)> {
        let directory = directory.canonicalize().ok()?;
        let resolved = (self.hook_config.get()?.resolve)(&directory).ok()?;
        Some((directory, resolved))
    }

    /// A live entry whose prompts may be offered: never the reserved `project` server.
    fn prompt_entry(
        &self,
        directory: &Path,
        resolved: &cyber_core::config::Resolved,
        server: &str,
    ) -> Option<Arc<Entry>> {
        let entry = self.live_entry(directory, resolved, server)?;
        if server == RESERVED_SERVER {
            if !published_prompts(&entry).is_empty() {
                cyber_core::log::warn(
                    "mcp",
                    "MCP prompts of a server named project are not offered as commands",
                    json!({"server":server}),
                );
            }
            return None;
        }
        Some(entry)
    }
}

fn published_prompts(entry: &Entry) -> Vec<DiscoveredPrompt> {
    entry
        .published
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .as_ref()
        .map(|published| published.prompts.clone())
        .unwrap_or_default()
}

fn unavailable_prompt(server: &str, prompt: &str) -> ToolError {
    ToolError::Failed(format!(
        "MCP prompt {server}:{prompt} is unavailable: the server is not connected or no longer offers it"
    ))
}
