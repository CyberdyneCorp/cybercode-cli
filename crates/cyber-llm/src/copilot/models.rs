//! The models the Copilot plan serves, as `GET <api>/models` lists them for the signed-in
//! account. The list is cached in `<data>/copilot-models.json` (written at login and
//! refreshed at most once a day) by the shared [`crate::subscription::ModelCache`], so the
//! synchronous catalog build never waits on the network. Only picker-enabled models not
//! disabled by an organization policy, with simple slugs, are kept.
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::http::{self, editor};
use super::{CopilotError, CopilotToken, join};
use crate::subscription::ModelCache;

const RESPONSE_LIMIT: usize = 1024 * 1024;

/// Which API a model is served through.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Protocol {
    /// `<api>/chat/completions`.
    Chat,
    /// `<api>/responses`.
    Responses,
}

/// One model of the plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CopilotModel {
    pub slug: String,
    pub name: String,
    pub context_window: u64,
    pub max_output: u64,
    pub vision: bool,
    pub tools: bool,
    pub protocol: Protocol,
    /// The API endpoint the Copilot token that listed this model named.
    #[serde(default)]
    pub api: String,
}

/// `<data>/copilot-models.json`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelsFile {
    cache: ModelCache<CopilotModel>,
}

impl ModelsFile {
    /// The list kept beside the login file `auth`.
    pub fn beside(auth: &Path) -> Self {
        Self {
            cache: ModelCache::new(auth.with_file_name("copilot-models.json")),
        }
    }

    pub fn path(&self) -> &Path {
        self.cache.path()
    }

    /// The cached models; empty when nothing usable is cached.
    pub fn load(&self) -> Vec<CopilotModel> {
        self.cache.load()
    }

    /// Whether the cache is missing or older than [`crate::subscription::MODEL_MAX_AGE`].
    pub fn stale(&self) -> bool {
        self.cache.stale()
    }

    pub fn save(&self, models: Vec<CopilotModel>) -> Result<(), CopilotError> {
        Ok(self.cache.save(models)?)
    }

    /// Cache a refresh's result; a failure keeps the old list for another day.
    pub fn record<E>(&self, fetched: Result<Vec<CopilotModel>, E>) {
        self.cache.record(fetched);
    }

    pub fn remove(&self) {
        self.cache.remove();
    }
}

/// The plan's listed models, from the API `token` names.
pub async fn fetch(
    client: &reqwest::Client,
    token: &CopilotToken,
) -> Result<Vec<CopilotModel>, CopilotError> {
    let request = editor(client.get(join(&token.api, "models"))).bearer_auth(&token.token);
    let (status, document) = http::send(request, "model list", RESPONSE_LIMIT).await?;
    match status {
        200 if document["data"].is_array() => Ok(parse(&document, token.api.as_str())),
        200 => Err(CopilotError::Body("model list is not a list")),
        401 => Err(CopilotError::Revoked),
        status => Err(CopilotError::Status {
            stage: "model list",
            status,
        }),
    }
}

fn parse(document: &Value, api: &str) -> Vec<CopilotModel> {
    document["data"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|m| m["model_picker_enabled"] == true && m["policy"]["state"] != "disabled")
        .filter_map(|entry| model(entry, api))
        .collect()
}

fn model(entry: &Value, api: &str) -> Option<CopilotModel> {
    let slug = entry["id"].as_str().filter(|s| valid_slug(s))?;
    let capabilities = &entry["capabilities"];
    let (limits, supports) = (&capabilities["limits"], &capabilities["supports"]);
    let responses = entry["supported_endpoints"]
        .as_array()
        .is_some_and(|endpoints| endpoints.iter().any(|e| e == "/responses"));
    Some(CopilotModel {
        slug: slug.into(),
        name: entry["name"].as_str().unwrap_or(slug).into(),
        context_window: limits["max_context_window_tokens"].as_u64().unwrap_or(0),
        max_output: limits["max_output_tokens"].as_u64().unwrap_or(0),
        vision: supports["vision"] == true,
        tools: supports["tool_calls"] == true,
        protocol: if responses {
            Protocol::Responses
        } else {
            Protocol::Chat
        },
        api: api.into(),
    })
}

/// Model IDs become references (`copilot/<slug>`), so only simple names are accepted.
fn valid_slug(slug: &str) -> bool {
    !slug.is_empty()
        && slug.len() <= 128
        && slug
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-._".contains(&b))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn only_enabled_allowed_simple_models_are_kept_with_their_protocol() {
        let models = parse(
            &json!({"data": [
                {"id": "claude-sonnet-4.5", "name": "Claude Sonnet 4.5", "model_picker_enabled": true,
                 "policy": {"state": "enabled"},
                 "capabilities": {"limits": {"max_context_window_tokens": 200000,
                                             "max_output_tokens": 16000},
                                  "supports": {"vision": true, "tool_calls": true}},
                 "supported_endpoints": ["/chat/completions"]},
                {"id": "gpt-5.6-sol", "model_picker_enabled": true,
                 "supported_endpoints": ["/responses"]},
                {"id": "org-blocked", "model_picker_enabled": true, "policy": {"state": "disabled"}},
                {"id": "text-embedding-3", "model_picker_enabled": false},
                {"id": "bad/slug", "model_picker_enabled": true}
            ]}),
            "https://api.individual.githubcopilot.com/",
        );
        assert_eq!(
            models,
            [
                CopilotModel {
                    slug: "claude-sonnet-4.5".into(),
                    name: "Claude Sonnet 4.5".into(),
                    context_window: 200_000,
                    max_output: 16_000,
                    vision: true,
                    tools: true,
                    protocol: Protocol::Chat,
                    api: "https://api.individual.githubcopilot.com/".into(),
                },
                CopilotModel {
                    slug: "gpt-5.6-sol".into(),
                    name: "gpt-5.6-sol".into(),
                    context_window: 0,
                    max_output: 0,
                    vision: false,
                    tools: false,
                    protocol: Protocol::Responses,
                    api: "https://api.individual.githubcopilot.com/".into(),
                },
            ]
        );
    }

    #[test]
    fn the_cache_lives_beside_the_login_and_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let file = ModelsFile::beside(&dir.path().join("copilot-auth.json"));
        assert_eq!(file.path(), dir.path().join("copilot-models.json"));
        assert!(file.stale());
        let model = CopilotModel {
            slug: "m".into(),
            name: "M".into(),
            context_window: 1,
            max_output: 1,
            vision: false,
            tools: true,
            protocol: Protocol::Responses,
            api: "https://api.githubcopilot.com/".into(),
        };
        file.save(vec![model.clone()]).unwrap();
        assert!(!file.stale());
        assert_eq!(file.load(), [model]);
        file.remove();
        assert!(file.load().is_empty());
    }
}
