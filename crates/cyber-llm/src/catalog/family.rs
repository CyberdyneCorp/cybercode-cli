//! Which base prompt a catalog model gets (`system-context` → Provider base prompts).

use cyber_core::prompts::ModelFamily;

use super::{Model, Provider};
use crate::adapters::ApiKind;

/// Name prefixes, tried in order. Open-weights names come first so that `gpt-oss` is not
/// taken for OpenAI and `gemma` is not taken for Gemini.
const NAME_PREFIXES: &[(&str, ModelFamily)] = &[
    ("gpt-oss", ModelFamily::Local),
    ("llama", ModelFamily::Local),
    ("qwen", ModelFamily::Local),
    ("mistral", ModelFamily::Local),
    ("mixtral", ModelFamily::Local),
    ("codestral", ModelFamily::Local),
    ("devstral", ModelFamily::Local),
    ("deepseek", ModelFamily::Local),
    ("gemma", ModelFamily::Local),
    ("phi", ModelFamily::Local),
    ("glm", ModelFamily::Local),
    ("kimi", ModelFamily::Local),
    ("minimax", ModelFamily::Local),
    ("claude", ModelFamily::Anthropic),
    ("gpt", ModelFamily::Openai),
    ("o1", ModelFamily::Openai),
    ("o3", ModelFamily::Openai),
    ("o4", ModelFamily::Openai),
    ("codex", ModelFamily::Openai),
    ("chatgpt", ModelFamily::Openai),
    ("gemini", ModelFamily::Gemini),
];

/// The model's family: by name (the catalog `family`, then the last segment of the API id),
/// then by protocol, then by a local endpoint, else [`ModelFamily::Default`].
pub fn model_family(provider: &Provider, model: &Model) -> ModelFamily {
    let api_name = model.api_id.rsplit('/').next().unwrap_or_default();
    let by_name = model
        .family
        .iter()
        .map(String::as_str)
        .chain([api_name])
        .find_map(|name| by_name(&name.to_ascii_lowercase()));
    by_name
        .or_else(|| by_protocol(model.api_kind.or(provider.kind)))
        .or_else(|| local_endpoint(provider, model).then_some(ModelFamily::Local))
        .unwrap_or_default()
}

fn by_name(name: &str) -> Option<ModelFamily> {
    NAME_PREFIXES
        .iter()
        .find(|(prefix, _)| name.starts_with(prefix))
        .map(|&(_, family)| family)
}

fn by_protocol(kind: Option<ApiKind>) -> Option<ModelFamily> {
    match kind? {
        ApiKind::Anthropic => Some(ModelFamily::Anthropic),
        ApiKind::OpenaiResponses | ApiKind::ChatgptCodex => Some(ModelFamily::Openai),
        ApiKind::OpenaiCompatible | ApiKind::CopilotChat => None,
    }
}

fn local_endpoint(provider: &Provider, model: &Model) -> bool {
    let url = model.api_url.as_deref().or(provider.url.as_deref());
    provider.auth_none || url.is_some_and(super::credentials::is_loopback)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    /// The family of `provider/model` in a models.dev-shaped document.
    fn family_of(data: serde_json::Value, provider: &str, model: &str) -> ModelFamily {
        let providers = super::super::models_dev::parse(&data);
        let p = &providers[provider];
        model_family(p, &p.models[model])
    }

    fn hosted(provider: &str, npm: &str, model: &str) -> ModelFamily {
        let data = json!({ provider: {
            "npm": npm, "api": format!("https://{provider}.example/v1"),
            "models": { model: {} },
        }});
        family_of(data, provider, model)
    }

    #[test]
    fn names_win_over_the_provider() {
        let compatible = "@ai-sdk/openai-compatible";
        let cases = [
            (
                "openrouter",
                "anthropic/claude-sonnet",
                ModelFamily::Anthropic,
            ),
            ("copilot", "gpt-5", ModelFamily::Openai),
            ("google", "gemini-2.5-pro", ModelFamily::Gemini),
            ("groq", "openai/gpt-oss-120b", ModelFamily::Local),
            ("google", "gemma-3-27b", ModelFamily::Local),
            ("ollama", "qwen3", ModelFamily::Local),
            ("deepinfra", "o3-mini", ModelFamily::Openai),
        ];
        for (provider, model, expected) in cases {
            assert_eq!(
                hosted(provider, compatible, model),
                expected,
                "{provider}/{model}"
            );
        }
    }

    #[test]
    fn the_catalog_family_is_tried_before_the_api_id() {
        let data = json!({ "router": {
            "npm": "@ai-sdk/openai-compatible", "api": "https://router.example/v1",
            "models": { "sonnet-latest": { "family": "claude-sonnet" } },
        }});
        assert_eq!(
            family_of(data, "router", "sonnet-latest"),
            ModelFamily::Anthropic
        );
    }

    #[test]
    fn protocol_then_local_endpoint_then_default() {
        assert_eq!(
            hosted("acme", "@ai-sdk/anthropic", "house-model"),
            ModelFamily::Anthropic
        );
        assert_eq!(
            hosted("acme", "@ai-sdk/openai", "house-model"),
            ModelFamily::Openai
        );
        let compatible = "@ai-sdk/openai-compatible";
        assert_eq!(
            hosted("acme", compatible, "house-model"),
            ModelFamily::Default
        );
        let data = json!({ "lab": {
            "npm": compatible, "api": "http://127.0.0.1:8080/v1", "models": { "house-model": {} },
        }});
        assert_eq!(family_of(data, "lab", "house-model"), ModelFamily::Local);
    }

    #[test]
    fn auth_none_is_a_local_endpoint() {
        let data = json!({ "lab": {
            "npm": "@ai-sdk/openai-compatible", "api": "https://gpu.lan/v1",
            "models": { "house-model": {} },
        }});
        let mut providers = super::super::models_dev::parse(&data);
        let provider = providers.get_mut("lab").unwrap();
        provider.auth_none = true;
        let model = &provider.models["house-model"];
        assert_eq!(model_family(provider, model), ModelFamily::Local);
    }

    #[test]
    fn chatgpt_codex_models_are_openai() {
        let data = json!({ "chatgpt": { "models": { "gpt-5-codex": {} } } });
        let mut providers = super::super::models_dev::parse(&data);
        let provider = providers.get_mut("chatgpt").unwrap();
        provider.kind = Some(ApiKind::ChatgptCodex);
        assert_eq!(
            model_family(provider, &provider.models["gpt-5-codex"]),
            ModelFamily::Openai
        );
        let mut model = provider.models["gpt-5-codex"].clone();
        model.api_id = "house-model".into();
        assert_eq!(model_family(provider, &model), ModelFamily::Openai);
    }
}
