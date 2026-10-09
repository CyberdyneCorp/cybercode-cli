//! Location-bound manual review. A request key reports evidence, never execution authority.
use crossterm::event::{KeyCode, KeyEvent};
use cyber_core::memory::{MemoryCatalog, MemoryDocument, MemoryRecoveryReview};
use cyber_server::runtime::{MemoryChange, MemoryRecoveryAdmission, MemoryRecoveryRequestStatus};
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
    Review(Option<Box<Review>>),
    Change(MemoryChange),
    Receipt(Option<Box<MemoryRecoveryRequestStatus>>),
}
#[derive(Debug, Clone)]
enum Confirmation {
    Delete(String),
    Recover(Box<Review>),
}
#[derive(Debug, Default)]
pub struct View {
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
            Ok(data) => self.apply_data(data),
            Err(error) => {
                self.confirmation = None;
                self.note = None;
                self.review = None;
                self.catalog = None;
                self.error = Some(error);
            }
        }
    }
    fn apply_data(&mut self, data: Data) {
        match data {
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
                self.note = None;
                self.review = None;
                self.catalog = None;
                self.notice = Some(format!(
                    "{} {} · receipt {}",
                    if change.receipt.deleted {
                        "Deleted"
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
        if self.confirmation.is_some() {
            return self.confirmation_key(key, session);
        }
        match key.code {
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
    pub fn cancel_confirmation(&mut self) -> bool {
        self.confirmation.take().is_some()
    }
    pub fn lines(&self) -> Vec<String> {
        let mut lines = vec![format!("{} memory · Enter read · G scope · R refresh · V recovery", self.scope.label()), "D delete reviewed note · C confirm recovery · K receipt · PgUp/PgDn scroll · Esc close".into()];
        if self.pending {
            lines.push("Loading…".into());
        }
        if let Some(error) = &self.error {
            lines.push(format!("Unavailable: {}", safe(error)));
        }
        if let Some(notice) = &self.notice {
            lines.push(notice.clone());
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
        self.content_lines(&mut lines);
        lines
    }
    fn content_lines(&self, lines: &mut Vec<String>) {
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
        Operation::Delete { .. } | Operation::Recover { .. } => {
            let change: MemoryChange = decode(data)?;
            check_identity(request, &change.directory, &change.project_id)?;
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
