//! The network allowlist proxy (`sandbox` → Network isolation and allowlist proxy).
//!
//! Sandboxed processes may reach only this proxy. It serves `CONNECT host:port` tunnels and
//! absolute-form HTTP requests, asking `decide` for each host before connecting upstream.

use std::sync::Arc;

use futures::future::BoxFuture;
use std::path::{Path, PathBuf};

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;

/// Decide whether a host may be reached.
pub type Decide = Arc<dyn Fn(String) -> BoxFuture<'static, bool> + Send + Sync>;

const MAX_HEAD: usize = 64 * 1024;

/// Where sandboxed processes reach the proxy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Endpoint {
    /// Loopback port, for sandboxes that share the host network (macOS).
    Tcp(u16),
    /// Unix socket bridged into a private network namespace (Linux).
    Unix(PathBuf),
}

/// A running proxy; dropping it stops accepting connections.
pub struct Proxy {
    pub endpoint: Endpoint,
    task: JoinHandle<()>,
}

impl Drop for Proxy {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl Proxy {
    /// Listen on a loopback port.
    pub async fn start(decide: Decide) -> std::io::Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let port = listener.local_addr()?.port();
        let task = tokio::spawn(async move {
            while let Ok((client, _)) = listener.accept().await {
                spawn_serve(client, Arc::clone(&decide));
            }
        });
        Ok(Self {
            endpoint: Endpoint::Tcp(port),
            task,
        })
    }

    /// Listen on a Unix socket at `path` (replaced if present).
    #[cfg(unix)]
    pub async fn start_unix(decide: Decide, path: &Path) -> std::io::Result<Self> {
        let _ = std::fs::remove_file(path);
        let listener = tokio::net::UnixListener::bind(path)?;
        let task = tokio::spawn(async move {
            while let Ok((client, _)) = listener.accept().await {
                spawn_serve(client, Arc::clone(&decide));
            }
        });
        Ok(Self {
            endpoint: Endpoint::Unix(path.to_path_buf()),
            task,
        })
    }
}

fn spawn_serve<S: AsyncRead + AsyncWrite + Unpin + Send + 'static>(client: S, decide: Decide) {
    tokio::spawn(async move {
        let _ = serve(client, decide).await;
    });
}

async fn serve<S: AsyncRead + AsyncWrite + Unpin>(
    mut client: S,
    decide: Decide,
) -> std::io::Result<()> {
    let head = read_head(&mut client).await?;
    let text = String::from_utf8_lossy(&head).to_string();
    let request_line = text.lines().next().unwrap_or_default().to_string();
    let mut parts = request_line.split_whitespace();
    let (method, target, version) = (
        parts.next().unwrap_or(""),
        parts.next().unwrap_or(""),
        parts.next().unwrap_or("HTTP/1.1"),
    );
    let Some((host, port, rewritten)) = destination(method, target, version) else {
        return reply(
            &mut client,
            400,
            "Bad Request",
            "Cyber sandbox proxy: unsupported request\n",
        )
        .await;
    };
    if !decide(host.clone()).await {
        let body = format!("Blocked by the Cyber sandbox: {host} is not an allowed domain\n");
        return reply(&mut client, 403, "Forbidden", &body).await;
    }
    let Ok(mut upstream) = TcpStream::connect((host.as_str(), port)).await else {
        return reply(
            &mut client,
            502,
            "Bad Gateway",
            &format!("Cyber sandbox proxy: cannot reach {host}:{port}\n"),
        )
        .await;
    };
    match rewritten {
        None => {
            client
                .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
                .await?
        }
        Some(line) => {
            let rest = head
                .splitn(2, |b| *b == b'\n')
                .nth(1)
                .unwrap_or_default()
                .to_vec();
            upstream.write_all(line.as_bytes()).await?;
            upstream.write_all(&rest).await?;
        }
    }
    tokio::io::copy_bidirectional(&mut client, &mut upstream).await?;
    Ok(())
}

/// `(host, port, rewritten request line)`; the line is `None` for a CONNECT tunnel.
fn destination(method: &str, target: &str, version: &str) -> Option<(String, u16, Option<String>)> {
    if method.eq_ignore_ascii_case("CONNECT") {
        let (host, port) = split_host(target, 443)?;
        return Some((host, port, None));
    }
    let rest = target.strip_prefix("http://")?;
    let (authority, path) = rest
        .split_once('/')
        .map_or((rest, "/".to_string()), |(a, p)| (a, format!("/{p}")));
    let (host, port) = split_host(authority, 80)?;
    Some((host, port, Some(format!("{method} {path} {version}\r\n"))))
}

fn split_host(authority: &str, default: u16) -> Option<(String, u16)> {
    let authority = authority.rsplit_once('@').map_or(authority, |(_, a)| a);
    if let Some(v6) = authority.strip_prefix('[') {
        let (host, rest) = v6.split_once(']')?;
        let port = rest
            .strip_prefix(':')
            .map_or(Some(default), |p| p.parse().ok())?;
        return Some((host.to_string(), port));
    }
    match authority.rsplit_once(':') {
        Some((host, port)) => Some((host.to_string(), port.parse().ok()?)),
        None => Some((authority.to_string(), default)),
    }
    .filter(|(h, _)| !h.is_empty())
}

async fn read_head<S: AsyncRead + Unpin>(stream: &mut S) -> std::io::Result<Vec<u8>> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
        let n = stream.read(&mut chunk).await?;
        if n == 0 || buf.len() > MAX_HEAD {
            return Err(std::io::Error::other("incomplete request head"));
        }
        buf.extend_from_slice(&chunk[..n]);
    }
    Ok(buf)
}

async fn reply<S: AsyncWrite + Unpin>(
    client: &mut S,
    status: u16,
    reason: &str,
    body: &str,
) -> std::io::Result<()> {
    let text = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    client.write_all(text.as_bytes()).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn destinations_parse_connect_and_absolute_form() {
        assert_eq!(
            destination("CONNECT", "crates.io:443", "HTTP/1.1"),
            Some(("crates.io".into(), 443, None))
        );
        assert_eq!(
            destination("GET", "http://example.com:8080/a?b", "HTTP/1.1"),
            Some((
                "example.com".into(),
                8080,
                Some("GET /a?b HTTP/1.1\r\n".into())
            ))
        );
        assert_eq!(destination("GET", "/relative", "HTTP/1.1"), None);
        assert_eq!(
            destination("CONNECT", "[::1]:8443", "HTTP/1.1"),
            Some(("::1".into(), 8443, None))
        );
    }
}
