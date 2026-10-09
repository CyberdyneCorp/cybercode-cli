//! MCP protocol foundations. Transport/process authority belongs to the caller.
mod stdio;
mod values;
pub use stdio::{McpError, StdioClient};
pub use values::{decision, render_arguments};
pub(crate) const LIMIT: usize = 1024 * 1024;
