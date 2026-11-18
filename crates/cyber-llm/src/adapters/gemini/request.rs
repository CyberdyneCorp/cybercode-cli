//! Request bodies (`provider-catalog` → Gemini API adapter, Gemini thought signatures,
//! Gemini thinking configuration, Gemini tool declarations).
//!
//! Every value is built from the request alone and in a fixed order, so the system
//! instruction, the tools and the replayed history stay byte-identical between the
//! requests of a Session (implicit caching).

use std::collections::{HashMap, HashSet};

use serde_json::{Map, Value, json};

use crate::catalog::profile::ThinkingParam;
use crate::error::{ErrorKind, LlmError};
use crate::json::{deep_merge, strip_credentials};
use crate::types::{Content, LlmRequest, Message, Opaque, OpaqueKind, Reasoning, Role, ToolSpec};

/// Gemini 4 streaming caps output, thinking included, at 65,536 tokens.
pub(super) const MAX_OUTPUT_TOKENS: u32 = 65_535;
/// The documented placeholder that skips signature validation (`signature_bypass`).
pub(super) const BYPASS_SIGNATURE: &str = "skip_thought_signature_validator";
/// The prefix of a call ID the decoder made up for a model that issued none; such IDs
/// are never sent back.
pub(super) const LOCAL_ID_PREFIX: &str = "gm_";
/// The lowest level every level-taking model accepts, for reasoning off.
const LOWEST_LEVEL: &str = "low";
/// JSON Schema keys Gemini rejects or ignores in `parametersJsonSchema`.
const STRIPPED_SCHEMA_KEYS: &[&str] = &["$schema", "$id", "examples"];
/// Schema keys whose members are property names, not schema keywords.
const NAMED_SCHEMAS: &[&str] = &["properties", "patternProperties", "$defs", "definitions"];

pub(super) fn build_body(request: &LlmRequest) -> Result<Value, LlmError> {
    check_tool_names(&request.tools)?;
    let mut body = Map::new();
    if !request.system.is_empty() {
        let parts: Vec<Value> = request.system.iter().map(|t| json!({"text": t})).collect();
        body.insert("systemInstruction".into(), json!({ "parts": parts }));
    }
    body.insert("contents".into(), Value::Array(contents(request)));
    if !request.tools.is_empty() {
        body.insert(
            "tools".into(),
            json!([{ "functionDeclarations": declarations(&request.tools) }]),
        );
        body.insert(
            "toolConfig".into(),
            json!({ "functionCallingConfig": { "mode": "AUTO" } }),
        );
    }
    body.insert("generationConfig".into(), generation_config(request));
    let mut body = Value::Object(body);
    deep_merge(&mut body, &request.body);
    if request.tools_disabled {
        no_tools(&mut body);
    }
    strip_credentials(&mut body);
    Ok(body)
}

/// A runtime no-tools ceiling is final, including over tools an overlay injected.
fn no_tools(body: &mut Value) {
    if let Some(map) = body.as_object_mut() {
        map.remove("tools");
        map.insert(
            "toolConfig".into(),
            json!({ "functionCallingConfig": { "mode": "NONE" } }),
        );
    }
}

/// The model name without a `models/` or `publishers/…/models/` prefix, lowercased.
fn model_name(model: &str) -> String {
    model
        .rsplit('/')
        .next()
        .unwrap_or(model)
        .to_ascii_lowercase()
}

fn generation_config(request: &LlmRequest) -> Value {
    let mut config = Map::new();
    let max = request
        .max_output_tokens
        .unwrap_or(MAX_OUTPUT_TOKENS)
        .min(MAX_OUTPUT_TOKENS);
    config.insert("maxOutputTokens".into(), json!(max));
    // Google recommends the default temperature for Gemini 3 and later.
    if let Some(temperature) = request.temperature {
        config.insert("temperature".into(), json!(temperature));
    }
    if let Some(thinking) = thinking_config(request) {
        config.insert("thinkingConfig".into(), thinking);
    }
    Value::Object(config)
}

/// `thinkingConfig` with exactly one of `thinkingLevel` or `thinkingBudget`: both in one
/// request is a 400.
fn thinking_config(request: &LlmRequest) -> Option<Value> {
    let Some(reasoning) = &request.reasoning else {
        // No variant: the model's own default, with its summaries streamed.
        return request
            .shaping
            .reasoning
            .then(|| json!({ "includeThoughts": true }));
    };
    let name = model_name(&request.model);
    let param = request
        .shaping
        .thinking_param
        .unwrap_or_else(|| default_param(&name));
    let (field, value) = match param {
        ThinkingParam::Level => ("thinkingLevel", json!(level(&name, reasoning))),
        ThinkingParam::Budget => ("thinkingBudget", json!(budget(&name, reasoning))),
    };
    let on = value != json!(0);
    let mut config = Map::new();
    config.insert(field.into(), value);
    if on {
        config.insert("includeThoughts".into(), Value::Bool(true));
    }
    Some(Value::Object(config))
}

/// Gemini 2.5 takes a budget; Gemini 3 and later take a level.
fn default_param(name: &str) -> ThinkingParam {
    if name.starts_with("gemini-2.5") {
        ThinkingParam::Budget
    } else {
        ThinkingParam::Level
    }
}

fn level(name: &str, reasoning: &Reasoning) -> String {
    match reasoning {
        Reasoning::Effort(effort) => effort.clone(),
        // These models cannot turn thinking off.
        Reasoning::Off => LOWEST_LEVEL.into(),
        Reasoning::BudgetTokens(budget) => {
            let level = match budget {
                0..=1024 => "low",
                1025..=8192 => "medium",
                _ => "high",
            };
            cyber_core::log::info(
                "gemini",
                "thinking budget converted to the nearest level",
                json!({ "model": name, "budget": budget, "level": level }),
            );
            level.into()
        }
    }
}

fn budget(name: &str, reasoning: &Reasoning) -> i64 {
    match reasoning {
        Reasoning::BudgetTokens(budget) => i64::from(*budget),
        // 2.5 Pro cannot turn thinking off; 128 tokens is its minimum.
        Reasoning::Off if name.contains("-pro") => 128,
        Reasoning::Off => 0,
        Reasoning::Effort(effort) => match effort.as_str() {
            "minimal" => 512,
            "low" => 1024,
            "medium" => 8192,
            "high" => 24_576,
            // Dynamic thinking.
            _ => -1,
        },
    }
}

fn check_tool_names(tools: &[ToolSpec]) -> Result<(), LlmError> {
    let valid = |name: &str| {
        let mut chars = name.chars();
        chars
            .next()
            .is_some_and(|first| first.is_ascii_alphabetic() || first == '_')
            && name.len() <= 64
            && chars.all(|c| c.is_ascii_alphanumeric() || "_.:-".contains(c))
    };
    match tools.iter().find(|tool| !valid(&tool.name)) {
        Some(tool) => Err(LlmError::new(
            ErrorKind::InvalidRequest,
            format!(
                "tool name {:?} is not a valid Gemini function name",
                tool.name
            ),
        )),
        None => Ok(()),
    }
}

/// Tools in registry order, each schema as written minus the keys Gemini rejects. Nothing
/// is shrunk or rewritten.
fn declarations(tools: &[ToolSpec]) -> Vec<Value> {
    tools
        .iter()
        .map(|tool| {
            let mut schema = tool.input_schema.clone();
            clean_schema(&mut schema);
            json!({
                "name": tool.name,
                "description": tool.description,
                "parametersJsonSchema": schema,
            })
        })
        .collect()
}

fn clean_schema(schema: &mut Value) {
    match schema {
        Value::Object(map) => {
            map.retain(|key, _| !STRIPPED_SCHEMA_KEYS.contains(&key.as_str()));
            for (key, value) in map.iter_mut() {
                match value.as_object_mut() {
                    Some(named) if NAMED_SCHEMAS.contains(&key.as_str()) => {
                        named.values_mut().for_each(clean_schema);
                    }
                    _ => clean_schema(value),
                }
            }
        }
        Value::Array(items) => items.iter_mut().for_each(clean_schema),
        _ => {}
    }
}

/// What lowering needs to know about the whole history.
struct History<'a> {
    /// Call ID → tool name, for `functionResponse.name`.
    names: HashMap<&'a str, &'a str>,
    /// Current-turn calls with no signature from this model.
    foreign: HashSet<&'a str>,
    /// Send `foreign` calls with the placeholder signature instead of as text.
    bypass: bool,
}

fn contents(request: &LlmRequest) -> Vec<Value> {
    let history = History {
        names: call_names(&request.messages),
        foreign: foreign_calls(request),
        bypass: request.shaping.signature_bypass,
    };
    if history.bypass && !history.foreign.is_empty() {
        warn_bypass(request);
    }
    request
        .messages
        .iter()
        .filter_map(|message| history.content(message))
        .collect()
}

fn call_names(messages: &[Message]) -> HashMap<&str, &str> {
    messages
        .iter()
        .flat_map(|message| &message.content)
        .filter_map(|part| match part {
            Content::ToolCall { id, name, .. } => Some((id.as_str(), name.as_str())),
            _ => None,
        })
        .collect()
}

/// Gemini 3 and later reject a current-turn step whose first call lost its signature.
/// A step none of whose calls carries one was made by another model (the runtime keeps a
/// signature only for the model that produced it). Gemini 2.5 accepts unsigned calls.
fn foreign_calls(request: &LlmRequest) -> HashSet<&str> {
    let name = model_name(&request.model);
    let validated = name.starts_with("gemini-")
        && !name.starts_with("gemini-1")
        && !name.starts_with("gemini-2");
    if !validated {
        return HashSet::new();
    }
    let messages = &request.messages;
    // The current turn starts after the latest user message with text in it; tool
    // results do not start one.
    let start = messages
        .iter()
        .rposition(|message| message.role == Role::User && has_text(message))
        .map_or(0, |at| at + 1);
    messages[start..]
        .iter()
        .filter(|message| message.role == Role::Assistant && !signed_step(message))
        .flat_map(|message| &message.content)
        .filter_map(|part| match part {
            Content::ToolCall { id, .. } => Some(id.as_str()),
            _ => None,
        })
        .collect()
}

fn has_text(message: &Message) -> bool {
    message
        .content
        .iter()
        .any(|part| matches!(part, Content::Text { text } if !text.is_empty()))
}

fn signed_step(message: &Message) -> bool {
    message.content.iter().any(|part| {
        matches!(part, Content::ToolCall { opaque: Some(opaque), .. } if is_signature(opaque))
    })
}

fn is_signature(opaque: &Opaque) -> bool {
    opaque.kind == OpaqueKind::Signature
}

/// One `warn` line per Session that sends the placeholder.
fn warn_bypass(request: &LlmRequest) {
    static WARNED: std::sync::Mutex<Option<HashSet<String>>> = std::sync::Mutex::new(None);
    let session = request
        .routing
        .as_ref()
        .map_or_else(|| request.model.clone(), |routing| routing.session.clone());
    let fresh = WARNED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get_or_insert_with(HashSet::new)
        .insert(session);
    if fresh {
        cyber_core::log::warn(
            "gemini",
            "signature_bypass: sending placeholder thought signatures on another model's tool calls",
            json!({ "model": request.model, "option": "signature_bypass" }),
        );
    }
}

impl History<'_> {
    /// One content per message. Signatures of reasoning items go last, each on a part of
    /// its own, so a signed part is never merged with another.
    fn content(&self, message: &Message) -> Option<Value> {
        let mut parts = Vec::new();
        let mut signed = Vec::new();
        for part in &message.content {
            match part {
                Content::Reasoning {
                    opaque: Some(opaque),
                    ..
                } if is_signature(opaque) => {
                    signed.push(json!({ "text": "", "thoughtSignature": opaque.data }));
                }
                _ => parts.extend(self.part(part)),
            }
        }
        parts.extend(signed);
        if parts.is_empty() {
            return None;
        }
        let role = match message.role {
            Role::User => "user",
            Role::Assistant => "model",
        };
        Some(json!({ "role": role, "parts": parts }))
    }

    fn part(&self, part: &Content) -> Option<Value> {
        match part {
            Content::Text { text } if !text.is_empty() => Some(json!({ "text": text })),
            Content::Image { media_type, data }
            | Content::Document {
                media_type, data, ..
            } => Some(json!({ "inlineData": { "mimeType": media_type, "data": data } })),
            Content::ToolCall {
                id,
                name,
                input,
                opaque,
            } => Some(self.call(id, name, input, opaque.as_ref())),
            Content::ToolResult {
                call_id,
                output,
                is_error,
            } => Some(self.result(call_id, output, *is_error)),
            // Summaries without a signature are display text; another model's reasoning
            // arrives already lowered to text.
            _ => None,
        }
    }

    fn lowered(&self, id: &str) -> bool {
        !self.bypass && self.foreign.contains(id)
    }

    fn call(&self, id: &str, name: &str, input: &Value, opaque: Option<&Opaque>) -> Value {
        if self.lowered(id) {
            return json!({ "text": format!("[tool call {name} {input}]") });
        }
        let args = if input.is_object() {
            input.clone()
        } else {
            json!({ "input": input })
        };
        let mut call = Map::new();
        if !id.starts_with(LOCAL_ID_PREFIX) {
            call.insert("id".into(), json!(id));
        }
        call.insert("name".into(), json!(name));
        call.insert("args".into(), args);
        let mut part = json!({ "functionCall": call });
        match opaque.filter(|opaque| is_signature(opaque)) {
            Some(opaque) => part["thoughtSignature"] = json!(opaque.data),
            None if self.foreign.contains(id) => part["thoughtSignature"] = json!(BYPASS_SIGNATURE),
            None => {}
        }
        part
    }

    fn result(&self, call_id: &str, output: &str, is_error: bool) -> Value {
        let name = self.names.get(call_id).copied().unwrap_or("unknown");
        if self.lowered(call_id) {
            return json!({ "text": format!("[tool result {name}] {output}") });
        }
        let response = if is_error {
            json!({ "error": output })
        } else {
            json!({ "output": output })
        };
        let mut result = Map::new();
        if !call_id.starts_with(LOCAL_ID_PREFIX) {
            result.insert("id".into(), json!(call_id));
        }
        result.insert("name".into(), json!(name));
        result.insert("response".into(), response);
        json!({ "functionResponse": result })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schemas_lose_only_rejected_keywords() {
        let mut schema = json!({
            "$schema": "https://json-schema.org/draft/2020-12/schema", "$id": "x",
            "type": "object", "examples": [{}],
            "properties": {
                "examples": {"type": "string", "examples": ["a"]},
                "nested": {"type": "object", "$defs": {"d": {"$id": "y", "type": "string"}}}
            }
        });
        clean_schema(&mut schema);
        assert_eq!(
            schema,
            json!({
                "type": "object",
                "properties": {
                    "examples": {"type": "string"},
                    "nested": {"type": "object", "$defs": {"d": {"type": "string"}}}
                }
            })
        );
    }

    #[test]
    fn tool_names_follow_gemini_rules() {
        let tool = |name: &str| ToolSpec {
            name: name.into(),
            description: String::new(),
            input_schema: json!({}),
        };
        assert!(check_tool_names(&[tool("read"), tool("mcp__a.b:c-d")]).is_ok());
        assert!(check_tool_names(&[tool("9lives")]).is_err());
        assert!(check_tool_names(&[tool("has space")]).is_err());
        assert!(check_tool_names(&[tool(&"a".repeat(65))]).is_err());
    }

    #[test]
    fn the_thinking_field_follows_the_generation() {
        assert_eq!(default_param("gemini-2.5-flash"), ThinkingParam::Budget);
        assert_eq!(
            default_param("gemini-3.1-pro-preview"),
            ThinkingParam::Level
        );
        assert_eq!(default_param("gemini-4-carbon"), ThinkingParam::Level);
        assert_eq!(budget("gemini-2.5-pro", &Reasoning::Off), 128);
        assert_eq!(budget("gemini-2.5-flash", &Reasoning::Off), 0);
        assert_eq!(level("gemini-4-carbon", &Reasoning::Off), "low");
        assert_eq!(
            level("gemini-4-carbon", &Reasoning::BudgetTokens(32_768)),
            "high"
        );
        assert_eq!(
            level("gemini-4-carbon", &Reasoning::BudgetTokens(512)),
            "low"
        );
    }
}
