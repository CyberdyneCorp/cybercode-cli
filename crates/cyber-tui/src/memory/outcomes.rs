//! Retained request inspection never dispatches a retained mutation.
use super::*;

pub(super) struct WriteRequest<'a> {
    pub method: &'static str,
    pub path: String,
    pub body: Option<Value>,
    pub key: &'a str,
}
pub(super) fn write_request(request: &Request) -> Result<WriteRequest<'_>, String> {
    let scope = request.scope.label();
    let wire = match &request.operation {
        Operation::Save(save) => {
            cyber_core::memory::validate_name(&save.name).map_err(|e| e.to_string())?;
            cyber_server::http::validate_edit_fingerprint(Some(&save.fingerprint))
                .map_err(|_| "Invalid retained edit fingerprint")?;
            WriteRequest {
                method: "PUT",
                path: format!("/memory/{scope}/{}", save.name),
                body: Some(json!({"content":save.content,"review_fingerprint":save.fingerprint})),
                key: &save.key,
            }
        }
        Operation::Delete { name, key } => {
            cyber_core::memory::validate_name(name).map_err(|e| e.to_string())?;
            WriteRequest {
                method: "DELETE",
                path: format!("/memory/{scope}/{name}"),
                body: None,
                key,
            }
        }
        _ => return Err("Expected retained save/delete request".into()),
    };
    Ok(wire)
}
fn fingerprint(request: &Request) -> Result<String, String> {
    let wire = write_request(request)?;
    let body = wire
        .body
        .as_ref()
        .map(serde_json::to_vec)
        .transpose()
        .map_err(|_| "Invalid retained request body")?
        .unwrap_or_default();
    Ok(cyber_server::http::request_fingerprint(
        wire.method,
        // The API router strips /api/v1 before its idempotency middleware.
        &wire.path,
        std::path::Path::new(&request.directory),
        &body,
        std::iter::empty(),
    ))
}
pub(super) fn parse(value: &Value, request: &Request, retained: &Request) -> Result<Data, String> {
    if retained.scope != request.scope
        || retained.directory != request.directory
        || retained.session_id != request.session_id
    {
        return Err("Retained request belongs to another Session/Location/scope".into());
    }
    let expected = fingerprint(retained)?;
    let status: Option<MemoryRequestStatus> = decode(&value["data"])?;
    if let Some(status) = &status {
        validate_status(request, retained, status, &expected)?;
    }
    Ok(Data::Outcome(status.map(Box::new)))
}
fn validate_status(
    request: &Request,
    retained: &Request,
    status: &MemoryRequestStatus,
    expected: &str,
) -> Result<(), String> {
    check_identity(request, &status.directory, &status.project_id)?;
    let (name, deleted) = match &retained.operation {
        Operation::Save(save) => (&save.name, false),
        Operation::Delete { name, .. } => (name, true),
        _ => return Err("Expected retained save/delete request".into()),
    };
    let key = persistence::request_key(retained).ok_or("Invalid retained request key")?;
    if status.id != cyber_server::runtime::memory_http_request_id(key)
        || status.request_fingerprint != expected
        || status.name != *name
        || status.deleted != deleted
    {
        return Err("Memory outcome does not match the exact retained request".into());
    }
    if let Some(journal) = &status.journal {
        cyber_server::http::validate_edit_fingerprint(Some(&journal.intent_fingerprint))
            .map_err(|_| "Invalid outcome journal fingerprint")?;
        if journal.receipt.name != *name || journal.receipt.deleted != deleted {
            return Err("Memory outcome journal does not match the request".into());
        }
    }
    if let Some(change) = &status.completed {
        check_identity(request, &change.directory, &change.project_id)?;
        if change.id != status.id
            || status.journal.as_ref().map(|j| &j.receipt) != Some(&change.receipt)
        {
            return Err("Memory outcome completion does not match its journal".into());
        }
    }
    Ok(())
}

impl View {
    pub(super) fn retained_headers(&self, lines: &mut Vec<String>) {
        if self.intents.len() > 3 {
            lines.push(format!(
                "{} retained requests; latest keys shown",
                self.intents.len()
            ));
        }
        for intent in self.intents.iter().rev().take(3) {
            if let Some(key) = persistence::request_key(intent) {
                lines.push(format!(
                    "Retained {} request: {} · {}",
                    intent.scope.label(),
                    safe(key),
                    safe(&intent.directory)
                ));
            }
        }
    }

    pub(super) fn apply_outcome(
        &mut self,
        request: &Request,
        status: Option<Box<MemoryRequestStatus>>,
    ) {
        let Operation::Outcome(retained) = &request.operation else {
            return;
        };
        let Some(status) = status else {
            self.notice = Some(
                "No durable admission found; retained request preserved, no retry sent".into(),
            );
            return;
        };
        let Some(change) = status.completed else {
            self.notice = Some(format!(
                "Unresolved {} · request retained; V inspects fresh recovery",
                safe(&status.id)
            ));
            return;
        };
        // Only exact recorded evidence is retired; newer edits are independent local work.
        self.intents.retain(|intent| intent != retained.as_ref());
        self.reconcile_draft(retained);
        self.notice = Some(format!(
            "Acknowledged {} {} · receipt {} · request retired",
            if change.receipt.deleted {
                "deletion of"
            } else {
                "save of"
            },
            safe(&change.receipt.name),
            safe(&change.id)
        ));
    }
    fn reconcile_draft(&mut self, retained: &Request) {
        let Operation::Save(save) = &retained.operation else {
            return;
        };
        let Some(draft) = &mut self.draft else {
            return;
        };
        if draft.scope != retained.scope
            || draft.directory != retained.directory
            || draft.session != retained.session_id
            || draft.name != save.name
            || draft.retained_key.as_deref() != Some(save.key.as_str())
        {
            return;
        }
        if draft.composer.text() == save.content {
            self.draft = None;
            self.editing = false;
        } else {
            draft.retained_key = None;
            draft.review = None;
        }
    }
    pub(super) fn requests_key(
        &mut self,
        key: KeyEvent,
        session: &crate::model::Session,
    ) -> Option<Request> {
        match key.code {
            KeyCode::Char('u' | 'U') => self.requests_open = false,
            KeyCode::Up => self.request_selected = self.request_selected.saturating_sub(1),
            KeyCode::Down => {
                self.request_selected = self
                    .request_selected
                    .saturating_add(1)
                    .min(self.intents.len().saturating_sub(1))
            }
            KeyCode::Char('f' | 'F') => {
                self.confirmation = self
                    .selected_request()
                    .map(|r| Confirmation::Forget(Box::new(r)));
            }
            KeyCode::Enter | KeyCode::Char('k' | 'K') => {
                let retained = self.selected_request()?;
                return self.lookup(retained, session);
            }
            _ => {}
        }
        if matches!(key.code, KeyCode::Up | KeyCode::Down) {
            self.scroll = self
                .request_selected
                .saturating_sub(5)
                .min(u16::MAX as usize) as u16;
        }
        None
    }
    fn selected_request(&self) -> Option<Request> {
        self.intents
            .get(
                self.request_selected
                    .min(self.intents.len().saturating_sub(1)),
            )
            .cloned()
    }
    fn lookup(&mut self, retained: Request, session: &crate::model::Session) -> Option<Request> {
        if retained.scope != self.scope
            || retained.directory != session.directory
            || retained.session_id != session.id
        {
            self.error = Some(
                "Return to the retained request's Session/Location/scope before lookup".into(),
            );
            return None;
        }
        let operation = match &retained.operation {
            Operation::Recover { key, .. } => Operation::Receipt(key.clone()),
            Operation::Save(_) | Operation::Delete { .. } => Operation::Outcome(Box::new(retained)),
            _ => return None,
        };
        Some(self.request(operation, session))
    }
    pub(super) fn latest_lookup(&mut self, session: &crate::model::Session) -> Option<Request> {
        let retained = self
            .intents
            .iter()
            .rev()
            .find(|r| {
                r.scope == self.scope
                    && r.directory == session.directory
                    && r.session_id == session.id
            })
            .cloned();
        if let Some(retained) = retained {
            return self.lookup(retained, session);
        }
        let (scope, key) = self.recovery_key.clone()?;
        (scope == self.scope).then(|| self.request(Operation::Receipt(key), session))
    }
    pub(super) fn request_lines(&self, lines: &mut Vec<String>) {
        lines.push(
            "Retained requests · ↑/↓ select · Enter/K lookup · F forget with confirmation · U back"
                .into(),
        );
        if self.intents.is_empty() {
            lines.push("No retained requests".into());
        }
        for (i, intent) in self.intents.iter().enumerate() {
            lines.push(format!(
                "{} {} {} · key {} · Session {} · {}",
                if i == self
                    .request_selected
                    .min(self.intents.len().saturating_sub(1))
                {
                    "›"
                } else {
                    " "
                },
                intent.scope.label(),
                label(&intent.operation),
                safe(persistence::request_key(intent).unwrap_or_default()),
                safe(&intent.session_id),
                safe(&intent.directory)
            ));
        }
    }
}
fn label(operation: &Operation) -> String {
    match operation {
        Operation::Save(save) => format!("save {}", safe(&save.name)),
        Operation::Delete { name, .. } => format!("delete {}", safe(name)),
        Operation::Recover { receipt, .. } => format!("recovery {}", safe(&receipt.name)),
        _ => "invalid".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn session() -> crate::model::Session {
        crate::model::Session {
            id: "ses_test".into(),
            directory: "/repo".into(),
            ..Default::default()
        }
    }
    fn content() -> String {
        "---\nname: policy\ndescription: Policy\ntype: reference\n---\n\nRetained Unicode 🦀\n"
            .into()
    }
    fn retained(deleted: bool) -> Request {
        Request {
            generation: 1,
            scope: Scope::Global,
            directory: session().directory,
            session_id: session().id,
            operation: if deleted {
                Operation::Delete {
                    name: "policy".into(),
                    key: "retained/key+value".into(),
                }
            } else {
                Operation::Save(Box::new(Save {
                    name: "policy".into(),
                    content: content(),
                    fingerprint: "a".repeat(64),
                    key: "retained/key+value".into(),
                }))
            },
        }
    }
    fn status(retained: &Request, completed: bool) -> Value {
        let deleted = matches!(retained.operation, Operation::Delete { .. });
        let id = cyber_server::runtime::memory_http_request_id(
            persistence::request_key(retained).unwrap(),
        );
        let journal = cyber_core::memory::MemoryJournalIdentity {
            receipt: cyber_core::memory::MemoryMutation {
                id: "mem_result".into(),
                name: "policy".into(),
                deleted,
            },
            intent_fingerprint: "c".repeat(64),
        };
        let change = completed.then(|| MemoryChange {
            id: id.clone(),
            directory: retained.directory.clone().into(),
            project_id: "global".into(),
            receipt: journal.receipt.clone(),
        });
        json!({"location":{"directory":retained.directory}, "data": MemoryRequestStatus {
            id, directory: retained.directory.clone().into(), project_id: "global".into(), name: "policy".into(), deleted,
            request_fingerprint: fingerprint(retained).unwrap(), journal: Some(journal), completed: change,
        }})
    }
    fn lookup(view: &mut View, retained: &Request) -> Request {
        view.scope = Scope::Global;
        view.request(Operation::Outcome(Box::new(retained.clone())), &session())
    }
    fn draft(view: &mut View, retained: &Request) {
        let mut draft = Draft::new(
            Scope::Global,
            &session(),
            cyber_core::memory::MemoryEditReview {
                name: "policy".into(),
                original: Some(content()),
                fingerprint: "a".repeat(64),
            },
        );
        draft.retained_key = persistence::request_key(retained).map(str::to_owned);
        view.draft = Some(draft);
    }
    #[test]
    fn completed_save_and_delete_lookup_retire_only_exact_intent_and_matching_draft() {
        for deleted in [false, true] {
            let mut view = View::default();
            let retained = retained(deleted);
            let mut other = retained.clone();
            other.generation = 2;
            view.intents = vec![retained.clone(), other.clone()];
            if !deleted {
                draft(&mut view, &retained);
            }
            let request = lookup(&mut view, &retained);
            let result = super::super::parse(&status(&retained, true), &request);
            assert!(result.is_ok());
            view.apply(&request, result);
            assert_eq!(view.intents, vec![other]);
            assert!(view.draft.is_none());
            assert!(view.notice.as_ref().unwrap().contains("request retired"));
        }
    }
    #[test]
    fn completed_save_keeps_newer_text_and_removes_consumed_review() {
        let retained = retained(false);
        let mut view = View::default();
        view.intents.push(retained.clone());
        draft(&mut view, &retained);
        view.draft
            .as_mut()
            .unwrap()
            .composer
            .insert_str("New user edits");
        let text = view.draft.as_ref().unwrap().composer.text();
        let request = lookup(&mut view, &retained);
        view.apply(
            &request,
            super::super::parse(&status(&retained, true), &request),
        );
        let draft = view.draft.unwrap();
        assert_eq!(draft.composer.text(), text);
        assert!(draft.retained_key.is_none());
        assert!(draft.review.is_none());
        assert!(view.intents.is_empty());
    }
    #[test]
    fn absent_unresolved_and_late_status_preserve_records_and_drafts() {
        let retained = retained(false);
        for value in [
            json!({"location":{"directory":"/repo"},"data":null}),
            status(&retained, false),
        ] {
            let mut view = View::default();
            view.intents.push(retained.clone());
            draft(&mut view, &retained);
            let request = lookup(&mut view, &retained);
            view.apply(&request, super::super::parse(&value, &request));
            assert_eq!(view.intents, vec![retained.clone()]);
            assert!(view.draft.is_some());
        }
        let mut view = View::default();
        view.intents.push(retained.clone());
        let request = lookup(&mut view, &retained);
        view.invalidate();
        view.apply(
            &request,
            super::super::parse(&status(&retained, true), &request),
        );
        assert_eq!(view.intents, vec![retained]);
        assert!(view.notice.is_none());
    }
    #[test]
    fn outcomes_reject_mismatched_wire_and_receipt_evidence() {
        let retained = retained(false);
        let mut view = View::default();
        let request = lookup(&mut view, &retained);
        for pointer in [
            "/data/id",
            "/data/name",
            "/data/request_fingerprint",
            "/data/project_id",
            "/data/directory",
            "/data/journal/receipt/name",
            "/data/journal/intent_fingerprint",
            "/data/completed/id",
            "/data/completed/receipt/id",
            "/data/completed/project_id",
        ] {
            let mut value = status(&retained, true);
            *value.pointer_mut(pointer).unwrap() = json!("foreign");
            assert!(super::super::parse(&value, &request).is_err(), "{pointer}");
        }
        let mut value = status(&retained, true);
        value["data"]["deleted"] = json!(true);
        assert!(super::super::parse(&value, &request).is_err());
        let mut value = status(&retained, true);
        value["data"]["journal"] = Value::Null;
        assert!(super::super::parse(&value, &request).is_err());
    }

    #[test]
    fn outcome_lookup_cannot_borrow_foreign_session_location_or_scope() {
        let retained = retained(false);
        let mut view = View::default();
        let request = lookup(&mut view, &retained);
        let mut other_session = request.clone();
        other_session.session_id = "ses_foreign".into();
        let mut other_location = request.clone();
        other_location.directory = "/other".into();
        let mut other_scope = request;
        other_scope.scope = Scope::Project;
        for foreign in [other_session, other_location, other_scope] {
            assert!(super::super::parse(&status(&retained, true), &foreign).is_err());
        }
    }
    #[test]
    fn retained_request_selection_is_scoped_and_forgetting_requires_exact_confirmation() {
        let mut view = View {
            scope: Scope::Global,
            intents: vec![retained(false), retained(true)],
            ..Default::default()
        };
        let key = |c| KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE);
        assert!(view.key(key('u'), &session()).is_none());
        let request = view.key(key('k'), &session()).unwrap();
        assert!(matches!(request.operation, Operation::Outcome(_)));
        view.apply(&request, Err("lookup failed".into()));
        let mut foreign = session();
        foreign.id = "ses_foreign".into();
        assert!(view.key(key('k'), &foreign).is_none());
        assert!(view.error.as_ref().unwrap().contains("Return to"));
        view.key(key('f'), &session());
        view.key(key('n'), &session());
        assert_eq!(view.intents.len(), 2);
        view.key(key('f'), &session());
        assert!(
            view.header_lines()
                .join("\n")
                .contains("Unknown effects remain unresolved")
        );
        assert!(view.key(key('y'), &session()).is_none());
        assert_eq!(view.intents, vec![retained(false)]);
    }
}
