//! Fresh checkout and per-server authorization before native/HTTP MCP effects.
use std::path::Path;

use cyber_core::config::{McpServer, McpSettings, Resolved};
use cyber_core::trust::TrustStore;

pub struct ServerSelection {
    pub definition: McpServer,
    pub digest: String,
    pub requires_sandbox: bool,
    pub requires_approval: bool,
}

/// Inspect all contributing leaves; a global command with project-controlled arguments
/// is still project-controlled. Authorization is never cached in the server settings.
pub fn authorize_server(
    resolved: &Resolved,
    trust: &TrustStore,
    location: &Path,
    name: &str,
) -> Result<ServerSelection, String> {
    let selected = inspect_server(resolved, location, name)?;
    if !selected.definition.enabled() {
        return Err("MCP server is disabled".into());
    }
    let checkout = &resolved.trust.checkout_root;
    if selected.requires_approval {
        let approved = match &resolved.trust.digest {
            Some(workspace) => trust.is_approved(checkout, workspace),
            None => Ok(false),
        }
        .map_err(|_| "MCP trust storage unavailable")?;
        if !resolved.trust.trusted || !approved {
            return Err("MCP checkout configuration is untrusted".into());
        }
        if !trust
            .is_mcp_approved(checkout, &selected.digest)
            .map_err(|_| "MCP trust storage unavailable")?
        {
            return Err("MCP server definition digest is untrusted".into());
        }
    }
    Ok(selected)
}

/// Side-effect-free review also includes disabled definitions; it grants no authority.
pub fn inspect_server(
    resolved: &Resolved,
    location: &Path,
    name: &str,
) -> Result<ServerSelection, String> {
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
    let project = provenance(resolved, name)?;
    let requires_sandbox = sandbox_selected(resolved, location)?;
    let digest = definition
        .digest(name)
        .map_err(|_| "MCP server digest unavailable")?;
    Ok(ServerSelection {
        definition,
        digest,
        requires_sandbox,
        requires_approval: project,
    })
}

fn provenance(resolved: &Resolved, name: &str) -> Result<bool, String> {
    let pointer = format!("/mcp/{name}");
    check_origins(&resolved.value["mcp"][name], &pointer, resolved)?;
    let prefix = format!("{pointer}/");
    let mut project = false;
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
        if source.starts_with("project:") {
            project = true;
        } else if !source.starts_with("env:") && !source.starts_with("cli:") {
            return Err("MCP server has unsupported provenance".into());
        }
    }
    if !found {
        return Err("MCP server provenance is absent".into());
    }
    Ok(project)
}

fn sandbox_selected(resolved: &Resolved, location: &Path) -> Result<bool, String> {
    let Some(value) = resolved.value.pointer("/sandbox/apply_to") else {
        return Ok(false);
    };
    let scopes = value
        .as_array()
        .filter(|scopes| scopes.iter().all(serde_json::Value::is_string))
        .ok_or("sandbox.apply_to must be an array of scope names")?;
    let selected = scopes.iter().any(|scope| scope == "mcp");
    Ok(selected
        && cyber_sandbox::SandboxConfig::resolve(
            &resolved.value,
            &resolved.sources,
            None,
            location,
        )
        .policy
            != cyber_sandbox::Policy::FullAccess)
}

fn check_origins(
    value: &serde_json::Value,
    pointer: &str,
    resolved: &Resolved,
) -> Result<(), String> {
    if let Some(fields) = value.as_object().filter(|fields| !fields.is_empty()) {
        for (name, value) in fields {
            let name = name.replace('~', "~0").replace('/', "~1");
            check_origins(value, &format!("{pointer}/{name}"), resolved)?;
        }
    } else if !resolved.sources.contains_key(pointer) {
        return Err("MCP server field provenance is absent".into());
    }
    Ok(())
}
