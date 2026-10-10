//! Read-only source discovery and pure migration planning; neither grants trust or execution authority.
mod claude;
mod codex;
mod codex_source;
mod discovery;
mod opencode;
mod snapshot;
pub use claude::{ClaudePermissions, ConversionError, PermissionRule, claude_permissions};
pub use codex::codex_prefix_rules;
pub use codex_source::{CodexRuleSource, CodexRules, codex_rules};
pub use discovery::{
    DiscoveryError, DiscoveryIssue, SourceFile, SourceInventory, SourceKind, SourceLayer,
    SourceRoots, SourceTool, discover_sources,
};
pub use opencode::opencode_permissions;
pub use snapshot::SourceSnapshot;
