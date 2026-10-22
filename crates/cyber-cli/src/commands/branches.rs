//! `cyber sessions tree|checkout|label|export` (`cli-commands` → Session branch
//! commands): the user-message tree of a Session's branches, checkout, labels and the
//! export of one or every branch, through the server's branch routes. A message argument is an ID, a label or a unique ID prefix.

use std::collections::BTreeMap;

use cyber_client::Client;
use serde_json::{Value, json};

use super::sessions::{api, clean, encode};
use crate::cli::GlobalArgs;
use crate::error::CliError;
use crate::output;

/// What a message argument names.
#[derive(Debug, PartialEq)]
enum Target {
    Message(String),
    Label(String),
}

/// `cyber sessions tree`: an ASCII tree with the active path marked `*`, or the server's
/// nodes with `--format json`.
pub async fn tree(client: &Client, session: &str, global: &GlobalArgs) -> Result<(), CliError> {
    let nodes = nodes(client, session).await?;
    if output::is_json(global.format) {
        return output::json(&nodes);
    }
    if nodes.is_empty() {
        println!("no messages yet");
    }
    for line in render(&nodes) {
        println!("{line}");
    }
    Ok(())
}

/// `cyber sessions checkout`: prints the new tip and, with `--code`, the restored files.
pub async fn checkout(
    client: &Client,
    session: &str,
    wanted: &str,
    code: bool,
    exact: bool,
    global: &GlobalArgs,
) -> Result<(), CliError> {
    let mut body = json!({ "code": code, "exact": exact });
    match resolve(client, session, wanted).await? {
        Target::Message(id) => body["message_id"] = json!(id),
        Target::Label(name) => body["label"] = json!(name),
    }
    let done = client
        .post(&format!("/sessions/{}/checkout", encode(session)), body)
        .await
        .map_err(api)?;
    let done = &done["data"];
    if output::is_json(global.format) {
        return output::json(done);
    }
    println!(
        "tip {}",
        done["tip_message_id"].as_str().unwrap_or_default()
    );
    if code {
        let files = done["restored_files"].as_array().map_or(0, Vec::len);
        let noun = if files == 1 { "file" } else { "files" };
        println!("restored {files} {noun}; undo with POST /sessions/{session}/revert/clear");
    }
    Ok(())
}

/// `cyber sessions label`: set `name` on `at` (the active tip by default), or remove it.
pub async fn label(
    client: &Client,
    session: &str,
    name: &str,
    at: Option<&str>,
    delete: bool,
) -> Result<(), CliError> {
    let path = format!("/sessions/{}/labels/{}", encode(session), encode(name));
    if delete {
        client.delete(&path).await.map_err(api)?;
        println!("removed label {name}");
        return Ok(());
    }
    let message = match at {
        Some(wanted) => match resolve(client, session, wanted).await? {
            Target::Message(id) => id,
            Target::Label(other) => labeled_node(client, session, &other).await?,
        },
        None => active_tip(client, session).await?,
    };
    client
        .request("PUT", &path, Some(json!({ "message_id": message })))
        .await
        .map_err(api)?;
    println!("labeled {message} {name}");
    Ok(())
}

async fn nodes(client: &Client, session: &str) -> Result<Vec<Value>, CliError> {
    let tree = client
        .get(&format!("/sessions/{}/tree", encode(session)))
        .await
        .map_err(api)?;
    Ok(tree["data"].as_array().cloned().unwrap_or_default())
}

/// `cyber sessions export`: the Session and its messages as one JSON document, with
/// the active path only or, with `all_branches`, every branch in creation order.
pub async fn export(client: &Client, session: &Value, all_branches: bool) -> Result<(), CliError> {
    let id = session["id"].as_str().unwrap_or_default();
    let messages = messages(client, id, all_branches).await?;
    output::json(&json!({
        "session": session,
        "branches": if all_branches { "all" } else { "active" },
        "messages": messages,
    }))
}

/// The active path's messages or every branch's, oldest first.
async fn messages(client: &Client, session: &str, all: bool) -> Result<Vec<Value>, CliError> {
    let branches = if all { "branches=all&" } else { "" };
    let mut messages = Vec::new();
    let mut cursor = String::new();
    loop {
        let page = client
            .get(&format!(
                "/sessions/{}/messages?{branches}order=asc&limit=200{cursor}",
                encode(session)
            ))
            .await
            .map_err(api)?;
        messages.extend(page["data"].as_array().into_iter().flatten().cloned());
        match page["cursor"]["next"].as_str() {
            Some(next) => cursor = format!("&cursor={next}"),
            None => return Ok(messages),
        }
    }
}

/// Every message ID of every branch, oldest first.
async fn message_ids(client: &Client, session: &str) -> Result<Vec<String>, CliError> {
    let messages = messages(client, session, true).await?;
    Ok(messages
        .iter()
        .filter_map(|m| m["id"].as_str().map(str::to_owned))
        .collect())
}

/// An ID, then a label, then a unique ID prefix; several matching prefixes are a usage
/// error that lists them.
async fn resolve(client: &Client, session: &str, wanted: &str) -> Result<Target, CliError> {
    let ids = message_ids(client, session).await?;
    if ids.iter().any(|id| id == wanted) {
        return Ok(Target::Message(wanted.to_owned()));
    }
    let nodes = nodes(client, session).await?;
    let labeled = nodes
        .iter()
        .flat_map(|n| n["labels"].as_array().into_iter().flatten())
        .any(|l| l == wanted);
    if labeled {
        return Ok(Target::Label(wanted.to_owned()));
    }
    let matches: Vec<&String> = ids
        .iter()
        .filter(|id| !wanted.is_empty() && id.starts_with(wanted))
        .collect();
    match matches.as_slice() {
        [one] => Ok(Target::Message((*one).clone())),
        [] => Err(
            CliError::runtime(format!("No message or label {wanted} in this Session"))
                .with_hint(format!("run \"cyber sessions tree {session}\"")),
        ),
        several => {
            let listed: Vec<&str> = several.iter().map(|id| id.as_str()).collect();
            Err(CliError::usage(format!(
                "\"{wanted}\" matches {} messages: {}",
                several.len(),
                listed.join(", ")
            ))
            .with_hint("use a longer prefix or the message ID"))
        }
    }
}

/// The user message a label shows on in the tree.
async fn labeled_node(client: &Client, session: &str, name: &str) -> Result<String, CliError> {
    let nodes = nodes(client, session).await?;
    nodes
        .iter()
        .find(|n| {
            n["labels"]
                .as_array()
                .is_some_and(|l| l.contains(&json!(name)))
        })
        .and_then(|n| n["id"].as_str().map(str::to_owned))
        .ok_or_else(|| CliError::runtime(format!("No branch label {name} in this Session")))
}

/// The last message of the active path.
async fn active_tip(client: &Client, session: &str) -> Result<String, CliError> {
    let page = client
        .get(&format!(
            "/sessions/{}/messages?order=desc&limit=1",
            encode(session)
        ))
        .await
        .map_err(api)?;
    page["data"][0]["id"]
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| CliError::runtime("The Session has no messages to label"))
}

/// One line per user message: `* ` on the active path, `o ` off it. A path continues at
/// its indentation; the branches of a branch point are drawn with box-drawing guides.
fn render(nodes: &[Value]) -> Vec<String> {
    let mut children: BTreeMap<Option<&str>, Vec<&Value>> = BTreeMap::new();
    for node in nodes {
        children
            .entry(node["parent_id"].as_str())
            .or_default()
            .push(node);
    }
    let mut lines = Vec::new();
    let roots = children.get(&None).cloned().unwrap_or_default();
    branch(&children, &roots, "", &mut lines);
    lines
}

fn branch(
    children: &BTreeMap<Option<&str>, Vec<&Value>>,
    level: &[&Value],
    indent: &str,
    lines: &mut Vec<String>,
) {
    let forks = level.len() > 1;
    for (i, node) in level.iter().enumerate() {
        let last = i + 1 == level.len();
        let (guide, next) = match (forks, last) {
            (false, _) => ("", indent.to_owned()),
            (true, true) => ("└─ ", format!("{indent}   ")),
            (true, false) => ("├─ ", format!("{indent}│  ")),
        };
        lines.push(format!("{indent}{guide}{}", line(node)));
        let below = children
            .get(&node["id"].as_str())
            .cloned()
            .unwrap_or_default();
        branch(children, &below, &next, lines);
    }
}

fn line(node: &Value) -> String {
    let marker = if node["active"] == true { '*' } else { 'o' };
    let labels: Vec<&str> = node["labels"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    let mut text = format!(
        "{marker} {}  {}",
        node["id"].as_str().unwrap_or_default(),
        clean(node["preview"].as_str().unwrap_or_default())
    );
    if !labels.is_empty() {
        text.push_str(&format!("  [{}]", labels.join(", ")));
    }
    if node["tip"] == true && node["active"] == true {
        text.push_str("  (active)");
    }
    text
}
