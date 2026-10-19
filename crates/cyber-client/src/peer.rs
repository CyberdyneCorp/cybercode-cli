//! The transport of `cyber attach <session>@<peer>` (`cross-session-messaging` → Peer
//! attach): one approved attach to one Session of a peer, through its peer listener.
//!
//! The client keeps the TUI's Session routes and maps the ones an attach serves onto
//! `/api/v1/peer/attach/:id/*`: the Session, its transcript, its event stream (the live
//! instance stream becomes the Session's), the prompt, interrupt, and its permission
//! requests and replies. Every other route answers a local [`ROUTE_NOT_FOUND`] without a
//! request, as an older server would, so the TUI leaves those features out. TLS accepts
//! only the remote's pinned key ([`PeerClient`]).

use cyber_server::http::ApiError;
use cyber_server::http::peer::attach::ATTACH_HEADER;
use cyber_server::runtime::messaging::{PeerClient, PeerError, PeerTarget};
use futures::StreamExt;
use serde_json::{Value, json};

use super::{ClientError, ROUTE_NOT_FOUND, decode};

/// An approved attach.
pub(crate) struct PeerAttach {
    client: PeerClient,
    pub(crate) session_id: String,
    attach_id: String,
}

impl PeerAttach {
    /// Ask peer `target` to attach to its Session `session` (an ID or a name it shows
    /// this machine), waiting while its user decides.
    pub(crate) async fn open(target: PeerTarget, session: &str) -> Result<Self, ClientError> {
        let client = PeerClient::long_lived(target).map_err(peer_failure)?;
        let path = format!("/attach/{}", super::encode(session));
        let (status, body) = client
            .request(reqwest::Method::POST, &path, Some(&json!({})))
            .await
            .map_err(peer_failure)?;
        let body = decode(status, &serde_json::to_vec(&body).unwrap_or_default())?;
        let (Some(session_id), Some(attach_id)) =
            (body["session_id"].as_str(), body["attach_id"].as_str())
        else {
            return Err(ClientError::Malformed(
                "the peer approved the attach without an attach ID".into(),
            ));
        };
        Ok(Self {
            session_id: session_id.into(),
            attach_id: attach_id.into(),
            client,
        })
    }

    /// One request on a mapped route: its status and body.
    pub(crate) async fn call(
        &self,
        method: &str,
        path: &str,
        body: Option<&Value>,
    ) -> Result<(u16, Vec<u8>), ClientError> {
        let route = self.route(method, path)?;
        let response = self.send(method, &route, body, &[]).await?;
        let status = response.status().as_u16();
        let bytes = response
            .bytes()
            .await
            .map_err(|e| ClientError::Transport(e.to_string()))?;
        Ok((status, bytes.to_vec()))
    }

    /// The event stream of a mapped stream route, as raw chunks.
    pub(crate) async fn stream(
        &self,
        path: &str,
    ) -> Result<futures::stream::BoxStream<'static, Result<Vec<u8>, String>>, ClientError> {
        let route = self.route("GET", path)?;
        let accept = [("accept", "text/event-stream")];
        let response = self.send("GET", &route, None, &accept).await?;
        let status = response.status().as_u16();
        if !response.status().is_success() {
            let bytes = response.bytes().await.unwrap_or_default();
            return Err(decode(status, &bytes)
                .err()
                .unwrap_or_else(|| ClientError::Malformed("stream refused".into())));
        }
        Ok(response
            .bytes_stream()
            .map(|c| c.map(|b| b.to_vec()).map_err(|e| e.to_string()))
            .boxed())
    }

    /// End the attach on the peer.
    pub(crate) async fn close(&self) -> Result<(), ClientError> {
        let path = format!("/attach/{}", self.session_id);
        let response = self.send("DELETE", &path, None, &[]).await?;
        let status = response.status().as_u16();
        let bytes = response.bytes().await.unwrap_or_default();
        decode(status, &bytes).map(|_| ())
    }

    fn route(&self, method: &str, path: &str) -> Result<String, ClientError> {
        attach_route(&self.session_id, method, path).ok_or_else(|| ClientError::Api {
            status: 404,
            tag: ROUTE_NOT_FOUND.into(),
            message: format!("{method} {path} is not available through a peer attach"),
            body: Value::Null,
        })
    }

    async fn send(
        &self,
        method: &str,
        route: &str,
        body: Option<&Value>,
        headers: &[(&str, &str)],
    ) -> Result<reqwest::Response, ClientError> {
        let method = reqwest::Method::from_bytes(method.as_bytes())
            .map_err(|e| ClientError::Transport(e.to_string()))?;
        let mut all = vec![(ATTACH_HEADER, self.attach_id.as_str())];
        all.extend_from_slice(headers);
        let response = self
            .client
            .send(method, route, body, &all)
            .await
            .map_err(peer_failure)?;
        if let Some(error) = self.failure(&response) {
            return Err(error);
        }
        Ok(response)
    }

    /// The transport failure an answer stands for (`401`, `429`, `5xx`), judged by status
    /// before its body is read.
    fn failure(&self, response: &reqwest::Response) -> Option<ClientError> {
        let status = response.status().as_u16();
        let transport = matches!(status, 401 | 429 | 500..);
        transport
            .then(|| self.client.classify(status, &Value::Null).err())
            .flatten()
            .map(peer_failure)
    }
}

/// The peer route of client route `path` for Session `session`; `None` for a route an
/// attach does not serve.
pub(crate) fn attach_route(session: &str, method: &str, path: &str) -> Option<String> {
    let (route, query) = match path.split_once('?') {
        Some((route, query)) => (route, Some(query)),
        None => (path, None),
    };
    let one = format!("/attach/{session}");
    let with_query = |target: String| match query {
        Some(query) => format!("{target}?{query}"),
        None => target,
    };
    if route == "/event" {
        return (method == "GET").then(|| format!("{one}/events"));
    }
    if route == "/permissions/requests" {
        let own = query.is_some_and(|q| q.split('&').any(|p| p == format!("session_id={session}")));
        return (method == "GET" && own).then(|| format!("{one}/permissions"));
    }
    let rest = route.strip_prefix(&format!("/sessions/{session}"))?;
    match (method, rest) {
        ("GET", "") => Some(format!("{one}/session")),
        ("GET", "/messages" | "/events") => Some(with_query(format!("{one}{rest}"))),
        ("POST", "/prompt" | "/interrupt") => Some(format!("{one}{rest}")),
        ("POST", reply) if is_permission_reply(reply) => Some(format!("{one}{reply}")),
        _ => None,
    }
}

/// `/permissions/<request>/reply`.
fn is_permission_reply(rest: &str) -> bool {
    rest.strip_prefix("/permissions/")
        .and_then(|r| r.strip_suffix("/reply"))
        .is_some_and(|id| !id.is_empty() && !id.contains('/'))
}

/// A peer failure as the server would answer it (`PeerUnreachableError`, `PeerAuthError`,
/// `PeerScopeError`), so callers read its tag.
fn peer_failure(error: PeerError) -> ClientError {
    let error = ApiError::from(error);
    let status = error.status.as_u16();
    let body = serde_json::to_value(&error.body).unwrap_or_default();
    ClientError::Api {
        status,
        tag: body["_tag"].as_str().unwrap_or_default().into(),
        message: body["message"].as_str().unwrap_or_default().into(),
        body,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The TUI's Session routes reach the attach routes; nothing else leaves the machine.
    #[test]
    fn only_the_attached_sessions_routes_are_mapped() {
        let map = |method: &str, path: &str| attach_route("ses_a", method, path);
        let one = "/attach/ses_a";
        assert_eq!(
            map("GET", "/sessions/ses_a").unwrap(),
            format!("{one}/session")
        );
        assert_eq!(
            map("GET", "/sessions/ses_a/messages?order=asc&limit=200").unwrap(),
            format!("{one}/messages?order=asc&limit=200")
        );
        assert_eq!(
            map("GET", "/sessions/ses_a/events?after=4").unwrap(),
            format!("{one}/events?after=4")
        );
        assert_eq!(
            map("GET", "/event?scope=all").unwrap(),
            format!("{one}/events")
        );
        assert_eq!(
            map("POST", "/sessions/ses_a/prompt").unwrap(),
            format!("{one}/prompt")
        );
        assert_eq!(
            map("POST", "/sessions/ses_a/interrupt").unwrap(),
            format!("{one}/interrupt")
        );
        assert_eq!(
            map("GET", "/permissions/requests?session_id=ses_a").unwrap(),
            format!("{one}/permissions")
        );
        assert_eq!(
            map("POST", "/sessions/ses_a/permissions/per_1/reply").unwrap(),
            format!("{one}/permissions/per_1/reply")
        );
        for (method, path) in [
            ("GET", "/sessions/ses_b"),
            ("GET", "/sessions/ses_ab"),
            ("GET", "/sessions?limit=1"),
            ("POST", "/sessions"),
            ("DELETE", "/sessions/ses_a"),
            ("PATCH", "/sessions/ses_a"),
            ("POST", "/sessions/ses_a/shell"),
            ("POST", "/sessions/ses_a/mode"),
            ("GET", "/sessions/ses_a/inbox"),
            ("POST", "/sessions/ses_a/permissions/a/b/reply"),
            ("POST", "/sessions/ses_a/questions/que_1/reply"),
            ("GET", "/permissions/requests?session_id=ses_b"),
            ("GET", "/permissions/requests"),
            ("GET", "/questions/requests?session_id=ses_a"),
            ("GET", "/agents"),
            ("GET", "/config"),
            ("POST", "/event"),
        ] {
            assert_eq!(map(method, path), None, "{method} {path}");
        }
    }

    #[test]
    fn peer_failures_keep_their_tags() {
        let error = peer_failure(PeerError::Unreachable {
            peer: "desk".into(),
            cause: "timed out".into(),
        });
        assert_eq!(error.tag(), Some("PeerUnreachableError"));
        let ClientError::Api { status, body, .. } = error else {
            panic!("api error")
        };
        assert_eq!((status, &body["peer"]), (502, &json!("desk")));
    }
}
