//! Reviewed rewind and staged undo/redo. Every async reply carries the exact
//! Session, Location and UI generation; only acknowledged operations edit a draft.
use crate::{
    app::{Action, App, Overlay},
    model::{Item, Session},
    perform::Msg,
};
use crossterm::event::{KeyCode, KeyEvent};
use cyber_server::runtime::{
    RevertState, RevertTarget, RewindAction, RewindConfirmation, RewindPreview, RewindSelection,
};
use std::collections::BTreeMap;
use std::time::Instant;
#[path = "rewind_client.rs"]
mod client;
pub use client::perform;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Intent {
    #[default]
    Menu,
    Undo,
    Redo,
}
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Phase {
    #[default]
    Closed,
    Loading,
    Choose,
    Review,
    Busy,
    Waiting,
    Recovery,
}
#[derive(Debug, Clone)]
pub struct Boundary {
    pub id: String,
    pub text: String,
}
#[derive(Debug)]
struct Draft {
    original: String,
    restored: Option<String>,
}

#[derive(Debug)]
pub struct View {
    pub recovery: crate::recovery::View,
    pub generation: u64,
    pub phase: Phase,
    pub intent: Intent,
    pub action: RewindAction,
    pub target: RevertTarget,
    pub boundaries: Vec<Boundary>,
    pub selected: usize,
    pub preview: Option<RewindPreview>,
    pub staged: Option<RevertState>,
    pub error: Option<String>,
    pub code_error: Option<String>,
    pub scroll: u16,
    pub escape: Option<Instant>,
    drafts: BTreeMap<(String, String), Draft>,
}
impl Default for View {
    fn default() -> Self {
        Self {
            recovery: Default::default(),
            generation: 0,
            phase: Phase::Closed,
            intent: Intent::Menu,
            action: RewindAction::Commit,
            target: RevertTarget::Both,
            boundaries: vec![],
            selected: 0,
            preview: None,
            staged: None,
            error: None,
            code_error: None,
            scroll: 0,
            escape: None,
            drafts: BTreeMap::new(),
        }
    }
}
impl View {
    pub fn invalidate(&mut self) {
        self.recovery = Default::default();
        self.generation = self.generation.wrapping_add(1);
        self.phase = Phase::Closed;
        self.preview = None;
        self.boundaries.clear();
        self.staged = None;
        self.escape = None;
    }
    pub fn saved_draft(&self, session: &Session) -> Option<&str> {
        self.drafts
            .get(&(session.id.clone(), session.directory.clone()))
            .map(|draft| draft.original.as_str())
    }
}
#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    Recovery(crate::recovery::Command),
    Load,
    Interrupt,
    Preview(RewindSelection),
    Confirm(RewindConfirmation),
}
#[derive(Debug, Clone, PartialEq)]
pub struct Request {
    pub session: String,
    pub directory: String,
    pub generation: u64,
    pub draft: String,
    pub command: Command,
}
#[derive(Debug, Clone)]
pub enum Reply {
    Recovery(Box<crate::recovery::Reply>),
    Loaded(Box<Msg>),
    Interrupted(Box<Session>),
    Preview(Box<RewindPreview>),
    Changed {
        snapshot: Box<Msg>,
        operation: RevertState,
    },
}

impl App {
    pub(crate) fn refuse_attached_undo_command(&mut self) -> bool {
        let command = self.composer.text();
        if !self.attachments.chips.is_empty()
            && matches!(command.split_whitespace().next(), Some("/undo" | "/redo"))
        {
            self.toast("Keep or send the current attachments before undo/redo");
            return true;
        }
        false
    }
    pub(crate) fn forget_rewind(&mut self) {
        self.rewind.invalidate();
        if matches!(self.overlay, Overlay::Rewind) {
            self.overlay = Overlay::None;
        }
    }
    pub(crate) fn rewind_escape(&mut self) -> Vec<Action> {
        let now = self.now();
        if self.rewind.escape.take().is_some_and(|first| {
            now.saturating_duration_since(first) <= std::time::Duration::from_millis(500)
        }) {
            self.open_rewind(Intent::Menu)
        } else {
            self.rewind.escape = Some(now);
            vec![]
        }
    }
    pub(crate) fn open_rewind(&mut self, intent: Intent) -> Vec<Action> {
        if matches!(
            self.overlay,
            Overlay::Permission(_) | Overlay::Question(_) | Overlay::ConfirmBypass
        ) {
            return vec![];
        }
        if intent != Intent::Menu && !self.attachments.chips.is_empty() {
            self.toast("Keep or send the current attachments before undo/redo; /rewind preserves the composer");
            return vec![];
        }
        self.rewind.invalidate();
        self.rewind.intent = intent;
        self.rewind.error = None;
        self.rewind.code_error = None;
        self.rewind.target = RevertTarget::Both;
        self.overlay = Overlay::Rewind;
        if self.session.running {
            self.rewind.phase = Phase::Busy;
            vec![]
        } else {
            self.rewind_request(Command::Load)
        }
    }
    pub(crate) fn rewind_request(&mut self, command: Command) -> Vec<Action> {
        self.rewind.generation = self.rewind.generation.wrapping_add(1);
        self.rewind.phase = Phase::Loading;
        self.rewind.error = None;
        vec![Action::Rewind(Request {
            session: self.session.id.clone(),
            directory: self.session.directory.clone(),
            generation: self.rewind.generation,
            draft: self.composer.text(),
            command,
        })]
    }
    fn rewind_preview_request(&mut self) -> Vec<Action> {
        let Some(boundary) = self.rewind.boundaries.get(self.rewind.selected) else {
            return vec![];
        };
        let selection = RewindSelection {
            action: self.rewind.action,
            message_id: boundary.id.clone(),
            target: self.rewind.target,
            expected_directory: self.session.directory.clone(),
        };
        self.rewind.preview = None;
        self.rewind.scroll = 0;
        self.rewind_request(Command::Preview(selection))
    }
    pub(crate) fn rewind_key(&mut self, key: KeyEvent) -> Vec<Action> {
        if self.rewind.phase == Phase::Recovery {
            return self.recovery_key(key.code);
        }
        if key.code == KeyCode::Char('c') && !matches!(self.rewind.phase, Phase::Loading) {
            return self.open_snapshot_recovery();
        }
        if key.code == KeyCode::Esc {
            self.rewind.generation = self.rewind.generation.wrapping_add(1);
            self.rewind.preview = None;
            if self.rewind.phase == Phase::Review {
                self.rewind.phase = Phase::Choose;
            } else {
                self.rewind.invalidate();
                self.overlay = Overlay::None;
            }
            return vec![];
        }
        match self.rewind.phase {
            Phase::Busy => {
                if key.code == KeyCode::Char('i') {
                    self.rewind_request(Command::Interrupt)
                } else {
                    vec![]
                }
            }
            Phase::Waiting => {
                if key.code == KeyCode::Char('r') {
                    self.rewind_request(Command::Load)
                } else {
                    vec![]
                }
            }
            Phase::Choose => self.rewind_choose(key.code),
            Phase::Review => self.rewind_review_key(key.code),
            _ => vec![],
        }
    }
    fn rewind_choose(&mut self, key: KeyCode) -> Vec<Action> {
        match key {
            KeyCode::Up => self.rewind.selected = self.rewind.selected.saturating_sub(1),
            KeyCode::Down => {
                self.rewind.selected =
                    (self.rewind.selected + 1).min(self.rewind.boundaries.len().saturating_sub(1))
            }
            KeyCode::Char('1' | '2' | '3') if self.rewind.staged.is_none() => {
                self.rewind.target = match key {
                    KeyCode::Char('1') => RevertTarget::Code,
                    KeyCode::Char('2') => RevertTarget::Conversation,
                    _ => RevertTarget::Both,
                };
            }
            KeyCode::Char('u') => return self.open_rewind(Intent::Undo),
            KeyCode::Char('d') => return self.open_rewind(Intent::Redo),
            KeyCode::Char('p') => {
                if let Some(draft) = self.rewind.saved_draft(&self.session) {
                    return vec![Action::Copy(draft.into())];
                }
            }
            KeyCode::Char('r') => return self.rewind_request(Command::Load),
            KeyCode::Enter => return self.rewind_preview_request(),
            _ => {}
        }
        vec![]
    }
    fn rewind_review_key(&mut self, key: KeyCode) -> Vec<Action> {
        match key {
            KeyCode::Up => self.rewind.scroll = self.rewind.scroll.saturating_sub(1),
            KeyCode::Down => self.rewind.scroll = self.rewind.scroll.saturating_add(1),
            KeyCode::PageUp => self.rewind.scroll = self.rewind.scroll.saturating_sub(10),
            KeyCode::PageDown => self.rewind.scroll = self.rewind.scroll.saturating_add(10),
            KeyCode::Char('r') => return self.rewind_preview_request(),
            KeyCode::Enter => {
                let Some(preview) = self.rewind.preview.clone() else {
                    return vec![];
                };
                if self.session.seq != preview.sequence {
                    self.rewind.preview = None;
                    self.rewind.phase = Phase::Choose;
                    self.rewind.error = Some("Session changed; review a fresh preview".into());
                    return vec![];
                }
                if preview.action != RewindAction::Commit && !self.attachments.chips.is_empty() {
                    self.rewind.error = Some(
                        "Keep or send current attachments before changing the composer".into(),
                    );
                    return vec![];
                }
                let scope = (self.session.id.clone(), self.session.directory.clone());
                if preview.action == RewindAction::Stage
                    && !self.rewind.drafts.contains_key(&scope)
                    && self.rewind.drafts.len() >= 32
                {
                    self.rewind.error=Some("Saved undo drafts are full; recover an earlier draft before starting another".into());
                    return vec![];
                }
                return self.rewind_request(Command::Confirm(RewindConfirmation {
                    selection: RewindSelection {
                        action: preview.action,
                        message_id: preview.message_id,
                        target: preview.target,
                        expected_directory: preview.directory,
                    },
                    confirmation: preview.confirmation,
                }));
            }
            _ => {}
        }
        vec![]
    }
    fn rewind_loaded(&mut self) -> Vec<Action> {
        self.rewind.boundaries = self
            .items
            .iter()
            .filter_map(|item| match item {
                Item::User { id, text, .. } => Some(Boundary {
                    id: id.clone(),
                    text: text.clone(),
                }),
                _ => None,
            })
            .collect();
        self.rewind.staged = self.session.revert.clone();
        if self.session.running {
            self.rewind.phase = Phase::Waiting;
            return vec![];
        }
        self.rewind.phase = Phase::Choose;
        if self.rewind.boundaries.is_empty() {
            self.rewind.error = Some("No active user boundaries to rewind".into());
            return vec![];
        }
        let current = self.session.revert.as_ref().and_then(|r| {
            self.rewind
                .boundaries
                .iter()
                .position(|b| b.id == r.message_id)
        });
        self.rewind.action = match self.rewind.intent {
            Intent::Menu => RewindAction::Commit,
            _ => RewindAction::Stage,
        };
        if let Some(staged) = &self.session.revert {
            self.rewind.target = staged.target;
        }
        self.rewind.selected = match (self.rewind.intent, current) {
            (Intent::Undo, Some(0)) => {
                self.rewind.error =
                    Some("Already at the oldest boundary; /redo moves forward".into());
                return vec![];
            }
            (Intent::Undo, Some(i)) => i - 1,
            (Intent::Redo, Some(i)) if i + 1 < self.rewind.boundaries.len() => i + 1,
            (Intent::Redo, Some(i)) => {
                self.rewind.action = RewindAction::Clear;
                i
            }
            (Intent::Redo, None) => {
                self.rewind.error = Some("No staged undo to redo".into());
                return vec![];
            }
            _ => self.rewind.boundaries.len() - 1,
        };
        if self.rewind.intent == Intent::Menu {
            vec![]
        } else {
            self.rewind_preview_request()
        }
    }
}

pub(crate) fn apply(
    app: &mut App,
    refresh: &mut crate::runner::Refresh,
    request: Request,
    result: Result<Reply, String>,
) -> Vec<Action> {
    if app.session.id != request.session
        || app.session.directory != request.directory
        || app.rewind.generation != request.generation
        || !matches!(app.overlay, Overlay::Rewind)
    {
        return vec![];
    }
    if matches!(&request.command, Command::Recovery(_)) {
        return crate::recovery::apply(app, &request, result);
    }
    match result {
        Ok(Reply::Recovery(_)) => vec![],
        Err(error) => {
            if matches!(&request.command,Command::Preview(s) if s.target.code()) {
                app.rewind.code_error = Some(error.clone());
            }
            app.rewind.preview = None;
            app.rewind.phase = Phase::Choose;
            app.rewind.error = Some(format!(
                "{error}. No acknowledged composer change; R reloads status."
            ));
            vec![]
        }
        Ok(Reply::Preview(preview)) => {
            let preview = *preview;
            if !matches!(&request.command,Command::Preview(s) if s.action==preview.action && s.message_id==preview.message_id && s.target==preview.target)
                || preview.session_id != request.session
                || preview.directory != request.directory
                || preview.sequence != app.session.seq
            {
                app.rewind.phase = Phase::Choose;
                app.rewind.error = Some("Preview context changed; refresh".into());
                return vec![];
            }
            if preview.code.is_some() {
                app.rewind.code_error = None;
            }
            app.rewind.preview = Some(preview);
            app.rewind.phase = Phase::Review;
            vec![]
        }
        Ok(Reply::Interrupted(session)) => {
            if session.id != request.session || session.directory != request.directory {
                return vec![];
            }
            app.rewind.phase = Phase::Waiting;
            if session.running {
                vec![]
            } else {
                app.rewind_request(Command::Load)
            }
        }
        Ok(Reply::Loaded(snapshot)) => apply_snapshot(app, refresh, &request, *snapshot, None),
        Ok(Reply::Changed {
            snapshot,
            operation,
        }) => apply_snapshot(app, refresh, &request, *snapshot, Some(operation)),
    }
}
fn apply_snapshot(
    app: &mut App,
    refresh: &mut crate::runner::Refresh,
    request: &Request,
    snapshot: Msg,
    operation: Option<RevertState>,
) -> Vec<Action> {
    let Msg::Snapshot { session, .. } = &snapshot else {
        return vec![];
    };
    if session.id != request.session
        || session.directory != request.directory
        || session.seq < app.session.seq
    {
        app.rewind.phase = Phase::Choose;
        app.rewind.error = Some("Session advanced; reload before another rewind".into());
        return vec![];
    }
    let mut actions = crate::runner::apply(app, Ok(snapshot), refresh);
    if !matches!(app.overlay, Overlay::Rewind) {
        return actions;
    }
    if let Some(operation) = operation {
        finish(app, request, operation);
    } else {
        actions.extend(app.rewind_loaded());
    }
    actions
}
fn finish(app: &mut App, request: &Request, operation: RevertState) {
    let Command::Confirm(confirmation) = &request.command else {
        return;
    };
    let selection = &confirmation.selection;
    let acknowledged =
        operation.message_id == selection.message_id && operation.target == selection.target;
    let matching_stage = match selection.action {
        RewindAction::Stage => app.session.revert.as_ref() == Some(&operation),
        _ => app.session.revert.is_none(),
    };
    if !acknowledged || !matching_stage {
        app.rewind.phase = Phase::Choose;
        app.rewind.error = Some("Session advanced after the operation; composer kept".into());
        return;
    }
    let scope = (request.session.clone(), request.directory.clone());
    let unchanged = app.composer.text() == request.draft && app.attachments.chips.is_empty();
    match selection.action {
        RewindAction::Stage => {
            let checkpoint = app.rewind.drafts.entry(scope).or_insert_with(|| Draft {
                original: request.draft.clone(),
                restored: None,
            });
            let may_replace = checkpoint
                .restored
                .as_deref()
                .map_or(checkpoint.original == request.draft, |text| {
                    text == request.draft
                });
            if unchanged
                && may_replace
                && let Some(boundary) = app
                    .rewind
                    .boundaries
                    .iter()
                    .find(|b| b.id == selection.message_id)
            {
                app.composer.set_text(&boundary.text);
                checkpoint.restored = Some(boundary.text.clone());
            }
        }
        RewindAction::Clear if unchanged => {
            if app
                .rewind
                .drafts
                .get(&scope)
                .is_some_and(|draft| draft.restored.as_deref() == Some(request.draft.as_str()))
                && let Some(original) = app.rewind.drafts.remove(&scope)
            {
                app.composer.set_text(&original.original);
            }
        }
        _ => {}
    }
    app.rewind.invalidate();
    app.overlay = Overlay::None;
    let notice = match selection.action {
        RewindAction::Commit => {
            "Rewind committed; inactive branches and recovery evidence are retained"
        }
        RewindAction::Stage => {
            "Undo staged; the next prompt commits it. /redo reviews how to restore it. P in /rewind copies the original draft"
        }
        RewindAction::Clear => {
            "Undo cleared; newer composer text is preserved. P in /rewind copies any retained original draft"
        }
    };
    app.toast(notice);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyModifiers;
    fn app() -> App {
        App::new(
            Session {
                id: "ses_one".into(),
                directory: "/project".into(),
                ..Default::default()
            },
            vec![],
            "cyber",
        )
    }
    #[test]
    fn double_escape_requires_idle_unmodified_adjacent_presses_and_preserves_native_stop() {
        let mut app = app();
        let start = app.now();
        app.clock = Some(start);
        let esc = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);
        assert!(app.on_key(esc).is_empty());
        assert!(matches!(app.overlay, Overlay::None));
        app.clock = Some(start + std::time::Duration::from_millis(600));
        assert!(app.on_key(esc).is_empty());
        let actions = app.on_key(esc);
        assert!(matches!(actions.as_slice(), [Action::Rewind(_)]));
        assert!(matches!(app.overlay, Overlay::Rewind));
        assert_eq!(
            app.on_key(KeyEvent::new(
                KeyCode::Esc,
                KeyModifiers::CONTROL | KeyModifiers::ALT
            )),
            vec![Action::ComputerStop]
        );
        app.rewind.invalidate();
        app.overlay = Overlay::None;
        app.session.running = true;
        assert_eq!(app.on_key(esc), vec![Action::Interrupt]);
        assert!(matches!(app.overlay, Overlay::None));
    }
    #[test]
    fn optional_snapshot_warning_keeps_the_running_session_and_draft() {
        let mut app = app();
        app.session.running = true;
        app.composer.set_text("keep this draft");
        let actions = app.on_event(
            "session.error",
            &serde_json::json!({"kind":"snapshot","message":"storage cap reached"}),
        );
        assert!(app.session.running);
        assert_eq!(app.composer.text(), "keep this draft");
        assert_eq!(actions, vec![Action::Refresh]);
        assert!(app.toast.as_ref().unwrap().0.contains("snapshot warning"));
    }
    #[test]
    fn history_search_cancel_and_snapshot_warning_do_not_arm_rewind_or_replace_the_draft() {
        let mut app = app();
        app.composer = crate::composer::Composer::new(vec!["older prompt".into()]);
        app.composer.set_text("keep this draft");
        app.on_key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL));
        assert!(app.history_search.is_some());
        app.on_event(
            "session.error",
            &serde_json::json!({"kind":"snapshot","message":"cap reached"}),
        );
        assert!(app.history_search.is_some());
        let esc = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);
        assert!(app.on_key(esc).is_empty());
        assert!(app.history_search.is_none());
        assert!(app.rewind.escape.is_none());
        assert_eq!(app.composer.text(), "keep this draft");
        assert!(app.on_key(esc).is_empty());
        assert!(matches!(app.on_key(esc).as_slice(), [Action::Rewind(_)]));
        assert_eq!(app.composer.text(), "keep this draft");
    }
    #[test]
    fn permission_and_attachment_drafts_keep_their_priority() {
        let mut app = app();
        app.overlay = Overlay::Permission(crate::app::PermStep::Choose(0));
        assert!(app.open_rewind(Intent::Undo).is_empty());
        assert!(matches!(app.overlay, Overlay::Permission(_)));
        app.overlay = Overlay::None;
        app.attachments.chips.push(crate::attachments::Chip {
            kind: crate::attachments::Kind::Text,
            number: 1,
            payload: crate::attachments::Payload::Text {
                text: "private pasted input".into(),
            },
        });
        app.composer.set_text("[Pasted text #1 +1 lines]");
        assert!(app.open_rewind(Intent::Undo).is_empty());
        assert_eq!(app.attachments.chips.len(), 1);
        assert!(app.composer.text().contains("Pasted text"));
        app.composer.set_text("/undo [Pasted text #1 +1 lines]");
        assert!(
            app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
                .is_empty()
        );
        assert_eq!(app.attachments.chips.len(), 1);
        assert_eq!(app.composer.text(), "/undo [Pasted text #1 +1 lines]");
    }
}
