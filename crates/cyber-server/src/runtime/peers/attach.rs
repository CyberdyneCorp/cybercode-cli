//! Approval of a peer attach (`cross-session-messaging` → Peer attach): every attach of a
//! peer to one Session raises the `remote.attach` permission in that Session, with the
//! resource `peer:<grant>`, and nothing of the Session is shown until it is approved.
//!
//! Only the permission rules decide whether to ask: any `deny` rule refuses, and only an
//! explicit `remote.attach` allow in user (or global, command-line, managed) config skips
//! the prompt. A blanket `"permissions": "allow"`, a `"*"` action and repository, agent
//! and Session rules still ask, and no Mode approves an attach on its own. A request nobody answers
//! within the wait (2 minutes) is withdrawn and the attach fails, as does a refusal, a
//! server without an interactive client, or rules that cannot be read.

use std::time::Duration;

use serde_json::json;

use super::super::requests::{Asker, PermissionAsk, PermissionReply};
use super::super::{RuleEffect, Runtime, RuntimeError};

/// The permission action of an attach.
pub const REMOTE_ATTACH: &str = "remote.attach";

/// How long an attach waits for the user to answer `remote.attach`.
pub const ATTACH_WAIT: Duration = Duration::from_secs(120);

/// The `remote.attach` resource of peer grant `grant`.
pub fn resource(grant: &str) -> String {
    format!("peer:{grant}")
}

impl Runtime {
    /// Whether peer grant `grant` may attach to Session `session_id`: `Ok` once the rules
    /// allow it or the user approves within `wait`, else `PermissionDeniedError`.
    pub async fn approve_attach(
        &self,
        session_id: &str,
        grant: &str,
        wait: Duration,
    ) -> Result<(), RuntimeError> {
        let info = self.state(session_id).await?.info;
        let turn = super::super::drain::turn_context_for_info(&info, false);
        let resource = resource(grant);
        let effect = self
            .inner
            .tools
            .permission_effect(&turn, REMOTE_ATTACH, &resource)
            .map_err(|error| {
                RuntimeError::PermissionDenied(format!(
                    "{REMOTE_ATTACH} for {resource} could not be evaluated: {error}"
                ))
            })?;
        match effect {
            RuleEffect::Allow => return Ok(()),
            RuleEffect::Deny => {
                return Err(RuntimeError::PermissionDenied(format!(
                    "{REMOTE_ATTACH} is denied for {resource}"
                )));
            }
            RuleEffect::Ask => {}
        }
        let asker = self.operation_asker(session_id).await?;
        let ask = PermissionAsk {
            action: REMOTE_ATTACH.into(),
            resources: vec![resource.clone()],
            // An approval is for this attach only.
            always_patterns: Vec::new(),
            metadata: json!({ "peer": grant, "requires_confirmation": true }),
        };
        let mut pending = Withdraw(Some(asker.clone()));
        let reply = tokio::time::timeout(wait, asker.permission(ask)).await;
        let withdrawn = pending.0.take();
        match reply {
            Ok(PermissionReply::Once | PermissionReply::Always) => Ok(()),
            Ok(PermissionReply::Unattended) => Err(RuntimeError::PermissionDenied(format!(
                "{REMOTE_ATTACH} for {resource} needs an interactive client on this machine"
            ))),
            Ok(PermissionReply::Reject { .. }) => Err(RuntimeError::PermissionDenied(format!(
                "{REMOTE_ATTACH} for {resource} was refused"
            ))),
            Err(_) => {
                if let Some(asker) = withdrawn {
                    asker.cancel_requests().await?;
                }
                Err(RuntimeError::PermissionDenied(format!(
                    "{REMOTE_ATTACH} for {resource} was not answered within {} s",
                    wait.as_secs()
                )))
            }
        }
    }
}

/// Withdraws the attach request when the wait is dropped (the peer hung up), so no
/// approval is left pending for an attach that no longer exists.
struct Withdraw(Option<Asker>);

impl Drop for Withdraw {
    fn drop(&mut self) {
        let Some(asker) = self.0.take() else {
            return;
        };
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(async move {
                let _ = asker.cancel_requests().await;
            });
        }
    }
}
