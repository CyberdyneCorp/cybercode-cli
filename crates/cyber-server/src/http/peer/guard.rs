//! The peer guard (`cross-session-messaging` → Peer pairing and grants): every peer route
//! needs `Authorization: Bearer cyp_…` of an active grant, and nothing else is accepted
//! here (not the local password, not Unix-socket credentials). Unknown, revoked or expired
//! tokens get `401` with no detail; each grant may make 120 requests a minute (`429`).
//! A remote address whose tokens failed [`FAILED_RATE`] times in a minute gets `429`
//! before any token lookup until the minute passes, so unauthenticated requests cannot
//! load the store without bound. Route scopes are checked by the routes through
//! [`require`].

use std::collections::{HashMap, VecDeque};
use std::net::IpAddr;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use axum::extract::{Request, State};
use axum::http::{HeaderValue, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use super::PeerState;
use crate::http::error::ApiError;
use crate::runtime::RuntimeError;
use crate::runtime::messaging::{PeerError, PeerScope};
use crate::runtime::peers::{Grant, grants};

/// Requests one grant may make per [`WINDOW`].
pub const GRANT_RATE: usize = 120;
/// Tokens that fail verification one remote address may present per [`WINDOW`].
pub const FAILED_RATE: usize = 30;
const WINDOW: Duration = Duration::from_secs(60);

/// Authenticate a peer request and put its [`Grant`] in the request extensions.
pub async fn authenticate(State(peer): State<PeerState>, mut req: Request, next: Next) -> Response {
    let Some(token) = bearer(&req).map(str::to_string) else {
        return unauthorized();
    };
    let from = remote_ip(&req);
    if let Err(retry_after) = peer.failures.check(&from, FAILED_RATE, Instant::now()) {
        return ApiError::rate_limited(
            "peer_auth",
            retry_after.as_millis() as u64,
            "RateLimitedError: too many invalid peer tokens from this address",
        )
        .into_response();
    }
    let store = Arc::clone(&peer.app.store);
    let now = now_ms();
    let verified = tokio::task::spawn_blocking(move || grants::verify(&store, &token, now)).await;
    let grant = match verified {
        Ok(Ok(Some(grant))) => grant,
        Ok(Ok(None)) => {
            peer.failures.count(&from, Instant::now());
            return unauthorized();
        }
        Ok(Err(error)) => return ApiError::from(RuntimeError::Store(error)).into_response(),
        Err(error) => return ApiError::unknown(error).into_response(),
    };
    if let Err(retry_after) = peer.limiter.admit(&grant.id, Instant::now()) {
        return ApiError::rate_limited(
            "peer_grant",
            retry_after.as_millis() as u64,
            format!(
                "RateLimitedError: grant `{}` may make {GRANT_RATE} requests a minute",
                grant.name
            ),
        )
        .into_response();
    }
    req.extensions_mut().insert(grant);
    next.run(req).await
}

/// The remote IP of a request (`unknown` without a TCP peer address), the key failed
/// tokens are counted by.
fn remote_ip(req: &Request) -> String {
    req.extensions()
        .get::<crate::http::PeerAddr>()
        .map(|addr| addr.0.ip())
        .map_or_else(|| "unknown".into(), |ip: IpAddr| ip.to_string())
}

/// `403 PeerScopeError` unless `grant` includes `scope`.
pub fn require(grant: &Grant, scope: PeerScope) -> Result<(), ApiError> {
    if grant.has(scope) {
        Ok(())
    } else {
        Err(PeerError::Scope { scope }.into())
    }
}

/// The token of `Authorization: Bearer cyp_…`, if that is what the request carries.
fn bearer(req: &Request) -> Option<&str> {
    let value = req.headers().get(header::AUTHORIZATION)?.to_str().ok()?;
    let (scheme, token) = value.split_once(' ')?;
    let token = token.trim();
    (scheme.eq_ignore_ascii_case("bearer") && token.starts_with(grants::TOKEN_PREFIX))
        .then_some(token)
}

/// Whether a request presents a peer token: the main API refuses them.
pub fn presents_peer_token(req: &Request) -> bool {
    let header = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split_once(' '))
        .is_some_and(|(_, token)| token.trim().starts_with(grants::TOKEN_PREFIX));
    let query = req
        .uri()
        .query()
        .and_then(|query| crate::http::envelope::query_value(query, "auth_token"))
        .is_some_and(|token| token.starts_with(grants::TOKEN_PREFIX));
    header || query
}

/// `401` with no detail and a Bearer challenge.
pub fn unauthorized() -> Response {
    let mut response = ApiError::unauthorized().into_response();
    response.headers_mut().insert(
        header::WWW_AUTHENTICATE,
        HeaderValue::from_static("Bearer realm=\"cyber-peer\""),
    );
    response
}

/// A sliding one-minute window of request times per grant.
#[derive(Default)]
pub struct GrantLimiter {
    windows: Mutex<HashMap<String, VecDeque<Instant>>>,
}

impl GrantLimiter {
    /// Count a request at `now`, or say how long until the grant may make the next one.
    fn admit(&self, grant: &str, now: Instant) -> Result<(), Duration> {
        let mut windows = self.windows.lock().unwrap_or_else(PoisonError::into_inner);
        let window = windows.entry(grant.to_string()).or_default();
        slide(window, now);
        if let Some(first) = window.front().filter(|_| window.len() >= GRANT_RATE) {
            return Err(WINDOW.saturating_sub(now.duration_since(*first)));
        }
        window.push_back(now);
        Ok(())
    }

    /// `Err` with the time until `key` may make the next request when it already made
    /// `rate` in the last minute.
    fn check(&self, key: &str, rate: usize, now: Instant) -> Result<(), Duration> {
        let mut windows = self.windows.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(window) = windows.get_mut(key) else {
            return Ok(());
        };
        slide(window, now);
        match window.front().filter(|_| window.len() >= rate) {
            Some(first) => Err(WINDOW.saturating_sub(now.duration_since(*first))),
            None => Ok(()),
        }
    }

    /// Count one request of `key` at `now`; windows left empty are forgotten.
    fn count(&self, key: &str, now: Instant) {
        let mut windows = self.windows.lock().unwrap_or_else(PoisonError::into_inner);
        windows.retain(|_, window| {
            slide(window, now);
            !window.is_empty()
        });
        windows.entry(key.to_string()).or_default().push_back(now);
    }
}

/// Drop the times that left the window ending at `now`.
fn slide(window: &mut VecDeque<Instant>, now: Instant) {
    while window
        .front()
        .is_some_and(|first| now.duration_since(*first) >= WINDOW)
    {
        window.pop_front();
    }
}

pub(crate) fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_grant_gets_120_requests_a_minute() {
        let limiter = GrantLimiter::default();
        let start = Instant::now();
        for i in 0..GRANT_RATE {
            let at = start + Duration::from_millis(i as u64 * 100);
            assert!(limiter.admit("pgr_a", at).is_ok(), "{i}");
        }
        let full = start + Duration::from_secs(20);
        let retry = limiter.admit("pgr_a", full).unwrap_err();
        assert_eq!(retry, Duration::from_secs(40));
        // Other grants have their own window.
        assert!(limiter.admit("pgr_b", full).is_ok());
        // The oldest request leaves the window after a minute.
        assert!(limiter.admit("pgr_a", start + WINDOW).is_ok());
        assert!(limiter.admit("pgr_a", start + WINDOW).is_err());
    }

    /// Regression: unauthenticated requests had no throttle, and each one looked its
    /// token up in the store. An address with [`FAILED_RATE`] failures is refused first.
    #[test]
    fn failed_tokens_are_limited_per_address() {
        let failures = GrantLimiter::default();
        let start = Instant::now();
        for _ in 0..FAILED_RATE {
            assert!(failures.check("10.0.0.9", FAILED_RATE, start).is_ok());
            failures.count("10.0.0.9", start);
        }
        assert!(failures.check("10.0.0.9", FAILED_RATE, start).is_err());
        assert!(failures.check("10.0.0.8", FAILED_RATE, start).is_ok());
        assert!(
            failures
                .check("10.0.0.9", FAILED_RATE, start + WINDOW)
                .is_ok()
        );
        // A forgotten window costs nothing: the next count drops it.
        failures.count("10.0.0.8", start + WINDOW);
        let windows = failures.windows.lock().unwrap();
        assert_eq!(windows.keys().collect::<Vec<_>>(), ["10.0.0.8"]);
    }

    #[test]
    fn only_bearer_peer_tokens_are_read() {
        let request = |value: &str| {
            Request::builder()
                .header(header::AUTHORIZATION, value)
                .body(axum::body::Body::empty())
                .unwrap()
        };
        assert_eq!(bearer(&request("Bearer cyp_pgrX_s")), Some("cyp_pgrX_s"));
        assert_eq!(bearer(&request("bearer  cyp_pgrX_s ")), Some("cyp_pgrX_s"));
        for value in [
            "Basic Y3liZXI6cA==",
            "Bearer sk-abc",
            "cyp_pgrX_s",
            "Basic cyp_x",
        ] {
            assert_eq!(bearer(&request(value)), None, "{value}");
        }
        assert!(presents_peer_token(&request("Bearer cyp_pgrX_s")));
        assert!(presents_peer_token(&request("Basic cyp_pgrX_s")));
        assert!(!presents_peer_token(&request("Basic Y3liZXI6cA==")));
        let query = Request::builder()
            .uri("/api/v1/event?auth_token=cyp_pgrX_s")
            .body(axum::body::Body::empty())
            .unwrap();
        assert!(presents_peer_token(&query));
    }
}
