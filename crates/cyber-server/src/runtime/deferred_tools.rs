//! Partition an already effective, permission-visible catalog for model projection.
use super::ToolDef;
use cyber_core::config::DeferredToolSettings;
use serde_json::json;
use std::collections::BTreeSet;

#[derive(Debug)]
pub struct DeferredCatalog {
    pub callable: Vec<ToolDef>,
    /// Full definitions remain server-side for search and stale-call checks.
    pub deferred: Vec<ToolDef>,
    pub estimated_tokens: usize,
}
impl DeferredCatalog {
    /// Schema-free, one-line descriptions; JSON escaping preserves frame boundaries.
    pub fn summary(&self) -> Result<String, String> {
        let summaries: Vec<_> = self
            .deferred
            .iter()
            .map(|tool| {
                let description = tool
                    .spec
                    .description
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ");
                json!({"name":tool.spec.name,"description":description})
            })
            .collect();
        serde_json::to_string(&summaries)
            .map_err(|_| "Deferred summary serialization failed".into())
    }
}

/// Apply only after scope precedence, Mode and permission omissions are resolved.
/// Loading schemas never grants execution permission or replaces registration identity.
pub fn materialize(
    tools: Vec<ToolDef>,
    settings: DeferredToolSettings,
    loaded: &BTreeSet<String>,
) -> Result<DeferredCatalog, String> {
    let specs: Vec<_> = tools.iter().map(|tool| &tool.spec).collect();
    let encoded = serde_json::to_string(&specs).map_err(|_| "Tool catalog serialization failed")?;
    let estimated_tokens = encoded.chars().count().div_ceil(4);
    let should_defer = estimated_tokens > settings.threshold_tokens;
    let (deferred, callable) = tools.into_iter().partition(|tool| {
        should_defer && tool.scope.deferrable() && !loaded.contains(&tool.spec.name)
    });
    Ok(DeferredCatalog {
        callable,
        deferred,
        estimated_tokens,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::{RetrySafety, ToolScope};
    use cyber_llm::ToolSpec;
    fn tool(name: &str, scope: ToolScope) -> ToolDef {
        ToolDef {
            scope,
            registration: Some(format!("reg_{name}")),
            spec: ToolSpec {
                name: name.into(),
                description: "first line\nsecond\tline".into(),
                input_schema: json!({"type":"object","properties":{"private_schema":{"type":"string"}}}),
            },
            retry_safety: RetrySafety::Never,
            concurrency_safe: false,
        }
    }
    #[test]
    fn scope_not_namespace_controls_deferral_and_preserves_metadata() {
        let catalog = materialize(
            vec![
                tool("native", ToolScope::Builtin),
                tool("plugin", ToolScope::Plugin),
                tool("mcp__shared__read", ToolScope::Session),
                tool("ordinary_name", ToolScope::Mcp),
            ],
            DeferredToolSettings {
                threshold_tokens: 0,
            },
            &BTreeSet::new(),
        )
        .unwrap();
        assert_eq!(
            catalog
                .callable
                .iter()
                .map(|t| t.spec.name.as_str())
                .collect::<Vec<_>>(),
            vec!["native", "mcp__shared__read"]
        );
        assert_eq!(catalog.deferred.len(), 2);
        assert_eq!(
            catalog.deferred[1].registration.as_deref(),
            Some("reg_ordinary_name")
        );
        assert_eq!(catalog.deferred[1].spec.input_schema["type"], "object");
        let summary = catalog.summary().unwrap();
        assert!(!summary.contains("private_schema"));
        assert!(!summary.contains("reg_ordinary_name"));
        assert!(!summary.contains('\n'));
        assert!(summary.contains("first line second line"));
    }
    #[test]
    fn exact_boundary_is_full_and_loaded_names_remain_full_above_it() {
        let tools = vec![tool("a", ToolScope::Mcp), tool("b", ToolScope::Plugin)];
        let count = materialize(
            tools.clone(),
            DeferredToolSettings::default(),
            &BTreeSet::new(),
        )
        .unwrap()
        .estimated_tokens;
        assert!(
            materialize(
                tools.clone(),
                DeferredToolSettings {
                    threshold_tokens: count
                },
                &BTreeSet::new()
            )
            .unwrap()
            .deferred
            .is_empty()
        );
        let catalog = materialize(
            tools,
            DeferredToolSettings {
                threshold_tokens: count - 1,
            },
            &BTreeSet::from(["a".into()]),
        )
        .unwrap();
        assert_eq!(catalog.callable[0].spec.name, "a");
        assert_eq!(catalog.deferred[0].spec.name, "b");
    }
    #[test]
    fn unicode_uses_characters_and_empty_catalog_is_not_deferred() {
        let mut entry = tool("a", ToolScope::Mcp);
        entry.spec.description = "🦀é界".into();
        let encoded = serde_json::to_string(&vec![&entry.spec]).unwrap();
        let catalog = materialize(
            vec![entry],
            DeferredToolSettings::default(),
            &BTreeSet::new(),
        )
        .unwrap();
        assert_eq!(
            catalog.estimated_tokens,
            encoded.chars().count().div_ceil(4)
        );
        assert!(catalog.estimated_tokens < encoded.len().div_ceil(4));
        let empty = materialize(
            vec![],
            DeferredToolSettings {
                threshold_tokens: 0,
            },
            &BTreeSet::new(),
        )
        .unwrap();
        assert!(empty.callable.is_empty() && empty.deferred.is_empty());
    }
}
