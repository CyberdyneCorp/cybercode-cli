//! Remote skill sources (`skills-commands` → Remote skill sources).
//!
//! A `skills` entry that is an `https://` URL names an index (`<url>/index.json`). Listed
//! files are fetched through a [`RemoteSkillFetcher`], validated, and written owner-only
//! into `<cache>/skills/<host>/<source>/<generation>/<name>/`, where `<source>` is derived
//! from the source URL, so two sources on one host never share a directory. A marker in the
//! source directory records the fetch time, the generation and the digest of what was
//! written; a generation is never modified after the marker names it, and every use of the
//! cache re-hashes it against that digest, so an approval binds to the bytes loaded.
//! Discovery reads only that cache; refreshes run in the background at most once every 24
//! hours, so startup never waits for the network and an offline machine keeps using the
//! last good copy.
use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use cyber_sandbox::NetworkMode;
use futures::StreamExt;
use futures::future::BoxFuture;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// A cache younger than this is used without contacting the source.
pub const REFRESH_INTERVAL: Duration = Duration::from_secs(24 * 3600);
/// After a failed refresh, the same source is not retried sooner than this.
const RETRY_BACKOFF: Duration = Duration::from_secs(300);
const INDEX_LIMIT: usize = 256 * 1024;
const FILE_LIMIT: usize = 1024 * 1024;
const SKILL_LIMIT: usize = 4 * 1024 * 1024;
const SOURCE_LIMIT: usize = 16 * 1024 * 1024;
const MAX_SKILLS: usize = 64;
const MAX_FILES: usize = 64;
const MARKER_LIMIT: u64 = 256 * 1024;
const FETCH_TIMEOUT: Duration = Duration::from_secs(30);

/// One GET the fetcher must perform. Implementations must refuse bodies over `max_bytes`.
#[derive(Debug, Clone)]
pub struct FetchRequest {
    pub url: reqwest::Url,
    pub max_bytes: usize,
    /// The network mode and allowlist the request is subject to.
    pub network: NetworkMode,
    pub allowed_domains: Vec<String>,
}

/// Fetches remote skill files. Production uses [`PolicyFetcher`]; embedders and tests
/// inject their own (for example an in-process server).
pub trait RemoteSkillFetcher: Send + Sync {
    fn fetch(&self, request: FetchRequest) -> BoxFuture<'_, Result<Vec<u8>, String>>;
}

/// HTTPS only, through the owned policy proxy: the effective network mode and allowlist
/// apply, ambient proxies are ignored, and redirects are never followed.
pub struct PolicyFetcher;

impl RemoteSkillFetcher for PolicyFetcher {
    fn fetch(&self, request: FetchRequest) -> BoxFuture<'_, Result<Vec<u8>, String>> {
        Box::pin(async move {
            if request.url.scheme() != "https" {
                return Err("remote skill sources require https://".into());
            }
            get_bounded(&request).await
        })
    }
}

/// A policy-checked GET whose body may not exceed `max_bytes`; any non-2xx status,
/// including a redirect, fails.
pub(crate) async fn get_bounded(request: &FetchRequest) -> Result<Vec<u8>, String> {
    let crate::net_client::PolicyClient { client, proxy } = crate::net_client::policy_client(
        &request.url,
        request.network,
        request.allowed_domains.clone(),
        "remote skill source",
    )
    .await?;
    let result = async {
        let response = client
            .get(request.url.clone())
            .timeout(FETCH_TIMEOUT)
            .send()
            .await
            .map_err(|e| format!("remote skill source transport failed: {}", e.without_url()))?;
        if !response.status().is_success() {
            return Err(format!(
                "remote skill source returned status {}",
                response.status().as_u16()
            ));
        }
        read_bounded(response, request.max_bytes).await
    }
    .await;
    let _ = tokio::time::timeout(Duration::from_secs(2), proxy.shutdown()).await;
    result
}

async fn read_bounded(response: reqwest::Response, limit: usize) -> Result<Vec<u8>, String> {
    let too_large = || format!("remote skill file exceeds {limit} bytes");
    if response.content_length().is_some_and(|n| n > limit as u64) {
        return Err(too_large());
    }
    let mut bytes = Vec::new();
    let mut chunks = response.bytes_stream();
    while let Some(chunk) = chunks.next().await {
        let chunk =
            chunk.map_err(|e| format!("remote skill source failed: {}", e.without_url()))?;
        if chunk.len() > limit - bytes.len() {
            return Err(too_large());
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

/// One `index.json` entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndexSkill {
    pub name: String,
    pub files: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

#[derive(Deserialize)]
struct Index {
    skills: Vec<IndexSkill>,
}

/// Parse and validate `index.json`: bounded counts, valid unique names, a `SKILL.md` in
/// every skill and only plain relative file paths.
pub fn parse_index(bytes: &[u8]) -> Result<Vec<IndexSkill>, String> {
    if bytes.len() > INDEX_LIMIT {
        return Err(format!("index.json exceeds {INDEX_LIMIT} bytes"));
    }
    let index: Index =
        serde_json::from_slice(bytes).map_err(|e| format!("invalid index.json: {e}"))?;
    if index.skills.len() > MAX_SKILLS {
        return Err(format!("index.json lists more than {MAX_SKILLS} skills"));
    }
    let mut names = BTreeSet::new();
    for skill in &index.skills {
        validate_skill(skill)?;
        if !names.insert(skill.name.as_str()) {
            return Err(format!("index.json lists skill {} twice", skill.name));
        }
    }
    Ok(index.skills)
}

fn validate_skill(skill: &IndexSkill) -> Result<(), String> {
    cyber_core::skills::validate_name(&skill.name)?;
    if skill.files.is_empty() || skill.files.len() > MAX_FILES {
        return Err(format!(
            "skill {} must list 1-{MAX_FILES} files",
            skill.name
        ));
    }
    if skill
        .version
        .as_ref()
        .is_some_and(|v| v.len() > 64 || v.chars().any(char::is_control))
    {
        return Err(format!("skill {} has an invalid version", skill.name));
    }
    let mut seen = BTreeSet::new();
    for file in &skill.files {
        validate_file_path(file).map_err(|e| format!("skill {}: {e}", skill.name))?;
        // Case-insensitive file systems would merge these.
        if !seen.insert(file.to_ascii_lowercase()) {
            return Err(format!("skill {} lists {file} twice", skill.name));
        }
    }
    if !skill.files.iter().any(|f| f == "SKILL.md") {
        return Err(format!("skill {} does not list SKILL.md", skill.name));
    }
    Ok(())
}

/// A file path from an index: relative, `/`-separated, at most 8 components of ASCII
/// letters, digits, `.`, `_` and `-`, with no `..`, hidden names, trailing dots or
/// Windows device names. Anything else could escape the skill directory or alias a file.
pub fn validate_file_path(path: &str) -> Result<(), String> {
    let invalid = |why: &str| Err(format!("invalid file path {path:?}: {why}"));
    if path.is_empty() || path.len() > 256 {
        return invalid("must be 1-256 characters");
    }
    if path.starts_with('/') || path.contains('\\') || path.contains(':') {
        return invalid("must be a relative path with / separators");
    }
    let parts: Vec<&str> = path.split('/').collect();
    if parts.len() > 8 {
        return invalid("is nested too deeply");
    }
    for part in parts {
        if part.is_empty() || part.starts_with('.') || part.ends_with('.') {
            return invalid("has an empty, hidden, relative or dot-terminated component");
        }
        if !part
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
        {
            return invalid("may contain only letters, digits, '.', '_' and '-'");
        }
        if reserved_device(part) {
            return invalid("uses a reserved device name");
        }
    }
    Ok(())
}

fn reserved_device(part: &str) -> bool {
    let stem = part.split('.').next().unwrap_or(part).to_ascii_uppercase();
    matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || ["COM", "LPT"].iter().any(|prefix| {
            stem.strip_prefix(prefix)
                .is_some_and(|n| n.len() == 1 && n.as_bytes()[0].is_ascii_digit())
        })
}

/// A remote source URL: `https://` with a host, no credentials, query or fragment.
pub fn parse_source_url(raw: &str) -> Result<reqwest::Url, String> {
    let url = reqwest::Url::parse(raw)
        .map_err(|_| "remote skill source is not a valid URL".to_string())?;
    if url.scheme() != "https" {
        return Err("remote skill sources require https://".into());
    }
    if url.host_str().is_none_or(str::is_empty)
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(
            "remote skill source URL must have a host and no credentials, query or fragment".into(),
        );
    }
    Ok(url)
}

/// `<host>` or `<host>_<port>`, safe as one directory name on every platform.
fn host_dir(url: &reqwest::Url) -> String {
    let host: String = url
        .host_str()
        .unwrap_or_default()
        .to_ascii_lowercase()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '.' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let host = host.trim_matches('.').to_string();
    match url.port() {
        Some(port) => format!("{host}_{port}"),
        None => host,
    }
}

/// The source with exactly one trailing slash, so joins stay beneath it.
fn base(url: &reqwest::Url) -> reqwest::Url {
    let mut base = url.clone();
    if !base.path().ends_with('/') {
        let path = format!("{}/", base.path());
        base.set_path(&path);
    }
    base
}

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

/// The digest a checkout approves for a project-configured source: its URL and content.
pub fn approval_digest(url: &str, content_digest: &str) -> String {
    format!(
        "sha256:{}",
        sha256_hex(format!("{url}\n{content_digest}").as_bytes())
    )
}

const MARKER_VERSION: u32 = 2;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Marker {
    version: u32,
    url: String,
    fetched_at: u64,
    digest: String,
    /// The directory, beside the marker, that holds this fetch's skills.
    generation: String,
    skills: Vec<IndexSkill>,
}

/// `g<pid>-<nanos>`, as [`RemoteSkills::store`] names a generation.
fn valid_generation(name: &str) -> bool {
    name.len() <= 64
        && name.strip_prefix('g').is_some_and(|rest| {
            !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit() || b == b'-')
        })
}

/// The network a refresh may use.
#[derive(Debug, Clone)]
pub struct Network {
    pub mode: NetworkMode,
    pub allowed_domains: Vec<String>,
}

/// What discovery may use from one source right now.
#[derive(Debug, Default)]
pub struct Resolved {
    /// Cached skill directories, in index order.
    pub dirs: Vec<PathBuf>,
    /// Digest of the cached content, when there is a cache.
    pub digest: Option<String>,
    /// [`approval_digest`] of the normalized URL and that content.
    pub approval: Option<String>,
    /// `(skill, message)`: a note on one cached skill, or on the whole source.
    pub notes: Vec<(Option<String>, String)>,
    /// Whether this call started a background refresh.
    pub refreshing: bool,
}

#[derive(Default)]
struct SourceState {
    running: bool,
    failure: Option<String>,
    retry_after: Option<Instant>,
}

/// The remote skill cache and its background refreshes.
#[derive(Clone)]
pub struct RemoteSkills {
    root: PathBuf,
    fetcher: Arc<dyn RemoteSkillFetcher>,
    state: Arc<Mutex<HashMap<String, SourceState>>>,
}

struct Cached {
    marker: Marker,
    age: Duration,
    dirs: Vec<PathBuf>,
}

impl RemoteSkills {
    /// `root` is `<cache>/skills`.
    pub fn new(root: PathBuf, fetcher: Arc<dyn RemoteSkillFetcher>) -> Self {
        Self {
            root,
            fetcher,
            state: Arc::default(),
        }
    }

    /// `<root>/<host>/<first 16 hex digits of sha256(url)>`: one directory per source.
    fn source_dir(&self, url: &reqwest::Url) -> PathBuf {
        let id = &sha256_hex(url.as_str().as_bytes())[..16];
        self.root.join(host_dir(url)).join(id)
    }

    fn marker_path(&self, url: &reqwest::Url) -> PathBuf {
        self.source_dir(url).join(".marker.json")
    }

    /// The cached skills of `raw`, scheduling a background refresh when the cache is
    /// missing or older than [`REFRESH_INTERVAL`]. Never waits for the network.
    pub fn resolve(&self, raw: &str, network: Network) -> Resolved {
        let mut out = Resolved::default();
        let url = match parse_source_url(raw) {
            Ok(url) => url,
            Err(reason) => {
                out.notes.push((None, reason));
                return out;
            }
        };
        let cached = self.cached(&url);
        let due = cached
            .as_ref()
            .ok()
            .and_then(Option::as_ref)
            .is_none_or(|c| c.age >= REFRESH_INTERVAL);
        if due {
            out.refreshing = self.schedule(&url, network);
        }
        let failure = self.failure(&url);
        match cached {
            Ok(Some(cached)) => {
                if cached.age >= REFRESH_INTERVAL {
                    let hours = cached.age.as_secs() / 3600;
                    let status = failure.map_or_else(
                        || "refresh pending".to_string(),
                        |f| format!("refresh failed: {f}"),
                    );
                    for skill in &cached.marker.skills {
                        out.notes.push((
                            Some(skill.name.clone()),
                            format!("using stale cache fetched {hours} h ago; {status}"),
                        ));
                    }
                }
                out.approval = Some(approval_digest(url.as_str(), &cached.marker.digest));
                out.digest = Some(cached.marker.digest);
                out.dirs = cached.dirs;
            }
            Ok(None) => out.notes.push((
                None,
                failure.map_or_else(
                    || "not cached yet; fetching in the background".to_string(),
                    |f| format!("fetch failed and nothing is cached: {f}"),
                ),
            )),
            Err(reason) => out.notes.push((None, reason)),
        }
        out
    }

    /// The approval digest of what is cached for `raw` now, if anything.
    pub fn cached_approval_digest(&self, raw: &str) -> Result<String, String> {
        let url = parse_source_url(raw)?;
        let cached = self
            .cached(&url)?
            .ok_or("remote skill source is not cached yet; try again after it is fetched")?;
        Ok(approval_digest(url.as_str(), &cached.marker.digest))
    }

    fn failure(&self, url: &reqwest::Url) -> Option<String> {
        let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        state.get(url.as_str()).and_then(|s| s.failure.clone())
    }

    /// The marker and its skill directories, re-hashed against the marker's digest; `Err`
    /// when the cache is unusable.
    fn cached(&self, url: &reqwest::Url) -> Result<Option<Cached>, String> {
        let path = self.marker_path(url);
        let metadata = match std::fs::symlink_metadata(&path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err("remote skill cache is unavailable".into()),
            Ok(m) => m,
        };
        if !metadata.is_file() || metadata.len() > MARKER_LIMIT {
            return Err("remote skill cache marker is not a bounded regular file".into());
        }
        let bytes = std::fs::read(&path).map_err(|_| "remote skill cache is unavailable")?;
        let marker: Marker = serde_json::from_slice(&bytes)
            .map_err(|_| "remote skill cache marker is invalid; it will be refetched")?;
        if marker.version != MARKER_VERSION
            || marker.url != url.as_str()
            || !valid_generation(&marker.generation)
            || marker.skills.iter().any(|s| validate_skill(s).is_err())
        {
            return Err("remote skill cache marker does not match its source".into());
        }
        let generation = self.source_dir(url).join(&marker.generation);
        let dirs: Vec<PathBuf> = marker
            .skills
            .iter()
            .map(|s| generation.join(&s.name))
            .collect();
        if dirs
            .iter()
            .any(|d| !std::fs::symlink_metadata(d).is_ok_and(|m| m.is_dir()))
        {
            return Err("remote skill cache is incomplete; it will be refetched".into());
        }
        // The approval and the loaded skills must describe the same bytes.
        if content_digest(&generation, &marker.skills).ok().as_ref() != Some(&marker.digest) {
            return Err(
                "remote skill cache does not match its recorded digest; it will be refetched"
                    .into(),
            );
        }
        let age = Duration::from_secs(unix_now().saturating_sub(marker.fetched_at));
        Ok(Some(Cached { marker, age, dirs }))
    }

    /// Start one background refresh unless one is running or a recent one failed.
    fn schedule(&self, url: &reqwest::Url, network: Network) -> bool {
        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            return false;
        };
        {
            let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
            let entry = state.entry(url.as_str().to_string()).or_default();
            if entry.running || entry.retry_after.is_some_and(|t| Instant::now() < t) {
                return false;
            }
            entry.running = true;
        }
        let (this, url) = (self.clone(), url.clone());
        handle.spawn(async move {
            let result = this.refresh(&url, &network).await;
            let mut state = this.state.lock().unwrap_or_else(PoisonError::into_inner);
            let entry = state.entry(url.as_str().to_string()).or_default();
            entry.running = false;
            match result {
                Ok(()) => {
                    entry.failure = None;
                    entry.retry_after = None;
                }
                Err(error) => {
                    entry.failure = Some(error);
                    entry.retry_after = Some(Instant::now() + RETRY_BACKOFF);
                }
            }
        });
        true
    }

    /// Fetch the index and every listed file, then replace the cache. On any failure the
    /// previous cache stays as it was.
    pub async fn refresh(&self, url: &reqwest::Url, network: &Network) -> Result<(), String> {
        let base = base(url);
        let request = |url: reqwest::Url, max_bytes: usize| FetchRequest {
            url,
            max_bytes,
            network: network.mode,
            allowed_domains: network.allowed_domains.clone(),
        };
        let index_url = base.join("index.json").map_err(|e| e.to_string())?;
        let index = self.get(request(index_url, INDEX_LIMIT)).await?;
        let skills = parse_index(&index)?;
        let mut total = 0;
        let mut fetched = Vec::new();
        for skill in &skills {
            let mut files = Vec::new();
            let mut size = 0;
            for file in &skill.files {
                let file_url = base
                    .join(&format!("{}/{file}", skill.name))
                    .map_err(|e| e.to_string())?;
                let limit = FILE_LIMIT.min(SKILL_LIMIT - size).min(SOURCE_LIMIT - total);
                let bytes = self.get(request(file_url, limit)).await?;
                size += bytes.len();
                total += bytes.len();
                files.push((file.clone(), bytes));
            }
            fetched.push(files);
        }
        self.store(url, &skills, fetched)
    }

    async fn get(&self, request: FetchRequest) -> Result<Vec<u8>, String> {
        let limit = request.max_bytes;
        let name = request.url.path().to_string();
        let bytes = self.fetcher.fetch(request).await?;
        if bytes.len() > limit {
            return Err(format!("remote skill file {name} exceeds its size limit"));
        }
        Ok(bytes)
    }

    /// Stage every skill into a fresh generation, check each `SKILL.md` parses under its
    /// index name, then point the marker at it. The marker is replaced atomically, so a
    /// failed or interrupted refresh leaves the previous generation in use.
    fn store(
        &self,
        url: &reqwest::Url,
        skills: &[IndexSkill],
        fetched: Vec<Vec<(String, Vec<u8>)>>,
    ) -> Result<(), String> {
        let dir = self.source_dir(url);
        let io = |e: std::io::Error| format!("remote skill cache write failed: {e}");
        let nonce = format!("{}-{}", std::process::id(), unix_nanos());
        let stage = dir.join(format!(".stage-{nonce}"));
        let digest = stage_skills(&stage, skills, fetched)
            .and_then(|()| content_digest(&stage, skills).map_err(io));
        let digest = match digest {
            Ok(digest) => digest,
            Err(error) => {
                let _ = std::fs::remove_dir_all(&stage);
                return Err(error);
            }
        };
        let generation = format!("g{nonce}");
        std::fs::rename(&stage, dir.join(&generation)).map_err(io)?;
        let marker = Marker {
            version: MARKER_VERSION,
            url: url.as_str().to_string(),
            fetched_at: unix_now(),
            digest,
            generation: generation.clone(),
            skills: skills.to_vec(),
        };
        let text = serde_json::to_vec_pretty(&marker).map_err(|e| e.to_string())?;
        cyber_core::trust::write_private_atomic(&self.marker_path(url), &text).map_err(io)?;
        sweep(&dir, &generation);
        Ok(())
    }
}

/// Write each skill's files under `stage/<name>/` and check its `SKILL.md`.
fn stage_skills(
    stage: &Path,
    skills: &[IndexSkill],
    fetched: Vec<Vec<(String, Vec<u8>)>>,
) -> Result<(), String> {
    let io = |e: std::io::Error| format!("remote skill cache write failed: {e}");
    for (skill, files) in skills.iter().zip(fetched) {
        let dir = stage.join(&skill.name);
        for (file, bytes) in files {
            cyber_core::trust::write_private_atomic(&dir.join(&file), &bytes).map_err(io)?;
        }
        let parsed =
            cyber_core::skills::load(&dir).map_err(|e| format!("skill {}: {e}", skill.name))?;
        if parsed.name != skill.name {
            return Err(format!(
                "skill {} declares the name {} in SKILL.md",
                skill.name, parsed.name
            ));
        }
    }
    Ok(())
}

/// Remove this source's older generations and leftover staging directories. A reader that
/// still holds an older generation's path then finds it missing instead of changed.
fn sweep(dir: &Path, current: &str) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten().take(4096) {
        let name = entry.file_name().to_string_lossy().into_owned();
        let leftover = name.starts_with(".stage-") || valid_generation(&name);
        if leftover && name != current && entry.file_type().is_ok_and(|t| t.is_dir()) {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

/// `sha256:` over each skill's name and each listed file's path, length and bytes, as
/// written to disk. Every file must be a regular file within the size limits.
fn content_digest(dir: &Path, skills: &[IndexSkill]) -> std::io::Result<String> {
    let mut hasher = Sha256::new();
    let mut total = 0;
    let mut skills: Vec<&IndexSkill> = skills.iter().collect();
    skills.sort_by(|a, b| a.name.cmp(&b.name));
    for skill in skills {
        hasher.update(skill.name.as_bytes());
        hasher.update([0]);
        let mut files: Vec<&String> = skill.files.iter().collect();
        files.sort();
        for file in files {
            let bytes = read_cached_file(&dir.join(&skill.name).join(file), &mut total)?;
            hasher.update(file.as_bytes());
            hasher.update([0]);
            hasher.update((bytes.len() as u64).to_le_bytes());
            hasher.update(&bytes);
        }
    }
    Ok(format!("sha256:{:x}", hasher.finalize()))
}

/// Read one cached file, refusing links, oversized files and a source over its limit.
fn read_cached_file(path: &Path, total: &mut usize) -> std::io::Result<Vec<u8>> {
    let invalid = || std::io::Error::new(std::io::ErrorKind::InvalidData, "invalid cached file");
    let metadata = std::fs::symlink_metadata(path)?;
    if !metadata.is_file() || metadata.len() > FILE_LIMIT as u64 {
        return Err(invalid());
    }
    let bytes = std::fs::read(path)?;
    *total += bytes.len();
    if bytes.len() > FILE_LIMIT || *total > SOURCE_LIMIT {
        return Err(invalid());
    }
    Ok(bytes)
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn unix_nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests;
