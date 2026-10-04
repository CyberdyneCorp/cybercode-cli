//! `websearch` (`builtin-tools` → websearch tool) over third-party backends.

use std::collections::HashMap;
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use cyber_core::env::EnvSource;
use cyber_server::runtime::{RetrySafety, ToolDef};
use futures::future::BoxFuture;
use serde_json::{Value, json};

use super::web::read_limited;
use super::{Tool, ToolError, def, failed, number, text};
use crate::host::Ctx;
use crate::permissions::Request;

pub(crate) const BACKENDS: &[&str] = &[
    "exa",
    "brave",
    "searxng",
    "parallel",
    "firecrawl",
    "tavily",
    "tinyfish",
];
const TIMEOUT: Duration = Duration::from_secs(25);
const MAX_RESPONSE: usize = 256 * 1024;
const COOLDOWN: Duration = Duration::from_secs(600);

pub(crate) struct WebSearch;

/// A backend with its credentials.
#[derive(Debug, Clone)]
pub(crate) struct Backend {
    pub name: &'static str,
    key: Option<String>,
    url: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
struct Hit {
    title: String,
    url: String,
    snippet: String,
}

/// Rate-limited backends skipped by `random` until the instant passes.
#[derive(Default)]
pub(crate) struct Cooldowns(Mutex<HashMap<&'static str, Instant>>);

impl Cooldowns {
    fn cooling(&self, name: &str) -> bool {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(name)
            .is_some_and(|until| Instant::now() < *until)
    }

    fn start(&self, name: &'static str) {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(name, Instant::now() + COOLDOWN);
    }
}

/// Backends this config can use, in rotation order; empty hides the tool.
pub(crate) fn backends(config: &Value, env: &dyn EnvSource) -> Vec<Backend> {
    let settings = config
        .pointer("/tools/websearch")
        .cloned()
        .unwrap_or(Value::Null);
    if settings.get("enabled").and_then(Value::as_bool) == Some(false) {
        return Vec::new();
    }
    let available: Vec<Backend> = BACKENDS
        .iter()
        .filter_map(|name| credentials(name, &settings, env))
        .collect();
    match settings
        .get("backend")
        .and_then(Value::as_str)
        .unwrap_or("random")
    {
        "random" => available,
        chosen => available.into_iter().filter(|b| b.name == chosen).collect(),
    }
}

fn credentials(name: &'static str, settings: &Value, env: &dyn EnvSource) -> Option<Backend> {
    let upper = name.to_ascii_uppercase();
    let cfg = |key: &str| {
        settings
            .pointer(&format!("/{name}/{key}"))
            .and_then(Value::as_str)
            .map(str::to_string)
    };
    let injected = env
        .get("CYBER_AUTH_CONTENT")
        .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
        .and_then(|v| {
            v.pointer(&format!("/{name}/key"))
                .and_then(Value::as_str)
                .map(str::to_string)
        });
    let key = cfg("api_key")
        .or(injected)
        .or_else(|| env.get(&format!("{upper}_API_KEY")));
    let url = cfg("url").or_else(|| env.get(&format!("{upper}_URL")));
    let usable = if name == "searxng" {
        url.is_some()
    } else {
        key.is_some()
    };
    usable.then_some(Backend { name, key, url })
}

impl Tool for WebSearch {
    fn def(&self) -> ToolDef {
        def(
            "websearch",
            "Search the web. Returns title, url and snippet per result. max_results defaults to 8 (max 20).",
            json!({"type": "object", "required": ["query"], "properties": {
                "query": {"type": "string"}, "max_results": {"type": "integer"},
                "allowed_domains": {"type": "array", "items": {"type": "string"}},
                "blocked_domains": {"type": "array", "items": {"type": "string"}}}}),
            RetrySafety::ReadOnly,
            true,
        )
    }

    fn run<'a>(&'a self, ctx: &'a Ctx<'a>) -> BoxFuture<'a, Result<String, ToolError>> {
        Box::pin(async move {
            let input = &ctx.inv.input;
            let query = text(input, "query").trim();
            if query.is_empty() {
                return Err(failed("query is empty"));
            }
            let req = Request {
                action: "websearch".into(),
                resources: vec![query.into()],
                read_only: true,
                ..Request::default()
            };
            ctx.authorize(req, vec!["*".into()], Value::Null).await?;
            let search = Search {
                query,
                max: number(input, "max_results").unwrap_or(8).clamp(1, 20) as usize,
                allowed: domains(input, "allowed_domains"),
                blocked: domains(input, "blocked_domains"),
            };
            let candidates = backends(&ctx.host.config_for(&ctx.location), &*ctx.host.opts.env);
            let hits = tokio::select! {
                _ = ctx.cancel.cancelled() => return Err(ToolError::Aborted),
                r = rotate(&ctx.host.search_cooldowns, candidates, &search) => r.map_err(failed)?,
            };
            Ok(render(&hits))
        })
    }
}

struct Search<'a> {
    query: &'a str,
    max: usize,
    allowed: Vec<String>,
    blocked: Vec<String>,
}

fn domains(input: &Value, key: &str) -> Vec<String> {
    input
        .get(key)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_ascii_lowercase)
        .collect()
}

/// Try backends starting at a rotating offset; a 429 cools a backend down and moves on.
async fn rotate(
    cooldowns: &Cooldowns,
    candidates: Vec<Backend>,
    search: &Search<'_>,
) -> Result<Vec<Hit>, String> {
    if candidates.is_empty() {
        return Err("No web search backend is configured".into());
    }
    let start = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos() as usize);
    let n = candidates.len();
    let mut last = String::from("every search backend is rate limited; try again later");
    for i in 0..n {
        let backend = &candidates[(start + i) % n];
        if n > 1 && cooldowns.cooling(backend.name) {
            continue;
        }
        match query(backend, search).await {
            Ok(hits) => return Ok(filter(hits, search)),
            Err(Failure::RateLimited) => {
                cooldowns.start(backend.name);
                last = format!("{} is rate limited", backend.name);
            }
            Err(Failure::Other(message)) => return Err(format!("{}: {message}", backend.name)),
        }
    }
    Err(last)
}

enum Failure {
    RateLimited,
    Other(String),
}

async fn query(backend: &Backend, search: &Search<'_>) -> Result<Vec<Hit>, Failure> {
    let client = reqwest::Client::builder()
        .timeout(TIMEOUT)
        .build()
        .map_err(|e| Failure::Other(e.to_string()))?;
    let response = request(&client, backend, search)
        .send()
        .await
        .map_err(|e| Failure::Other(e.to_string()))?;
    if response.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
        return Err(Failure::RateLimited);
    }
    if !response.status().is_success() {
        return Err(Failure::Other(format!("status {}", response.status())));
    }
    let body = read_limited(response, MAX_RESPONSE)
        .await
        .map_err(|_| Failure::Other("response exceeded 256 KiB".into()))?;
    let value: Value = serde_json::from_slice(&body)
        .map_err(|e| Failure::Other(format!("invalid response: {e}")))?;
    Ok(parse(backend.name, &value))
}

fn request(client: &reqwest::Client, b: &Backend, s: &Search<'_>) -> reqwest::RequestBuilder {
    let key = b.key.clone().unwrap_or_default();
    let n = s.max;
    match b.name {
        "exa" => {
            let mut body = json!({"query": s.query, "numResults": n, "contents": {"text": {"maxCharacters": 400}}});
            if !s.allowed.is_empty() {
                body["includeDomains"] = json!(s.allowed);
            }
            if !s.blocked.is_empty() {
                body["excludeDomains"] = json!(s.blocked);
            }
            client.post("https://api.exa.ai/search").header("x-api-key", key).json(&body)
        }
        "brave" => client
            .get("https://api.search.brave.com/res/v1/web/search")
            .query(&[("q", s.query), ("count", &n.to_string())])
            .header("X-Subscription-Token", key)
            .header("Accept", "application/json"),
        "searxng" => {
            let base = b.url.clone().unwrap_or_default();
            let req = client.get(format!("{}/search", base.trim_end_matches('/'))).query(&[("q", s.query), ("format", "json")]);
            if key.is_empty() { req } else { req.bearer_auth(key) }
        }
        "parallel" => client.post("https://api.parallel.ai/v1/search").header("x-api-key", key).json(&json!({
            "objective": s.query, "search_queries": [s.query], "advanced_settings": {"max_results": n}})),
        "firecrawl" => client.post("https://api.firecrawl.dev/v2/search").bearer_auth(key).json(&json!({"query": s.query, "limit": n})),
        "tavily" => {
            let mut body = json!({"query": s.query, "max_results": n});
            if !s.allowed.is_empty() {
                body["include_domains"] = json!(s.allowed);
            }
            if !s.blocked.is_empty() {
                body["exclude_domains"] = json!(s.blocked);
            }
            client.post("https://api.tavily.com/search").bearer_auth(key).json(&body)
        }
        _ => client.get("https://api.search.tinyfish.ai").query(&[("query", s.query)]).header("X-API-Key", key),
    }
}

/// Normalize a backend response into hits.
fn parse(backend: &str, v: &Value) -> Vec<Hit> {
    let (list, snippet_key) = match backend {
        "exa" => (v.pointer("/results"), "text"),
        "brave" => (v.pointer("/web/results"), "description"),
        "searxng" | "tavily" => (v.pointer("/results"), "content"),
        "parallel" => (v.pointer("/results"), "excerpts"),
        "firecrawl" => (
            v.pointer("/data/web").or_else(|| v.pointer("/data")),
            "description",
        ),
        _ => (v.pointer("/results"), "snippet"),
    };
    list.and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|r| {
            let snippet = match &r[snippet_key] {
                Value::Array(parts) => parts
                    .iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join(" "),
                other => other.as_str().unwrap_or_default().to_string(),
            };
            Some(Hit {
                title: r["title"].as_str().unwrap_or_default().into(),
                url: r["url"].as_str()?.into(),
                snippet,
            })
        })
        .collect()
}

fn filter(hits: Vec<Hit>, s: &Search<'_>) -> Vec<Hit> {
    let host_of = |url: &str| {
        reqwest::Url::parse(url)
            .ok()
            .and_then(|u| u.host_str().map(str::to_ascii_lowercase))
            .unwrap_or_default()
    };
    let under = |host: &str, domain: &str| host == domain || host.ends_with(&format!(".{domain}"));
    hits.into_iter()
        .filter(|h| {
            let host = host_of(&h.url);
            (s.allowed.is_empty() || s.allowed.iter().any(|d| under(&host, d)))
                && !s.blocked.iter().any(|d| under(&host, d))
        })
        .take(s.max)
        .collect()
}

fn render(hits: &[Hit]) -> String {
    if hits.is_empty() {
        return "No results found".into();
    }
    hits.iter()
        .enumerate()
        .map(|(i, h)| {
            let snippet: String = h
                .snippet
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .chars()
                .take(400)
                .collect();
            format!("{}. {}\n   {}\n   {}", i + 1, h.title, h.url, snippet)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Env(Vec<(&'static str, &'static str)>);
    impl EnvSource for Env {
        fn get(&self, key: &str) -> Option<String> {
            self.0
                .iter()
                .find(|(k, _)| *k == key)
                .map(|(_, v)| v.to_string())
        }
    }

    #[test]
    fn backends_follow_credentials_and_config() {
        let env = Env(vec![("EXA_API_KEY", "k"), ("BRAVE_API_KEY", "b")]);
        let names = |c: Value| {
            backends(&c, &env)
                .into_iter()
                .map(|b| b.name)
                .collect::<Vec<_>>()
        };
        assert_eq!(names(json!({})), vec!["exa", "brave"]);
        assert_eq!(
            names(json!({"tools": {"websearch": {"backend": "brave"}}})),
            vec!["brave"]
        );
        assert!(names(json!({"tools": {"websearch": {"backend": "tavily"}}})).is_empty());
        assert!(names(json!({"tools": {"websearch": {"enabled": false}}})).is_empty());
        assert_eq!(
            names(
                json!({"tools": {"websearch": {"searxng": {"url": "http://x"}, "backend": "searxng"}}})
            ),
            vec!["searxng"]
        );
    }

    #[test]
    fn responses_normalize_and_filter_by_domain() {
        let brave = json!({"web": {"results": [
            {"title": "A", "url": "https://docs.rs/a", "description": "first"},
            {"title": "B", "url": "https://evil.example/b", "description": "second"}]}});
        let hits = parse("brave", &brave);
        assert_eq!(hits.len(), 2);
        let s = Search {
            query: "q",
            max: 8,
            allowed: vec![],
            blocked: vec!["ample".into()],
        };
        assert_eq!(
            filter(hits.clone(), &s).len(),
            2,
            "block matches whole labels only"
        );
        let s = Search {
            query: "q",
            max: 8,
            allowed: vec!["docs.rs".into()],
            blocked: vec![],
        };
        assert_eq!(filter(hits, &s)[0].title, "A");
        let parallel =
            json!({"results": [{"title": "P", "url": "https://p.ai", "excerpts": ["one", "two"]}]});
        assert_eq!(parse("parallel", &parallel)[0].snippet, "one two");
    }
}
