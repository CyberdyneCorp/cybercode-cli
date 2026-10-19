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
    /// Steps of the Session and its descendants without a price.
    pub unpriced_steps: u64,
    /// Cost of descendant Sessions; `children_incomplete` when some of it is lost.
    pub children_cost: f64,
    pub children_incomplete: bool,
    /// The context window meter after the last finished step.
    pub context: Option<ContextMeter>,
    /// Why the Session's `messaging.inbound` override refuses messages
    /// (`non_interactive`), which `a` in `/messages` clears.
    pub inbound_reason: Option<String>,
}

/// `observability-costs` → Context window meter, as the server reports it.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct ContextMeter {
    pub used_tokens: u64,
    pub window: Option<u64>,
    pub compact_at: Option<u64>,
    /// `used_tokens` comes from a finished step; false before the first reply and after
    /// a compaction, when only the window is known.
    pub measured: bool,
    /// No reply has finished since the last compaction.
    pub compacted: bool,
}

impl ContextMeter {
    pub fn parse(v: &Value) -> Option<Self> {
        Some(Self {
            used_tokens: v["used_tokens"].as_u64()?,
            window: v["window"].as_u64().filter(|w| *w > 0),
            compact_at: v["compact_at"].as_u64(),
            measured: v["measured"].as_bool().unwrap_or(true),
            compacted: v["compacted"].as_bool().unwrap_or(false),
        })
    }

    /// The auto-compaction point as a percent of the window.
    pub fn compact_percent(&self) -> Option<f64> {
        let window = self.window?;
        Some(self.compact_at? as f64 * 100.0 / window as f64)
    }

    /// Percent of the window used, when the window is known.
    pub fn percent(&self) -> Option<f64> {
        self.window
            .map(|window| self.used_tokens as f64 * 100.0 / window as f64)
    }
}

impl Session {
    /// The meter once a reply has measured it: the footer and Status tab show nothing
    /// before that, the sidebar shows the window alone.
    pub fn measured_context(&self) -> Option<ContextMeter> {
        self.context.filter(|meter| meter.measured)
    }

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
            unpriced_steps: v["totals"]["unpriced_steps"].as_u64().unwrap_or(0)
                + v["children_unpriced_steps"].as_u64().unwrap_or(0),
            children_cost: v["children_cost"].as_f64().unwrap_or(0.0),
            children_incomplete: v["children_usage_complete"] == false,
            context: ContextMeter::parse(&v["context"]),
            inbound_reason: v["messaging"]["reason"].as_str().map(str::to_string),
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

/// Durable timing and attribution of one assistant step (`session-runtime` →
/// Assistant message timing).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Step {
    pub agent: String,
    pub provider: String,
    pub model: String,
    pub started_ms: Option<i64>,
    /// `None` while the step runs.
    pub ended_ms: Option<i64>,
    /// The failure kind, such as `interrupted`.
    pub error_kind: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Item {
    User {
        id: String,
        text: String,
        promoted_ms: Option<i64>,
    },
    Assistant {
        id: String,
        text: String,
        reasoning: String,
        tools: Vec<Tool>,
        error: Option<String>,
        step: Step,
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
                promoted_ms: v["promoted_ms"].as_i64(),
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
                step: Step {
                    agent: v["agent"].as_str().unwrap_or_default().into(),
                    provider: v["provider"].as_str().unwrap_or_default().into(),
                    model: v["model"].as_str().unwrap_or_default().into(),
                    started_ms: v["started_ms"].as_i64(),
                    ended_ms: v["ended_ms"].as_i64(),
                    error_kind: v["error_kind"].as_str().map(str::to_string),
                },
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
    /// Recorded but not promotable until released.
    pub held: bool,
}

impl Queued {
    /// Unpromoted rows only.
    pub fn parse(v: &Value) -> Option<Self> {
        let held = match v["status"].as_str() {
            Some("pending") => false,
            Some("held") => true,
            _ => return None,
        };
        Some(Self {
            message_id: v["message_id"].as_str()?.into(),
            text: parts_text(&v["parts"]),
            delivery: v["delivery"].as_str().unwrap_or_default().into(),
            held,
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

/// Model and agent metadata from `GET /models` and `GET /agents`, for display names
/// and agent colors.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Catalog {
    /// `provider/model` → display name.
    pub models: Vec<(String, String)>,
    /// Agent name → configured color, as written.
    pub agents: Vec<(String, Option<String>)>,
}

impl Catalog {
    pub fn parse(models: &Value, agents: &Value) -> Self {
        let rows = |v: &Value| v.as_array().cloned().unwrap_or_default();
        Self {
            models: rows(models)
                .iter()
                .filter_map(|m| {
                    let id = m["id"].as_str()?;
                    let name = m["name"].as_str().filter(|n| !n.trim().is_empty())?;
                    Some((id.to_owned(), name.to_owned()))
                })
                .collect(),
            agents: rows(agents)
                .iter()
                .filter_map(|a| {
                    let name = a["name"].as_str()?;
                    Some((name.to_owned(), a["color"].as_str().map(str::to_owned)))
                })
                .collect(),
        }
    }

    /// The display name of `provider/model[#variant]`, else its model ID.
    pub fn model_name(&self, reference: &str) -> String {
        let base = reference.split('#').next().unwrap_or(reference);
        let name = self
            .models
            .iter()
            .find(|(id, _)| id == base)
            .map(|(_, name)| name.as_str())
            .unwrap_or_else(|| base.split_once('/').map_or(base, |(_, model)| model));
        name.chars().filter(|c| !c.is_control()).collect()
    }

    pub fn agent_color(&self, agent: &str) -> Option<&str> {
        self.agents
            .iter()
            .find(|(name, _)| name == agent)
            .and_then(|(_, color)| color.as_deref())
    }
}
