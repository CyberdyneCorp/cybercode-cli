//! Provider-neutral LLM layer (`provider-catalog`, `provider-credentials`).
//!
//! Every adapter turns one [`LlmRequest`] into one stream of [`LlmEvent`]s, so no
//! provider-specific type crosses the adapter boundary. The [`catalog`] module resolves
//! providers, models, credentials, roles, variants and prices.

pub mod adapters;
pub mod catalog;
mod error;
mod json;
mod retry;
mod sse;
mod turn;
mod types;

pub use adapters::{Adapter, ApiKind, Endpoint, EventStream};
pub use error::{ErrorKind, LlmError, classify};
pub use retry::{RetryPolicy, open_with_retry};
pub use sse::{SseEvent, SseParser};
pub use turn::{TurnOutput, collect};
pub use types::{
    Content, FinishReason, LlmEvent, LlmRequest, Message, Reasoning, Role, ToolCall, ToolSpec,
    Usage,
};
