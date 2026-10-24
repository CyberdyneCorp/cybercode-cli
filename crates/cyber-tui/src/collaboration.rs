//! Project collaboration is read through the local owner API; credentials and remote
//! author selection never enter the terminal UI. All hub text is untrusted.
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crossterm::event::{KeyCode, KeyEvent};
use cyber_client::Client;
use serde_json::{Value, json};

use crate::app::{Action, App, Overlay};
use crate::messaging::clean;
use crate::model::Session;
use crate::perform::{Msg, err};
use crate::workflows::encode;

#[derive(Debug, Clone, PartialEq)]
pub struct Request {
    pub generation: u64,
    pub mutation: Option<(String, Value)>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Snapshot {
    pub status: Value,
    pub notice: Option<String>,
    pub members: Vec<Value>,
    pub agents: Vec<Value>,
    pub messages: Vec<Value>,
    pub tasks: Vec<Value>,
}

impl Snapshot {
    pub fn writable(&self) -> bool {
        matches!(
            self.status["current_member"]["role"].as_str(),
            Some("owner" | "member")
        )
    }

    pub fn project(&self) -> Option<&str> {
        self.status["binding"]["project"]
            .as_str()
            .or_else(|| self.status["binding"]["project"]["id"].as_str())
    }

    pub fn participant(&self, who: &Value) -> String {
        let id = who["id"].as_str().unwrap_or_default();
        let agent = who["kind"] == "agent";
        let list = if agent { &self.agents } else { &self.members };
        let found = list.iter().find(|v| v["id"] == id);
        let label = found.map_or_else(|| clean(id), |v| text(v, "name"));
        let kind = if agent { "agent" } else { "human" };
        let guest = found.is_some_and(|v| {
            v["kind"] == "guest"
                || (agent
                    && self
                        .members
                        .iter()
                        .any(|m| m["id"] == v["owner"] && m["kind"] == "guest"))
        });
        format!("{label} [{kind}{}]", if guest { ", guest" } else { "" })
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Tab {
    #[default]
    People,
    Conversation,
    Tasks,
}

#[derive(Debug, Clone)]
pub struct Editor {
    pub title: &'static str,
    pub input: String,
    pub kind: Edit,
}
impl Editor {
    fn limit(&self) -> usize {
        if matches!(&self.kind, Edit::Task) {
            300
        } else {
            16 * 1024
        }
    }
}

#[derive(Debug, Clone)]
pub enum Edit {
    Message(Value),
    Task,
}

#[derive(Debug, Default)]
pub struct PanelState {
    pub snapshot: Option<Snapshot>,
    pub error: Option<String>,
    pub tab: Tab,
    pub selected: usize,
    pub editor: Option<Editor>,
    pub loading: bool,
    generation: u64,
    last_poll: Option<Instant>,
}

impl PanelState {
    pub fn forget(&mut self) {
        let generation = self.generation.wrapping_add(1);
        *self = Self {
            generation,
            ..Default::default()
        };
    }
}

fn rows(value: &Value, key: &str) -> Vec<Value> {
    value
        .as_array()
        .or_else(|| value[key].as_array())
        .cloned()
        .unwrap_or_default()
}

pub fn text(value: &Value, key: &str) -> String {
    clean(value[key].as_str().unwrap_or_default())
}

/// Network errors clear the previous snapshot, including all stale authorization.
pub async fn perform(client: &Client, session: &Session, request: Request) -> Result<Msg, String> {
    let result = load(client, session, request.mutation)
        .await
        .map_err(|s| clean(&s));
    Ok(Msg::Collaboration {
        session_id: session.id.clone(),
        generation: request.generation,
        result: Box::new(result),
    })
}

async fn load(
    client: &Client,
    session: &Session,
    mutation: Option<(String, Value)>,
) -> Result<Snapshot, String> {
    let at = client.at(&session.directory);
    let notice = if let Some((path, body)) = mutation {
        let result = at.post(&path, body).await.map_err(err)?;
        queued_notice(&result["data"])
    } else {
        None
    };
    match read(client, session).await {
        Ok(mut snapshot) => {
            snapshot.notice = notice;
            Ok(snapshot)
        }
        Err(error) => Err(match notice {
            Some(notice) => format!("{notice}; refresh unavailable: {error}"),
            None => error,
        }),
    }
}

fn queued_notice(result: &Value) -> Option<String> {
    (result["status"] == "queued").then(|| {
        format!(
            "Queued operation {}: {}",
            text(result, "operation_id"),
            text(result, "message")
        )
    })
}

async fn read(client: &Client, session: &Session) -> Result<Snapshot, String> {
    let at = client.at(&session.directory);
    let query = format!("session_id={}", encode(&session.id));
    let status = at
        .get(&format!("/collaboration/status?{query}"))
        .await
        .map_err(err)?;
    let mut snapshot = Snapshot {
        status: status["data"].clone(),
        ..Default::default()
    };
    if snapshot.status["enabled"] != true || snapshot.project().is_none() {
        return Ok(snapshot);
    }
    let query = format!(
        "{query}&project={}",
        encode(snapshot.project().unwrap_or_default())
    );
    // A failed source must not leave an old roster or task board on screen.
    let roster = at
        .get(&format!("/collaboration/participants?{query}"))
        .await
        .map_err(err)?;
    let messages = at
        .get(&format!("/collaboration/messages?{query}"))
        .await
        .map_err(err)?;
    let tasks = at
        .get(&format!("/collaboration/tasks?{query}"))
        .await
        .map_err(err)?;
    snapshot.members = rows(&roster["data"]["members"], "members");
    snapshot.agents = rows(&roster["data"]["agents"], "agents");
    snapshot.messages = rows(&messages["data"], "messages");
    snapshot.tasks = rows(&tasks["data"], "tasks");
    Ok(snapshot)
}

impl App {
    pub(crate) fn open_collaboration(&mut self) -> Vec<Action> {
        self.overlay = Overlay::Collaboration;
        self.collaboration.editor = None;
        self.collaboration_request(None)
    }

    fn collaboration_request(&mut self, mutation: Option<(String, Value)>) -> Vec<Action> {
        let state = &mut self.collaboration;
        if state.loading {
            return Vec::new();
        }
        state.generation = state.generation.wrapping_add(1);
        state.loading = true;
        state.last_poll = Some(Instant::now());
        vec![Action::Collaboration(Request {
            generation: state.generation,
            mutation,
        })]
    }

    pub(crate) fn collaboration_reply(
        &mut self,
        session_id: &str,
        generation: u64,
        result: Result<Snapshot, String>,
    ) {
        if session_id != self.session.id || generation != self.collaboration.generation {
            return;
        }
        let state = &mut self.collaboration;
        state.loading = false;
        match result {
            Ok(snapshot) => {
                state.snapshot = Some(snapshot);
                state.error = None;
            }
            Err(error) => {
                state.snapshot = None;
                state.error = Some(clean(&error));
            }
        }
    }

    pub(crate) fn collaboration_poll(&mut self) -> Vec<Action> {
        let state = &self.collaboration;
        if matches!(self.overlay, Overlay::Collaboration)
            && state.editor.is_none()
            && state
                .last_poll
                .is_none_or(|at| at.elapsed() >= Duration::from_secs(5))
        {
            return self.collaboration_request(None);
        }
        Vec::new()
    }

    pub(crate) fn collaboration_key(&mut self, key: KeyEvent) -> Vec<Action> {
        if self.collaboration.editor.is_some() {
            return self.collaboration_edit(key);
        }
        match key.code {
            KeyCode::Esc => self.overlay = Overlay::None,
            KeyCode::Tab | KeyCode::BackTab => {
                let tab = &mut self.collaboration.tab;
                *tab = match tab {
                    Tab::People => Tab::Conversation,
                    Tab::Conversation => Tab::Tasks,
                    Tab::Tasks => Tab::People,
                };
                self.collaboration.selected = 0;
            }
            KeyCode::Up => {
                self.collaboration.selected = self.collaboration.selected.saturating_sub(1)
            }
            KeyCode::Down => {
                self.collaboration.selected = self.collaboration.selected.saturating_add(1)
            }
            KeyCode::Char('r') => return self.collaboration_request(None),
            KeyCode::Char('n' | 'd' | 'c' | 'b') => return self.collaboration_command(key.code),
            _ => {}
        }
        Vec::new()
    }

    fn collaboration_command(&mut self, key: KeyCode) -> Vec<Action> {
        let Some(snapshot) = &self.collaboration.snapshot else {
            return Vec::new();
        };
        if !snapshot.writable() || self.collaboration.loading {
            return Vec::new();
        }
        let selected = self.collaboration.selected;
        let edit = match (self.collaboration.tab, key) {
            (Tab::People, KeyCode::Char('d')) => {
                let who = snapshot
                    .members
                    .iter()
                    .map(|v| ("human", v))
                    .chain(snapshot.agents.iter().map(|v| ("agent", v)))
                    .nth(selected);
                who.map(|(kind, v)| {
                    Edit::Message(
                        json!({"kind":"direct", "participant":{"kind":kind,"id":v["id"]}}),
                    )
                })
            }
            (Tab::Conversation, KeyCode::Char('n')) => {
                Some(Edit::Message(json!({"kind":"project"})))
            }
            (Tab::Tasks, KeyCode::Char('n')) => Some(Edit::Task),
            (Tab::Tasks, KeyCode::Char('c' | 'd' | 'b')) => return self.collaboration_task(key),
            _ => None,
        };
        if let Some(kind) = edit {
            let title = if matches!(kind, Edit::Task) {
                "New task title"
            } else {
                "Human message"
            };
            self.collaboration.editor = Some(Editor {
                title,
                input: String::new(),
                kind,
            });
        }
        Vec::new()
    }

    fn collaboration_task(&mut self, key: KeyCode) -> Vec<Action> {
        let Some(snapshot) = &self.collaboration.snapshot else {
            return Vec::new();
        };
        let Some(task) = snapshot.tasks.get(self.collaboration.selected) else {
            return Vec::new();
        };
        let claim = key == KeyCode::Char('c');
        let path = format!(
            "/collaboration/tasks/{}/{}",
            encode(task["id"].as_str().unwrap_or_default()),
            if claim { "claim" } else { "update" }
        );
        let mut body = json!({"expected_revision":task["revision"],"claim_generation":task["claim_generation"]});
        if claim {
            body.as_object_mut().unwrap().remove("claim_generation");
        } else {
            body["status"] = json!(if key == KeyCode::Char('d') {
                "done"
            } else {
                "blocked"
            });
        }
        self.collaboration_mutate(path, body)
    }

    pub(crate) fn collaboration_paste(&mut self, text: &str) -> bool {
        if !matches!(self.overlay, Overlay::Collaboration) {
            return false;
        }
        if let Some(editor) = &mut self.collaboration.editor {
            let text = clean(&text.replace(['\r', '\n'], " "));
            for c in text.chars() {
                if editor.input.len() + c.len_utf8() > editor.limit() {
                    break;
                }
                editor.input.push(c);
            }
        }
        true
    }

    fn collaboration_edit(&mut self, key: KeyEvent) -> Vec<Action> {
        let Some(editor) = &mut self.collaboration.editor else {
            return Vec::new();
        };
        match key.code {
            KeyCode::Esc => self.collaboration.editor = None,
            KeyCode::Backspace => {
                editor.input.pop();
            }
            KeyCode::Char(c)
                if !c.is_control() && editor.input.len() + c.len_utf8() <= editor.limit() =>
            {
                editor.input.push(c)
            }
            KeyCode::Enter if !editor.input.trim().is_empty() => {
                let editor = self.collaboration.editor.take().unwrap();
                let (path, body) = match editor.kind {
                    Edit::Message(target) => (
                        "/collaboration/messages",
                        json!({"target":target,"text":editor.input}),
                    ),
                    Edit::Task => (
                        "/collaboration/tasks",
                        json!({"title":editor.input,"description":"","dependencies":[]}),
                    ),
                };
                return self.collaboration_mutate(path.into(), body);
            }
            _ => {}
        }
        Vec::new()
    }

    fn collaboration_mutate(&mut self, path: String, mut body: Value) -> Vec<Action> {
        let Some(snapshot) = &self.collaboration.snapshot else {
            return Vec::new();
        };
        body["session_id"] = json!(self.session.id);
        body["project"] = json!(snapshot.project());
        // Created once per user action, so any transport retry keeps the same key.
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        body["operation_id"] = json!(format!("tui-{}-{nonce}", self.collaboration.generation));
        self.collaboration_request(Some((path, body)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyModifiers;

    fn app(role: &str) -> App {
        let mut app = App::new(Session::default(), Vec::new(), "cyber");
        app.collaboration.snapshot = Some(Snapshot {
            status: json!({"enabled":true,"binding":{"project":"cprj_fixture"},"current_member":{"role":role}}),
            members: vec![
                json!({"id":"cmem_alice","name":"Alice\u{001b}[31m","kind":"guest","role":role,"active":true}),
            ],
            agents: vec![
                json!({"id":"cagt_bob","name":"Builder","owner":"cmem_alice","online":false}),
            ],
            tasks: vec![
                json!({"id":"ctask_one","title":"Test","status":"todo","revision":3,"claim_generation":2}),
            ],
            ..Default::default()
        });
        app.overlay = Overlay::Collaboration;
        app
    }
    fn key(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
    }

    #[test]
    fn queued_mutation_notice_preserves_receipt_without_claiming_it_was_applied() {
        assert_eq!(
            queued_notice(
                &json!({"status":"queued","operation_id":"cop_pending","message":"Do not resend"})
            )
            .as_deref(),
            Some("Queued operation cop_pending: Do not resend")
        );
        assert!(queued_notice(&json!({"id":"ctask_applied"})).is_none());
    }

    #[test]
    fn paste_stays_in_the_editor_with_utf8_api_bounds_and_never_reaches_the_composer() {
        let mut app = app("member");
        app.collaboration.tab = Tab::Tasks;
        app.collaboration_key(key('n'));
        app.on_paste(&format!("\u{001b}{}", "é".repeat(200)));
        assert_eq!(app.collaboration.editor.as_ref().unwrap().input.len(), 300);
        assert!(app.composer.is_empty());
    }

    #[test]
    fn viewer_cannot_create_send_or_claim() {
        let mut app = app("viewer");
        for tab in [Tab::People, Tab::Conversation, Tab::Tasks] {
            app.collaboration.tab = tab;
            for c in ['d', 'n', 'c', 'b'] {
                assert!(app.collaboration_key(key(c)).is_empty());
                assert!(app.collaboration.editor.is_none());
            }
        }
    }

    #[test]
    fn direct_message_has_explicit_target_and_no_claimed_author() {
        let mut app = app("member");
        assert!(app.collaboration_key(key('d')).is_empty());
        for c in "hello".chars() {
            app.collaboration_key(key(c));
        }
        let actions = app.collaboration_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        let Action::Collaboration(request) = &actions[0] else {
            panic!("expected mutation")
        };
        let (path, body) = request.mutation.as_ref().unwrap();
        assert_eq!(path, "/collaboration/messages");
        assert_eq!(
            body["target"],
            json!({"kind":"direct","participant":{"kind":"human","id":"cmem_alice"}})
        );
        assert_eq!(body["text"], "hello");
        assert!(body.get("author").is_none());
        assert!(body["operation_id"].as_str().unwrap().starts_with("tui-"));
    }

    #[test]
    fn task_updates_carry_revision_and_claim_fence() {
        let mut app = app("owner");
        app.collaboration.tab = Tab::Tasks;
        let actions = app.collaboration_key(key('d'));
        let Action::Collaboration(request) = &actions[0] else {
            panic!("expected mutation")
        };
        let (path, body) = request.mutation.as_ref().unwrap();
        assert_eq!(path, "/collaboration/tasks/ctask_one/update");
        assert_eq!(body["expected_revision"], 3);
        assert_eq!(body["claim_generation"], 2);
        assert_eq!(body["status"], "done");
    }

    #[test]
    fn failed_refresh_forgets_roster_and_stale_reply_cannot_restore_it() {
        let mut app = app("member");
        let actions = app.open_collaboration();
        let Action::Collaboration(request) = &actions[0] else {
            panic!("expected refresh")
        };
        let generation = request.generation;
        let previous = app.collaboration.snapshot.clone().unwrap();
        app.collaboration_reply("another-session", generation, Ok(previous.clone()));
        assert!(app.collaboration.loading);
        let id = app.session.id.clone();
        app.collaboration_reply(&id, generation, Err("Access revoked\u{001b}".into()));
        assert!(app.collaboration.snapshot.is_none());
        assert_eq!(app.collaboration.error.as_deref(), Some("Access revoked"));
        app.collaboration_reply(&id, generation.wrapping_sub(1), Ok(previous));
        assert!(app.collaboration.snapshot.is_none());
    }

    #[test]
    fn forgetting_a_session_invalidates_its_old_replies_even_when_returning_to_it() {
        let mut app = app("member");
        let previous = app.collaboration.snapshot.clone().unwrap();
        let actions = app.open_collaboration();
        let Action::Collaboration(request) = &actions[0] else {
            panic!("expected refresh")
        };
        let generation = request.generation;
        app.collaboration.forget();
        app.open_collaboration();
        let id = app.session.id.clone();
        app.collaboration_reply(&id, generation, Ok(previous));
        assert!(app.collaboration.snapshot.is_none());
        assert!(app.collaboration.loading);
    }

    #[test]
    fn screen_labels_guests_and_offline_agents_without_terminal_controls() {
        let mut app = app("member");
        let screen = crate::screens::render(&mut app, 140, 30, &crate::screens::Opts::default());
        assert!(screen.contains("[human, guest]"));
        assert!(screen.contains("offline"));
        assert!(screen.contains("receiver consent"));
        assert!(!screen.contains('\u{001b}'));
    }
}
