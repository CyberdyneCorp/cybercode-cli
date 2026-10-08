//! Session data as the TUI shows it, parsed leniently from API responses.

use serde_json::Value;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Session {
    pub id: String,
    pub parent_id: Option<String>,
    pub title: String,
    pub directory: String,
    pub model: String,
    pub agent: String,
    pub mode: String,
    pub effective_mode: String,
    pub pending_mode: Option<String>,
    pub running: bool,
    pub seq: i64,
    pub cost: f64,
    pub input_tokens: u64,
    pub output_tokens: u64,
}

impl Session {
    pub fn mode_label(&self) -> String {
        let effective = if self.effective_mode.is_empty() {
            &self.mode
        } else {
            &self.effective_mode
        };
        let current = if effective == "plan" {
            "⏸ plan"
        } else {
            effective
        };
        match &self.pending_mode {
            Some(pending) => format!("{current} → {pending} (pending)"),
            None => current.to_string(),
        }
    }

    pub fn parse(v: &Value) -> Self {
        let s = |k: &str| v[k].as_str().unwrap_or_default().to_string();
        Self {
            id: s("id"),
            parent_id: v["parent_id"].as_str().map(str::to_string),
            title: s("title"),
            directory: s("directory"),
            model: s("model"),
            agent: s("agent"),
            mode: s("mode"),
            effective_mode: v["effective_mode"]
                .as_str()
                .unwrap_or_else(|| v["mode"].as_str().unwrap_or_default())
                .into(),
            pending_mode: v["pending_mode"].as_str().map(str::to_string),
            running: v["status"] == "running",
            seq: v["seq"].as_i64().unwrap_or(-1),
            cost: v["totals"]["cost"].as_f64().unwrap_or(0.0),
            input_tokens: v["totals"]["usage"]["input"].as_u64().unwrap_or(0),
            output_tokens: v["totals"]["usage"]["output"].as_u64().unwrap_or(0),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Tool {
    pub call_id: String,
    pub name: String,
    pub input: Value,
    /// `called`, `dispatched`, `ok`, `error`, `interrupted`, `outcome_unknown`.
    pub status: String,
    pub output: String,
}

impl Tool {
    /// The display status from the spec: pending, running, completed, failed, interrupted.
    pub fn display_status(&self) -> &'static str {
        match self.status.as_str() {
            "called" => "pending",
            "dispatched" => "running",
            "ok" => "completed",
            "interrupted" => "interrupted",
            "outcome_unknown" => "unknown",
            _ => "failed",
        }
    }

    /// One-line summary of the input.
    pub fn summary(&self) -> String {
        for key in ["command", "path", "pattern", "url", "query", "name"] {
            if let Some(v) = self.input[key].as_str() {
                return v.lines().next().unwrap_or_default().to_string();
            }
        }
        if self.name == "apply_patch" {
            return "patch".into();
        }
        String::new()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Item {
    User {
        id: String,
        text: String,
    },
    Assistant {
        id: String,
        text: String,
        reasoning: String,
        tools: Vec<Tool>,
        error: Option<String>,
    },
    System {
        id: String,
        text: String,
    },
}

impl Item {
    pub fn id(&self) -> &str {
        match self {
            Item::User { id, .. } | Item::Assistant { id, .. } | Item::System { id, .. } => id,
        }
    }

    pub fn parse(v: &Value) -> Option<Self> {
        let id = v["id"].as_str()?.to_string();
        match v["kind"].as_str()? {
            "user" => Some(Item::User {
                id,
                text: parts_text(&v["parts"]),
            }),
            "system" => Some(Item::System {
                id,
                text: v["text"].as_str().unwrap_or_default().into(),
            }),
            "assistant" => Some(Item::Assistant {
                id,
                text: v["text"].as_str().unwrap_or_default().into(),
                reasoning: v["reasoning"].as_str().unwrap_or_default().into(),
                tools: v["tools"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(tool)
                    .collect(),
                error: v["error"].as_str().map(str::to_string),
            }),
            _ => None,
        }
    }
}

fn tool(v: &Value) -> Tool {
    let s = |k: &str| v[k].as_str().unwrap_or_default().to_string();
    Tool {
        call_id: s("call_id"),
        name: s("name"),
        input: v["input"].clone(),
        status: s("status"),
        output: s("output"),
    }
}

pub fn parts_text(parts: &Value) -> String {
    parts
        .as_array()
        .into_iter()
        .flatten()
        .map(|p| match p["type"].as_str() {
            Some("text") => p["text"].as_str().unwrap_or_default().to_string(),
            Some("image") => "[Image]".to_string(),
            _ => String::new(),
        })
        .filter(|t| !t.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

/// A queued or held input.
#[derive(Debug, Clone, PartialEq)]
pub struct Queued {
    pub message_id: String,
    pub text: String,
    pub delivery: String,
}

impl Queued {
    /// Unpromoted rows only.
    pub fn parse(v: &Value) -> Option<Self> {
        if !matches!(v["status"].as_str(), Some("pending" | "held")) {
            return None;
        }
        Some(Self {
            message_id: v["message_id"].as_str()?.into(),
            text: parts_text(&v["parts"]),
            delivery: v["delivery"].as_str().unwrap_or_default().into(),
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct QuestionSpec {
    pub question: String,
    pub header: String,
    pub options: Vec<(String, String)>,
    pub multi: bool,
    pub custom: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Request {
    Permission {
        id: String,
        session_id: String,
        origin: Option<String>,
        routed_to: Vec<String>,
        action: String,
        resources: Vec<String>,
        patterns: Vec<String>,
        metadata: Value,
    },
    Question {
        id: String,
        session_id: String,
        origin: Option<String>,
        routed_to: Vec<String>,
        questions: Vec<QuestionSpec>,
    },
}

impl Request {
    pub fn id(&self) -> &str {
        match self {
            Request::Permission { id, .. } | Request::Question { id, .. } => id,
        }
    }

    pub fn session(&self) -> &str {
        match self {
            Request::Permission { session_id, .. } | Request::Question { session_id, .. } => {
                session_id
            }
        }
    }

    pub fn parse(v: &Value) -> Option<Self> {
        let id = v["id"].as_str()?.to_string();
        let session_id = v["session_id"].as_str()?.to_string();
        let origin = v["origin"]["title"].as_str().map(str::to_string);
        let routed_to = v["routed_to"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|route| route["session_id"].as_str().map(str::to_string))
            .collect();
        let strings = |k: &str| {
            v[k].as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect::<Vec<_>>()
        };
        match v["kind"].as_str()? {
            "permission" => Some(Request::Permission {
                id,
                session_id,
                origin,
                routed_to,
                action: v["action"].as_str().unwrap_or_default().into(),
                resources: strings("resources"),
                patterns: strings("always_patterns"),
                metadata: v["metadata"].clone(),
            }),
            "question" => Some(Request::Question {
                id,
                session_id,
                origin,
                routed_to,
                questions: v["questions"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(question)
                    .collect(),
            }),
            _ => None,
        }
    }

    pub fn visible_in(&self, session_id: &str) -> bool {
        let routes = match self {
            Self::Permission { routed_to, .. } | Self::Question { routed_to, .. } => routed_to,
        };
        self.session() == session_id || routes.iter().any(|route| route == session_id)
    }

    pub fn origin_title(&self) -> Option<&str> {
        match self {
            Self::Permission { origin, .. } | Self::Question { origin, .. } => origin.as_deref(),
        }
    }
}

fn question(v: &Value) -> QuestionSpec {
    QuestionSpec {
        question: v["question"].as_str().unwrap_or_default().into(),
        header: v["header"].as_str().unwrap_or_default().into(),
        options: v["options"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|o| {
                (
                    o["label"].as_str().unwrap_or_default().to_string(),
                    o["description"].as_str().unwrap_or_default().to_string(),
                )
            })
            .collect(),
        multi: v["multi_select"].as_bool().unwrap_or(false),
        custom: v["allow_custom"].as_bool().unwrap_or(false),
    }
}

/// A row in a picker.
#[derive(Debug, Clone, PartialEq)]
pub struct Choice {
    pub key: String,
    pub label: String,
    pub detail: String,
}
