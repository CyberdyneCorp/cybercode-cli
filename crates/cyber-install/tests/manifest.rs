//! The latest-version lookup (`installation-upgrade` → Latest version lookup) against a
//! loopback release source that counts requests.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use cyber_core::version::{Channel, ReleaseSource};
use cyber_install::manifest::{CACHE_TTL, Lookup, LookupError, Origin};

mod support {
    pub mod stub_release;
}
use support::stub_release::{Reply, StubRelease};

const MINUTE_MS: u64 = 60 * 1000;
const NOW_MS: u64 = 1_800_000_000_000;

struct Fixture {
    stub: StubRelease,
    source: ReleaseSource,
    cache: tempfile::TempDir,
}

impl Fixture {
    fn new() -> Self {
        let stub = StubRelease::start();
        let source = ReleaseSource {
            base_url: format!("{}/", stub.base_url),
            ..ReleaseSource::built_in()
        };
        Self {
            stub,
            source,
            cache: tempfile::tempdir().unwrap(),
        }
    }

    fn lookup(&self) -> Lookup<'_> {
        let mut lookup = Lookup::new(&self.source, self.cache.path(), &HashMap::new());
        lookup.now_ms = NOW_MS;
        lookup.timeout = Duration::from_millis(500);
        lookup.backoff = vec![Duration::ZERO, Duration::ZERO];
        lookup
    }

    fn write_cache(&self, channel: &str, version: &str, fetched_at_ms: u64) {
        let body = serde_json::json!({
            "fetched_at_ms": fetched_at_ms,
            "manifest": { "version": version },
        });
        std::fs::write(
            self.cache.path().join(format!("release-{channel}.json")),
            body.to_string(),
        )
        .unwrap();
    }

    fn manifest_requests(&self) -> usize {
        self.stub.requests().len()
    }
}

fn block_on<F: std::future::Future>(future: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(future)
}

#[test]
fn cached_lookup_makes_no_request() {
    let f = Fixture::new();
    f.stub.manifest("stable", "9.9.9");
    f.write_cache("stable", "1.4.0", NOW_MS - 20 * MINUTE_MS);
    let latest = block_on(f.lookup().latest(Channel::Stable)).unwrap();
    assert_eq!(latest.manifest.version, "1.4.0");
    assert_eq!(latest.origin, Origin::Cache);
    assert_eq!(f.manifest_requests(), 0);
}

#[test]
fn stale_cache_is_refreshed_from_the_network_and_rewritten() {
    let f = Fixture::new();
    f.stub.manifest("stable", "1.5.0");
    f.write_cache("stable", "1.4.0", NOW_MS - 61 * MINUTE_MS);
    let latest = block_on(f.lookup().latest(Channel::Stable)).unwrap();
    assert_eq!(latest.manifest.version, "1.5.0");
    assert_eq!(latest.manifest.min_supported.as_deref(), Some("0.0.1"));
    assert_eq!(latest.origin, Origin::Network);
    let requests = f.stub.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].path, "/channel-stable/stable.json");
    let agent = requests[0].user_agent.clone().unwrap();
    assert!(
        agent.starts_with(&format!("cyber/{} (", env!("CARGO_PKG_VERSION")))
            && agent.ends_with("; cli)"),
        "{agent}"
    );
    // The rewritten cache answers the next lookup.
    let again = block_on(f.lookup().latest(Channel::Stable)).unwrap();
    assert_eq!(again.origin, Origin::Cache);
    assert_eq!(again.fetched_at_ms, NOW_MS);
    assert_eq!(f.manifest_requests(), 1);
}

#[test]
fn a_cache_stamped_in_the_future_is_not_fresh() {
    let f = Fixture::new();
    f.stub.manifest("stable", "1.5.0");
    f.write_cache("stable", "1.4.0", NOW_MS + CACHE_TTL.as_millis() as u64);
    let latest = block_on(f.lookup().latest(Channel::Stable)).unwrap();
    assert_eq!(latest.origin, Origin::Network);
}

#[test]
fn offline_lookup_uses_a_cache_of_any_age() {
    let f = Fixture::new();
    f.stub.manifest("stable", "9.9.9");
    f.write_cache("stable", "1.4.0", NOW_MS - 3 * 24 * 60 * MINUTE_MS);
    let env = HashMap::from([("CYBER_OFFLINE".to_string(), "1".to_string())]);
    let mut lookup = Lookup::new(&f.source, f.cache.path(), &env);
    lookup.now_ms = NOW_MS;
    assert!(lookup.offline);
    let latest = block_on(lookup.latest(Channel::Stable)).unwrap();
    assert_eq!(latest.manifest.version, "1.4.0");
    assert_eq!(f.manifest_requests(), 0);
}

#[test]
fn offline_without_a_cache_fails_without_a_request() {
    let f = Fixture::new();
    let mut lookup = f.lookup();
    lookup.offline = true;
    let error = block_on(lookup.latest(Channel::Stable)).unwrap_err();
    assert!(matches!(error, LookupError::Offline));
    assert_eq!(error.to_string(), "offline: no cached release information");
    assert_eq!(f.manifest_requests(), 0);
}

#[test]
fn dev_builds_read_the_stable_manifest_and_cache() {
    let f = Fixture::new();
    f.stub.manifest("stable", "1.5.0");
    let latest = block_on(f.lookup().latest(Channel::Dev)).unwrap();
    assert_eq!(latest.channel, Channel::Stable);
    assert!(f.cache.path().join("release-stable.json").is_file());
    assert!(!f.cache.path().join("release-dev.json").exists());
}

#[test]
fn server_errors_are_retried_twice() {
    let f = Fixture::new();
    let body = br#"{"version": "1.5.0"}"#.to_vec();
    f.stub.route(
        "/channel-beta/beta.json",
        vec![Reply::Status(503), Reply::Status(502), Reply::Ok(body)],
    );
    let latest = block_on(f.lookup().latest(Channel::Beta)).unwrap();
    assert_eq!(latest.manifest.version, "1.5.0");
    assert_eq!(f.manifest_requests(), 3);
}

#[test]
fn a_third_failure_ends_the_lookup() {
    let f = Fixture::new();
    f.stub
        .route("/channel-stable/stable.json", vec![Reply::Status(500)]);
    let error = block_on(f.lookup().latest(Channel::Stable)).unwrap_err();
    assert!(error.to_string().contains("HTTP 500"), "{error}");
    assert_eq!(f.manifest_requests(), 3);
}

#[test]
fn a_missing_manifest_is_not_retried() {
    let f = Fixture::new();
    let error = block_on(f.lookup().latest(Channel::Nightly)).unwrap_err();
    assert!(error.to_string().contains("HTTP 404"), "{error}");
    assert_eq!(f.manifest_requests(), 1);
}

#[test]
fn a_slow_source_times_out_on_every_attempt() {
    let f = Fixture::new();
    f.stub.route(
        "/channel-stable/stable.json",
        vec![Reply::Delay(Duration::from_secs(3), b"{}".to_vec())],
    );
    let mut lookup = f.lookup();
    lookup.timeout = Duration::from_millis(200);
    let started = Instant::now();
    let error = block_on(lookup.latest(Channel::Stable)).unwrap_err();
    assert!(matches!(error, LookupError::Fetch { .. }), "{error}");
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "{:?}",
        started.elapsed()
    );
    assert_eq!(f.manifest_requests(), 3);
}

#[test]
fn an_invalid_manifest_is_refused_and_not_cached() {
    let f = Fixture::new();
    f.stub.route(
        "/channel-stable/stable.json",
        vec![Reply::Ok(br#"{"version": "latest"}"#.to_vec())],
    );
    let error = block_on(f.lookup().latest(Channel::Stable)).unwrap_err();
    assert!(matches!(error, LookupError::Invalid { .. }), "{error}");
    assert!(!f.cache.path().join("release-stable.json").exists());
}
