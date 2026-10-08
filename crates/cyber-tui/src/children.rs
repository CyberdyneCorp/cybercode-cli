//! Child-thread browsing keeps request and response parent identities together.

use std::collections::HashSet;

use cyber_client::Client;
use serde::Deserialize;

use crate::{
    model::{Choice, Session},
    perform::err,
};

#[derive(Deserialize)]
struct Envelope {
    data: Page,
}
#[derive(Deserialize)]
struct Page {
    parent_id: String,
    data: Vec<Thread>,
    cursor: Cursor,
}
#[derive(Deserialize)]
struct Cursor {
    next: Option<String>,
}
#[derive(Deserialize)]
struct Thread {
    session: Identity,
    status: Status,
    reason: Option<String>,
}
#[derive(Deserialize)]
struct Identity {
    id: String,
    parent_id: String,
    subagent_name: Option<String>,
    title: String,
    directory: String,
    agent: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum Status {
    Running,
    Waiting,
    Completed,
    Failed,
}
impl Status {
    fn label(&self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Waiting => "waiting",
            Self::Completed => "completed",
            Self::Failed => "failed",
        }
    }
}

fn encode(value: &str) -> String {
    value
        .bytes()
        .map(|byte| {
            if byte.is_ascii_alphanumeric() || b"-_.~".contains(&byte) {
                (byte as char).to_string()
            } else {
                format!("%{byte:02X}")
            }
        })
        .collect()
}

pub(crate) async fn load(client: &Client, session: &Session) -> Result<Vec<Choice>, String> {
    let mut items = Vec::new();
    if let Some(parent) = &session.parent_id {
        items.push(Choice {
            key: parent.clone(),
            label: "↑ Parent thread".into(),
            detail: parent.clone(),
        });
    }
    let mut cursor = None;
    let mut seen = HashSet::new();
    loop {
        let suffix = cursor
            .as_deref()
            .map_or(String::new(), |value| format!("&cursor={}", encode(value)));
        let value = client
            .get(&format!(
                "/sessions/{}/children?limit=200{suffix}",
                encode(&session.id)
            ))
            .await
            .map_err(err)?;
        let page: Envelope = serde_json::from_value(value)
            .map_err(|error| format!("Invalid child-thread response: {error}"))?;
        let page = page.data;
        if page.parent_id != session.id {
            return Err("Child-thread response belongs to another parent".into());
        }
        for thread in page.data {
            if thread.session.parent_id != session.id {
                return Err("Child thread belongs to another parent".into());
            }
            let info = thread.session;
            items.push(Choice {
                key: info.id,
                label: format!(
                    "{} ({})",
                    info.subagent_name.as_deref().unwrap_or(&info.title),
                    thread.status.label()
                ),
                detail: format!(
                    "{} · {}{}",
                    info.agent,
                    info.directory,
                    thread
                        .reason
                        .map_or(String::new(), |reason| format!(" · {reason}"))
                ),
            });
        }
        match page.cursor.next {
            Some(next) if !next.is_empty() && seen.insert(next.clone()) => cursor = Some(next),
            Some(_) => return Err("Child-thread pagination cursor is invalid or repeated".into()),
            None => {
                return Ok(items);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};
    use std::io::{Read, Write};

    async fn fetch(pages: Vec<Value>) -> (Result<Vec<Choice>, String>, Vec<String>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let mut requests = Vec::new();
            for page in pages {
                let (mut socket, _) = listener.accept().unwrap();
                socket
                    .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                    .unwrap();
                let mut head = Vec::new();
                let mut buffer = [0; 1024];
                while !head.windows(4).any(|w| w == b"\r\n\r\n") {
                    let n = socket.read(&mut buffer).unwrap();
                    assert!(n > 0);
                    head.extend_from_slice(&buffer[..n]);
                }
                requests.push(String::from_utf8(head).unwrap());
                let body = page.to_string();
                write!(
                    socket,
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                )
                .unwrap();
            }
            requests
        });
        let client = Client::http(&base, Some("secret".into()));
        let result = load(
            &client,
            &Session {
                id: "ses/parent".into(),
                parent_id: Some("ses_root".into()),
                ..Default::default()
            },
        )
        .await;
        (result, server.join().unwrap())
    }

    fn page(parent: &str, child_parent: &str, next: Option<&str>) -> Value {
        json!({"data": {"parent_id":parent,"data":[{
            "session":{"id":"ses_child","parent_id":child_parent,
                "subagent_name":"review","title":"Review","directory":"/other/worktree","agent":"explore"},
            "status":"waiting","reason":"Waiting for a user reply"
        }],"cursor":{"next":next}}})
    }

    #[tokio::test]
    async fn child_threads_load_all_pages_and_preserve_parent_navigation() {
        let (result, requests) = fetch(vec![
            page("ses/parent", "ses/parent", Some("cursor /+")),
            json!({"data":{"parent_id":"ses/parent","data":[],"cursor":{"next":null}}}),
        ])
        .await;
        let choices = result.unwrap();
        assert_eq!(choices.len(), 2);
        assert_eq!(choices[0].key, "ses_root");
        assert_eq!(choices[1].key, "ses_child");
        assert!(choices[1].label.contains("waiting"));
        assert!(choices[1].detail.contains("/other/worktree"));
        assert!(requests[0].contains("/sessions/ses%2Fparent/children?limit=200"));
        assert!(requests[1].contains("cursor=cursor%20%2F%2B"));
        assert!(
            requests[0]
                .to_ascii_lowercase()
                .contains("authorization: basic")
        );
    }

    #[tokio::test]
    async fn child_threads_refuse_foreign_scope_and_looping_cursors() {
        for invalid in [
            page("foreign", "ses/parent", None),
            page("ses/parent", "foreign", None),
            json!({"data":{}}),
        ] {
            let (result, _) = fetch(vec![invalid]).await;
            assert!(result.is_err());
        }
        let (result, _) = fetch(vec![
            page("ses/parent", "ses/parent", Some("repeat")),
            page("ses/parent", "ses/parent", Some("repeat")),
        ])
        .await;
        assert!(result.unwrap_err().contains("repeated"));
    }
}
