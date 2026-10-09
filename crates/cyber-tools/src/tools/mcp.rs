//! Read-only observations of independently owned MCP startup.
use cyber_server::runtime::{McpConnectionStatus, RetrySafety, ToolDef};
use futures::future::BoxFuture;
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::time::Duration;

use super::{Tool, ToolError, def, failed};
use crate::host::Ctx;
use crate::permissions::Request;

pub(crate) struct WaitForMcp;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Input {
    servers: Vec<String>,
    #[serde(default = "default_timeout")]
    timeout: u64,
}
fn default_timeout() -> u64 {
    60
}

impl Tool for WaitForMcp {
    fn def(&self) -> ToolDef {
        def(
            "wait_for_mcp",
            "Wait up to 60 seconds for named MCP servers that are still connecting. Reports status without starting servers or authorizing their tools; connected tools become available on following Turns.",
            json!({"type":"object","additionalProperties":false,"required":["servers"],"properties":{
                "servers":{"type":"array","minItems":1,"maxItems":64,"uniqueItems":true,
                    "items":{"type":"string","pattern":"^[A-Za-z0-9_-]{1,48}$"}},
                "timeout":{"type":"integer","minimum":0,"maximum":60,"default":60}
            }}),
            RetrySafety::ReadOnly,
            true,
        )
    }

    fn run<'a>(&'a self, ctx: &'a Ctx<'a>) -> BoxFuture<'a, Result<String, ToolError>> {
        Box::pin(async move {
            let input: Input = serde_json::from_value(ctx.inv.input.clone())
                .map_err(|_| failed("Invalid MCP wait input"))?;
            let names: BTreeSet<_> = input.servers.iter().collect();
            if input.timeout > 60 || names.is_empty() || names.len() != input.servers.len() {
                return Err(failed("Invalid MCP wait input"));
            }
            ctx.authorize(
                Request {
                    action: "wait_for_mcp".into(),
                    resources: input.servers.clone(),
                    read_only: true,
                    ..Request::default()
                },
                input.servers.clone(),
                Value::Null,
            )
            .await?;
            let deadline = tokio::time::Instant::now() + Duration::from_secs(input.timeout);
            loop {
                if ctx.cancel.is_cancelled() {
                    return Err(ToolError::Aborted);
                }
                let (servers, connecting) = observations(ctx, &input.servers)?;
                let now = tokio::time::Instant::now();
                if !connecting || now >= deadline {
                    return serde_json::to_string_pretty(&json!({
                        "servers":servers,"timed_out":connecting
                    }))
                    .map_err(|_| failed("MCP wait result serialization failed"));
                }
                tokio::select! {
                    biased;
                    _ = ctx.cancel.cancelled() => return Err(ToolError::Aborted),
                    _ = tokio::time::sleep_until(deadline.min(now + Duration::from_millis(50))) => {},
                }
            }
        })
    }
}

fn observations(ctx: &Ctx<'_>, names: &[String]) -> Result<(Vec<Value>, bool), ToolError> {
    let snapshot = ctx
        .host
        .mcp_status(&ctx.location)
        .map_err(|_| failed("MCP status unavailable; inspect local configuration and logs"))?;
    let mut connecting = false;
    let mut servers = Vec::with_capacity(names.len());
    for name in names {
        let server = snapshot
            .iter()
            .find(|server| &server.name == name && server.configured)
            .ok_or_else(|| failed(format!("Unknown MCP server: {name}")))?;
        connecting |= server.status == McpConnectionStatus::Connecting;
        servers.push(json!({"name":name,"status":server.status,"error":server.error}));
    }
    Ok((servers, connecting))
}
