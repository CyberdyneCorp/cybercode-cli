//! Catalog source: pinned file, fresh cache, network refresh or bundled snapshot
//! (`provider-catalog` → models.dev catalog source and cache, Offline and pinned catalog).

use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use cyber_core::env::EnvSource;
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};

use super::CatalogError;

const DEFAULT_URL: &str = "https://models.dev";
const SNAPSHOT: &[u8] = include_bytes!("../../snapshot/models.json.gz");

pub struct SourceOptions {
    pub url: String,
    pub cache_dir: PathBuf,
    /// `CYBER_MODELS_PATH`: read this file and never touch the network.
    pub path: Option<PathBuf>,
    pub allow_fetch: bool,
    /// Ignore cache freshness (`cyber models --refresh`).
    pub refresh: bool,
    pub fresh_for: Duration,
    pub timeout: Duration,
}

impl SourceOptions {
    pub fn from_env(env: &dyn EnvSource, cache_dir: &Path) -> Self {
        Self {
            url: env
                .get("CYBER_MODELS_URL")
                .unwrap_or_else(|| DEFAULT_URL.into()),
            cache_dir: cache_dir.to_path_buf(),
            path: env.get("CYBER_MODELS_PATH").map(PathBuf::from),
            allow_fetch: !env.flag("CYBER_DISABLE_MODELS_FETCH") && !env.flag("CYBER_OFFLINE"),
            refresh: false,
            fresh_for: Duration::from_secs(300),
            timeout: Duration::from_secs(10),
        }
    }

    pub fn cache_file(&self) -> PathBuf {
        let name = if self.url == DEFAULT_URL {
            "models.json".to_string()
        } else {
            let hash = format!("{:x}", Sha256::digest(self.url.as_bytes()));
            format!("models-{}.json", &hash[..12])
        };
        self.cache_dir.join(name)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Origin {
    Path,
    FreshCache,
    Fetched,
    StaleCache,
    Snapshot,
}

pub struct Loaded {
    pub data: Value,
    pub origin: Origin,
    pub warning: Option<String>,
}

pub async fn load(opts: &SourceOptions) -> Result<Loaded, CatalogError> {
    if let Some(path) = &opts.path {
        return Ok(Loaded {
            data: read_json(path)?,
            origin: Origin::Path,
            warning: None,
        });
    }
    let cache = opts.cache_file();
    if !opts.refresh
        && is_fresh(&cache, opts.fresh_for)
        && let Ok(data) = read_json(&cache)
    {
        return Ok(Loaded {
            data,
            origin: Origin::FreshCache,
            warning: None,
        });
    }
    let mut warning = None;
    if opts.allow_fetch {
        match refresh(opts, &cache).await {
            Ok(loaded) => return Ok(loaded),
            Err(e) => warning = Some(format!("model catalog refresh failed: {e}")),
        }
    }
    if let Ok(data) = read_json(&cache) {
        return Ok(Loaded {
            data,
            origin: Origin::StaleCache,
            warning,
        });
    }
    Ok(Loaded {
        data: snapshot()?,
        origin: Origin::Snapshot,
        warning,
    })
}

/// The catalog bundled into the binary.
pub fn snapshot() -> Result<Value, CatalogError> {
    let mut text = String::new();
    flate2::read::GzDecoder::new(SNAPSHOT)
        .read_to_string(&mut text)
        .map_err(|e| CatalogError::Source(format!("bundled snapshot: {e}")))?;
    serde_json::from_str(&text).map_err(|e| CatalogError::Source(format!("bundled snapshot: {e}")))
}

/// Fetch under a cross-process lock; another process may have refreshed while we waited.
async fn refresh(opts: &SourceOptions, cache: &Path) -> Result<Loaded, CatalogError> {
    std::fs::create_dir_all(&opts.cache_dir).map_err(io)?;
    let lock_path = cache.with_extension("lock");
    let _lock = tokio::task::spawn_blocking(move || lock(&lock_path))
        .await
        .map_err(|e| CatalogError::Source(e.to_string()))??;
    if !opts.refresh
        && is_fresh(cache, opts.fresh_for)
        && let Ok(data) = read_json(cache)
    {
        return Ok(Loaded {
            data,
            origin: Origin::FreshCache,
            warning: None,
        });
    }
    let body = fetch(opts).await?;
    let data: Value = serde_json::from_str(&body)
        .map_err(|e| CatalogError::Source(format!("invalid catalog: {e}")))?;
    if !data.is_object() {
        return Err(CatalogError::Source(
            "invalid catalog: expected an object".into(),
        ));
    }
    let tmp = cache.with_extension(format!("tmp-{}", std::process::id()));
    std::fs::write(&tmp, body).map_err(io)?;
    std::fs::rename(&tmp, cache).map_err(io)?;
    Ok(Loaded {
        data,
        origin: Origin::Fetched,
        warning: None,
    })
}

/// GET `<url>/api.json` with a timeout and two retries on transient failures.
async fn fetch(opts: &SourceOptions) -> Result<String, CatalogError> {
    let client = reqwest::Client::builder()
        .timeout(opts.timeout)
        .build()
        .map_err(|e| CatalogError::Source(e.to_string()))?;
    let url = format!("{}/api.json", opts.url.trim_end_matches('/'));
    let mut last = String::new();
    for attempt in 0..3_u32 {
        if attempt > 0 {
            tokio::time::sleep(Duration::from_millis(500 * u64::from(attempt))).await;
        }
        match client.get(&url).send().await {
            Ok(resp) if resp.status().is_success() => {
                return resp
                    .text()
                    .await
                    .map_err(|e| CatalogError::Source(e.to_string()));
            }
            Ok(resp) if resp.status().is_client_error() => {
                return Err(CatalogError::Source(format!(
                    "{url}: HTTP {}",
                    resp.status()
                )));
            }
            Ok(resp) => last = format!("{url}: HTTP {}", resp.status()),
            Err(e) => last = e.to_string(),
        }
    }
    Err(CatalogError::Source(last))
}

fn lock(path: &Path) -> Result<File, CatalogError> {
    let file = File::options()
        .create(true)
        .truncate(false)
        .write(true)
        .open(path)
        .map_err(io)?;
    file.lock().map_err(io)?;
    Ok(file)
}

fn is_fresh(path: &Path, fresh_for: Duration) -> bool {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| SystemTime::now().duration_since(t).ok())
        .is_some_and(|age| age < fresh_for)
}

fn read_json(path: &Path) -> Result<Value, CatalogError> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| CatalogError::Source(format!("{}: {e}", path.display())))?;
    serde_json::from_str(&text)
        .map_err(|e| CatalogError::Source(format!("{}: {e}", path.display())))
}

fn io(e: std::io::Error) -> CatalogError {
    CatalogError::Source(e.to_string())
}
