//! `cyber doctor --network` (`cli-commands` → Doctor network check): whether this machine
//! reaches each usable provider and the update channel, through the HTTP client settings
//! model requests use. Probes send no credentials and follow no redirects.

use std::time::{Duration, Instant};

use cyber_core::env::EnvSource;
use cyber_core::version::{Channel, ReleaseSource, build_info};
use cyber_llm::catalog::{Catalog, CredentialSource};
use serde_json::Value;

use super::{Check, Status, check};

const TIMEOUT: Duration = Duration::from_secs(10);
const PROXY_VARS: [&str; 4] = ["HTTPS_PROXY", "HTTP_PROXY", "ALL_PROXY", "NO_PROXY"];

/// One endpoint to reach, and the status when it cannot be reached.
pub(super) struct Target {
    pub name: String,
    pub url: String,
    pub unreachable: Status,
}

/// The proxy variables in effect, with credentials in proxy URLs hidden.
pub(super) fn proxy(env: &dyn EnvSource) -> Check {
    let set: Vec<String> = PROXY_VARS
        .iter()
        .filter_map(|var| {
            let value = env
                .get(var)
                .or_else(|| env.get(&var.to_ascii_lowercase()))?;
            Some(format!("{var}={}", redact_userinfo(&value)))
        })
        .collect();
    let detail = if set.is_empty() {
        "none set".to_owned()
    } else {
        set.join(", ")
    };
    check("proxy", Status::Pass, detail)
}

fn redact_userinfo(value: &str) -> String {
    let Some((scheme, rest)) = value.split_once("://") else {
        return value.to_owned();
    };
    let authority = &rest[..rest.find('/').unwrap_or(rest.len())];
    match authority.rfind('@') {
        Some(at) => format!("{scheme}://***@{}", &rest[at + 1..]),
        None => value.to_owned(),
    }
}

/// Every enabled provider with a key or a subscription login, and every endpoint that needs
/// no credential that `providers` configures, with its URL; then the update channel. The
/// catalog lists local servers (LM Studio and others) whether or not they are used, so an
/// unconfigured one is left out. A local endpoint and the update channel only warn when
/// unreachable.
pub(super) fn targets(catalog: &Catalog, config: &Value, env: &dyn EnvSource) -> Vec<Target> {
    let mut out: Vec<Target> = catalog
        .providers
        .values()
        .filter(|provider| !provider.disabled)
        .filter_map(|provider| {
            let source = &catalog.credential(&provider.id)?.source;
            let local = *source == CredentialSource::NotRequired;
            if local && config["providers"].get(&provider.id).is_none() {
                return None;
            }
            let unreachable = if local { Status::Warn } else { Status::Fail };
            Some(Target {
                name: format!("network {}", provider.id),
                url: provider.url.clone()?,
                unreachable,
            })
        })
        .collect();
    if let Ok(source) = ReleaseSource::resolve(env) {
        let channel = match build_info().channel {
            Channel::Dev => Channel::Stable,
            channel => channel,
        };
        out.push(Target {
            name: "network updates".into(),
            url: source.manifest_url(channel),
            unreachable: Status::Warn,
        });
    }
    out
}

/// Probe every target at once.
pub(super) async fn probe_all(targets: Vec<Target>) -> Vec<Check> {
    let client = match cyber_llm::Endpoint::client_builder()
        .timeout(TIMEOUT)
        .build()
    {
        Ok(client) => client,
        Err(error) => return vec![check("network", Status::Fail, error.to_string())],
    };
    futures::future::join_all(targets.into_iter().map(|target| probe(&client, target))).await
}

async fn probe(client: &reqwest::Client, target: Target) -> Check {
    let host = host(&target.url);
    let started = Instant::now();
    match client.get(&target.url).send().await {
        Ok(response) => check(
            target.name,
            Status::Pass,
            format!(
                "{host} reachable: HTTP {} in {} ms",
                response.status().as_u16(),
                started.elapsed().as_millis()
            ),
        ),
        Err(error) => check(
            target.name,
            target.unreachable,
            format!("{host} unreachable: {}", cause(&error)),
        ),
    }
}

/// The URL's host and port, never its path or query.
fn host(url: &str) -> String {
    reqwest::Url::parse(url)
        .ok()
        .and_then(|url| {
            let host = url.host_str()?.to_owned();
            Some(match url.port() {
                Some(port) => format!("{host}:{port}"),
                None => host,
            })
        })
        .unwrap_or_else(|| "invalid URL".into())
}

/// Why a request failed: a timeout, or the innermost cause (DNS, connection refused,
/// TLS) without the URL.
fn cause(error: &reqwest::Error) -> String {
    if error.is_timeout() {
        return format!("timed out after {} s", TIMEOUT.as_secs());
    }
    let mut innermost: &dyn std::error::Error = error;
    while let Some(source) = innermost.source() {
        innermost = source;
    }
    let kind = if error.is_connect() {
        "cannot connect"
    } else {
        "request failed"
    };
    format!("{kind} ({innermost})")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proxy_credentials_are_hidden() {
        assert_eq!(
            redact_userinfo("http://user:secret@proxy.corp:8080"),
            "http://***@proxy.corp:8080"
        );
        assert_eq!(
            redact_userinfo("http://proxy.corp:8080/"),
            "http://proxy.corp:8080/"
        );
        assert_eq!(redact_userinfo("localhost,.corp"), "localhost,.corp");
    }

    #[test]
    fn hosts_never_show_paths_or_queries() {
        assert_eq!(host("https://api.openai.com/v1?key=sk-1"), "api.openai.com");
        assert_eq!(host("http://127.0.0.1:11434/v1"), "127.0.0.1:11434");
        assert_eq!(host("not a url"), "invalid URL");
    }
}
