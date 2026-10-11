//! The request ID exists before a POST, so even a lost response remains stoppable.
use super::{Scope, api, safe_id, stop_job};
use crate::error::CliError;
use cyber_client::Client;
use cyber_server::runtime::{Delegation, DelegationPhase, DelegationStatus};
use serde_json::{Value, json};
use std::time::Duration;

pub(in crate::commands::exec) struct Admission {
    pub id: String,
    parent: String,
}
impl Admission {
    /// An admission the server derived itself (a forwarded `context: fork` command).
    pub fn existing(parent: &str, id: &str) -> Result<Self, CliError> {
        if !safe_id(parent, "ses") || !safe_id(id, "op") {
            return Err(CliError::runtime("Invalid delegation admission identity"));
        }
        Ok(Self {
            id: id.into(),
            parent: parent.into(),
        })
    }
    pub fn new(parent: &str) -> Result<Self, CliError> {
        if !safe_id(parent, "ses") {
            return Err(CliError::runtime("Invalid source Session identity"));
        }
        Ok(Self {
            id: cyber_core::ids::new_id("op"),
            parent: parent.into(),
        })
    }
    fn path(&self) -> String {
        format!("/sessions/{}/delegations/{}", self.parent, self.id)
    }
    fn check(&self, response: &Value) -> Result<Delegation, CliError> {
        let record: Delegation = serde_json::from_value(response["data"].clone())
            .map_err(|_| CliError::runtime("Invalid delegation admission record"))?;
        if record.id != self.id || record.session_id != self.parent {
            return Err(CliError::runtime("Delegation admission identity changed"));
        }
        if record
            .job_id
            .as_deref()
            .is_some_and(|id| !safe_id(id, "job"))
            || (record.job_id.is_some()
                && (record.phase != DelegationPhase::Launching
                    || !matches!(
                        record.status,
                        DelegationStatus::Admitted | DelegationStatus::Unknown
                    )))
            || (record.status == DelegationStatus::Admitted && record.job_id.is_none())
            || (matches!(
                record.status,
                DelegationStatus::Cancelled | DelegationStatus::Failed
            ) && record.phase != DelegationPhase::Reserved)
        {
            return Err(CliError::runtime("Invalid delegation admission Job"));
        }
        Ok(record)
    }
    pub(in crate::commands::exec) async fn lookup(
        &self,
        client: &Client,
    ) -> Result<Delegation, CliError> {
        self.check(&client.get(&self.path()).await.map_err(api)?)
    }
    /// Like `lookup`, but a request the server never recorded is `None`.
    pub(in crate::commands::exec) async fn find(
        &self,
        client: &Client,
    ) -> Result<Option<Delegation>, CliError> {
        match client.get(&self.path()).await {
            Ok(response) => self.check(&response).map(Some),
            Err(error) if error.tag() == Some("RequestNotFoundError") => Ok(None),
            Err(error) => Err(api(error)),
        }
    }
    async fn scope(&self, client: &Client, id: &str) -> Result<Scope, CliError> {
        let response = client.get(&format!("/jobs/{id}")).await.map_err(api)?;
        if response["data"]["id"] != id {
            return Err(CliError::runtime("Admission Job identity changed"));
        }
        Scope::new(&self.parent, &response["data"])
    }
    pub async fn start(&self, client: &Client, body: Value) -> Result<Scope, CliError> {
        let mut record = match client.post(&self.path(), body).await {
            Ok(response) => self.check(&response)?,
            Err(error) => self.lookup(client).await.map_err(|lookup| {
                CliError::runtime(format!(
                    "Admission was not acknowledged; inspect {}: {error}; {}",
                    self.id, lookup.message
                ))
            })?,
        };
        loop {
            match record.status {
                DelegationStatus::Admitted => {
                    return self
                        .scope(client, record.job_id.as_deref().expect("checked Job"))
                        .await;
                }
                DelegationStatus::Pending | DelegationStatus::Cancelling => {}
                _ => {
                    return Err(CliError::runtime(format!(
                        "Delegation admission {:?}; inspect {}: {}",
                        record.status,
                        self.id,
                        record
                            .error
                            .as_deref()
                            .unwrap_or("No child Job was acknowledged")
                    )));
                }
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
            record = self.lookup(client).await?;
        }
    }
    pub(in crate::commands::exec) async fn stop(
        &self,
        client: &Client,
        owned: &mut Option<Scope>,
    ) -> Result<(), CliError> {
        let response = client
            .post(&format!("{}/stop", self.path()), json!({}))
            .await
            .map_err(api)?;
        let mut record = self.check(&response)?;
        loop {
            if let Some(job) = &record.job_id {
                *owned = Some(self.scope(client, job).await?);
                let scope = owned.as_ref().expect("validated scope");
                return scope.check(&stop_job(client, job).await?);
            }
            match record.status {
                DelegationStatus::Cancelled | DelegationStatus::Failed => return Ok(()),
                DelegationStatus::Pending | DelegationStatus::Cancelling => {}
                _ => {
                    return Err(CliError::runtime(
                        record
                            .error
                            .unwrap_or("Admission cancellation outcome is unknown".into()),
                    ));
                }
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
            record = self.lookup(client).await?;
        }
    }
}
