//! Reviewed recovery never edits the composer or infers native acknowledgement.
use crate::{
    app::{Action, App, Overlay},
    rewind,
};
use crossterm::event::KeyCode;
use cyber_server::runtime::{
    SnapshotRecovery, SnapshotRecoveryAction as RecoveryAction,
    SnapshotRecoveryConfirmation as Confirmation, SnapshotRecoveryResult as Outcome,
    SnapshotRecoveryReview as Review, SnapshotRecoverySelection as Selection,
    SnapshotRecoveryState,
};
#[path = "recovery_client.rs"]
mod client;
#[path = "preparation_recovery.rs"]
mod preparation;
pub use client::perform;
#[path = "recovery_view.rs"]
mod view;
pub use view::lines;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Phase {
    #[default]
    Journals,
    Preparation,
    Requests,
    Review,
    Outcome,
}
#[derive(Debug, Default)]
pub struct View {
    pub phase: Phase,
    pub busy: bool,
    pub preparation: preparation::View,
    pub discovered: bool,
    pub journals: Vec<SnapshotRecovery>,
    pub requests: Vec<Confirmation>,
    pub selected: usize,
    pub journal: Option<String>,
    pub review: Option<Review>,
    pub request: Option<Confirmation>,
    pub outcome: Option<Outcome>,
}
#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    Preparation(preparation::Command),
    List,
    Discover(String),
    Review(Selection),
    Confirm(Confirmation),
    Lookup(Confirmation),
    Acknowledge(Confirmation),
}
#[derive(Debug, Clone)]
pub enum Reply {
    Preparation(Box<preparation::Reply>),
    Journals(Vec<SnapshotRecovery>),
    Requests(Vec<Confirmation>),
    Review(Box<Review>),
    Outcome(Box<Outcome>),
}

impl App {
    pub(crate) fn open_snapshot_recovery(&mut self) -> Vec<Action> {
        self.overlay = Overlay::Rewind;
        self.rewind.preview = None;
        self.rewind.recovery = Default::default();
        self.rewind.scroll = 0;
        self.recovery_request(Command::List)
    }
    pub(crate) fn recovery_request(&mut self, command: Command) -> Vec<Action> {
        match &command {
            Command::Discover(_) => self.rewind.recovery.discovered = false,
            Command::Review(_) => self.rewind.recovery.review = None,
            Command::Lookup(_) | Command::Acknowledge(_) => self.rewind.recovery.outcome = None,
            _ => {}
        }
        let actions = self.rewind_request(rewind::Command::Recovery(command));
        self.rewind.phase = rewind::Phase::Recovery;
        self.rewind.recovery.busy = true;
        actions
    }
    pub(crate) fn recovery_key(&mut self, key: KeyCode) -> Vec<Action> {
        if key == KeyCode::Esc {
            self.rewind.invalidate();
            self.overlay = Overlay::None;
            return vec![];
        }
        if key == KeyCode::PageUp {
            self.rewind.scroll = self.rewind.scroll.saturating_sub(10);
            return vec![];
        }
        if key == KeyCode::PageDown {
            self.rewind.scroll = self.rewind.scroll.saturating_add(10);
            return vec![];
        }
        if self.rewind.recovery.busy {
            return vec![];
        }
        if key == KeyCode::Char('p') {
            return self.preparation_request(preparation::Command::List);
        }
        match self.rewind.recovery.phase {
            Phase::Preparation => self.preparation_key(key),
            Phase::Journals => self.recovery_journal_key(key),
            Phase::Requests => self.recovery_requests_key(key),
            Phase::Review => self.recovery_review_key(key),
            Phase::Outcome => self.recovery_outcome_key(key),
        }
    }
    fn recovery_select(&mut self, key: KeyCode, length: usize) {
        match key {
            KeyCode::Up => {
                self.rewind.recovery.selected = self.rewind.recovery.selected.saturating_sub(1)
            }
            KeyCode::Down => {
                self.rewind.recovery.selected =
                    (self.rewind.recovery.selected + 1).min(length.saturating_sub(1))
            }
            _ => {}
        }
    }
    fn recovery_journal_key(&mut self, key: KeyCode) -> Vec<Action> {
        self.recovery_select(key, self.rewind.recovery.journals.len());
        match key {
            KeyCode::Char('r') => self.recovery_request(Command::List),
            KeyCode::Enter => {
                let Some(journal) = self
                    .rewind
                    .recovery
                    .journals
                    .get(self.rewind.recovery.selected)
                else {
                    return vec![];
                };
                self.recovery_request(Command::Discover(journal.id.clone()))
            }
            _ => vec![],
        }
    }
    fn recovery_requests_key(&mut self, key: KeyCode) -> Vec<Action> {
        self.recovery_select(key, self.rewind.recovery.requests.len());
        match key {
            KeyCode::Enter => {
                let Some(request) = self
                    .rewind
                    .recovery
                    .requests
                    .get(self.rewind.recovery.selected)
                    .cloned()
                else {
                    return vec![];
                };
                self.rewind.recovery.request = Some(request.clone());
                self.recovery_request(Command::Lookup(request))
            }
            KeyCode::Char('k' | 'b')
                if self.rewind.recovery.requests.is_empty() && self.rewind.recovery.discovered =>
            {
                self.recovery_review(key)
            }
            KeyCode::Char('r') => {
                let Some(journal) = self.rewind.recovery.journal.clone() else {
                    return vec![];
                };
                self.recovery_request(Command::Discover(journal))
            }
            _ => vec![],
        }
    }
    fn recovery_review(&mut self, key: KeyCode) -> Vec<Action> {
        let Some(journal_id) = self.rewind.recovery.journal.clone() else {
            return vec![];
        };
        self.rewind.recovery.review = None;
        self.rewind.scroll = 0;
        self.recovery_request(Command::Review(Selection {
            journal_id,
            action: if key == KeyCode::Char('k') {
                RecoveryAction::Keep
            } else {
                RecoveryAction::Rollback
            },
        }))
    }
    fn recovery_review_key(&mut self, key: KeyCode) -> Vec<Action> {
        match key {
            KeyCode::Up => self.rewind.scroll = self.rewind.scroll.saturating_sub(1),
            KeyCode::Down => self.rewind.scroll = self.rewind.scroll.saturating_add(1),
            KeyCode::Enter if !self.session.running => {
                let Some(review) = self.rewind.recovery.review.clone() else {
                    return vec![];
                };
                let Some(confirmation) = review.confirmation else {
                    return vec![];
                };
                let request = Confirmation {
                    journal_id: review.journal_id,
                    action: review.action,
                    confirmation,
                    request_id: cyber_core::ids::new_id("recovery"),
                };
                self.rewind.recovery.request = Some(request.clone());
                self.rewind.recovery.outcome = None;
                self.rewind.recovery.review = None;
                self.rewind.recovery.phase = Phase::Outcome;
                return self.recovery_request(Command::Confirm(request));
            }
            KeyCode::Char('r') => {
                let Some(review) = self.rewind.recovery.review.clone() else {
                    return vec![];
                };
                return self.recovery_request(Command::Review(Selection {
                    journal_id: review.journal_id,
                    action: review.action,
                }));
            }
            _ => {}
        }
        vec![]
    }
    fn recovery_outcome_key(&mut self, key: KeyCode) -> Vec<Action> {
        let Some(request) = self.rewind.recovery.request.clone() else {
            return vec![];
        };
        match key {
            KeyCode::Char('r') => self.recovery_request(Command::Lookup(request)),
            KeyCode::Char('a')
                if self
                    .rewind
                    .recovery
                    .outcome
                    .as_ref()
                    .is_some_and(|outcome| {
                        outcome.state == SnapshotRecoveryState::Acknowledged
                    }) =>
            {
                self.recovery_request(Command::Acknowledge(request))
            }
            KeyCode::Char('k' | 'b')
                if self
                    .rewind
                    .recovery
                    .outcome
                    .as_ref()
                    .is_some_and(|outcome| may_review_again(&outcome.state)) =>
            {
                self.recovery_review(key)
            }
            _ => vec![],
        }
    }
}

fn may_review_again(state: &SnapshotRecoveryState) -> bool {
    matches!(
        state,
        SnapshotRecoveryState::CancelledBeforeOrigin
            | SnapshotRecoveryState::CancelledBeforeEffects
            | SnapshotRecoveryState::ResolvedByAnotherRequest
    )
}

/// Called only after rewind's Session/Location/generation/overlay guard.
pub(crate) fn apply(
    app: &mut App,
    request: &rewind::Request,
    result: Result<rewind::Reply, String>,
) -> Vec<Action> {
    if matches!(
        request.command,
        rewind::Command::Recovery(Command::Preparation(_))
    ) {
        return preparation::apply(app, request, result);
    }
    app.rewind.recovery.busy = false;
    match result {
        Err(error) => {
            app.rewind.recovery.review = None;
            app.rewind.recovery.outcome = None;
            app.rewind.error = Some(error);
        }
        Ok(rewind::Reply::Recovery(reply)) => {
            if let Err(error) = accept(&mut app.rewind.recovery, request, *reply) {
                app.rewind.recovery.review = None;
                app.rewind.recovery.outcome = None;
                app.rewind.error = Some(error);
            }
        }
        _ => app.rewind.error = Some("Unexpected recovery response; refresh without replay".into()),
    }
    vec![]
}
fn accept(view: &mut View, request: &rewind::Request, reply: Reply) -> Result<(), String> {
    let rewind::Command::Recovery(command) = &request.command else {
        return Err("Foreign recovery command".into());
    };
    match (command, reply) {
        (Command::List, Reply::Journals(journals)) => {
            view.journals = journals;
            view.phase = Phase::Journals;
            view.selected = 0;
        }
        (Command::Discover(journal), Reply::Requests(requests))
            if requests
                .iter()
                .all(|request| &request.journal_id == journal) =>
        {
            view.requests = requests;
            view.discovered = true;
            view.journal = Some(journal.clone());
            view.phase = Phase::Requests;
            view.selected = 0;
        }
        (Command::Review(selection), Reply::Review(review))
            if valid_review(&review, request, selection) =>
        {
            view.review = Some(*review);
            view.phase = Phase::Review;
        }
        (
            Command::Confirm(expected) | Command::Lookup(expected) | Command::Acknowledge(expected),
            Reply::Outcome(outcome),
        ) if valid_outcome(&outcome, request, expected) => {
            view.request = Some(expected.clone());
            view.outcome = Some(*outcome);
            view.phase = Phase::Outcome;
        }
        _ => {
            return Err(
                "Recovery reply does not match the selected owner; refresh without replay".into(),
            );
        }
    }
    Ok(())
}

fn valid_review(review: &Review, request: &rewind::Request, selection: &Selection) -> bool {
    review.journal_id == selection.journal_id
        && review.action == selection.action
        && (review.confirmation.is_none() || review.origin.is_some())
        && review.origin.as_ref().is_none_or(|origin| {
            origin.session_id == request.session
                && origin.binding.directory == std::path::Path::new(&request.directory)
                && origin.binding.journal_id == selection.journal_id
        })
}
fn valid_outcome(outcome: &Outcome, request: &rewind::Request, expected: &Confirmation) -> bool {
    outcome.request == *expected
        && outcome.original.session_id == request.session
        && outcome.original.binding.directory == std::path::Path::new(&request.directory)
        && outcome.original.binding.journal_id == expected.journal_id
}
#[cfg(test)]
#[path = "recovery_tests.rs"]
mod tests;
