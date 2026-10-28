//! Model behavior profiles (`provider-catalog` → Model behavior profiles): one typed record
//! of how a model wants to be spoken to, resolved field by field from four layers: family
//! and protocol defaults, built-in name and provider rules, catalog metadata, then config.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{Model, Provider, model_family};
use crate::adapters::ApiKind;
use crate::types::Reasoning;

/// Which assistant messages get their reasoning items back natively.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum ReasoningReplay {
    /// Lower reasoning to text.
    #[default]
    Never,
    /// Native items for the assistant messages after the latest user message.
    ToolLoop,
    /// Native items for the whole history.
    Always,
}

/// The request field that carries reasoning state back to the provider.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum ReasoningCarrier {
    #[default]
    None,
    Responses,
    Anthropic,
    ReasoningContent,
    Reasoning,
    ReasoningDetails,
    GeminiSignature,
    CopilotOpaque,
}

impl ReasoningCarrier {
    pub fn is_none(&self) -> bool {
        *self == Self::None
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum VerbosityLevel {
    Low,
    Medium,
    High,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, schemars::JsonSchema)]
pub struct Verbosity {
    pub supported: bool,
    pub default: Option<VerbosityLevel>,
}

/// The tool shape the model edits files with.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum EditTool {
    FreeformPatch,
    JsonPatch,
    #[default]
    Edit,
}

/// Which part of a long tool output the model sees.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum Truncation {
    #[default]
    Head,
    Tail,
    Middle,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, schemars::JsonSchema)]
pub struct ToolOutput {
    /// Token budget of a model-facing tool output; `None` keeps the byte limit alone.
    pub tokens: Option<u32>,
    pub truncation: Truncation,
}

/// What happens to a derived effort the model does not list.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum EffortClamp {
    #[default]
    NearestLower,
    Fail,
}

/// A model's resolved behavior profile.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, schemars::JsonSchema)]
pub struct ModelProfile {
    pub reasoning_replay: ReasoningReplay,
    pub reasoning_carrier: ReasoningCarrier,
    /// The model accepts a reasoning summary request.
    pub reasoning_summary: bool,
    pub verbosity: Verbosity,
    pub parallel_tool_calls: bool,
    pub edit_tool: EditTool,
    pub tool_output: ToolOutput,
    /// Reasoning efforts the model accepts, lowest first.
    pub efforts: Vec<String>,
    pub effort_clamp: EffortClamp,
}

impl Default for ModelProfile {
    fn default() -> Self {
        Self {
            reasoning_replay: ReasoningReplay::Never,
            reasoning_carrier: ReasoningCarrier::None,
            reasoning_summary: false,
            verbosity: Verbosity::default(),
            parallel_tool_calls: true,
            edit_tool: EditTool::Edit,
            tool_output: ToolOutput::default(),
            efforts: Vec::new(),
            effort_clamp: EffortClamp::NearestLower,
        }
    }
}

/// One layer's fields; unset fields leave the earlier layer's value.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfilePatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_replay: Option<ReasoningReplay>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_carrier: Option<ReasoningCarrier>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_summary: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verbosity_supported: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verbosity_default: Option<VerbosityLevel>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parallel_tool_calls: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub edit_tool: Option<EditTool>,
    /// `0` clears the token budget.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_output_tokens: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub truncation: Option<Truncation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub efforts: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort_clamp: Option<EffortClamp>,
}

impl ProfilePatch {
    const EMPTY: Self = Self {
        reasoning_replay: None,
        reasoning_carrier: None,
        reasoning_summary: None,
        verbosity_supported: None,
        verbosity_default: None,
        parallel_tool_calls: None,
        edit_tool: None,
        tool_output_tokens: None,
        truncation: None,
        efforts: None,
        effort_clamp: None,
    };

    /// Config `providers.<p>.models.<m>.profile`. A key or value that does not parse is
    /// skipped; configuration validation warns about it (`cyber_core::config`).
    pub fn from_config(value: Option<&Value>) -> Self {
        let Some(map) = value.and_then(Value::as_object) else {
            return Self::default();
        };
        let verbosity = map.get("verbosity");
        let output = map.get("tool_output");
        Self {
            reasoning_replay: parsed(map.get("reasoning_replay")),
            reasoning_carrier: parsed(map.get("reasoning_carrier")),
            reasoning_summary: parsed(map.get("reasoning_summary")),
            verbosity_supported: parsed(verbosity.and_then(|v| v.get("supported"))),
            verbosity_default: parsed(verbosity.and_then(|v| v.get("default"))),
            parallel_tool_calls: parsed(map.get("parallel_tool_calls")),
            edit_tool: parsed(map.get("edit_tool")),
            tool_output_tokens: parsed(output.and_then(|v| v.get("tokens"))),
            truncation: parsed(output.and_then(|v| v.get("truncation"))),
            efforts: parsed(map.get("efforts")),
            effort_clamp: parsed(map.get("effort_clamp")),
        }
    }
}

fn parsed<T: serde::de::DeserializeOwned>(value: Option<&Value>) -> Option<T> {
    serde_json::from_value(value?.clone()).ok()
}

impl ModelProfile {
    /// Lay `patch` over this profile, field by field.
    pub fn apply(&mut self, patch: &ProfilePatch) {
        set(&mut self.reasoning_replay, patch.reasoning_replay);
        set(&mut self.reasoning_carrier, patch.reasoning_carrier);
        set(&mut self.reasoning_summary, patch.reasoning_summary);
        set(&mut self.verbosity.supported, patch.verbosity_supported);
        if let Some(level) = patch.verbosity_default {
            self.verbosity.default = Some(level);
        }
        set(&mut self.parallel_tool_calls, patch.parallel_tool_calls);
        set(&mut self.edit_tool, patch.edit_tool);
        if let Some(tokens) = patch.tool_output_tokens {
            self.tool_output.tokens = (tokens > 0).then_some(tokens);
        }
        set(&mut self.tool_output.truncation, patch.truncation);
        if let Some(efforts) = &patch.efforts {
            self.efforts = sorted_efforts(efforts.iter().map(String::as_str));
        }
        set(&mut self.effort_clamp, patch.effort_clamp);
    }
}

fn set<T>(slot: &mut T, value: Option<T>) {
    if let Some(value) = value {
        *slot = value;
    }
}

/// A built-in rule: a model-name prefix (empty matches every model), optionally scoped to
/// a provider or to the Chat Completions protocol, where these carriers live.
struct Rule {
    provider: Option<&'static str>,
    prefix: &'static str,
    chat_only: bool,
    patch: ProfilePatch,
}

const OPENAI_REASONING: ProfilePatch = ProfilePatch {
    reasoning_summary: Some(true),
    verbosity_supported: Some(true),
    edit_tool: Some(EditTool::FreeformPatch),
    tool_output_tokens: Some(10_000),
    truncation: Some(Truncation::Middle),
    ..ProfilePatch::EMPTY
};

const fn carrier(carrier: ReasoningCarrier) -> ProfilePatch {
    ProfilePatch {
        reasoning_carrier: Some(carrier),
        reasoning_replay: Some(ReasoningReplay::ToolLoop),
        ..ProfilePatch::EMPTY
    }
}

const fn by_name(prefix: &'static str, chat_only: bool, patch: ProfilePatch) -> Rule {
    Rule {
        provider: None,
        prefix,
        chat_only,
        patch,
    }
}

const fn by_provider(provider: &'static str, patch: ProfilePatch) -> Rule {
    Rule {
        provider: Some(provider),
        prefix: "",
        chat_only: false,
        patch,
    }
}

/// Applied in order, so the provider rules, which know their gateway's carrier, come last.
const RULES: &[Rule] = &[
    by_name("gpt-5", false, OPENAI_REASONING),
    by_name("gpt-6", false, OPENAI_REASONING),
    by_name("o3", false, OPENAI_REASONING),
    by_name("o4", false, OPENAI_REASONING),
    by_name(
        "deepseek",
        true,
        carrier(ReasoningCarrier::ReasoningContent),
    ),
    by_name("kimi", true, carrier(ReasoningCarrier::ReasoningContent)),
    by_name("glm", true, carrier(ReasoningCarrier::ReasoningContent)),
    by_name("qwen", true, carrier(ReasoningCarrier::ReasoningContent)),
    by_name("gemini-3", true, carrier(ReasoningCarrier::GeminiSignature)),
    by_provider("openrouter", carrier(ReasoningCarrier::ReasoningDetails)),
    by_provider("ollama", carrier(ReasoningCarrier::Reasoning)),
    by_provider("lmstudio", carrier(ReasoningCarrier::Reasoning)),
    by_provider("copilot", carrier(ReasoningCarrier::CopilotOpaque)),
];

/// The profile of `model`: defaults, rules, catalog metadata, then config.
pub fn resolve(provider: &Provider, model: &Model) -> ModelProfile {
    let kind = model.api_kind.or(provider.kind);
    let mut profile = defaults(provider, model, kind);
    let name = model
        .api_id
        .rsplit('/')
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    let chat = kind == Some(ApiKind::OpenaiCompatible);
    for rule in RULES {
        let applies = rule.provider.is_none_or(|id| id == provider.id)
            && name.starts_with(rule.prefix)
            && (chat || !rule.chat_only);
        if applies {
            profile.apply(&rule.patch);
        }
    }
    profile.apply(&model.profile_catalog);
    profile.apply(&model.profile_config);
    if profile.efforts.is_empty() {
        profile.efforts = sorted_efforts(
            model
                .variants
                .values()
                .filter_map(|v| effort_of(&v.reasoning)),
        );
    }
    profile
}

/// The Responses request fields `model`'s resolved profile allows
/// (`provider-catalog` → Responses request shaping).
pub(super) fn shaping(model: &Model) -> crate::types::Shaping {
    let profile = &model.profile;
    crate::types::Shaping {
        reasoning: model.capabilities.reasoning,
        reasoning_summary: profile.reasoning_summary,
        effort_none: profile.efforts.iter().any(|e| e == "none"),
        verbosity: profile
            .verbosity
            .supported
            .then_some(profile.verbosity.default)
            .flatten(),
        parallel_tool_calls: Some(profile.parallel_tool_calls),
    }
}

/// Warn, once per model and process, about a configured verbosity default the model
/// does not support; it is never sent.
pub(super) fn warn_unsupported_verbosity(provider: &str, model: &Model) {
    static WARNED: std::sync::Mutex<std::collections::BTreeSet<String>> =
        std::sync::Mutex::new(std::collections::BTreeSet::new());
    if model.profile_config.verbosity_default.is_none() || model.profile.verbosity.supported {
        return;
    }
    let id = format!("{provider}/{}", model.id);
    let fresh = WARNED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .insert(id.clone());
    if fresh {
        cyber_core::log::warn(
            "catalog",
            "configured verbosity default ignored: the model does not support verbosity",
            serde_json::json!({ "model": id }),
        );
    }
}

/// Layer 1: the protocol's carrier, and the patch tool for a model whose family edits
/// with it (today's `prefers_apply_patch`).
fn defaults(provider: &Provider, model: &Model, kind: Option<ApiKind>) -> ModelProfile {
    let mut profile = ModelProfile::default();
    let native = match kind {
        Some(ApiKind::OpenaiResponses | ApiKind::ChatgptCodex) => Some(ReasoningCarrier::Responses),
        Some(ApiKind::Anthropic) => Some(ReasoningCarrier::Anthropic),
        _ => None,
    };
    if let Some(carrier) = native {
        profile.reasoning_carrier = carrier;
        profile.reasoning_replay = ReasoningReplay::Always;
    }
    let openai = model_family(provider, model) == cyber_core::prompts::ModelFamily::Openai;
    if openai && model.capabilities.prefers_apply_patch {
        profile.edit_tool = EditTool::JsonPatch;
    }
    profile
}

/// Reasoning effort levels, lowest first. A clamp moves down this ladder.
pub const EFFORT_LADDER: &[&str] = &["none", "minimal", "low", "medium", "high", "xhigh", "max"];

pub fn effort_rank(effort: &str) -> Option<usize> {
    EFFORT_LADDER.iter().position(|level| *level == effort)
}

/// The effort a variant selects; `Off` is `none`.
pub(super) fn effort_of(reasoning: &Option<Reasoning>) -> Option<&str> {
    match reasoning.as_ref()? {
        Reasoning::Effort(effort) => Some(effort),
        Reasoning::Off => Some("none"),
        Reasoning::BudgetTokens(_) => None,
    }
}

fn sorted_efforts<'a>(efforts: impl Iterator<Item = &'a str>) -> Vec<String> {
    let mut out: Vec<String> = efforts.map(str::to_owned).collect();
    out.sort_by_key(|effort| effort_rank(effort).unwrap_or(usize::MAX));
    out.dedup();
    out
}

/// Catalog metadata from a models.dev entry: a model without reasoning has nothing to
/// replay or summarize, and one without tool calls makes no parallel calls.
pub(super) fn from_models_dev(entry: &Value) -> ProfilePatch {
    let mut patch = ProfilePatch::default();
    if entry.get("reasoning").and_then(Value::as_bool) == Some(false) {
        patch.reasoning_replay = Some(ReasoningReplay::Never);
        patch.reasoning_summary = Some(false);
    }
    if entry.get("tool_call").and_then(Value::as_bool) == Some(false) {
        patch.parallel_tool_calls = Some(false);
    }
    patch
}

/// Catalog metadata from a ChatGPT `/models` entry.
pub(crate) fn from_chatgpt(entry: &Value) -> ProfilePatch {
    let output = &entry["truncation_policy"];
    let limit = output["limit"].as_u64().filter(|limit| *limit > 0);
    let tokens = match output["mode"].as_str() {
        Some("tokens") => limit,
        Some("bytes") => limit.map(|bytes| bytes.div_ceil(4)),
        _ => None,
    };
    ProfilePatch {
        reasoning_summary: entry["supports_reasoning_summary_parameter"].as_bool(),
        verbosity_supported: entry["support_verbosity"].as_bool(),
        verbosity_default: parsed(Some(&entry["default_verbosity"])),
        edit_tool: match entry["apply_patch_tool_type"].as_str() {
            Some("freeform") => Some(EditTool::FreeformPatch),
            Some("function") => Some(EditTool::JsonPatch),
            _ => None,
        },
        tool_output_tokens: tokens.map(|t| u32::try_from(t).unwrap_or(u32::MAX)),
        truncation: tokens.map(|_| Truncation::Middle),
        ..ProfilePatch::default()
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn catalog_model(data: Value, provider: &str, model: &str) -> (Provider, Model) {
        let providers = super::super::models_dev::parse(&data);
        let p = providers[provider].clone();
        let m = p.models[model].clone();
        (p, m)
    }

    fn compatible(provider: &str, model: &str) -> (Provider, Model) {
        let data = json!({ provider: {
            "npm": "@ai-sdk/openai-compatible", "api": "https://llm.example/v1",
            "models": { model: { "tool_call": true, "reasoning": true } },
        }});
        catalog_model(data, provider, model)
    }

    #[test]
    fn family_and_protocol_defaults() {
        let data = json!({
            "openai": { "npm": "@ai-sdk/openai", "models": { "gpt-4.1": { "tool_call": true } } },
            "anthropic": { "npm": "@ai-sdk/anthropic", "models": { "claude-x": {} } },
        });
        let (p, m) = catalog_model(data.clone(), "openai", "gpt-4.1");
        let openai = resolve(&p, &m);
        assert_eq!(openai.reasoning_carrier, ReasoningCarrier::Responses);
        assert_eq!(openai.reasoning_replay, ReasoningReplay::Always);
        assert_eq!(openai.edit_tool, EditTool::JsonPatch);
        let (p, m) = catalog_model(data, "anthropic", "claude-x");
        let anthropic = resolve(&p, &m);
        assert_eq!(anthropic.reasoning_carrier, ReasoningCarrier::Anthropic);
        assert_eq!(anthropic.edit_tool, EditTool::Edit);
        let (p, m) = compatible("corp", "house-model");
        let plain = ModelProfile {
            efforts: vec!["low".into(), "medium".into(), "high".into()],
            ..ModelProfile::default()
        };
        assert_eq!(resolve(&p, &m), plain);
    }

    #[test]
    fn a_name_rule_wins_over_the_defaults() {
        let (p, m) = compatible("corp", "kimi-k2-thinking");
        let kimi = resolve(&p, &m);
        assert_eq!(kimi.reasoning_carrier, ReasoningCarrier::ReasoningContent);
        assert_eq!(kimi.reasoning_replay, ReasoningReplay::ToolLoop);
        let data = json!({ "openai": { "npm": "@ai-sdk/openai",
            "models": { "gpt-6-sol": { "tool_call": true, "reasoning": true } } } });
        let (p, m) = catalog_model(data, "openai", "gpt-6-sol");
        let gpt = resolve(&p, &m);
        assert_eq!(gpt.edit_tool, EditTool::FreeformPatch);
        assert_eq!(
            gpt.tool_output,
            ToolOutput {
                tokens: Some(10_000),
                truncation: Truncation::Middle
            }
        );
        assert!(gpt.verbosity.supported && gpt.reasoning_summary);
        assert_eq!(gpt.reasoning_carrier, ReasoningCarrier::Responses);
    }

    #[test]
    fn chat_carrier_rules_need_the_chat_protocol_and_provider_rules_come_last() {
        let data = json!({ "deepseek": { "npm": "@ai-sdk/anthropic",
            "models": { "deepseek-v4": { "reasoning": true } } } });
        let (p, m) = catalog_model(data, "deepseek", "deepseek-v4");
        assert_eq!(
            resolve(&p, &m).reasoning_carrier,
            ReasoningCarrier::Anthropic
        );
        let (p, m) = compatible("openrouter", "deepseek/deepseek-v4");
        assert_eq!(
            resolve(&p, &m).reasoning_carrier,
            ReasoningCarrier::ReasoningDetails
        );
        let (p, m) = compatible("ollama", "qwen3");
        assert_eq!(
            resolve(&p, &m).reasoning_carrier,
            ReasoningCarrier::Reasoning
        );
        let (p, m) = compatible("corp", "gemini-3-pro");
        assert_eq!(
            resolve(&p, &m).reasoning_carrier,
            ReasoningCarrier::GeminiSignature
        );
    }

    #[test]
    fn catalog_metadata_wins_over_the_rules() {
        let (p, mut m) = compatible("corp", "kimi-k2");
        m.profile_catalog = from_models_dev(&json!({"reasoning": false, "tool_call": false}));
        let profile = resolve(&p, &m);
        assert_eq!(profile.reasoning_replay, ReasoningReplay::Never);
        assert!(!profile.parallel_tool_calls);
        assert_eq!(
            profile.reasoning_carrier,
            ReasoningCarrier::ReasoningContent
        );

        let data = json!({ "openai": { "npm": "@ai-sdk/openai",
            "models": { "gpt-6-sol": { "tool_call": true, "reasoning": true } } } });
        let (p, mut m) = catalog_model(data, "openai", "gpt-6-sol");
        m.profile_catalog = from_chatgpt(&json!({
            "support_verbosity": true, "default_verbosity": "low",
            "supports_reasoning_summary_parameter": false,
            "apply_patch_tool_type": null,
            "truncation_policy": {"mode": "bytes", "limit": 10000},
        }));
        let profile = resolve(&p, &m);
        assert_eq!(profile.verbosity.default, Some(VerbosityLevel::Low));
        assert!(!profile.reasoning_summary);
        assert_eq!(profile.tool_output.tokens, Some(2_500));
        assert_eq!(
            profile.edit_tool,
            EditTool::FreeformPatch,
            "null keeps the rule"
        );
    }

    #[test]
    fn config_wins_over_catalog_metadata() {
        let (p, mut m) = compatible("corp", "kimi-k2");
        m.profile_catalog = from_models_dev(&json!({"tool_call": false}));
        m.profile_config = ProfilePatch::from_config(Some(&json!({
            "reasoning_replay": "never",
            "parallel_tool_calls": true,
            "verbosity": {"supported": true, "default": "high"},
            "tool_output": {"tokens": 0, "truncation": "tail"},
            "efforts": ["high", "low"],
            "edit_tool": "bogus",
        })));
        let profile = resolve(&p, &m);
        assert_eq!(profile.reasoning_replay, ReasoningReplay::Never);
        assert_eq!(
            profile.reasoning_carrier,
            ReasoningCarrier::ReasoningContent
        );
        assert!(profile.parallel_tool_calls);
        assert_eq!(
            profile.verbosity,
            Verbosity {
                supported: true,
                default: Some(VerbosityLevel::High)
            }
        );
        assert_eq!(profile.tool_output.truncation, Truncation::Tail);
        assert_eq!(profile.efforts, ["low", "high"]);
        assert_eq!(
            profile.edit_tool,
            EditTool::Edit,
            "an invalid value is skipped"
        );
    }

    #[test]
    fn efforts_default_to_the_variants_lowest_first() {
        let data = json!({ "corp": { "npm": "@ai-sdk/openai-compatible", "api": "https://x/v1",
            "models": { "m": { "reasoning": true,
                "reasoning_options": [{"type": "effort", "values": ["high", "none", "low"]}] } } } });
        let (p, m) = catalog_model(data, "corp", "m");
        assert_eq!(resolve(&p, &m).efforts, ["none", "low", "high"]);
    }

    /// The vocabulary configuration validation checks is the one the profile parses.
    #[test]
    fn config_vocabulary_matches_the_profile_types() {
        use cyber_core::config::model_profiles as vocabulary;
        fn all_parse<T: serde::de::DeserializeOwned>(values: &[&str]) {
            for value in values {
                assert!(parsed::<T>(Some(&json!(value))).is_some(), "{value}");
            }
        }
        all_parse::<ReasoningReplay>(vocabulary::REASONING_REPLAY);
        all_parse::<ReasoningCarrier>(vocabulary::REASONING_CARRIER);
        all_parse::<VerbosityLevel>(vocabulary::VERBOSITY);
        all_parse::<EditTool>(vocabulary::EDIT_TOOL);
        all_parse::<Truncation>(vocabulary::TRUNCATION);
        all_parse::<EffortClamp>(vocabulary::EFFORT_CLAMP);
        assert_eq!(vocabulary::REASONING_CARRIER.len(), 8);
    }
}
