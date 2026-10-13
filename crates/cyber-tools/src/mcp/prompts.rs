//! MCP prompts (`mcp` → Prompts as commands): bounded discovery and validated
//! `prompts/get` replies.
//!
//! Everything a server sends is untrusted. Discovery keeps only prompts whose names can
//! be a `/<server>:<prompt>` command and whose arguments are well formed; anything else
//! is dropped with a log line. Pagination is bounded like tool discovery: a repeated
//! cursor or a catalog past the byte cap fails closed.
use std::collections::HashSet;
use std::time::Duration;

use serde_json::{Map, Value, json};
use tokio::io::{AsyncRead, AsyncWrite};

use super::{McpError, StdioClient};

/// Characters in a prompt or argument name.
pub const NAME_LIMIT: usize = 64;
/// Characters kept from a server-supplied description.
pub const DESCRIPTION_LIMIT: usize = 200;
/// Prompts kept per server; later ones are dropped with a log line.
const PROMPT_LIMIT: usize = 256;
const ARGUMENT_LIMIT: usize = 32;
const CATALOG_LIMIT: usize = 4 * super::LIMIT;
const CURSOR_LIMIT: usize = 4096;
/// Messages one `prompts/get` reply may hold.
const MESSAGE_LIMIT: usize = 256;
/// Text bytes one `prompts/get` reply may hold across its messages.
pub const PROMPT_TEXT_LIMIT: usize = 256 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredPrompt {
    pub name: String,
    /// Control characters replaced and capped at [`DESCRIPTION_LIMIT`] characters.
    pub description: String,
    pub arguments: Vec<PromptArgument>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptArgument {
    pub name: String,
    pub required: bool,
}

/// A discovered prompt offered as the slash command `/<server>:<prompt>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpPromptCommand {
    pub server: String,
    pub prompt: DiscoveredPrompt,
}

impl McpPromptCommand {
    pub fn name(&self) -> String {
        format!("{}:{}", self.server, self.prompt.name)
    }

    /// Declared arguments in order: `<required>` and `[optional]`.
    pub fn argument_hint(&self) -> Option<String> {
        let hint: Vec<String> = self
            .prompt
            .arguments
            .iter()
            .map(|argument| {
                if argument.required {
                    format!("<{}>", argument.name)
                } else {
                    format!("[{}]", argument.name)
                }
            })
            .collect();
        (!hint.is_empty()).then(|| hint.join(" "))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptRole {
    User,
    Assistant,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PromptContent {
    Text(String),
    /// Content that is not attached yet, by kind (`image`, `audio`, ...).
    Omitted(&'static str),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptMessage {
    pub role: PromptRole,
    pub content: PromptContent,
}

/// A name usable inside `/<server>:<prompt>`: bounded, without whitespace, control
/// characters or `:`.
pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.chars().count() <= NAME_LIMIT
        && !name
            .chars()
            .any(|c| c.is_whitespace() || c.is_control() || c == ':')
}

/// A prompts listing the server rejected (a JSON-RPC error) or answered invalidly (a
/// malformed page, a repeated cursor, a catalog over the byte cap) settled its request,
/// so it only withdraws the prompts: the server offers none and keeps its tools.
/// Transport failures, timeouts and a request left unresolved still fail the connection.
pub(crate) fn optional(
    server: &str,
    result: Result<Vec<DiscoveredPrompt>, McpError>,
    unresolved: bool,
) -> Result<Vec<DiscoveredPrompt>, McpError> {
    match result {
        Err(error @ (McpError::Remote(_) | McpError::Protocol(_))) if !unresolved => {
            cyber_core::log::warn(
                "mcp",
                "MCP prompts listing failed; the server offers no prompts",
                json!({"server":server,"error":error.to_string()}),
            );
            Ok(Vec::new())
        }
        result => result,
    }
}

pub(crate) async fn discover<R: AsyncRead + Unpin, W: AsyncWrite + Unpin>(
    client: &mut StdioClient<R, W>,
    server: &str,
    timeout: Duration,
) -> Result<Vec<DiscoveredPrompt>, McpError> {
    if !client.supports("prompts") {
        return Ok(Vec::new());
    }
    if timeout.is_zero() {
        return Err(McpError::Timeout);
    }
    let deadline = tokio::time::Instant::now() + timeout;
    let mut catalog = Catalog::default();
    let mut cursor = None;
    loop {
        let page =
            tokio::time::timeout_at(deadline, client.list_prompts(cursor.as_deref(), timeout))
                .await
                .map_err(|_| McpError::Timeout)??;
        cursor = catalog.page(&page, server)?;
        if cursor.is_none() {
            return Ok(catalog.prompts);
        }
    }
}

#[derive(Default)]
struct Catalog {
    prompts: Vec<DiscoveredPrompt>,
    names: HashSet<String>,
    cursors: HashSet<String>,
    bytes: usize,
}

impl Catalog {
    fn page(&mut self, page: &Value, server: &str) -> Result<Option<String>, McpError> {
        self.bytes += serde_json::to_vec(page)
            .map_err(|_| McpError::Protocol("invalid MCP prompt listing"))?
            .len();
        if self.bytes > CATALOG_LIMIT {
            return Err(McpError::Protocol("MCP prompt catalog exceeds byte limit"));
        }
        let prompts = page["prompts"].as_array().ok_or(McpError::Protocol(
            "MCP prompt listing requires prompts array",
        ))?;
        for definition in prompts {
            self.keep(definition, server);
        }
        let cursor = match page.get("nextCursor") {
            None | Some(Value::Null) => return Ok(None),
            Some(Value::String(cursor)) if valid_cursor(cursor) => cursor,
            Some(_) => return Err(McpError::Protocol("invalid MCP prompt listing cursor")),
        };
        if !self.cursors.insert(cursor.clone()) {
            return Err(McpError::Protocol("repeated MCP prompt listing cursor"));
        }
        Ok(Some(cursor.clone()))
    }

    fn keep(&mut self, definition: &Value, server: &str) {
        let dropped = match parse_prompt(definition) {
            Err(reason) => reason,
            Ok(_) if self.prompts.len() >= PROMPT_LIMIT => "too many MCP prompts",
            Ok(prompt) if !self.names.insert(prompt.name.clone()) => "duplicate MCP prompt name",
            Ok(prompt) => {
                self.prompts.push(prompt);
                return;
            }
        };
        cyber_core::log::warn(
            "mcp",
            "MCP prompt dropped",
            json!({"server":server,"reason":dropped}),
        );
    }
}

fn valid_cursor(cursor: &str) -> bool {
    !cursor.is_empty() && cursor.len() <= CURSOR_LIMIT && !cursor.chars().any(char::is_control)
}

fn parse_prompt(definition: &Value) -> Result<DiscoveredPrompt, &'static str> {
    let name = definition["name"]
        .as_str()
        .filter(|name| valid_name(name))
        .ok_or("invalid MCP prompt name")?;
    let description = match definition.get("description") {
        None | Some(Value::Null) => "",
        Some(Value::String(text)) => text,
        Some(_) => return Err("invalid MCP prompt description"),
    };
    let arguments = match definition.get("arguments") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(arguments)) if arguments.len() <= ARGUMENT_LIMIT => {
            parse_arguments(arguments)?
        }
        Some(_) => return Err("invalid MCP prompt arguments"),
    };
    Ok(DiscoveredPrompt {
        name: name.into(),
        description: clean(description, DESCRIPTION_LIMIT),
        arguments,
    })
}

fn parse_arguments(arguments: &[Value]) -> Result<Vec<PromptArgument>, &'static str> {
    let mut names = HashSet::new();
    let mut parsed = Vec::with_capacity(arguments.len());
    for argument in arguments {
        let name = argument["name"]
            .as_str()
            .filter(|name| valid_name(name) && names.insert(*name))
            .ok_or("invalid MCP prompt argument name")?;
        let required = match argument.get("required") {
            None | Some(Value::Null) => false,
            Some(Value::Bool(required)) => *required,
            Some(_) => return Err("invalid MCP prompt argument required flag"),
        };
        parsed.push(PromptArgument {
            name: name.into(),
            required,
        });
    }
    Ok(parsed)
}

/// Control characters become spaces; the result is trimmed and capped in characters.
pub(crate) fn clean(text: &str, limit: usize) -> String {
    let cleaned: String = text
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .take(limit)
        .collect();
    cleaned.trim().into()
}

/// `prompts/get` parameters: the prompt name and its string arguments.
pub(super) fn get_params(name: &str, arguments: &Map<String, Value>) -> Value {
    json!({"name":name,"arguments":arguments})
}

/// The validated messages of a `prompts/get` reply.
pub(super) fn messages(result: &Value) -> Result<Vec<PromptMessage>, McpError> {
    let invalid = McpError::Protocol("invalid MCP prompt messages");
    let messages = result["messages"]
        .as_array()
        .filter(|messages| messages.len() <= MESSAGE_LIMIT)
        .ok_or(invalid)?;
    let mut text = 0usize;
    let mut kept = Vec::with_capacity(messages.len());
    for message in messages {
        let role = match message["role"].as_str() {
            Some("user") => PromptRole::User,
            Some("assistant") => PromptRole::Assistant,
            _ => return Err(McpError::Protocol("invalid MCP prompt message role")),
        };
        let content = content(&message["content"])?;
        if let PromptContent::Text(body) = &content {
            text += body.len();
        }
        if text > PROMPT_TEXT_LIMIT {
            return Err(McpError::Protocol("MCP prompt exceeds 256 KiB"));
        }
        kept.push(PromptMessage { role, content });
    }
    Ok(kept)
}

fn content(content: &Value) -> Result<PromptContent, McpError> {
    let invalid = McpError::Protocol("invalid MCP prompt message content");
    Ok(match content["type"].as_str().ok_or(invalid)? {
        "text" => PromptContent::Text(
            content["text"]
                .as_str()
                .ok_or(McpError::Protocol("invalid MCP prompt message content"))?
                .into(),
        ),
        "image" => PromptContent::Omitted("image"),
        "audio" => PromptContent::Omitted("audio"),
        "resource" => PromptContent::Omitted("embedded resource"),
        "resource_link" => PromptContent::Omitted("resource link"),
        _ => PromptContent::Omitted("unsupported"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page(catalog: &mut Catalog, value: Value) -> Result<Option<String>, McpError> {
        catalog.page(&value, "srv")
    }

    #[test]
    fn invalid_prompt_names_and_arguments_are_dropped() {
        let mut catalog = Catalog::default();
        let next = page(
            &mut catalog,
            json!({"prompts":[
                {"name":"review_pr","description":"Review\na PR\u{7}","arguments":[
                    {"name":"number","required":true},{"name":"focus"}]},
                {"name":"has space"},
                {"name":"has:colon"},
                {"name":"ctl\u{1}"},
                {"name":"x".repeat(NAME_LIMIT + 1)},
                {"name":""},
                {"name":7},
                {"name":"bad_required","arguments":[{"name":"a","required":"yes"}]},
                {"name":"bad_arg","arguments":[{"name":"a b"}]},
                {"name":"dup_arg","arguments":[{"name":"a"},{"name":"a"}]},
                {"name":"bad_description","description":5},
                {"name":"review_pr"},
                {"name":"y".repeat(NAME_LIMIT)}
            ]}),
        )
        .unwrap();
        assert_eq!(next, None);
        assert_eq!(
            catalog.prompts,
            [
                DiscoveredPrompt {
                    name: "review_pr".into(),
                    description: "Review a PR".into(),
                    arguments: vec![
                        PromptArgument {
                            name: "number".into(),
                            required: true
                        },
                        PromptArgument {
                            name: "focus".into(),
                            required: false
                        },
                    ],
                },
                DiscoveredPrompt {
                    name: "y".repeat(NAME_LIMIT),
                    description: String::new(),
                    arguments: Vec::new(),
                },
            ]
        );
    }

    #[test]
    fn repeated_or_invalid_cursor_is_rejected() {
        let mut catalog = Catalog::default();
        assert_eq!(
            page(&mut catalog, json!({"prompts":[],"nextCursor":"a"})).unwrap(),
            Some("a".into())
        );
        assert!(page(&mut catalog, json!({"prompts":[],"nextCursor":"a"})).is_err());
        for cursor in [json!(5), json!(""), json!("a\nb"), json!("c".repeat(4097))] {
            let mut catalog = Catalog::default();
            assert!(
                page(&mut catalog, json!({"prompts":[],"nextCursor":cursor})).is_err(),
                "{cursor}"
            );
        }
        assert!(page(&mut Catalog::default(), json!({"tools":[]})).is_err());
    }

    #[test]
    fn catalog_byte_cap_is_enforced() {
        let mut catalog = Catalog::default();
        let filler = "d".repeat(super::super::LIMIT - 64);
        for index in 0..4 {
            assert!(
                page(
                    &mut catalog,
                    json!({"prompts":[{"name":"p","description":filler}],"nextCursor":index.to_string()})
                )
                .is_ok()
            );
        }
        let error = page(
            &mut catalog,
            json!({"prompts":[{"name":"p","description":filler}]}),
        )
        .unwrap_err();
        assert_eq!(
            error.to_string(),
            "MCP protocol error: MCP prompt catalog exceeds byte limit"
        );
    }

    #[test]
    fn prompt_messages_are_validated_and_bounded() {
        let parsed = messages(&json!({"messages":[
            {"role":"user","content":{"type":"text","text":"hi"}},
            {"role":"assistant","content":{"type":"image","data":"AA==","mimeType":"image/png"}},
            {"role":"user","content":{"type":"resource","resource":{"uri":"file:///x","text":"x"}}}
        ]}))
        .unwrap();
        assert_eq!(
            parsed,
            [
                PromptMessage {
                    role: PromptRole::User,
                    content: PromptContent::Text("hi".into())
                },
                PromptMessage {
                    role: PromptRole::Assistant,
                    content: PromptContent::Omitted("image")
                },
                PromptMessage {
                    role: PromptRole::User,
                    content: PromptContent::Omitted("embedded resource")
                },
            ]
        );
        for result in [
            json!({}),
            json!({"messages":[{"role":"system","content":{"type":"text","text":"x"}}]}),
            json!({"messages":[{"role":"user","content":{"type":"text"}}]}),
            json!({"messages":[{"role":"user","content":{}}]}),
            json!({"messages":[{"role":"user","content":{"type":"text","text":"x".repeat(PROMPT_TEXT_LIMIT + 1)}}]}),
        ] {
            assert!(messages(&result).is_err(), "{result}");
        }
    }
}
