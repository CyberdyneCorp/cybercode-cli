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
    let (deferred, callable) = tools
        .into_iter()
        .map(|mut tool| {
            tool.deferred =
                should_defer && tool.scope.deferrable() && !loaded.contains(&tool.spec.name);
            tool
        })
        .partition(|tool| tool.deferred);
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
            deferred: false,
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

pub(super) const LOADED: &str = "session.tools.loaded.1";
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Loaded {
    pub names: Vec<String>,
}

pub(super) fn validate_names(names: &[String]) -> Result<(), String> {
    if names.is_empty()
        || names.len() > 1000
        || names.iter().any(|name| {
            name.is_empty()
                || name.len() > 64
                || !name.as_bytes()[0].is_ascii_alphabetic()
                || !name
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
        })
        || names.iter().collect::<BTreeSet<_>>().len() != names.len()
    {
        return Err("Invalid loaded tool names".into());
    }
    Ok(())
}

pub(super) fn validate_event(
    tx: &rusqlite::Transaction<'_>,
    event: &cyber_store::StoredEvent,
) -> Result<(), String> {
    if event.kind != LOADED {
        return Ok(());
    }
    if !event.aggregate_id.starts_with("ses_") {
        return Err("Loaded tools require a Session".into());
    }
    let exists: bool = tx
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM session WHERE id = ?1)",
            [&event.aggregate_id],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    if !exists {
        return Err("Loaded tools require an existing Session".into());
    }
    let loaded: Loaded =
        serde_json::from_value(event.data.clone()).map_err(|_| "Invalid loaded tools event")?;
    validate_names(&loaded.names)
}

impl super::Runtime {
    pub fn tool_catalog(&self, turn: &super::TurnContext) -> Vec<ToolDef> {
        self.inner.tools.definitions(turn)
    }

    pub async fn load_tool_schemas(
        &self,
        session_id: &str,
        names: Vec<String>,
        cancel: &tokio_util::sync::CancellationToken,
    ) -> Result<(), super::RuntimeError> {
        if names.is_empty() {
            return Ok(());
        }
        validate_names(&names).map_err(super::RuntimeError::Invalid)?;
        self.inner.ensure_admission_open(session_id)?;
        let interrupted = || super::RuntimeError::Invalid("Tool search interrupted".into());
        let _admission = tokio::select! {
            biased;
            _ = cancel.cancelled() => return Err(interrupted()),
            admission = self.inner.open() => admission?,
        };
        let handle = tokio::select! {
            biased;
            _ = cancel.cancelled() => return Err(interrupted()),
            handle = self.inner.handle(session_id) => handle?,
        };
        let mut state = tokio::select! {
            biased;
            _ = cancel.cancelled() => return Err(interrupted()),
            state = handle.state.lock() => state,
        };
        if cancel.is_cancelled() {
            return Err(interrupted());
        }
        let names: Vec<_> = names
            .into_iter()
            .filter(|name| !state.loaded_tools.contains(name))
            .collect();
        if !names.is_empty() {
            self.inner.commit_locked(
                &mut state,
                vec![super::events::event(LOADED, &Loaded { names })],
            )?;
        }
        Ok(())
    }
}
