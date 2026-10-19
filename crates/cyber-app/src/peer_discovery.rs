//! mDNS discovery of peer listeners (`server-api` → Peer discovery).
//!
//! Publishing is off unless user config sets `peers.mdns` or the server runs with
//! `cyber serve --mdns`, and only a peer listener off loopback is published (the main API
//! never is). Discovery only lists what answers: adding a peer still needs its pairing
//! code, and `cyber peers add` refuses a code whose fingerprint differs from the one
//! discovered at the same address.

use std::collections::BTreeMap;
use std::net::IpAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use cyber_core::config::{PeersSettings, valid_peer_name};
use cyber_core::paths::Paths;
use cyber_server::http::peer::{PeerHandle, lan_host, lan_name};
use mdns_sd::{IfKind, ResolvedService, ServiceDaemon, ServiceEvent, ServiceInfo};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::App;

/// The DNS-SD service type of peer listeners.
pub const SERVICE_TYPE: &str = "_cyber._tcp.local.";
/// What `cyber peers discover` found last, for `cyber peers add` to compare against.
const DISCOVERED_FILE: &str = "peers-discovered.json";
/// How long a discovery is compared against: an hour.
const DISCOVERED_TTL_MS: i64 = 3_600_000;
/// How long shutdown waits for the goodbye packets.
const GOODBYE_WAIT: Duration = Duration::from_secs(2);

/// A peer listener found on the network. Nothing here is trusted: it is what answered.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Discovered {
    pub machine: String,
    /// The advertised host name, without the trailing dot.
    pub host: String,
    pub port: u16,
    /// Sorted.
    pub addresses: Vec<IpAddr>,
    pub fingerprint: String,
    /// The host's own LAN name its pairing codes name (TXT `host`), when it says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lan_host: Option<String>,
}

impl Discovered {
    /// A resolved service with the TXT of a peer listener; `None` for anything else.
    fn from_resolved(service: &ResolvedService) -> Option<Self> {
        let txt = |key| service.get_property_val_str(key);
        if txt("role") != Some("peer") || txt("v") != Some("1") {
            return None;
        }
        let machine = txt("machine").filter(|m| valid_peer_name(m))?;
        let fingerprint = txt("fp").filter(|fp| valid_fingerprint(fp))?;
        let mut addresses: Vec<IpAddr> = service
            .get_addresses()
            .iter()
            .map(mdns_sd::ScopedIp::to_ip_addr)
            .collect();
        addresses.sort();
        Some(Self {
            machine: machine.to_string(),
            host: service.get_hostname().trim_end_matches('.').to_string(),
            port: service.get_port(),
            addresses,
            fingerprint: fingerprint.to_string(),
            lan_host: txt("host").and_then(lan_name),
        })
    }

    /// Whether `authority` (`host:port` of a pairing code) names this listener: the same
    /// port and one of its addresses, its advertised host name, its LAN name or
    /// `<machine>.local`.
    pub fn serves(&self, authority: &str) -> bool {
        let Ok(url) = url::Url::parse(&format!("https://{authority}")) else {
            return false;
        };
        if url.port() != Some(self.port) {
            return false;
        }
        match url.host() {
            Some(url::Host::Ipv4(ip)) => self.addresses.contains(&IpAddr::V4(ip)),
            Some(url::Host::Ipv6(ip)) => self.addresses.contains(&IpAddr::V6(ip)),
            Some(url::Host::Domain(name)) => {
                let name = name.trim_end_matches('.');
                name.eq_ignore_ascii_case(&self.host)
                    || name.eq_ignore_ascii_case(&format!("{}.local", self.machine))
                    || self
                        .lan_host
                        .as_deref()
                        .is_some_and(|host| name.eq_ignore_ascii_case(host))
            }
            None => false,
        }
    }
}

fn valid_fingerprint(fp: &str) -> bool {
    fp.strip_prefix("sha256:").is_some_and(|hex| {
        hex.len() == 64 && hex.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
    })
}

/// The first discovery at `authority` whose fingerprint differs from `fingerprint`.
pub fn conflict<'a>(
    found: &'a [Discovered],
    authority: &str,
    fingerprint: &str,
) -> Option<&'a Discovered> {
    found
        .iter()
        .find(|entry| entry.serves(authority) && entry.fingerprint != fingerprint)
}

/// Browse for peer listeners for `timeout`; a listener that says goodbye meanwhile is
/// left out. Fails only when mDNS cannot start at all.
pub fn browse(timeout: Duration) -> Result<Vec<Discovered>, String> {
    let daemon = ServiceDaemon::new().map_err(|error| format!("mDNS unavailable: {error}"))?;
    let events = daemon
        .browse(SERVICE_TYPE)
        .map_err(|error| format!("mDNS unavailable: {error}"))?;
    let deadline = Instant::now() + timeout;
    let mut found = BTreeMap::new();
    while let Some(left) = deadline.checked_duration_since(Instant::now()) {
        match events.recv_timeout(left) {
            Ok(ServiceEvent::ServiceResolved(service)) => {
                if let Some(entry) = Discovered::from_resolved(&service) {
                    found.insert(service.fullname.to_lowercase(), entry);
                }
            }
            Ok(ServiceEvent::ServiceRemoved(_, fullname)) => {
                found.remove(&fullname.to_lowercase());
            }
            Ok(_) => {}
            Err(_) => break,
        }
    }
    let _ = daemon.stop_browse(SERVICE_TYPE);
    let _ = daemon.shutdown();
    Ok(found.into_values().collect())
}

#[derive(Serialize, Deserialize)]
struct Remembered {
    at_ms: i64,
    peers: Vec<Discovered>,
}

/// Keep what a discovery found, replacing the previous one.
pub fn remember(paths: &Paths, found: &[Discovered], now_ms: i64) -> Result<(), String> {
    let text = serde_json::to_string(&Remembered {
        at_ms: now_ms,
        peers: found.to_vec(),
    })
    .map_err(|error| error.to_string())?;
    std::fs::create_dir_all(&paths.state).map_err(|error| error.to_string())?;
    std::fs::write(paths.state.join(DISCOVERED_FILE), text).map_err(|error| error.to_string())
}

/// What the last discovery found, if it ran within the hour.
pub fn recalled(paths: &Paths, now_ms: i64) -> Vec<Discovered> {
    std::fs::read_to_string(paths.state.join(DISCOVERED_FILE))
        .ok()
        .and_then(|text| serde_json::from_str::<Remembered>(&text).ok())
        .filter(|kept| (0..DISCOVERED_TTL_MS).contains(&(now_ms - kept.at_ms)))
        .map(|kept| kept.peers)
        .unwrap_or_default()
}

/// A published peer listener; [`Publication::unpublish`] says goodbye.
pub(crate) struct Publication {
    daemon: ServiceDaemon,
    fullname: String,
}

impl Publication {
    /// Withdraw the record and stop the responder (blocks up to a few seconds).
    pub fn unpublish(self) {
        if let Ok(status) = self.daemon.unregister(&self.fullname) {
            let _ = status.recv_timeout(GOODBYE_WAIT);
        }
        if let Ok(status) = self.daemon.shutdown() {
            let _ = status.recv_timeout(GOODBYE_WAIT);
        }
        cyber_core::log::info(
            "server",
            "mDNS unpublished",
            json!({ "name": self.fullname }),
        );
    }
}

/// Say goodbye on the network, off the async workers.
pub(crate) async fn unpublish(publication: Option<Publication>) {
    if let Some(publication) = publication {
        let _ = tokio::task::spawn_blocking(move || publication.unpublish()).await;
    }
}

/// Publish the peer listener when `peers.mdns` or `flag` asks for it; skipped with a
/// warning when there is no peer listener or it is on loopback.
pub(crate) fn start(
    app: &App,
    handle: Option<&Arc<PeerHandle>>,
    flag: bool,
) -> Option<Publication> {
    let configured = (app.global_config)()
        .ok()
        .and_then(|config| PeersSettings::from_config(&config).ok())
        .is_some_and(|settings| settings.mdns);
    if !configured && !flag {
        return None;
    }
    let skipped = |reason: String| {
        cyber_core::log::warn("server", "mDNS not published", json!({ "reason": reason }));
        None
    };
    let Some(handle) = handle else {
        return skipped("no peer listener; set peers.listen in user config".into());
    };
    if handle.address.ip().is_loopback() {
        return skipped(format!(
            "the peer listener is on loopback {}",
            handle.address
        ));
    }
    match publish(handle) {
        Ok(publication) => {
            cyber_core::log::info(
                "server",
                "mDNS published",
                json!({ "name": publication.fullname, "port": handle.address.port() }),
            );
            Some(publication)
        }
        Err(reason) => skipped(reason),
    }
}

fn publish(handle: &Arc<PeerHandle>) -> Result<Publication, String> {
    let daemon = ServiceDaemon::new().map_err(|error| format!("mDNS unavailable: {error}"))?;
    // Loopback addresses are never advertised to the network.
    let _ = daemon.disable_interface(vec![IfKind::LoopbackV4, IfKind::LoopbackV6]);
    let info = handle.info();
    let service = record(handle, &info.machine, &info.fingerprint)?;
    let fullname = service.get_fullname().to_string();
    daemon
        .register(service)
        .map_err(|error| format!("mDNS register failed: {error}"))?;
    let republish = daemon.clone();
    let weak = Arc::downgrade(handle);
    let machine = info.machine;
    handle.on_rotate(Box::new(move |fingerprint| {
        let Some(handle) = weak.upgrade() else {
            return;
        };
        if let Ok(service) = record(&handle, &machine, fingerprint) {
            let _ = republish.register(service);
        }
    }));
    Ok(Publication { daemon, fullname })
}

/// The `_cyber._tcp` record of design section 8: instance `cyber-<machine>`, TXT `role`,
/// `v`, `machine`, `fp` and, for a wildcard bind, `host` (the LAN name its pairing codes
/// name); the bound address, or every interface's for a wildcard bind.
fn record(handle: &PeerHandle, machine: &str, fingerprint: &str) -> Result<ServiceInfo, String> {
    let ip = handle.address.ip();
    let addresses: Vec<IpAddr> = if ip.is_unspecified() {
        Vec::new()
    } else {
        vec![ip]
    };
    let host = lan_host(machine);
    let mut properties = vec![
        ("role", "peer"),
        ("v", "1"),
        ("machine", machine),
        ("fp", fingerprint),
    ];
    if ip.is_unspecified() {
        properties.push(("host", host.as_str()));
    }
    let mut service = ServiceInfo::new(
        SERVICE_TYPE,
        &format!("cyber-{machine}"),
        &format!("cyber-{machine}.local."),
        &addresses[..],
        handle.address.port(),
        &properties[..],
    )
    .map_err(|error| format!("mDNS record: {error}"))?;
    if ip.is_unspecified() {
        service = service.enable_addr_auto();
        // A `0.0.0.0` listener answers IPv4 only.
        if ip.is_ipv4() {
            service.set_interfaces(vec![IfKind::IPv4]);
        }
    }
    Ok(service)
}

#[cfg(test)]
mod tests {
    use super::*;

    const FP: &str = "sha256:9f2c000000000000000000000000000000000000000000000000000000000001";

    fn desk() -> Discovered {
        Discovered {
            machine: "desk".into(),
            host: "cyber-desk.local".into(),
            port: 4748,
            addresses: vec!["192.168.1.20".parse().unwrap(), "fd00::5".parse().unwrap()],
            fingerprint: FP.into(),
            lan_host: Some("leos-mini.local".into()),
        }
    }

    #[test]
    fn a_discovery_serves_its_addresses_and_names_on_its_port() {
        let desk = desk();
        for authority in [
            "192.168.1.20:4748",
            "[fd00::5]:4748",
            "desk.local:4748",
            "DESK.local.:4748",
            "cyber-desk.local:4748",
            "Leos-Mini.local:4748",
        ] {
            assert!(desk.serves(authority), "{authority}");
        }
        for authority in [
            "192.168.1.20:4749",
            "192.168.1.21:4748",
            "laptop.local:4748",
            "desk.tailnet.example:4748",
            "not a host",
        ] {
            assert!(!desk.serves(authority), "{authority}");
        }
    }

    #[test]
    fn only_a_different_fingerprint_at_the_same_address_conflicts() {
        let found = [desk()];
        let other = format!("sha256:{}", "a".repeat(64));
        assert_eq!(
            conflict(&found, "192.168.1.20:4748", &other),
            Some(&found[0])
        );
        assert_eq!(conflict(&found, "192.168.1.20:4748", FP), None);
        assert_eq!(conflict(&found, "10.0.0.9:4748", &other), None);
        assert_eq!(conflict(&[], "192.168.1.20:4748", &other), None);
    }

    #[test]
    fn a_discovery_is_recalled_within_the_hour() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let paths = Paths {
            data: root.join("data"),
            config: root.join("config"),
            state: root.join("state"),
            cache: root.join("cache"),
            tmp: root.join("tmp"),
        };
        assert!(recalled(&paths, 0).is_empty());
        remember(&paths, &[desk()], 1_000).unwrap();
        assert_eq!(
            recalled(&paths, 1_000 + DISCOVERED_TTL_MS - 1),
            vec![desk()]
        );
        assert!(recalled(&paths, 1_000 + DISCOVERED_TTL_MS).is_empty());
        assert!(recalled(&paths, 999).is_empty(), "a clock that went back");
        std::fs::write(paths.state.join(DISCOVERED_FILE), "{").unwrap();
        assert!(recalled(&paths, 1_000).is_empty());
    }

    #[test]
    fn only_peer_records_with_a_valid_name_and_pin_are_discovered() {
        let service = |txt: &[(&str, &str)]| {
            ServiceInfo::new(
                SERVICE_TYPE,
                "cyber-desk",
                "cyber-desk.local.",
                "192.168.1.20",
                4748,
                txt,
            )
            .unwrap()
            .as_resolved_service()
        };
        let good = [
            ("role", "peer"),
            ("v", "1"),
            ("machine", "desk"),
            ("fp", FP),
        ];
        let entry = Discovered::from_resolved(&service(&good)).unwrap();
        assert_eq!(entry.machine, "desk");
        assert_eq!(entry.host, "cyber-desk.local");
        assert_eq!(entry.port, 4748);
        assert_eq!(
            entry.addresses,
            vec!["192.168.1.20".parse::<IpAddr>().unwrap()]
        );
        assert_eq!(entry.fingerprint, FP);
        assert_eq!(entry.lan_host, None);
        let mut named = good.to_vec();
        named.push(("host", "leos-mini.local"));
        let entry = Discovered::from_resolved(&service(&named)).unwrap();
        assert_eq!(entry.lan_host.as_deref(), Some("leos-mini.local"));
        named.pop();
        named.push(("host", "not a host"));
        assert_eq!(
            Discovered::from_resolved(&service(&named))
                .unwrap()
                .lan_host,
            None
        );
        let upper = FP.to_uppercase();
        for bad in [
            [("role", "api"), ("v", "1"), ("machine", "desk"), ("fp", FP)],
            [
                ("role", "peer"),
                ("v", "2"),
                ("machine", "desk"),
                ("fp", FP),
            ],
            [("role", "peer"), ("v", "1"), ("machine", "a@b"), ("fp", FP)],
            [
                ("role", "peer"),
                ("v", "1"),
                ("machine", "desk"),
                ("fp", "sha256:00"),
            ],
            [
                ("role", "peer"),
                ("v", "1"),
                ("machine", "desk"),
                ("fp", upper.as_str()),
            ],
        ] {
            assert_eq!(Discovered::from_resolved(&service(&bad)), None, "{bad:?}");
        }
    }
}
