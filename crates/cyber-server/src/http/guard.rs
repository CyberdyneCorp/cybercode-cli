//! Origin policy, CORS and authentication (`server-api` → Origin and CORS policy, Local
//! password authentication).

use axum::extract::{Request, State};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use base64::Engine;

use super::envelope::query_value;
use super::error::ApiError;
use super::{AppState, Transport};

pub async fn layer(State(state): State<AppState>, req: Request, next: Next) -> Response {
    let transport = req
        .extensions()
        .get::<Transport>()
        .copied()
        .unwrap_or(Transport::Tcp);
    let origin = req
        .headers()
        .get(header::ORIGIN)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    if let Some(origin) = &origin {
        if !allowed_origin(origin, req.headers(), &state.options.cors_origins) {
            return ApiError::forbidden(format!("origin {origin} is not allowed")).into_response();
        }
        if req.method() == Method::OPTIONS {
            return preflight(origin);
        }
    }
    if transport == Transport::Tcp
        && !public(req.uri().path())
        && !authorized(&req, state.options.password.as_deref())
    {
        let mut response = ApiError::unauthorized().into_response();
        response.headers_mut().insert(
            header::WWW_AUTHENTICATE,
            HeaderValue::from_static("Basic realm=\"cyber\""),
        );
        return response;
    }
    let mut response = next.run(req).await;
    if let Some(origin) = origin.and_then(|o| HeaderValue::from_str(&o).ok()) {
        response
            .headers_mut()
            .insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, origin);
        response
            .headers_mut()
            .insert(header::VARY, HeaderValue::from_static("Origin"));
    }
    response
}

fn public(path: &str) -> bool {
    path.ends_with("/health")
}

/// Routes browsers open with `EventSource` or WebSocket, which cannot set headers.
fn token_route(path: &str) -> bool {
    path.ends_with("/event")
        || (path.contains("/sessions/") && path.ends_with("/events"))
        || path.ends_with("/connect")
        || path.ends_with("/ws")
}

fn authorized(req: &Request, password: Option<&str>) -> bool {
    let Some(password) = password else {
        return true;
    };
    let expected = base64::engine::general_purpose::STANDARD.encode(format!("cyber:{password}"));
    let basic = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Basic "))
        .is_some_and(|v| constant_eq(v.trim(), &expected));
    let token = token_route(req.uri().path())
        && req
            .uri()
            .query()
            .and_then(|q| query_value(q, "auth_token"))
            .is_some_and(|t| constant_eq(&t, &expected));
    basic || token
}

fn constant_eq(a: &str, b: &str) -> bool {
    a.len() == b.len()
        && a.bytes()
            .zip(b.bytes())
            .fold(0u8, |acc, (x, y)| acc | (x ^ y))
            == 0
}

pub fn allowed_origin(origin: &str, headers: &HeaderMap, extra: &[String]) -> bool {
    let host = headers
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    let same_host = origin
        .split_once("://")
        .is_some_and(|(_, rest)| !host.is_empty() && rest == host);
    same_host
        || origin.starts_with("http://localhost:")
        || origin.starts_with("http://127.0.0.1:")
        || origin == "tauri://localhost"
        || extra.iter().any(|o| o == origin)
}

fn preflight(origin: &str) -> Response {
    let mut response = StatusCode::NO_CONTENT.into_response();
    let headers = response.headers_mut();
    if let Ok(origin) = HeaderValue::from_str(origin) {
        headers.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, origin);
    }
    headers.insert(
        header::ACCESS_CONTROL_ALLOW_METHODS,
        HeaderValue::from_static("GET, POST, PATCH, PUT, DELETE, OPTIONS"),
    );
    headers.insert(
        header::ACCESS_CONTROL_ALLOW_HEADERS,
        HeaderValue::from_static("authorization, content-type, idempotency-key, last-event-id, x-cyber-directory, x-cyber-workspace, x-cyber-ticket"),
    );
    headers.insert(
        header::ACCESS_CONTROL_MAX_AGE,
        HeaderValue::from_static("86400"),
    );
    headers.insert(header::VARY, HeaderValue::from_static("Origin"));
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn origins_follow_the_policy() {
        let mut headers = HeaderMap::new();
        headers.insert(header::HOST, HeaderValue::from_static("127.0.0.1:4747"));
        let extra = vec!["https://app.example".to_string()];
        assert!(allowed_origin("http://127.0.0.1:4747", &headers, &[]));
        assert!(allowed_origin("http://localhost:3000", &headers, &[]));
        assert!(allowed_origin("tauri://localhost", &headers, &[]));
        assert!(allowed_origin("https://app.example", &headers, &extra));
        assert!(!allowed_origin("https://evil.example", &headers, &extra));
        assert!(!allowed_origin(
            "http://localhost.evil.example",
            &headers,
            &[]
        ));
    }
}
