//! Durable request identities survive response loss and TUI reconnects.
use crate::{app::Action, model::Choice, perform::Msg, store::LocalStore};
use cyber_client::Client;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use tokio::sync::mpsc;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Request {
    pub id: String,
    pub source: String,
    pub directory: String,
}
impl Request {
    fn path(&self) -> String {
        format!("/sessions/{}/delegations/{}", self.source, self.id)
    }
    pub fn valid(&self) -> bool {
        safe_id(&self.id, "op") && safe_id(&self.source, "ses")
    }
    pub fn action(&self, stop: bool) -> Action {
        Action::Admission {
            request: self.clone(),
            stop,
        }
    }
}
fn safe_id(id: &str, prefix: &str) -> bool {
    cyber_core::ids::has_prefix(id, prefix)
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
}
#[derive(Debug, Clone)]
pub struct Entry {
    pub request: Request,
    pub status: String,
    pub error: Option<String>,
    pub job: Option<String>,
    pub busy: bool,
    pub stopping: bool,
    pub stop_at: Option<std::time::Instant>,
}
impl Entry {
    fn cancellation_expired(&self) -> bool {
        self.stop_at
            .is_some_and(|at| at.elapsed() >= std::time::Duration::from_secs(3))
    }
    fn expire_cancellation(&mut self) {
        self.status = "unknown".into();
        self.error = Some("Cancellation acknowledgement timed out".into());
    }
}
#[derive(Default)]
pub struct Admissions(pub BTreeMap<String, Entry>);
impl Admissions {
    pub fn prepare(&mut self, action: &Action, store: &LocalStore) -> Result<(), String> {
        let (request, stop, persist, label) = match action {
            Action::ReconcileAdmission(request) => (request, false, true, "reconciliation"),
            Action::Admission { request, stop } => (request, *stop, *stop, "cancellation"),
            _ => return Ok(()),
        };
        if persist {
            store
                .save_admission(request)
                .map_err(|error| format!("Cannot retain {label} identity: {error}"))?;
        }
        if let Some(entry) = self.0.get_mut(&request.id) {
            entry.busy = true;
            if stop {
                if entry.status == "unknown" {
                    entry.stop_at = Some(std::time::Instant::now());
                } else {
                    entry.stop_at.get_or_insert_with(std::time::Instant::now);
                }
            }
            entry.stopping |= stop;
        }
        Ok(())
    }

    pub fn register(&mut self, request: Request) {
        self.0.entry(request.id.clone()).or_insert(Entry {
            request,
            status: "pending".into(),
            error: None,
            job: None,
            busy: true,
            stopping: false,
            stop_at: None,
        });
    }
    pub fn pending(&self, source: &str) -> Option<&Entry> {
        self.0.values().rev().find(|entry| {
            entry.request.source == source
                && matches!(entry.status.as_str(), "pending" | "cancelling" | "unknown")
        })
    }
    pub fn poll(&mut self) -> Vec<Action> {
        self.0
            .values_mut()
            .filter_map(|entry| {
                if entry.cancellation_expired()
                    && matches!(entry.status.as_str(), "pending" | "cancelling")
                {
                    entry.expire_cancellation();
                    return None;
                }
                if entry.busy || !matches!(entry.status.as_str(), "pending" | "cancelling") {
                    return None;
                }
                entry.busy = true;
                Some(entry.request.action(entry.stopping))
            })
            .collect()
    }
    pub fn update(
        &mut self,
        request: Request,
        result: Result<Value, String>,
        stop: bool,
        store: &LocalStore,
    ) -> String {
        self.register(request.clone());
        let entry = self.0.get_mut(&request.id).expect("registered");
        entry.busy = false;
        if matches!(entry.status.as_str(), "cancelled" | "failed") {
            store.forget_admission(&request.id);
            return format!("{} · {}", entry.status, request.id);
        }
        match result {
            Ok(record) => {
                let status = record["status"].as_str().expect("validated record");
                if entry.cancellation_expired() && matches!(status, "pending" | "cancelling") {
                    entry.expire_cancellation();
                    return format!(
                        "unknown · {}: Cancellation acknowledgement timed out",
                        request.id
                    );
                }
                // A delayed submission response cannot undo terminal cancellation.
                if matches!(entry.status.as_str(), "admitted" | "cancelled" | "failed")
                    && matches!(status, "pending" | "cancelling")
                {
                    return format!("{} · {}", entry.status, request.id);
                }
                entry.status =
                    if entry.stopping && (status == "pending" || !stop && status == "admitted") {
                        "cancelling"
                    } else {
                        status
                    }
                    .into();
                if let Some(job) = record["job_id"].as_str() {
                    if entry.job.as_deref().is_some_and(|owned| owned != job) {
                        entry.status = "unknown".into();
                        entry.error = Some("Delegation Job identity changed".into());
                        retain_unknown(entry, store);
                        return format!(
                            "unknown · {}: Delegation Job identity changed",
                            request.id
                        );
                    }
                    entry.job = Some(job.into());
                }
                entry.error = record["error"].as_str().map(str::to_string);
                if status == "unknown" {
                    retain_unknown(entry, store);
                }
                if matches!(status, "admitted" | "cancelled" | "failed")
                    && (!entry.stopping || stop || status != "admitted")
                {
                    store.forget_admission(&request.id);
                }
            }
            Err(error) => {
                entry.status = "unknown".into();
                entry.error = Some(error);
                retain_unknown(entry, store);
            }
        }
        format!(
            "{} · {}{} · /admissions to inspect",
            entry.status,
            request.id,
            entry
                .error
                .as_ref()
                .map_or(String::new(), |e| format!(": {e}"))
        )
    }
    pub fn choices(&self, source: &str) -> Vec<Choice> {
        self.0
            .values()
            .filter(|e| e.request.source == source)
            .map(|e| Choice {
                key: e.request.id.clone(),
                label: format!("{} · {}", e.status, e.request.id),
                detail: e
                    .error
                    .clone()
                    .or_else(|| e.job.clone())
                    .unwrap_or_default(),
            })
            .collect()
    }
}

pub struct Context<'a> {
    pub store: &'a LocalStore,
    pub tx: &'a mpsc::Sender<Result<Msg, String>>,
}

pub async fn start(
    client: &Client,
    source: &str,
    directory: &str,
    body: Value,
    context: Option<&Context<'_>>,
) -> Result<Msg, String> {
    let request = Request {
        id: cyber_core::ids::new_id("op"),
        source: source.into(),
        directory: directory.into(),
    };
    if !request.valid() {
        return Err("Invalid delegation source identity".into());
    }
    if let Some(context) = context {
        context.store.save_admission(&request)?;
        context
            .tx
            .send(Ok(Msg::AdmissionStarted(request.clone())))
            .await
            .map_err(|_| "TUI closed before submission".to_string())?;
    }
    let result = match client.post(&request.path(), body).await {
        Ok(response) => check(&request, response),
        Err(error) => lookup(client, &request)
            .await
            .map_err(|lookup| format!("Submission was not acknowledged: {error}; {lookup}")),
    };
    Ok(Msg::AdmissionUpdated {
        request,
        result,
        stop: false,
    })
}
pub async fn perform_action(client: &Client, action: Action) -> Result<Msg, String> {
    match action {
        Action::ReconcileAdmission(request) => Ok(reconcile(client, request).await),
        Action::Admission { request, stop } => Ok(perform(client, request, stop).await),
        _ => Err("Unsupported admission action".into()),
    }
}

pub async fn perform(client: &Client, request: Request, stop: bool) -> Msg {
    if !request.valid() {
        return Msg::AdmissionUpdated {
            request,
            stop,
            result: Err("Invalid delegation identity".into()),
        };
    }
    let scoped = client.at(&request.directory);
    let result = if stop {
        tokio::time::timeout(std::time::Duration::from_secs(3), cancel(&scoped, &request))
            .await
            .unwrap_or_else(|_| Err("Cancellation acknowledgement timed out".into()))
    } else {
        lookup(&scoped, &request).await
    };
    Msg::AdmissionUpdated {
        request,
        result,
        stop,
    }
}
pub async fn reconcile(client: &Client, request: Request) -> Msg {
    let result = if request.valid() {
        tokio::time::timeout(std::time::Duration::from_secs(3), async {
            let scoped = client.at(&request.directory);
            match scoped
                .post(&format!("{}/reconcile", request.path()), Value::Null)
                .await
            {
                Ok(response) => check(&request, response),
                Err(error) => lookup(&scoped, &request).await.map_err(|lookup| {
                    format!("Reconciliation was not acknowledged: {error}; {lookup}")
                }),
            }
        })
        .await
        .unwrap_or_else(|_| Err("Reconciliation acknowledgement timed out".into()))
    } else {
        Err("Invalid delegation identity".into())
    };
    Msg::AdmissionUpdated {
        request,
        result,
        stop: false,
    }
}

async fn lookup(client: &Client, request: &Request) -> Result<Value, String> {
    check(
        request,
        client
            .get(&request.path())
            .await
            .map_err(|e| e.to_string())?,
    )
}
fn check(request: &Request, response: Value) -> Result<Value, String> {
    if !request.valid() {
        return Err("Invalid delegation identity".into());
    }
    let r = &response["data"];
    if r["id"] != request.id || r["session_id"] != request.source {
        return Err("Delegation admission identity changed".into());
    }
    let phase = r["phase"].as_str().unwrap_or_default();
    let status = r["status"].as_str().unwrap_or_default();
    let job = r["job_id"].as_str();
    if (!r["job_id"].is_null() && job.is_none())
        || (!r["error"].is_null() && !r["error"].is_string())
    {
        return Err("Invalid delegation admission record".into());
    }
    let valid = match status {
        "pending" | "cancelling" => matches!(phase, "reserved" | "launching") && job.is_none(),
        "cancelled" | "failed" => phase == "reserved" && job.is_none(),
        "admitted" => phase == "launching" && job.is_some_and(|id| safe_id(id, "job")),
        "unknown" => {
            matches!(phase, "reserved" | "launching")
                && job.is_none_or(|id| phase == "launching" && safe_id(id, "job"))
        }
        _ => false,
    };
    if !valid {
        return Err("Invalid delegation admission record".into());
    }
    Ok(response["data"].clone())
}
async fn cancel(client: &Client, request: &Request) -> Result<Value, String> {
    let record = check(
        request,
        client
            .post(&format!("{}/stop", request.path()), json!({}))
            .await
            .map_err(|e| e.to_string())?,
    )?;
    if let Some(id) = record["job_id"].as_str() {
        let response = client
            .get(&format!("/jobs/{id}"))
            .await
            .map_err(|e| e.to_string())?;
        let job = &response["data"];
        if job["id"] != id
            || job["session_id"] != request.source
            || !job["child_id"]
                .as_str()
                .is_some_and(|child| safe_id(child, "ses") && child != request.source)
        {
            return Err("Delegation Job ownership changed".into());
        }
        if !matches!(
            job["status"].as_str(),
            Some("completed" | "cancelled" | "error" | "interrupted")
        ) {
            return Err(format!(
                "Cancellation was not acknowledged; Job {id} is not terminal"
            ));
        }
    }
    Ok(record)
}

#[cfg(test)]
#[path = "admissions_tests.rs"]
mod tests;

fn retain_unknown(entry: &mut Entry, store: &LocalStore) {
    if let Err(error) = store.save_admission(&entry.request) {
        let detail = entry
            .error
            .take()
            .unwrap_or_else(|| "Admission outcome is unknown".into());
        entry.error = Some(format!(
            "{detail}; recovery identity could not be saved: {error}"
        ));
    }
}
