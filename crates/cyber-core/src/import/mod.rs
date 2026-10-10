//! Pure migration planning. Conversion grants no filesystem, trust or execution authority.
mod claude;
mod codex;
mod codex_source;
mod opencode;
pub use claude::{ClaudePermissions, ConversionError, PermissionRule, claude_permissions};
pub use codex::codex_prefix_rules;
pub use codex_source::{CodexRuleSource, CodexRules, codex_rules};
pub use opencode::opencode_permissions;
