//! Configuration discovery, layering, substitution, trust gating and validation
//! (`configuration`, `workspace-trust`).

mod agent_profiles;
mod agents;
mod auto_mode;
mod gate;
mod hooks;
mod jsonc;
mod load;
mod merge;
mod subst;
mod validate;

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::Serialize;
use serde_json::Value;

pub use agent_profiles::{AgentProfile, AgentTools, resolve_agents};
pub use auto_mode::{AutoFallback, AutoModeSettings, AutoPattern};
pub use gate::{SensitiveSplit, canonical_json, split_sensitive};
pub use hooks::{HookCondition, HookGroup, HookHandler, HookKind, HookSettings};
pub use jsonc::{parse as parse_jsonc, to_json as jsonc_to_json};
pub use load::{LoadRequest, ensure_global_config, load, project_root, trust_report};
pub use validate::redact_secrets;

/// Published JSON Schema URL written into new config documents.
pub const SCHEMA_URL: &str = "https://cyber-code.dev/schema/config.json";

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("ConfigParseError: {path}:{line}:{column} {message}")]
    Parse {
        path: String,
        line: usize,
        column: usize,
        message: String,
    },
    #[error("ConfigInvalidError: {}", .issues.join("; "))]
    Invalid { issues: Vec<String> },
    #[error("could not read {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
}

/// Workspace-trust state of the project layers.
#[derive(Debug, Clone, Serialize)]
pub struct TrustReport {
    /// Canonical checkout root the approval is keyed by.
    pub checkout_root: PathBuf,
    /// `sha256:<hex>` of the security-sensitive definitions, absent when there are none.
    pub digest: Option<String>,
    pub trusted: bool,
    /// `<file>#<json pointer>` of every security-sensitive definition. When `trusted` is
    /// false these are held back from the resolved configuration until approved.
    pub definitions: Vec<String>,
}

/// The merged configuration for one Location.
#[derive(Debug, Clone, Serialize)]
pub struct Resolved {
    pub value: Value,
    /// JSON pointer of each leaf → label of the layer that set it.
    /// Hook groups and handlers also retain their own indexed origin labels.
    pub sources: BTreeMap<String, String>,
    pub warnings: Vec<String>,
    pub trust: TrustReport,
    /// Labels of the layers that were merged, lowest priority first.
    pub layers: Vec<String>,
}
