//! Performing [`Action`]s through the API and turning results into messages.

use cyber_client::Client;
use cyber_core::agent_mentions::parse as leading_agent_mention;
use serde_json::{Value, json};

use crate::app::Action;
use crate::model::{Choice, Item, Queued, Request, Session};

/// Results the runner applies to the App.
#[derive(Debug, Clone)]
pub enum Msg {
    AdmissionStarted(crate::admissions::Request),
    AdmissionUpdated {
        stop: bool,
        request: crate::admissions::Request,
        result: Result<Value, String>,
    },
    Snapshot {
        session: Session,
        items: Vec<Item>,
        queued: Vec<Queued>,
        requests: Vec<Request>,
    },
    Cost {
        session_id: String,
        generation: u64,
        result: Result<crate::cost::Cost, String>,
    },
    Tasks {
        session_id: String,
        items: Vec<Choice>,
    },
    Children {
        session_id: String,
        result: Result<Vec<Choice>, String>,
    },
    Sessions(Vec<Choice>),
    Models(Vec<Choice>),
    Commands(Vec<Choice>),
    Files(Vec<String>),
    Agents {
        directory: String,
        items: Vec<Choice>,
    },
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

pub(crate) fn err(e: cyber_client::ClientError) -> String {
    e.to_string()
}

#[cfg(test)]
pub async fn perform(client: &Client, session: &Session, action: Action) -> Result<Msg, String> {
    perform_owned(client, session, action, None).await
}

pub async fn perform_owned(
    client: &Client,
    session: &Session,
    action: Action,
    context: Option<&crate::admissions::Context<'_>>,
) -> Result<Msg, String> {
    let scoped = client.at(&session.directory);
    let client = &scoped;
    match action {
        Action::Admission { request, stop } => {
            Ok(crate::admissions::perform(client, request, stop).await)
        }
        Action::Refresh => snapshot(client, &session.id).await,
        Action::LoadCost { generation } => {
            let result = client
                .get(&format!("/usage?scope=session&id={}", encode(&session.id)))
                .await
                .map_err(err)
                .and_then(|value| crate::cost::Cost::parse_report(&value["data"], &session.id));
            Ok(Msg::Cost {
                session_id: session.id.clone(),
                generation,
                result,
            })
        }
        Action::SwitchModel(_)
        | Action::SwitchMode(_)
        | Action::Fork
        | Action::NewSession
        | Action::Open(_) => switch(client, session, action).await,
        Action::LoadTasks | Action::OpenTask(_) | Action::StopTask(_) | Action::StopTasks => {
            manage_tasks(client, session, action).await
        }
        Action::LoadChildren => Ok(Msg::Children {
            session_id: session.id.clone(),
            result: crate::children::load(client, session).await,
        }),
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
        Action::LoadAgents => {
            let items = agent_choices(&client.get("/agents").await.map_err(err)?["data"]);
            Ok(Msg::Agents {
                directory: session.directory.clone(),
                items,
            })
        }
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
        other => converse(client, session, other, context).await,
    }
}

/// Actions on the open Session's conversation.
async fn converse(
    client: &Client,
    session: &Session,
    action: Action,
    context: Option<&crate::admissions::Context<'_>>,
) -> Result<Msg, String> {
    let id = session.id.as_str();
    match action {
        Action::Prompt { text, delivery } => {
            if let Some((agent, prompt)) = leading_agent_mention(&text) {
                let agents = agent_choices(&client.get("/agents").await.map_err(err)?["data"]);
                if agents.iter().any(|profile| profile.key == agent) {
                    if delivery != "steer" {
                        return Err("Submit an agent mention with Enter to start the child".into());
                    }
                    if prompt.trim().is_empty() {
                        return Err(format!("Usage: @{agent} <prompt>"));
                    }
                    return crate::admissions::start(
                        client,
                        id,
                        &session.directory,
                        json!({"prompt":prompt,"agent":agent}),
                        context,
                    )
                    .await;
                }
            }
            let parts = parts(&text, &session.directory);
            post(
                client,
                &format!("/sessions/{id}/prompt"),
                json!({ "parts": parts, "delivery": delivery }),
            )
            .await
        }
        Action::Subtask(prompt) => {
            crate::admissions::start(
                client,
                id,
                &session.directory,
                json!({"prompt":prompt}),
                context,
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

fn agent_choices(data: &Value) -> Vec<Choice> {
    data.as_array()
        .into_iter()
        .flatten()
        .filter(|profile| matches!(profile["mode"].as_str(), Some("subagent" | "all")))
        .filter_map(|profile| {
            let name = profile["name"].as_str()?;
            Some(Choice {
                key: name.into(),
                label: name.into(),
                detail: format!(
                    "agent · {}",
                    profile["description"].as_str().unwrap_or_default()
                ),
            })
        })
        .collect()
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

async fn manage_tasks(client: &Client, session: &Session, action: Action) -> Result<Msg, String> {
    match action {
        Action::LoadTasks => tasks(client, &session.id).await,
        Action::OpenTask(id) => {
            let job = client.get(&format!("/jobs/{id}")).await.map_err(err)?;
            let child = job["data"]["child_id"]
                .as_str()
                .ok_or("Task has no child Session")?;
            switch(client, session, Action::Open(child.into())).await
        }
        Action::StopTask(id) => {
            client
                .post(&format!("/jobs/{id}/stop"), json!({}))
                .await
                .map_err(err)?;
            tasks(client, &session.id).await
        }
        Action::StopTasks => {
            let jobs = fetch_jobs(client, &session.id).await?;
            for job in jobs.iter().filter(|job| job["status"] == "running") {
                if let Some(id) = job["id"].as_str() {
                    client
                        .post(&format!("/jobs/{id}/stop"), json!({}))
                        .await
                        .map_err(err)?;
                }
            }
            tasks(client, &session.id).await
        }
        _ => Ok(Msg::Done),
    }
}

async fn tasks(client: &Client, session: &str) -> Result<Msg, String> {
    let jobs = fetch_jobs(client, session).await?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64;
    let items = jobs
        .iter()
        .map(|job| {
            let elapsed = (job["ended_ms"].as_i64().unwrap_or(now)
                - job["started_ms"].as_i64().unwrap_or(now))
            .max(0)
                / 1000;
            let last = job
                .pointer("/result/text")
                .and_then(Value::as_str)
                .or_else(|| job["error"].as_str())
                .unwrap_or("")
                .lines()
                .last()
                .unwrap_or("")
                .chars()
                .take(120)
                .collect::<String>();
            Choice {
                key: job["id"].as_str().unwrap_or("").into(),
                label: format!(
                    "{} ({})",
                    job["name"].as_str().unwrap_or("task"),
                    job["status"].as_str().unwrap_or("unknown")
                ),
                detail: format!(
                    "{} · {elapsed}s · {last}",
                    job["kind"].as_str().unwrap_or("task")
                ),
            }
        })
        .collect();
    Ok(Msg::Tasks {
        session_id: session.into(),
        items,
    })
}

async fn fetch_jobs(client: &Client, session: &str) -> Result<Vec<Value>, String> {
    let mut jobs = Vec::new();
    let mut cursor = String::new();
    loop {
        let page = client
            .get(&format!("/jobs?session_id={session}&limit=200{cursor}"))
            .await
            .map_err(err)?;
        jobs.extend(page["data"].as_array().into_iter().flatten().cloned());
        match page.pointer("/cursor/next").and_then(Value::as_str) {
            Some(next) => cursor = format!("&cursor={}", encode(next)),
            None => return Ok(jobs),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn leading_agent_mentions_require_eligible_catalogue_entries() {
        let choices = agent_choices(&json!([
            {"name":"build","mode":"primary"},
            {"name":"explore","mode":"subagent","description":"read only"},
            {"name":"review/security","mode":"all"},
            {"name":"invalid","mode":"other"}
        ]));
        assert_eq!(
            choices
                .iter()
                .map(|choice| choice.key.as_str())
                .collect::<Vec<_>>(),
            vec!["explore", "review/security"]
        );
        assert_eq!(
            leading_agent_mention(" @review/security\ninspect"),
            Some(("review/security".into(), "inspect"))
        );
        assert_eq!(
            leading_agent_mention("@explore"),
            Some(("explore".into(), ""))
        );
        assert_eq!(
            leading_agent_mention("@\"review team\" inspect"),
            Some(("review team".into(), "inspect"))
        );
        assert_eq!(
            leading_agent_mention("@\"review team\"suffix inspect"),
            None
        );
        assert_eq!(leading_agent_mention("look at @explore"), None);
        assert_eq!(leading_agent_mention("\"@explore\" inspect"), None);
        assert_eq!(
            leading_agent_mention("@./explore inspect"),
            Some(("./explore".into(), "inspect"))
        );
    }

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
