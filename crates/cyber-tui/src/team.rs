//! Agent teams in the TUI (`agent-teams`, design section 16): `/team`, `/team start`,
//! `/team stop` and `/team resume`, the team the shown Session leads or belongs to (read
//! with the sidebar, refreshed from `GET /teams/:id/events`), the team view's keys, and
//! the way back from a teammate opened there to the lead.
//!
//! Names, titles and messages come from teammates and models: they are untrusted and
//! are kept without control characters.

use crossterm::event::{KeyCode, KeyEvent};
use futures::StreamExt;
use serde_json::{Value, json};

use cyber_client::{Client, ClientError};

use crate::app::{Action, App, Overlay};
use crate::messaging::clean;
use crate::model::Session;
use crate::perform::{Msg, err};
use crate::sidebar::{Loaded, Part, Source};
use crate::workflows::encode;

/// What `/team` says while `teams` is on and the Session is in no team.
pub const NO_TEAM: &str = "This Session is in no team. Start one with /team start <definition>, \
     or ask for teammates and the model spawns them with team_spawn.";
const USAGE: &str = "Use /team, /team start <definition>, /team stop or /team resume [team]";

/// The task statuses the board shows, in column order; `cancelled` only with `c`.
pub const COLUMNS: [&str; 4] = ["todo", "in_progress", "blocked", "done"];

/// A team as `GET /teams/:id` shows it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TeamView {
    pub id: String,
    pub name: String,
    pub lead_id: String,
    /// `active`, `stopping` or `stopped`.
    pub status: String,
    pub stop_reason: Option<String>,
    pub cost_usd: f64,
    /// The lead first, then teammates in spawn order.
    pub members: Vec<Member>,
    /// In creation order.
    pub tasks: Vec<Task>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Member {
    pub name: String,
    pub session_id: String,
    /// `lead` or `teammate`.
    pub role: String,
    /// `working`, `idle`, `waiting_input` or `stopped`.
    pub status: String,
    /// The title of the first `in_progress` task it owns.
    pub task: Option<String>,
    pub cost_usd: f64,
    /// `→ lead: …` (sent) or `← lead: …` (received), its first line.
    pub last_message: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Task {
    pub id: String,
    pub title: String,
    pub status: String,
    pub owner: Option<String>,
    /// The tasks it depends on.
    pub depends_on: Vec<String>,
}

impl TeamView {
    pub fn parse(v: &Value) -> Option<Self> {
        let team = &v["team"];
        let list = |key: &str| v[key].as_array().cloned().unwrap_or_default();
        Some(Self {
            id: team["id"].as_str()?.to_owned(),
            name: clean(team["name"].as_str().unwrap_or_default()),
            lead_id: team["lead_id"].as_str().unwrap_or_default().to_owned(),
            status: clean(team["status"].as_str().unwrap_or_default()),
            stop_reason: team["stop_reason"].as_str().map(clean),
            cost_usd: v["spent"]["cost_usd"].as_f64().unwrap_or(0.0),
            members: list("members").iter().filter_map(Member::parse).collect(),
            tasks: list("tasks").iter().filter_map(Task::parse).collect(),
        })
    }

    /// The tasks of a board column.
    pub fn column<'a>(&'a self, status: &'a str) -> impl Iterator<Item = &'a Task> + 'a {
        self.tasks.iter().filter(move |task| task.status == status)
    }

    pub fn done(&self) -> usize {
        self.column("done").count()
    }

    /// How many of `task`'s dependencies are not `done` yet.
    pub fn waits(&self, task: &Task) -> usize {
        task.depends_on
            .iter()
            .filter(|id| {
                !self
                    .tasks
                    .iter()
                    .any(|t| &t.id == *id && t.status == "done")
            })
            .count()
    }

    /// `active`, or `stopped (budget)`.
    pub fn status_label(&self) -> String {
        match &self.stop_reason {
            Some(reason) if self.status != "active" => format!("{} ({reason})", self.status),
            _ => self.status.clone(),
        }
    }
}

impl Member {
    fn parse(v: &Value) -> Option<Self> {
        let message = &v["last_message"];
        let last_message = message["text"].as_str().map(|text| {
            let arrow = if message["direction"] == "out" {
                "→"
            } else {
                "←"
            };
            let first = text.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
            format!(
                "{arrow} {}: {}",
                clean(message["peer"].as_str().unwrap_or_default()),
                clean(first)
            )
        });
        Some(Self {
            name: clean(v["name"].as_str()?),
            session_id: v["session_id"].as_str()?.to_owned(),
            role: clean(v["role"].as_str().unwrap_or_default()),
            status: clean(v["status"].as_str().unwrap_or_default()),
            task: v["current_task"]["title"].as_str().map(clean),
            cost_usd: v["cost_usd"].as_f64().unwrap_or(0.0),
            last_message,
        })
    }
}

impl Task {
    fn parse(v: &Value) -> Option<Self> {
        Some(Self {
            id: v["id"].as_str()?.to_owned(),
            title: clean(v["title"].as_str()?),
            status: v["status"].as_str()?.to_owned(),
            owner: v["owner"].as_str().map(clean),
            depends_on: v["depends_on"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|id| id.as_str().map(str::to_owned))
                .collect(),
        })
    }
}

/// A teammate opened from the team view, and the lead `esc` returns to.
#[derive(Debug, Clone, PartialEq)]
pub struct Opened {
    pub member: String,
    pub lead: String,
}

/// The team view's selection and the way back from an opened teammate; the team itself
/// is `Sidebar::team`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TeamState {
    pub selected: usize,
    /// `c`: the board also shows `cancelled`.
    pub show_cancelled: bool,
    pub opened: Option<Opened>,
}

/// A `/team` command.
#[derive(Debug, Clone, PartialEq)]
pub enum Request {
    /// `/team`: open the team view.
    Open,
    /// `/team start <definition>`.
    Start(String),
    Stop,
    /// `/team resume [team]`: the Session's team, or a stopped team of the Location by
    /// ID or name.
    Resume(String),
}

impl Request {
    /// The request for `/team <args>`; `None` for anything else.
    pub fn parse(args: &str) -> Option<Self> {
        let (verb, rest) = args
            .split_once(' ')
            .map_or((args, ""), |(verb, rest)| (verb, rest.trim()));
        match verb {
            "" => Some(Self::Open),
            "start" if !rest.is_empty() => Some(Self::Start(rest.to_owned())),
            "stop" if rest.is_empty() => Some(Self::Stop),
            "resume" => Some(Self::Resume(rest.to_owned())),
            _ => None,
        }
    }
}

/// Whether `GET /features` reports `teams` on.
fn enabled(features: &Value) -> bool {
    features["data"].as_array().is_some_and(|list| {
        list.iter().any(|feature| {
            feature["name"] == cyber_core::features::TEAMS && feature["enabled"] == true
        })
    })
}

/// Run a `/team` command.
pub async fn perform(client: &Client, session: &Session, request: Request) -> Result<Msg, String> {
    let at = client.at(&session.directory);
    if !enabled(&at.get("/features").await.map_err(err)?) {
        return Ok(Msg::Toast(cyber_core::teams::DISABLED.into()));
    }
    match request {
        Request::Open => match team_of(&at, session).await? {
            Some(id) => shown(&at, session, &id, true, None).await,
            None => Ok(Msg::Toast(NO_TEAM.into())),
        },
        Request::Start(definition) => {
            let body = json!({ "lead_session_id": session.id, "definition": definition });
            let created = at.post("/teams", body).await.map_err(err)?;
            let view = TeamView::parse(&created["data"]["team"]).ok_or("Invalid team")?;
            let names: Vec<&str> = view
                .members
                .iter()
                .skip(1)
                .map(|m| m.name.as_str())
                .collect();
            let toast = format!("Team {} started: {}", view.name, names.join(", "));
            Ok(team_msg(session, view, true, Some(toast)))
        }
        Request::Stop => {
            let id = team_of(&at, session).await?.ok_or(NO_TEAM)?;
            let path = format!("/teams/{}/stop", encode(&id));
            let team = at.post(&path, json!({})).await.map_err(err)?;
            let toast = format!("Team {}", stop_label(&team["data"]));
            shown(&at, session, &id, false, Some(toast)).await
        }
        Request::Resume(wanted) => resume(&at, session, &wanted).await,
    }
}

/// `feature stopping`, `feature stopped`.
fn stop_label(team: &Value) -> String {
    format!(
        "{} {}",
        clean(team["name"].as_str().unwrap_or_default()),
        clean(team["status"].as_str().unwrap_or_default())
    )
}

/// `/team resume [team]`: a team whose lead was deleted is resumed with this Session as
/// its new lead.
async fn resume(at: &Client, session: &Session, wanted: &str) -> Result<Msg, String> {
    let (id, lead_deleted) = if wanted.is_empty() {
        (team_of(at, session).await?.ok_or(NO_TEAM)?, false)
    } else {
        let teams = at.get("/teams?status=stopped").await.map_err(err)?;
        let team = teams["data"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|t| t["id"] == wanted || t["name"] == wanted)
            .ok_or_else(|| format!("No stopped team {wanted}"))?;
        let id = team["id"].as_str().unwrap_or_default().to_owned();
        (id, team["stop_reason"] == "lead_deleted")
    };
    let body = if lead_deleted {
        json!({ "lead": session.id })
    } else {
        json!({})
    };
    let path = format!("/teams/{}/resume", encode(&id));
    let team = at.post(&path, body).await.map_err(err)?;
    let toast = format!(
        "Team {} resumed",
        clean(team["data"]["name"].as_str().unwrap_or_default())
    );
    shown(at, session, &id, false, Some(toast)).await
}

/// The team the Session belongs to: a teammate's tag, else the team it leads in its
/// Location.
async fn team_of(at: &Client, session: &Session) -> Result<Option<String>, String> {
    if let Some(tag) = &session.team {
        return Ok(Some(tag.team_id.clone()));
    }
    led(at, session).await.map_err(|error| err(*error))
}

/// The team the Session leads in its Location.
async fn led(at: &Client, session: &Session) -> Result<Option<String>, Box<ClientError>> {
    let teams = at.get("/teams").await.map_err(Box::new)?;
    Ok(teams["data"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|team| team["lead_id"] == session.id.as_str())
        .and_then(|team| team["id"].as_str())
        .map(str::to_owned))
}

async fn shown(
    at: &Client,
    session: &Session,
    id: &str,
    open: bool,
    toast: Option<String>,
) -> Result<Msg, String> {
    let value = at
        .get(&format!("/teams/{}", encode(id)))
        .await
        .map_err(err)?;
    let view = TeamView::parse(&value["data"]).ok_or("Invalid team")?;
    Ok(team_msg(session, view, open, toast))
}

fn team_msg(session: &Session, view: TeamView, open: bool, toast: Option<String>) -> Msg {
    Msg::Team {
        session_id: session.id.clone(),
        view: Box::new(view),
        open,
        toast,
    }
}

/// The sidebar's Team part: the Session's team, or `Missing` when it is in none or
/// `teams` is off (404).
pub async fn load(client: &Client, session: &Session) -> Loaded {
    let at = client.at(&session.directory);
    let id = match &session.team {
        Some(tag) => Ok(Some(tag.team_id.clone())),
        None => led(&at, session).await,
    };
    let missing = |error: &ClientError| {
        matches!(
            error,
            ClientError::Api {
                status: 404 | 503,
                ..
            }
        )
    };
    let id = match id {
        Ok(Some(id)) => id,
        Ok(None) => return Loaded::Team(Source::Missing),
        Err(error) if missing(&error) => return Loaded::Team(Source::Missing),
        Err(_) => return Loaded::Failed,
    };
    match at.get(&format!("/teams/{}", encode(&id))).await {
        Ok(value) => {
            Loaded::Team(TeamView::parse(&value["data"]).map_or(Source::Missing, Source::Ready))
        }
        Err(error) if missing(&error) => Loaded::Team(Source::Missing),
        Err(_) => Loaded::Failed,
    }
}

/// Ticks (250 ms) between attempts to open the team's event stream.
const REOPEN_TICKS: u32 = 8;

/// The live events of the team the shown Session belongs to (`GET /teams/:id/events`),
/// followed while the team is known; each one re-reads the team.
#[derive(Default)]
pub struct Events {
    team_id: Option<String>,
    stream: Option<futures::stream::BoxStream<'static, cyber_client::Event>>,
    ticks: u32,
}

impl Events {
    /// The next event; nothing while no stream is open.
    pub async fn next(&mut self) -> Option<cyber_client::Event> {
        let Some(stream) = self.stream.as_mut() else {
            return std::future::pending().await;
        };
        let event = stream.next().await;
        if event.is_none() {
            self.stream = None;
        }
        event
    }

    /// Follow the team the sidebar shows: open its stream, or close it for none.
    pub async fn sync(&mut self, app: &App, client: &Client) {
        let wanted = app.sidebar.team.ready().map(|team| team.id.clone());
        if wanted != self.team_id {
            self.stream = None;
            self.team_id = wanted;
            self.ticks = 0;
        }
        let Some(id) = self.team_id.clone() else {
            return;
        };
        if self.stream.is_some() || !self.ticks.is_multiple_of(REOPEN_TICKS) {
            self.ticks = self.ticks.wrapping_add(1);
            return;
        }
        self.ticks = self.ticks.wrapping_add(1);
        let at = client.at(&app.session.directory);
        let open = at.team_events(&id);
        if let Ok(Ok(stream)) = tokio::time::timeout(std::time::Duration::from_secs(2), open).await
        {
            self.stream = Some(stream.boxed());
        }
    }
}

impl App {
    /// `/team …`.
    pub(crate) fn team_command(&mut self, args: &str) -> Vec<Action> {
        match Request::parse(args) {
            Some(request) => vec![Action::Team(request)],
            None => {
                self.toast(USAGE);
                Vec::new()
            }
        }
    }

    /// A `/team` reply for the Session it was read for.
    pub(crate) fn team_reply(
        &mut self,
        session_id: &str,
        view: TeamView,
        open: bool,
        toast: Option<String>,
    ) {
        if let Some(toast) = toast {
            self.toast(toast);
        }
        if session_id != self.session.id {
            return;
        }
        self.sidebar.team = Source::Ready(view);
        if open {
            self.team.selected = 0;
            self.overlay = Overlay::Team;
        }
    }

    /// An event of the followed team: read it again.
    pub(crate) fn team_event(&mut self, _event: &cyber_client::Event) -> Vec<Action> {
        vec![Action::LoadSidebar(Part::Team)]
    }

    /// Keys of the team view.
    pub(crate) fn team_key(&mut self, key: KeyEvent) -> Vec<Action> {
        let count = self.sidebar.team.ready().map_or(0, |t| t.members.len());
        let last = count.saturating_sub(1);
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                self.team.selected = self.team.selected.saturating_sub(1)
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.team.selected = (self.team.selected + 1).min(last)
            }
            KeyCode::Char('c') => self.team.show_cancelled = !self.team.show_cancelled,
            KeyCode::Char('r') => return vec![Action::LoadSidebar(Part::Team)],
            KeyCode::Enter => return self.open_member(),
            KeyCode::Esc | KeyCode::Char('q') => self.overlay = Overlay::None,
            _ => {}
        }
        Vec::new()
    }

    /// `enter` on a member: its Session in the normal transcript, where prompts reach it
    /// alone; `esc` there comes back to the lead.
    fn open_member(&mut self) -> Vec<Action> {
        let Some(team) = self.sidebar.team.ready() else {
            return Vec::new();
        };
        let Some(member) = team.members.get(self.team.selected) else {
            return Vec::new();
        };
        let (session_id, name, lead) = (
            member.session_id.clone(),
            member.name.clone(),
            team.lead_id.clone(),
        );
        self.overlay = Overlay::None;
        if session_id == self.session.id {
            return Vec::new();
        }
        self.team.opened = (session_id != lead).then(|| Opened {
            member: session_id.clone(),
            lead,
        });
        if self.team.opened.is_some() {
            self.toast(format!("Teammate {name} · esc returns to the lead"));
        }
        vec![Action::Open(session_id)]
    }

    /// Esc in a teammate opened from the team view, unless a completion menu takes it:
    /// back to the lead and the team view. `None` lets the key through.
    pub(crate) fn team_return_key(&mut self, key: KeyEvent) -> Option<Vec<Action>> {
        if key.code != KeyCode::Esc || self.completion.is_some() {
            return None;
        }
        let opened = self.team.opened.take_if(|o| o.member == self.session.id)?;
        self.overlay = Overlay::Team;
        Some(vec![Action::Open(opened.lead)])
    }

    /// Another Session was opened another way: an opened teammate forgets its way back.
    pub(crate) fn team_session(&mut self, session_id: &str) {
        if self
            .team
            .opened
            .as_ref()
            .is_some_and(|o| o.member != session_id && o.lead != session_id)
        {
            self.team.opened = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn team_says_how_to_enable_teams_while_they_are_off() {
        let off = json!({"data": [{"name": "teams", "enabled": false}]});
        assert!(!enabled(&off));
        assert!(!enabled(&json!({})));
        assert!(enabled(
            &json!({"data": [{"name": "teams", "enabled": true}]})
        ));
        assert_eq!(
            cyber_core::teams::DISABLED,
            "Agent teams are disabled. Enable them with \"cyber features enable teams\"."
        );
    }

    #[test]
    fn team_commands_parse() {
        assert_eq!(Request::parse(""), Some(Request::Open));
        assert_eq!(
            Request::parse("start feature"),
            Some(Request::Start("feature".into()))
        );
        assert_eq!(Request::parse("start"), None);
        assert_eq!(Request::parse("stop"), Some(Request::Stop));
        assert_eq!(Request::parse("stop now"), None);
        assert_eq!(
            Request::parse("resume"),
            Some(Request::Resume(String::new()))
        );
        assert_eq!(
            Request::parse("resume feature"),
            Some(Request::Resume("feature".into()))
        );
        assert_eq!(Request::parse("merge api"), None);
    }

    #[test]
    fn views_parse_without_control_characters() {
        let view = TeamView::parse(&json!({
            "team": {"id": "team_1", "name": "feat\u{1b}ure", "lead_id": "ses_lead",
                     "status": "stopped", "stop_reason": "budget"},
            "spent": {"cost_usd": 0.5},
            "members": [
                {"name": "lead", "session_id": "ses_lead", "role": "lead", "status": "idle"},
                {"name": "api", "session_id": "ses_api", "role": "teammate", "status": "working",
                 "current_task": {"id": "task_1", "title": "Build\u{7} the API"}, "cost_usd": 0.25,
                 "last_message": {"direction": "out", "peer": "lead", "text": "\nDone\nmore"}}
            ],
            "tasks": [{"id": "task_0", "title": "Schema", "status": "done"},
                      {"id": "task_1", "title": "Build the API", "status": "in_progress",
                       "owner": "api", "depends_on": ["task_0", "task_9"]}]
        }))
        .unwrap();
        assert_eq!(view.name, "feature");
        assert_eq!(view.status_label(), "stopped (budget)");
        let api = &view.members[1];
        assert_eq!(api.task.as_deref(), Some("Build the API"));
        assert_eq!(api.last_message.as_deref(), Some("→ lead: Done"));
        assert_eq!(view.waits(&view.tasks[1]), 1, "task_0 is done");
        assert_eq!(view.column("in_progress").count(), 1);
    }
}
