//! Read-only source discovery and pure migration planning; neither grants trust or execution authority.
mod adapted;
mod agents;
mod claude;
mod claude_settings;
mod codex;
mod codex_policy;
mod codex_profiles;
mod codex_providers;
mod codex_source;
mod detection;
mod detection_sessions;
mod discovery;
mod environment_setup;
mod hooks;
mod mcp;
mod opencode;
mod preview;
mod report_values;
pub use report_values::ValueComparison;
mod provenance;
mod snapshot;
mod write;
pub(crate) use agents::plugin_agent;
pub use claude::{ClaudePermissions, ConversionError, PermissionRule, claude_permissions};
pub use codex::codex_prefix_rules;
pub use codex_policy::{CodexPolicyConfig, codex_policy_config};
pub use codex_providers::{
    CodexProviderConfig, ProviderMappingIssue, RequiredEnvironment, codex_provider_config,
    codex_provider_source,
};
pub use codex_source::{CodexRuleSource, CodexRules, codex_rules};
pub use detection::{
    DefinitionCounts, DetectedTool, DetectionReport, ReadTimeCoverage, detect_sources,
};
pub use discovery::{
    DiscoveryError, DiscoveryIssue, SourceFile, SourceInventory, SourceKind, SourceLayer,
    SourceRoots, SourceTool, discover_sources,
};
pub use mcp::{McpImportConfig, McpMappingIssue, mcp_config};
pub use opencode::opencode_permissions;
pub use preview::{
    ImportPreview, ImportScope, MappingRecord, PreviewEnvironment, PreviewOutput, WrittenImport,
    preview_import,
};
pub use provenance::SourceReference;
pub use snapshot::SourceSnapshot;
pub(crate) use snapshot::{command_directory, verify_command_directory};

mod opencode_providers;
pub use opencode_providers::{OpenCodeProviderConfig, opencode_provider_config};

mod opencode_settings;
pub use opencode_settings::{OpenCodeSettingsConfig, opencode_settings_config};
