//! Credential resolution (`provider-credentials` → Credential precedence).
//!
//! Order: explicit config key, `CYBER_AUTH_CONTENT`, the provider's environment variables,
//! then "none required" for loopback endpoints or `auth: "none"`. Stored keyring
//! connections join this order with `cyber providers login` (milestone M0.5).

use std::collections::BTreeMap;

use cyber_core::env::EnvSource;
use serde::Serialize;
use serde_json::Value;

use super::Provider;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", content = "name", rename_all = "snake_case")]
pub enum CredentialSource {
    Config,
    AuthContent,
    Env(String),
    NotRequired,
}

#[derive(Clone, PartialEq, Eq)]
pub struct Credential {
    pub api_key: Option<String>,
    pub source: CredentialSource,
}

impl std::fmt::Debug for Credential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Credential")
            .field("api_key", &self.api_key.as_ref().map(|_| "***"))
            .field("source", &self.source)
            .finish()
    }
}

pub fn resolve_all(
    providers: &BTreeMap<String, Provider>,
    env: &dyn EnvSource,
) -> BTreeMap<String, Credential> {
    let auth_content = auth_content(env);
    providers
        .values()
        .filter_map(|p| Some((p.id.clone(), resolve(p, env, &auth_content)?)))
        .collect()
}

fn resolve(p: &Provider, env: &dyn EnvSource, auth_content: &Value) -> Option<Credential> {
    let with = |key: String, source| {
        Some(Credential {
            api_key: Some(key),
            source,
        })
    };
    if let Some(key) = &p.api_key {
        return with(key.clone(), CredentialSource::Config);
    }
    let injected = auth_content
        .get(&p.id)
        .and_then(|c| c.get("key"))
        .and_then(Value::as_str);
    if let Some(key) = injected {
        return with(key.into(), CredentialSource::AuthContent);
    }
    if let Some((name, key)) = p
        .env
        .iter()
        .find_map(|name| Some((name.clone(), env.get(name)?)))
    {
        return with(key, CredentialSource::Env(name));
    }
    let local = p.url.as_deref().is_some_and(is_loopback);
    (p.auth_none || local).then_some(Credential {
        api_key: None,
        source: CredentialSource::NotRequired,
    })
}

/// `CYBER_AUTH_CONTENT`: `{ "<provider>": { "kind": "api_key", "key": "..." } }`.
/// Invalid JSON is ignored with a warning.
fn auth_content(env: &dyn EnvSource) -> Value {
    let Some(raw) = env.get("CYBER_AUTH_CONTENT") else {
        return Value::Null;
    };
    serde_json::from_str(&raw).unwrap_or_else(|_| {
        eprintln!("warning: CYBER_AUTH_CONTENT is not valid JSON and was ignored");
        Value::Null
    })
}

fn is_loopback(url: &str) -> bool {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    let authority = rest.split('/').next().unwrap_or_default();
    let host = match authority.strip_prefix('[') {
        Some(v6) => v6.split(']').next().unwrap_or_default(),
        None => authority.split(':').next().unwrap_or_default(),
    };
    matches!(host, "localhost" | "127.0.0.1" | "::1" | "0.0.0.0")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loopback_hosts() {
        assert!(is_loopback("http://127.0.0.1:11434/v1"));
        assert!(is_loopback("http://localhost:1234/v1"));
        assert!(is_loopback("http://[::1]:8080"));
        assert!(!is_loopback("https://api.openai.com/v1"));
    }
}
