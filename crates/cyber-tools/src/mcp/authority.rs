//! Fresh checkout and per-server authorization before native/HTTP MCP effects.
use std::path::Path;

use cyber_core::config::{McpServer, McpSettings, Resolved};
use cyber_core::trust::TrustStore;

pub struct AuthorizedServer {
    pub definition: McpServer,
    pub digest: String,
    pub requires_sandbox: bool,
}

/// Inspect all contributing leaves; a global command with project-controlled arguments
/// is still project-controlled. Authorization is never cached in the server settings.
pub fn authorize_server(
    resolved: &Resolved,
    trust: &TrustStore,
    location: &Path,
    name: &str,
) -> Result<AuthorizedServer, String> {
    let checkout = std::fs::canonicalize(cyber_core::config::project_root(location))
        .map_err(|_| "MCP checkout identity unavailable")?;
    if checkout != resolved.trust.checkout_root {
        return Err("MCP Location does not match resolved checkout".into());
    }
    let mut settings = McpSettings::from_config(&resolved.value)?;
    let definition = settings
        .servers
        .remove(name)
        .ok_or("MCP server is absent from loaded configuration")?;
    if !definition.enabled() {
        return Err("MCP server is disabled".into());
    }
    let (project, requires_sandbox) = provenance(resolved, name)?;
    let digest = definition
        .digest(name)
        .map_err(|_| "MCP server digest unavailable")?;
    if project {
        let approved = match &resolved.trust.digest {
            Some(workspace) => trust.is_approved(&checkout, workspace),
            None => Ok(false),
        }
        .map_err(|_| "MCP trust storage unavailable")?;
        if !resolved.trust.trusted || !approved {
            return Err("MCP checkout configuration is untrusted".into());
        }
        if !trust
            .is_mcp_approved(&checkout, &digest)
            .map_err(|_| "MCP trust storage unavailable")?
        {
            return Err("MCP server definition digest is untrusted".into());
        }
    }
    Ok(AuthorizedServer {
        definition,
        digest,
        requires_sandbox,
    })
}

fn provenance(resolved: &Resolved, name: &str) -> Result<(bool, bool), String> {
    let pointer = format!("/mcp/{name}");
    let prefix = format!("{pointer}/");
    let mut project = false;
    let mut requires_sandbox = false;
    let mut found = false;
    for (key, source) in &resolved.sources {
        if key != &pointer && !key.starts_with(&prefix) {
            continue;
        }
        found = true;
        if !resolved.layers.contains(source) {
            return Err("MCP server has unknown provenance".into());
        }
        if source.starts_with("global:") {
            continue;
        }
        requires_sandbox = true;
        if source.starts_with("project:") {
            project = true;
        } else if !source.starts_with("env:") && !source.starts_with("cli:") {
            return Err("MCP server has unsupported provenance".into());
        }
    }
    if !found {
        return Err("MCP server provenance is absent".into());
    }
    Ok((project, requires_sandbox))
}
