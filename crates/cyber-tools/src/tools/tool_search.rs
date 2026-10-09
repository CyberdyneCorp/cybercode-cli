//! Discover full schemas without authorizing or executing the selected tools.
use cyber_server::runtime::{RetrySafety, ToolDef, ToolHost, TurnContext};
use futures::future::BoxFuture;
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::BTreeSet;

use super::{Tool, ToolError, def, failed};
use crate::{host::Ctx, permissions::Request};

pub(crate) struct ToolSearch;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Input {
    query: Option<String>,
    select: Option<Vec<String>>,
    #[serde(default = "default_limit")]
    limit: usize,
}
fn default_limit() -> usize {
    5
}

fn select(mut tools: Vec<ToolDef>, input: &Input) -> Result<Vec<ToolDef>, ToolError> {
    if input.limit > 1000 {
        return Err(failed("Tool search limit exceeds 1000"));
    }
    tools.retain(|tool| tool.scope.deferrable());
    if let Some(names) = &input.select {
        let unique: BTreeSet<_> = names.iter().collect();
        if names.len() > 1000 || unique.len() != names.len() {
            return Err(failed("Invalid tool search selection"));
        }
        for name in names {
            if !tools.iter().any(|tool| &tool.spec.name == name) {
                return Err(failed(format!("Unavailable tool: {name}")));
            }
        }
        tools.retain(|tool| unique.contains(&tool.spec.name));
    } else if let Some(query) = &input.query {
        let words: Vec<_> = query
            .to_lowercase()
            .split_whitespace()
            .map(str::to_owned)
            .collect();
        tools.retain(|tool| {
            let text = format!("{} {}", tool.spec.name, tool.spec.description).to_lowercase();
            words.iter().all(|word| text.contains(word))
        });
    }
    tools.sort_by(|a, b| a.spec.name.cmp(&b.spec.name));
    tools.truncate(input.limit);
    Ok(tools)
}

impl Tool for ToolSearch {
    fn def(&self) -> ToolDef {
        def(
            "tool_search",
            "Search MCP and plugin tools by name or description, or select exact names. Returns full schemas and loads selected tools for subsequent steps in this Session; execution permissions still apply.",
            json!({"type":"object","additionalProperties":false,"properties":{
                "query":{"type":"string"},
                "select":{"type":"array","maxItems":1000,"uniqueItems":true,"items":{"type":"string"}},
                "limit":{"type":"integer","minimum":0,"maximum":1000,"default":5}
            }}),
            RetrySafety::ReadOnly,
            true,
        )
    }

    fn run<'a>(&'a self, ctx: &'a Ctx<'a>) -> BoxFuture<'a, Result<String, ToolError>> {
        Box::pin(async move {
            let input: Input = serde_json::from_value(ctx.inv.input.clone())
                .map_err(|_| failed("Invalid tool search input"))?;
            ctx.authorize(
                Request {
                    action: "tool_search".into(),
                    resources: vec!["*".into()],
                    read_only: true,
                    ..Request::default()
                },
                vec!["*".into()],
                Value::Null,
            )
            .await?;
            if ctx.cancel.is_cancelled() {
                return Err(ToolError::Aborted);
            }
            let turn = TurnContext {
                session_id: ctx.inv.session_id.clone(),
                directory: ctx.inv.directory.clone(),
                agent: ctx.inv.agent.clone(),
                mode: ctx.inv.mode.clone(),
                prefers_apply_patch: false,
                rules: ctx.inv.rules.clone(),
            };
            let runtime = ctx.host.runtime();
            let catalog = match &runtime {
                Some(runtime) => runtime.tool_catalog(&turn),
                None => ctx.host.definitions(&turn),
            };
            let selected = select(catalog, &input)?;
            let specs: Vec<_> = selected.iter().map(|tool| &tool.spec).collect();
            let output = serde_json::to_string_pretty(&json!({"tools":specs}))
                .map_err(|_| failed("Tool search result serialization failed"))?;
            if !selected.is_empty() {
                let runtime = runtime
                    .ok_or_else(|| failed("Tool search requires an attached Session runtime"))?;
                runtime
                    .load_tool_schemas(
                        &ctx.inv.session_id,
                        selected.iter().map(|tool| tool.spec.name.clone()).collect(),
                        &ctx.cancel,
                    )
                    .await
                    .map_err(|error| {
                        if ctx.cancel.is_cancelled() {
                            ToolError::Aborted
                        } else {
                            failed(error.to_string())
                        }
                    })?;
            }
            Ok(output)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cyber_server::runtime::ToolScope;
    fn entry(name: &str, scope: ToolScope) -> ToolDef {
        let mut tool = def(
            name,
            "Jira issue reader",
            json!({"type":"object"}),
            RetrySafety::ReadOnly,
            true,
        );
        tool.scope = scope;
        tool
    }
    #[test]
    fn search_uses_explicit_scope_casefolds_tokens_and_sorts() {
        let tools = vec![
            entry("mcp__jira__z", ToolScope::Mcp),
            entry("mcp__jira__a", ToolScope::Plugin),
            entry("mcp__jira__client", ToolScope::Session),
            entry("native", ToolScope::Builtin),
        ];
        let input = Input {
            query: Some("JIRA reader".into()),
            select: None,
            limit: 5,
        };
        let result = select(tools.clone(), &input).unwrap();
        assert_eq!(
            result
                .iter()
                .map(|tool| tool.spec.name.as_str())
                .collect::<Vec<_>>(),
            ["mcp__jira__a", "mcp__jira__z"]
        );
        let input = Input {
            query: None,
            select: Some(vec!["mcp__jira__client".into()]),
            limit: 5,
        };
        assert!(select(tools, &input).is_err());
    }
    #[test]
    fn default_limit_is_five_and_selection_is_atomic() {
        let tools: Vec<_> = (0..8)
            .map(|i| entry(&format!("plugin_{i}"), ToolScope::Plugin))
            .collect();
        let input: Input = serde_json::from_value(json!({})).unwrap();
        assert_eq!(select(tools.clone(), &input).unwrap().len(), 5);
        let input: Input =
            serde_json::from_value(json!({"select":["plugin_1","missing"]})).unwrap();
        assert!(select(tools.clone(), &input).is_err());
        let input: Input = serde_json::from_value(json!({"limit":0})).unwrap();
        assert!(select(tools.clone(), &input).unwrap().is_empty());
        let input: Input = serde_json::from_value(json!({"limit":1001})).unwrap();
        assert!(select(tools, &input).is_err());
    }
}
