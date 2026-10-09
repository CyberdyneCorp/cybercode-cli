//! MCP protocol foundations. Transport/process authority belongs to the caller.
mod authority;
mod connection;
mod launch;
mod stdio;
mod values;
pub use authority::{ServerSelection, authorize_server, inspect_server};
pub use connection::{ConnectionError, StderrCapture, StdioConnection};
pub use launch::{LocalLaunchError, LocalLauncher, LocalServer};
pub use stdio::{McpError, StdioClient};
pub use values::{decision, render_arguments};
pub(crate) const LIMIT: usize = 1024 * 1024;
