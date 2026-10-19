//! `cyber browser health`: start the configured browser server once, outside any
//! Session, under the same trust, approval and sandbox rules as a Session's browser, and
//! report the tools it offers against the ones verification needs.

use std::path::Path;

use cyber_core::browser::{BrowserSettings, REQUIRED_TOOLS};
use cyber_core::config::Resolved;
use cyber_core::trust::TrustStore;
use serde::Serialize;

use crate::mcp::{LocalLauncher, authorize_server};

#[derive(Debug, Clone, Serialize)]
pub struct BrowserHealth {
    /// The MCP server that is the browser.
    pub server: String,
    /// Whether its definition is the pinned one `cyber browser install` writes.
    pub pinned: bool,
    pub tools: Vec<String>,
    /// Required tools the server does not offer.
    pub missing: Vec<String>,
}

impl BrowserHealth {
    pub fn healthy(&self) -> bool {
        self.missing.is_empty()
    }
}

/// Where and as whom the check runs.
pub struct HealthCheck<'a> {
    pub resolved: &'a Resolved,
    pub trust: &'a TrustStore,
    pub location: &'a Path,
    pub home: &'a Path,
    /// `<data>/browser`: the check's profile lives under its `profiles/`.
    pub browser_root: &'a Path,
    pub helper: Option<&'a Path>,
}

pub async fn browser_health(check: HealthCheck<'_>) -> Result<BrowserHealth, String> {
    let settings = BrowserSettings::from_config(&check.resolved.value)?
        .ok_or("browser support is off: run cyber browser install")?;
    let selection = authorize_server(
        check.resolved,
        check.trust,
        check.location,
        &settings.server,
    )
    .map_err(|error| format!("browser server {}: {error}", settings.server))?;
    let pinned = serde_json::to_value(&selection.definition).ok()
        == serde_json::from_value::<cyber_core::config::McpServer>(
            cyber_core::browser::pinned_definition(),
        )
        .ok()
        .and_then(|pinned| serde_json::to_value(pinned).ok());
    let profile =
        super::profiles::Profile::new(check.browser_root, &cyber_core::ids::new_id("health"));
    let temp_dir = profile.create()?;
    let launcher = LocalLauncher {
        resolved: check.resolved,
        trust: check.trust,
        location: check.location,
        home: check.home,
        temp_dir: &temp_dir,
        helper: check.helper,
        credential_env_names: &[],
        credentials: None,
    };
    let mut server = launcher
        .connect(&settings.server, || Ok(()))
        .await
        .map_err(|error| {
            format!(
                "browser server {} did not start: {}",
                settings.server, error.diagnostic
            )
        })?;
    let tools: Vec<String> = server
        .tools()
        .iter()
        .map(|tool| tool.remote_name.clone())
        .collect();
    let (acknowledged, _) = server.shutdown_gracefully().await;
    if acknowledged {
        profile.remove();
    }
    let missing = REQUIRED_TOOLS
        .iter()
        .filter(|name| !tools.iter().any(|tool| tool == *name))
        .map(|name| (*name).to_string())
        .collect();
    Ok(BrowserHealth {
        server: settings.server,
        pinned,
        tools,
        missing,
    })
}
