//! The models the ChatGPT plan serves, as the Codex backend lists them for the signed-in
//! account (`GET <backend>/models`). The list is cached in `<data>/chatgpt-models.json`
//! (written at login and refreshed at most once a day), so the synchronous catalog build
//! never waits on the network. Only models the backend marks `visibility: "list"` are kept.
use std::path::{Path, PathBuf};
use std::time::Duration;

use futures::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{ChatGptError, Login, ORIGINATOR, now};

/// The backend filters its list by the Codex client version it is told; Cyber Code asks
/// for every model current clients may use.
pub const CLIENT_VERSION: &str = "1.0.0";
/// A cached list older than this is refreshed before the next request.
pub const MAX_AGE: u64 = 24 * 60 * 60;
const RESPONSE_LIMIT: usize = 1024 * 1024;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

/// One model of the plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanModel {
    pub slug: String,
    pub name: String,
    pub context_window: u64,
    pub vision: bool,
    /// Reasoning efforts the model accepts, in the backend's order.
    pub efforts: Vec<String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Document {
    fetched_at: u64,
    models: Vec<PlanModel>,
}

/// `<data>/chatgpt-models.json`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelsFile {
    path: PathBuf,
}

impl ModelsFile {
    /// The list kept beside the login file `auth`.
    pub fn beside(auth: &Path) -> Self {
        Self {
            path: auth.with_file_name("chatgpt-models.json"),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The cached models; empty when nothing usable is cached.
    pub fn load(&self) -> Vec<PlanModel> {
        self.document().map(|d| d.models).unwrap_or_default()
    }

    /// Whether the cache is missing or older than [`MAX_AGE`].
    pub fn stale(&self) -> bool {
        self.document()
            .is_none_or(|d| d.fetched_at.saturating_add(MAX_AGE) <= now())
    }

    pub fn save(&self, models: Vec<PlanModel>) -> Result<(), ChatGptError> {
        let document = Document {
            fetched_at: now(),
            models,
        };
        let bytes = serde_json::to_vec_pretty(&document)
            .map_err(|_| ChatGptError::Storage("model list encoding failed"))?;
        cyber_core::trust::write_private_atomic(&self.path, &bytes)
            .map_err(|_| ChatGptError::Storage("model list write failed"))
    }

    pub fn remove(&self) {
        let _ = std::fs::remove_file(&self.path);
    }

    fn document(&self) -> Option<Document> {
        let bytes = std::fs::read(&self.path).ok()?;
        serde_json::from_slice(&bytes).ok()
    }
}

/// The plan's listed models for `login`, from the backend at `base_url`.
pub async fn fetch(
    http: &reqwest::Client,
    base_url: &str,
    login: &Login,
) -> Result<Vec<PlanModel>, ChatGptError> {
    let url = format!(
        "{}/models?client_version={CLIENT_VERSION}",
        base_url.trim_end_matches('/')
    );
    let response = http
        .get(url)
        .bearer_auth(&login.access_token)
        .header("chatgpt-account-id", &login.account_id)
        .header("originator", ORIGINATOR)
        .header("accept", "application/json")
        .timeout(REQUEST_TIMEOUT)
        .send()
        .await
        .map_err(|_| ChatGptError::Transport("model list"))?;
    let status = response.status().as_u16();
    if status != 200 {
        return Err(ChatGptError::Status {
            stage: "model list",
            status,
        });
    }
    let mut bytes = Vec::new();
    let mut chunks = response.bytes_stream();
    while let Some(chunk) = chunks.next().await {
        let chunk = chunk.map_err(|_| ChatGptError::Transport("model list"))?;
        if bytes.len() + chunk.len() > RESPONSE_LIMIT {
            return Err(ChatGptError::Body("model list too large"));
        }
        bytes.extend_from_slice(&chunk);
    }
    let document: Value =
        serde_json::from_slice(&bytes).map_err(|_| ChatGptError::Body("model list is not JSON"))?;
    Ok(parse(&document))
}

fn parse(document: &Value) -> Vec<PlanModel> {
    document["models"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|m| m["visibility"] == "list")
        .filter_map(|m| {
            let slug = m["slug"].as_str().filter(|s| valid_slug(s))?;
            Some(PlanModel {
                slug: slug.into(),
                name: m["display_name"].as_str().unwrap_or(slug).into(),
                context_window: m["context_window"].as_u64().unwrap_or(0),
                vision: m["input_modalities"]
                    .as_array()
                    .is_some_and(|a| a.iter().any(|v| v == "image")),
                efforts: m["supported_reasoning_levels"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|level| level["effort"].as_str())
                    .filter(|effort| valid_slug(effort))
                    .map(str::to_owned)
                    .collect(),
            })
        })
        .collect()
}

/// Model IDs become references (`chatgpt/<slug>`), so only simple names are accepted.
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
    fn only_listed_models_with_simple_slugs_are_kept() {
        let models = parse(&json!({"models": [
            {"slug": "gpt-6-sol", "display_name": "GPT-6-Sol", "visibility": "list",
             "context_window": 272000, "input_modalities": ["text", "image"],
             "supported_reasoning_levels": [{"effort": "low"}, {"effort": "max"}]},
            {"slug": "gpt-reserve", "visibility": "hide"},
            {"slug": "bad/slug", "visibility": "list"},
            {"slug": "gpt-plain", "visibility": "list"}
        ]}));
        assert_eq!(
            models,
            [
                PlanModel {
                    slug: "gpt-6-sol".into(),
                    name: "GPT-6-Sol".into(),
                    context_window: 272_000,
                    vision: true,
                    efforts: vec!["low".into(), "max".into()],
                },
                PlanModel {
                    slug: "gpt-plain".into(),
                    name: "gpt-plain".into(),
                    context_window: 0,
                    vision: false,
                    efforts: Vec::new(),
                },
            ]
        );
    }

    #[test]
    fn the_cache_round_trips_and_ages() {
        let dir = tempfile::tempdir().unwrap();
        let file = ModelsFile::beside(&dir.path().join("chatgpt-auth.json"));
        assert!(file.stale());
        assert!(file.load().is_empty());
        let model = PlanModel {
            slug: "gpt-6-sol".into(),
            name: "GPT-6-Sol".into(),
            context_window: 1,
            vision: false,
            efforts: Vec::new(),
        };
        file.save(vec![model.clone()]).unwrap();
        assert!(!file.stale());
        assert_eq!(file.load(), [model]);
    }
}
