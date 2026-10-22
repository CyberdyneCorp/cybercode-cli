//! Session branches in the TUI (`tui` → Branch tree view): `/tree` lists the user
//! messages of every branch as a collapsible tree and checks one out (`c` adds the code
//! restore, `l` labels), `/label` labels the active tip, and the header names the active
//! branch when the Session has more than one tip.
//!
//! Previews and labels come from the server; previews are user text and are kept
//! without control characters.

use std::collections::{BTreeMap, BTreeSet};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use cyber_client::Client;
use serde_json::{Value, json};

use crate::app::{Action, App, Overlay};
use crate::messaging::clean;
use crate::model::Session;
use crate::perform::{Msg, err};
use crate::workflows::encode;

const LABEL_USAGE: &str = "Use /label <name> or /label --clear <name>";
const IDLE: &str = "The Session must be idle to switch branches; interrupt it (Esc) first";

/// A user message of the tree, as `GET .../tree` lists it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Node {
    pub id: String,
    pub parent_id: Option<String>,
    pub preview: String,
    pub labels: Vec<String>,
    pub active: bool,
    pub tip: bool,
}

impl Node {
    pub fn parse(v: &Value) -> Option<Self> {
        let labels = v["labels"].as_array().into_iter().flatten();
        Some(Self {
            id: v["id"].as_str()?.to_owned(),
            parent_id: v["parent_id"].as_str().map(str::to_owned),
            preview: clean(v["preview"].as_str().unwrap_or_default()),
            labels: labels.filter_map(Value::as_str).map(clean).collect(),
            active: v["active"] == true,
            tip: v["tip"] == true,
        })
    }
}

/// A shown row of the tree: the node, its depth and how many branches start below it.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub index: usize,
    pub depth: usize,
    pub children: usize,
    pub collapsed: bool,
}

/// The shown Session's branch tree and the `/tree` view's state.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Tree {
    /// Oldest first.
    pub nodes: Vec<Node>,
    pub selected: usize,
    pub collapsed: BTreeSet<String>,
    /// `c`: restore code with the checkout.
    pub code: bool,
    /// `l`: the label being typed for the selected message.
    pub label: Option<String>,
}

/// Branch requests the runner performs.
#[derive(Debug, Clone, PartialEq)]
pub enum Request {
    /// Read the tree; `open` shows `/tree`.
    Load {
        open: bool,
    },
    Checkout {
        message_id: String,
        code: bool,
    },
    /// Label `message_id`, or the active tip when `None`.
    Label {
        message_id: Option<String>,
        name: String,
    },
    Unlabel(String),
}

/// The outcome of a [`Request`].
#[derive(Debug, Clone, PartialEq)]
pub enum Reply {
    Loaded { nodes: Vec<Node>, open: bool },
    CheckedOut(Result<CheckedOut, String>),
    Labeled(Result<(Vec<Node>, String), String>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct CheckedOut {
    pub tip: String,
    /// The restored files' diff, when code was restored.
    pub diff: Option<(usize, String)>,
}

/// A long message ID's end, which tells generated IDs apart.
pub fn short_id(id: &str) -> String {
    let chars: Vec<char> = id.chars().collect();
    match chars.len() {
        0..=12 => id.to_owned(),
        n => format!("…{}", chars[n - 8..].iter().collect::<String>()),
    }
}

impl Tree {
    /// Replace the nodes, keeping the selected message while it is still listed.
    pub fn set(&mut self, nodes: Vec<Node>) {
        let selected = self.selected_node().map(|n| n.id.clone());
        self.nodes = nodes;
        let rows = self.rows();
        self.selected = selected
            .and_then(|id| rows.iter().position(|r| self.nodes[r.index].id == id))
            .unwrap_or_else(|| self.active_row(&rows));
    }

    fn children(&self) -> BTreeMap<Option<&str>, Vec<usize>> {
        let mut children: BTreeMap<Option<&str>, Vec<usize>> = BTreeMap::new();
        for (i, node) in self.nodes.iter().enumerate() {
            children
                .entry(node.parent_id.as_deref())
                .or_default()
                .push(i);
        }
        children
    }

    /// Visible rows, depth first. A path continues at its depth; the branches of a
    /// branch point are one level deeper. A collapsed node hides what is below it.
    pub fn rows(&self) -> Vec<Row> {
        let children = self.children();
        let mut rows = Vec::new();
        let mut stack: Vec<(usize, usize)> = Vec::new();
        let roots = children.get(&None).cloned().unwrap_or_default();
        let depth = usize::from(roots.len() > 1);
        stack.extend(roots.iter().rev().map(|&i| (i, depth)));
        while let Some((index, depth)) = stack.pop() {
            let id = self.nodes[index].id.as_str();
            let below = children.get(&Some(id)).cloned().unwrap_or_default();
            let collapsed = !below.is_empty() && self.collapsed.contains(id);
            rows.push(Row {
                index,
                depth,
                children: below.len(),
                collapsed,
            });
            if !collapsed {
                let next = depth + usize::from(below.len() > 1);
                stack.extend(below.iter().rev().map(|&i| (i, next)));
            }
        }
        rows
    }

    fn active_row(&self, rows: &[Row]) -> usize {
        rows.iter()
            .rposition(|r| self.nodes[r.index].active)
            .unwrap_or_default()
    }

    pub fn selected_node(&self) -> Option<&Node> {
        let rows = self.rows();
        rows.get(self.selected).map(|r| &self.nodes[r.index])
    }

    /// `branch <label or short id> (<n> of <total>)` when there is more than one tip.
    pub fn header(&self) -> Option<String> {
        let tips: Vec<&Node> = self.nodes.iter().filter(|n| n.tip).collect();
        let at = tips.iter().position(|n| n.active)?;
        if tips.len() < 2 {
            return None;
        }
        let name = self
            .branch_label(tips[at])
            .unwrap_or_else(|| short_id(&tips[at].id));
        Some(format!("branch {name} ({} of {})", at + 1, tips.len()))
    }

    /// The nearest label on the active tip's own branch, below its branch point.
    fn branch_label(&self, tip: &Node) -> Option<String> {
        let children = self.children();
        let by_id: BTreeMap<&str, &Node> = self.nodes.iter().map(|n| (n.id.as_str(), n)).collect();
        let mut node = tip;
        loop {
            if let Some(label) = node.labels.first() {
                return Some(label.clone());
            }
            let parent = by_id.get(node.parent_id.as_deref()?)?;
            if children.get(&Some(parent.id.as_str())).map_or(0, Vec::len) > 1 {
                return None;
            }
            node = parent;
        }
    }
}

/// Run a branch request; every reply is a [`Msg::Branches`] for `session`.
pub async fn perform(client: &Client, session: &Session, request: Request) -> Result<Msg, String> {
    let id = encode(&session.id);
    let reply = match request {
        Request::Load { open } => Reply::Loaded {
            nodes: load(client, &session.id).await?,
            open,
        },
        Request::Checkout { message_id, code } => {
            Reply::CheckedOut(checkout(client, &id, &message_id, code).await)
        }
        Request::Label { message_id, name } => {
            Reply::Labeled(label(client, session, message_id, &name).await)
        }
        Request::Unlabel(name) => {
            let path = format!("/sessions/{id}/labels/{}", encode(&name));
            let removed = client.delete(&path).await.map_err(err);
            Reply::Labeled(match removed {
                Ok(_) => load(client, &session.id)
                    .await
                    .map(|nodes| (nodes, format!("Removed label {name}"))),
                Err(error) => Err(error),
            })
        }
    };
    Ok(Msg::Branches {
        session_id: session.id.clone(),
        reply,
    })
}

/// The Session's tree; empty from a server without branch routes.
pub async fn load(client: &Client, session_id: &str) -> Result<Vec<Node>, String> {
    match client
        .get(&format!("/sessions/{}/tree", encode(session_id)))
        .await
    {
        Ok(tree) => Ok(tree["data"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Node::parse)
            .collect()),
        Err(error) if error.route_missing() => Ok(Vec::new()),
        Err(error) => Err(err(error)),
    }
}

async fn checkout(
    client: &Client,
    id: &str,
    message_id: &str,
    code: bool,
) -> Result<CheckedOut, String> {
    let body = json!({ "message_id": message_id, "code": code });
    let done = client
        .post(&format!("/sessions/{id}/checkout"), body)
        .await
        .map_err(err)?;
    let done = &done["data"];
    let diff = code.then(|| {
        let files = done["restored_files"].as_array().map_or(0, Vec::len);
        let text = done["revert"]["diff"].as_str().unwrap_or_default();
        (files, text.to_owned())
    });
    Ok(CheckedOut {
        tip: done["tip_message_id"]
            .as_str()
            .unwrap_or_default()
            .to_owned(),
        diff,
    })
}

async fn label(
    client: &Client,
    session: &Session,
    message_id: Option<String>,
    name: &str,
) -> Result<(Vec<Node>, String), String> {
    let id = encode(&session.id);
    let message = match message_id {
        Some(message) => message,
        None => {
            let last = client
                .get(&format!("/sessions/{id}/messages?order=desc&limit=1"))
                .await
                .map_err(err)?;
            last["data"][0]["id"]
                .as_str()
                .ok_or("The Session has no messages to label")?
                .to_owned()
        }
    };
    client
        .request(
            "PUT",
            &format!("/sessions/{id}/labels/{}", encode(name)),
            Some(json!({ "message_id": message })),
        )
        .await
        .map_err(err)?;
    let nodes = load(client, &session.id).await?;
    Ok((nodes, format!("Labeled {} {name}", short_id(&message))))
}

/// Whether `name` matches `[A-Za-z0-9._-]{1,48}`, the server's label grammar.
fn valid_label(name: &str) -> bool {
    (1..=48).contains(&name.len())
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

impl App {
    /// `/tree`: read the tree, then show it.
    pub(crate) fn tree_command(&mut self) -> Vec<Action> {
        vec![Action::Branch(Request::Load { open: true })]
    }

    /// `/label <name>` labels the active tip; `/label --clear <name>` removes a label.
    pub(crate) fn label_command(&mut self, args: &str) -> Vec<Action> {
        let words: Vec<&str> = args.split_whitespace().collect();
        let request = match words.as_slice() {
            ["--clear", name] => Request::Unlabel((*name).to_owned()),
            [name] if !name.starts_with('-') => Request::Label {
                message_id: None,
                name: (*name).to_owned(),
            },
            _ => {
                self.toast(LABEL_USAGE);
                return Vec::new();
            }
        };
        let name = match &request {
            Request::Label { name, .. } | Request::Unlabel(name) => name,
            _ => unreachable!("label requests"),
        };
        if !valid_label(name) {
            self.toast("A label is 1 to 48 letters, digits, '.', '_' or '-'");
            return Vec::new();
        }
        vec![Action::Branch(request)]
    }

    /// Keys of `/tree`: move, fold, `c` code, `l` label, Enter checkout, Esc close.
    pub(crate) fn tree_key(&mut self, key: KeyEvent) -> Vec<Action> {
        if self.branches.label.is_some() {
            return self.tree_label_key(key);
        }
        let rows = self.branches.rows().len();
        let tree = &mut self.branches;
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => self.overlay = Overlay::None,
            KeyCode::Up | KeyCode::Char('k') => tree.selected = tree.selected.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => {
                tree.selected = (tree.selected + 1).min(rows.saturating_sub(1));
            }
            KeyCode::Char(' ') | KeyCode::Left | KeyCode::Right => self.toggle_fold(),
            KeyCode::Char('c') => tree.code = !tree.code,
            KeyCode::Char('l') if tree.selected_node().is_some() => {
                tree.label = Some(String::new());
            }
            KeyCode::Enter => return self.checkout_selected(),
            _ => {}
        }
        Vec::new()
    }

    fn toggle_fold(&mut self) {
        let tree = &mut self.branches;
        let Some(id) = tree.selected_node().map(|n| n.id.clone()) else {
            return;
        };
        if !tree.collapsed.remove(&id) {
            tree.collapsed.insert(id);
        }
    }

    fn checkout_selected(&mut self) -> Vec<Action> {
        if self.session.running {
            self.toast(IDLE);
            return Vec::new();
        }
        let tree = &self.branches;
        let Some(node) = tree.selected_node() else {
            return Vec::new();
        };
        vec![Action::Branch(Request::Checkout {
            message_id: node.id.clone(),
            code: tree.code,
        })]
    }

    fn tree_label_key(&mut self, key: KeyEvent) -> Vec<Action> {
        let tree = &mut self.branches;
        let Some(label) = &mut tree.label else {
            return Vec::new();
        };
        match key.code {
            KeyCode::Esc => tree.label = None,
            KeyCode::Backspace => {
                label.pop();
            }
            KeyCode::Enter => {
                let name = std::mem::take(label);
                tree.label = None;
                let message_id = tree.selected_node().map(|n| n.id.clone());
                if !valid_label(&name) {
                    self.toast("A label is 1 to 48 letters, digits, '.', '_' or '-'");
                    return Vec::new();
                }
                return vec![Action::Branch(Request::Label { message_id, name })];
            }
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => label.push(c),
            _ => {}
        }
        Vec::new()
    }

    /// A branch reply for `session_id`; replies for another Session are dropped.
    pub(crate) fn branch_reply(&mut self, session_id: &str, reply: Reply) -> Vec<Action> {
        if session_id != self.session.id {
            return Vec::new();
        }
        match reply {
            Reply::Loaded { nodes, open } => {
                self.branches.set(nodes);
                if open {
                    self.branches.label = None;
                    self.overlay = Overlay::Tree;
                }
            }
            Reply::Labeled(Ok((nodes, toast))) => {
                self.branches.set(nodes);
                self.toast(toast);
            }
            Reply::CheckedOut(Ok(done)) => return self.checked_out(done),
            Reply::Labeled(Err(error)) | Reply::CheckedOut(Err(error)) => {
                self.toast(error);
            }
        }
        Vec::new()
    }

    /// The new branch's transcript and tree come with the refresh; a code restore shows
    /// its diff.
    fn checked_out(&mut self, done: CheckedOut) -> Vec<Action> {
        self.overlay = Overlay::None;
        self.toast(format!("Checked out {}", short_id(&done.tip)));
        if let Some((files, diff)) = done.diff {
            let noun = if files == 1 { "file" } else { "files" };
            let title = format!("Restored {files} {noun} for {}", short_id(&done.tip));
            self.open_task_log(title, diff);
        }
        self.items.clear();
        self.streaming.clear();
        vec![Action::Refresh]
    }
}
