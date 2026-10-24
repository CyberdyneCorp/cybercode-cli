//! Configuration discovery, layering, substitution, trust gating and validation
//! (`configuration`, `workspace-trust`).

mod agent_profiles;
mod agents;
pub(crate) mod annotated;
mod attachments;
mod auto_mode;
mod deferred_tools;
mod edit;
mod gate;
mod git;
mod hooks;
mod intelligence;
mod jsonc;
mod load;
mod managed;
mod mcp;
mod mcp_files;
mod mcp_serve;
mod merge;
mod messaging;
mod modes;
mod peers;
mod subst;
mod validate;
mod workflows;

pub(crate) use hooks::is_event_name;
pub(crate) use mcp_files::convert as convert_mcp_file;
pub(crate) use subst::has_placeholder;
pub(crate) use validate::is_mode;

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::Serialize;
use serde_json::Value;

pub use agent_profiles::{AgentProfile, AgentTools, resolve_agents};
pub use attachments::AttachmentSettings;
pub use auto_mode::{AutoFallback, AutoModeSettings, AutoPattern};
pub use deferred_tools::DeferredToolSettings;
pub use edit::{set_document_member, set_global_key};
pub use gate::{SensitiveSplit, canonical_json, split_sensitive};
pub use git::GitSettings;
pub use hooks::{HookCondition, HookGroup, HookHandler, HookKind, HookSettings};
pub use intelligence::{FormatterConfig, FormatterSettings, LspServerConfig, LspSettings};
pub use jsonc::{parse as parse_jsonc, to_json as jsonc_to_json};
pub use load::{
    LoadRequest, RawHookSection, ensure_global_config, load, load_global, project_root,
    trust_report, withheld_hook_sections,
};
pub use managed::{
    MANAGED_POLICY_DIR_ENV, MCP_BLOCKED, ManagedPolicy, McpPattern, McpPolicy, os_managed_dir,
};
pub use mcp::{
    McpOAuth, McpOAuthSetting, McpServer, McpSettings, McpToolFilter, OAUTH_REDIRECT_PORT_MIN,
    transport_owned_header,
};
pub use mcp_serve::{
    FALLBACK_CLIENT_LABEL, McpServeLimits, McpServeSettings, derive_client_label,
    valid_client_label,
};
pub use messaging::{DEFAULT_MAX_HOPS, InboundPolicy, InboundSettings, MessagingSettings};
pub use modes::{bound_mode, mode_ceiling, mode_rank, never_ask_rules};
pub use peers::{PeerListen, PeerRemote, PeersSettings, valid_peer_name};
pub use validate::redact_secrets;
pub use workflows::{TierTarget, WorkflowSettings};

/// `compat.claude` of a resolved config: whether Claude Code files such as `CLAUDE.md`
/// are read in place. On unless set to `false`. The runtime and `cyber doctor` both
/// read it through here, from the same per-Location config.
pub fn claude_compat(config: &Value) -> bool {
    config
        .pointer("/compat/claude")
        .and_then(Value::as_bool)
        .unwrap_or(true)
}

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
    /// JSON pointer of each merged string leaf a project layer supplied with an
    /// `{env:}`/`{file:}` placeholder → that unsubstituted template. Review shows these so
    /// an approver sees which local values a project definition would send.
    #[serde(skip)]
    pub templates: BTreeMap<String, String>,
    /// JSON pointer of each leaf → label of the layer that set it.
    /// Hook groups and handlers also retain their own indexed origin labels.
    pub sources: BTreeMap<String, String>,
    pub warnings: Vec<String>,
    pub trust: TrustReport,
    /// Labels of the layers that were merged, lowest priority first.
    pub layers: Vec<String>,
    /// OS-managed policy state; its hooks are merged with `managed:` origins.
    pub managed: ManagedPolicy,
    /// Every plugin a scope lists and whether it loaded (`plugins-marketplace`).
    pub plugins: Vec<crate::plugins::PluginStatus>,
    /// `--pure` or `CYBER_PURE=1`: no plugins, and hooks and MCP servers only from
    /// organization policy.
    pub pure: bool,
}

/// Merge raw file layers without substitution, validation or trust side effects.
pub(crate) fn merge_raw_layer(base: &mut serde_json::Value, overlay: &serde_json::Value) {
    merge::merge_layer(base, overlay, "raw file layer", &mut merge::Sources::new());
}
pub(crate) use load::{global_layer_paths, project_layer_paths};
