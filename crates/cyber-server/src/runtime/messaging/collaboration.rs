//! Authenticated hub ingress. Provenance is part of the immutable sender key in the
//! normal messaging event/projection, so restart and rebuild retain release authority.
use super::{
    Messaging, RuntimeError, SendResult, Sender,
    inbound::{Reach, SenderAddresses, Verdict},
    limits, records,
    send::{Outgoing, check_id},
};
use crate::runtime::Runtime;
use futures::future::BoxFuture;
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, PoisonError};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CollaborationOrigin {
    pub hub_id: String,
    pub project_id: String,
    pub member_id: String,
    pub agent_id: Option<String>,
    pub member_generation: u64,
    pub membership_kind: String,
    pub remote_message_id: String,
    pub target_agent_id: String,
    #[serde(default)]
    pub depth: u32,
    #[serde(default = "first_hop")]
    pub hops: u32,
}

/// The connector must revalidate current membership/publication at the hub, without
/// trusting the persisted admission snapshot. Unavailable checks keep content held.
pub type CollaborationAuthorizer =
    dyn Fn(String, CollaborationOrigin) -> BoxFuture<'static, Result<(), String>> + Send + Sync;

fn first_hop() -> u32 {
    1
}

impl CollaborationOrigin {
    fn valid_id(value: &str, prefix: &str) -> bool {
        value.len() <= 128
            && value.strip_prefix(prefix).is_some_and(|tail| {
                !tail.is_empty() && tail.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
            })
    }
    pub fn validate(&self) -> Result<(), RuntimeError> {
        let valid = Self::valid_id(&self.hub_id, "chub_")
            && Self::valid_id(&self.project_id, "cprj_")
            && Self::valid_id(&self.member_id, "cmem_")
            && Self::valid_id(&self.remote_message_id, "cmsg_")
            && Self::valid_id(&self.target_agent_id, "cagt_")
            && self
                .agent_id
                .as_deref()
                .is_none_or(|id| Self::valid_id(id, "cagt_"))
            && self.hops > 0
            && self.member_generation > 0
            && matches!(
                self.membership_kind.as_str(),
                "internal" | "guest" | "friend"
            );
        if valid {
            Ok(())
        } else {
            Err(RuntimeError::Invalid(
                "invalid collaboration provenance".into(),
            ))
        }
    }
    pub(super) fn prefix(&self) -> String {
        format!(
            "collaboration:{}:{}:{}:",
            self.hub_id, self.project_id, self.member_id
        )
    }
    pub(super) fn key(&self) -> String {
        format!(
            "{}{}",
            self.prefix(),
            serde_json::to_string(self).expect("string-only collaboration provenance")
        )
    }
    pub(super) fn label(&self) -> String {
        format!(
            "{} [collaboration {}, {} owner {}]",
            self.agent_id.as_deref().unwrap_or(&self.member_id),
            self.project_id,
            self.membership_kind,
            self.member_id
        )
    }
    pub fn from_key(key: &str) -> Option<Self> {
        let (_, json) = key.split_once('{')?;
        let origin: Self = serde_json::from_str(&format!("{{{json}")).ok()?;
        (origin.validate().is_ok() && origin.key() == key).then_some(origin)
    }
}

impl Messaging {
    pub async fn receive_collaboration(
        &self,
        local_session_id: &str,
        stable_local_msgx_id: &str,
        origin: CollaborationOrigin,
        text: String,
    ) -> Result<SendResult, RuntimeError> {
        origin.validate()?;
        check_id(Some(stable_local_msgx_id))?;
        limits::check_text(&text)?;
        let sender = Sender::Collaboration(origin);
        let max_hops = self.sender_enabled(&sender)?.max_hops;
        let target = self.resolve(None, local_session_id)?;
        if target.candidate.session_id != local_session_id {
            return Err(RuntimeError::Invalid(
                "collaboration target must be a local Session ID".into(),
            ));
        }
        let Sender::Collaboration(origin) = &sender else {
            unreachable!()
        };
        let from = origin.label();
        let (origin_depth, origin_hops) = (origin.depth, origin.hops);
        let outgoing = Outgoing {
            id: stable_local_msgx_id.into(),
            sender,
            reach: Reach::CrossMachine,
            target,
            from: from.clone(),
            addresses: SenderAddresses {
                trusted: Vec::new(),
                other: vec![from],
            },
            text,
            depth: origin_depth,
            hops: origin_hops,
            max_hops,
        };
        self.send_locked(&outgoing).await
    }
}

impl Runtime {
    /// Registered by the explicit local connector. The callback must not capture a
    /// strong Runtime reference (the host owns it alongside the Runtime).
    pub fn set_collaboration_authorizer(&self, callback: Arc<CollaborationAuthorizer>) {
        *self
            .inner
            .collaboration_authorizer
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(callback);
    }

    pub(crate) async fn authorize_collaboration_release(
        &self,
        session_id: &str,
        message_id: &str,
    ) -> Result<(), RuntimeError> {
        let (session, id) = (session_id.to_owned(), message_id.to_owned());
        let row = self.inner.store.read(move |db| {
            Ok(db.query_row(
                &format!("SELECT {} FROM xsession_message WHERE session_id=?1 AND message_id=?2 AND direction='in'", records::MESSAGE_COLUMNS),
                rusqlite::params![session,id], records::message_row,
            ).optional()?)
        })?;
        if row.is_none() {
            let handle = self.inner.handle(session_id).await?;
            if handle
                .state
                .lock()
                .await
                .message_chains
                .contains_key(message_id)
            {
                return Err(RuntimeError::Invalid(
                    "message provenance is unavailable; message remains held".into(),
                ));
            }
        }
        let Some(row) = row.filter(|r| r.origin_kind == "collaboration") else {
            return Ok(());
        };
        let origin = CollaborationOrigin::from_key(&row.sender_key).ok_or_else(|| {
            RuntimeError::Invalid(
                "collaboration provenance is unavailable; message remains held".into(),
            )
        })?;
        let callback = self
            .inner
            .collaboration_authorizer
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
            .ok_or_else(|| {
                RuntimeError::Invalid(
                    "collaboration authorization is unavailable; message remains held".into(),
                )
            })?;
        // No Session state lock is held across remote I/O.
        callback(session_id.to_owned(), origin.clone())
            .await
            .map_err(|_| {
                RuntimeError::Invalid(
                    "collaboration authorization failed; message remains held".into(),
                )
            })?;
        let messaging = self.messaging();
        let target = messaging.resolve(None, session_id)?;
        let settings = messaging.settings(Some(&target.candidate.location.directory));
        if settings.as_ref().is_some_and(|s| row.hops > s.max_hops) {
            return Err(RuntimeError::Invalid(
                "collaboration message remains held: chain_limit".into(),
            ));
        }
        let addresses = SenderAddresses {
            trusted: Vec::new(),
            other: vec![origin.label()],
        };
        if let Verdict::Refuse(reason) =
            super::inbound_verdict(&target, settings.as_ref(), &addresses, Reach::CrossMachine)
        {
            return Err(RuntimeError::Invalid(format!(
                "collaboration message remains held: {reason}"
            )));
        }
        Ok(())
    }
}
