//! Release downloads (`installation-upgrade` → Self-update integrity): small assets read
//! into memory, archives streamed to a temporary file beside the binary they replace.
//! Both are capped, and a body shorter than its `Content-Length` is an error.

use std::io::Write;
use std::path::Path;
use std::time::Duration;

use crate::manifest::user_agent;

/// No release archive is anywhere near this; a larger body is refused.
pub const MAX_ARCHIVE_BYTES: u64 = 512 * 1024 * 1024;

/// `SHA256SUMS` and Sigstore bundles are a few kilobytes.
pub const MAX_ASSET_BYTES: u64 = 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum FetchError {
    #[error("download failed: {url}: {reason}")]
    Http { url: String, reason: String },
    #[error("download failed: {url}: larger than {max} bytes")]
    TooLarge { url: String, max: u64 },
    #[error("download failed: {url}: {source}")]
    Io {
        url: String,
        #[source]
        source: std::io::Error,
    },
}

/// One HTTP client for a release download: a connect timeout and a per-read timeout, so
/// a slow but live transfer of a large archive is not cut off.
pub struct Fetcher {
    client: reqwest::Client,
    pub max_archive_bytes: u64,
}

impl Fetcher {
    pub fn new(timeout: Duration) -> Result<Self, FetchError> {
        let client = reqwest::Client::builder()
            .connect_timeout(timeout)
            .read_timeout(timeout)
            .user_agent(user_agent("cli"))
            .build()
            .map_err(|e| FetchError::Http {
                url: String::new(),
                reason: e.to_string(),
            })?;
        Ok(Self {
            client,
            max_archive_bytes: MAX_ARCHIVE_BYTES,
        })
    }

    /// A small asset, in memory.
    pub async fn bytes(&self, url: &str) -> Result<Vec<u8>, FetchError> {
        let mut body = Vec::new();
        self.stream(url, MAX_ASSET_BYTES, &mut body).await?;
        Ok(body)
    }

    /// An archive, streamed to a hidden temporary file in `dir`; it is deleted when the
    /// returned path drops.
    pub async fn to_file(&self, url: &str, dir: &Path) -> Result<tempfile::TempPath, FetchError> {
        let io = |source| FetchError::Io {
            url: url.to_string(),
            source,
        };
        let mut file = tempfile::Builder::new()
            .prefix(".cyber-upgrade-")
            .suffix(".tmp")
            .tempfile_in(dir)
            .map_err(io)?;
        self.stream(url, self.max_archive_bytes, file.as_file_mut())
            .await?;
        file.as_file().sync_all().map_err(io)?;
        Ok(file.into_temp_path())
    }

    async fn stream(&self, url: &str, max: u64, out: &mut dyn Write) -> Result<(), FetchError> {
        let http = |reason: String| FetchError::Http {
            url: url.to_string(),
            reason,
        };
        let too_large = || FetchError::TooLarge {
            url: url.to_string(),
            max,
        };
        let mut response = self
            .client
            .get(url)
            .send()
            .await
            .map_err(|e| http(e.to_string()))?;
        let status = response.status();
        if !status.is_success() {
            return Err(http(format!("HTTP {status}")));
        }
        let declared = response.content_length();
        if declared.is_some_and(|len| len > max) {
            return Err(too_large());
        }
        let mut received: u64 = 0;
        while let Some(chunk) = response.chunk().await.map_err(|e| http(e.to_string()))? {
            received += chunk.len() as u64;
            if received > max {
                return Err(too_large());
            }
            out.write_all(&chunk).map_err(|source| FetchError::Io {
                url: url.to_string(),
                source,
            })?;
        }
        match declared {
            Some(len) if len != received => Err(http(format!(
                "truncated: received {received} of {len} bytes"
            ))),
            _ => Ok(()),
        }
    }
}
