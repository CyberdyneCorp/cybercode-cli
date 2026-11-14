//! Pre-effect cancellation retains its exact request before sending any mutation.
use crate::{
    app::{Action, App},
    recovery, rewind,
};
use crossterm::event::KeyCode;
use cyber_server::runtime::{
    SnapshotLifetimeBinding, SnapshotOrigin, SnapshotPreparation as Candidate,
    SnapshotPreparationConfirmation as Confirmation, SnapshotPreparationReceipt as Receipt,
    SnapshotPreparationReview as Review,
};

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    #[default]
    Candidates,
    Review,
    Outcome,
}
#[derive(Debug, Default)]
pub struct View {
    pub phase: Phase,
    pub candidates: Vec<Candidate>,
    pub selected: usize,
    pub activity: Option<String>,
    pub review: Option<Review>,
    pub request: Option<Confirmation>,
    pub receipt: Option<Receipt>,
    pub reconciled: bool,
}
#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    List,
    Discover(String),
    Review(String),
    Confirm(String, Confirmation),
    Lookup(String, Confirmation),
    Acknowledge(String, Confirmation),
}
#[derive(Debug, Clone)]
pub enum Reply {
    Candidates(Vec<Candidate>),
    Discovered(Option<Box<Receipt>>),
    Review(Box<Review>),
    Receipt(Box<Receipt>),
}

impl App {
    pub(crate) fn preparation_request(&mut self, command: Command) -> Vec<Action> {
        self.rewind.recovery.preparation.review = None;
        self.rewind.recovery.preparation.receipt = None;
        self.rewind.recovery.preparation.reconciled = false;
        self.rewind.recovery.phase = recovery::Phase::Preparation;
        self.rewind.scroll = 0;
        self.recovery_request(recovery::Command::Preparation(command))
    }
    pub(crate) fn preparation_key(&mut self, key: KeyCode) -> Vec<Action> {
        if key == KeyCode::Char('j') {
            return self.recovery_request(recovery::Command::List);
        }
        let view = &mut self.rewind.recovery.preparation;
        if view.phase == Phase::Candidates {
            return self.preparation_candidates_key(key);
        }
        let Some(activity) = view.activity.clone() else {
            return vec![];
        };
        match (view.phase, key) {
            (Phase::Review, KeyCode::Enter) if !self.session.running => {
                let Some(review) = view.review.take() else {
                    return vec![];
                };
                let confirmation = Confirmation {
                    confirmation: review.confirmation,
                    request_id: cyber_core::ids::new_id("preparation"),
                };
                view.request = Some(confirmation.clone());
                view.phase = Phase::Outcome;
                self.preparation_request(Command::Confirm(activity, confirmation))
            }
            (Phase::Review, KeyCode::Char('r')) => {
                self.preparation_request(Command::Discover(activity))
            }
            (Phase::Outcome, KeyCode::Char('r' | 'a')) => {
                let Some(request) = view.request.clone() else {
                    return vec![];
                };
                if key == KeyCode::Char('a') && view.receipt.is_some() {
                    self.preparation_request(Command::Acknowledge(activity, request))
                } else if key == KeyCode::Char('r') {
                    self.preparation_request(Command::Lookup(activity, request))
                } else {
                    vec![]
                }
            }
            _ => vec![],
        }
    }
    fn preparation_candidates_key(&mut self, key: KeyCode) -> Vec<Action> {
        let view = &mut self.rewind.recovery.preparation;
        match key {
            KeyCode::Up => view.selected = view.selected.saturating_sub(1),
            KeyCode::Down => {
                view.selected = (view.selected + 1).min(view.candidates.len().saturating_sub(1))
            }
            KeyCode::Char('r') => return self.preparation_request(Command::List),
            KeyCode::Enter => {
                let Some(candidate) = view.candidates.get(view.selected) else {
                    return vec![];
                };
                let activity = candidate.activity_id.clone();
                view.activity = Some(activity.clone());
                view.request = None;
                return self.preparation_request(Command::Discover(activity));
            }
            _ => {}
        }
        vec![]
    }
}

pub(crate) fn apply(
    app: &mut App,
    request: &rewind::Request,
    result: Result<rewind::Reply, String>,
) -> Vec<Action> {
    app.rewind.recovery.busy = false;
    let rewind::Command::Recovery(recovery::Command::Preparation(command)) = &request.command
    else {
        return vec![];
    };
    let result = match result {
        Ok(rewind::Reply::Recovery(reply)) => match *reply {
            recovery::Reply::Preparation(reply) => accept(
                &mut app.rewind.recovery.preparation,
                request,
                command,
                *reply,
            ),
            _ => Err("Unexpected preparation response".into()),
        },
        Err(error) => Err(error),
        _ => Err("Unexpected preparation response".into()),
    };
    match result {
        Ok(Some(activity)) => app.preparation_request(Command::Review(activity)),
        Ok(None) => vec![],
        Err(error) => {
            app.rewind.recovery.preparation.review = None;
            app.rewind.recovery.preparation.receipt = None;
            app.rewind.error = Some(error);
            vec![]
        }
    }
}
fn accept(
    view: &mut View,
    request: &rewind::Request,
    command: &Command,
    reply: Reply,
) -> Result<Option<String>, String> {
    match (command, reply) {
        (Command::List, Reply::Candidates(candidates)) if candidates.len() <= 4096 => {
            *view = View { candidates, ..Default::default() };
        }
        (Command::Discover(activity), Reply::Discovered(None)) => return Ok(Some(activity.clone())),
        (Command::Discover(activity), Reply::Discovered(Some(receipt))) if valid_receipt(&receipt, request, activity) => {
            view.request = Some(Confirmation { confirmation: receipt.confirmation.clone(), request_id: receipt.request_id.clone() });
            view.receipt = Some(*receipt);
            view.phase = Phase::Outcome;
        }
        (Command::Review(activity), Reply::Review(review)) if valid_review(&review, request, activity) => {
            view.review = Some(*review);
            view.phase = Phase::Review;
        }
        (Command::Confirm(activity, expected) | Command::Lookup(activity, expected) | Command::Acknowledge(activity, expected), Reply::Receipt(receipt))
            if valid_receipt(&receipt, request, activity) && receipt.request_id == expected.request_id && receipt.confirmation == expected.confirmation => {
                view.receipt = Some(*receipt);
                view.reconciled = matches!(command, Command::Acknowledge(..));
                view.phase = Phase::Outcome;
            }
        _ => return Err("Preparation reply lacks matching ownership proof; retain the request and refresh without replay".into()),
    }
    Ok(None)
}
fn fingerprint(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}
fn owner_matches(
    lifetime: &SnapshotLifetimeBinding,
    origin: Option<&SnapshotOrigin>,
    request: &rewind::Request,
    activity: &str,
) -> bool {
    lifetime.owner.activity_id == activity
        && lifetime.owner.session_id == request.session
        && lifetime.owner.directory == std::path::Path::new(&request.directory)
        && origin.is_none_or(|origin| {
            origin.version == 1
                && lifetime.version == 2
                && origin.session_id == lifetime.owner.session_id
                && origin.activity_id == lifetime.owner.activity_id
                && origin.session_created_id == lifetime.owner.session_created_id
                && origin.worktree_id == lifetime.owner.worktree_id
                && origin.binding.directory == lifetime.owner.directory
                && origin.binding.root_identity == lifetime.root_identity
        })
}
fn valid_review(review: &Review, request: &rewind::Request, activity: &str) -> bool {
    review.activity_id == activity
        && review.lifetime.version == 2
        && review.lifetime.source_phase.is_some()
        && fingerprint(&review.confirmation)
        && owner_matches(&review.lifetime, review.origin.as_ref(), request, activity)
}
fn valid_receipt(receipt: &Receipt, request: &rewind::Request, activity: &str) -> bool {
    receipt.activity_id == activity
        && fingerprint(&receipt.confirmation)
        && !receipt.request_id.is_empty()
        && receipt.request_id.len() <= 256
        && !receipt.request_id.contains('\0')
        && receipt.source_admitted != Some(true)
        && (receipt.origin.is_none() || receipt.source_admitted == Some(false))
        && owner_matches(
            &receipt.lifetime,
            receipt.origin.as_ref(),
            request,
            activity,
        )
}

#[path = "preparation_recovery_view.rs"]
mod view;
pub use view::lines;
#[path = "preparation_recovery_client.rs"]
pub mod client;
#[cfg(test)]
#[path = "preparation_recovery_tests.rs"]
mod tests;
