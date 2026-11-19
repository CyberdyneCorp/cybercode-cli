//! `/map [<path>]` (`tui` → Repository map view): the Location's repository map from
//! `GET /lsp/map`, at the Session model's `repo_map.tool_tokens` budget and focused on
//! the optional path. `r` rebuilds the index, Enter opens the selected path in the file
//! explorer and Esc closes. Opening the view adds nothing to the transcript or context.

use crossterm::event::{KeyCode, KeyEvent};
use serde_json::Value;

use cyber_client::Client;

use crate::app::{Action, App, Overlay};
use crate::perform::{Msg, err};

/// The message of a Location whose map is turned off.
pub const DISABLED: &str = "Repository map is disabled (repo_map.enabled)";

#[derive(Debug, Clone, PartialEq)]
pub enum Request {
    Load {
        session_id: String,
        model: String,
        focus: Option<String>,
    },
    Rebuild {
        session_id: String,
        model: String,
        focus: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum Reply {
    Loaded { session_id: String, view: MapView },
    Failed(String),
}

/// One line of the map; file lines carry their path.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub text: String,
    pub path: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MapView {
    pub focus: Option<String>,
    pub header: String,
    pub rows: Vec<Row>,
    pub selected: usize,
}

impl MapView {
    /// The view of a `GET /lsp/map` body's `data`.
    pub fn parse(data: &Value, focus: Option<String>) -> Self {
        let index = &data["index"];
        let excluded: u64 = index["excluded"]
            .as_object()
            .map(|reasons| reasons.values().filter_map(Value::as_u64).sum())
            .unwrap_or(0);
        let header = format!(
            "{} files indexed, {} excluded · {} symbols · index {} · refreshed {}",
            index["files"].as_u64().unwrap_or(0),
            excluded,
            index["symbols"].as_u64().unwrap_or(0),
            index["status"].as_str().unwrap_or("unknown"),
            refreshed(index["refreshed_at"].as_i64()),
        );
        let text = data["text"].as_str().unwrap_or_default();
        let mut rows: Vec<Row> = text.lines().map(row).collect();
        if rows.is_empty() {
            rows.push(Row {
                text: "No indexed files".into(),
                path: None,
            });
        }
        Self {
            focus,
            header,
            rows,
            selected: 0,
        }
    }

    pub fn selected_path(&self) -> Option<&str> {
        self.rows.get(self.selected)?.path.as_deref()
    }
}

/// `path:` heads a file's rows; an `other files:` line lists bare paths.
fn row(line: &str) -> Row {
    let path = if let Some(path) = line.strip_suffix(':') {
        (!line.starts_with(' ') && line != "other files:").then(|| path.to_owned())
    } else {
        (!line.starts_with(' ')).then(|| line.to_owned())
    };
    Row {
        text: line.to_owned(),
        path,
    }
}

fn refreshed(at: Option<i64>) -> String {
    at.and_then(chrono::DateTime::from_timestamp_millis)
        .map_or_else(
            || "never".into(),
            |t| t.format("%Y-%m-%d %H:%M:%S UTC").to_string(),
        )
}

impl App {
    /// `/map [<path>]`: load the map; the view opens when it arrives.
    pub(crate) fn map_command(&mut self, args: &str) -> Vec<Action> {
        let focus = (!args.is_empty()).then(|| args.trim_start_matches("./").to_owned());
        vec![Action::RepoMap(Request::Load {
            session_id: self.session.id.clone(),
            model: self.session.model.clone(),
            focus,
        })]
    }

    pub(crate) fn repo_map_reply(&mut self, reply: Reply) -> Vec<Action> {
        match reply {
            Reply::Loaded { session_id, view } if session_id == self.session.id => {
                let selected = match &self.overlay {
                    Overlay::RepoMap(old) => old.selected.min(view.rows.len().saturating_sub(1)),
                    _ => 0,
                };
                self.overlay = Overlay::RepoMap(Box::new(MapView { selected, ..view }));
            }
            Reply::Loaded { .. } => {}
            Reply::Failed(message) => self.toast(message),
        }
        Vec::new()
    }

    pub(crate) fn repo_map_key(&mut self, key: KeyEvent) -> Vec<Action> {
        let Overlay::RepoMap(view) = &mut self.overlay else {
            return Vec::new();
        };
        let last = view.rows.len().saturating_sub(1);
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => view.selected = view.selected.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => view.selected = (view.selected + 1).min(last),
            KeyCode::PageUp => view.selected = view.selected.saturating_sub(10),
            KeyCode::PageDown => view.selected = (view.selected + 10).min(last),
            KeyCode::Char('r') => {
                let focus = view.focus.clone();
                self.toast("Rebuilding the repository index…");
                return vec![Action::RepoMap(Request::Rebuild {
                    session_id: self.session.id.clone(),
                    model: self.session.model.clone(),
                    focus,
                })];
            }
            KeyCode::Enter => return self.open_map_path(),
            KeyCode::Esc | KeyCode::Char('q') => self.overlay = Overlay::None,
            _ => {}
        }
        Vec::new()
    }

    /// Enter: the file explorer, filtered to the selected path.
    fn open_map_path(&mut self) -> Vec<Action> {
        let Overlay::RepoMap(view) = &self.overlay else {
            return Vec::new();
        };
        let Some(path) = view.selected_path().map(str::to_owned) else {
            return Vec::new();
        };
        let mut actions = self.open_explorer();
        self.explorer.filter = path.clone();
        self.explorer.filtering = false;
        self.explorer.selected = 0;
        actions.push(Action::SearchFiles(path));
        actions
    }
}

pub async fn perform(client: &Client, request: Request) -> Result<Msg, String> {
    let (session_id, model, focus) = match request {
        Request::Load {
            session_id,
            model,
            focus,
        } => (session_id, model, focus),
        Request::Rebuild {
            session_id,
            model,
            focus,
        } => {
            if let Err(e) = client.request("POST", "/lsp/map/rebuild", None).await {
                return Ok(Msg::RepoMap(Reply::Failed(failure(e))));
            }
            (session_id, model, focus)
        }
    };
    let tokens = tool_tokens(client, &model).await;
    let mut path = String::from("/lsp/map?");
    if let Some(focus) = &focus {
        path.push_str(&format!("focus={}&", crate::perform::encode(focus)));
    }
    if let Some(tokens) = tokens {
        path.push_str(&format!("tokens={tokens}"));
    }
    let reply = match client.get(path.trim_end_matches(['?', '&'])).await {
        Ok(body) => Reply::Loaded {
            session_id,
            view: MapView::parse(&body["data"], focus),
        },
        Err(e) => Reply::Failed(failure(e)),
    };
    Ok(Msg::RepoMap(reply))
}

/// The Session model's `repo_map.tool_tokens`; `None` lets the server choose.
async fn tool_tokens(client: &Client, model: &str) -> Option<u64> {
    let body = client.get("/models").await.ok()?;
    body["data"]
        .as_array()?
        .iter()
        .find(|m| m["id"] == model)?["profile"]["repo_map"]["tool_tokens"]
        .as_u64()
}

/// The server's own words for a refusal (such as the disabled map), else the error.
fn failure(e: cyber_client::ClientError) -> String {
    match e {
        cyber_client::ClientError::Api { message, .. } if message == DISABLED => message,
        other => format!("repository map: {}", err(other)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn rows_carry_the_paths_of_files() {
        let view = MapView::parse(
            &json!({
                "text": "src/auth/mod.rs:\n  3: pub fn login()\nother files:\nREADME.md\n",
                "index": {"files": 4, "symbols": 9, "status": "complete",
                          "excluded": {"binary": 2}, "refreshed_at": 0},
            }),
            Some("src/auth".into()),
        );
        assert_eq!(
            view.header,
            "4 files indexed, 2 excluded · 9 symbols · index complete · refreshed 1970-01-01 00:00:00 UTC"
        );
        let paths: Vec<_> = view.rows.iter().map(|r| r.path.as_deref()).collect();
        assert_eq!(
            paths,
            vec![Some("src/auth/mod.rs"), None, None, Some("README.md")]
        );
    }
}
