//! Policy-checked HTTP clients for host-initiated requests (HTTP hooks, remote MCP).
//!
//! Every client routes through an owned loopback TCP proxy that enforces the effective
//! network mode, ignores ambient proxy settings, never follows redirects or retries, and
//! keeps no idle connections, so stopping the proxy settles every local socket.
use std::collections::BTreeMap;
use std::sync::Arc;

use cyber_sandbox::NetworkMode;
use cyber_sandbox::proxy::{Decide, Endpoint, Proxy};
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};

/// A client bound to its owned proxy. The caller must stop the proxy to settle sockets.
pub(crate) struct PolicyClient {
    pub client: reqwest::Client,
    pub proxy: Proxy,
}

/// Refuse a disabled network or an unlisted proxy host before any contact, then start
/// the owned proxy. `label` prefixes every diagnostic (e.g. "HTTP hook").
pub(crate) async fn policy_client(
    url: &reqwest::Url,
    network: NetworkMode,
    allowed_domains: Vec<String>,
    label: &str,
) -> Result<PolicyClient, String> {
    if network == NetworkMode::Off {
        return Err(format!("{label} network is disabled"));
    }
    let host = url
        .host_str()
        .ok_or_else(|| format!("{label} URL has no host"))?;
    let host = host
        .strip_prefix('[')
        .and_then(|host| host.strip_suffix(']'))
        .unwrap_or(host);
    if network == NetworkMode::Proxy && !cyber_sandbox::matches_domain(host, &allowed_domains) {
        return Err(format!(
            "{label} destination is outside the network allowlist"
        ));
    }
    let decide: Decide = Arc::new(move |host| {
        let allowed =
            network == NetworkMode::On || cyber_sandbox::matches_domain(&host, &allowed_domains);
        Box::pin(async move { allowed })
    });
    let proxy = Proxy::start(decide)
        .await
        .map_err(|_| format!("could not start {label} proxy"))?;
    let Endpoint::Tcp(port) = proxy.endpoint else {
        return Err(format!("{label} requires a TCP proxy"));
    };
    let client = reqwest::Client::builder()
        .no_proxy()
        .proxy(
            reqwest::Proxy::all(format!("http://127.0.0.1:{port}"))
                .map_err(|_| format!("invalid {label} proxy"))?,
        )
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .pool_max_idle_per_host(0)
        .build()
        .map_err(|_| format!("could not create {label} client"))?;
    Ok(PolicyClient { client, proxy })
}

/// Configured headers without values in diagnostics; `reserved` names are refused.
pub(crate) fn headers(
    configured: &BTreeMap<String, String>,
    label: &str,
    reserved: fn(&str) -> bool,
) -> Result<HeaderMap, String> {
    let mut headers = HeaderMap::new();
    for (name, value) in configured {
        let name = HeaderName::from_bytes(name.as_bytes())
            .map_err(|_| format!("invalid {label} header name"))?;
        if reserved(name.as_str()) {
            return Err(format!("{label} cannot override transport header {name}"));
        }
        headers.insert(
            name,
            HeaderValue::from_str(value).map_err(|_| format!("invalid {label} header value"))?,
        );
    }
    Ok(headers)
}
