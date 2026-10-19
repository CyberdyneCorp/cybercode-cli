//! The sending side of the direct peer transport (`cross-session-messaging` → Direct peer
//! transport; design section 5): `send_message({ to: "X@P" })`.
//!
//! The size, disable switch, hops and the caller's `message.send` check run here, before
//! anything leaves the machine; `P` must be an enabled `peers.remotes` entry
//! (`PeerNotConfiguredError` before any network request). The peer client then posts the
//! message to `P`, which runs its own pipeline. The sender's `out` record keeps the
//! synchronous outcome with the peer's name and the Session there; a network, TLS or
//! guard failure records only outcome `failed` and fails the send
//! (`PeerUnreachableError`, `PeerAuthError`), nothing is queued, and a retry with the
//! same ID sends again, which the peer answers with its first outcome.

use cyber_store::NewEvent;
use serde_json::Value;

use super::address::{self, AddressError, Candidate, Target};
use super::peer::{PeerClient, PeerTarget};
use super::receive::{PeerMessage, PeerSendResult};
use super::records::{
    self, DELIVERED, Direction, FAILED, HELD, MessageAccepted, MessageRefused, MessageRow, REFUSED,
};
use super::send::{Outcome, SendRequest, SendResult, SentTo, check_id};
use super::{Messaging, PeerError, RuntimeError, limits};
use crate::runtime::Delivery;
use crate::runtime::events::event;

/// The sender records a chain-limit refusal with this reason, as for a local target.
const CHAIN_LIMIT: &str = "chain_limit";

/// One message on its way to a peer.
struct Outbound {
    /// The sending Session of this server.
    sender: String,
    /// The remote's name in `peers.remotes`.
    peer: String,
    /// `X@P`, as records and results show it.
    address: String,
    message: PeerMessage,
}

impl Outbound {
    fn result(
        &self,
        outcome: Outcome,
        reason: Option<String>,
        session: Option<String>,
    ) -> SendResult {
        SendResult {
            message_id: self.message.id.clone(),
            outcome,
            reason,
            limit: None,
            to: SentTo {
                address: self.address.clone(),
                session_id: session,
            },
            watch_id: None,
            replayed: false,
        }
    }

    fn size(&self) -> u32 {
        u32::try_from(self.message.text.len()).unwrap_or(u32::MAX)
    }

    /// The `out` record of an accepted message.
    fn accepted(&self, delivery: Delivery, session: &str) -> MessageAccepted {
        MessageAccepted {
            message_id: self.message.id.clone(),
            direction: Direction::Out,
            from: self.sender.clone(),
            to: self.address.clone(),
            peer_session_id: None,
            origin_kind: "peer".into(),
            sender_key: self.sender.clone(),
            size: self.size(),
            sha256: records::text_sha256(&self.message.text),
            text: self.message.text.clone(),
            depth: self.message.depth,
            hops: self.message.hops,
            delivery,
            remote_peer: Some(self.peer.clone()),
            remote_session_id: Some(session.to_owned()),
        }
    }

    /// The `out` record of a refused or failed message: metadata only.
    fn refused(&self, reason: &str, session: Option<String>) -> MessageRefused {
        MessageRefused {
            message_id: self.message.id.clone(),
            direction: Direction::Out,
            from: self.sender.clone(),
            to: self.address.clone(),
            peer_session_id: None,
            origin_kind: "peer".into(),
            sender_key: self.sender.clone(),
            size: self.size(),
            sha256: records::text_sha256(&self.message.text),
            reason: reason.into(),
            limit: None,
            hops: self.message.hops,
            remote_peer: Some(self.peer.clone()),
            remote_session_id: session,
        }
    }

    /// Whether a recorded message is this one sent again.
    fn matches(&self, row: &MessageRow) -> bool {
        row.direction == Direction::Out
            && row.remote_peer.as_deref() == Some(self.peer.as_str())
            && row.peer_address == self.address
            && row.sender_key == self.sender
            && row.sha256 == records::text_sha256(&self.message.text)
    }
}

/// The outcome a recorded `out` row replays.
fn recorded_outcome(outcome: &str) -> Outcome {
    match outcome {
        "delivered" => Outcome::Delivered,
        "refused" => Outcome::Refused,
        // A held message keeps its outcome after a release or drop.
        _ => Outcome::Held,
    }
}

/// The reason a failed send records.
fn failure_reason(error: &PeerError) -> &'static str {
    match error {
        PeerError::Auth { reason, .. } => reason.as_str(),
        _ => "unreachable",
    }
}

impl Messaging {
    /// The enabled remote `peer` with its token; `PeerNotConfiguredError` without one.
    pub(super) fn peer_target(&self, peer: &str) -> Result<PeerTarget, RuntimeError> {
        match self.runtime.inner.tools.peer_target(peer) {
            Ok(Some(target)) => Ok(target),
            Ok(None) => Err(RuntimeError::Peer(PeerError::NotConfigured {
                peer: peer.to_owned(),
            })),
            Err(error) => Err(RuntimeError::Invalid(format!("peer `{peer}`: {error}"))),
        }
    }

    /// Send `request` to Session `session` of peer `peer` (steps 1–2 of design section 5
    /// here, the rest on the peer).
    pub(super) async fn send_to_peer(
        &self,
        request: SendRequest,
        max_hops: u32,
        session: &str,
        peer: &str,
    ) -> Result<SendResult, RuntimeError> {
        limits::check_text(&request.text)?;
        check_id(request.id.as_deref())?;
        let target = self.peer_target(peer)?;
        let outbound = self.outbound(request, session, peer)?;
        let lock = self.runtime.message_lock(&outbound.address);
        let _sending = lock.lock().await;
        if let Some(replayed) = self.replay_peer(&outbound)? {
            return Ok(replayed);
        }
        if outbound.message.hops > max_hops {
            let record = event(REFUSED, &outbound.refused(CHAIN_LIMIT, None));
            self.runtime
                .record_message(&outbound.sender, record)
                .await?;
            return Ok(outbound.result(Outcome::Refused, Some(CHAIN_LIMIT.into()), None));
        }
        let posted = post(target, &outbound.message).await;
        self.record_answer(&outbound, posted).await
    }

    /// The message as the peer receives it. Only a Session of this server sends to a
    /// peer: the peer names the sender after one of its Sessions.
    fn outbound(
        &self,
        request: SendRequest,
        session: &str,
        peer: &str,
    ) -> Result<Outbound, RuntimeError> {
        let address = format!("{session}@{peer}");
        if let Some(expected) = request
            .expected_target
            .as_deref()
            .filter(|expected| *expected != address)
        {
            return Err(RuntimeError::Conflict(format!(
                "`{}` now names {address} instead of the approved {expected}",
                request.to
            )));
        }
        let Some(sender) = request.from.session_id().map(str::to_owned) else {
            return Err(RuntimeError::Invalid(
                "a message to a peer Session needs a sending Session (`from`; \
                 `--from <session>` with `cyber messages send`)"
                    .into(),
            ));
        };
        if request.notify_when_idle {
            return Err(RuntimeError::Invalid(
                "notify_when_idle is not available for peer Sessions".into(),
            ));
        }
        self.caller_exists(Some(&sender))?;
        let probe = sender.clone();
        let names = self
            .runtime
            .inner
            .store
            .read(move |db| Ok(address::sender_addresses(db, &probe)?))?;
        // Its unique root title, the one name no other Session here can claim.
        let from_name = names.trusted.get(1).cloned();
        let hops = self
            .runtime
            .effective_message_hops(&sender)
            .saturating_add(1);
        Ok(Outbound {
            message: PeerMessage {
                id: request
                    .id
                    .unwrap_or_else(|| cyber_core::ids::new_id("msgx")),
                to: session.to_owned(),
                text: request.text,
                notify_when_idle: false,
                depth: request.depth,
                hops,
                from_session: sender.clone(),
                from_name,
            },
            sender,
            peer: peer.to_owned(),
            address,
        })
    }

    /// An ID this Session already sent returns that send's outcome without a request; a
    /// send that failed is sent again.
    fn replay_peer(&self, outbound: &Outbound) -> Result<Option<SendResult>, RuntimeError> {
        let (id, sender) = (outbound.message.id.clone(), outbound.sender.clone());
        let row = self
            .runtime
            .inner
            .store
            .read(move |db| Ok(records::recorded(db, &id, Some(&sender))?))?;
        let Some(row) = row else {
            return Ok(None);
        };
        if !outbound.matches(&row) {
            return Err(RuntimeError::Conflict(format!(
                "message {} was already sent with another text, sender or target",
                outbound.message.id
            )));
        }
        if row.outcome == "failed" {
            return Ok(None);
        }
        let outcome = recorded_outcome(&row.outcome);
        let mut result = outbound.result(outcome, row.reason, row.remote_session_id);
        result.replayed = true;
        Ok(Some(result))
    }

    /// Record the peer's answer in the sender and return it.
    async fn record_answer(
        &self,
        outbound: &Outbound,
        posted: Result<(u16, Value), PeerError>,
    ) -> Result<SendResult, RuntimeError> {
        let body = match posted {
            Ok((200, body)) => body,
            Ok((status, body)) => return Err(remote_error(outbound, status, &body)),
            Err(error) => {
                let record = outbound.refused(failure_reason(&error), None);
                self.record_out(outbound, event(FAILED, &record)).await?;
                return Err(RuntimeError::Peer(error));
            }
        };
        let answer: PeerSendResult = serde_json::from_value(body).map_err(|_| {
            RuntimeError::Peer(PeerError::Unreachable {
                peer: outbound.peer.clone(),
                cause: "the peer answered an unexpected body".into(),
            })
        })?;
        let session = answer.target_session_id.clone();
        let record = match answer.outcome {
            Outcome::Delivered => event(DELIVERED, &outbound.accepted(Delivery::Steer, &session)),
            Outcome::Held => event(HELD, &outbound.accepted(Delivery::Hold, &session)),
            Outcome::Refused => {
                let mut refused = outbound.refused(
                    answer.reason.as_deref().unwrap_or("refused"),
                    Some(session.clone()),
                );
                refused.limit = answer.limit.clone();
                event(REFUSED, &refused)
            }
        };
        self.record_out(outbound, record).await?;
        let mut result = outbound.result(answer.outcome, answer.reason, Some(session));
        result.limit = answer.limit;
        result.replayed = answer.replayed;
        Ok(result)
    }

    async fn record_out(&self, outbound: &Outbound, record: NewEvent) -> Result<(), RuntimeError> {
        self.runtime.record_message(&outbound.sender, record).await
    }
}

/// Post `message` to the peer; a client that cannot be built is unreachable.
async fn post(target: PeerTarget, message: &PeerMessage) -> Result<(u16, Value), PeerError> {
    let client = PeerClient::new(target)?;
    let body = serde_json::to_value(message).unwrap_or_default();
    client
        .request(reqwest::Method::POST, "/messages", Some(&body))
        .await
}

/// The error a peer answered (a `4xx` the peer client does not classify), as the same
/// error here with the address the sender used. Nothing was written on either side.
fn remote_error(outbound: &Outbound, status: u16, body: &Value) -> RuntimeError {
    let message = body["message"].as_str().unwrap_or_default();
    let address = outbound.address.clone();
    match body["_tag"].as_str().unwrap_or_default() {
        "AddressNotFoundError" => RuntimeError::Address(AddressError::NotFound(address)),
        "AmbiguousAddressError" => RuntimeError::Address(AddressError::Ambiguous {
            candidates: remote_candidates(&outbound.peer, &body["candidates"]),
            address,
        }),
        "MessageTooLargeError" => RuntimeError::MessageTooLarge {
            size: outbound.message.text.len(),
            limit: limits::MAX_MESSAGE_BYTES,
        },
        "ServiceUnavailableError" => RuntimeError::Disabled("messaging on the peer"),
        "ConflictError" => RuntimeError::Conflict(format!("peer `{}`: {message}", outbound.peer)),
        _ => RuntimeError::Invalid(format!(
            "peer `{}` refused the message (HTTP {status}): {message}",
            outbound.peer
        )),
    }
}

/// The peer's candidates, addressed from here (`ses_…@<peer>`).
fn remote_candidates(peer: &str, candidates: &Value) -> Vec<Candidate> {
    let parsed: Vec<Candidate> = serde_json::from_value(candidates.clone()).unwrap_or_default();
    parsed
        .into_iter()
        .map(|candidate| {
            let remote = Target::remote(&candidate.session_id, peer).candidate;
            Candidate {
                name: candidate.name,
                location: candidate.location,
                ..remote
            }
        })
        .collect()
}
