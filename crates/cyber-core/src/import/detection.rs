//! Definition counts only: no source values, trust grants or execution.
use super::{
    DiscoveryError, DiscoveryIssue, SourceFile, SourceKind, SourceRoots, SourceSnapshot,
    SourceTool, discover_sources,
};
use serde::Serialize;
use serde_json::Value;

#[derive(Debug, Default, Clone, Serialize)]
pub struct DefinitionCounts {
    pub agents: usize,
    pub commands: usize,
    pub skills: usize,
    pub mcp_servers: usize,
    pub hooks: usize,
    /// Session databases/transcripts are not inspected by this implementation.
    pub sessions: Option<usize>,
}
#[derive(Debug, Serialize)]
pub struct DetectedTool {
    pub tool: SourceTool,
    pub files: Vec<SourceFile>,
    /// Raw definitions, including definitions repeated in different layers.
    pub counts: DefinitionCounts,
    pub counts_complete: bool,
    /// Effective compatibility depends on configuration and precedence, not existence.
    pub read_time_coverage: Option<bool>,
}
#[derive(Debug, Serialize)]
pub struct DetectionReport {
    pub tools: Vec<DetectedTool>,
    pub issues: Vec<DiscoveryIssue>,
    pub complete: bool,
}

pub fn detect_sources(roots: &SourceRoots) -> Result<DetectionReport, DiscoveryError> {
    let inventory = discover_sources(roots)?;
    let mut issues = inventory.issues;
    let mut tools = Vec::new();
    let mut remaining = 16 * 1024 * 1024;
    for tool in [SourceTool::OpenCode, SourceTool::Codex, SourceTool::Claude] {
        let files: Vec<_> = inventory
            .files
            .iter()
            .filter(|f| f.tool == tool)
            .cloned()
            .collect();
        if files.is_empty() {
            continue;
        }
        let mut counts = DefinitionCounts::default();
        for source in &files {
            match count_file(roots, source, &inventory.files, &mut remaining, &mut counts) {
                Ok(()) => (),
                Err(error) => issues.push(DiscoveryIssue {
                    path: error.path,
                    reason: error.reason,
                }),
            }
        }
        tools.push(DetectedTool {
            tool,
            files,
            counts,
            // Referenced sources, session counts and effective coverage still need resolution.
            counts_complete: false,
            read_time_coverage: None,
        });
    }
    Ok(DetectionReport {
        tools,
        issues,
        complete: false,
    })
}
fn failure(source: &SourceFile, reason: &'static str) -> DiscoveryError {
    DiscoveryError {
        path: source.path.clone(),
        reason,
    }
}
fn count_file(
    roots: &SourceRoots,
    source: &SourceFile,
    inventory: &[SourceFile],
    remaining: &mut usize,
    counts: &mut DefinitionCounts,
) -> Result<(), DiscoveryError> {
    use SourceKind::*;
    match source.kind {
        Agent => counts.agents += 1,
        Command => counts.commands += 1,
        Skill => counts.skills += 1,
        Config | Profile | Mcp | Hook => {
            let snapshot = SourceSnapshot::read_admitted(roots, source, inventory)?;
            *remaining = remaining
                .checked_sub(snapshot.bytes().len())
                .ok_or_else(|| {
                    failure(
                        source,
                        "detection exceeds the aggregate sixteen MiB parse limit",
                    )
                })?;
            let document = parse_document(source, snapshot.text()?)?;
            let delta =
                count_document(source.tool, &document).map_err(|reason| failure(source, reason))?;
            snapshot.verify()?;
            counts.agents += delta.agents;
            counts.commands += delta.commands;
            counts.mcp_servers += delta.mcp_servers;
            counts.hooks += delta.hooks;
        }
        _ => (),
    }
    Ok(())
}
fn parse_document(source: &SourceFile, text: &str) -> Result<Value, DiscoveryError> {
    let value = if source.path.extension().is_some_and(|e| e == "toml") {
        let table = text
            .parse::<toml::Table>()
            .map_err(|_| failure(source, "invalid source TOML"))?;
        serde_json::to_value(table)
            .map_err(|_| failure(source, "source TOML cannot be represented"))?
    } else {
        crate::config::parse_jsonc("import source", text)
            .map_err(|_| failure(source, "invalid source JSON/JSONC"))?
    };
    if !value.is_object() {
        return Err(failure(source, "source configuration must be an object"));
    }
    Ok(value)
}
fn map_count(value: Option<&Value>) -> Result<usize, &'static str> {
    match value {
        None => Ok(0),
        Some(Value::Object(map)) => Ok(map.len()),
        _ => Err("source definition collection must be an object"),
    }
}
fn aliases(document: &Value, first: &str, second: &str) -> Result<usize, &'static str> {
    // Count raw definitions in both supported source shapes; do not pretend to resolve them.
    Ok(map_count(document.get(first))? + map_count(document.get(second))?)
}
fn hooks_count(value: Option<&Value>) -> Result<usize, &'static str> {
    let Some(value) = value else {
        return Ok(0);
    };
    let events = value.as_object().ok_or("source hooks must be an object")?;
    let mut count = 0;
    for groups in events.values() {
        for group in groups
            .as_array()
            .ok_or("source hook event must be an array")?
        {
            let handlers = group
                .get("hooks")
                .and_then(Value::as_array)
                .ok_or("source hook group must contain a hooks array")?;
            if handlers.iter().any(|h| !h.is_object()) {
                return Err("source hook handler must be an object");
            }
            count += handlers.len();
        }
    }
    Ok(count)
}
fn count_document(tool: SourceTool, document: &Value) -> Result<DefinitionCounts, &'static str> {
    let mut counts = DefinitionCounts::default();
    match tool {
        SourceTool::Claude => {
            counts.mcp_servers = map_count(document.get("mcpServers"))?;
            counts.hooks = hooks_count(document.get("hooks"))?;
        }
        SourceTool::Codex => {
            counts.mcp_servers = map_count(document.get("mcp_servers"))?;
            if let Some(agents) = document.get("agents") {
                let agents = agents
                    .as_object()
                    .ok_or("source agents must be an object")?;
                counts.agents = agents.values().filter(|v| v.is_object()).count();
            }
            counts.hooks =
                hooks_count(document.get("hooks"))? + usize::from(document.get("notify").is_some());
        }
        SourceTool::OpenCode => {
            counts.agents = aliases(document, "agent", "agents")?;
            counts.commands = aliases(document, "command", "commands")?;
            if let Some(mcp) = document.get("mcp") {
                counts.mcp_servers = map_count(Some(mcp.get("servers").unwrap_or(mcp)))?;
            }
        }
    }
    Ok(counts)
}
