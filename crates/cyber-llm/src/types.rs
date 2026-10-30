//! Provider-neutral request, history and stream event types.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::catalog::profile::{ReasoningCarrier, VerbosityLevel};

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
    /// Base64 document data (`application/pdf`).
    Document {
        media_type: String,
        data: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
    },
    /// A stored prompt attachment (`storage-events` → Attachment storage). The runtime
    /// resolves it to [`Content::Image`] or [`Content::Document`] before a request; an
    /// adapter that receives one fails the request.
    Attachment(AttachmentRef),
    /// One reasoning item from an earlier Turn (`session-runtime` → Reasoning items).
    /// `opaque` is sent back only to the provider and model that produced it; the runtime
    /// lowers reasoning to text across models. The legacy `signature` string reads as
    /// [`OpaqueKind::Signature`] state.
    Reasoning {
        text: String,
        #[serde(
            default,
            alias = "signature",
            deserialize_with = "opaque_or_signature",
            skip_serializing_if = "Option::is_none"
        )]
        #[schemars(with = "Option<Opaque>")]
        opaque: Option<Opaque>,
    },
    ToolCall {
        id: String,
        name: String,
        input: Value,
        /// State of this one call, such as a Gemini thought signature.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        opaque: Option<Opaque>,
    },
    ToolResult {
        call_id: String,
        output: String,
        #[serde(default)]
        is_error: bool,
    },
}

impl Content {
    /// The opaque provider state this part carries.
    pub fn opaque(&self) -> Option<&Opaque> {
        match self {
            Self::Reasoning { opaque, .. } | Self::ToolCall { opaque, .. } => opaque.as_ref(),
            _ => None,
        }
    }
}

/// What stands in for opaque data in exports, logs and telemetry.
pub const OPAQUE_REDACTED: &str = "[opaque reasoning]";
/// The largest opaque value kept; a larger one is dropped when received.
pub const OPAQUE_MAX_BYTES: usize = 1024 * 1024;

/// The kind of provider state an [`Opaque`] carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum OpaqueKind {
    /// OpenAI Responses `encrypted_content`.
    EncryptedReasoning,
    /// An Anthropic thinking signature, or a per-call thought signature.
    Signature,
    /// An Anthropic `redacted_thinking` block; the item has no text.
    Redacted,
    /// Provider reasoning details (OpenRouter `reasoning_details`), as JSON text.
    Details,
}

/// Provider state that must travel back with reasoning or a tool call. `data` is stored
/// exactly as received and never decoded; `provider` and `model` name its producer, so
/// only that model receives it back. `Debug` never prints `data`.
#[derive(Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Opaque {
    #[serde(default)]
    pub provider: String,
    #[serde(default)]
    pub model: String,
    pub kind: OpaqueKind,
    pub data: String,
    /// The provider's item ID, when it issued one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
}

impl Opaque {
    /// State an adapter decoded; the runtime stamps its provider and model.
    pub fn new(kind: OpaqueKind, data: impl Into<String>) -> Self {
        Self {
            provider: String::new(),
            model: String::new(),
            kind,
            data: data.into(),
            id: None,
        }
    }

    /// Whether `provider`/`model` produced this state.
    pub fn is_from(&self, provider: &str, model: &str) -> bool {
        self.provider == provider && self.model == model
    }

    /// Replace the data with [`OPAQUE_REDACTED`], for anything that leaves the store.
    pub fn redact(&mut self) {
        self.data = OPAQUE_REDACTED.into();
    }

    /// Context tokens of opaque bytes: 3 tokens per 4 bytes.
    pub fn tokens(bytes: usize) -> u64 {
        (bytes as u64).saturating_mul(3) / 4
    }
}

/// Redact every `opaque` object of a serialized event or part, at any depth, for what
/// leaves the store: a forked Session's creation event carries whole entries and calls.
/// The legacy `signature` string of a reasoning payload or part (`{text, signature}`)
/// is signature state too.
pub fn redact_opaque_fields(value: &mut Value) {
    match value {
        Value::Object(map) => {
            if map.contains_key("text")
                && let Some(signature) = map.get_mut("signature")
                && signature.is_string()
            {
                *signature = Value::String(OPAQUE_REDACTED.into());
            }
            for (key, field) in map.iter_mut() {
                match field.get_mut("data") {
                    Some(data) if key == "opaque" && data.is_string() => {
                        *data = Value::String(OPAQUE_REDACTED.into());
                    }
                    _ => redact_opaque_fields(field),
                }
            }
        }
        Value::Array(items) => items.iter_mut().for_each(redact_opaque_fields),
        _ => {}
    }
}

impl std::fmt::Debug for Opaque {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Opaque")
            .field("provider", &self.provider)
            .field("model", &self.model)
            .field("kind", &self.kind)
            .field(
                "data",
                &format_args!("{OPAQUE_REDACTED} ({} bytes)", self.data.len()),
            )
            .field("id", &self.id)
            .finish()
    }
}

/// A reasoning `opaque` object, or the legacy `signature` string.
fn opaque_or_signature<'de, D>(deserializer: D) -> Result<Option<Opaque>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Stored {
        Signature(String),
        Opaque(Opaque),
    }
    Ok(match Option::<Stored>::deserialize(deserializer)? {
        Some(Stored::Signature(signature)) => Some(Opaque::new(OpaqueKind::Signature, signature)),
        Some(Stored::Opaque(opaque)) => Some(opaque),
        None => None,
    })
}

/// The durable reference to a content-addressed attachment blob of one Session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct AttachmentRef {
    /// Lowercase hex SHA-256 of the stored (normalized) bytes.
    pub sha256: String,
    /// Sniffed from the bytes, never taken from the client.
    pub media_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Stored size in bytes.
    pub size: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
    /// Page count of a PDF.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pages: Option<u32>,
    /// SHA-256 of the original blob, kept beside a normalized one with
    /// `attachments.keep_original`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original: Option<String>,
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

/// Who a model call is made for (`provider-catalog` → GitHub Copilot subscription provider):
/// the first call after a promoted user prompt is the `user`'s; every other call (tool-result
/// continuations, hook checks, summaries, sampling, Turns other sources open, compaction)
/// is the `agent`'s. Copilot counts premium requests by it; other adapters ignore it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Initiator {
    User,
    Agent,
}

impl Initiator {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Agent => "agent",
        }
    }
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
    /// Set by the Turn loop; `None` for calls made outside one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub initiator: Option<Initiator>,
    /// The request field that carries reasoning state back, from the model's profile
    /// (`provider-catalog` → Model behavior profiles); `none` when the profile replays no
    /// reasoning. An adapter never guesses it.
    #[serde(default, skip_serializing_if = "ReasoningCarrier::is_none")]
    pub reasoning_carrier: ReasoningCarrier,
    /// Optional Responses fields the model's profile allows.
    #[serde(default, skip_serializing_if = "Shaping::is_default")]
    pub shaping: Shaping,
    /// Who the request is for, for the routing headers; `None` outside a Session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub routing: Option<Routing>,
}

/// Responses request fields from the model's profile and capabilities
/// (`provider-catalog` → Responses request shaping). Other protocols ignore them.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Shaping {
    /// The model reasons, so its encrypted reasoning is requested back.
    #[serde(default)]
    pub reasoning: bool,
    /// The model accepts `reasoning.summary`.
    #[serde(default)]
    pub reasoning_summary: bool,
    /// The model lists the `none` effort, so reasoning off is sent as that effort.
    #[serde(default)]
    pub effort_none: bool,
    /// `text.verbosity`, set only when the model supports it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verbosity: Option<VerbosityLevel>,
    /// `parallel_tool_calls`, sent whenever tools are offered.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parallel_tool_calls: Option<bool>,
    /// Offer `apply_patch` as a grammar-constrained custom tool taking the raw patch
    /// (`builtin-tools` → Freeform apply_patch).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub freeform_patch: bool,
}

impl Shaping {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

/// The Session a request belongs to (`provider-catalog` → Session routing headers).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Routing {
    /// The root Session ID.
    pub root: String,
    /// The requesting Session's ID.
    pub session: String,
    /// What a call outside a Turn is for (`compact`, `title`, `recap`), or a subagent's
    /// agent name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subagent: Option<String>,
    /// The sticky-routing token of the current Turn; never serialized.
    #[serde(skip)]
    #[schemars(skip)]
    pub turn_state: Option<TurnState>,
}

/// The `x-codex-turn-state` value the backend returned first in a Turn, shared by every
/// request and retry of that Turn. Clones share one value; [`TurnState::reset`] forgets
/// it when the next Turn starts.
#[derive(Clone, Default)]
pub struct TurnState(std::sync::Arc<std::sync::Mutex<Option<String>>>);

impl TurnState {
    pub fn get(&self) -> Option<String> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Keep `value` unless the Turn already has one: only the first counts.
    pub fn offer(&self, value: &str) {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get_or_insert_with(|| value.to_owned());
    }

    /// Forget the kept value, for every clone.
    pub fn reset(&self) {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
    }
}

/// Two handles are equal when they share one Turn's state.
impl PartialEq for TurnState {
    fn eq(&self, other: &Self) -> bool {
        std::sync::Arc::ptr_eq(&self.0, &other.0)
    }
}

impl std::fmt::Debug for TurnState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("TurnState(..)")
    }
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
    /// State of this one call, such as a Gemini thought signature.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub opaque: Option<Opaque>,
}

/// The provider-neutral stream: `text.delta`, `reasoning.delta`, `reasoning_item`,
/// `tool_call.delta`, `tool_call.done`, `usage` and `finish`. Errors are the stream's
/// `Err` items.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum LlmEvent {
    TextDelta {
        text: String,
    },
    ReasoningDelta {
        text: String,
    },
    /// Closes the reasoning block streamed since the last one: `text` is its full text
    /// when the adapter has it (empty keeps the streamed text), and `opaque` the state to
    /// replay with it, such as an Anthropic signature. The runtime numbers the blocks.
    ReasoningItem {
        #[serde(default)]
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        opaque: Option<Opaque>,
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
    /// Never sent by an adapter: [`crate::open_with_retry`] discarded the attempt streamed
    /// so far after a retryable failure and replays the request. Consumers drop every
    /// event since the stream opened or the previous `Discarded`; usage they saw stays spent.
    #[serde(skip)]
    Discarded,
}

impl LlmEvent {
    /// Whether the event is assistant output (anything but usage and retry markers).
    pub fn is_output(&self) -> bool {
        !matches!(self, Self::Usage(_) | Self::Discarded)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn legacy_reasoning_signature_reads_as_signature_state() {
        let old = json!({"type": "reasoning", "text": "think", "signature": "sig=="});
        let Content::Reasoning { text, opaque } = serde_json::from_value(old).unwrap() else {
            panic!("not reasoning");
        };
        assert_eq!(text, "think");
        let opaque = opaque.unwrap();
        assert_eq!(
            (opaque.kind, opaque.data.as_str()),
            (OpaqueKind::Signature, "sig==")
        );
        let bare: Content =
            serde_json::from_value(json!({"type": "reasoning", "text": "t"})).unwrap();
        assert_eq!(bare.opaque(), None);
    }

    #[test]
    fn reasoning_items_and_call_state_round_trip() {
        let mut opaque = Opaque::new(OpaqueKind::EncryptedReasoning, "gAAAA");
        opaque.provider = "openai".into();
        opaque.model = "gpt-6-sol".into();
        opaque.id = Some("rs_1".into());
        let parts = vec![
            Content::Reasoning {
                text: "plan".into(),
                opaque: Some(opaque.clone()),
            },
            Content::ToolCall {
                id: "c1".into(),
                name: "read".into(),
                input: json!({}),
                opaque: Some(Opaque::new(OpaqueKind::Signature, "ts")),
            },
        ];
        let value = serde_json::to_value(&parts).unwrap();
        assert_eq!(value[0]["opaque"]["kind"], "encrypted_reasoning");
        assert_eq!(value[0]["opaque"]["id"], "rs_1");
        let back: Vec<Content> = serde_json::from_value(value).unwrap();
        assert_eq!(back, parts);
        let plain = serde_json::to_value(Content::Reasoning {
            text: "t".into(),
            opaque: None,
        })
        .unwrap();
        assert_eq!(plain, json!({"type": "reasoning", "text": "t"}));
    }

    #[test]
    fn opaque_debug_and_redaction_hide_the_data() {
        let mut opaque = Opaque::new(OpaqueKind::Redacted, "secret-blob");
        assert!(!format!("{opaque:?}").contains("secret-blob"));
        opaque.redact();
        assert_eq!(opaque.data, OPAQUE_REDACTED);
        assert_eq!(Opaque::tokens(4000), 3000);
    }

    #[test]
    fn serialized_opaque_fields_are_redacted() {
        let mut event = json!({"text": "t", "opaque": {"kind": "signature", "data": "blob"}});
        redact_opaque_fields(&mut event);
        assert_eq!(event["opaque"]["data"], OPAQUE_REDACTED);
        let mut plain = json!({"text": "t", "data": "kept"});
        redact_opaque_fields(&mut plain);
        assert_eq!(plain, json!({"text": "t", "data": "kept"}));
    }

    /// Regression: the legacy `{text, signature}` reasoning shape kept its signature.
    #[test]
    fn legacy_reasoning_signatures_are_redacted() {
        let mut ended = json!({"text": "t", "signature": "sig"});
        redact_opaque_fields(&mut ended);
        assert_eq!(ended["signature"], OPAQUE_REDACTED);
        let mut part = json!({"parts": [{"type": "reasoning", "text": "t", "signature": "sig"}]});
        redact_opaque_fields(&mut part);
        assert_eq!(part["parts"][0]["signature"], OPAQUE_REDACTED);
        let mut other = json!({"signature": "kept"});
        redact_opaque_fields(&mut other);
        assert_eq!(other["signature"], "kept");
    }

    /// Regression: only a top-level `opaque` was redacted, so a fork's creation event
    /// streamed the nested entries' and calls' blobs.
    #[test]
    fn nested_opaque_fields_are_redacted() {
        let blob = json!({"kind": "encrypted_reasoning", "data": "gAAAA"});
        let mut created = json!({
            "history": [{"reasoning_items": [{"text": "t", "opaque": blob}]}],
            "calls": [{"call_id": "c", "opaque": blob}],
        });
        redact_opaque_fields(&mut created);
        assert!(!created.to_string().contains("gAAAA"));
        assert_eq!(
            created["history"][0]["reasoning_items"][0]["opaque"]["data"],
            OPAQUE_REDACTED
        );
        assert_eq!(created["calls"][0]["opaque"]["data"], OPAQUE_REDACTED);
    }

    #[test]
    fn reasoning_item_event_round_trips() {
        let event = LlmEvent::ReasoningItem {
            text: String::new(),
            opaque: Some(Opaque::new(OpaqueKind::Signature, "B")),
        };
        let value = serde_json::to_value(&event).unwrap();
        assert_eq!(value["type"], "reasoning_item");
        assert_eq!(serde_json::from_value::<LlmEvent>(value).unwrap(), event);
        let call: ToolCall =
            serde_json::from_value(json!({"id": "c", "name": "n", "arguments": "{}", "input": {}}))
                .unwrap();
        assert_eq!(call.opaque, None);
    }

    /// Clones share one Turn's value: the first offer wins, and a reset clears it for all.
    #[test]
    fn turn_state_keeps_the_first_value_until_reset() {
        let state = TurnState::default();
        let request_copy = state.clone();
        assert_eq!(state.get(), None);
        state.offer("abc");
        request_copy.offer("later");
        assert_eq!(request_copy.get().as_deref(), Some("abc"));
        assert_eq!(state, request_copy);
        assert_ne!(state, TurnState::default());
        request_copy.reset();
        assert_eq!(state.get(), None);
        state.offer("next");
        assert_eq!(request_copy.get().as_deref(), Some("next"));
    }
}
