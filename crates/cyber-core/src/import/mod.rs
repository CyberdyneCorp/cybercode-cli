//! Pure migration planning. Conversion grants no filesystem, trust or execution authority.
mod claude;
mod opencode;
pub use claude::{ClaudePermissions, ConversionError, PermissionRule, claude_permissions};
pub use opencode::opencode_permissions;
