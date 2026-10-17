//! Performing [`Action`]s through the API and turning results into messages.

use cyber_client::Client;
use cyber_core::agent_mentions::parse as leading_agent_mention;
use serde_json::{Value, json};

use crate::app::Action;
use crate::model::{Choice, Item, Queued, Request, Session};

/// Results the runner applies to the App.
#[derive(Debug, Clone)]
pub enum Msg {
    Memory {
        request: crate::memory::Request,
        result: Result<crate::memory::Data, String>,
    },
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
        result: Result<crate::settings::cost::Cost, String>,
    },
    /// A file explorer level, for the Session and directory it was read in.
    Dir {
        session_id: String,
        directory: String,
        path: String,
        result: Result<(crate::explorer::Level, Option<usize>), String>,
    },
    /// The explorer filter's search hits.
    FileHits {
        session_id: String,
        query: String,
        hits: Vec<crate::explorer::Node>,
    },
    /// A file preview, or why it was refused.
    Preview {
        session_id: String,
        path: String,
        result: Result<crate::explorer::Preview, String>,
    },
    /// The Settings Status tab's sources, read for this Session.
    Status {
        session_id: String,
        generation: u64,
        status: crate::settings::status::Status,
    },
    /// A Stats tab reply; it is not tied to a Session.
    Stats {
        generation: u64,
        result: Result<crate::settings::stats::Stats, String>,
    },
    Recap {
        session_id: String,
        manual: bool,
        result: Result<Value, String>,
    },
    Insights {
        generation: u64,
        result: Result<crate::settings::insights::Insights, String>,
    },
    Limits {
        generation: u64,
        result: Result<Vec<crate::settings::limits::Reading>, String>,
    },
    HookDefinitions {
        session_id: String,
        directory: String,
        generation: u64,
        result: Result<crate::hook_definitions::Catalog, String>,
    },
    HookHistory {
        session_id: String,
        generation: u64,
        result: Result<crate::hooks::Page, String>,
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
    Catalog(crate::model::Catalog),
    Models(Vec<Choice>),
    Skills {
        directory: String,
        result: Result<Vec<Choice>, String>,
    },
    Commands {
        items: Vec<Choice>,
        /// The same commands with their source, for the palette.
        palette: Vec<crate::palette::ServerCommand>,
        /// `review` resolves to the bundled skill rather than an override.
        bundled_review: bool,
    },
    /// The server refused a command before admission; nothing runs.
    Refused {
        command: String,
        message: String,
    },
    /// The awaited review's delegation record.
    ReviewDelegation(Value),
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
    /// A sidebar source answered, for the Session and directory it was read in.
    Sidebar {
        session_id: String,
        directory: String,
        loaded: crate::sidebar::Loaded,
    },
    /// A Workflow monitor reply.
    Workflow(crate::workflows::Reply),
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

/// A sidebar source; 404 or 503 omits its section and any other failure keeps the last
/// value.
pub async fn sidebar(client: &Client, session: &Session, part: crate::sidebar::Part) -> Msg {
    use crate::sidebar::{Loaded, Part, clean, parse_files, parse_lsp, parse_mcp, parse_vcs};
    let at = client.at(&session.directory);
    let loaded = match part {
        Part::Mcp => source(&at, "/mcp", |s| Loaded::Mcp(s.map(parse_mcp))).await,
        Part::Lsp => source(&at, "/lsp", |s| Loaded::Lsp(s.map(parse_lsp))).await,
        Part::Vcs => source(&at, "/vcs", |s| Loaded::Vcs(s.map(parse_vcs))).await,
        Part::Health => health(&at).await,
        Part::Machine => {
            match source(&at, "/system", |s| {
                Loaded::Machine(s.map(crate::sidebar::parse_machine))
            })
            .await
            {
                Loaded::Failed => Loaded::MachineFailed,
                loaded => loaded,
            }
        }
        Part::Files => {
            let path = format!("/sessions/{}/diff", encode(&session.id));
            source(&at, &path, |s| Loaded::Files(s.map(parse_files))).await
        }
        Part::Parent => match &session.parent_id {
            Some(parent) => {
                let path = format!("/sessions/{}", encode(parent));
                let title = |data: &Value| data["title"].as_str().map(clean);
                source(&at, &path, |s| Loaded::Parent(s.ready().and_then(title))).await
            }
            None => Loaded::Parent(None),
        },
    };
    Msg::Sidebar {
        session_id: session.id.clone(),
        directory: session.directory.clone(),
        loaded,
    }
}

/// Read `path` and turn its `data` (or a 404/503 as `Missing`) into a sidebar part.
async fn source(
    at: &Client,
    path: &str,
    wrap: impl FnOnce(crate::sidebar::Source<Value>) -> crate::sidebar::Loaded,
) -> crate::sidebar::Loaded {
    use crate::sidebar::Source;
    match at.get(path).await {
        Ok(mut value) => wrap(Source::Ready(value["data"].take())),
        Err(cyber_client::ClientError::Api {
            status: 404 | 503, ..
        }) => wrap(Source::Missing),
        Err(_) => crate::sidebar::Loaded::Failed,
    }
}

/// The server version from `/health`.
async fn health(at: &Client) -> crate::sidebar::Loaded {
    match at.get("/health").await {
        Ok(value) => crate::sidebar::Loaded::Version(
            value["version"]
                .as_str()
                .map(crate::sidebar::clean)
                .filter(|v| !v.is_empty()),
        ),
        Err(_) => crate::sidebar::Loaded::Failed,
    }
}

async fn sidebar_action(client: &Client, session: &Session, action: Action) -> Result<Msg, String> {
    match action {
        Action::ReconnectMcp(name) => reconnect_mcp(client, session, &name).await,
        Action::LoadSidebar(part) => Ok(sidebar(client, session, part).await),
        _ => Ok(Msg::Done),
    }
}

/// A Settings tab or file explorer read.
async fn panel(client: &Client, session: &Session, action: Action) -> Msg {
    match action {
        Action::LoadCost { generation } => load_cost(client, session, generation).await,
        Action::LoadStats {
            generation,
            period,
            project,
            zone,
        } => Msg::Stats {
            generation,
            result: load_stats(client, session, period, project, zone.as_deref()).await,
        },
        Action::Recap { session_id, manual } => {
            let path = format!("/sessions/{}/recap", encode(&session_id));
            let result = client
                .request("POST", &path, None)
                .await
                .map(|value| value["data"].clone())
                .map_err(recap_error);
            Msg::Recap {
                session_id,
                manual,
                result,
            }
        }
        Action::LoadInsights { generation, window } => Msg::Insights {
            generation,
            result: load_insights(client, window).await,
        },
        Action::LoadLimits { generation } => Msg::Limits {
            generation,
            result: load_limits(client).await,
        },
        Action::LoadStatus { generation } => Msg::Status {
            session_id: session.id.clone(),
            generation,
            status: status(client, session).await,
        },
        action => explore(client, session, action).await,
    }
}

/// A file explorer read: a level, a search or a preview, with the Session's rules.
async fn explore(client: &Client, session: &Session, action: Action) -> Msg {
    use crate::explorer::{parse_hits, parse_level, parse_preview};
    let at = client.at(&session.directory);
    let id = encode(&session.id);
    let session_id = session.id.clone();
    match action {
        Action::LoadDir { path, hidden } => {
            let url = format!(
                "/files?path={}&hidden={hidden}&session_id={id}",
                encode(&path)
            );
            let result = at.get(&url).await.map_err(err);
            Msg::Dir {
                session_id,
                directory: session.directory.clone(),
                path,
                result: result.map(|v| parse_level(&v["data"])),
            }
        }
        Action::SearchFiles(query) => {
            let url = format!(
                "/files/search?query={}&limit=50&session_id={id}",
                encode(&query)
            );
            let hits = at
                .get(&url)
                .await
                .map_or_else(|_| Vec::new(), |v| parse_hits(&v["data"]));
            Msg::FileHits {
                session_id,
                query,
                hits,
            }
        }
        Action::LoadPreview(path) => {
            let url = format!("/files/content?path={}&session_id={id}", encode(&path));
            let result = at.get(&url).await.map_err(refusal);
            Msg::Preview {
                session_id,
                result: result.map(|v| parse_preview(&path, &v["data"])),
                path,
            }
        }
        _ => Msg::Done,
    }
}

/// Why the server refused a preview, in words.
fn refusal(error: cyber_client::ClientError) -> String {
    match &error {
        cyber_client::ClientError::Api { status: 403, .. }
            if error.to_string().contains("outside") =>
        {
            "outside the project".into()
        }
        cyber_client::ClientError::Api { status: 403, .. } => "permission denied".into(),
        _ => err(error),
    }
}

/// The Status tab's sources, read together; each one fails or is missing on its own.
pub async fn status(client: &Client, session: &Session) -> crate::settings::status::Status {
    use crate::settings::status::{Health, Status};
    use crate::sidebar::{parse_lsp, parse_mcp, parse_vcs};
    let at = client.at(&session.directory);
    let session_path = format!("/sessions/{}", encode(&session.id));
    let (health, current, vcs, mcp, lsp, machine) = futures::join!(
        section(&at, "/health", Health::parse),
        section(&at, &session_path, |v| Session::parse(&v["data"])),
        section(&at, "/vcs", |v| parse_vcs(&v["data"])),
        section(&at, "/mcp", |v| parse_mcp(&v["data"])),
        section(&at, "/lsp", |v| parse_lsp(&v["data"])),
        section(&at, "/system", |v| Box::new(crate::sidebar::parse_machine(
            &v["data"]
        ))),
    );
    Status {
        health,
        session: current,
        vcs,
        mcp,
        lsp,
        machine,
    }
}

/// One Status source: its parsed reply, `Missing` on 404 or 503, else the error.
async fn section<T>(
    at: &Client,
    path: &str,
    parse: impl FnOnce(&Value) -> T,
) -> crate::settings::status::Section<T> {
    use crate::settings::status::Section;
    match at.get(path).await {
        Ok(value) => Section::Ready(parse(&value)),
        Err(cyber_client::ClientError::Api {
            status: 404 | 503, ..
        }) => Section::Missing,
        Err(error) => Section::Failed(err(error)),
    }
}

async fn load_cost(client: &Client, session: &Session, generation: u64) -> Msg {
    let result = client
        .get(&format!("/usage?scope=session&id={}", encode(&session.id)))
        .await
        .map_err(err)
        .and_then(|value| crate::settings::cost::Cost::parse_report(&value["data"], &session.id));
    Msg::Cost {
        session_id: session.id.clone(),
        generation,
        result,
    }
}

/// `GET /usage/stats` for the period, the TUI's zone and, with `project`, the Location.
async fn load_stats(
    client: &Client,
    session: &Session,
    period: &str,
    project: bool,
    zone: Option<&str>,
) -> Result<crate::settings::stats::Stats, String> {
    let mut path = format!("/usage/stats?period={}", encode(period));
    if let Some(zone) = zone {
        path.push_str(&format!("&tz={}", encode(zone)));
    }
    if project && !session.directory.is_empty() {
        path.push_str(&format!("&directory={}", encode(&session.directory)));
    }
    let value = client.get(&path).await.map_err(err)?;
    crate::settings::stats::Stats::parse(&value["data"])
}

/// `409 nothing_to_recap` in words; any other failure as the client reports it.
fn recap_error(e: cyber_client::ClientError) -> String {
    match &e {
        cyber_client::ClientError::Api { status: 409, .. } => "nothing to recap yet".into(),
        _ => err(e),
    }
}

/// `GET /usage/insights` for the window, over every Session the server stored.
async fn load_insights(
    client: &Client,
    window: &str,
) -> Result<crate::settings::insights::Insights, String> {
    let value = client
        .get(&format!("/usage/insights?window={}", encode(window)))
        .await
        .map_err(err)?;
    crate::settings::insights::Insights::parse(&value["data"])
}

/// `GET /usage/limits`; a server without the route reports no windows.
async fn load_limits(client: &Client) -> Result<Vec<crate::settings::limits::Reading>, String> {
    match client.get("/usage/limits").await {
        Ok(value) => crate::settings::limits::parse(&value["data"]),
        Err(cyber_client::ClientError::Api {
            status: 404 | 503, ..
        }) => Ok(Vec::new()),
        Err(e) => Err(err(e)),
    }
}

/// Reconnect a failed MCP server, then read the statuses again.
async fn reconnect_mcp(client: &Client, session: &Session, name: &str) -> Result<Msg, String> {
    // Server names come from the server; anything but a plain name never reaches a path.
    if name.is_empty()
        || !name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
        return Err(format!("reconnect: invalid MCP server name {name:?}"));
    }
    let at = client.at(&session.directory);
    // The route takes no body.
    at.request("POST", &format!("/mcp/{}/reconnect", encode(name)), None)
        .await
        .map_err(|e| format!("reconnect {name}: {}", err(e)))?;
    Ok(sidebar(client, session, crate::sidebar::Part::Mcp).await)
}

pub(crate) fn err(e: cyber_client::ClientError) -> String {
    e.to_string()
}

async fn hook_definitions(
    client: &Client,
    session: &Session,
    generation: u64,
    change: Option<(String, bool)>,
) -> Result<Msg, String> {
    let result = async {
        if let Some((digest, approve)) = change {
            client
                .post(
                    if approve {
                        "/hooks/trust"
                    } else {
                        "/hooks/untrust"
                    },
                    json!({"digest":digest}),
                )
                .await
                .map_err(err)?;
        }
        let value = client.get("/hooks").await.map_err(err)?;
        crate::hook_definitions::Catalog::parse(&value)
    }
    .await;
    Ok(Msg::HookDefinitions {
        session_id: session.id.clone(),
        directory: session.directory.clone(),
        generation,
        result,
    })
}

async fn hook_history(
    client: &Client,
    session: &Session,
    generation: u64,
    cursor: Option<String>,
) -> Result<Msg, String> {
    let cursor = cursor
        .as_deref()
        .map(|value| format!("&cursor={}", encode(value)))
        .unwrap_or_default();
    let result = client
        .get(&format!(
            "/sessions/{}/hook-executions?limit=50{cursor}",
            encode(&session.id)
        ))
        .await
        .map_err(err)
        .and_then(|value| crate::hooks::Page::parse(&value, &session.id));
    Ok(Msg::HookHistory {
        session_id: session.id.clone(),
        generation,
        result,
    })
}

async fn submit_prompt(
    client: &Client,
    session: &Session,
    text: &str,
    delivery: &str,
    context: Option<&crate::admissions::Context<'_>>,
) -> Result<Msg, String> {
    let id = session.id.as_str();
    if let Some((agent, prompt)) = leading_agent_mention(text) {
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
    let parts = parts(text, &session.directory);
    post(
        client,
        &format!("/sessions/{id}/prompt"),
        json!({ "parts": parts, "delivery": delivery }),
    )
    .await
}

/// Posts a command, with a client-chosen message ID when one is given.
async fn command(
    client: &Client,
    session: &str,
    name: String,
    arguments: String,
    message: Option<String>,
) -> Result<Msg, String> {
    let mut body = json!({ "name": name, "arguments": arguments });
    if let Some(message) = message {
        body["id"] = message.into();
    }
    match client
        .post(&format!("/sessions/{session}/command"), body)
        .await
    {
        Ok(_) => Ok(Msg::Done),
        Err(error) => refused(name, error),
    }
}

/// The awaited review's delegation record, which names its reviewer Job.
async fn review_delegation(
    client: &Client,
    session: &str,
    delegation: &str,
) -> Result<Msg, String> {
    let record = client
        .get(&format!("/sessions/{session}/delegations/{delegation}"))
        .await
        .map_err(err)?;
    Ok(Msg::ReviewDelegation(record["data"].clone()))
}

/// A command the server did not admit. A permission denial or review target
/// error is a refusal; any failure of a `review` post is too, so the review
/// is not left awaiting a Job that will never exist.
fn refused(command: String, error: cyber_client::ClientError) -> Result<Msg, String> {
    match error {
        cyber_client::ClientError::Api {
            status, message, ..
        } if status == 403 || status == 422 => Ok(Msg::Refused { command, message }),
        error if command == "review" => Ok(Msg::Refused {
            command,
            message: error.to_string(),
        }),
        error => Err(err(error)),
    }
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
        Action::Memory(request) => Ok(crate::memory::perform(client, request).await),
        Action::ReconcileAdmission(_) | Action::Admission { .. } => {
            crate::admissions::perform_action(client, action).await
        }
        Action::Refresh => snapshot(client, &session.id).await,
        Action::LoadSidebar(_) | Action::ReconnectMcp(_) => {
            sidebar_action(client, session, action).await
        }
        Action::LoadHookDefinitions { .. }
        | Action::ChangeHookTrust { .. }
        | Action::LoadHookHistory { .. } => inspect_hooks(client, session, action).await,
        Action::LoadCost { .. }
        | Action::LoadStatus { .. }
        | Action::LoadStats { .. }
        | Action::LoadInsights { .. }
        | Action::LoadLimits { .. }
        | Action::Recap { .. }
        | Action::LoadDir { .. }
        | Action::SearchFiles(_)
        | Action::LoadPreview(_) => Ok(panel(client, session, action).await),
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
        Action::LoadSessions
        | Action::Rename { .. }
        | Action::Archive(_)
        | Action::Delete(_)
        | Action::ForkSession(_) => manage(client, action).await,
        Action::LoadModels
        | Action::LoadSkills
        | Action::LoadCommands
        | Action::LoadAgents
        | Action::FindFiles(_) => load_choices(client, session, action).await,
        Action::Workflow(request) => Ok(Msg::Workflow(
            crate::workflows::perform(client, request).await,
        )),
        Action::SaveTheme(_)
        | Action::SaveProjectTheme(_)
        | Action::SavePreference { .. }
        | Action::Copy(_)
        | Action::Editor(_)
        | Action::EditFile(_)
        | Action::Quit => Ok(Msg::Done),
        other => converse(client, session, other, context).await,
    }
}

async fn inspect_hooks(client: &Client, session: &Session, action: Action) -> Result<Msg, String> {
    match action {
        Action::LoadHookDefinitions { generation } => {
            hook_definitions(client, session, generation, None).await
        }
        Action::ChangeHookTrust {
            generation,
            digest,
            approve,
        } => hook_definitions(client, session, generation, Some((digest, approve))).await,
        Action::LoadHookHistory { generation, cursor } => {
            hook_history(client, session, generation, cursor).await
        }
        _ => Err("Unsupported hook inspection action".into()),
    }
}

async fn load_choices(client: &Client, session: &Session, action: Action) -> Result<Msg, String> {
    match action {
        Action::LoadModels => models(client).await,
        Action::LoadSkills => {
            let path = format!("/skills?agent={}", encode(&session.agent));
            let result = client
                .get(&path)
                .await
                .map(|body| skill_choices(&body["data"]))
                .map_err(err);
            Ok(Msg::Skills {
                directory: session.directory.clone(),
                result,
            })
        }
        Action::LoadCommands => commands(client).await,
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
        _ => Err("Unsupported choice-list action".into()),
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
            submit_prompt(client, session, &text, delivery, context).await
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
        Action::ApproveAuto => post(client, &format!("/sessions/{id}/approve"), json!({})).await,
        Action::Shell(command) => {
            post(
                client,
                &format!("/sessions/{id}/shell"),
                json!({ "command": command }),
            )
            .await?;
            snapshot(client, id).await
        }
        Action::Command {
            name,
            arguments,
            id: message,
        } => command(client, id, name, arguments, message).await,
        Action::ReviewDelegation {
            session,
            delegation,
        } => review_delegation(client, &session, &delegation).await,
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
async fn fork_session(client: &Client, id: &str) -> Result<Msg, String> {
    switched(
        client
            .post(&format!("/sessions/{id}/fork"), json!({}))
            .await,
    )
}

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
        Action::ForkSession(id) => return fork_session(client, &id).await,
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

fn printable(text: &str) -> String {
    text.chars().filter(|c| !c.is_control()).collect()
}

/// `GET /skills` rows: invalid definitions are listed with their diagnostics and no key.
pub(crate) fn skill_choices(catalog: &Value) -> Vec<Choice> {
    let field = |entry: &Value, key: &str| printable(entry[key].as_str().unwrap_or(""));
    catalog["skills"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|entry| {
            let loaded = entry["status"] == "loaded";
            let invocable = loaded && entry["user_invocable"] == true;
            let mut detail = vec![
                field(entry, "scope"),
                entry["permission"].as_str().unwrap_or("skipped").into(),
                field(entry, "source"),
            ];
            detail.extend(
                entry["diagnostics"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .map(printable),
            );
            Choice {
                key: if invocable {
                    field(entry, "name")
                } else {
                    String::new()
                },
                label: if loaded {
                    format!("{} — {}", field(entry, "name"), field(entry, "description"))
                } else {
                    format!("{} (invalid)", field(entry, "name"))
                },
                detail: detail.join(" · "),
            }
        })
        .collect()
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
    let entries = list["data"].as_array().map_or(&[][..], Vec::as_slice);
    let bundled_review = entries
        .iter()
        .any(|c| c["name"] == "review" && c["source"] == "builtin");
    let items = entries
        .iter()
        .map(|c| Choice {
            key: c["name"].as_str().unwrap_or_default().into(),
            label: c["name"].as_str().unwrap_or_default().into(),
            detail: c["description"].as_str().unwrap_or_default().into(),
        })
        .collect();
    let palette = entries
        .iter()
        .map(|c| crate::palette::ServerCommand {
            name: c["name"].as_str().unwrap_or_default().into(),
            description: c["description"].as_str().unwrap_or_default().into(),
            source: c["source"].as_str().unwrap_or_default().into(),
            takes_arguments: c["argument_hint"]
                .as_str()
                .is_some_and(|hint| !hint.trim().is_empty()),
        })
        .collect();
    Ok(Msg::Commands {
        items,
        palette,
        bundled_review,
    })
}

/// Model display names and agent colors for the composer and the Turn marker.
pub async fn catalog(client: &Client) -> Result<Msg, String> {
    let models = client.get("/models").await.map_err(err)?;
    let agents = client.get("/agents").await.map_err(err)?;
    Ok(Msg::Catalog(crate::model::Catalog::parse(
        &models["data"],
        &agents["data"],
    )))
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
/// `@server:uri` is an MCP resource mention the server resolves, never a file.
pub fn parts(text: &str, directory: &str) -> Vec<Value> {
    let mut parts = vec![json!({ "type": "text", "text": text })];
    for token in text
        .split_whitespace()
        .filter_map(|w| w.strip_prefix('@'))
        .filter(|token| !resource_mention(token))
    {
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

/// A server-name-like prefix followed by `:` (this also keeps `@C:\x` from being read
/// as a Windows path or alternate data stream).
fn resource_mention(token: &str) -> bool {
    token.split_once(':').is_some_and(|(server, _)| {
        (1..=48).contains(&server.len())
            && server
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    })
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

    /// `@server:uri` is an MCP resource mention the server resolves, never a file, even
    /// when a path of that spelling exists (it also covers `@C:\x` on Windows).
    #[test]
    fn resource_mentions_are_not_read_as_files() {
        let tmp = tempfile::tempdir().unwrap();
        // Windows file names cannot contain `:`, so the colliding file exists only on Unix.
        #[cfg(unix)]
        {
            std::fs::create_dir_all(tmp.path().join("remote:file:")).unwrap();
            std::fs::write(tmp.path().join("remote:file:/guide.md"), "LOCAL FILE").unwrap();
        }
        std::fs::write(tmp.path().join("notes.txt"), "notes").unwrap();
        let dir = tmp.path().display().to_string();
        let parts = parts("@remote:file:///guide.md and @notes.txt", &dir);
        assert_eq!(parts.len(), 2, "{parts:?}");
        assert_eq!(
            parts[1]["text"],
            "<file path=\"notes.txt\">\nnotes\n</file>"
        );
        assert!(!format!("{parts:?}").contains("LOCAL FILE"));
    }
}
