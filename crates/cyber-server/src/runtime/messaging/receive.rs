//! The receiving side of the direct peer transport (`cross-session-messaging` → Direct
//! peer transport; design sections 3 and 5): a message another machine posted to
//! `POST /api/v1/peer/messages`, through the same pipeline as a local send.
//!
//! The sender is named after the grant that authenticated the request
//! ([`Sender::Peer`]): `from_session` and `from_name` are labels inside that grant's
//! namespace (`ses_…@laptop`, `build@laptop`), so a peer can speak only for its own
//! Sessions. The target is resolved with the normal tiers minus the caller-scoped ones (a
//! peer has no subagents or team here), never as a further peer, and only among the
//! Sessions the grant is shown (as for its listing): a Session that refuses the grant is
//! not found, an ambiguous name lists only the shown candidates, located by workspace
//! name unless `peers.share_paths`. Peer senders are off machine: the inbound default is
//! `hold`, limits count every Session of the grant together (`peer:<grant>:*`), and the
//! chain hops and MCP depth come from the body. A retry with the same ID from the same
//! grant returns the first outcome.

use rusqlite::{OptionalExtension, params};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::address::{self, AddressError, Target, addressable_name};
use super::inbound::SenderAddresses;
use super::listing::shown_location;
use super::send::{Outcome, Outgoing, SendResult, Sender, check_id};
use super::{Messaging, RuntimeError, limits};

/// Longest `from_session` and `from_name` a peer may send.
const LABEL_MAX: usize = 128;

/// `POST /api/v1/peer/messages`: one message from a Session of the calling machine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PeerMessage {
    /// The sender's `msgx_` ID; a retry with the same ID returns the first outcome.
    pub id: String,
    /// The target's address on this machine (`ses_` ID or Session name).
    pub to: String,
    /// Plain UTF-8 text, at most 16384 bytes.
    pub text: String,
    /// The sender watches the target itself; accepted for compatibility.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub notify_when_idle: bool,
    /// The sending run's MCP recursion depth.
    #[serde(default)]
    pub depth: u32,
    /// The message's chain hops (the sending run's hops + 1).
    pub hops: u32,
    /// The sending Session's ID on the calling machine.
    pub from_session: String,
    /// The sending Session's unique title there, if it has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_name: Option<String>,
}

/// The answer to `POST /api/v1/peer/messages`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PeerSendResult {
    pub message_id: String,
    pub outcome: Outcome,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// The limit a `rate_limited` refusal hit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<String>,
    /// The Session the address resolved to on this machine.
    pub target_session_id: String,
    /// The ID was received before; this is that message's outcome.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub replayed: bool,
}

impl PeerSendResult {
    fn new(result: SendResult, target: String) -> Self {
        Self {
            message_id: result.message_id,
            outcome: result.outcome,
            reason: result.reason,
            limit: result.limit,
            target_session_id: target,
            replayed: result.replayed,
        }
    }
}

/// `GET /api/v1/peer/messages/:id`: the current outcome of a message this grant sent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PeerOutcome {
    pub message_id: String,
    /// `delivered`, `held`, `released`, `dropped` or `refused`.
    pub outcome: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub target_session_id: String,
}

impl PeerMessage {
    /// The body's own rules; nothing is resolved or written before they pass.
    fn check(&self) -> Result<(), RuntimeError> {
        check_id(Some(&self.id))?;
        if self.hops == 0 {
            return Err(invalid("hops starts at 1"));
        }
        if !session_label(&self.from_session) {
            return Err(invalid(
                "from_session is not a ses_ ID of letters, digits, _ or -",
            ));
        }
        if self
            .from_name
            .as_deref()
            .is_some_and(|name| !name_label(name))
        {
            return Err(invalid("from_name is not a plain Session name"));
        }
        Ok(())
    }
}

fn not_found(to: &str) -> RuntimeError {
    RuntimeError::Address(AddressError::NotFound(to.to_owned()))
}

fn invalid(message: &str) -> RuntimeError {
    RuntimeError::Invalid(message.into())
}

/// A `ses_` ID as a peer may send it.
fn session_label(id: &str) -> bool {
    id.len() <= LABEL_MAX
        && id.strip_prefix("ses_").is_some_and(|rest| {
            !rest.is_empty()
                && rest
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        })
}

/// A name a caller would type for a Session, without control characters.
fn name_label(name: &str) -> bool {
    name.len() <= LABEL_MAX && !name.chars().any(char::is_control) && addressable_name(name)
}

impl Messaging {
    /// Receive a message from a Session of the machine `grant` names. A refusal is an
    /// outcome, not an error; errors leave nothing written. `share_paths` is
    /// `peers.share_paths`: whether an ambiguous address shows full Location paths.
    pub async fn receive(
        &self,
        grant: &str,
        share_paths: bool,
        message: PeerMessage,
    ) -> Result<PeerSendResult, RuntimeError> {
        message.check()?;
        let sender = Sender::Peer {
            grant: grant.to_owned(),
            session_id: message.from_session,
            name: message.from_name,
        };
        let max_hops = self.sender_enabled(&sender)?.max_hops;
        limits::check_text(&message.text)?;
        let target = self.resolve_for_grant(grant, share_paths, &message.to)?;
        let addresses = sender.peer_addresses();
        // The name when the peer gave one, else the ID: what the target replies to.
        let from = addresses.last().cloned().unwrap_or_default();
        let target_id = target.candidate.session_id.clone();
        let reach = sender.reach();
        let outgoing = Outgoing {
            id: message.id,
            sender,
            reach,
            target,
            from,
            addresses: SenderAddresses::trusted(addresses),
            text: message.text,
            depth: message.depth,
            hops: message.hops,
            max_hops,
        };
        let result = self.send_locked(&outgoing).await?;
        Ok(PeerSendResult::new(result, target_id))
    }

    /// Resolve `to` among the Sessions grant `grant` is shown, so that an answer never
    /// names, locates or confirms one it is not: a hidden target is not found, and an
    /// ambiguous name lists only the shown candidates (resolving to the one left, if only
    /// one is), located as its listing locates them.
    fn resolve_for_grant(
        &self,
        grant: &str,
        share_paths: bool,
        to: &str,
    ) -> Result<Target, RuntimeError> {
        let found = self.resolve_root(to)?;
        let candidates = match found {
            Ok(target) => {
                return match self.shown_to(grant, &target.candidate.session_id)? {
                    Some(_) => Ok(target),
                    None => Err(not_found(to)),
                };
            }
            Err(AddressError::Ambiguous { candidates, .. }) => candidates,
            Err(error) => return Err(RuntimeError::Address(error)),
        };
        let mut shown = Vec::new();
        for candidate in candidates {
            if self.shown_to(grant, &candidate.session_id)?.is_some() {
                shown.push(candidate);
            }
        }
        match shown.len() {
            0 => Err(not_found(to)),
            1 => {
                let mut target = self
                    .resolve_root(&shown[0].session_id)?
                    .map_err(|_| not_found(to))?;
                target.address = to.strip_prefix('@').unwrap_or(to).to_owned();
                Ok(target)
            }
            _ => Err(RuntimeError::Address(AddressError::Ambiguous {
                address: to.to_owned(),
                candidates: shown
                    .into_iter()
                    .map(|mut candidate| {
                        candidate.location = shown_location(candidate.location, share_paths);
                        candidate
                    })
                    .collect(),
            })),
        }
    }

    /// `to` resolved among this server's root Sessions, with no caller.
    fn resolve_root(&self, to: &str) -> Result<Result<Target, AddressError>, RuntimeError> {
        let to = to.to_owned();
        Ok(self
            .runtime
            .inner
            .store
            .read(move |db| Ok(address::resolve(db, None, &to)?))?)
    }

    /// The current outcome of message `id` that `grant` sent, if this machine recorded it.
    pub fn peer_outcome(&self, grant: &str, id: &str) -> Result<Option<PeerOutcome>, RuntimeError> {
        let (grant, id) = (grant.to_owned(), id.to_owned());
        Ok(self.runtime.inner.store.read(move |db| {
            Ok(db
                .query_row(
                    "SELECT session_id, outcome, reason FROM xsession_message
                     WHERE message_id = ?1 AND direction = 'in' AND origin_kind = 'peer'
                       AND remote_peer = ?2",
                    params![id, grant],
                    |row| {
                        Ok(PeerOutcome {
                            message_id: id.clone(),
                            target_session_id: row.get(0)?,
                            outcome: row.get(1)?,
                            reason: row.get(2)?,
                        })
                    },
                )
                .optional()?)
        })?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message() -> PeerMessage {
        PeerMessage {
            id: "msgx_1".into(),
            to: "migration".into(),
            text: "hi".into(),
            notify_when_idle: false,
            depth: 0,
            hops: 1,
            from_session: "ses_01JA".into(),
            from_name: Some("build".into()),
        }
    }

    #[test]
    fn peer_bodies_carry_only_labels_of_the_grant_namespace() {
        assert!(message().check().is_ok());
        let bad = [
            PeerMessage {
                id: "msg_1".into(),
                ..message()
            },
            PeerMessage {
                hops: 0,
                ..message()
            },
            PeerMessage {
                from_session: "build".into(),
                ..message()
            },
            PeerMessage {
                from_session: "ses_a@desk".into(),
                ..message()
            },
            PeerMessage {
                from_name: Some("build@workstation".into()),
                ..message()
            },
            PeerMessage {
                from_name: Some("ses_01JB".into()),
                ..message()
            },
            PeerMessage {
                from_name: Some("a\nb".into()),
                ..message()
            },
        ];
        for body in bad {
            assert!(
                matches!(body.check(), Err(RuntimeError::Invalid(_))),
                "{body:?}"
            );
        }
        let unknown = serde_json::json!({"id": "msgx_1", "to": "a", "text": "t", "hops": 1,
            "from_session": "ses_a", "from_machine": "workstation"});
        assert!(serde_json::from_value::<PeerMessage>(unknown).is_err());
    }
}
