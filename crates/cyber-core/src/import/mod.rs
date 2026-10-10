//! Read-only source discovery and pure migration planning; neither grants trust or execution authority.
mod claude;
mod codex;
mod codex_policy;
mod codex_providers;
mod codex_source;
mod detection;
mod discovery;
mod opencode;
mod preview;
mod provenance;
mod snapshot;
pub use claude::{ClaudePermissions, ConversionError, PermissionRule, claude_permissions};
pub use codex::codex_prefix_rules;
pub use codex_policy::{CodexPolicyConfig, codex_policy_config};
pub use codex_providers::{
    CodexProviderConfig, ProviderMappingIssue, RequiredEnvironment, codex_provider_config,
    codex_provider_source,
};
pub use codex_source::{CodexRuleSource, CodexRules, codex_rules};
pub use detection::{DefinitionCounts, DetectedTool, DetectionReport, detect_sources};
pub use discovery::{
    DiscoveryError, DiscoveryIssue, SourceFile, SourceInventory, SourceKind, SourceLayer,
    SourceRoots, SourceTool, discover_sources,
};
pub use opencode::opencode_permissions;
pub use preview::{
    ImportPreview, ImportScope, MappingRecord, PreviewEnvironment, PreviewOutput, preview_import,
};
pub use provenance::SourceReference;
pub use snapshot::SourceSnapshot;
