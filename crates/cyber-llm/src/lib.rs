//! Provider-neutral LLM layer (`provider-catalog`, `provider-credentials`).
//!
//! Every adapter turns one [`LlmRequest`] into one stream of [`LlmEvent`]s, so no
//! provider-specific type crosses the adapter boundary. The [`catalog`] module resolves
//! providers, models, credentials, roles, variants and prices.

pub mod adapters;
pub mod catalog;
pub mod chatgpt;
pub mod copilot;
mod error;
mod json;
pub mod limits;
pub mod reservations;
mod retry;
mod sse;
pub mod subscription;
mod turn;
mod types;

pub use adapters::{Adapter, ApiKind, Endpoint, EventStream};
pub use error::{ErrorKind, LlmError, classify};
pub use retry::{RetryPolicy, open_with_retry};
pub use sse::{SseEvent, SseParser};
pub use turn::{TurnOutput, collect};
pub use types::{
    AttachmentRef, Content, FinishReason, Initiator, LlmEvent, LlmRequest, Message,
    OPAQUE_MAX_BYTES, OPAQUE_REDACTED, Opaque, OpaqueKind, Reasoning, Role, Routing, Shaping,
    ToolCall, ToolSpec, TurnState, Usage, redact_opaque_fields,
};
