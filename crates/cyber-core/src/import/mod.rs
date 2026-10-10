//! Pure migration planning. Conversion grants no filesystem, trust or execution authority.
mod claude;
pub use claude::{ClaudePermissions, ConversionError, PermissionRule, claude_permissions};
