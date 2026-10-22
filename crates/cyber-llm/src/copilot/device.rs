//! GitHub's OAuth device flow: a code request, then polling the token endpoint at the
//! server's `interval` until the user approves, denies, or the code expires. The caller
//! supplies the wait between polls and cancels by dropping the future (Ctrl+C); nothing is
//! stored here either way.
use std::time::{Duration, Instant};

use serde_json::Value;

use super::http::{self, REQUEST_TIMEOUT};
use super::{CopilotError, Urls, join, secure};

const SCOPE: &str = "read:user";
const GRANT_TYPE: &str = "urn:ietf:params:oauth:grant-type:device_code";
/// A code is never polled for longer than this, whatever the server says.
pub const MAX_EXPIRY: u64 = 15 * 60;
const DEFAULT_INTERVAL: u64 = 5;
/// What `slow_down` adds to the interval.
pub const SLOW_DOWN: u64 = 5;
const RESPONSE_LIMIT: usize = 64 * 1024;

/// One device-flow code. `Debug` never shows the device code.
#[derive(Clone, PartialEq, Eq)]
pub struct DeviceCode {
    pub device_code: String,
    /// What the user enters at [`DeviceCode::verification_uri`].
    pub user_code: String,
    pub verification_uri: String,
    /// Seconds between polls.
    pub interval: u64,
    /// Seconds the code stays valid, at most [`MAX_EXPIRY`].
    pub expires_in: u64,
}

impl std::fmt::Debug for DeviceCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeviceCode")
            .field("user_code", &self.user_code)
            .field("verification_uri", &self.verification_uri)
            .field("interval", &self.interval)
            .field("expires_in", &self.expires_in)
            .finish_non_exhaustive()
    }
}

/// Ask `<github>/login/device/code` for a code.
pub async fn request_code(
    client: &reqwest::Client,
    urls: &Urls,
    client_id: &str,
) -> Result<DeviceCode, CopilotError> {
    let request = client
        .post(join(&urls.github, "login/device/code"))
        .header("accept", "application/json")
        .timeout(REQUEST_TIMEOUT)
        .form(&[("client_id", client_id), ("scope", SCOPE)]);
    let (status, document) = http::send(request, "device code", RESPONSE_LIMIT).await?;
    if status != 200 {
        return Err(CopilotError::Status {
            stage: "device code",
            status,
        });
    }
    parse_code(&document)
}

fn parse_code(document: &Value) -> Result<DeviceCode, CopilotError> {
    let field = |name| http::token(document, name).ok_or(CopilotError::Body("device code"));
    let verification_uri = field("verification_uri")?;
    secure(&verification_uri, "verification_uri")?;
    Ok(DeviceCode {
        device_code: field("device_code")?,
        user_code: field("user_code")?,
        verification_uri,
        interval: document["interval"]
            .as_u64()
            .unwrap_or(DEFAULT_INTERVAL)
            .clamp(1, MAX_EXPIRY),
        expires_in: document["expires_in"]
            .as_u64()
            .unwrap_or(MAX_EXPIRY)
            .min(MAX_EXPIRY),
    })
}

/// One poll's answer.
enum Answer {
    Token(String),
    Pending,
    SlowDown(u64),
}

/// Poll `<github>/login/oauth/access_token` until the user approves `code`; the GitHub
/// token. `wait` sleeps between polls (the CLI passes `tokio::time::sleep`). `slow_down`
/// adds [`SLOW_DOWN`] seconds; `access_denied`, `expired_token` and the code's expiry end
/// the flow.
pub async fn poll<W, F>(
    client: &reqwest::Client,
    urls: &Urls,
    client_id: &str,
    code: &DeviceCode,
    mut wait: W,
) -> Result<String, CopilotError>
where
    W: FnMut(Duration) -> F,
    F: Future<Output = ()>,
{
    let started = Instant::now();
    let (mut interval, mut waited) = (code.interval, 0u64);
    loop {
        wait(Duration::from_secs(interval)).await;
        waited = waited.saturating_add(interval);
        if waited > code.expires_in || started.elapsed().as_secs() > code.expires_in {
            return Err(CopilotError::Expired);
        }
        match poll_once(client, urls, client_id, &code.device_code).await? {
            Answer::Token(token) => return Ok(token),
            Answer::Pending => {}
            Answer::SlowDown(asked) => {
                interval = asked.max(interval + SLOW_DOWN).min(MAX_EXPIRY);
            }
        }
    }
}

async fn poll_once(
    client: &reqwest::Client,
    urls: &Urls,
    client_id: &str,
    device_code: &str,
) -> Result<Answer, CopilotError> {
    let form = [
        ("client_id", client_id),
        ("device_code", device_code),
        ("grant_type", GRANT_TYPE),
    ];
    let request = client
        .post(join(&urls.github, "login/oauth/access_token"))
        .header("accept", "application/json")
        .timeout(REQUEST_TIMEOUT)
        .form(&form);
    let (status, document) = http::send(request, "sign-in", RESPONSE_LIMIT).await?;
    if status != 200 {
        return Err(CopilotError::Status {
            stage: "sign-in",
            status,
        });
    }
    answer(&document)
}

fn answer(document: &Value) -> Result<Answer, CopilotError> {
    if let Some(token) = http::token(document, "access_token") {
        return Ok(Answer::Token(token));
    }
    match document["error"].as_str() {
        Some("authorization_pending") => Ok(Answer::Pending),
        Some("slow_down") => Ok(Answer::SlowDown(document["interval"].as_u64().unwrap_or(0))),
        Some("expired_token") => Err(CopilotError::Expired),
        Some("access_denied") => Err(CopilotError::Denied),
        other => Err(CopilotError::Device(error_code(other))),
    }
}

/// The OAuth `error` code, reduced to a safe token for diagnostics.
fn error_code(code: Option<&str>) -> String {
    code.unwrap_or("invalid_response")
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '_')
        .take(64)
        .collect()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn codes_are_bounded_and_need_a_secure_verification_uri() {
        let code = parse_code(&json!({
            "device_code": "dc", "user_code": "ABCD-1234",
            "verification_uri": "https://github.com/login/device",
            "interval": 0, "expires_in": 7200
        }))
        .unwrap();
        assert_eq!((code.interval, code.expires_in), (1, MAX_EXPIRY));
        assert!(!format!("{code:?}").contains("dc\""));
        let insecure = json!({"device_code": "dc", "user_code": "u",
            "verification_uri": "http://evil.example/device"});
        assert!(parse_code(&insecure).is_err());
        let no_code = json!({"user_code": "u", "verification_uri": "https://github.com/x"});
        assert_eq!(parse_code(&no_code), Err(CopilotError::Body("device code")));
    }

    #[test]
    fn unknown_errors_are_sanitized() {
        assert_eq!(error_code(Some("bad_thing\n<x>")), "bad_thingx");
        assert_eq!(error_code(None), "invalid_response");
        assert!(matches!(
            answer(&json!({"error": "unsupported_grant_type"})),
            Err(CopilotError::Device(code)) if code == "unsupported_grant_type"
        ));
    }
}
