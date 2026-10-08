//! Hook catalog with explicit provenance; constructing a catalog performs no effects.

mod decisions;
mod envelope;
mod selectors;

pub use decisions::{HookAction, HookDecision, ParsedDecision};
pub use envelope::{HookEvent, HookIdentity, HookLocation};

use std::io;
use std::path::Path;

use serde::Serialize;

use crate::config::{HookHandler, HookKind, HookSettings, Resolved};
use crate::trust::{HookInvocationTrust, TrustStore};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HookScope {
    Managed,
    Global,
    Project,
    Local,
    Plugin,
    Invocation,
}

impl HookScope {
    pub fn requires_handler_trust(self) -> bool {
        matches!(self, Self::Project | Self::Local)
    }

    pub fn requires_sandbox(self, sandbox_all: bool) -> bool {
        sandbox_all || !matches!(self, Self::Managed | Self::Global)
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct HookDefinition {
    pub event: String,
    pub scope: HookScope,
    pub source: String,
    pub pointer: String,
    pub matcher: Option<String>,
    pub paths: Vec<String>,
    pub handler: HookHandler,
    pub digest: String,
    #[serde(skip)]
    selector: selectors::Selector,
}

impl HookDefinition {
    /// Re-read durable approval at use; catalog construction does not cache trust.
    pub fn is_trusted(
        &self,
        root: &Path,
        store: &TrustStore,
        invocation: Option<&HookInvocationTrust>,
    ) -> io::Result<bool> {
        if !self.scope.requires_handler_trust() {
            return Ok(true);
        }
        if store.is_hook_approved(root, &self.digest)? {
            return Ok(true);
        }
        match invocation {
            Some(invocation) => invocation.is_approved(root, &self.digest),
            None => Ok(false),
        }
    }

    pub fn kind(&self) -> HookKind {
        self.handler.kind
    }
}

#[derive(Debug, Clone)]
pub struct HookCatalog {
    pub settings: HookSettings,
    pub definitions: Vec<HookDefinition>,
}

impl HookCatalog {
    pub fn from_config(resolved: &Resolved) -> Result<Self, String> {
        let settings = HookSettings::from_config(&resolved.value)?;
        let mut definitions = Vec::new();
        for (event, groups) in &settings.events {
            for (group_index, group) in groups.iter().enumerate() {
                for (handler_index, handler) in group.hooks.iter().enumerate() {
                    let pointer = format!("/hooks/{event}/{group_index}/hooks/{handler_index}");
                    let source = resolved
                        .sources
                        .get(&pointer)
                        .filter(|source| resolved.layers.contains(source))
                        .ok_or_else(|| format!("{pointer}: missing loaded handler origin"))?;
                    definitions.push(HookDefinition {
                        event: event.clone(),
                        scope: scope(source)?,
                        source: source.clone(),
                        pointer,
                        matcher: group.matcher.clone(),
                        paths: group.paths.clone(),
                        handler: handler.clone(),
                        digest: handler.digest().map_err(|error| error.to_string())?,
                        selector: selectors::Selector::new(
                            group.matcher.as_deref(),
                            &group.paths,
                            handler,
                        )?,
                    });
                }
            }
        }
        // Stable sorting preserves declared group/handler order inside each scope.
        definitions.sort_by_key(|definition| definition.scope);
        Ok(Self {
            settings,
            definitions,
        })
    }
}

fn scope(source: &str) -> Result<HookScope, String> {
    if source
        .strip_prefix("global:")
        .is_some_and(|path| !path.is_empty())
    {
        return Ok(HookScope::Global);
    }
    if let Some(path) = source
        .strip_prefix("project:")
        .filter(|path| !path.is_empty())
    {
        let path = Path::new(path);
        return Ok(
            if path
                .file_name()
                .is_some_and(|name| name == "cyber.local.jsonc")
                && path
                    .parent()
                    .and_then(Path::file_name)
                    .is_some_and(|name| name == ".cyber")
            {
                HookScope::Local
            } else {
                HookScope::Project
            },
        );
    }
    if source.starts_with("cli:") || source.starts_with("env:") {
        return Ok(HookScope::Invocation);
    }
    Err(format!("unsupported hook origin: {source}"))
}
