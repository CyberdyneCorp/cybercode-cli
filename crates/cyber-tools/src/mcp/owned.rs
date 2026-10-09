//! Couple real local transport lifetime to durable MCP ownership and checkout pins.
use std::future::Future;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use cyber_core::config::McpServer;
use cyber_core::worktrees::{CheckoutLease, Repository};
use cyber_server::runtime::{
    McpConnectionObserver, McpConnectionOwner, McpConnectionPhase, McpConnectionRecord,
};
use cyber_store::Store;
use serde_json::Value;

use super::{
    DiscoveredTool, LocalLaunchError, LocalLauncher, LocalServer, McpError, StderrCapture,
    authorize_server,
};

/// Real checkout leases; immutable owner/creation identities come from the locked records.
#[derive(Default)]
pub struct McpLocationPin(Vec<CheckoutLease>);

impl McpLocationPin {
    pub fn unmanaged() -> Self {
        Self::default()
    }
    pub fn managed(leases: Vec<CheckoutLease>) -> Self {
        Self(leases)
    }

    fn ids(&self) -> Vec<String> {
        self.0
            .iter()
            .map(|lease| lease.worktree_id().into())
            .collect()
    }

    fn verify(&self, location: &Path, connection: &str) -> Result<(), String> {
        if self.0.iter().any(|lease| lease.owner_id() != connection) {
            return Err("MCP checkout pin belongs to another native owner".into());
        }
        let mut expected: Vec<_> = Repository::managed_locations_at(location)
            .map_err(|_| "MCP managed Location verification failed")?
            .into_iter()
            .map(|(_, managed)| managed.id)
            .collect();
        let mut observed = self.ids();
        expected.sort();
        observed.sort();
        if expected != observed {
            return Err("MCP checkout pins do not match the complete managed Location".into());
        }
        Ok(())
    }

    fn settle_retained(self) -> Result<Vec<CheckoutLease>, String> {
        self.0
            .into_iter()
            .map(|lease| lease.settle_retained().map_err(|error| error.to_string()))
            .collect()
    }
}

pub struct OwnedLocalServer {
    // Scratch cleanup occurs before acknowledged checkout locks are released on disposal.
    server: LocalServer,
    owner: McpConnectionOwner,
    pin: Option<McpLocationPin>,
    retained: Option<Vec<CheckoutLease>>,
    closing: bool,
}

impl OwnedLocalServer {
    pub fn record(&self) -> &McpConnectionRecord {
        self.owner.record()
    }
    pub fn tools(&self) -> &[DiscoveredTool] {
        if self.closing {
            &[]
        } else {
            self.server.tools()
        }
    }
    pub fn metadata(&self) -> &Value {
        self.server.metadata()
    }
    pub fn unresolved(&self) -> bool {
        self.server.unresolved()
    }
    pub fn scratch_path(&self) -> &Path {
        self.server.scratch_path()
    }

    pub async fn call_exposed_tool(
        &mut self,
        name: &str,
        arguments: Value,
        timeout: Duration,
    ) -> Result<Value, McpError> {
        self.running()?;
        self.server
            .call_exposed_tool(name, arguments, timeout)
            .await
    }

    pub async fn call_tool(
        &mut self,
        name: &str,
        arguments: Value,
        timeout: Duration,
    ) -> Result<Value, McpError> {
        self.running()?;
        self.server.call_tool(name, arguments, timeout).await
    }

    pub async fn refresh_tools(&mut self, timeout: Duration) -> Result<(), McpError> {
        self.running()?;
        self.server.refresh_tools(timeout).await
    }

    fn running(&self) -> Result<(), McpError> {
        if self.closing || self.owner.record().phase != McpConnectionPhase::Running {
            return Err(McpError::Protocol("MCP native owner is not running"));
        }
        Ok(())
    }

    /// Keep the object and its capabilities after unknown or failed settlement for retry.
    pub async fn shutdown(
        &mut self,
    ) -> Result<(McpConnectionRecord, StderrCapture), LocalLaunchError> {
        self.closing = true;
        let (acknowledged, stderr) = self.server.shutdown().await;
        if !acknowledged {
            let _ = self
                .owner
                .finish(false, "MCP native shutdown is unverified".into());
            return Err(LocalLaunchError {
                diagnostic: "MCP native shutdown is unverified".into(),
                acknowledged: false,
                stderr,
                scratch: Some(self.server.scratch_path().into()),
            });
        }
        if self.retained.is_some() {
            return Ok((self.owner.record().clone(), stderr));
        }
        let owner = &mut self.owner;
        let pin = &mut self.pin;
        let settled = self.server.settle_after_shutdown(|| {
            if owner.record().phase != McpConnectionPhase::Settled {
                owner
                    .finish(true, "MCP server stopped".into())
                    .map_err(|error| error.to_string())?;
            }
            pin.take()
                .ok_or("MCP checkout settlement requires recovery")?
                .settle_retained()
        });
        match settled {
            Ok(pin) => {
                self.retained = Some(pin);
                Ok((self.owner.record().clone(), stderr))
            }
            Err(error) => Err(LocalLaunchError {
                diagnostic: error,
                acknowledged: true,
                stderr,
                scratch: Some(self.server.scratch_path().into()),
            }),
        }
    }
}

impl LocalLauncher<'_> {
    /// The callback must claim all actual managed leases under this connection identity.
    pub async fn connect_owned<F>(
        &self,
        name: &str,
        store: Arc<Store>,
        claim: impl FnOnce(String) -> F,
    ) -> Result<OwnedLocalServer, LocalLaunchError>
    where
        F: Future<Output = Result<McpLocationPin, String>>,
    {
        self.connect_owned_observed(name, store, None, claim).await
    }

    pub async fn connect_owned_observed<F>(
        &self,
        name: &str,
        store: Arc<Store>,
        observer: Option<McpConnectionObserver>,
        claim: impl FnOnce(String) -> F,
    ) -> Result<OwnedLocalServer, LocalLaunchError>
    where
        F: Future<Output = Result<McpLocationPin, String>>,
    {
        let selected = authorize_server(self.resolved, self.trust, self.location, name)
            .map_err(LocalLaunchError::before_launch)?;
        if !matches!(selected.definition, McpServer::Local { .. }) {
            return Err(LocalLaunchError::before_launch(
                "MCP server requires remote transport",
            ));
        }
        let deadline =
            tokio::time::Instant::now() + Duration::from_secs(selected.definition.timeout().into());
        let mut owner = McpConnectionOwner::admit_observed(
            store,
            self.location,
            &self.resolved.trust.checkout_root,
            name,
            &selected.digest,
            observer,
        )
        .map_err(|error| LocalLaunchError::before_launch(error.to_string()))?;
        owner
            .preparing()
            .map_err(|error| LocalLaunchError::before_launch(error.to_string()))?;
        let pin = tokio::time::timeout_at(deadline, claim(owner.record().id.clone()))
            .await
            .map_err(|_| LocalLaunchError {
                diagnostic: "MCP native preparation timed out; settlement is unverified".into(),
                acknowledged: false,
                stderr: StderrCapture::default(),
                scratch: None,
            })?
            .map_err(|diagnostic| LocalLaunchError {
                diagnostic,
                acknowledged: false,
                stderr: StderrCapture::default(),
                scratch: None,
            })?;
        pin.verify(self.location, &owner.record().id)
            .map_err(|diagnostic| LocalLaunchError {
                diagnostic,
                acknowledged: false,
                stderr: StderrCapture::default(),
                scratch: None,
            })?;
        let ids = pin.ids();
        match self
            .connect_before(
                name,
                || owner.launching(ids).map_err(|error| error.to_string()),
                Some(deadline),
            )
            .await
        {
            Ok(mut server) => {
                if let Err(error) = owner.connected() {
                    let (acknowledged, stderr) = server.shutdown().await;
                    let _ =
                        owner.finish(acknowledged, "MCP connected receipt commit failed".into());
                    return Err(LocalLaunchError {
                        diagnostic: error.to_string(),
                        acknowledged,
                        stderr,
                        scratch: Some(server.scratch_path().into()),
                    });
                }
                Ok(OwnedLocalServer {
                    server,
                    owner,
                    pin: Some(pin),
                    retained: None,
                    closing: false,
                })
            }
            Err(error) => {
                // Acknowledged preparation/spawn failure still needs durable settlement.
                owner
                    .finish(error.acknowledged, error.diagnostic.clone())
                    .map_err(|failure| LocalLaunchError {
                        diagnostic: failure.to_string(),
                        acknowledged: error.acknowledged,
                        stderr: StderrCapture::default(),
                        scratch: error.scratch.clone(),
                    })?;
                if error.acknowledged {
                    let _retained = pin
                        .settle_retained()
                        .map_err(LocalLaunchError::before_launch)?;
                }
                Err(error)
            }
        }
    }
}
