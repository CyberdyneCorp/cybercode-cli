//! The network a plugin process may use (`plugins-marketplace` → Capability declaration
//! and enforcement), shared by plugin hosts and plugin MCP servers.
//!
//! The declared `network` capability is an upper bound, never a grant over the
//! resolved sandbox: `sandbox.network: "off"` leaves a plugin no network whatever it
//! declares, and `sandbox.network.denied_domains` is refused even for `network: true`.
//! Only a resolved `on` network without a denylist lets `network: true` connect
//! directly; everything else goes through the allowlist proxy.

use cyber_core::plugins::NetworkCapability;
use cyber_sandbox::{NetworkMode, SandboxConfig, matches_domain};

/// Which hosts a plugin process may reach through the proxy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PluginReach {
    /// `None`: every host the denylist does not name.
    allowed: Option<Vec<String>>,
    denied: Vec<String>,
}

impl PluginReach {
    pub(crate) fn allows(&self, host: &str) -> bool {
        !matches_domain(host, &self.denied)
            && self
                .allowed
                .as_ref()
                .is_none_or(|allowed| matches_domain(host, allowed))
    }
}

/// Narrow `config` to the plugin's `capability`. `None` means the process may connect
/// directly (`config.network` stays `on`); otherwise `config.network` is the proxy, its
/// `allowed_domains` the declared hosts, and the returned reach decides each host.
pub(crate) fn narrow(
    config: &mut SandboxConfig,
    capability: &NetworkCapability,
) -> Option<PluginReach> {
    let allowed = match capability {
        _ if config.network == NetworkMode::Off => Some(Vec::new()),
        NetworkCapability::None => Some(Vec::new()),
        NetworkCapability::Hosts(hosts) => Some(hosts.clone()),
        NetworkCapability::Any
            if config.network == NetworkMode::On && config.denied_domains.is_empty() =>
        {
            return None;
        }
        NetworkCapability::Any => None,
    };
    config.network = NetworkMode::Proxy;
    config.allowed_domains = allowed.clone().unwrap_or_default();
    Some(PluginReach {
        allowed,
        denied: config.denied_domains.clone(),
    })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::path::Path;

    use serde_json::{Value, json};

    use super::*;

    fn resolved(sandbox: Value) -> SandboxConfig {
        SandboxConfig::resolve(
            &json!({ "sandbox": sandbox }),
            &BTreeMap::new(),
            None,
            Path::new("/home/u"),
        )
    }

    fn hosts(list: &[&str]) -> NetworkCapability {
        NetworkCapability::Hosts(list.iter().map(|h| (*h).to_owned()).collect())
    }

    /// Regression: `network: true` set the network `on` and `network: { hosts }`
    /// replaced the allowlist after the sandbox was resolved, so a plugin escaped
    /// `sandbox.network: "off"` and the denylist.
    #[test]
    fn the_capability_never_widens_the_resolved_network() {
        for capability in [NetworkCapability::Any, hosts(&["api.example.com"])] {
            let mut config = resolved(json!({"network": "off"}));
            let reach = narrow(&mut config, &capability).unwrap();
            assert_eq!(config.network, NetworkMode::Proxy);
            assert!(config.allowed_domains.is_empty());
            assert!(!reach.allows("api.example.com") && !reach.allows("crates.io"));
        }
        let denied = json!({"network": {"mode": "on", "denied_domains": ["evil.test"]}});
        let mut config = resolved(denied.clone());
        let reach = narrow(&mut config, &NetworkCapability::Any).unwrap();
        assert_eq!(config.network, NetworkMode::Proxy);
        assert!(reach.allows("api.example.com") && !reach.allows("evil.test"));
        let mut config = resolved(denied);
        let reach = narrow(&mut config, &hosts(&["evil.test", "ok.test"])).unwrap();
        assert!(reach.allows("ok.test") && !reach.allows("evil.test"));
    }

    #[test]
    fn declared_hosts_and_no_network_go_through_the_proxy() {
        let mut config = resolved(json!({}));
        let reach = narrow(&mut config, &NetworkCapability::None).unwrap();
        assert_eq!(config.network, NetworkMode::Proxy);
        assert!(!reach.allows("crates.io"));
        let mut config = resolved(json!({}));
        let reach = narrow(&mut config, &hosts(&["api.example.com"])).unwrap();
        assert_eq!(config.allowed_domains, ["api.example.com"]);
        assert!(reach.allows("api.example.com") && !reach.allows("crates.io"));
        // Under the default proxy network `network: true` reaches any host not denied.
        let mut config = resolved(json!({}));
        let reach = narrow(&mut config, &NetworkCapability::Any).unwrap();
        assert!(reach.allows("api.example.com"));
        let mut config = resolved(json!({"network": "on"}));
        assert_eq!(narrow(&mut config, &NetworkCapability::Any), None);
        assert_eq!(config.network, NetworkMode::On);
    }
}
