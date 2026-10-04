//! A Rust client for the `/api/v1` HTTP API, over TCP or the in-process embedded transport.
//! Used by `cyber exec` and the TUI; every call goes through the public routes.

use std::time::Duration;

use axum::body::Body;
use cyber_llm::SseParser;
use cyber_server::http::EmbeddedClient;
use futures::{Stream, StreamExt};
use serde_json::Value;
use tokio::sync::mpsc;

#[derive(Debug, Clone, thiserror::Error)]
pub enum ClientError {
    /// A declared server error.
    #[error("{tag}: {message}")]
    Api {
        status: u16,
        tag: String,
        message: String,
        body: Value,
    },
    #[error("transport: {0}")]
    Transport(String),
    #[error("malformed response: {0}")]
    Malformed(String),
}

impl ClientError {
    pub fn tag(&self) -> Option<&str> {
        match self {
            Self::Api { tag, .. } => Some(tag),
            _ => None,
        }
    }
}

#[derive(Clone)]
enum Transport {
    Http {
        base: String,
        password: Option<String>,
        http: reqwest::Client,
    },
    Embedded(EmbeddedClient),
}

#[derive(Clone)]
pub struct Client {
    transport: Transport,
    directory: Option<String>,
}

/// One event from a stream.
#[derive(Debug, Clone)]
pub struct Event {
    pub kind: String,
    pub data: Value,
    /// For durable events: the aggregate sequence.
    pub seq: Option<i64>,
    pub session_id: Option<String>,
}

impl Client {
    /// A client for `base` (`http://127.0.0.1:4747`) with the local password.
    pub fn http(base: &str, password: Option<String>) -> Self {
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(5))
            .build()
            .unwrap_or_default();
        Self {
            transport: Transport::Http {
                base: base.trim_end_matches('/').into(),
                password,
                http,
            },
            directory: None,
        }
    }

    pub fn embedded(client: EmbeddedClient) -> Self {
        Self {
            transport: Transport::Embedded(client),
            directory: None,
        }
    }

    /// A client that sends `directory` as the Location on every request.
    pub fn at(&self, directory: &str) -> Self {
        Self {
            transport: self.transport.clone(),
            directory: Some(directory.into()),
        }
    }

    pub async fn get(&self, path: &str) -> Result<Value, ClientError> {
        self.request("GET", path, None).await
    }

    pub async fn post(&self, path: &str, body: Value) -> Result<Value, ClientError> {
        self.request("POST", path, Some(body)).await
    }

    pub async fn patch(&self, path: &str, body: Value) -> Result<Value, ClientError> {
        self.request("PATCH", path, Some(body)).await
    }

    pub async fn delete(&self, path: &str) -> Result<Value, ClientError> {
        self.request("DELETE", path, None).await
    }

    /// Send one request to `/api/v1{path}`. Mutations carry a fresh `Idempotency-Key` and
    /// are retried up to 3 times on transport failure with the same key.
    pub async fn request(
        &self,
        method: &str,
        path: &str,
        body: Option<Value>,
    ) -> Result<Value, ClientError> {
        let key = (method != "GET").then(|| ulid::Ulid::new().to_string());
        let mut attempt = 0;
        loop {
            match self.once(method, path, body.as_ref(), key.as_deref()).await {
                Err(ClientError::Transport(_)) if attempt < 3 && key.is_some() => {
                    attempt += 1;
                    tokio::time::sleep(Duration::from_millis(200 * attempt)).await;
                }
                other => return other,
            }
        }
    }

    async fn once(
        &self,
        method: &str,
        path: &str,
        body: Option<&Value>,
        key: Option<&str>,
    ) -> Result<Value, ClientError> {
        let (status, bytes) = match &self.transport {
            Transport::Http {
                base,
                password,
                http,
            } => {
                let method = reqwest::Method::from_bytes(method.as_bytes())
                    .map_err(|e| ClientError::Transport(e.to_string()))?;
                let mut req = http.request(method, format!("{base}/api/v1{path}"));
                if let Some(pw) = password {
                    req = req.basic_auth("cyber", Some(pw));
                }
                req = self.headers(req, key);
                if let Some(body) = body {
                    req = req.json(body);
                }
                let res = req
                    .send()
                    .await
                    .map_err(|e| ClientError::Transport(e.to_string()))?;
                let status = res.status().as_u16();
                (
                    status,
                    res.bytes()
                        .await
                        .map_err(|e| ClientError::Transport(e.to_string()))?
                        .to_vec(),
                )
            }
            Transport::Embedded(client) => {
                let req = self.embedded_request(method, path, body, key)?;
                let res = client.request(req).await;
                let status = res.status().as_u16();
                let bytes = http_body_util::BodyExt::collect(res.into_body())
                    .await
                    .map_err(|e| ClientError::Transport(e.to_string()))?
                    .to_bytes();
                (status, bytes.to_vec())
            }
        };
        decode(status, &bytes)
    }

    fn headers(
        &self,
        mut req: reqwest::RequestBuilder,
        key: Option<&str>,
    ) -> reqwest::RequestBuilder {
        if let Some(dir) = &self.directory {
            req = req.header("x-cyber-directory", encode(dir));
        }
        if let Some(key) = key {
            req = req.header("idempotency-key", key);
        }
        req
    }

    fn embedded_request(
        &self,
        method: &str,
        path: &str,
        body: Option<&Value>,
        key: Option<&str>,
    ) -> Result<axum::http::Request<Body>, ClientError> {
        let mut req = axum::http::Request::builder()
            .method(method)
            .uri(format!("http://cyber.internal/api/v1{path}"));
        if let Some(dir) = &self.directory {
            req = req.header("x-cyber-directory", encode(dir));
        }
        if let Some(key) = key {
            req = req.header("idempotency-key", key);
        }
        let body = match body {
            Some(b) => {
                req = req.header("content-type", "application/json");
                Body::from(b.to_string())
            }
            None => Body::empty(),
        };
        req.body(body)
            .map_err(|e| ClientError::Transport(e.to_string()))
    }

    /// Open one SSE stream as raw byte chunks.
    async fn open_stream(
        &self,
        path: &str,
    ) -> Result<futures::stream::BoxStream<'static, Result<Vec<u8>, String>>, ClientError> {
        match &self.transport {
            Transport::Http {
                base,
                password,
                http,
            } => {
                let mut req = http
                    .get(format!("{base}/api/v1{path}"))
                    .header("accept", "text/event-stream");
                if let Some(pw) = password {
                    req = req.basic_auth("cyber", Some(pw));
                }
                req = self.headers(req, None);
                let res = req
                    .send()
                    .await
                    .map_err(|e| ClientError::Transport(e.to_string()))?;
                if !res.status().is_success() {
                    let status = res.status().as_u16();
                    let bytes = res.bytes().await.unwrap_or_default();
                    return Err(decode(status, &bytes)
                        .err()
                        .unwrap_or_else(|| ClientError::Malformed("stream refused".into())));
                }
                Ok(res
                    .bytes_stream()
                    .map(|c| c.map(|b| b.to_vec()).map_err(|e| e.to_string()))
                    .boxed())
            }
            Transport::Embedded(client) => {
                let res = client
                    .request(self.embedded_request("GET", path, None, None)?)
                    .await;
                if !res.status().is_success() {
                    let status = res.status().as_u16();
                    let bytes = http_body_util::BodyExt::collect(res.into_body())
                        .await
                        .map(|b| b.to_bytes())
                        .unwrap_or_default();
                    return Err(decode(status, &bytes)
                        .err()
                        .unwrap_or_else(|| ClientError::Malformed("stream refused".into())));
                }
                Ok(res
                    .into_body()
                    .into_data_stream()
                    .map(|c| c.map(|b| b.to_vec()).map_err(|e| e.to_string()))
                    .boxed())
            }
        }
    }

    /// Live events for this client's Location (`/event`), or every Location with `all`.
    pub async fn events(
        &self,
        all: bool,
    ) -> Result<impl Stream<Item = Event> + use<>, ClientError> {
        let chunks = self
            .open_stream(if all { "/event?scope=all" } else { "/event" })
            .await?;
        Ok(parse_events(chunks))
    }

    /// One request with extra headers, returning the status and body even on failure
    /// (`cyber api`).
    pub async fn raw(
        &self,
        method: &str,
        path: &str,
        body: Option<Value>,
        headers: &[(String, String)],
    ) -> Result<(u16, Value), ClientError> {
        let Transport::Http {
            base,
            password,
            http,
        } = &self.transport
        else {
            return self.request(method, path, body).await.map(|v| (200, v));
        };
        let m = reqwest::Method::from_bytes(method.as_bytes())
            .map_err(|e| ClientError::Transport(e.to_string()))?;
        let mut req = http.request(m, format!("{base}/api/v1{path}"));
        if let Some(pw) = password {
            req = req.basic_auth("cyber", Some(pw));
        }
        req = self.headers(req, None);
        for (k, v) in headers {
            req = req.header(k.as_str(), v.as_str());
        }
        if let Some(body) = body {
            req = req.json(&body);
        }
        let res = req
            .send()
            .await
            .map_err(|e| ClientError::Transport(e.to_string()))?;
        let status = res.status().as_u16();
        let bytes = res
            .bytes()
            .await
            .map_err(|e| ClientError::Transport(e.to_string()))?;
        let value = serde_json::from_slice(&bytes)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into_owned()));
        Ok((status, value))
    }

    /// Durable events of one Session after `after`, reconnecting with backoff (500 ms to
    /// 15 s) and resuming from the last sequence seen, so nothing is lost or repeated.
    pub fn session_events(
        &self,
        session_id: &str,
        after: i64,
    ) -> impl Stream<Item = Event> + use<> {
        let (tx, rx) = mpsc::channel(256);
        let (client, id) = (self.clone(), session_id.to_string());
        tokio::spawn(async move {
            let mut last = after;
            let mut backoff = Duration::from_millis(500);
            loop {
                if let Ok(chunks) = client
                    .open_stream(&format!("/sessions/{id}/events?after={last}"))
                    .await
                {
                    backoff = Duration::from_millis(500);
                    let mut events = Box::pin(parse_events(chunks));
                    while let Some(event) = events.next().await {
                        if event.seq.is_some_and(|s| s <= last) {
                            continue;
                        }
                        last = event.seq.unwrap_or(last);
                        if tx.send(event).await.is_err() {
                            return;
                        }
                    }
                }
                if tx.is_closed() {
                    return;
                }
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(Duration::from_secs(15));
            }
        });
        tokio_stream(rx)
    }
}

fn tokio_stream(mut rx: mpsc::Receiver<Event>) -> impl Stream<Item = Event> {
    futures::stream::poll_fn(move |cx| rx.poll_recv(cx))
}

fn parse_events(
    chunks: futures::stream::BoxStream<'static, Result<Vec<u8>, String>>,
) -> impl Stream<Item = Event> {
    let mut parser = SseParser::default();
    chunks
        .take_while(|c| futures::future::ready(c.is_ok()))
        .flat_map(move |chunk| futures::stream::iter(parser.push(&chunk.unwrap_or_default())))
        .filter_map(|e| futures::future::ready(serde_json::from_str::<Value>(&e.data).ok()))
        .map(|v| Event {
            kind: v["type"].as_str().unwrap_or_default().to_string(),
            seq: v["durable"]["seq"].as_i64(),
            session_id: v["durable"]["aggregateID"]
                .as_str()
                .or_else(|| v["data"]["session_id"].as_str())
                .map(str::to_string),
            data: v["data"].clone(),
        })
}

fn decode(status: u16, bytes: &[u8]) -> Result<Value, ClientError> {
    let value: Value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(bytes)
            .map_err(|e| ClientError::Malformed(format!("status {status}: {e}")))?
    };
    if (200..300).contains(&status) {
        return Ok(value);
    }
    match value["_tag"].as_str() {
        Some(tag) => Err(ClientError::Api {
            status,
            tag: tag.into(),
            message: value["message"].as_str().unwrap_or_default().into(),
            body: value,
        }),
        None => Err(ClientError::Malformed(format!(
            "unexpected status {status}"
        ))),
    }
}

/// Percent-encode a header value (the server URI-decodes `x-cyber-directory`).
fn encode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'/' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn responses_decode_to_values_or_tagged_errors() {
        assert_eq!(decode(204, b"").unwrap(), Value::Null);
        assert_eq!(decode(200, br#"{"data":1}"#).unwrap()["data"], 1);
        let err = decode(404, br#"{"_tag":"SessionNotFoundError","message":"no"}"#).unwrap_err();
        assert_eq!(err.tag(), Some("SessionNotFoundError"));
        assert!(matches!(
            decode(502, b"<html>"),
            Err(ClientError::Malformed(_))
        ));
        assert!(matches!(decode(500, b""), Err(ClientError::Malformed(_))));
    }

    #[test]
    fn directories_are_percent_encoded_for_headers() {
        assert_eq!(encode("/tmp/a b/ç"), "/tmp/a%20b/%C3%A7");
    }
}
