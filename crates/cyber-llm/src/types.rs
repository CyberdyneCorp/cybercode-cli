//! Provider-neutral request, history and stream event types.

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    User,
    Assistant,
}

/// One part of a message. Tool results travel in user messages.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Content {
    Text {
        text: String,
    },
    /// Base64 image data.
    Image {
        media_type: String,
        data: String,
    },
    /// Reasoning from an earlier Turn. `signature` is sent back only to the provider that
    /// produced it; the runtime lowers reasoning to text across providers.
    Reasoning {
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        signature: Option<String>,
    },
    ToolCall {
        id: String,
        name: String,
        input: Value,
    },
    ToolResult {
        call_id: String,
        output: String,
        #[serde(default)]
        is_error: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Message {
    pub role: Role,
    pub content: Vec<Content>,
}

impl Message {
    pub fn user_text(text: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            content: vec![Content::Text { text: text.into() }],
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    /// JSON Schema of the input object.
    pub input_schema: Value,
}

/// Reasoning control derived from the selected variant.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Reasoning {
    Off,
    Effort(String),
    BudgetTokens(u32),
}

/// One provider stream request.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct LlmRequest {
    /// The provider's model identifier (`api_id`).
    pub model: String,
    /// System prompt blocks; the cache breakpoint goes after the last one.
    pub system: Vec<String>,
    pub messages: Vec<Message>,
    pub tools: Vec<ToolSpec>,
    /// Force no-tools selection after all request overlays.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub tools_disabled: bool,
    pub max_output_tokens: Option<u32>,
    pub temperature: Option<f64>,
    pub reasoning: Option<Reasoning>,
    /// Stable key for keyed prompt caching, derived from the Session ID.
    pub cache_key: Option<String>,
    /// `false` disables caching hints (`cache: "none"`).
    #[serde(default = "default_true")]
    pub cache: bool,
    /// Layered request body overlay, merged last.
    #[serde(default)]
    pub body: Value,
    #[serde(default)]
    pub headers: Vec<(String, String)>,
}

fn default_true() -> bool {
    true
}

/// Token classes of one step. `input` excludes cache reads and writes; `output` excludes
/// reasoning.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema,
)]
pub struct Usage {
    pub input: u64,
    pub output: u64,
    pub reasoning: u64,
    pub cache_read: u64,
    pub cache_write: u64,
}

impl Usage {
    pub fn add(&mut self, other: &Usage) {
        self.input += other.input;
        self.output += other.output;
        self.reasoning += other.reasoning;
        self.cache_read += other.cache_read;
        self.cache_write += other.cache_write;
    }

    /// Tokens of the prompt, for context-window utilization.
    pub fn context_tokens(&self) -> u64 {
        self.input + self.cache_read + self.cache_write
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FinishReason {
    Stop,
    ToolCalls,
    Length,
    ContentFilter,
    Other(String),
}

/// A complete tool call. `input` is `None` when the arguments are not valid JSON; the
/// runtime routes such calls to the `invalid` tool.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: String,
    pub input: Option<Value>,
}

/// The provider-neutral stream: `text.delta`, `reasoning.delta`, `tool_call.delta`,
/// `tool_call.done`, `usage` and `finish`. Errors are the stream's `Err` items.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum LlmEvent {
    TextDelta {
        text: String,
    },
    ReasoningDelta {
        text: String,
    },
    /// Provider metadata that must accompany the reasoning when replayed natively.
    ReasoningSignature {
        signature: String,
    },
    ToolCallDelta {
        id: String,
        name: String,
        arguments: String,
    },
    ToolCallDone(ToolCall),
    Usage(Usage),
    Finish {
        reason: FinishReason,
    },
}

impl LlmEvent {
    /// Whether the event is assistant output (anything but usage).
    pub fn is_output(&self) -> bool {
        !matches!(self, Self::Usage(_))
    }
}
