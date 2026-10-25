//! `/agents`: the directory of every Session and agent the attached Session can reach by
//! message (`tui` → Agent directory). Rows come from a list of sources, each with its own
//! loader and last result: the messaging listing (`GET /messaging/sessions?from=`) and,
//! when the Session is bound to a project, the project hub's People. Another transport
//! adds a [`Source`], its loader and its field on [`Directory`]; rows, selection and
//! filtering read every source through [`Source::ALL`].
//!
//! Peer and hub text comes from other machines and people: every string is cleaned, and
//! an address that cleaning changes is never sent to (the Session ID is, or nothing).

use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use cyber_client::{Client, ClientError};
use serde_json::{Value, json};

use crate::app::{Action, App, Overlay};
use crate::collaboration::{Snapshot, text};
use crate::messaging::{MESSAGE_LIMIT, clean, clean_line, outcome_label, push_within};
use crate::model::Session;
use crate::perform::{Msg, err};
use crate::workflows::encode;

/// How often the open directory reads its sources again.
const POLL: Duration = Duration::from_secs(10);
/// The longest title the server keeps, in characters.
const TITLE_LIMIT: usize = 100;

/// Where rows come from, in the order their groups are drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// `GET /messaging/sessions?from=`: this server's Sessions and direct peers.
    Messaging,
    /// The project hub's active agents and members.
    Hub,
}

impl Source {
    pub const ALL: [Source; 2] = [Source::Messaging, Source::Hub];
}

/// What a row is to the attached Session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Kind {
    Subagent,
    WorkflowAgent,
    Teammate,
    /// Another root Session on this server.
    Session,
    Peer,
    HubAgent,
    HubMember,
    /// A kind this build does not know, cleaned.
    Other(String),
}

impl Kind {
    pub fn label(&self) -> &str {
        match self {
            Kind::Subagent => "subagent",
            Kind::WorkflowAgent => "workflow agent",
            Kind::Teammate => "teammate",
            Kind::Session => "session",
            Kind::Peer => "peer",
            Kind::HubAgent => "hub agent",
            Kind::HubMember => "hub member",
            Kind::Other(kind) => kind,
        }
    }

    /// A local Session the TUI can open and rename.
    fn local(&self) -> bool {
        matches!(
            self,
            Kind::Subagent | Kind::WorkflowAgent | Kind::Teammate | Kind::Session
        )
    }
}

/// What a row is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Idle,
    Running,
    WaitingInput,
    Offline,
    Online,
    Unknown,
}

impl State {
    fn parse(state: &str) -> Self {
        match state {
            "idle" => State::Idle,
            "running" => State::Running,
            "waiting_input" => State::WaitingInput,
            "offline" => State::Offline,
            "online" => State::Online,
            _ => State::Unknown,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            State::Idle => "idle",
            State::Running => "running",
            State::WaitingInput => "waiting_input",
            State::Offline => "offline",
            State::Online => "online",
            State::Unknown => "-",
        }
    }
}

/// Where a row runs, cleaned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Location {
    /// A directory on this server.
    Local(String),
    /// A peer, and the directory or workspace name it shared.
    Peer { peer: String, place: String },
    /// A hub project.
    Hub(String),
}

impl Location {
    pub fn label(&self) -> String {
        match self {
            Location::Local(directory) => directory.clone(),
            Location::Peer { peer, place } if place.is_empty() => format!("peer:{peer}"),
            Location::Peer { peer, place } => format!("peer:{peer} {place}"),
            Location::Hub(project) => format!("hub:{project}"),
        }
    }
}

/// The group a row is drawn under.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Group {
    Server,
    Peers,
    Hub,
}

impl Group {
    pub const ALL: [Group; 3] = [Group::Server, Group::Peers, Group::Hub];

    pub fn title(self) -> &'static str {
        match self {
            Group::Server => "This server",
            Group::Peers => "Peers",
            Group::Hub => "Project hub",
        }
    }
}

/// One reachable Session or agent.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    /// Stable across reloads: what the selection follows.
    pub key: String,
    pub source: Source,
    pub kind: Kind,
    /// The address shown, cleaned.
    pub address: String,
    /// What actions send to: the wire address when cleaning leaves it unchanged, else
    /// the Session ID under the same rule; `None` sends nothing.
    pub target: Option<String>,
    pub name: String,
    pub state: State,
    pub can_receive: bool,
    pub location: Location,
    /// A Session of this server: what open and rename use. Never set for a peer row.
    pub session_id: Option<String>,
    /// A hub row's `{kind, id}`, as the hub gave it.
    pub participant: Option<Value>,
}

/// `text` when it has nothing [`clean`] removes.
fn safe(text: &str) -> Option<String> {
    (!text.is_empty() && clean(text) == text).then(|| text.to_owned())
}

impl Entry {
    /// An entry of `GET /messaging/sessions`. `remote` is read as `peer`, an unknown kind
    /// is kept as text, and a subagent with a `workflowRunId` is a workflow agent.
    pub fn from_listing(value: &Value) -> Self {
        let field = |key: &str| value[key].as_str().unwrap_or_default();
        let (address, session_id) = (field("address"), field("session_id"));
        let kind = match field("kind") {
            "subagent" if value["workflowRunId"].as_str().is_some() => Kind::WorkflowAgent,
            "subagent" => Kind::Subagent,
            "teammate" => Kind::Teammate,
            "local" => Kind::Session,
            "peer" | "remote" => Kind::Peer,
            other => Kind::Other(clean(other)),
        };
        let directory = value["location"]["directory"].as_str().unwrap_or_default();
        let location = if kind == Kind::Peer {
            let workspace = value["location"]["workspace"].as_str();
            let peer = session_id.rsplit_once('@').map_or("", |(_, peer)| peer);
            Location::Peer {
                peer: clean(peer),
                place: clean(workspace.unwrap_or(directory)),
            }
        } else {
            Location::Local(clean(directory))
        };
        let local =
            kind != Kind::Peer && session_id.starts_with("ses_") && !session_id.contains('@');
        Entry {
            key: format!("listing:{}", clean(session_id)),
            source: Source::Messaging,
            target: safe(address).or_else(|| safe(session_id)),
            address: clean(address),
            name: clean(field("name")),
            state: State::parse(field("state")),
            can_receive: value["canReceive"] == true,
            location,
            session_id: safe(session_id).filter(|_| local),
            participant: None,
            kind,
        }
    }

    /// A hub agent or member of `roster`'s project (`/collaboration/participants`). An
    /// agent is `online` or `offline`; a member has no presence on the hub, so its
    /// state is unknown (`-`).
    fn from_hub(roster: &Snapshot, value: &Value, agent: bool, project: &str) -> Self {
        let id = value["id"].as_str().unwrap_or_default();
        let name = clean(value["name"].as_str().unwrap_or(id));
        let guest = roster.guest(value, agent);
        let (kind, state, detail) = if agent {
            let online = if value["online"] == true {
                State::Online
            } else {
                State::Offline
            };
            let detail = if guest { " (guest)" } else { "" };
            (Kind::HubAgent, online, detail.to_owned())
        } else {
            let role = text(value, "role");
            let guest = if guest { ", guest" } else { "" };
            (Kind::HubMember, State::Unknown, format!(" ({role}{guest})"))
        };
        let participant = if agent { "agent" } else { "human" };
        Entry {
            key: format!("hub:{participant}:{}", clean(id)),
            source: Source::Hub,
            address: name.clone(),
            target: None,
            name: format!("{name}{detail}"),
            state,
            can_receive: true,
            location: Location::Hub(clean(project)),
            session_id: None,
            participant: Some(json!({"kind": participant, "id": id})),
            kind,
        }
    }

    pub fn group(&self) -> Group {
        match (self.source, &self.kind) {
            (Source::Hub, _) => Group::Hub,
            (_, Kind::Peer) => Group::Peers,
            _ => Group::Server,
        }
    }

    fn matches(&self, filter: &str) -> bool {
        if filter.is_empty() {
            return true;
        }
        let filter = filter.to_lowercase();
        [
            self.address.as_str(),
            self.name.as_str(),
            self.kind.label(),
            &self.location.label(),
        ]
        .iter()
        .any(|text| text.to_lowercase().contains(&filter))
    }
}

/// The kind filter, cycled with Tab.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum KindFilter {
    #[default]
    All,
    Sessions,
    Subagents,
    Teammates,
    Peers,
    Hub,
}

impl KindFilter {
    pub const ALL: [KindFilter; 6] = [
        KindFilter::All,
        KindFilter::Sessions,
        KindFilter::Subagents,
        KindFilter::Teammates,
        KindFilter::Peers,
        KindFilter::Hub,
    ];

    pub fn label(self) -> &'static str {
        match self {
            KindFilter::All => "all",
            KindFilter::Sessions => "sessions",
            KindFilter::Subagents => "subagents",
            KindFilter::Teammates => "teammates",
            KindFilter::Peers => "peers",
            KindFilter::Hub => "hub",
        }
    }

    fn step(self, forward: bool) -> Self {
        let at = Self::ALL.iter().position(|f| *f == self).unwrap_or(0);
        let n = Self::ALL.len();
        Self::ALL[if forward {
            (at + 1) % n
        } else {
            (at + n - 1) % n
        }]
    }

    fn admits(self, entry: &Entry) -> bool {
        match self {
            KindFilter::All => true,
            KindFilter::Sessions => matches!(entry.kind, Kind::Session | Kind::Other(_)),
            KindFilter::Subagents => matches!(entry.kind, Kind::Subagent | Kind::WorkflowAgent),
            KindFilter::Teammates => entry.kind == Kind::Teammate,
            KindFilter::Peers => entry.kind == Kind::Peer,
            KindFilter::Hub => entry.source == Source::Hub,
        }
    }
}

/// What a source last answered besides its rows.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Status {
    /// Not read yet.
    #[default]
    Unread,
    Ready,
    /// Not configured here (the hub off or unbound): no group and no error.
    Inactive,
    /// Messaging is disabled for the Location (503).
    Disabled,
    /// The server has no such route (an older build).
    Unsupported,
}

/// A source's answer.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Fetched {
    pub rows: Vec<Entry>,
    pub status: Status,
    /// Hub only: the current member may send (`owner` or `member`).
    pub writable: bool,
}

impl Fetched {
    fn status(status: Status) -> Self {
        Self {
            status,
            ..Self::default()
        }
    }
}

/// A source's last result.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Loaded {
    pub rows: Vec<Entry>,
    pub status: Status,
    pub error: Option<String>,
    pub loading: bool,
    pub writable: bool,
}

/// Who a compose editor sends to.
#[derive(Debug, Clone, PartialEq)]
pub enum Recipient {
    /// A listing row's target, through `POST /messaging/send`.
    Listing(String),
    /// A hub participant, as a direct hub message.
    Hub { participant: Value, project: String },
}

#[derive(Debug, Clone, PartialEq)]
pub enum EditorKind {
    Message { to: Recipient },
    Rename { id: String },
}

/// The compose or rename editor over the bottom of the directory.
#[derive(Debug, Clone, PartialEq)]
pub struct Editor {
    pub kind: EditorKind,
    /// The row's address as shown.
    pub label: String,
    pub input: String,
    /// `notify_when_idle` for a listing message (Ctrl+N).
    pub notify: bool,
    /// The request is in flight; the input is frozen until it answers.
    pub busy: bool,
    /// The ID of the action this editor submitted last: only its answer closes or
    /// annotates this editor.
    op: Option<u64>,
    pub error: Option<String>,
}

impl Editor {
    fn new(kind: EditorKind, label: &str, input: String) -> Self {
        Self {
            kind,
            label: label.to_owned(),
            input,
            notify: false,
            busy: false,
            op: None,
            error: None,
        }
    }

    /// A message is bounded in UTF-8 bytes, as the routes count; a title in characters,
    /// as the server keeps it.
    fn push(&mut self, c: char) {
        match self.kind {
            EditorKind::Message { .. } => {
                push_within(&mut self.input, c, MESSAGE_LIMIT);
            }
            EditorKind::Rename { .. } => {
                if self.input.chars().count() < TITLE_LIMIT {
                    self.input.push(c);
                }
            }
        }
    }

    pub fn multiline(&self) -> bool {
        matches!(self.kind, EditorKind::Message { .. })
    }
}

/// The directory's sources, filter, selection and editor; kept on `App`.
#[derive(Debug, Default)]
pub struct Directory {
    pub messaging: Loaded,
    pub hub: Loaded,
    pub filter: String,
    pub kind: KindFilter,
    /// The selected row's key, kept across reloads and filters.
    pub selected: Option<String>,
    /// The selected row's index, where the selection lands when its row goes.
    selected_at: usize,
    pub editor: Option<Editor>,
    generation: u64,
    /// The last action ID handed out.
    ops: u64,
    last_poll: Option<Instant>,
}

impl Directory {
    pub fn source(&self, source: Source) -> &Loaded {
        match source {
            Source::Messaging => &self.messaging,
            Source::Hub => &self.hub,
        }
    }

    fn source_mut(&mut self, source: Source) -> &mut Loaded {
        match source {
            Source::Messaging => &mut self.messaging,
            Source::Hub => &mut self.hub,
        }
    }

    /// Every row, grouped, before filtering.
    pub fn rows(&self) -> impl Iterator<Item = &Entry> {
        let all = || Source::ALL.into_iter().flat_map(|s| &self.source(s).rows);
        Group::ALL
            .into_iter()
            .flat_map(move |group| all().filter(move |e| e.group() == group))
    }

    /// The rows shown: grouped, under the kind filter and the typed filter.
    pub fn visible(&self) -> Vec<&Entry> {
        self.rows()
            .filter(|e| self.kind.admits(e) && e.matches(&self.filter))
            .collect()
    }

    /// The selected row's index in [`Self::visible`]; where it was when its row went.
    pub fn selected_index(&self, visible: &[&Entry]) -> Option<usize> {
        let last = visible.len().checked_sub(1)?;
        let by_key = self
            .selected
            .as_deref()
            .and_then(|key| visible.iter().position(|e| e.key == key));
        Some(by_key.unwrap_or(self.selected_at.min(last)))
    }

    pub fn current(&self) -> Option<&Entry> {
        let visible = self.visible();
        self.selected_index(&visible).map(|at| visible[at])
    }

    /// Move the selection by `delta` rows, clamped.
    pub fn select(&mut self, delta: isize) {
        let visible = self.visible();
        let Some(at) = self.selected_index(&visible) else {
            return;
        };
        let at = at.saturating_add_signed(delta).min(visible.len() - 1);
        self.selected = Some(visible[at].key.clone());
        self.selected_at = at;
    }

    /// Select row `index` of [`Self::visible`]; `false` past the end.
    pub fn select_index(&mut self, index: usize) -> bool {
        let Some(key) = self.visible().get(index).map(|e| e.key.clone()) else {
            return false;
        };
        self.selected = Some(key);
        self.selected_at = index;
        true
    }

    /// Follow the selected row after the rows changed: its key when it is still shown,
    /// else the row now at its index.
    fn anchor(&mut self) {
        let visible = self.visible();
        let Some(at) = self.selected_index(&visible) else {
            return;
        };
        self.selected = Some(visible[at].key.clone());
        self.selected_at = at;
    }

    /// A Session switch: everything goes, and replies already on their way are dropped.
    pub fn forget(&mut self) {
        let (generation, ops) = (self.generation.wrapping_add(1), self.ops);
        *self = Self {
            generation,
            ops,
            ..Self::default()
        };
    }

    /// `op` under a new action ID.
    fn job(&mut self, op: Op) -> Job {
        self.ops = self.ops.wrapping_add(1);
        Job { id: self.ops, op }
    }

    fn loading(&self) -> bool {
        Source::ALL.iter().any(|s| self.source(*s).loading)
    }

    /// The listing answered and nothing at all is reachable.
    pub fn empty(&self) -> bool {
        self.rows().next().is_none()
            && !self.loading()
            && self.messaging.status == Status::Ready
            && Source::ALL.iter().all(|s| self.source(*s).error.is_none())
    }
}

/// A read of one source for the attached Session.
#[derive(Debug, Clone, PartialEq)]
pub struct Request {
    pub generation: u64,
    pub source: Source,
}

/// A row action, with the ID its answer comes back under.
#[derive(Debug, Clone, PartialEq)]
pub struct Job {
    pub id: u64,
    pub op: Op,
}

/// A row action through an existing route.
#[derive(Debug, Clone, PartialEq)]
pub enum Op {
    /// `POST /messaging/send` from the attached Session.
    Send {
        to: String,
        text: String,
        notify: bool,
    },
    /// `POST /collaboration/messages`, a direct message as the signed-in account.
    HubSend {
        participant: Value,
        project: String,
        text: String,
    },
    /// `POST /messaging/watch` from the attached Session.
    Watch { target: String },
    /// `PATCH /sessions/{id}`, the rename `/rename` and the Session picker use.
    Rename { id: String, title: String },
}

/// Why an action failed.
#[derive(Debug, Clone, PartialEq)]
pub struct Failure(pub String);

/// Read `request.source` for `session`.
pub async fn perform(client: &Client, session: &Session, request: Request) -> Result<Msg, String> {
    let result = match request.source {
        Source::Messaging => listing(client, session).await,
        Source::Hub => hub(client, session).await,
    };
    Ok(Msg::Directory {
        session_id: session.id.clone(),
        generation: request.generation,
        source: request.source,
        result: result.map_err(|e| clean(&e)),
    })
}

/// A missing route or a disabled feature is a state of the source, not an error.
fn unavailable(e: &ClientError) -> Option<Status> {
    match e {
        ClientError::Api { status: 503, .. } => Some(Status::Disabled),
        ClientError::Api { status: 501, .. } => Some(Status::Unsupported),
        e if e.route_missing() => Some(Status::Unsupported),
        _ => None,
    }
}

async fn listing(client: &Client, session: &Session) -> Result<Fetched, String> {
    let path = format!("/messaging/sessions?from={}", encode(&session.id));
    match client.at(&session.directory).get(&path).await {
        Ok(listing) => {
            let rows = listing["data"].as_array().into_iter().flatten();
            Ok(Fetched {
                rows: rows.map(Entry::from_listing).collect(),
                status: Status::Ready,
                writable: false,
            })
        }
        Err(e) => unavailable(&e).map(Fetched::status).ok_or_else(|| err(e)),
    }
}

/// The hub's People, only when collaboration is enabled and a project is bound.
async fn hub(client: &Client, session: &Session) -> Result<Fetched, String> {
    let mut roster = match crate::collaboration::status(client, session).await {
        Ok(roster) => roster,
        Err(e) if unavailable(&e).is_some() => return Ok(Fetched::status(Status::Inactive)),
        Err(e) => return Err(err(*e)),
    };
    if roster.bound().is_none() {
        return Ok(Fetched::status(Status::Inactive));
    }
    crate::collaboration::roster(client, session, &mut roster)
        .await
        .map_err(|e| err(*e))?;
    Ok(Fetched {
        rows: hub_rows(&roster),
        status: Status::Ready,
        writable: roster.writable(),
    })
}

/// The active agents and members of a bound roster, without the signed-in member and
/// the agent the attached Session is published as: neither is someone else to message.
fn hub_rows(roster: &Snapshot) -> Vec<Entry> {
    let project = roster.bound().unwrap_or_default();
    let me = &roster.status["current_member"]["id"];
    let mine = &roster.status["published"]["agent_id"];
    let other =
        |v: &&Value, own: &Value| v["active"] != false && (own.is_null() || v["id"] != *own);
    let agents = roster.agents.iter().filter(|v| other(v, mine));
    let members = roster.members.iter().filter(|v| other(v, me));
    agents
        .map(|v| Entry::from_hub(roster, v, true, project))
        .chain(members.map(|v| Entry::from_hub(roster, v, false, project)))
        .collect()
}

/// Run `job` from `session`; the reply's text is the outcome to show.
pub async fn perform_op(client: &Client, session: &Session, job: Job) -> Result<Msg, String> {
    let at = client.at(&session.directory);
    let result = match job.op {
        Op::Send { to, text, notify } => {
            let body =
                json!({"from": session.id, "to": to, "text": text, "notify_when_idle": notify});
            at.post("/messaging/send", body)
                .await
                .map(|v| sent(&v["data"], &to))
        }
        Op::HubSend {
            participant,
            project,
            text,
        } => {
            let body = hub_message(session, participant, project, text);
            at.post("/collaboration/messages", body)
                .await
                .map(|v| hub_sent(&v["data"]))
        }
        Op::Watch { target } => {
            let body = json!({"from": session.id, "target": target});
            at.post("/messaging/watch", body)
                .await
                .map(|v| watching(&v["data"], &target))
        }
        Op::Rename { id, title } => {
            let path = format!("/sessions/{}", encode(&id));
            let renamed = at.patch(&path, json!({"title": title})).await;
            renamed.map(|v| {
                format!(
                    "Renamed to {}",
                    clean(v["data"]["title"].as_str().unwrap_or(&title))
                )
            })
        }
    };
    Ok(Msg::DirectoryDone {
        session_id: session.id.clone(),
        id: job.id,
        result: result.map_err(failure),
    })
}

/// The `/collab` People direct-message body; the author is the signed-in account.
fn hub_message(session: &Session, participant: Value, project: String, text: String) -> Value {
    let body = json!({"target": {"kind": "direct", "participant": participant}, "text": text});
    crate::collaboration::stamp(body, &session.id, Some(&project), "tui-agents")
}

/// `delivered to migration · msgx_…`, `held by …` or `refused: <reason> · msgx_…`.
fn sent(result: &Value, to: &str) -> String {
    let (outcome, id) = (text(result, "outcome"), text(result, "message_id"));
    let label = outcome_label(&outcome, Some(&text(result, "reason")));
    match outcome.as_str() {
        "refused" | "dropped" => format!("{label} · {id}"),
        "held" => format!("held by {} · {id}", clean(to)),
        _ => format!("{label} to {} · {id}", clean(to)),
    }
}

fn hub_sent(result: &Value) -> String {
    if result["status"] == "queued" {
        let id = clean(result["operation_id"].as_str().unwrap_or_default());
        return format!("Hub message queued · {id}");
    }
    "Hub message sent".into()
}

fn watching(result: &Value, target: &str) -> String {
    let until = clean(result["expires_at"].as_str().unwrap_or_default());
    format!("Watching {} until {until}", clean(target))
}

/// An error as the user reads it; an ambiguous address lists its candidates.
fn failure(e: ClientError) -> Failure {
    if let ClientError::Api {
        status: 409, body, ..
    } = &e
        && let Some(candidates) = body["candidates"].as_array()
    {
        let named: Vec<String> = candidates
            .iter()
            .map(|c| format!("{} ({})", text(c, "session_id"), text(c, "name")))
            .collect();
        return Failure(format!(
            "ambiguous address; candidates: {}",
            named.join(", ")
        ));
    }
    Failure(clean(&err(e)))
}

impl App {
    /// `/agents`, the palette entry and `agents_view`.
    pub(crate) fn open_agents(&mut self) -> Vec<Action> {
        self.overlay = Overlay::Agents;
        self.directory.editor = None;
        self.agents_request()
    }

    /// Read every source again under a new generation; older replies are dropped.
    pub(crate) fn agents_request(&mut self) -> Vec<Action> {
        let d = &mut self.directory;
        d.generation = d.generation.wrapping_add(1);
        d.last_poll = Some(Instant::now());
        let generation = d.generation;
        Source::ALL
            .into_iter()
            .map(|source| {
                d.source_mut(source).loading = true;
                Action::Directory(Request { generation, source })
            })
            .collect()
    }

    /// A source answered. A failed listing keeps its last rows; a failed hub read
    /// clears them, so a revoked membership leaves no stale roster.
    pub(crate) fn agents_reply(
        &mut self,
        session_id: &str,
        generation: u64,
        source: Source,
        result: Result<Fetched, String>,
    ) {
        if session_id != self.session.id || generation != self.directory.generation {
            return;
        }
        let loaded = self.directory.source_mut(source);
        loaded.loading = false;
        match result {
            Ok(fetched) => {
                loaded.rows = fetched.rows;
                loaded.status = fetched.status;
                loaded.writable = fetched.writable;
                loaded.error = None;
            }
            Err(error) => {
                if source == Source::Hub {
                    loaded.rows.clear();
                    loaded.writable = false;
                }
                loaded.error = Some(error);
            }
        }
        self.directory.anchor();
    }

    /// Action `id` finished: the outcome in a toast, then a refresh. The editor that
    /// sent it closes on success and shows a failure; any other editor (the first was
    /// closed, another opened) is left alone and the outcome is a toast.
    pub(crate) fn agents_done(
        &mut self,
        session_id: &str,
        id: u64,
        result: Result<String, Failure>,
    ) -> Vec<Action> {
        if session_id != self.session.id {
            return Vec::new();
        }
        let d = &mut self.directory;
        let sender = d.editor.as_mut().filter(|e| e.op == Some(id));
        match (result, sender) {
            (Ok(outcome), Some(_)) => {
                d.editor = None;
                self.toast(outcome);
            }
            (Err(Failure(message)), Some(editor)) => {
                editor.busy = false;
                editor.error = Some(message);
            }
            (Ok(text) | Err(Failure(text)), None) => self.toast(text),
        }
        if matches!(self.overlay, Overlay::Agents) {
            return self.agents_request();
        }
        Vec::new()
    }

    /// Every [`POLL`] while open and no editor is, as long as nothing is loading.
    pub(crate) fn agents_poll(&mut self) -> Vec<Action> {
        let d = &self.directory;
        let due = d.last_poll.is_none_or(|at| at.elapsed() >= POLL);
        if matches!(self.overlay, Overlay::Agents) && d.editor.is_none() && !d.loading() && due {
            return self.agents_request();
        }
        Vec::new()
    }

    /// Pasted text goes into an open editor, or into the filter.
    pub(crate) fn agents_paste(&mut self, text: &str) -> bool {
        if !matches!(self.overlay, Overlay::Agents) {
            return false;
        }
        let d = &mut self.directory;
        match &mut d.editor {
            Some(editor) if editor.busy => {}
            Some(editor) => {
                let text = if editor.multiline() {
                    clean_lines(text)
                } else {
                    clean_line(text)
                };
                text.chars().for_each(|c| editor.push(c));
            }
            None => {
                d.filter.push_str(&clean_line(text));
                d.anchor();
            }
        }
        true
    }

    /// Keys of the directory: the editor's when one is open.
    pub(crate) fn agents_key(&mut self, key: KeyEvent) -> Vec<Action> {
        if self.directory.editor.is_some() {
            return self.agents_editor_key(key);
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            return self.agents_chord(key.code);
        }
        match key.code {
            KeyCode::Enter => return self.agents_compose(),
            KeyCode::Esc => self.agents_escape(),
            KeyCode::Tab | KeyCode::BackTab => self.agents_kind(key.code == KeyCode::Tab),
            KeyCode::Backspace => self.agents_filter(|f| {
                f.pop();
            }),
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::ALT) => {
                self.agents_filter(|f| f.push(c))
            }
            code => self.agents_move(code),
        }
        Vec::new()
    }

    fn agents_move(&mut self, code: KeyCode) {
        let delta = match code {
            KeyCode::Up => -1,
            KeyCode::Down => 1,
            KeyCode::PageUp => -10,
            KeyCode::PageDown => 10,
            KeyCode::Home => isize::MIN / 2,
            KeyCode::End => isize::MAX / 2,
            _ => return,
        };
        self.directory.select(delta);
    }

    /// Ctrl chords: the row actions, refresh, and clearing the filter.
    fn agents_chord(&mut self, code: KeyCode) -> Vec<Action> {
        match code {
            KeyCode::Char('w') => self.agents_watch(),
            KeyCode::Char('o') => self.agents_open(),
            KeyCode::Char('r') => {
                self.agents_rename();
                Vec::new()
            }
            KeyCode::Char('g') => self.agents_held(),
            KeyCode::Char('l') => self.agents_request(),
            KeyCode::Char('u') => {
                self.agents_filter(String::clear);
                Vec::new()
            }
            _ => Vec::new(),
        }
    }

    fn agents_escape(&mut self) {
        if self.directory.filter.is_empty() {
            self.overlay = Overlay::None;
        } else {
            self.agents_filter(String::clear);
        }
    }

    fn agents_filter(&mut self, edit: impl FnOnce(&mut String)) {
        edit(&mut self.directory.filter);
        self.directory.anchor();
    }

    fn agents_kind(&mut self, forward: bool) {
        let d = &mut self.directory;
        d.kind = d.kind.step(forward);
        d.anchor();
    }

    /// The selected row, or a toast that there is none.
    fn agents_row(&mut self) -> Option<Entry> {
        let row = self.directory.current().cloned();
        if row.is_none() {
            self.toast("No row selected");
        }
        row
    }

    /// Enter: the compose editor, when the row can be sent to.
    fn agents_compose(&mut self) -> Vec<Action> {
        let Some(row) = self.agents_row() else {
            return Vec::new();
        };
        match compose_target(&row, &self.directory.hub) {
            Ok(to) => {
                let kind = EditorKind::Message { to };
                self.directory.editor = Some(Editor::new(kind, &row.address, String::new()));
            }
            Err(reason) => self.toast(reason),
        }
        Vec::new()
    }

    /// Ctrl+W: watch a listing row until it next goes idle.
    fn agents_watch(&mut self) -> Vec<Action> {
        let Some(row) = self.agents_row() else {
            return Vec::new();
        };
        let reason = match (&row.source, row.state, &row.target) {
            (Source::Hub, _, _) => "watch does not apply to hub rows",
            (_, State::Offline, _) => "an offline peer cannot be watched",
            (_, _, None) => "this row has no safe address to watch",
            (_, _, Some(target)) => {
                let op = Op::Watch {
                    target: target.clone(),
                };
                return vec![Action::DirectoryOp(self.directory.job(op))];
            }
        };
        self.toast(reason);
        Vec::new()
    }

    /// Ctrl+O: switch to a local Session; on a peer row, the attach command.
    fn agents_open(&mut self) -> Vec<Action> {
        let Some(row) = self.agents_row() else {
            return Vec::new();
        };
        if let Some(id) = row.session_id.clone() {
            return vec![Action::Open(id)];
        }
        let message = match &row.kind {
            Kind::Peer => format!("Attach from a shell: cyber attach {}", attach_address(&row)),
            Kind::HubAgent | Kind::HubMember => "hub rows cannot be opened here".into(),
            _ => "this row has no Session to open".into(),
        };
        self.toast(message);
        Vec::new()
    }

    /// Ctrl+R: the inline rename of a local Session or subagent.
    fn agents_rename(&mut self) {
        let Some(row) = self.agents_row() else {
            return;
        };
        let id = row.session_id.clone().filter(|_| row.kind.local());
        let Some(id) = id else {
            let reason = match row.source {
                Source::Hub => "hub rows cannot be renamed",
                Source::Messaging => "peer rows cannot be renamed here",
            };
            self.toast(reason);
            return;
        };
        let input = if row.kind == Kind::Session {
            row.name.clone()
        } else {
            String::new()
        };
        let kind = EditorKind::Rename { id };
        self.directory.editor = Some(Editor::new(kind, &row.address, input));
    }

    /// Ctrl+G: `/messages` on its held filter, at the newest held message from the row.
    fn agents_held(&mut self) -> Vec<Action> {
        let Some(row) = self.agents_row() else {
            return Vec::new();
        };
        if row.source == Source::Hub {
            self.toast("held hub messages are in /messages; hub rows have no listing address");
        }
        let held = |id: &str| self.queued.iter().any(|q| q.held && q.message_id == id);
        let newest = self
            .messaging
            .records
            .iter()
            .filter(|r| r.incoming && held(&r.message_id))
            .filter(|r| {
                let session = row.session_id.is_some() && r.peer_session_id == row.session_id;
                session || row.target.as_deref() == Some(r.from.as_str())
            })
            .max_by_key(|r| r.created_ms)
            .map(|r| r.message_id.clone());
        self.messaging.filter = crate::messaging::Filter::Held;
        self.messaging.selected = newest;
        self.messaging.expanded = false;
        self.open_messages()
    }

    fn agents_editor_key(&mut self, key: KeyEvent) -> Vec<Action> {
        let Some(editor) = &mut self.directory.editor else {
            return Vec::new();
        };
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let newline = key
            .modifiers
            .intersects(KeyModifiers::SHIFT | KeyModifiers::ALT);
        // While the request is in flight only Esc does anything; its answer then
        // arrives as a toast.
        if editor.busy && key.code != KeyCode::Esc {
            return Vec::new();
        }
        match key.code {
            KeyCode::Esc => self.directory.editor = None,
            KeyCode::Enter if newline && editor.multiline() => editor.push('\n'),
            KeyCode::Enter => return self.agents_submit(),
            KeyCode::Backspace => {
                editor.input.pop();
            }
            KeyCode::Char('u') if ctrl => editor.input.clear(),
            KeyCode::Char('n') if ctrl => {
                editor.notify = !editor.notify
                    && matches!(
                        &editor.kind,
                        EditorKind::Message {
                            to: Recipient::Listing(_)
                        }
                    )
            }
            KeyCode::Char(c) if !ctrl && !c.is_control() => editor.push(c),
            _ => {}
        }
        Vec::new()
    }

    /// Enter in the editor: send or rename, once.
    fn agents_submit(&mut self) -> Vec<Action> {
        let Some(editor) = &mut self.directory.editor else {
            return Vec::new();
        };
        if editor.busy {
            return Vec::new();
        }
        let text = editor.input.trim();
        if text.is_empty() {
            editor.error = Some(match editor.kind {
                EditorKind::Rename { .. } => "A Session title cannot be empty".into(),
                EditorKind::Message { .. } => "Write a message first".into(),
            });
            return Vec::new();
        }
        let op = match &editor.kind {
            EditorKind::Rename { id } => Op::Rename {
                id: id.clone(),
                title: text.to_owned(),
            },
            EditorKind::Message {
                to: Recipient::Listing(to),
            } => Op::Send {
                to: to.clone(),
                text: editor.input.clone(),
                notify: editor.notify,
            },
            EditorKind::Message {
                to:
                    Recipient::Hub {
                        participant,
                        project,
                    },
            } => Op::HubSend {
                participant: participant.clone(),
                project: project.clone(),
                text: editor.input.clone(),
            },
        };
        editor.busy = true;
        editor.error = None;
        let job = self.directory.job(op);
        if let Some(editor) = &mut self.directory.editor {
            editor.op = Some(job.id);
        }
        vec![Action::DirectoryOp(job)]
    }
}

/// Who Enter on `row` writes to, or why it cannot.
fn compose_target(row: &Entry, hub: &Loaded) -> Result<Recipient, String> {
    if let (Some(participant), Location::Hub(project)) = (&row.participant, &row.location) {
        if !hub.writable {
            return Err("viewers cannot message hub participants".into());
        }
        return Ok(Recipient::Hub {
            participant: participant.clone(),
            project: project.clone(),
        });
    }
    match (&row.target, row.can_receive) {
        (_, false) => Err(format!(
            "refused: {} cannot receive messages from this Session",
            row.address
        )),
        (None, _) => Err("this row has no safe address to send to".into()),
        (Some(target), true) => Ok(Recipient::Listing(target.clone())),
    }
}

/// `<session>@<peer>` for `cyber attach`: the row's target, or a placeholder for an
/// offline peer's `*@peer`.
fn attach_address(row: &Entry) -> String {
    match (&row.target, &row.location) {
        (Some(target), _) if !target.starts_with('*') => target.clone(),
        (_, Location::Peer { peer, .. }) => format!("<session>@{peer}"),
        _ => row.address.clone(),
    }
}

/// Pasted text for a multi-line editor: each line cleaned, line breaks kept.
fn clean_lines(text: &str) -> String {
    text.replace("\r\n", "\n")
        .split(['\n', '\r'])
        .map(clean)
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn listing() -> Vec<Entry> {
        let data = json!([
            {"address": "tests", "session_id": "ses_01TESTS", "name": "tests", "kind": "subagent",
             "state": "running", "location": {"directory": "/repo"}, "canReceive": true},
            {"address": "reviewer", "session_id": "ses_01REVIEW", "name": "reviewer", "kind": "subagent",
             "state": "idle", "location": {"directory": "/repo"}, "canReceive": true, "workflowRunId": "wfr_1"},
            {"address": "api", "session_id": "ses_01API", "name": "api", "kind": "teammate",
             "state": "waiting_input", "location": {"directory": "/repo"}, "canReceive": true},
            {"address": "migration", "session_id": "ses_01MIGR", "name": "migration", "kind": "local",
             "state": "idle", "location": {"directory": "/other"}, "canReceive": true},
            {"address": "build@desk", "session_id": "ses_01BUILD@desk", "name": "build", "kind": "remote",
             "state": "idle", "location": {"directory": "app"}, "canReceive": true},
            {"address": "*@nas", "session_id": "*@nas", "name": "nas", "kind": "peer",
             "state": "offline", "location": {"directory": ""}, "canReceive": false},
            {"address": "odd", "session_id": "ses_01ODD", "name": "odd", "kind": "relay",
             "state": "busy", "location": {"directory": "/x"}, "canReceive": false},
        ]);
        data.as_array()
            .unwrap()
            .iter()
            .map(Entry::from_listing)
            .collect()
    }

    fn app() -> App {
        let mut app = App::new(Session::default(), Vec::new(), "cyber");
        app.session.id = "ses_ME".into();
        app.overlay = Overlay::Agents;
        app
    }

    fn loaded(app: &mut App, rows: Vec<Entry>) {
        let actions = app.agents_request();
        let generation = match &actions[0] {
            Action::Directory(request) => request.generation,
            other => panic!("{other:?}"),
        };
        let fetched = Fetched {
            rows,
            status: Status::Ready,
            writable: false,
        };
        app.agents_reply("ses_ME", generation, Source::Messaging, Ok(fetched));
        app.agents_reply(
            "ses_ME",
            generation,
            Source::Hub,
            Ok(Fetched::status(Status::Inactive)),
        );
    }

    fn keys(app: &mut App, text: &str) {
        for c in text.chars() {
            app.agents_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
        }
    }

    fn ctrl(app: &mut App, c: char) -> Vec<Action> {
        app.agents_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL))
    }

    fn press(app: &mut App, code: KeyCode) -> Vec<Action> {
        app.agents_key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    /// The operations of `actions`, without their IDs.
    fn ops(actions: &[Action]) -> Vec<Op> {
        actions
            .iter()
            .map(|a| match a {
                Action::DirectoryOp(job) => job.op.clone(),
                other => panic!("{other:?}"),
            })
            .collect()
    }

    /// The ID of the single action in `actions`.
    fn job_id(actions: &[Action]) -> u64 {
        match actions {
            [Action::DirectoryOp(job)] => job.id,
            other => panic!("{other:?}"),
        }
    }

    fn hub_agent(id: &str, name: &str) -> Value {
        json!({"id": id, "name": name, "online": true, "active": true, "owner": "cmem_me"})
    }

    #[test]
    fn listing_entries_parse_every_kind_and_state() {
        let rows = listing();
        let kinds: Vec<&str> = rows.iter().map(|e| e.kind.label()).collect();
        assert_eq!(
            kinds,
            [
                "subagent",
                "workflow agent",
                "teammate",
                "session",
                "peer",
                "peer",
                "relay"
            ]
        );
        let states: Vec<&str> = rows.iter().map(|e| e.state.label()).collect();
        assert_eq!(
            states,
            [
                "running",
                "idle",
                "waiting_input",
                "idle",
                "idle",
                "offline",
                "-"
            ]
        );
        assert_eq!(rows[3].location, Location::Local("/other".into()));
        assert_eq!(rows[3].session_id.as_deref(), Some("ses_01MIGR"));
        assert_eq!(rows[3].target.as_deref(), Some("migration"));
        assert_eq!(
            rows[4].location,
            Location::Peer {
                peer: "desk".into(),
                place: "app".into()
            }
        );
    }

    #[test]
    fn an_offline_peer_is_one_closed_row_and_peers_never_open_as_local() {
        let rows = listing();
        let offline = &rows[5];
        assert_eq!(
            (offline.address.as_str(), offline.state),
            ("*@nas", State::Offline)
        );
        assert!(!offline.can_receive);
        assert_eq!(offline.location.label(), "peer:nas");
        assert_eq!(offline.group(), Group::Peers);
        for peer in &rows[4..6] {
            assert_eq!(peer.session_id, None, "{peer:?}");
        }
        // A peer entry claiming a local ID is still a peer.
        let spoof = Entry::from_listing(&json!({"address": "x", "session_id": "ses_LOCAL",
            "kind": "peer", "state": "idle", "location": {"directory": ""}}));
        assert_eq!(spoof.session_id, None);
        assert_eq!(attach_address(offline), "<session>@nas");
        assert_eq!(attach_address(&rows[4]), "build@desk");
    }

    #[test]
    fn crafted_text_is_cleaned_and_never_sent_to() {
        let crafted = Entry::from_listing(&json!({
            "address": "mig\u{202e}ration\u{1b}[31m@desk", "session_id": "ses_01X@desk",
            "name": "mig\u{202e}ration\u{1b}[31m", "kind": "peer", "state": "idle",
            "location": {"directory": "/home/\u{2066}evil"}, "canReceive": true}));
        assert_eq!(crafted.address, "migration[31m@desk");
        assert_eq!(crafted.name, "migration[31m");
        assert_eq!(crafted.target.as_deref(), Some("ses_01X@desk"));
        assert_eq!(crafted.location.label(), "peer:desk /home/evil");
        let hopeless = Entry::from_listing(&json!({
            "address": "a\u{7}", "session_id": "ses_\u{202e}1", "kind": "local", "state": "idle",
            "location": {"directory": "/"}, "canReceive": true}));
        assert_eq!(hopeless.target, None);
        assert_eq!(hopeless.session_id, None);
        let mut app = app();
        loaded(&mut app, vec![hopeless]);
        press(&mut app, KeyCode::Enter);
        assert!(app.directory.editor.is_none());
        assert!(app.toast.as_ref().unwrap().0.contains("no safe address"));
    }

    #[test]
    fn typing_filters_on_address_name_kind_and_location_and_tab_cycles_kinds() {
        let mut app = app();
        loaded(&mut app, listing());
        let shown = |app: &App| -> Vec<String> {
            app.directory
                .visible()
                .iter()
                .map(|e| e.address.clone())
                .collect()
        };
        keys(&mut app, "migr");
        assert_eq!(shown(&app), ["migration"]);
        ctrl(&mut app, 'u');
        keys(&mut app, "workflow");
        assert_eq!(shown(&app), ["reviewer"]);
        ctrl(&mut app, 'u');
        keys(&mut app, "/OTHER");
        assert_eq!(shown(&app), ["migration"]);
        ctrl(&mut app, 'u');
        keys(&mut app, "peer:nas");
        assert_eq!(shown(&app), ["*@nas"]);
        // Esc clears the filter first, then closes.
        press(&mut app, KeyCode::Esc);
        assert!(app.directory.filter.is_empty());
        assert!(matches!(app.overlay, Overlay::Agents));
        let mut cycle = Vec::new();
        for _ in 0..KindFilter::ALL.len() {
            press(&mut app, KeyCode::Tab);
            cycle.push((app.directory.kind.label(), shown(&app).len()));
        }
        assert_eq!(
            cycle,
            [
                ("sessions", 2),
                ("subagents", 2),
                ("teammates", 1),
                ("peers", 2),
                ("hub", 0),
                ("all", 7)
            ]
        );
        press(&mut app, KeyCode::BackTab);
        assert_eq!(app.directory.kind, KindFilter::Hub);
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Esc);
        assert!(matches!(app.overlay, Overlay::None));
    }

    #[test]
    fn the_selection_follows_its_row_across_a_reload_that_reorders_and_removes() {
        let mut app = app();
        loaded(&mut app, listing());
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Down);
        assert_eq!(app.directory.current().unwrap().address, "api");
        let mut rows = listing();
        rows.reverse();
        loaded(&mut app, rows);
        assert_eq!(app.directory.current().unwrap().address, "api");
        // Its row goes: the selection lands on the row now at its index.
        let rows: Vec<Entry> = listing()
            .into_iter()
            .filter(|e| e.address != "api")
            .collect();
        let at = app
            .directory
            .selected_index(&app.directory.visible())
            .unwrap();
        loaded(&mut app, rows);
        let visible = app.directory.visible();
        assert_eq!(app.directory.selected_index(&visible), Some(at));
        assert_eq!(
            app.directory.selected.as_deref(),
            Some(visible[at].key.as_str())
        );
    }

    #[test]
    fn stale_replies_are_dropped_and_a_failed_listing_keeps_its_rows() {
        let mut app = app();
        loaded(&mut app, listing());
        let old = app.directory.generation;
        app.agents_request();
        app.agents_reply(
            "ses_ME",
            old,
            Source::Messaging,
            Ok(Fetched::status(Status::Ready)),
        );
        assert_eq!(app.directory.messaging.rows.len(), 7);
        app.agents_reply(
            "ses_OTHER",
            app.directory.generation,
            Source::Messaging,
            Ok(Fetched::default()),
        );
        assert_eq!(app.directory.messaging.rows.len(), 7);
        let current = app.directory.generation;
        app.agents_reply(
            "ses_ME",
            current,
            Source::Messaging,
            Err("transport: reset".into()),
        );
        assert_eq!(app.directory.messaging.rows.len(), 7);
        assert_eq!(
            app.directory.messaging.error.as_deref(),
            Some("transport: reset")
        );
        // A failed hub read clears the hub's rows.
        app.directory.hub.rows = vec![Entry::from_hub(
            &Snapshot::default(),
            &json!({"id": "cagt_1", "name": "r"}),
            true,
            "web",
        )];
        app.agents_reply("ses_ME", current, Source::Hub, Err("Access revoked".into()));
        assert!(app.directory.hub.rows.is_empty());
        // A Session switch forgets everything and invalidates replies on their way.
        app.directory.forget();
        app.agents_reply("ses_ME", current, Source::Messaging, Ok(Fetched::default()));
        assert_eq!(app.directory.messaging.status, Status::Unread);
    }

    #[test]
    fn an_inactive_hub_adds_no_rows_and_no_error() {
        let mut app = app();
        loaded(&mut app, Vec::new());
        assert_eq!(app.directory.hub.status, Status::Inactive);
        assert!(app.directory.hub.error.is_none());
        assert!(app.directory.empty());
    }

    #[test]
    fn hub_rows_message_through_the_hub_and_refuse_watch_open_and_rename() {
        let mut app = app();
        loaded(&mut app, Vec::new());
        let agent = Entry::from_hub(
            &Snapshot::default(),
            &json!({"id": "cagt_rev", "name": "reviewer\u{1b}", "online": true, "active": true}),
            true,
            "web",
        );
        assert_eq!(
            (agent.kind.label(), agent.state),
            ("hub agent", State::Online)
        );
        assert_eq!(agent.location.label(), "hub:web");
        app.directory.hub = Loaded {
            rows: vec![agent],
            status: Status::Ready,
            writable: true,
            ..Loaded::default()
        };
        for (c, reason) in [
            ('w', "watch does not apply"),
            ('r', "cannot be renamed"),
            ('o', "cannot be opened"),
        ] {
            assert!(ctrl(&mut app, c).is_empty());
            assert!(app.toast.as_ref().unwrap().0.contains(reason), "{c}");
        }
        press(&mut app, KeyCode::Enter);
        keys(&mut app, "hello");
        let actions = press(&mut app, KeyCode::Enter);
        assert_eq!(
            ops(&actions),
            [Op::HubSend {
                participant: json!({"kind": "agent", "id": "cagt_rev"}),
                project: "web".into(),
                text: "hello".into(),
            }]
        );
        // A viewer cannot.
        app.directory.editor = None;
        app.directory.hub.writable = false;
        press(&mut app, KeyCode::Enter);
        assert!(app.directory.editor.is_none());
    }

    #[test]
    fn listing_actions_build_their_route_requests() {
        let mut app = app();
        loaded(&mut app, listing());
        keys(&mut app, "migration");
        press(&mut app, KeyCode::Enter);
        keys(&mut app, "schema");
        app.agents_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT));
        keys(&mut app, "merged");
        ctrl(&mut app, 'n');
        let actions = press(&mut app, KeyCode::Enter);
        assert_eq!(
            ops(&actions),
            [Op::Send {
                to: "migration".into(),
                text: "schema\nmerged".into(),
                notify: true,
            }]
        );
        let id = job_id(&actions);
        // A second Enter while it is in flight sends nothing more.
        assert!(press(&mut app, KeyCode::Enter).is_empty());
        let refresh = app.agents_done("ses_ME", id, Err(Failure("ambiguous address".into())));
        assert_eq!(refresh.len(), 2);
        let editor = app.directory.editor.as_ref().unwrap();
        assert_eq!(
            (editor.busy, editor.error.as_deref()),
            (false, Some("ambiguous address"))
        );
        let resent = press(&mut app, KeyCode::Enter);
        app.agents_done(
            "ses_ME",
            job_id(&resent),
            Ok("delivered to migration · msgx_1".into()),
        );
        assert!(app.directory.editor.is_none());
        assert_eq!(app.directory.filter, "migration", "the filter stays");
        assert_eq!(
            ops(&ctrl(&mut app, 'w')),
            [Op::Watch {
                target: "migration".into()
            }]
        );
        assert_eq!(ctrl(&mut app, 'o'), [Action::Open("ses_01MIGR".into())]);
        ctrl(&mut app, 'r');
        ctrl(&mut app, 'u');
        keys(&mut app, "api-server");
        assert_eq!(
            ops(&press(&mut app, KeyCode::Enter)),
            [Op::Rename {
                id: "ses_01MIGR".into(),
                title: "api-server".into()
            }]
        );
    }

    /// Regression: an answer belongs to the editor that sent it. A send whose editor
    /// was closed with Esc must not close, or write its error into, the next editor.
    #[test]
    fn a_late_answer_never_touches_another_editor() {
        let mut app = app();
        loaded(&mut app, listing());
        keys(&mut app, "migration");
        press(&mut app, KeyCode::Enter);
        keys(&mut app, "first");
        let first = job_id(&press(&mut app, KeyCode::Enter));
        // Busy: typing does nothing, Esc closes.
        keys(&mut app, "x");
        assert_eq!(app.directory.editor.as_ref().unwrap().input, "first");
        press(&mut app, KeyCode::Esc);
        assert!(app.directory.editor.is_none());
        press(&mut app, KeyCode::Enter);
        keys(&mut app, "draft");
        app.agents_done(
            "ses_ME",
            first,
            Ok("delivered to migration · msgx_1".into()),
        );
        let editor = app.directory.editor.as_ref().expect("the draft stays open");
        assert_eq!(
            (editor.input.as_str(), editor.error.as_deref()),
            ("draft", None)
        );
        assert_eq!(
            app.toast.as_ref().unwrap().0,
            "delivered to migration · msgx_1"
        );
        app.agents_done("ses_ME", first, Err(Failure("ambiguous address".into())));
        let editor = app.directory.editor.as_ref().unwrap();
        assert_eq!(editor.error, None);
        assert_eq!(app.toast.as_ref().unwrap().0, "ambiguous address");
        // A late answer does not touch a rename editor either.
        press(&mut app, KeyCode::Esc);
        ctrl(&mut app, 'r');
        app.agents_done("ses_ME", first, Err(Failure("late".into())));
        let editor = app.directory.editor.as_ref().unwrap();
        assert!(matches!(editor.kind, EditorKind::Rename { .. }));
        assert_eq!((editor.busy, editor.error.as_deref()), (false, None));
    }

    /// Regression: a title is bounded in characters, as the server keeps it, so a
    /// non-ASCII title is not cut at 100 bytes; a message stays bounded in bytes.
    #[test]
    fn a_title_is_bounded_in_characters() {
        let mut app = app();
        loaded(&mut app, listing());
        keys(&mut app, "migration");
        ctrl(&mut app, 'r');
        ctrl(&mut app, 'u');
        keys(&mut app, &"日".repeat(TITLE_LIMIT + 5));
        let input = &app.directory.editor.as_ref().unwrap().input;
        assert_eq!(input.chars().count(), TITLE_LIMIT);
        assert!(input.len() > TITLE_LIMIT);
    }

    /// Regression: the agent the attached Session is published as and the signed-in
    /// member are not listed; other agents and members are, a member without presence
    /// as `-` and a guest's agent marked guest.
    #[test]
    fn the_hub_lists_neither_this_session_nor_this_account() {
        let roster = Snapshot {
            status: json!({"enabled": true, "binding": {"project": "web"},
                "current_member": {"id": "cmem_me", "role": "member"},
                "published": {"agent_id": "cagt_self"}}),
            agents: vec![
                hub_agent("cagt_self", "reviewer"),
                hub_agent("cagt_other", "builder"),
                json!({"id": "cagt_guest", "name": "visitor", "online": false, "owner": "cmem_bob"}),
            ],
            members: vec![
                json!({"id": "cmem_me", "name": "me", "role": "member", "kind": "internal"}),
                json!({"id": "cmem_bob", "name": "bob", "role": "viewer", "kind": "guest", "active": true}),
            ],
            ..Default::default()
        };
        let rows = hub_rows(&roster);
        let shown: Vec<(&str, &str, State)> = rows
            .iter()
            .map(|e| (e.name.as_str(), e.kind.label(), e.state))
            .collect();
        assert_eq!(
            shown,
            [
                ("builder", "hub agent", State::Online),
                ("visitor (guest)", "hub agent", State::Offline),
                ("bob (viewer, guest)", "hub member", State::Unknown),
            ]
        );
        assert_eq!(State::Unknown.label(), "-");
        // Unpublished: every active agent is listed.
        let mut unpublished = roster.clone();
        unpublished.status["published"] = Value::Null;
        assert_eq!(hub_rows(&unpublished).len(), 4);
    }

    /// The poll fires once [`POLL`] has passed with no editor, and not while one is open.
    #[test]
    fn the_poll_is_due_every_ten_seconds_without_an_editor() {
        let mut app = app();
        loaded(&mut app, listing());
        assert!(app.agents_poll().is_empty(), "not due yet");
        let overdue = || Instant::now().checked_sub(POLL);
        app.directory.last_poll = overdue();
        let polled = app.agents_poll();
        assert_eq!(polled.len(), 2, "{polled:?}");
        assert!(polled.iter().all(|a| matches!(a, Action::Directory(_))));
        loaded(&mut app, listing());
        keys(&mut app, "migration");
        press(&mut app, KeyCode::Enter);
        app.directory.last_poll = overdue();
        assert!(app.agents_poll().is_empty(), "held back by the editor");
    }

    /// A Session switch through `set_session` closes `/agents` and resets it.
    #[test]
    fn a_session_switch_closes_and_resets_the_directory() {
        let mut app = app();
        loaded(&mut app, listing());
        keys(&mut app, "migration");
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Enter);
        app.set_session(Session {
            id: "ses_OTHER".into(),
            ..Session::default()
        });
        assert!(matches!(app.overlay, Overlay::None));
        let d = &app.directory;
        assert!(d.filter.is_empty() && d.editor.is_none() && d.selected.is_none());
        assert_eq!(d.kind, KindFilter::All);
        assert!(d.rows().next().is_none());
        assert_eq!(d.messaging.status, Status::Unread);
    }

    #[test]
    fn closed_and_peer_rows_say_why_an_action_does_not_apply() {
        let mut app = app();
        loaded(&mut app, listing());
        keys(&mut app, "*@nas");
        press(&mut app, KeyCode::Enter);
        assert!(app.directory.editor.is_none());
        assert!(app.toast.as_ref().unwrap().0.contains("cannot receive"));
        assert!(ctrl(&mut app, 'w').is_empty());
        assert!(app.toast.as_ref().unwrap().0.contains("offline"));
        assert!(ctrl(&mut app, 'o').is_empty());
        assert!(
            app.toast
                .as_ref()
                .unwrap()
                .0
                .contains("cyber attach <session>@nas")
        );
        ctrl(&mut app, 'r');
        assert!(app.directory.editor.is_none());
        assert!(
            app.toast
                .as_ref()
                .unwrap()
                .0
                .contains("peer rows cannot be renamed")
        );
    }

    #[test]
    fn outcomes_and_errors_read_as_sentences() {
        let result =
            json!({"message_id": "msgx_1", "outcome": "refused", "reason": "rate_limited"});
        assert_eq!(sent(&result, "api"), "refused: rate_limited · msgx_1");
        let result = json!({"message_id": "msgx_2", "outcome": "delivered"});
        assert_eq!(sent(&result, "api"), "delivered to api · msgx_2");
        let ambiguous = ClientError::Api {
            status: 409,
            tag: "AmbiguousAddressError".into(),
            message: "x".into(),
            body: json!({"candidates": [{"session_id": "ses_a", "name": "m"}, {"session_id": "ses_b", "name": "m"}]}),
        };
        assert_eq!(
            failure(ambiguous),
            Failure("ambiguous address; candidates: ses_a (m), ses_b (m)".into())
        );
    }

    #[test]
    fn a_click_selects_a_row_by_its_key_and_an_open_editor_takes_no_clicks() {
        use crate::mouse::List;
        let mut app = app();
        loaded(&mut app, listing());
        assert!(app.select_row(List::Directory, 3));
        assert_eq!(app.directory.current().unwrap().address, "migration");
        assert!(!app.select_row(List::Directory, 99));
        press(&mut app, KeyCode::Enter);
        assert!(!app.select_row(List::Directory, 0));
        assert_eq!(app.directory.current().unwrap().address, "migration");
    }

    #[test]
    fn paste_stays_in_the_directory() {
        let mut app = app();
        loaded(&mut app, listing());
        app.on_paste("mig\u{1b}");
        assert_eq!(app.directory.filter, "mig");
        press(&mut app, KeyCode::Enter);
        app.on_paste(&format!("a\nb\u{202e}{}", "é".repeat(MESSAGE_LIMIT)));
        let editor = app.directory.editor.as_ref().unwrap();
        assert!(editor.input.starts_with("a\nb"));
        assert!(editor.input.len() <= MESSAGE_LIMIT);
        assert!(app.composer.is_empty());
    }
}

#[cfg(test)]
mod computer_integration_tests {
    use super::*;

    #[test]
    fn busy_agents_editor_keeps_native_warning_and_global_stop() {
        let session = Session {
            id: "owner-session".into(),
            ..Default::default()
        };
        let mut app = App::new(session, Vec::new(), "cyber");
        app.overlay = Overlay::Agents;
        let mut editor = Editor::new(
            EditorKind::Rename {
                id: "other-session".into(),
            },
            "Other Session",
            "Keep this draft".into(),
        );
        editor.busy = true;
        app.directory.editor = Some(editor);
        app.computer.poll(&app.session, Instant::now());
        app.computer.apply("owner-session", Ok(json!({"data":{"grant":{"session_id":"owner-session","target":{"identity":"app:fixture"},"expires_at":i64::MAX}}})));
        let screen = crate::screens::render(&mut app, 160, 48, &Default::default());
        assert!(screen.contains("Computer ARMED"), "{screen}");
        assert!(screen.contains("Ctrl+Alt+Esc STOP"), "{screen}");
        assert_eq!(
            app.on_key(KeyEvent::new(
                KeyCode::Esc,
                KeyModifiers::CONTROL | KeyModifiers::ALT
            )),
            vec![Action::ComputerStop]
        );
        assert_eq!(app.overlay, Overlay::Agents);
        assert_eq!(
            app.directory.editor.as_ref().unwrap().input,
            "Keep this draft"
        );
    }
}
