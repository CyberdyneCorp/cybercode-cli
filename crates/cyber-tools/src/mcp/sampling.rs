//! Bounded basic sampling requests. Conversion grants no model or server authority.
use base64::Engine;
use cyber_llm::{Content, LlmRequest, Message, Role};
use serde::Deserialize;
use serde_json::Value;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Params {
    messages: Vec<SamplingMessage>,
    max_tokens: u32,
    system_prompt: Option<String>,
    temperature: Option<f64>,
    #[serde(default)]
    stop_sequences: Vec<String>,
    include_context: Option<String>,
    model_preferences: Option<Preferences>,
    metadata: Option<Value>,
    #[serde(rename = "_meta")]
    meta: Option<Value>,
}
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Preferences {
    #[serde(default)]
    hints: Vec<Hint>,
    cost_priority: Option<f64>,
    speed_priority: Option<f64>,
    intelligence_priority: Option<f64>,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Hint {
    name: Option<String>,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SamplingMessage {
    role: Role,
    content: Value,
}

/// Validated server-supplied input; callers still own permissions, billing and cancellation.
#[derive(Debug)]
pub struct SamplingRequest {
    messages: Vec<Message>,
    system: Option<String>,
    max_tokens: u32,
    temperature: Option<f64>,
    pub stop_sequences: Vec<String>,
}

impl SamplingRequest {
    pub fn parse(value: &Value) -> Result<Self, &'static str> {
        let bytes = serde_json::to_vec(value).map_err(|_| "Invalid sampling request")?;
        if bytes.len() > super::LIMIT {
            return Err("Sampling request exceeds 1 MiB");
        }
        let params: Params = serde_json::from_value(value.clone())
            .map_err(|_| "Invalid or unsupported basic sampling request")?;
        if params.messages.is_empty()
            || params.messages.len() > 256
            || params.max_tokens == 0
            || params
                .include_context
                .as_deref()
                .is_some_and(|v| v != "none")
            || params
                .temperature
                .is_some_and(|v| !v.is_finite() || !(0.0..=2.0).contains(&v))
            || params.stop_sequences.len() > 16
            || params
                .stop_sequences
                .iter()
                .any(|v| v.is_empty() || v.len() > 2048)
            || params.metadata.as_ref().is_some_and(|v| !v.is_object())
            || params.meta.as_ref().is_some_and(|v| !v.is_object())
        {
            return Err("Invalid or unsupported basic sampling request");
        }
        if let Some(preferences) = &params.model_preferences {
            preferences.validate()?;
        }
        let messages = params
            .messages
            .into_iter()
            .map(message)
            .collect::<Result<_, _>>()?;
        Ok(Self {
            messages,
            system: params.system_prompt,
            max_tokens: params.max_tokens,
            temperature: params.temperature,
            stop_sequences: params.stop_sequences,
        })
    }

    /// Keep the selected model's transport configuration, replacing history and tool authority.
    pub fn request(
        &self,
        template: &LlmRequest,
        output_limit: u32,
    ) -> Result<LlmRequest, &'static str> {
        if output_limit == 0 {
            return Err("Sampling model has no output allowance");
        }
        let mut request = LlmRequest {
            messages: self.messages.clone(),
            system: self.system.iter().cloned().collect(),
            tools: Vec::new(),
            tools_disabled: true,
            max_output_tokens: Some(self.max_tokens.min(output_limit)),
            temperature: self.temperature,
            cache_key: None,
            ..template.clone()
        };
        if let Some(body) = request.body.as_object_mut() {
            for key in [
                "max_tokens",
                "max_output_tokens",
                "max_completion_tokens",
                "messages",
                "input",
                "instructions",
                "system",
                "tools",
                "tool_choice",
                "toolChoice",
                "temperature",
                "stop",
                "stop_sequences",
                "model",
                "prompt_cache_key",
                "previous_response_id",
                "conversation",
                "prompt",
            ] {
                body.remove(key);
            }
        }
        Ok(request)
    }
}

impl Preferences {
    fn validate(&self) -> Result<(), &'static str> {
        if self.hints.len() > 64
            || self
                .hints
                .iter()
                .any(|hint| hint.name.as_ref().is_some_and(|name| name.len() > 1024))
            || [
                self.cost_priority,
                self.speed_priority,
                self.intelligence_priority,
            ]
            .into_iter()
            .flatten()
            .any(|v| !v.is_finite() || !(0.0..=1.0).contains(&v))
        {
            return Err("Invalid sampling model preferences");
        }
        Ok(())
    }
}

fn message(message: SamplingMessage) -> Result<Message, &'static str> {
    let parts = match &message.content {
        Value::Array(parts) => parts.clone(),
        Value::Object(_) => vec![message.content],
        _ => return Err("Invalid sampling message content"),
    };
    if parts.is_empty() || parts.len() > 64 {
        return Err("Invalid sampling message content");
    }
    Ok(Message {
        role: message.role,
        content: parts.iter().map(content).collect::<Result<_, _>>()?,
    })
}

fn content(value: &Value) -> Result<Content, &'static str> {
    let props = value.as_object().ok_or("Invalid sampling content")?;
    if props.keys().any(|key| {
        !matches!(
            key.as_str(),
            "type" | "text" | "data" | "mimeType" | "annotations" | "_meta"
        )
    }) {
        return Err("Invalid sampling content");
    }
    match value["type"].as_str() {
        Some("text") if !props.contains_key("data") && !props.contains_key("mimeType") => {
            Ok(Content::Text {
                text: value["text"]
                    .as_str()
                    .ok_or("Invalid sampling text")?
                    .into(),
            })
        }
        Some("image") if !props.contains_key("text") => {
            let mime = value["mimeType"].as_str().ok_or("Invalid sampling image")?;
            if !matches!(
                mime,
                "image/png" | "image/jpeg" | "image/gif" | "image/webp"
            ) {
                return Err("Unsupported sampling image type");
            }
            let data = value["data"].as_str().ok_or("Invalid sampling image")?;
            let decoded = base64::engine::general_purpose::STANDARD
                .decode(data)
                .map_err(|_| "Invalid sampling image encoding")?;
            if decoded.is_empty() {
                return Err("Empty sampling image");
            }
            Ok(Content::Image {
                media_type: mime.into(),
                data: data.into(),
            })
        }
        _ => Err("Unsupported sampling content type"),
    }
}

/// Callback authority is transient and belongs to the current native tool invocation.
pub(super) trait SamplingHandler: Send + Sync {
    fn sample<'a>(&'a self, params: &'a Value) -> futures::future::BoxFuture<'a, (Value, bool)>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn params() -> Value {
        json!({"messages":[{"role":"user","content":{"type":"text","text":"server prompt"}}],"maxTokens":100})
    }

    #[test]
    fn selected_model_has_only_server_messages_and_no_tools_or_ambient_history() {
        let mut value = params();
        value["systemPrompt"] = json!("server system");
        value["temperature"] = json!(0.5);
        value["modelPreferences"] =
            json!({"hints":[{"name":"unconfigured-provider/model"}],"speedPriority":1.0});
        let parsed = SamplingRequest::parse(&value).unwrap();
        let template = LlmRequest {
            model: "chosen-small-model".into(),
            messages: vec![Message::user_text("private session history")],
            system: vec!["private context".into()],
            cache_key: Some("private-session".into()),
            body: json!({"messages":"private","system":"private","input":"private","model":"other","tools":[{}],"tool_choice":"auto","max_tokens":9999,"temperature":1.9,"provider_option":true,"previous_response_id":"private-parent","conversation":"private-session","prompt":"private-prompt"}),
            ..Default::default()
        };
        let request = parsed.request(&template, 64).unwrap();
        assert_eq!(request.model, "chosen-small-model");
        assert_eq!(request.messages, vec![Message::user_text("server prompt")]);
        assert_eq!(request.system, ["server system"]);
        assert!(request.tools_disabled && request.tools.is_empty());
        assert_eq!(request.max_output_tokens, Some(64));
        assert_eq!(request.temperature, Some(0.5));
        assert!(request.cache_key.is_none());
        assert_eq!(request.body, json!({"provider_option":true}));
        assert!(parsed.request(&template, 0).is_err());
    }

    #[test]
    fn basic_sampling_preserves_roles_text_arrays_and_valid_image_data() {
        let value = json!({"messages":[{"role":"assistant","content":[{"type":"text","text":"earlier"},{"type":"image","mimeType":"image/png","data":"aGVsbG8="}]}],"maxTokens":12,"includeContext":"none","stopSequences":["END"]});
        let parsed = SamplingRequest::parse(&value).unwrap();
        assert_eq!(parsed.stop_sequences, ["END"]);
        let request = parsed.request(&LlmRequest::default(), 32).unwrap();
        assert_eq!(request.messages[0].role, Role::Assistant);
        assert_eq!(request.messages[0].content.len(), 2);
        assert!(
            matches!(&request.messages[0].content[1], Content::Image {data, ..} if data == "aGVsbG8=")
        );
    }

    #[test]
    fn unsupported_or_malformed_requests_fail_without_echoing_payloads() {
        for (key, value) in [
            ("maxTokens", json!(0)),
            ("messages", json!([])),
            ("includeContext", json!("allServers")),
            ("temperature", json!(3.0)),
            ("stopSequences", json!([""])),
            ("metadata", json!("private-secret")),
            ("tools", json!([])),
            ("toolChoice", json!({"mode":"none"})),
            ("modelPreferences", json!({"costPriority":-1})),
            (
                "messages",
                json!([{"role":"system","content":{"type":"text","text":"private-secret"}}]),
            ),
            (
                "messages",
                json!([{"role":"user","content":{"type":"audio","data":"private-secret"}}]),
            ),
            (
                "messages",
                json!([{"role":"user","content":{"type":"image","mimeType":"image/png","data":"private-secret"}}]),
            ),
            (
                "messages",
                json!([{"role":"user","content":{"type":"tool_use","name":"shell"}}]),
            ),
        ] {
            let mut input = params();
            input[key] = value;
            let error = SamplingRequest::parse(&input).unwrap_err();
            assert!(!error.contains("private-secret"));
        }
        let mut input = params();
        input["systemPrompt"] = json!("x".repeat(super::super::LIMIT));
        assert!(SamplingRequest::parse(&input).is_err());
    }
}
