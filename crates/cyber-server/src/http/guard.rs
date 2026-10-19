//! Origin policy, CORS and authentication (`server-api` → Origin and CORS policy, Local
//! password authentication).

use axum::extract::{Request, State};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use base64::Engine;

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use super::envelope::query_value;
use super::error::ApiError;
use super::serve::PeerAddr;
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
    // Peer grant tokens belong to the peer listener only, on every transport.
    if super::peer::guard::presents_peer_token(&req) {
        return basic_challenge();
    }
    if transport != Transport::Embedded
        && super::mcp_serve::is_endpoint(req.uri().path())
        && let Err(error) = local_mcp_client(&req, transport)
    {
        return error.into_response();
    }
    // The PTY socket authenticates with its single-use ticket on every transport.
    if transport == Transport::Tcp
        && !public(req.uri().path())
        && !super::pty::attach_route(req.uri().path())
        && !authorized(&req, state.options.password.as_deref())
    {
        return basic_challenge();
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

/// `401` with the local password's Basic challenge.
fn basic_challenge() -> Response {
    let mut response = ApiError::unauthorized().into_response();
    response.headers_mut().insert(
        header::WWW_AUTHENTICATE,
        HeaderValue::from_static("Basic realm=\"cyber\""),
    );
    response
}

/// MCP serve accepts trusted local clients only (`mcp-server` → Local client trust model):
/// loopback TCP peers or the Unix socket, local credentials (never Bearer), and a
/// loopback `Host` against DNS rebinding.
fn local_mcp_client(req: &Request, transport: Transport) -> Result<(), ApiError> {
    let loopback_only = || {
        ApiError::forbidden("MCP serve accepts only local clients on loopback or the Unix socket")
            .with_reason("loopback_only")
    };
    let peer = req.extensions().get::<PeerAddr>();
    if transport == Transport::Tcp && !peer.is_some_and(|peer| loopback_peer(peer.0.ip())) {
        return Err(loopback_only());
    }
    let foreign = req
        .headers()
        .get(header::AUTHORIZATION)
        .is_some_and(|value| !value.as_bytes().starts_with(b"Basic "));
    if foreign {
        return Err(loopback_only());
    }
    let host = req
        .headers()
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .or_else(|| req.uri().authority().map(|a| a.as_str()));
    if transport == Transport::Tcp && !host.is_some_and(loopback_host) {
        return Err(ApiError::forbidden("MCP serve requires a loopback Host")
            .with_reason("host_not_allowed"));
    }
    Ok(())
}

/// `127.0.0.0/8`, `::1` and their IPv4-mapped forms.
pub fn loopback_peer(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => v4.is_loopback(),
        IpAddr::V6(v6) => {
            v6.is_loopback() || v6.to_ipv4_mapped().is_some_and(|v4| v4.is_loopback())
        }
    }
}

/// `localhost`, a `127.0.0.0/8` address or `[::1]`, each with an optional port.
pub fn loopback_host(host: &str) -> bool {
    let (name_ok, port) = match host.strip_prefix('[').and_then(|rest| rest.split_once(']')) {
        Some((ip, port)) => (
            ip.parse::<Ipv6Addr>().is_ok_and(|ip| ip.is_loopback()),
            port,
        ),
        None if host.starts_with('[') => return false,
        None => {
            let (name, port) = host.find(':').map_or((host, ""), |at| host.split_at(at));
            let name_ok = name.eq_ignore_ascii_case("localhost")
                || name.parse::<Ipv4Addr>().is_ok_and(|ip| ip.is_loopback());
            (name_ok, port)
        }
    };
    let port_ok = port.is_empty()
        || port.strip_prefix(':').is_some_and(|digits| {
            (1..=5).contains(&digits.len()) && digits.bytes().all(|b| b.is_ascii_digit())
        });
    name_ok && port_ok
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
        HeaderValue::from_static("authorization, content-type, idempotency-key, last-event-id, x-cyber-client, x-cyber-directory, x-cyber-workspace, x-cyber-ticket"),
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

    #[test]
    fn loopback_peers_are_recognized() {
        for ip in [
            "127.0.0.1",
            "127.1.2.3",
            "::1",
            "::ffff:127.0.0.1",
            "::ffff:127.9.9.9",
        ] {
            assert!(loopback_peer(ip.parse().unwrap()), "{ip}");
        }
        for ip in [
            "192.168.1.20",
            "10.0.0.5",
            "0.0.0.0",
            "fe80::1",
            "2001:db8::1",
            "::ffff:192.168.1.20",
            "::",
        ] {
            assert!(!loopback_peer(ip.parse().unwrap()), "{ip}");
        }
    }

    #[test]
    fn loopback_hosts_are_recognized() {
        for host in [
            "localhost",
            "LOCALHOST:4747",
            "127.0.0.1",
            "127.0.0.1:4747",
            "127.200.1.1:80",
            "[::1]",
            "[::1]:4747",
        ] {
            assert!(loopback_host(host), "{host}");
        }
        for host in [
            "attacker.example:4747",
            "attacker.example",
            "localhost.attacker.example",
            "192.168.1.20:4747",
            "[fe80::1]:4747",
            "[::1",
            "::1",
            "localhost:",
            "localhost:abc",
            "localhost:4747:1",
            "127.0.0.1:123456",
            "",
        ] {
            assert!(!loopback_host(host), "{host}");
        }
    }
}
