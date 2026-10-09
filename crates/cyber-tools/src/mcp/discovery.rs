//! Initial tool discovery with bounded pagination and stable model-visible identities.
use std::collections::{BTreeMap, HashSet};
use std::time::Duration;

use cyber_core::config::McpToolFilter;
use cyber_llm::ToolSpec;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncRead, AsyncWrite};

use super::{McpError, StdioClient};

const CATALOG_LIMIT: usize = 16 * super::LIMIT;

#[derive(Debug, Clone)]
pub struct DiscoveredTool {
    pub remote_name: String,
    pub exposed_name: String,
    /// The complete server definition, including annotations and extension fields.
    pub definition: Value,
}

impl DiscoveredTool {
    pub fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: self.exposed_name.clone(),
            description: self.definition["description"]
                .as_str()
                .unwrap_or_default()
                .into(),
            input_schema: self.definition["inputSchema"].clone(),
        }
    }

    pub fn read_only_hint(&self) -> bool {
        self.definition["annotations"]["readOnlyHint"] == true
    }

    pub fn destructive_hint(&self) -> Option<bool> {
        self.definition["annotations"]["destructiveHint"].as_bool()
    }
}

/// Keep an exposed name bound to one remote identity for this connection's lifetime.
#[derive(Default)]
pub(crate) struct ToolAliases(BTreeMap<String, String>);

impl ToolAliases {
    pub fn retain(&mut self, server: &str, tools: &mut [DiscoveredTool]) {
        let mut historical: HashSet<String> = self.0.values().cloned().collect();
        let mut used: HashSet<String> = historical
            .iter()
            .cloned()
            .chain(tools.iter().map(|tool| tool.exposed_name.clone()))
            .collect();
        let mut indexes: Vec<_> = (0..tools.len()).collect();
        indexes.sort_by(|a, b| tools[*a].remote_name.cmp(&tools[*b].remote_name));
        for index in indexes {
            let tool = &mut tools[index];
            if let Some(name) = self.0.get(&tool.remote_name) {
                tool.exposed_name = name.clone();
                continue;
            }
            if historical.contains(&tool.exposed_name) {
                tool.exposed_name = unique_name(server, &tool.remote_name, &mut used);
            }
            historical.insert(tool.exposed_name.clone());
            self.0
                .insert(tool.remote_name.clone(), tool.exposed_name.clone());
        }
    }
}

/// Invalid characters become underscores; long identities have a stable hash suffix.
pub fn exposed_tool_name(server: &str, tool: &str) -> String {
    let original = format!("mcp__{server}__{tool}");
    let mut name: String = original
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '_' | '-') {
                c
            } else {
                '_'
            }
        })
        .collect();
    if name.len() > 64 {
        name = hashed_tool_name(server, tool, 0);
    }
    name
}

fn hashed_tool_name(server: &str, tool: &str, attempt: usize) -> String {
    let original = format!("mcp__{server}__{tool}");
    let identity = if attempt == 0 {
        original.clone()
    } else {
        format!("{original}#{attempt}")
    };
    let hash = format!("{:x}", Sha256::digest(identity.as_bytes()));
    let mut name: String = original
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '_' | '-') {
                c
            } else {
                '_'
            }
        })
        .take(57)
        .collect();
    name.push('_');
    name.push_str(&hash[..6]);
    name
}

pub(crate) async fn discover<R: AsyncRead + Unpin, W: AsyncWrite + Unpin>(
    client: &mut StdioClient<R, W>,
    server: &str,
    filter: &McpToolFilter,
    timeout: Duration,
) -> Result<Vec<DiscoveredTool>, McpError> {
    if client.metadata().is_none() {
        return Err(McpError::Protocol("MCP server is not initialized"));
    }
    if timeout.is_zero() {
        return Err(McpError::Timeout);
    }
    if !client.supports("tools") {
        return Ok(Vec::new());
    }
    let deadline = tokio::time::Instant::now() + timeout;
    let mut catalog = Catalog::default();
    let mut cursor = None;
    loop {
        let page = tokio::time::timeout_at(deadline, client.list_tools(cursor.as_deref(), timeout))
            .await
            .map_err(|_| McpError::Timeout)??;
        cursor = catalog.page(page, server, filter)?;
        if cursor.is_none() {
            return Ok(catalog.finish(server));
        }
    }
}

#[derive(Default)]
struct Catalog {
    tools: Vec<DiscoveredTool>,
    names: HashSet<String>,
    cursors: HashSet<String>,
    bytes: usize,
}

impl Catalog {
    fn finish(mut self, server: &str) -> Vec<DiscoveredTool> {
        let mut groups: BTreeMap<String, Vec<usize>> = BTreeMap::new();
        for (index, tool) in self.tools.iter().enumerate() {
            groups
                .entry(tool.exposed_name.clone())
                .or_default()
                .push(index);
        }
        let mut used: HashSet<String> = groups
            .iter()
            .filter(|(_, indexes)| indexes.len() == 1)
            .map(|(name, _)| name.clone())
            .collect();
        for mut indexes in groups.into_values().filter(|indexes| indexes.len() > 1) {
            indexes.sort_by(|a, b| self.tools[*a].remote_name.cmp(&self.tools[*b].remote_name));
            for index in indexes {
                let name = unique_name(server, &self.tools[index].remote_name, &mut used);
                self.tools[index].exposed_name = name;
            }
        }
        self.tools
    }

    fn page(
        &mut self,
        page: Value,
        server: &str,
        filter: &McpToolFilter,
    ) -> Result<Option<String>, McpError> {
        self.bytes += serde_json::to_vec(&page)
            .map_err(|_| McpError::Protocol("invalid tool listing"))?
            .len();
        if self.bytes > CATALOG_LIMIT {
            return Err(McpError::Protocol("MCP tool catalog exceeds byte limit"));
        }
        let tools = page["tools"]
            .as_array()
            .ok_or(McpError::Protocol("MCP tool listing requires tools array"))?;
        for definition in tools {
            let tool = parse_tool(server, definition.clone())?;
            if !self.names.insert(tool.remote_name.clone()) {
                return Err(McpError::Protocol("duplicate MCP tool name"));
            }
            if !filter
                .permits(&tool.remote_name)
                .map_err(|_| McpError::Protocol("invalid MCP tool filter"))?
            {
                continue;
            }
            self.tools.push(tool);
        }
        let Some(next) = page.get("nextCursor") else {
            return Ok(None);
        };
        let cursor = next
            .as_str()
            .ok_or(McpError::Protocol("invalid MCP tool listing cursor"))?;
        if !self.cursors.insert(cursor.into()) {
            return Err(McpError::Protocol("repeated MCP tool listing cursor"));
        }
        Ok(Some(cursor.into()))
    }
}

fn unique_name(server: &str, tool: &str, used: &mut HashSet<String>) -> String {
    for attempt in 0.. {
        let name = hashed_tool_name(server, tool, attempt);
        if used.insert(name.clone()) {
            return name;
        }
    }
    unreachable!("unbounded name search");
}

fn parse_tool(server: &str, mut definition: Value) -> Result<DiscoveredTool, McpError> {
    let name = definition["name"]
        .as_str()
        .filter(|name| !name.trim().is_empty())
        .ok_or(McpError::Protocol("invalid MCP tool name"))?
        .to_owned();
    if !definition["inputSchema"].is_object()
        || definition
            .get("description")
            .is_some_and(|value| !value.is_string())
        || definition
            .get("annotations")
            .is_some_and(|value| !value.is_object())
    {
        return Err(McpError::Protocol("invalid MCP tool definition"));
    }
    for hint in [
        "readOnlyHint",
        "destructiveHint",
        "idempotentHint",
        "openWorldHint",
    ] {
        if definition["annotations"]
            .get(hint)
            .is_some_and(|value| !value.is_boolean())
        {
            return Err(McpError::Protocol("invalid MCP tool annotation"));
        }
    }
    definition["inputSchema"]["type"] = json!("object");
    Ok(DiscoveredTool {
        exposed_name: exposed_tool_name(server, &name),
        remote_name: name,
        definition,
    })
}
