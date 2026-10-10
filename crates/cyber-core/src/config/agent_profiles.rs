//! Shared built-in and configured agent profiles from resolved configuration.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct AgentTools {
    pub allow: Option<Vec<String>>,
    pub deny: Vec<String>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct AgentProfile {
    #[serde(skip_deserializing)]
    pub name: String,
    #[serde(skip_deserializing)]
    pub builtin: bool,
    /// Built-in exploration/review identity; a model patch cannot remove it.
    #[serde(skip_deserializing)]
    pub read_only: bool,
    pub description: String,
    pub system: Option<String>,
    pub model: Option<String>,
    pub variant: Option<String>,
    pub mode: String,
    pub permission_mode: Option<String>,
    pub tools: AgentTools,
    pub permissions: Value,
    pub request: Value,
    pub steps: Option<u64>,
    pub color: Option<String>,
    pub hidden: bool,
    pub isolation: Option<String>,
    pub background: bool,
    pub memory: Option<String>,
    pub skills: Vec<String>,
    pub mcp: Option<Vec<String>>,
}

impl AgentProfile {
    pub fn subagent_capable(&self) -> bool {
        !self.hidden && matches!(self.mode.as_str(), "subagent" | "all")
    }

    pub fn primary_capable(&self) -> bool {
        !self.hidden && matches!(self.mode.as_str(), "primary" | "all")
    }
}

/// Input must be the already layered, substituted and trust-filtered document.
/// This resolver performs validation again for non-loader callers.
pub fn resolve_agents(config: &Value) -> Result<BTreeMap<String, AgentProfile>, String> {
    let mut issues = Vec::new();
    super::agents::check(config.get("agents"), &mut issues);
    if !issues.is_empty() {
        return Err(issues.join("; "));
    }
    let mut profiles = builtins();
    if let Some(agents) = config.get("agents").and_then(Value::as_object) {
        for (name, patch) in agents.iter().filter(|(_, value)| value.is_object()) {
            let profile = profiles
                .entry(name.clone())
                .or_insert_with(|| json!({"description":"", "mode":"all"}));
            super::merge::merge_layer(profile, patch, "agent", &mut BTreeMap::new());
        }
    }
    let mut resolved = BTreeMap::new();
    for (name, mut value) in profiles {
        if system_agent(&name) {
            value["hidden"] = json!(true);
            value["disabled"] = json!(false);
            value["mode"] = json!("primary");
            value["tools"] = json!({"deny":["*"]});
            value["permissions"] = json!("deny");
        }
        if value["disabled"] == true {
            continue;
        }
        let mut profile: AgentProfile =
            serde_json::from_value(value).map_err(|error| format!("agents.{name}: {error}"))?;
        profile.builtin = builtin_agent(&name);
        profile.read_only = matches!(name.as_str(), "explore" | "reviewer");
        profile.name = name.clone();
        resolved.insert(name, profile);
    }
    Ok(resolved)
}

fn builtin_agent(name: &str) -> bool {
    matches!(name, "build" | "explore" | "general" | "reviewer") || system_agent(name)
}

fn system_agent(name: &str) -> bool {
    matches!(name, "compaction" | "title" | "summary" | "evaluator")
}

fn builtins() -> BTreeMap<String, Value> {
    let mut profiles = BTreeMap::from([
        (
            "build".into(),
            json!({"description":"The default agent, with full tool access", "mode":"primary"}),
        ),
        (
            "explore".into(),
            json!({"description":"Explore code and answer questions without changing files", "mode":"subagent", "steps":50,
            "tools":{"allow":["read","glob","grep","webfetch","websearch","bash"]}}),
        ),
        (
            "reviewer".into(),
            json!({"description":"Review changes and report structured findings without modifying files", "mode":"subagent", "permission_mode":"plan", "steps":50,
            "tools":{"allow":["read","glob","grep","webfetch","websearch","bash"]}}),
        ),
        (
            "general".into(),
            json!({"description":"Run coding subtasks with full tool access", "mode":"subagent", "steps":50,
            "tools":{"deny":["todo"]}, "permissions":{"todo":"deny"}}),
        ),
    ]);
    for name in ["compaction", "title", "summary", "evaluator"] {
        profiles.insert(
            name.into(),
            json!({"description":"Internal system agent", "mode":"primary", "hidden":true,
            "tools":{"deny":["*"]}, "permissions":"deny"}),
        );
    }
    profiles
}
