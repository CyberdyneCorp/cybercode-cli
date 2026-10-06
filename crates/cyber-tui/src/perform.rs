//! Performing [`Action`]s through the API and turning results into messages.

use cyber_client::Client;
use serde_json::{Value, json};

use crate::app::Action;
use crate::model::{Choice, Item, Queued, Request, Session};

/// Results the runner applies to the App.
#[derive(Debug, Clone)]
pub enum Msg {
    Snapshot {
        session: Session,
        items: Vec<Item>,
        queued: Vec<Queued>,
        requests: Vec<Request>,
    },
    Sessions(Vec<Choice>),
    Models(Vec<Choice>),
    Commands(Vec<Choice>),
    Files(Vec<String>),
    /// Another Session is now open.
    Switched(Session),
    ModeChanged {
        session_id: String,
        result: Result<Session, String>,
    },
    Toast(String),
    Done,
}

pub async fn snapshot(client: &Client, id: &str) -> Result<Msg, String> {
    let session =
        Session::parse(&client.get(&format!("/sessions/{id}")).await.map_err(err)?["data"]);
    let mut items = Vec::new();
    let mut cursor: Option<String> = None;
    loop {
        let page_cursor = cursor
            .as_deref()
            .map_or(String::new(), |c| format!("&cursor={c}"));
        let page = client
            .get(&format!(
                "/sessions/{id}/messages?order=asc&limit=200{page_cursor}"
            ))
            .await
            .map_err(err)?;
        items.extend(
            page["data"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Item::parse),
        );
        cursor = page["cursor"]["next"].as_str().map(str::to_string);
        if cursor.is_none() {
            break;
        }
    }
    let inbox = client
        .get(&format!("/sessions/{id}/inbox"))
        .await
        .map_err(err)?;
    let queued = inbox["data"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Queued::parse)
        .collect();
    let mut requests = Vec::new();
    for kind in ["permissions", "questions"] {
        let list = client
            .get(&format!("/{kind}/requests?session_id={id}"))
            .await
            .map_err(err)?;
        requests.extend(
            list["data"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Request::parse),
        );
    }
    Ok(Msg::Snapshot {
        session,
        items,
        queued,
        requests,
    })
}

fn err(e: cyber_client::ClientError) -> String {
    e.to_string()
}

pub async fn perform(client: &Client, session: &Session, action: Action) -> Result<Msg, String> {
    match action {
        Action::Refresh => snapshot(client, &session.id).await,
        Action::SwitchModel(_)
        | Action::SwitchMode(_)
        | Action::Fork
        | Action::NewSession
        | Action::Open(_) => switch(client, session, action).await,
        Action::LoadSessions => sessions(client).await,
        Action::Rename { .. } | Action::Archive(_) | Action::Delete(_) => {
            manage(client, action).await
        }
        Action::ForkSession(id) => switched(
            client
                .post(&format!("/sessions/{id}/fork"), json!({}))
                .await,
        ),
        Action::LoadModels => models(client).await,
        Action::FindFiles(query) => {
            let found = client
                .get(&format!("/fs/find?limit=20&query={}", encode(&query)))
                .await
                .map_err(err)?;
            Ok(Msg::Files(
                found["data"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect(),
            ))
        }
        Action::SaveTheme(_) | Action::Editor(_) | Action::Quit => Ok(Msg::Done),
        other => converse(client, session, other).await,
    }
}

/// Actions on the open Session's conversation.
async fn converse(client: &Client, session: &Session, action: Action) -> Result<Msg, String> {
    let id = session.id.as_str();
    match action {
        Action::Prompt { text, delivery } => {
            let parts = parts(&text, &session.directory);
            post(
                client,
                &format!("/sessions/{id}/prompt"),
                json!({ "parts": parts, "delivery": delivery }),
            )
            .await
        }
        Action::Shell(command) => {
            post(
                client,
                &format!("/sessions/{id}/shell"),
                json!({ "command": command }),
            )
            .await?;
            snapshot(client, id).await
        }
        Action::Command { name, arguments } => {
            post(
                client,
                &format!("/sessions/{id}/command"),
                json!({ "name": name, "arguments": arguments }),
            )
            .await
        }
        Action::Interrupt => post(client, &format!("/sessions/{id}/interrupt"), json!({})).await,
        Action::Reply { request, body } => {
            post(
                client,
                &format!("/sessions/{id}/permissions/{request}/reply"),
                body,
            )
            .await
        }
        Action::Answer { request, body } => {
            post(
                client,
                &format!("/sessions/{id}/questions/{request}/reply"),
                body,
            )
            .await
        }
        Action::RemoveQueued(message) => {
            post(
                client,
                &format!("/sessions/{id}/inbox/{message}/drop"),
                json!({}),
            )
            .await
        }
        Action::Compact(instructions) => {
            post(
                client,
                &format!("/sessions/{id}/compact"),
                json!({ "instructions": instructions }),
            )
            .await?;
            Ok(Msg::Toast("compacting…".into()))
        }
        _ => Ok(Msg::Done),
    }
}

/// Actions that change which Session or settings are in effect.
async fn switch(client: &Client, session: &Session, action: Action) -> Result<Msg, String> {
    let id = session.id.as_str();
    let result =
        match action {
            Action::SwitchModel(model) => {
                client
                    .post(&format!("/sessions/{id}/model"), json!({ "model": model }))
                    .await
            }
            Action::SwitchMode(mode) => {
                return Ok(Msg::ModeChanged {
                    session_id: id.into(),
                    result: client
                        .post(&format!("/sessions/{id}/mode"), json!({ "mode": mode }))
                        .await
                        .map_err(err)
                        .map(|v| Session::parse(&v["data"])),
                });
            }
            Action::Fork => {
                client
                    .post(&format!("/sessions/{id}/fork"), json!({}))
                    .await
            }
            Action::NewSession => client
                .post(
                    "/sessions",
                    json!({ "model": session.model, "agent": session.agent, "mode": session.mode }),
                )
                .await,
            Action::Open(other) => client.get(&format!("/sessions/{other}")).await,
            _ => return Ok(Msg::Done),
        };
    switched(result)
}

async fn post(client: &Client, path: &str, body: Value) -> Result<Msg, String> {
    client.post(path, body).await.map_err(err)?;
    Ok(Msg::Done)
}

fn switched(result: Result<Value, cyber_client::ClientError>) -> Result<Msg, String> {
    Ok(Msg::Switched(Session::parse(&result.map_err(err)?["data"])))
}

/// Change another Session from the picker, then reload the list.
async fn manage(client: &Client, action: Action) -> Result<Msg, String> {
    match action {
        Action::Rename { id, title } => client
            .patch(&format!("/sessions/{id}"), json!({ "title": title }))
            .await
            .map_err(err)?,
        Action::Archive(id) => client
            .patch(&format!("/sessions/{id}"), json!({ "archived": true }))
            .await
            .map_err(err)?,
        Action::Delete(id) => client
            .delete(&format!("/sessions/{id}"))
            .await
            .map_err(err)?,
        _ => Value::Null,
    };
    sessions(client).await
}

pub async fn sessions(client: &Client) -> Result<Msg, String> {
    let list = client.get("/sessions?limit=100").await.map_err(err)?;
    let items = list["data"]["data"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|s| Choice {
            key: s["id"].as_str().unwrap_or_default().into(),
            label: s["title"].as_str().unwrap_or_default().into(),
            detail: format!(
                "{} · ${:.4}",
                s["model"].as_str().unwrap_or_default(),
                s["cost"].as_f64().unwrap_or(0.0)
            ),
        })
        .collect();
    Ok(Msg::Sessions(items))
}

async fn models(client: &Client) -> Result<Msg, String> {
    let list = client.get("/models").await.map_err(err)?;
    let items = list["data"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|m| m["available"] == true)
        .map(|m| Choice {
            key: m["id"].as_str().unwrap_or_default().into(),
            label: m["id"].as_str().unwrap_or_default().into(),
            detail: m["name"].as_str().unwrap_or_default().into(),
        })
        .collect();
    Ok(Msg::Models(items))
}

pub async fn commands(client: &Client) -> Result<Msg, String> {
    let list = client.get("/commands").await.map_err(err)?;
    let items = list["data"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|c| Choice {
            key: c["name"].as_str().unwrap_or_default().into(),
            label: c["name"].as_str().unwrap_or_default().into(),
            detail: c["description"].as_str().unwrap_or_default().into(),
        })
        .collect();
    Ok(Msg::Commands(items))
}

/// The prompt text plus a part for each `@path` or `@path#L10-40` mention that exists.
pub fn parts(text: &str, directory: &str) -> Vec<Value> {
    let mut parts = vec![json!({ "type": "text", "text": text })];
    for token in text.split_whitespace().filter_map(|w| w.strip_prefix('@')) {
        let (path, range) = token
            .split_once("#L")
            .map_or((token, None), |(p, r)| (p, Some(r)));
        let full = std::path::Path::new(directory).join(path);
        let Ok(content) = std::fs::read_to_string(&full) else {
            continue;
        };
        let body = match range.and_then(parse_range) {
            Some((a, b)) => content
                .lines()
                .skip(a - 1)
                .take(b + 1 - a)
                .collect::<Vec<_>>()
                .join("\n"),
            None => content,
        };
        parts.push(
            json!({ "type": "text", "text": format!("<file path=\"{token}\">\n{body}\n</file>") }),
        );
    }
    parts
}

/// `10` or `10-40`, 1-based and inclusive.
fn parse_range(r: &str) -> Option<(usize, usize)> {
    let (a, b) = r.split_once('-').unwrap_or((r, r));
    let (a, b) = (
        a.parse::<usize>().ok()?,
        b.trim_start_matches('L').parse::<usize>().ok()?,
    );
    (a >= 1 && b >= a).then_some((a, b))
}

fn encode(s: &str) -> String {
    s.bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-_.~/".contains(&b) {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mentions_attach_files_and_line_ranges() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("a.txt"), "one\ntwo\nthree\nfour\n").unwrap();
        let dir = tmp.path().display().to_string();
        let parts = parts("look at @a.txt#L2-3 and @missing.rs", &dir);
        assert_eq!(parts.len(), 2);
        assert_eq!(
            parts[1]["text"],
            "<file path=\"a.txt#L2-3\">\ntwo\nthree\n</file>"
        );
        assert_eq!(parse_range("5"), Some((5, 5)));
        assert_eq!(parse_range("4-2"), None);
    }
}
