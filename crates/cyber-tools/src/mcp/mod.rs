//! MCP protocol foundations. Transport/process authority belongs to the caller.
mod authority;
mod connection;
mod discovery;
mod elicitation;
mod headers_command;
mod http;
mod launch;
pub(crate) mod mention;
pub mod oauth;
mod owned;
mod prompts;
mod remote_auth;
mod required;
mod resources;
mod roots;
mod sampler;
mod sampling;
mod stdio;
mod values;
pub use authority::{ServerSelection, authorize_server, inspect_server};
pub use connection::{ConnectionError, StartupOptions, StderrCapture, StdioConnection};
pub use discovery::{DiscoveredTool, exposed_tool_name};
pub use elicitation::ElicitationContext;
pub use http::{HeaderSet, HttpConnection, HttpFailure, HttpTarget, RemoteTransport};
pub use launch::{LaunchFailure, LocalLaunchError, LocalLauncher, LocalServer};
pub use owned::{McpLocationPin, OwnedLocalServer};
pub use prompts::{
    DiscoveredPrompt, McpPromptCommand, PromptArgument, PromptContent, PromptMessage, PromptRole,
    valid_name as valid_prompt_name,
};
pub use resources::{CONTENT_LIMIT, ResourceBody, ResourceContent, ResourceReply, ResourceRequest};
pub use roots::McpRoots;
pub use sampling::SamplingRequest;
pub use stdio::{IdleUpdate, ListChange, McpError, StdioClient};
pub use values::{decision, render_arguments};
pub(crate) const LIMIT: usize = 1024 * 1024;
pub(crate) mod pool;
pub use pool::McpReconnectError;
