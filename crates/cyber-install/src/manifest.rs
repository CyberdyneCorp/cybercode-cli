//! The channel manifest and its cached lookup (`installation-upgrade` → Latest version
//! lookup): `<base>/channel-<channel>/<channel>.json`, fetched with a 10 second timeout
//! and 2 retries, cached for an hour in `<cache>/release-<channel>.json`.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use cyber_core::env::EnvSource;
use cyber_core::version::{Channel, ReleaseSource};
use serde::{Deserialize, Serialize};

/// How long a fetched manifest is used without asking the network again.
pub const CACHE_TTL: Duration = Duration::from_secs(60 * 60);

/// A manifest is a few hundred bytes; anything larger is not one.
const MAX_MANIFEST_BYTES: usize = 64 * 1024;

/// The latest release of one channel, as the release workflow publishes it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChannelManifest {
    pub version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub published_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_supported: Option<String>,
}

impl ChannelManifest {
    /// Both versions must be semver: a manifest that is not is refused, never guessed at.
    fn validate(self) -> Result<Self, String> {
        let versions = std::iter::once(&self.version).chain(&self.min_supported);
        for version in versions {
            semver::Version::parse(version)
                .map_err(|e| format!("version \"{version}\" is not semver: {e}"))?;
        }
        Ok(self)
    }
}

#[derive(Serialize, Deserialize)]
struct CacheFile {
    fetched_at_ms: u64,
    manifest: ChannelManifest,
}

/// Where a lookup's answer came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Origin {
    Cache,
    Network,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Latest {
    /// The channel looked up (`dev` builds look up `stable`).
    pub channel: Channel,
    pub manifest: ChannelManifest,
    pub fetched_at_ms: u64,
    pub origin: Origin,
}

#[derive(Debug, thiserror::Error)]
pub enum LookupError {
    #[error("offline: no cached release information")]
    Offline,
    #[error("release lookup failed: {url}: {reason}")]
    Fetch { url: String, reason: String },
    #[error("invalid release manifest {url}: {reason}")]
    Invalid { url: String, reason: String },
}

/// The channel whose manifest a build of `channel` reads: a `dev` build reads `stable`.
pub fn lookup_channel(channel: Channel) -> Channel {
    match channel {
        Channel::Dev => Channel::Stable,
        other => other,
    }
}

/// `cyber/<version> (<os>; <arch>; <client>)` (`cli-commands` → Version output).
pub fn user_agent(client: &str) -> String {
    format!(
        "cyber/{} ({}; {}; {client})",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH
    )
}

/// `<cache_dir>/release-<channel>.json`: the file a lookup of `channel` caches into.
pub fn cache_path(cache_dir: &Path, channel: Channel) -> PathBuf {
    cache_dir.join(format!("release-{}.json", lookup_channel(channel).as_str()))
}

/// The cached manifest of `channel`, of any age, without the network: what `exec` and
/// the server read at start for the minimum supported version.
pub fn read_cache(cache_dir: &Path, channel: Channel) -> Option<(u64, ChannelManifest)> {
    let text = std::fs::read_to_string(cache_path(cache_dir, channel)).ok()?;
    let cache: CacheFile = serde_json::from_str(&text).ok()?;
    let manifest = cache.manifest.validate().ok()?;
    Some((cache.fetched_at_ms, manifest))
}

/// One latest-version lookup: the release source, the cache directory and the network
/// policy. Tests shorten the timeout and the backoff.
pub struct Lookup<'a> {
    pub source: &'a ReleaseSource,
    pub cache_dir: &'a Path,
    /// `CYBER_OFFLINE`: answer from the cache of any age, never the network.
    pub offline: bool,
    pub timeout: Duration,
    /// The wait before each retry; its length is the number of retries.
    pub backoff: Vec<Duration>,
    pub now_ms: u64,
    /// The `<client>` of the User-Agent.
    pub client: &'static str,
}

impl<'a> Lookup<'a> {
    pub fn new(source: &'a ReleaseSource, cache_dir: &'a Path, env: &dyn EnvSource) -> Self {
        Self {
            source,
            cache_dir,
            offline: env.flag("CYBER_OFFLINE"),
            timeout: Duration::from_secs(10),
            backoff: vec![Duration::from_secs(1), Duration::from_secs(2)],
            now_ms: now_ms(),
            client: "cli",
        }
    }

    pub fn cache_path(&self, channel: Channel) -> PathBuf {
        cache_path(self.cache_dir, channel)
    }

    /// The cached manifest of `channel`, of any age, when one is readable and valid.
    pub fn cached(&self, channel: Channel) -> Option<(u64, ChannelManifest)> {
        read_cache(self.cache_dir, channel)
    }

    /// The latest release of `channel`: a cache younger than an hour, else the network.
    pub async fn latest(&self, channel: Channel) -> Result<Latest, LookupError> {
        let channel = lookup_channel(channel);
        let cached = self.cached(channel);
        let from_cache = |(fetched_at_ms, manifest)| Latest {
            channel,
            manifest,
            fetched_at_ms,
            origin: Origin::Cache,
        };
        if self.offline {
            return cached.map(from_cache).ok_or(LookupError::Offline);
        }
        if let Some(entry) = cached.filter(|(at, _)| self.is_fresh(*at)) {
            return Ok(from_cache(entry));
        }
        let manifest = self.fetch(channel).await?;
        // The answer stands even when the cache cannot be written; the next lookup asks again.
        let _ = self.write_cache(channel, &manifest);
        Ok(Latest {
            channel,
            manifest,
            fetched_at_ms: self.now_ms,
            origin: Origin::Network,
        })
    }

    fn is_fresh(&self, fetched_at_ms: u64) -> bool {
        // A cache stamped in the future (a clock moved back) is not trusted as fresh.
        self.now_ms
            .checked_sub(fetched_at_ms)
            .is_some_and(|age| u128::from(age) < CACHE_TTL.as_millis())
    }

    async fn fetch(&self, channel: Channel) -> Result<ChannelManifest, LookupError> {
        let url = self.source.manifest_url(channel);
        let failed = |reason: String| LookupError::Fetch {
            url: url.clone(),
            reason,
        };
        let client = reqwest::Client::builder()
            .timeout(self.timeout)
            .user_agent(user_agent(self.client))
            .build()
            .map_err(|e| failed(e.to_string()))?;
        let mut retries = self.backoff.iter();
        loop {
            match fetch_once(&client, &url).await {
                Ok(body) => return parse(&url, &body),
                Err(Attempt::Final(reason)) => return Err(failed(reason)),
                Err(Attempt::Retry(reason)) => match retries.next() {
                    Some(wait) => tokio::time::sleep(*wait).await,
                    None => return Err(failed(reason)),
                },
            }
        }
    }

    /// Write the cache beside its final name and rename it into place.
    fn write_cache(&self, channel: Channel, manifest: &ChannelManifest) -> std::io::Result<()> {
        std::fs::create_dir_all(self.cache_dir)?;
        let body = serde_json::to_vec_pretty(&CacheFile {
            fetched_at_ms: self.now_ms,
            manifest: manifest.clone(),
        })?;
        let mut file = tempfile::NamedTempFile::new_in(self.cache_dir)?;
        file.write_all(&body)?;
        file.as_file().sync_all()?;
        file.persist(self.cache_path(channel))
            .map(|_| ())
            .map_err(|e| e.error)
    }
}

/// A failed attempt: worth retrying (transport errors, timeouts, 5xx, 429) or not.
enum Attempt {
    Retry(String),
    Final(String),
}

async fn fetch_once(client: &reqwest::Client, url: &str) -> Result<Vec<u8>, Attempt> {
    let mut response = client
        .get(url)
        .send()
        .await
        .map_err(|e| Attempt::Retry(e.to_string()))?;
    let status = response.status();
    if status.is_server_error() || status == reqwest::StatusCode::TOO_MANY_REQUESTS {
        return Err(Attempt::Retry(format!("HTTP {status}")));
    }
    if !status.is_success() {
        return Err(Attempt::Final(format!("HTTP {status}")));
    }
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|e| Attempt::Retry(e.to_string()))?
    {
        body.extend_from_slice(&chunk);
        if body.len() > MAX_MANIFEST_BYTES {
            return Err(Attempt::Final(format!(
                "larger than {MAX_MANIFEST_BYTES} bytes"
            )));
        }
    }
    Ok(body)
}

fn parse(url: &str, body: &[u8]) -> Result<ChannelManifest, LookupError> {
    serde_json::from_slice::<ChannelManifest>(body)
        .map_err(|e| e.to_string())
        .and_then(ChannelManifest::validate)
        .map_err(|reason| LookupError::Invalid {
            url: url.to_string(),
            reason,
        })
}

/// Milliseconds since the Unix epoch.
pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dev_builds_look_up_stable() {
        assert_eq!(lookup_channel(Channel::Dev), Channel::Stable);
        assert_eq!(lookup_channel(Channel::Beta), Channel::Beta);
        assert_eq!(lookup_channel(Channel::Nightly), Channel::Nightly);
    }

    #[test]
    fn user_agent_names_version_platform_and_client() {
        let agent = user_agent("server");
        assert!(
            agent.starts_with(&format!("cyber/{} (", env!("CARGO_PKG_VERSION"))),
            "{agent}"
        );
        assert!(agent.ends_with("; server)"), "{agent}");
    }

    #[test]
    fn manifests_with_non_semver_versions_are_refused() {
        let bad = |body: &str| parse("u", body.as_bytes()).unwrap_err().to_string();
        assert!(bad(r#"{"version": "latest"}"#).contains("not semver"));
        assert!(bad(r#"{"version": "1.0.0", "min_supported": "x"}"#).contains("not semver"));
        assert!(bad(r#"{"tag": "v1"}"#).contains("invalid release manifest"));
        let good = parse("u", br#"{"version": "1.4.0", "min_supported": "1.0.0"}"#).unwrap();
        assert_eq!(good.min_supported.as_deref(), Some("1.0.0"));
    }
}
