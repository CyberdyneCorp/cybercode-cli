//! Cancellable readiness observations; native startup remains independently owned.
use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

use cyber_core::config::McpSettings;
use cyber_server::runtime::{McpConnectionStatus, RuntimeError};
use tokio_util::sync::CancellationToken;

use crate::host::BuiltinHost;

impl BuiltinHost {
    fn required_settings(&self, directory: &Path) -> Result<McpSettings, RuntimeError> {
        let value = match self.hook_config.get() {
            Some(config) => (config.resolve)(directory).map(|resolved| resolved.value),
            None => (self.opts.config)(directory).map(|(value, _)| value),
        }
        .map_err(|_| RuntimeError::Invalid("Required MCP configuration unavailable".into()))?;
        McpSettings::from_config(&value)
            .map_err(|_| RuntimeError::Invalid("Required MCP configuration unavailable".into()))
    }

    pub(crate) async fn wait_required_mcp(
        &self,
        directory: &Path,
        cancel: CancellationToken,
    ) -> Result<(), RuntimeError> {
        let mut deadlines = BTreeMap::new();
        loop {
            if cancel.is_cancelled() {
                return Err(RuntimeError::Invalid(
                    "Required MCP wait interrupted".into(),
                ));
            }
            let settings = self.required_settings(directory)?;
            let required: Vec<_> = settings
                .servers
                .iter()
                .filter(|(_, server)| server.required())
                .collect();
            if required.is_empty() {
                return Ok(());
            }
            let snapshot = self
                .mcp_status(directory)
                .map_err(|_| RuntimeError::McpRequired(required[0].0.clone()))?;
            let now = tokio::time::Instant::now();
            let mut waiting = false;
            let mut wake = now + Duration::from_millis(50);
            for (name, server) in required {
                let deadline = *deadlines
                    .entry(name.clone())
                    .or_insert_with(|| now + Duration::from_secs(server.timeout().into()));
                let status = snapshot
                    .iter()
                    .find(|status| status.name == *name && status.configured)
                    .ok_or_else(|| RuntimeError::McpRequired(name.clone()))?;
                let ready = status.status == McpConnectionStatus::Connected
                    && status.connection.as_ref().is_some_and(|connection| {
                        self.mcp_is_published(directory, name, &connection.connection_id)
                    });
                if ready {
                    continue;
                }
                if !matches!(
                    status.status,
                    McpConnectionStatus::Connecting | McpConnectionStatus::Connected
                ) || now >= deadline
                {
                    return Err(RuntimeError::McpRequired(name.clone()));
                }
                waiting = true;
                wake = wake.min(deadline);
            }
            if !waiting {
                return Ok(());
            }
            tokio::select! {
                biased;
                _ = cancel.cancelled() => return Err(RuntimeError::Invalid("Required MCP wait interrupted".into())),
                _ = tokio::time::sleep_until(wake) => {},
            }
        }
    }
}
