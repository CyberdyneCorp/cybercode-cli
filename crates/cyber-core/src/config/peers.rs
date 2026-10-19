//! `peers`: direct peer transport settings (`cross-session-messaging` → Direct peer
//! transport, `server-api` → Peer listener).
//!
//! `peers` can open a network listener and names the machines a Session may message, so
//! it is accepted only from user config: project, local, environment, profile and
//! `--config` layers that set it fail validation ([`layer_issue`]). The rules fail
//! closed: plain HTTP only on loopback, pinned SHA-256 fingerprints, unique names.

use std::collections::HashMap;
use std::fmt;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use serde_json::{Map, Value};

const KEY: &str = "peers";
/// The command that writes `peers.remotes`, named by every message about them.
const ADD: &str = "`cyber peers add <name> '<code>'`";
/// Longest peer or machine name.
const NAME_MAX: usize = 64;

/// `peers.listen`: the second listener that serves only `/api/v1/peer/*`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PeerListen {
    pub address: SocketAddr,
    /// TLS with the self-signed peer certificate; `false` only on a loopback address.
    pub tls: bool,
}

/// One entry of `peers.remotes`: a machine this one sends to.
#[derive(Clone, PartialEq, Eq)]
pub struct PeerRemote {
    pub name: String,
    /// `https://…`, or `http://…` on a loopback host (an SSH-forwarded port).
    pub url: url::Url,
    /// `sha256:<64 hex>` of the peer certificate's SubjectPublicKeyInfo.
    pub fingerprint: String,
    pub enabled: bool,
    /// A token given in config (usually `{env:NAME}`); otherwise it is in the keyring.
    pub token: Option<String>,
}

impl fmt::Debug for PeerRemote {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PeerRemote")
            .field("name", &self.name)
            .field("url", &self.url.as_str())
            .field("fingerprint", &self.fingerprint)
            .field("enabled", &self.enabled)
            .field("token", &self.token.as_ref().map(|_| "***"))
            .finish()
    }
}

/// The resolved `peers` key of the user configuration.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PeersSettings {
    /// `None`: no peer listener (the default).
    pub listen: Option<PeerListen>,
    pub mdns: bool,
    /// The name shown to peers; the host name when unset.
    pub machine: Option<String>,
    /// Peer listings show full Location paths, not only the workspace name.
    pub share_paths: bool,
    pub remotes: Vec<PeerRemote>,
}

impl PeersSettings {
    /// Validate `peers` in a configuration value and apply the defaults. Errors name the
    /// offending path.
    pub fn from_config(config: &Value) -> Result<Self, String> {
        let mut settings = Self::default();
        let Some(value) = config.get(KEY) else {
            return Ok(settings);
        };
        let map = match value {
            Value::Object(map) => map,
            Value::Array(_) => {
                return Err(format!(
                    "{KEY}: a list of peers is not supported; pair each machine with {ADD}, \
                     which writes {KEY}.remotes"
                ));
            }
            other => return Err(format!("{KEY}: expected an object, got {other}")),
        };
        for (key, value) in map {
            let path = format!("{KEY}.{key}");
            match key.as_str() {
                "listen" => settings.listen = Some(listen(value)?),
                "mdns" => settings.mdns = boolean(&path, value)?,
                "machine" => settings.machine = Some(name(&path, value)?),
                "share_paths" => settings.share_paths = boolean(&path, value)?,
                "remotes" => settings.remotes = remotes(value)?,
                _ => return Err(format!("{path}: unknown key")),
            }
        }
        Ok(settings)
    }
}

/// The issue of a layer other than user config that sets `peers`, naming the layer.
pub(super) fn layer_issue(label: &str, layer: &Value) -> Option<String> {
    layer.get(KEY)?;
    Some(format!(
        "{KEY}: accepted only in user config (cyber.jsonc in the Cyber config directory), \
         not in {label}; remove it there and pair machines with {ADD}"
    ))
}

fn boolean(path: &str, value: &Value) -> Result<bool, String> {
    value
        .as_bool()
        .ok_or_else(|| format!("{path}: expected true or false, got {value}"))
}

fn object<'a>(path: &str, value: &'a Value) -> Result<&'a Map<String, Value>, String> {
    value
        .as_object()
        .ok_or_else(|| format!("{path}: expected an object, got {value}"))
}

/// A peer or machine name: it follows `@` in addresses, so letters, digits, `.`, `_` and
/// `-`, starting with a letter or digit.
fn name(path: &str, value: &Value) -> Result<String, String> {
    value
        .as_str()
        .filter(|name| valid_peer_name(name))
        .map(str::to_owned)
        .ok_or_else(|| {
            format!(
                "{path}: expected a name of letters, digits, '.', '_' or '-' \
                 (at most {NAME_MAX}), got {value}"
            )
        })
}

/// A peer or machine name: what may follow `@` in an address.
pub fn valid_peer_name(name: &str) -> bool {
    name.len() <= NAME_MAX
        && name.starts_with(|c: char| c.is_ascii_alphanumeric())
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

fn listen(value: &Value) -> Result<PeerListen, String> {
    let path = format!("{KEY}.listen");
    let mut address = None;
    let mut tls = true;
    for (key, value) in object(&path, value)? {
        match key.as_str() {
            "address" => address = Some(socket_address(value)?),
            "tls" => tls = boolean(&format!("{path}.tls"), value)?,
            _ => return Err(format!("{path}.{key}: unknown key")),
        }
    }
    let address = address.ok_or_else(|| format!("{path}.address: required"))?;
    if !tls && !address.ip().is_loopback() {
        return Err(format!(
            "{path}.tls: false is allowed only on a loopback address, got {address}; keep \
             TLS on or bind 127.0.0.1 behind an SSH tunnel"
        ));
    }
    Ok(PeerListen { address, tls })
}

/// `ip:port`, or `localhost:port` for `127.0.0.1:port`.
fn socket_address(value: &Value) -> Result<SocketAddr, String> {
    let text = value.as_str().unwrap_or_default();
    let localhost = text
        .strip_prefix("localhost:")
        .and_then(|port| port.parse::<u16>().ok())
        .map(|port| SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port));
    localhost
        .or_else(|| text.parse().ok())
        .ok_or_else(|| format!("{KEY}.listen.address: expected ip:port, got {value}"))
}

fn remotes(value: &Value) -> Result<Vec<PeerRemote>, String> {
    let path = format!("{KEY}.remotes");
    let list = value
        .as_array()
        .ok_or_else(|| format!("{path}: expected an array of remotes, got {value}"))?;
    let mut seen = HashMap::new();
    let mut remotes = Vec::with_capacity(list.len());
    for (index, value) in list.iter().enumerate() {
        let path = format!("{path}[{index}]");
        let remote = remote(&path, value)?;
        if let Some(first) = seen.insert(remote.name.clone(), index) {
            return Err(format!(
                "{path}.name: \"{}\" is already {KEY}.remotes[{first}]; keep one entry per \
                 machine and re-pair it with {ADD}",
                remote.name
            ));
        }
        remotes.push(remote);
    }
    Ok(remotes)
}

fn remote(path: &str, value: &Value) -> Result<PeerRemote, String> {
    let map = object(path, value)?;
    for key in map.keys() {
        if !matches!(
            key.as_str(),
            "name" | "url" | "fingerprint" | "enabled" | "token"
        ) {
            return Err(format!("{path}.{key}: unknown key"));
        }
    }
    let field = |key: &str| {
        map.get(key)
            .ok_or_else(|| format!("{path}.{key}: required; {ADD} writes it"))
    };
    let token = match map.get("token") {
        None => None,
        Some(value) => Some(token(&format!("{path}.token"), value)?),
    };
    Ok(PeerRemote {
        name: name(&format!("{path}.name"), field("name")?)?,
        url: remote_url(&format!("{path}.url"), field("url")?)?,
        fingerprint: fingerprint(&format!("{path}.fingerprint"), field("fingerprint")?)?,
        enabled: match map.get("enabled") {
            None => true,
            Some(value) => boolean(&format!("{path}.enabled"), value)?,
        },
        token,
    })
}

fn token(path: &str, value: &Value) -> Result<String, String> {
    value
        .as_str()
        .filter(|token| !token.trim().is_empty())
        .map(str::to_owned)
        .ok_or_else(|| format!("{path}: expected a non-empty string such as \"{{env:NAME}}\""))
}

/// `https`, or `http` only to a loopback host; no credentials in the URL.
fn remote_url(path: &str, value: &Value) -> Result<url::Url, String> {
    let url = value
        .as_str()
        .and_then(|text| url::Url::parse(text).ok())
        .filter(|url| url.host().is_some())
        .ok_or_else(|| format!("{path}: expected an https URL, got {value}"))?;
    if !url.username().is_empty() || url.password().is_some() {
        return Err(format!(
            "{path}: credentials do not belong in the URL; {ADD} stores the token"
        ));
    }
    match url.scheme() {
        "https" => Ok(url),
        "http" if loopback_host(&url) => Ok(url),
        "http" => Err(format!(
            "{path}: plain http is allowed only on loopback, got {url}; use the https URL \
             of the pairing code with {ADD}"
        )),
        scheme => Err(format!("{path}: expected https, got {scheme}")),
    }
}

fn loopback_host(url: &url::Url) -> bool {
    match url.host() {
        Some(url::Host::Domain(domain)) => domain.eq_ignore_ascii_case("localhost"),
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        None => false,
    }
}

/// `sha256:` and 64 lowercase hex digits.
fn fingerprint(path: &str, value: &Value) -> Result<String, String> {
    value
        .as_str()
        .filter(|text| {
            text.strip_prefix("sha256:").is_some_and(|hex| {
                hex.len() == 64
                    && hex
                        .chars()
                        .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c))
            })
        })
        .map(str::to_owned)
        .ok_or_else(|| {
            format!(
                "{path}: expected sha256:<64 lowercase hex digits>, got {value}; copy it \
                 from the pairing code with {ADD}"
            )
        })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    const FP: &str = "sha256:9f2c000000000000000000000000000000000000000000000000000000000001";

    fn error(peers: Value) -> String {
        PeersSettings::from_config(&json!({ "peers": peers })).unwrap_err()
    }

    #[test]
    fn defaults_apply_without_the_key() {
        let settings = PeersSettings::from_config(&json!({})).unwrap();
        assert_eq!(settings, PeersSettings::default());
        assert!(settings.listen.is_none() && !settings.mdns && !settings.share_paths);
    }

    #[test]
    fn every_key_is_read() {
        let settings = PeersSettings::from_config(&json!({"peers": {
            "listen": {"address": "0.0.0.0:4748"},
            "mdns": true,
            "machine": "desk",
            "share_paths": true,
            "remotes": [
                {"name": "laptop", "url": "https://laptop.local:4748", "fingerprint": FP},
                {"name": "box", "url": "http://127.0.0.1:14748", "fingerprint": FP,
                    "enabled": false, "token": "cyp_from_env"},
            ],
        }}))
        .unwrap();
        assert_eq!(
            settings.listen,
            Some(PeerListen {
                address: "0.0.0.0:4748".parse().unwrap(),
                tls: true
            })
        );
        assert!(settings.mdns && settings.share_paths);
        assert_eq!(settings.machine.as_deref(), Some("desk"));
        let [laptop, tunnel] = settings.remotes.as_slice() else {
            panic!("{settings:?}")
        };
        assert_eq!(
            (laptop.name.as_str(), laptop.enabled, &laptop.token),
            ("laptop", true, &None)
        );
        assert_eq!(laptop.url.as_str(), "https://laptop.local:4748/");
        assert_eq!(tunnel.token.as_deref(), Some("cyp_from_env"));
        assert!(!tunnel.enabled);
    }

    #[test]
    fn a_plain_listener_is_allowed_only_on_loopback() {
        for address in ["127.0.0.1:4748", "[::1]:4748", "localhost:4748"] {
            let settings = PeersSettings::from_config(&json!({"peers": {
                "listen": {"address": address, "tls": false}}}))
            .unwrap();
            let listen = settings.listen.unwrap();
            assert!(
                !listen.tls && listen.address.ip().is_loopback(),
                "{address}"
            );
        }
        for address in ["0.0.0.0:4748", "192.168.1.20:4748", "[::]:4748"] {
            let error = error(json!({"listen": {"address": address, "tls": false}}));
            assert!(error.starts_with("peers.listen.tls:"), "{error}");
            assert!(error.contains("loopback"), "{error}");
        }
    }

    #[test]
    fn plain_http_remotes_are_allowed_only_on_loopback() {
        for url in [
            "http://127.0.0.1:4748",
            "http://localhost:4748",
            "http://[::1]:4748",
        ] {
            let remote = json!([{"name": "desk", "url": url, "fingerprint": FP}]);
            PeersSettings::from_config(&json!({"peers": {"remotes": remote}})).unwrap();
        }
        for url in ["http://desk.local:4748", "http://192.168.1.20:4748"] {
            let error =
                error(json!({"remotes": [{"name": "desk", "url": url, "fingerprint": FP}]}));
            assert!(error.starts_with("peers.remotes[0].url:"), "{error}");
            assert!(error.contains("cyber peers add"), "{error}");
        }
        for url in ["ftp://desk:21", "https://me:pw@desk:4748", "desk:4748"] {
            let error =
                error(json!({"remotes": [{"name": "desk", "url": url, "fingerprint": FP}]}));
            assert!(error.starts_with("peers.remotes[0].url:"), "{url}: {error}");
        }
    }

    #[test]
    fn duplicate_remote_names_are_refused() {
        let remote = json!({"name": "laptop", "url": "https://laptop:4748", "fingerprint": FP});
        let error = error(json!({"remotes": [remote, {"name": "box",
            "url": "https://box:4748", "fingerprint": FP}, remote]}));
        assert!(error.starts_with("peers.remotes[2].name:"), "{error}");
        assert!(error.contains("peers.remotes[0]"), "{error}");
        assert!(error.contains("cyber peers add"), "{error}");
    }

    #[test]
    fn the_old_array_shape_is_refused() {
        let error = error(json!([{"name": "laptop", "url": "https://laptop:4748"}]));
        assert!(error.starts_with("peers:"), "{error}");
        assert!(error.contains("cyber peers add"), "{error}");
    }

    #[test]
    fn validation_errors_name_the_path() {
        let remote = |extra: Value| {
            let mut remote = json!({"name": "desk", "url": "https://desk:4748", "fingerprint": FP});
            remote
                .as_object_mut()
                .unwrap()
                .extend(extra.as_object().unwrap().clone());
            json!({"remotes": [remote]})
        };
        for (peers, path) in [
            (json!(true), "peers:"),
            (json!({"listen": "0.0.0.0:4748"}), "peers.listen:"),
            (json!({"listen": {}}), "peers.listen.address:"),
            (
                json!({"listen": {"address": "desk:4748"}}),
                "peers.listen.address:",
            ),
            (
                json!({"listen": {"address": "127.0.0.1:1", "tls": "no"}}),
                "peers.listen.tls:",
            ),
            (
                json!({"listen": {"address": "127.0.0.1:1", "cert": "x"}}),
                "peers.listen.cert:",
            ),
            (json!({"mdns": "yes"}), "peers.mdns:"),
            (json!({"share_paths": 1}), "peers.share_paths:"),
            (json!({"machine": "my desk"}), "peers.machine:"),
            (json!({"machine": "a@b"}), "peers.machine:"),
            (json!({"insecure": true}), "peers.insecure:"),
            (json!({"remotes": {}}), "peers.remotes:"),
            (json!({"remotes": ["desk"]}), "peers.remotes[0]:"),
            (remote(json!({"name": "-desk"})), "peers.remotes[0].name:"),
            (
                remote(json!({"name": "x".repeat(65)})),
                "peers.remotes[0].name:",
            ),
            (
                remote(json!({"fingerprint": "9f2c"})),
                "peers.remotes[0].fingerprint:",
            ),
            (
                remote(json!({"fingerprint": FP.to_uppercase()})),
                "peers.remotes[0].fingerprint:",
            ),
            (
                remote(json!({"enabled": "yes"})),
                "peers.remotes[0].enabled:",
            ),
            (remote(json!({"token": ""})), "peers.remotes[0].token:"),
            (
                remote(json!({"insecure": true})),
                "peers.remotes[0].insecure:",
            ),
            (
                json!({"remotes": [{"name": "desk", "url": "https://desk"}]}),
                "peers.remotes[0].fingerprint:",
            ),
        ] {
            let error = error(peers.clone());
            assert!(error.starts_with(path), "{peers}: {error}");
        }
    }

    #[test]
    fn layers_other_than_user_config_are_named() {
        assert_eq!(
            layer_issue("project:/r/cyber.jsonc", &json!({"mcp": {}})),
            None
        );
        let issue =
            layer_issue("project:/r/.cyber/cyber.local.jsonc", &json!({"peers": {}})).unwrap();
        assert!(issue.contains("user config"), "{issue}");
        assert!(
            issue.contains("project:/r/.cyber/cyber.local.jsonc"),
            "{issue}"
        );
        assert!(issue.contains("cyber peers add"), "{issue}");
    }

    #[test]
    fn debug_output_hides_the_token() {
        let settings = PeersSettings::from_config(&json!({"peers": {"remotes": [
            {"name": "desk", "url": "https://desk:4748", "fingerprint": FP,
                "token": "cyp_secret_value"}]}}))
        .unwrap();
        let shown = format!("{settings:?}");
        assert!(
            !shown.contains("cyp_secret_value") && shown.contains("***"),
            "{shown}"
        );
    }
}
