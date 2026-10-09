//! Checkpoint bytes describe local work and previous intent; they never restore review authority.
use super::{Draft, Operation, Request, Scope, View};
use crate::{
    app::{Action, App},
    model::Session,
};
use cyber_core::memory::{MemoryClientStore, MemoryEditReview};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

const LIMIT: usize = 1024 * 1024;
const REQUEST_LIMIT: usize = 32;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredDraft {
    scope: Scope,
    directory: String,
    session_id: String,
    name: String,
    content: String,
    retained_key: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Checkpoint {
    version: u8,
    draft: Option<StoredDraft>,
    intents: Vec<Request>,
}
impl Checkpoint {
    fn empty(&self) -> bool {
        self.draft.is_none() && self.intents.is_empty()
    }
    fn validate(&self) -> Result<(), String> {
        if self.version != 1 || self.intents.len() > REQUEST_LIMIT {
            return Err(invalid());
        }
        if let Some(draft) = &self.draft {
            identity(&draft.directory, &draft.session_id)?;
            cyber_core::memory::validate_name(&draft.name).map_err(|_| invalid())?;
            if draft.content.len() > LIMIT
                || draft
                    .retained_key
                    .as_deref()
                    .is_some_and(|key| !valid_key(key))
            {
                return Err(invalid());
            }
        }
        for intent in &self.intents {
            validate_intent(intent)?;
        }
        Ok(())
    }
}
fn invalid() -> String {
    "Invalid retained memory client state; existing files were preserved".into()
}
fn valid_key(key: &str) -> bool {
    (1..=128).contains(&key.len()) && key.bytes().all(|b| (0x21..=0x7e).contains(&b))
}
fn valid_id(id: &str) -> bool {
    (1..=128).contains(&id.len())
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
}
fn identity(directory: &str, session: &str) -> Result<(), String> {
    if directory.len() > 16 * 1024 || !Path::new(directory).is_absolute() || !valid_id(session) {
        return Err(invalid());
    }
    Ok(())
}
pub(super) fn request_key(request: &Request) -> Option<&str> {
    match &request.operation {
        Operation::Save(save) => Some(&save.key),
        Operation::Delete { key, .. } | Operation::Recover { key, .. } => Some(key),
        _ => None,
    }
}
fn validate_intent(request: &Request) -> Result<(), String> {
    identity(&request.directory, &request.session_id)?;
    if !request_key(request).is_some_and(valid_key) {
        return Err(invalid());
    }
    match &request.operation {
        Operation::Save(save) => {
            cyber_core::memory::validate_name(&save.name).map_err(|_| invalid())?;
            if save.content.len() > LIMIT {
                return Err(invalid());
            }
            let document = cyber_core::memory::MemoryDocument::for_write(&save.content)
                .map_err(|_| invalid())?;
            if document.metadata.name != save.name {
                return Err(invalid());
            }
            cyber_server::http::validate_edit_fingerprint(Some(&save.fingerprint))
                .map_err(|_| invalid())?;
        }
        Operation::Delete { name, .. } => {
            cyber_core::memory::validate_name(name).map_err(|_| invalid())?;
        }
        Operation::Recover {
            mutation_id,
            receipt,
            storage,
            admission,
            ..
        } => {
            if !valid_id(mutation_id) || !valid_id(&receipt.id) {
                return Err(invalid());
            }
            cyber_core::memory::validate_name(&receipt.name).map_err(|_| invalid())?;
            cyber_server::http::RecoverMemory {
                storage_fingerprint: storage.clone(),
                admission_fingerprint: admission.clone(),
            }
            .validate()
            .map_err(|_| invalid())?;
        }
        _ => return Err(invalid()),
    }
    Ok(())
}
impl View {
    fn checkpoint(&self) -> Checkpoint {
        Checkpoint {
            version: 1,
            draft: self.draft.as_ref().map(|draft| StoredDraft {
                scope: draft.scope,
                directory: draft.directory.clone(),
                session_id: draft.session.clone(),
                name: draft.name.clone(),
                content: draft.composer.text(),
                retained_key: draft.retained_key.clone(),
            }),
            intents: self.intents.clone(),
        }
    }
    fn restore(&mut self, checkpoint: Checkpoint) {
        self.invalidate();
        self.recovery_key = None;
        self.intents = checkpoint.intents;
        self.draft = checkpoint.draft.map(|draft| {
            let session = Session {
                id: draft.session_id,
                directory: draft.directory,
                ..Default::default()
            };
            let mut restored = Draft::new(
                draft.scope,
                &session,
                MemoryEditReview {
                    name: draft.name,
                    original: None,
                    fingerprint: String::new(),
                },
            );
            restored.composer.set_text(&draft.content);
            restored.review = None;
            restored.retained_key = draft.retained_key;
            restored
        });
    }
    fn record(&mut self, request: &Request) -> Result<(), String> {
        validate_intent(request)?;
        if self.intents.contains(request) {
            return Ok(());
        }
        if self.intents.len() >= REQUEST_LIMIT {
            return Err("Retained memory request limit reached; reconcile prior outcomes before new effects".into());
        }
        self.intents.push(request.clone());
        Ok(())
    }
}

pub(crate) struct Persistence {
    state: PathBuf,
    owner: Option<MemoryClientStore>,
    previous: Checkpoint,
    blocked: Option<String>,
    failure: Option<(Checkpoint, String)>,
}
impl Persistence {
    pub fn load(state: &Path, view: &mut View) -> Self {
        let mut persistence = Self {
            state: state.into(),
            owner: None,
            previous: Checkpoint {
                version: 1,
                draft: None,
                intents: Vec::new(),
            },
            blocked: None,
            failure: None,
        };
        if let Err(error) = persistence.open_existing(view) {
            view.persistence_error = Some(error.clone());
            persistence.blocked = Some(error);
        }
        persistence
    }
    fn open_existing(&mut self, view: &mut View) -> Result<(), String> {
        self.owner = MemoryClientStore::existing(&self.state).map_err(|e| e.to_string())?;
        let Some(bytes) = self.owner.as_ref().and_then(MemoryClientStore::checkpoint) else {
            return Ok(());
        };
        let checkpoint: Checkpoint = serde_json::from_slice(bytes).map_err(|_| invalid())?;
        checkpoint.validate()?;
        self.previous = checkpoint.clone();
        view.restore(checkpoint);
        Ok(())
    }
    fn save(&mut self, view: &View, require_durable: bool, retry: bool) -> Result<(), String> {
        let checkpoint = view.checkpoint();
        checkpoint.validate()?;
        if checkpoint == self.previous && !require_durable {
            return Ok(());
        }
        if let Some(error) = &self.blocked {
            return Err(error.clone());
        }
        if !retry
            && let Some((failed, error)) = &self.failure
            && *failed == checkpoint
        {
            return Err(error.clone());
        }
        if self.owner.is_none() && checkpoint.empty() {
            return Ok(());
        }
        if self.owner.is_none() {
            self.owner = Some(MemoryClientStore::open(&self.state).map_err(|e| e.to_string())?);
        }
        let bytes = serde_json::to_vec(&checkpoint).map_err(|_| invalid())?;
        if let Err(error) = self.owner.as_mut().unwrap().save(&bytes) {
            let error = error.to_string();
            self.failure = Some((checkpoint, error.clone()));
            return Err(error);
        }
        self.failure = None;
        self.previous = checkpoint;
        Ok(())
    }
    /// Called before dispatch even for local input. Nothing restored becomes an action.
    pub fn gate(&mut self, app: &mut App, actions: Vec<Action>) -> Vec<Action> {
        let previous_intents = app.memory.intents.clone();
        let actions: Vec<_> = actions
            .into_iter()
            .filter(|action| {
                let Action::Memory(request) = action else {
                    return true;
                };
                if request_key(request).is_none() {
                    return true;
                }
                let valid = request.directory == app.session.directory
                    && request.session_id == app.session.id
                    && request.generation == app.memory.generation
                    && request.scope == app.memory.scope
                    && app.memory.pending;
                let result = if valid {
                    app.memory.record(request)
                } else {
                    Err("Memory request belongs to another Session/Location".into())
                };
                if let Err(error) = result {
                    app.memory.apply(request, Err(error));
                    return false;
                }
                true
            })
            .collect();
        let require_durable = actions.iter().any(
            |action| matches!(action, Action::Memory(request) if request_key(request).is_some()),
        );
        let retry = !actions.is_empty();
        match self.save(&app.memory, require_durable, retry) {
            Ok(()) => {
                app.memory.persistence_error = self.blocked.clone();
                actions
            }
            Err(error) => {
                app.memory.intents = previous_intents;
                if app.memory.persistence_error.as_ref() != Some(&error) {
                    app.toast("Memory checkpoint failed; effects and exit were withheld. Draft remains in this process.");
                }
                app.memory.persistence_error = Some(error.clone());
                actions
                    .into_iter()
                    .filter(|action| {
                        if let Action::Memory(request) = action
                            && request_key(request).is_some()
                        {
                            app.memory
                                .apply(request, Err(format!("Not dispatched: {error}")));
                            return false;
                        }
                        !matches!(action, Action::Quit)
                    })
                    .collect()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::{Data, Save};
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    fn session() -> Session {
        Session {
            id: "ses_test".into(),
            directory: std::env::temp_dir().to_string_lossy().into(),
            ..Default::default()
        }
    }
    fn app() -> App {
        App::new(session(), Vec::new(), "cyber")
    }
    fn content() -> String {
        "---\nname: policy\ndescription: Policy\ntype: reference\n---\n\nFact\n".into()
    }
    fn edit(app: &mut App) {
        let request = app
            .memory
            .request(Operation::EditReview("policy".into()), &app.session);
        app.memory.apply(
            &request,
            Ok(Data::EditReview(MemoryEditReview {
                name: "policy".into(),
                original: Some(content()),
                fingerprint: "a".repeat(64),
            })),
        );
        app.memory.paste("Retained Unicode 🦀 draft");
    }
    fn save_action(app: &mut App) -> Action {
        assert!(
            app.memory
                .key(
                    KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL),
                    &app.session
                )
                .is_none()
        );
        Action::Memory(
            app.memory
                .key(
                    KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE),
                    &app.session,
                )
                .unwrap(),
        )
    }
    #[test]
    fn checkpoint_schema_validates_version_scopes_identity_limits_and_mutation_payloads() {
        let mut app = app();
        edit(&mut app);
        let mut checkpoint = app.memory.checkpoint();
        assert!(checkpoint.validate().is_ok());
        checkpoint.version = 2;
        assert!(checkpoint.validate().is_err());
        checkpoint.version = 1;
        checkpoint.draft.as_mut().unwrap().directory = "relative".into();
        assert!(checkpoint.validate().is_err());
        checkpoint.draft.as_mut().unwrap().directory = session().directory;
        checkpoint.draft.as_mut().unwrap().content = "x".repeat(LIMIT + 1);
        assert!(checkpoint.validate().is_err());
        let unknown = br#"{"version":1,"draft":null,"intents":[],"review_authority":true}"#;
        assert!(serde_json::from_slice::<Checkpoint>(unknown).is_err());
        let request = Request {
            generation: 1,
            scope: Scope::Global,
            directory: session().directory,
            session_id: session().id,
            operation: Operation::Save(Box::new(Save {
                name: "policy".into(),
                content: content(),
                fingerprint: "a".repeat(64),
                key: "retained".into(),
            })),
        };
        assert!(validate_intent(&request).is_ok());
        let mut foreign = request.clone();
        foreign.operation = Operation::List;
        assert!(validate_intent(&foreign).is_err());
        foreign = request;
        let Operation::Save(save) = &mut foreign.operation else {
            panic!("save payload")
        };
        save.content = "invalid".into();
        assert!(validate_intent(&foreign).is_err());
    }
    #[cfg(unix)]
    #[test]
    fn restart_restores_draft_and_key_without_review_or_automatic_actions() {
        let state = tempfile::tempdir().unwrap();
        let mut original = app();
        let mut persistence = Persistence::load(state.path(), &mut original.memory);
        assert!(!state.path().join("memory-client").exists());
        edit(&mut original);
        let action = save_action(&mut original);
        let actions = persistence.gate(&mut original, vec![action]);
        let Action::Memory(request) = &actions[0] else {
            panic!("memory request")
        };
        let key = request_key(request).unwrap().to_string();
        let bytes = std::fs::read(state.path().join("memory-client/state.json")).unwrap();
        let checkpoint: Checkpoint = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(request_key(&checkpoint.intents[0]), Some(key.as_str()));
        drop(persistence);
        let mut resumed = app();
        let mut persistence = Persistence::load(state.path(), &mut resumed.memory);
        assert!(persistence.gate(&mut resumed, Vec::new()).is_empty());
        assert!(
            resumed
                .memory
                .draft
                .as_ref()
                .unwrap()
                .composer
                .text()
                .contains("Unicode 🦀")
        );
        assert!(resumed.memory.draft.as_ref().unwrap().review.is_none());
        assert!(!resumed.memory.pending);
        assert!(resumed.memory.draft.as_ref().unwrap().snapshot().is_err());
        assert_eq!(request_key(&resumed.memory.intents[0]), Some(key.as_str()));
        let mut foreign = resumed.session.clone();
        foreign.id = "ses_foreign".into();
        resumed.set_session(foreign);
        assert!(
            !resumed
                .memory
                .draft
                .as_ref()
                .unwrap()
                .belongs(Scope::Project, &resumed.session)
        );
        assert!(persistence.gate(&mut resumed, Vec::new()).is_empty());
        assert!(resumed.memory.draft.is_some());
    }
    #[cfg(unix)]
    #[test]
    fn external_checkpoint_edit_withholds_dispatch_and_exit_and_preserves_both_drafts() {
        let state = tempfile::tempdir().unwrap();
        let mut app = app();
        let mut persistence = Persistence::load(state.path(), &mut app.memory);
        edit(&mut app);
        persistence.gate(&mut app, Vec::new());
        let path = state.path().join("memory-client/state.json");
        std::fs::write(&path, b"external user edit").unwrap();
        let action = save_action(&mut app);
        assert!(
            persistence
                .gate(&mut app, vec![action, Action::Quit])
                .is_empty()
        );
        assert_eq!(std::fs::read(path).unwrap(), b"external user edit");
        assert!(
            app.memory
                .draft
                .as_ref()
                .unwrap()
                .composer
                .text()
                .contains("Retained Unicode")
        );
        assert!(app.memory.draft.as_ref().unwrap().review.is_none());
        assert!(app.memory.intents.is_empty());
        assert!(!app.memory.pending);
        assert!(!app.quit);
    }
    #[cfg(unix)]
    #[test]
    fn malformed_saved_state_is_preserved_and_never_replaced_by_new_intent() {
        let state = tempfile::tempdir().unwrap();
        let mut owner = MemoryClientStore::open(state.path()).unwrap();
        owner.save(b"invalid JSON").unwrap();
        drop(owner);
        let mut app = app();
        let mut persistence = Persistence::load(state.path(), &mut app.memory);
        assert!(app.memory.persistence_error.is_some());
        edit(&mut app);
        let action = save_action(&mut app);
        assert!(persistence.gate(&mut app, vec![action]).is_empty());
        assert_eq!(
            std::fs::read(state.path().join("memory-client/state.json")).unwrap(),
            b"invalid JSON"
        );
    }
    #[cfg(unix)]
    #[test]
    fn stale_actions_cannot_dispatch_and_successful_discard_persists_a_tombstone() {
        let state = tempfile::tempdir().unwrap();
        let mut app = app();
        let mut persistence = Persistence::load(state.path(), &mut app.memory);
        edit(&mut app);
        persistence.gate(&mut app, Vec::new());
        let stale = save_action(&mut app);
        app.memory.invalidate();
        assert!(persistence.gate(&mut app, vec![stale]).is_empty());
        app.memory.draft = None;
        persistence.gate(&mut app, Vec::new());
        drop(persistence);
        let mut resumed = self::app();
        Persistence::load(state.path(), &mut resumed.memory);
        assert!(resumed.memory.draft.is_none());
        assert!(resumed.memory.intents.is_empty());
    }
    #[cfg(unix)]
    #[test]
    fn unchanged_failed_checkpoint_waits_for_explicit_activity_instead_of_rewriting_on_ticks() {
        let state = tempfile::tempdir().unwrap();
        let mut app = app();
        let mut persistence = Persistence::load(state.path(), &mut app.memory);
        edit(&mut app);
        persistence.gate(&mut app, Vec::new());
        let path = state.path().join("memory-client/state.json");
        let original = std::fs::read(&path).unwrap();
        app.memory.paste("additional draft text");
        std::fs::write(&path, b"external edit").unwrap();
        persistence.gate(&mut app, Vec::new());
        std::fs::write(&path, &original).unwrap();
        assert!(persistence.gate(&mut app, Vec::new()).is_empty());
        assert_eq!(std::fs::read(&path).unwrap(), original);
        let request = app.memory.request(Operation::List, &app.session);
        assert_eq!(
            persistence
                .gate(&mut app, vec![Action::Memory(request)])
                .len(),
            1
        );
        let checkpoint: Checkpoint = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        assert!(
            checkpoint
                .draft
                .unwrap()
                .content
                .ends_with("additional draft text")
        );
    }

    #[cfg(not(unix))]
    #[test]
    fn unsupported_private_retention_refuses_memory_effects_before_creation() {
        let state = tempfile::tempdir().unwrap();
        let mut app = app();
        let mut persistence = Persistence::load(state.path(), &mut app.memory);
        edit(&mut app);
        let action = save_action(&mut app);
        assert!(persistence.gate(&mut app, vec![action]).is_empty());
        assert!(!state.path().join("memory-client").exists());
        assert!(app.memory.draft.is_some());
    }
}

#[cfg(test)]
mod recovery_restore_tests {
    use super::*;
    use crate::memory::Data;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    #[test]
    fn restored_recovery_key_is_only_a_scoped_read_and_never_restores_confirmation() {
        let session = Session {
            id: "ses_test".into(),
            directory: std::env::temp_dir().to_string_lossy().into(),
            ..Default::default()
        };
        let request = Request {
            generation: 7,
            scope: Scope::Global,
            directory: session.directory.clone(),
            session_id: session.id.clone(),
            operation: Operation::Recover {
                mutation_id: "mwr_prior".into(),
                receipt: Box::new(cyber_core::memory::MemoryMutation {
                    id: "mem_prior".into(),
                    name: "policy".into(),
                    deleted: false,
                }),
                storage: "a".repeat(64),
                admission: format!("sha256:{}", "b".repeat(64)),
                key: "retained-recovery".into(),
            },
        };
        let checkpoint = Checkpoint {
            version: 1,
            draft: None,
            intents: vec![request],
        };
        checkpoint.validate().unwrap();
        let mut view = View::default();
        view.restore(checkpoint);
        assert!(view.confirmation.is_none());
        assert!(view.review.is_none());
        assert!(!view.pending);
        let list = view.open(Scope::Global, &session);
        view.apply(&list, Ok(Data::Catalog(Default::default())));
        let receipt = view
            .key(
                KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE),
                &session,
            )
            .unwrap();
        assert_eq!(
            receipt.operation,
            Operation::Receipt("retained-recovery".into())
        );
        view.invalidate();
        let mut foreign = session.clone();
        foreign.id = "ses_foreign".into();
        let list = view.open(Scope::Global, &foreign);
        view.apply(&list, Ok(Data::Catalog(Default::default())));
        assert!(
            view.key(
                KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE),
                &foreign
            )
            .is_none()
        );
        assert!(
            view.key(
                KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE),
                &foreign
            )
            .is_none()
        );
    }
}
