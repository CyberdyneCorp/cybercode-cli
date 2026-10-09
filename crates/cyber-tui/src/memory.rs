//! Location-bound manual review. A request key reports evidence, never execution authority.
mod editor;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use cyber_core::memory::{MemoryCatalog, MemoryDocument, MemoryRecoveryReview};
use cyber_server::runtime::{MemoryChange, MemoryRecoveryAdmission, MemoryRecoveryRequestStatus};
use editor::{Draft, Save};
use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    #[default]
    Project,
    Global,
}
impl Scope {
    pub fn label(self) -> &'static str {
        match self {
            Self::Project => "project",
            Self::Global => "global",
        }
    }
}
#[derive(Debug, Clone, PartialEq)]
pub enum Operation {
    List,
    Read(String),
    Review,
    EditReview(String),
    Save(Box<Save>),
    Delete {
        name: String,
        key: String,
    },
    Recover {
        mutation_id: String,
        receipt: Box<cyber_core::memory::MemoryMutation>,
        storage: String,
        admission: String,
        key: String,
    },
    Receipt(String),
}
#[derive(Debug, Clone, PartialEq)]
pub struct Request {
    pub generation: u64,
    pub scope: Scope,
    pub directory: String,
    pub session_id: String,
    pub operation: Operation,
}
#[derive(Debug, Clone, Deserialize)]
pub struct Review {
    pub storage: MemoryRecoveryReview,
    pub admission: MemoryRecoveryAdmission,
}
#[derive(Debug, Clone)]
pub enum Data {
    Catalog(MemoryCatalog),
    Note(MemoryDocument),
    EditReview(cyber_core::memory::MemoryEditReview),
    Review(Option<Box<Review>>),
    Change(MemoryChange),
    Receipt(Option<Box<MemoryRecoveryRequestStatus>>),
}
#[derive(Debug, Clone)]
enum Confirmation {
    Save(Box<Save>),
    DiscardDraft,
    Delete(String),
    Recover(Box<Review>),
}
#[derive(Debug, Default)]
pub struct View {
    draft: Option<Draft>,
    editing: bool,
    scope: Scope,
    generation: u64,
    pending: bool,
    selected: usize,
    catalog: Option<MemoryCatalog>,
    note: Option<MemoryDocument>,
    review: Option<Box<Review>>,
    confirmation: Option<Confirmation>,
    error: Option<String>,
    notice: Option<String>,
    recovery_key: Option<(Scope, String)>,
    pub scroll: u16,
}
impl View {
    pub fn invalidate(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.pending = false;
        self.editing = false;
        self.catalog = None;
        self.note = None;
        self.review = None;
        self.confirmation = None;
        self.error = None;
        self.notice = None;
        self.scroll = 0;
    }
    pub fn forget_location(&mut self) {
        self.invalidate();
        self.recovery_key = None;
    }
    pub fn open(&mut self, scope: Scope, session: &crate::model::Session) -> Request {
        self.invalidate();
        self.scope = scope;
        self.request(Operation::List, session)
    }
    fn request(&mut self, operation: Operation, session: &crate::model::Session) -> Request {
        self.generation = self.generation.wrapping_add(1);
        self.pending = true;
        self.confirmation = None;
        self.error = None;
        self.notice = None;
        self.scroll = 0;
        Request {
            generation: self.generation,
            scope: self.scope,
            directory: session.directory.clone(),
            session_id: session.id.clone(),
            operation,
        }
    }
    pub fn apply(&mut self, request: &Request, result: Result<Data, String>) {
        if request.generation != self.generation || request.scope != self.scope || !self.pending {
            return;
        }
        self.pending = false;
        match result {
            Ok(data) => self.apply_data(data, request),
            Err(error) => {
                self.confirmation = None;
                self.note = None;
                self.review = None;
                self.catalog = None;
                if matches!(
                    request.operation,
                    Operation::Save(_) | Operation::EditReview(_)
                ) && let Some(draft) = &mut self.draft
                {
                    draft.review = None;
                }
                self.error = Some(error);
            }
        }
    }
    fn apply_data(&mut self, data: Data, request: &Request) {
        match data {
            Data::EditReview(review) => {
                if let Some(draft) = &mut self.draft {
                    if draft.scope == request.scope
                        && draft.directory == request.directory
                        && draft.name == review.name
                    {
                        draft.review = Some(review);
                    } else {
                        self.error = Some(
                            "A draft for another note is retained; discard it explicitly first"
                                .into(),
                        );
                        return;
                    }
                } else {
                    let session = crate::model::Session {
                        id: request.session_id.clone(),
                        directory: request.directory.clone(),
                        ..Default::default()
                    };
                    self.draft = Some(Draft::new(request.scope, &session, review));
                }
                self.editing = true;
                self.follow_cursor();
            }
            Data::Catalog(catalog) => {
                self.selected = self.selected.min(catalog.memories.len().saturating_sub(1));
                self.catalog = Some(catalog);
                self.note = None;
                self.review = None;
            }
            Data::Note(note) => {
                self.note = Some(note);
                self.review = None;
            }
            Data::Review(review) => {
                self.review = review;
                self.note = None;
                if self.review.is_none() {
                    self.notice = Some("No pinned recovery pending".into());
                }
            }
            Data::Change(change) => {
                let saved = matches!(request.operation, Operation::Save(_));
                if saved {
                    self.draft = None;
                    self.editing = false;
                }
                self.note = None;
                self.review = None;
                self.catalog = None;
                self.notice = Some(format!(
                    "{} {} · receipt {}",
                    if change.receipt.deleted {
                        "Deleted"
                    } else if saved {
                        "Saved"
                    } else {
                        "Recovered"
                    },
                    safe(&change.receipt.name),
                    safe(&change.id)
                ));
            }
            Data::Receipt(status) => {
                self.notice = Some(match status {
                    None => "No recovery request recorded for this scope".into(),
                    Some(status) if status.completed.is_none() => format!(
                        "Unresolved {} · inspect a fresh review before recovery",
                        safe(&status.mutation_id)
                    ),
                    Some(status) => format!("Acknowledged receipt {}", safe(&status.mutation_id)),
                })
            }
        }
    }
    pub fn key(&mut self, key: KeyEvent, session: &crate::model::Session) -> Option<Request> {
        if self.pending {
            return None;
        }
        if matches!(key.code, KeyCode::PageUp | KeyCode::PageDown) {
            self.scroll = if key.code == KeyCode::PageUp {
                self.scroll.saturating_sub(5)
            } else {
                self.scroll
                    .saturating_add(5)
                    .min(self.lines().len().saturating_sub(1).min(u16::MAX as usize) as u16)
            };
            return None;
        }
        if self.confirmation.is_some() {
            return self.confirmation_key(key, session);
        }
        if self.editing {
            return self.editor_key(key, session);
        }
        match key.code {
            KeyCode::Char('e' | 'E') => {
                if let Some(draft) = &self.draft {
                    if draft.belongs(self.scope, session) {
                        self.editing = true;
                    } else {
                        self.error = Some("Draft retained for another Session/Location/scope; return there or X to discard".into());
                    }
                    return None;
                }
                let name = self.note.as_ref()?.metadata.name.clone();
                Some(self.request(Operation::EditReview(name), session))
            }
            KeyCode::Char('x' | 'X') if self.draft.is_some() => {
                self.confirmation = Some(Confirmation::DiscardDraft);
                None
            }
            KeyCode::Char('r' | 'R') => Some(self.request(Operation::List, session)),
            KeyCode::Char('g' | 'G') => {
                let scope = if self.scope == Scope::Project {
                    Scope::Global
                } else {
                    Scope::Project
                };
                Some(self.open(scope, session))
            }
            KeyCode::Char('v' | 'V') => Some(self.request(Operation::Review, session)),
            KeyCode::Char('k' | 'K') => {
                let (scope, key) = self.recovery_key.clone()?;
                (scope == self.scope).then(|| self.request(Operation::Receipt(key), session))
            }
            KeyCode::Enter => {
                let name = self
                    .catalog
                    .as_ref()?
                    .memories
                    .get(self.selected)?
                    .name
                    .clone();
                Some(self.request(Operation::Read(name), session))
            }
            KeyCode::Char('d' | 'D') => {
                self.confirmation = self
                    .note
                    .as_ref()
                    .map(|note| Confirmation::Delete(note.metadata.name.clone()));
                None
            }
            KeyCode::Char('c' | 'C') => {
                self.confirmation = self.review.clone().map(Confirmation::Recover);
                None
            }
            KeyCode::Up => {
                self.select(false);
                None
            }
            KeyCode::Down => {
                self.select(true);
                None
            }
            KeyCode::PageUp => {
                self.scroll = self.scroll.saturating_sub(5);
                None
            }
            KeyCode::PageDown => {
                self.scroll = self
                    .scroll
                    .saturating_add(5)
                    .min(self.lines().len().saturating_sub(1).min(u16::MAX as usize) as u16);
                None
            }
            _ => None,
        }
    }
    fn select(&mut self, next: bool) {
        let len = self.catalog.as_ref().map_or(0, |c| c.memories.len());
        self.selected = if next {
            self.selected.saturating_add(1).min(len.saturating_sub(1))
        } else {
            self.selected.saturating_sub(1)
        };
        self.note = None;
        self.review = None;
        self.scroll = 0;
    }
    fn confirmation_key(
        &mut self,
        key: KeyEvent,
        session: &crate::model::Session,
    ) -> Option<Request> {
        match key.code {
            KeyCode::Char('y' | 'Y') => {
                let key = cyber_core::ids::new_id("mrq");
                let operation = match self.confirmation.take()? {
                    Confirmation::DiscardDraft => {
                        self.draft = None;
                        self.editing = false;
                        return None;
                    }
                    Confirmation::Save(save) => {
                        let draft = self.draft.as_mut()?;
                        if !draft.belongs(self.scope, session) {
                            return None;
                        }
                        draft.retained_key = Some(save.key.clone());
                        Operation::Save(save)
                    }
                    Confirmation::Delete(name) => Operation::Delete { name, key },
                    Confirmation::Recover(review) => {
                        self.recovery_key = Some((self.scope, key.clone()));
                        Operation::Recover {
                            mutation_id: review.admission.id,
                            receipt: Box::new(review.storage.receipt),
                            storage: review.storage.fingerprint,
                            admission: review.admission.fingerprint,
                            key,
                        }
                    }
                };
                Some(self.request(operation, session))
            }
            KeyCode::Esc | KeyCode::Char('n' | 'N') => {
                self.confirmation = None;
                None
            }
            _ => None,
        }
    }
    fn editor_key(&mut self, key: KeyEvent, session: &crate::model::Session) -> Option<Request> {
        let draft = self.draft.as_mut()?;
        if !draft.belongs(self.scope, session) {
            self.editing = false;
            return None;
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            match key.code {
                KeyCode::Char('s') => match draft.snapshot() {
                    Ok(save) => self.confirmation = Some(Confirmation::Save(Box::new(save))),
                    Err(error) => self.error = Some(error),
                },
                KeyCode::Char('r') => {
                    let name = draft.name.clone();
                    return Some(self.request(Operation::EditReview(name), session));
                }
                _ => {}
            }
        } else if let Err(error) = draft.key(key) {
            self.error = Some(error);
        }
        self.follow_cursor();
        None
    }
    pub fn paste(&mut self, text: &str) {
        if !self.editing || self.pending || self.confirmation.is_some() {
            return;
        }
        if let Some(draft) = &mut self.draft
            && let Err(error) = draft.paste(text)
        {
            self.error = Some(error);
        }
        self.follow_cursor();
    }
    fn follow_cursor(&mut self) {
        let Some(draft) = &self.draft else {
            return;
        };
        let original = draft.review.as_ref().map_or(0, |review| {
            review
                .original
                .as_deref()
                .unwrap_or_default()
                .lines()
                .count()
                + 1
        });
        let row = original + 2 + draft.composer.cursor().0;
        self.scroll = row.saturating_sub(10).min(u16::MAX as usize) as u16;
    }
    pub fn horizontal_scroll(&self, width: u16) -> u16 {
        use unicode_width::UnicodeWidthStr;
        if !self.editing {
            return 0;
        }
        let Some(draft) = &self.draft else {
            return 0;
        };
        let (row, col) = draft.composer.cursor();
        let prefix: String = draft.composer.lines()[row].chars().take(col).collect();
        safe(&prefix)
            .width()
            .saturating_sub(width.saturating_sub(3) as usize)
            .min(u16::MAX as usize) as u16
    }
    pub fn leave_editor(&mut self) -> bool {
        if !self.editing || self.confirmation.is_some() {
            return false;
        }
        self.editing = false;
        true
    }
    pub fn cancel_confirmation(&mut self) -> bool {
        self.confirmation.take().is_some()
    }
    pub fn lines(&self) -> Vec<String> {
        let mut lines = self.header_lines();
        self.content_lines(&mut lines);
        lines
    }
    pub fn header_lines(&self) -> Vec<String> {
        let mut lines = vec![format!("{} memory · Enter read · G scope · R refresh · V recovery", self.scope.label()), "E edit/resume draft · X discard draft · D delete reviewed note · C confirm recovery · K receipt · PgUp/PgDn scroll · Esc close".into()];
        if self.pending {
            lines.push("Loading…".into());
        }
        if let Some(error) = &self.error {
            lines.push(format!("Unavailable: {}", safe(error)));
        }
        if let Some(notice) = &self.notice {
            lines.push(notice.clone());
        }
        if let Some(draft) = &self.draft {
            lines.push(format!(
                "Retained {} draft: {} · {}",
                draft.scope.label(),
                safe(&draft.name),
                safe(&draft.directory)
            ));
            if let Some(key) = &draft.retained_key {
                lines.push(format!(
                    "Save request key: {} · verify outcome before another save",
                    safe(key)
                ));
            }
        }
        if let Some((scope, key)) = &self.recovery_key {
            lines.push(format!(
                "Retained {} recovery key: {}",
                scope.label(),
                safe(key)
            ));
        }
        if let Some(confirmation) = &self.confirmation {
            lines.push(match confirmation {
                Confirmation::Save(save) => format!(
                    "Save {} to {} memory? Y confirms · N returns to draft",
                    safe(&save.name),
                    self.scope.label()
                ),
                Confirmation::DiscardDraft => {
                    "Discard the retained unsaved draft? Y confirms · N cancels".into()
                }
                Confirmation::Delete(name) => format!(
                    "Delete {} from {} memory? Y confirms · N cancels",
                    safe(name),
                    self.scope.label()
                ),
                Confirmation::Recover(_) => {
                    "Apply this reviewed recovery? Y confirms · N cancels".into()
                }
            });
        }
        lines
    }
    fn content_lines(&self, lines: &mut Vec<String>) {
        if self.editing
            && let Some(draft) = &self.draft
        {
            lines.extend(draft.lines());
            return;
        }
        if let Some(note) = &self.note {
            lines.push(format!(
                "{} · {}",
                safe(&note.metadata.name),
                safe(&note.metadata.description)
            ));
            lines.extend(note.body.lines().map(safe));
            return;
        }
        if let Some(review) = &self.review {
            lines.push(format!(
                "{} {} · files committed={} · database acknowledged={}",
                if review.storage.receipt.deleted {
                    "Delete"
                } else {
                    "Save"
                },
                safe(&review.storage.receipt.name),
                review.storage.completed,
                review.admission.completed.is_some()
            ));
            lines.push(format!(
                "Storage review: {}",
                safe(&review.storage.fingerprint)
            ));
            lines.push(format!(
                "Database review: {}",
                safe(&review.admission.fingerprint)
            ));
            if let Some(note) = &review.storage.proposed_note {
                lines.push("Proposed content:".into());
                lines.extend(
                    note.render_for_write()
                        .unwrap_or_default()
                        .lines()
                        .map(safe),
                );
            }
            return;
        }
        if let Some(catalog) = &self.catalog {
            if catalog.memories.is_empty() {
                lines.push("No memories in this scope".into());
            }
            lines.extend(catalog.memories.iter().enumerate().map(|(i, note)| {
                format!(
                    "{} {} · {}",
                    if i == self.selected { "›" } else { " " },
                    safe(&note.name),
                    safe(&note.description)
                )
            }));
            lines.extend(catalog.invalid.iter().map(|invalid| {
                format!(
                    "Invalid {}: {}",
                    safe(&invalid.filename),
                    safe(&invalid.diagnostic)
                )
            }));
        }
    }
}
fn safe(text: &str) -> String {
    text.escape_debug().to_string()
}

pub async fn perform(client: &cyber_client::Client, request: Request) -> crate::perform::Msg {
    let result = execute(client, &request).await;
    crate::perform::Msg::Memory { request, result }
}
async fn execute(client: &cyber_client::Client, request: &Request) -> Result<Data, String> {
    let scope = request.scope.label();
    let recovery = format!("/memory/recovery/{scope}");
    let (method, path, body, key) = match &request.operation {
        Operation::List => ("GET", format!("/memory?scope={scope}"), None, None),
        Operation::Read(name) => {
            cyber_core::memory::validate_name(name).map_err(|e| e.to_string())?;
            ("GET", format!("/memory/{scope}/{name}"), None, None)
        }
        Operation::Review => ("GET", recovery, None, None),
        Operation::EditReview(name) => {
            cyber_core::memory::validate_name(name).map_err(|e| e.to_string())?;
            ("GET", format!("/memory/edit/{scope}/{name}"), None, None)
        }
        Operation::Save(save) => {
            cyber_core::memory::validate_name(&save.name).map_err(|e| e.to_string())?;
            cyber_server::http::validate_edit_fingerprint(Some(&save.fingerprint))
                .map_err(|e| e.body.message)?;
            (
                "PUT",
                format!("/memory/{scope}/{}", save.name),
                Some(json!({"content":save.content,"review_fingerprint":save.fingerprint})),
                Some(&save.key),
            )
        }
        Operation::Delete { name, key } => {
            cyber_core::memory::validate_name(name).map_err(|e| e.to_string())?;
            ("DELETE", format!("/memory/{scope}/{name}"), None, Some(key))
        }
        Operation::Recover {
            storage,
            admission,
            key,
            ..
        } => {
            cyber_server::http::RecoverMemory {
                storage_fingerprint: storage.clone(),
                admission_fingerprint: admission.clone(),
            }
            .validate()
            .map_err(|_| "Invalid recovery fingerprints")?;
            (
                "POST",
                recovery,
                Some(json!({"storage_fingerprint":storage,"admission_fingerprint":admission})),
                Some(key),
            )
        }
        Operation::Receipt(key) => (
            "GET",
            format!(
                "/memory/recovery/requests?scope={scope}&key={}",
                encode(key)
            ),
            None,
            None,
        ),
    };
    let headers = key
        .map(|key| vec![("idempotency-key".into(), key.clone())])
        .unwrap_or_default();
    let client = client.at(&request.directory);
    let (status, value) = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        client.raw(method, &path, body, &headers),
    )
    .await
    .map_err(|_| "Response timed out; inspect recovery and retained receipt before retrying")?
    .map_err(|_| "Transport failed; inspect recovery and retained receipt before retrying")?;
    if !(200..300).contains(&status) {
        return Err(format!(
            "Memory request refused ({status}): {}",
            value["message"].as_str().unwrap_or("request failed")
        ));
    }
    parse(&value, request)
}
fn decode<T: serde::de::DeserializeOwned>(value: &Value) -> Result<T, String> {
    serde_json::from_value(value.clone()).map_err(|_| "Malformed memory response".into())
}
fn parse(value: &Value, request: &Request) -> Result<Data, String> {
    if value["location"]["directory"].as_str() != Some(&request.directory)
        || value.get("data").is_none()
    {
        return Err("Memory response belongs to a different Location".into());
    }
    let data = &value["data"];
    match &request.operation {
        Operation::List => {
            let catalog: MemoryCatalog = decode(data)?;
            if catalog.memories.len() > 4096
                || catalog.invalid.len() > 4096
                || catalog.memories.iter().any(|m| m.validate().is_err())
            {
                return Err("Invalid memory catalog".into());
            }
            Ok(Data::Catalog(catalog))
        }
        Operation::EditReview(name) => {
            let review: cyber_core::memory::MemoryEditReview = decode(data)?;
            if review.name != *name
                || review
                    .original
                    .as_ref()
                    .is_some_and(|text| text.len() > 1024 * 1024)
            {
                return Err("Invalid memory edit review identity or size".into());
            }
            cyber_server::http::validate_edit_fingerprint(Some(&review.fingerprint))
                .map_err(|_| "Invalid memory edit fingerprint")?;
            Ok(Data::EditReview(review))
        }
        Operation::Read(name) => {
            let note: MemoryDocument = decode(data)?;
            if note.metadata.name != *name
                || note.metadata.validate().is_err()
                || note.body.len() > 1024 * 1024
            {
                return Err("Invalid memory document identity".into());
            }
            Ok(Data::Note(note))
        }
        Operation::Review => {
            let review: Option<Review> = decode(data)?;
            if let Some(review) = &review {
                check_identity(
                    request,
                    &review.admission.directory,
                    &review.admission.project_id,
                )?;
                cyber_server::http::RecoverMemory {
                    storage_fingerprint: review.storage.fingerprint.clone(),
                    admission_fingerprint: review.admission.fingerprint.clone(),
                }
                .validate()
                .map_err(|_| "Invalid memory review fingerprints")?;
                if review.storage.journal != review.admission.journal
                    || review.storage.receipt != review.admission.journal.receipt
                {
                    return Err("Memory reviews disagree about the journal".into());
                }
            }
            Ok(Data::Review(review.map(Box::new)))
        }
        Operation::Delete { .. } | Operation::Recover { .. } | Operation::Save(_) => {
            let change: MemoryChange = decode(data)?;
            check_identity(request, &change.directory, &change.project_id)?;
            if let Operation::Save(save) = &request.operation
                && (change.receipt.deleted || change.receipt.name != save.name)
            {
                return Err("Memory save receipt does not match the draft".into());
            }
            if let Operation::Delete { name, .. } = &request.operation
                && (!change.receipt.deleted || change.receipt.name != *name)
            {
                return Err("Memory deletion receipt does not match".into());
            }
            if let Operation::Recover {
                mutation_id,
                receipt,
                ..
            } = &request.operation
                && (change.id != *mutation_id || change.receipt != **receipt)
            {
                return Err(
                    "Memory recovery receipt does not match the reviewed transaction".into(),
                );
            }
            Ok(Data::Change(change))
        }
        Operation::Receipt(_) => {
            let status: Option<MemoryRecoveryRequestStatus> = decode(data)?;
            if let Some(status) = &status {
                check_identity(request, &status.directory, &status.project_id)?;
                if let Some(change) = &status.completed {
                    check_identity(request, &change.directory, &change.project_id)?;
                    if change.id != status.mutation_id || change.receipt != status.journal.receipt {
                        return Err(
                            "Memory recovery status receipt does not match its journal".into()
                        );
                    }
                }
            }
            Ok(Data::Receipt(status.map(Box::new)))
        }
    }
}
fn check_identity(
    request: &Request,
    directory: &std::path::Path,
    project: &str,
) -> Result<(), String> {
    let expected = if request.scope == Scope::Global {
        "global".into()
    } else {
        cyber_core::project::identify(std::path::Path::new(&request.directory)).id
    };
    if directory != std::path::Path::new(&request.directory) || project != expected {
        return Err("Foreign memory scope identity".into());
    }
    Ok(())
}
fn encode(key: &str) -> String {
    key.bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
                char::from(b).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    fn request(operation: Operation) -> Request {
        Request {
            generation: 1,
            scope: Scope::Global,
            directory: "/repo".into(),
            session_id: "session".into(),
            operation,
        }
    }
    #[test]
    fn malformed_foreign_and_mismatched_note_responses_refuse() {
        let request = request(Operation::Read("policy".into()));
        let mut value = json!({"location":{"directory":"/repo"},"data":{"metadata":{"name":"policy","description":"Policy","type":"reference"},"body":"Fact"}});
        assert!(matches!(parse(&value, &request), Ok(Data::Note(_))));
        value["data"]["metadata"]["name"] = json!("other");
        assert!(parse(&value, &request).is_err());
        value["location"]["directory"] = json!("/foreign");
        assert!(parse(&value, &request).is_err());
        assert!(
            parse(
                &json!({"location":{"directory":"/repo"},"data":null}),
                &request
            )
            .is_err()
        );
        assert!(parse(&json!({"location":{"directory":"/repo"}}), &request).is_err());
    }
    #[test]
    fn invalid_catalog_metadata_refuses_and_unknown_receipt_does_not_mutate() {
        let invalid = json!({"location":{"directory":"/repo"},"data":{"memories":[{"name":"../escape","description":"Policy","type":"reference"}],"invalid":[]}});
        assert!(parse(&invalid, &request(Operation::List)).is_err());
        assert!(matches!(
            parse(
                &json!({"location":{"directory":"/repo"},"data":null}),
                &request(Operation::Receipt("retained/key+value".into()))
            ),
            Ok(Data::Receipt(None))
        ));
        assert_eq!(encode("retained/key+value"), "retained%2Fkey%2Bvalue");
    }
}

#[cfg(test)]
mod receipt_tests {
    use super::*;
    #[test]
    fn completed_receipt_must_match_the_reviewed_mutation_and_journal() {
        let receipt = cyber_core::memory::MemoryMutation {
            id: "mem_original".into(),
            name: "policy".into(),
            deleted: false,
        };
        let request = Request {
            generation: 1,
            scope: Scope::Global,
            directory: "/repo".into(),
            session_id: "session".into(),
            operation: Operation::Recover {
                mutation_id: "mwr_original".into(),
                receipt: Box::new(receipt.clone()),
                storage: "a".repeat(64),
                admission: format!("sha256:{}", "b".repeat(64)),
                key: "retained".into(),
            },
        };
        let change = MemoryChange {
            id: "mwr_original".into(),
            directory: "/repo".into(),
            project_id: "global".into(),
            receipt: receipt.clone(),
        };
        let mut value = json!({"location":{"directory":"/repo"},"data":change});
        assert!(matches!(parse(&value, &request), Ok(Data::Change(_))));
        value["data"]["id"] = json!("mwr_other");
        assert!(parse(&value, &request).is_err());
        value["data"]["id"] = json!("mwr_original");
        value["data"]["receipt"]["name"] = json!("other");
        assert!(parse(&value, &request).is_err());
    }
}

#[cfg(test)]
mod editor_response_tests {
    use super::*;
    fn request(operation: Operation) -> Request {
        Request {
            generation: 1,
            scope: Scope::Global,
            directory: "/repo".into(),
            session_id: "session".into(),
            operation,
        }
    }
    #[test]
    fn edit_reviews_refuse_foreign_names_locations_invalid_fingerprints_and_oversized_originals() {
        let request = request(Operation::EditReview("policy".into()));
        let mut value = json!({"location":{"directory":"/repo"},"data":{"name":"policy","original":null,"fingerprint":"a".repeat(64)}});
        assert!(matches!(parse(&value, &request), Ok(Data::EditReview(_))));
        value["data"]["name"] = json!("foreign");
        assert!(parse(&value, &request).is_err());
        value["data"]["name"] = json!("policy");
        value["data"]["fingerprint"] = json!("invalid");
        assert!(parse(&value, &request).is_err());
        value["data"]["fingerprint"] = json!("a".repeat(64));
        value["data"]["original"] = json!("x".repeat(1024 * 1024 + 1));
        assert!(parse(&value, &request).is_err());
        value["data"]["original"] = json!("Fact");
        value["location"]["directory"] = json!("/foreign");
        assert!(parse(&value, &request).is_err());
    }
    #[test]
    fn save_completion_must_match_the_pinned_note_scope_and_operation() {
        let request = request(Operation::Save(Box::new(Save {
            name: "policy".into(),
            content: "Fact".into(),
            fingerprint: "a".repeat(64),
            key: "retained".into(),
        })));
        let mut value = json!({"location":{"directory":"/repo"},"data":{"id":"mwr_saved","directory":"/repo","project_id":"global","receipt":{"id":"mem_saved","name":"policy","deleted":false}}});
        assert!(matches!(parse(&value, &request), Ok(Data::Change(_))));
        value["data"]["receipt"]["deleted"] = json!(true);
        assert!(parse(&value, &request).is_err());
        value["data"]["receipt"]["deleted"] = json!(false);
        value["data"]["receipt"]["name"] = json!("foreign");
        assert!(parse(&value, &request).is_err());
        value["data"]["receipt"]["name"] = json!("policy");
        value["data"]["project_id"] = json!("foreign");
        assert!(parse(&value, &request).is_err());
    }
}
