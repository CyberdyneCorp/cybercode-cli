//! The send pipeline (`cross-session-messaging` → Send message tool, Delivery semantics,
//! Outcome reporting; design section 5). Every surface sends through [`Messaging::send`]:
//! the text limits, then the address, then — under the target's send lock — the exact
//! retry, the inbound decision, admission with the `in` record in one target commit, and
//! the sender's `out` record after it. Nothing is written before the checks that can
//! refuse the message have passed.

use std::future::Future;
use std::sync::{Arc, PoisonError};
use std::time::Duration;

use cyber_llm::Content;
use cyber_store::NewEvent;
use serde::Serialize;
use sha2::{Digest, Sha256};

use super::Messaging;
use super::address::{self, Target};
use super::inbound::{Reach, SenderAddresses, Verdict};
use super::limits::{self, Limit, LimitKey};
use super::records::{
    self, DELIVERED, Direction, HELD, MessageAccepted, MessageRefused, MessageRow, REFUSED,
};
use super::render::{self, Origin};
use super::watch;
use crate::runtime::events::event;
use crate::runtime::{Admission, Delivery, Runtime, RuntimeError};

/// The label of a client sender that names none (`client:api`).
pub const DEFAULT_CLIENT: &str = "api";

/// The refusal reasons the sender alone records (design section 7).
const CHAIN_LIMIT: &str = "chain_limit";
const RATE_LIMITED: &str = "rate_limited";

/// How long a settling child target gets before the one retry.
pub(super) const BUSY_RETRY: Duration = Duration::from_millis(250);

/// Who sends a message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Sender {
    /// A Session of this server (an agent's `send_message`, or a client sending as it).
    Session(String),
    /// A Messaging API client with its label (`X-Cyber-Client`, else `api`).
    Client(String),
    /// An MCP serve client without a caller Session, by its client label (`mcp:<label>`).
    Mcp(String),
    /// A Session of another machine, named after the grant that authenticated the
    /// request (`cross-session-messaging` → Direct peer transport). `session_id` and
    /// `name` come from the request body: labels inside that grant's namespace, so a
    /// peer can speak only for its own Sessions.
    Peer {
        grant: String,
        session_id: String,
        name: Option<String>,
    },
}

impl Sender {
    /// The sending Session of this server, if any (a peer's Session is not one).
    pub fn session_id(&self) -> Option<&str> {
        match self {
            Self::Session(id) => Some(id),
            Self::Client(_) | Self::Mcp(_) | Self::Peer { .. } => None,
        }
    }

    /// The sender key of the records (design section 7): the Session ID,
    /// `client:<label>`, `mcp:<label>` or `peer:<grant>:<ses_>`. A client's key is also
    /// the `from` its records show.
    pub(super) fn key(&self) -> String {
        match self {
            Self::Session(id) => id.clone(),
            Self::Client(label) => format!("client:{label}"),
            Self::Mcp(label) => format!("mcp:{label}"),
            Self::Peer {
                grant, session_id, ..
            } => format!("peer:{grant}:{session_id}"),
        }
    }

    /// What the rate and refusal limits count as this sender: its [`Self::key`], except
    /// that a peer's Sessions count together under their grant (`peer:<grant>:*`), since
    /// the peer chooses their labels and a new label per message must not escape them.
    pub(super) fn limit_key(&self) -> LimitKey {
        match self {
            Self::Peer { grant, .. } => LimitKey::Prefix(format!("peer:{grant}:")),
            _ => LimitKey::Exact(self.key()),
        }
    }

    /// `origin_kind` of the records: an open set. A message sent through MCP keeps its
    /// Messaging API origin.
    pub(super) fn origin_kind(&self) -> &'static str {
        match self {
            Self::Session(_) => "session",
            Self::Client(_) | Self::Mcp(_) => "client",
            Self::Peer { .. } => "peer",
        }
    }

    /// Where the sender runs, for the inbound default.
    pub(super) fn reach(&self) -> Reach {
        match self {
            Self::Peer { .. } => Reach::CrossMachine,
            _ => Reach::SameMachine,
        }
    }

    /// A peer sender's addresses: `ses_…@<grant>`, then `<name>@<grant>` when the peer
    /// gave a name. The first name is the one the target shows and replies to.
    pub(super) fn peer_addresses(&self) -> Vec<String> {
        let Self::Peer {
            grant,
            session_id,
            name,
        } = self
        else {
            return Vec::new();
        };
        let mut addresses = vec![format!("{session_id}@{grant}")];
        if let Some(name) = name {
            addresses.push(format!("{name}@{grant}"));
        }
        addresses
    }

    /// The grant and Session of a peer sender.
    fn remote(&self) -> (Option<String>, Option<String>) {
        match self {
            Self::Peer {
                grant, session_id, ..
            } => (Some(grant.clone()), Some(session_id.clone())),
            _ => (None, None),
        }
    }
}

/// One message to send.
#[derive(Debug, Clone)]
pub struct SendRequest {
    /// A `msgx_` ID; a repeated ID returns the original result.
    pub id: Option<String>,
    pub from: Sender,
    pub to: String,
    pub text: String,
    /// The sender's MCP recursion depth, carried to the run that reads the message.
    pub depth: u32,
    /// The Session a permission check approved for `to`; an address that resolves to
    /// another Session by the time of the send is a `ConflictError` and nothing is sent.
    pub expected_target: Option<String>,
    /// Also subscribe the sending Session to one notice when the target next goes idle
    /// (step 11); a client has no Session to notify.
    pub notify_when_idle: bool,
}

/// What became of a message.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, serde::Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
#[schemars(rename = "MessageOutcome")]
pub enum Outcome {
    /// Durably admitted to the target inbox (not necessarily read yet).
    Delivered,
    /// Admitted with `delivery: "hold"`; the target's user releases or drops it.
    Held,
    /// Not admitted; `reason` says why.
    Refused,
}

/// The target of a sent message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, schemars::JsonSchema)]
pub struct SentTo {
    /// The address the records show: the name used when it resolved uniquely, else the ID.
    pub address: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
}

/// The synchronous outcome of a send (`v1.messaging.send`, the `send_message` tool).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, schemars::JsonSchema)]
pub struct SendResult {
    pub message_id: String,
    pub outcome: Outcome,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// The limit a `rate_limited` refusal hit (`per_target_minute`, `inbox_pending`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<String>,
    pub to: SentTo,
    /// The idle subscription `notify_when_idle` created.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub watch_id: Option<String>,
    /// The ID was sent before; this is that send's outcome and nothing was written.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub replayed: bool,
}

/// The ID an API send gets from its `Idempotency-Key`: `msgx_` and the first 20 hex
/// characters of the SHA-256 of the principal and the key.
pub fn idempotent_id(principal: &str, key: &str) -> String {
    let digest = Sha256::digest(format!("{principal}\0{key}").as_bytes());
    format!("msgx_{}", &format!("{digest:x}")[..20])
}

/// Whether `id` is a caller-supplied message ID: `msgx_` and 1–120 ASCII letters, digits,
/// `_` or `-`.
pub fn valid_id(id: &str) -> bool {
    id.strip_prefix("msgx_").is_some_and(|rest| {
        (1..=120).contains(&rest.len())
            && rest
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    })
}

/// The Session and peer of an `X@P` address.
pub(super) fn remote_address(to: &str) -> Option<(String, String)> {
    match address::parse(to) {
        Ok(address::Address::Remote { session, peer }) => Some((session, peer)),
        _ => None,
    }
}

/// A caller-supplied message ID must be [`valid_id`].
pub(super) fn check_id(id: Option<&str>) -> Result<(), RuntimeError> {
    match id.filter(|id| !valid_id(id)) {
        Some(id) => Err(RuntimeError::Invalid(format!(
            "message ID {id:?} is not msgx_ followed by 1-120 letters, digits, _ or -"
        ))),
        None => Ok(()),
    }
}

/// Run `attempt`; a `SessionBusyError` (a settling child) is retried once after `pause`,
/// and a second one is `None`.
pub(super) async fn retry_busy<T, F, Fut>(
    pause: Duration,
    mut attempt: F,
) -> Result<Option<T>, RuntimeError>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, RuntimeError>>,
{
    match attempt().await {
        Err(RuntimeError::Busy(_)) => {}
        other => return other.map(Some),
    }
    tokio::time::sleep(pause).await;
    match attempt().await {
        Err(RuntimeError::Busy(_)) => Ok(None),
        other => other.map(Some),
    }
}

impl Runtime {
    /// The send lock of one target; a lock nobody holds or waits for is recreated.
    pub(super) fn message_lock(&self, target: &str) -> Arc<tokio::sync::Mutex<()>> {
        let mut locks = self
            .inner
            .message_locks
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        locks.retain(|_, lock| Arc::strong_count(lock) > 1);
        Arc::clone(locks.entry(target.to_owned()).or_default())
    }

    /// Append one messaging record to a Session's stream.
    pub(super) async fn record_message(
        &self,
        session_id: &str,
        record: NewEvent,
    ) -> Result<(), RuntimeError> {
        let handle = self.inner.handle(session_id).await?;
        self.inner.commit(&handle, vec![record]).await?;
        Ok(())
    }
}

/// One message being sent: the resolved target and the records both sides get.
pub(super) struct Outgoing {
    pub(super) id: String,
    pub(super) sender: Sender,
    pub(super) target: Target,
    /// The sender as the target shows it and replies to.
    pub(super) from: String,
    /// What the target's inbound patterns match for this sender.
    pub(super) addresses: SenderAddresses,
    pub(super) text: String,
    pub(super) depth: u32,
    /// The message's chain hops: the sending run's hops + 1.
    pub(super) hops: u32,
    /// The sender Location's `messaging.max_hops`.
    pub(super) max_hops: u32,
}

impl Outgoing {
    fn result(&self, outcome: Outcome, reason: Option<&str>) -> SendResult {
        SendResult {
            message_id: self.id.clone(),
            outcome,
            reason: reason.map(str::to_owned),
            limit: None,
            to: SentTo {
                address: self.target.address.clone(),
                session_id: Some(self.target.candidate.session_id.clone()),
            },
            watch_id: None,
            replayed: false,
        }
    }

    fn accepted(&self, delivery: Delivery) -> MessageAccepted {
        MessageAccepted {
            message_id: self.id.clone(),
            direction: Direction::In,
            from: self.from.clone(),
            to: self.target.address.clone(),
            peer_session_id: self.sender.session_id().map(str::to_owned),
            origin_kind: self.sender.origin_kind().into(),
            sender_key: self.sender.key(),
            size: u32::try_from(self.text.len()).unwrap_or(u32::MAX),
            sha256: records::text_sha256(&self.text),
            text: self.text.clone(),
            depth: self.depth,
            hops: self.hops,
            delivery,
            remote_peer: self.sender.remote().0,
            remote_session_id: self.sender.remote().1,
        }
    }

    fn refused(&self, direction: Direction, reason: &str) -> MessageRefused {
        let peer = match direction {
            Direction::In => self.sender.session_id().map(str::to_owned),
            Direction::Out => Some(self.target.candidate.session_id.clone()),
        };
        MessageRefused {
            message_id: self.id.clone(),
            direction,
            from: self.from.clone(),
            to: self.target.address.clone(),
            peer_session_id: peer,
            origin_kind: self.sender.origin_kind().into(),
            sender_key: self.sender.key(),
            size: u32::try_from(self.text.len()).unwrap_or(u32::MAX),
            sha256: records::text_sha256(&self.text),
            reason: reason.into(),
            limit: None,
            hops: self.hops,
            remote_peer: self.sender.remote().0,
            remote_session_id: self.sender.remote().1,
        }
    }

    /// The inbox row: one text part holding the untrusted block.
    fn admission(&self, delivery: Delivery) -> Admission {
        let (_, remote_session) = self.sender.remote();
        let origin = Origin {
            from: &self.from,
            session_id: self.sender.session_id().or(remote_session.as_deref()),
            remote: remote_session.is_some(),
        };
        let sent_at = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        Admission {
            skill_command: None,
            template: None,
            message_id: Some(self.id.clone()),
            parts: vec![Content::Text {
                text: render::message_block(&origin, &self.id, &sent_at, &self.text),
            }],
            delivery,
            source: "session".into(),
            resume: true,
        }
    }

    /// Whether a recorded message is this one sent again.
    fn matches(&self, row: &MessageRow) -> bool {
        let target = &self.target.candidate.session_id;
        let same_target = match row.direction {
            Direction::In => &row.session_id == target,
            Direction::Out => row.peer_session_id.as_ref() == Some(target),
        };
        same_target
            && row.sender_key == self.sender.key()
            && row.sha256 == records::text_sha256(&self.text)
    }
}

impl Messaging {
    /// Send a message (design section 5, steps 1–3 and 5–11; the model tool checks
    /// `message.send` between resolution and this call). A refusal is an outcome, not an
    /// error; errors leave nothing written.
    pub async fn send(&self, request: SendRequest) -> Result<SendResult, RuntimeError> {
        let max_hops = self.sender_enabled(&request.from)?.max_hops;
        if let Some((session, peer)) = remote_address(&request.to) {
            return self.send_to_peer(request, max_hops, &session, &peer).await;
        }
        let notify = request.notify_when_idle;
        let outgoing = self.prepare(request, max_hops)?;
        let watch_id = notify.then(|| watch::watch_id_for(&outgoing.id));
        if let Some(watch_id) = &watch_id {
            // A full quota refuses the whole send before anything is written.
            let subscriber = outgoing.sender.session_id().ok_or_else(|| {
                RuntimeError::Invalid("notify_when_idle needs a sending Session (`from`)".into())
            })?;
            self.runtime.check_watch_quota(subscriber, Some(watch_id))?;
        }
        let result = self.send_locked(&outgoing).await?;
        match watch_id {
            Some(watch_id) => self.subscribe(&outgoing, result, watch_id).await,
            None => Ok(result),
        }
    }

    /// Steps 5–10 under the target's send lock.
    pub(super) async fn send_locked(
        &self,
        outgoing: &Outgoing,
    ) -> Result<SendResult, RuntimeError> {
        let lock = self
            .runtime
            .message_lock(&outgoing.target.candidate.session_id);
        let _sending = lock.lock().await;
        if let Some(replayed) = self.replay(outgoing).await? {
            return Ok(replayed);
        }
        match self.decide(outgoing) {
            Verdict::Refuse(reason) => self.refuse_within_limits(outgoing, reason).await,
            Verdict::Hold => self.admit_within_limits(outgoing, Delivery::Hold).await,
            Verdict::Deliver => self.admit_within_limits(outgoing, Delivery::Steer).await,
        }
    }

    /// Step 11: a message that was not refused subscribes its sender to the target's
    /// next idle notice, with a watch ID derived from the message ID so a retried send
    /// finds the same subscription.
    async fn subscribe(
        &self,
        outgoing: &Outgoing,
        mut result: SendResult,
        watch_id: String,
    ) -> Result<SendResult, RuntimeError> {
        let Some(subscriber) = outgoing.sender.session_id() else {
            return Ok(result);
        };
        if result.outcome == Outcome::Refused {
            return Ok(result);
        }
        let target = outgoing.target.candidate.session_id.clone();
        let watch = self
            .watch(watch::WatchRequest {
                id: Some(watch_id),
                subscriber: subscriber.to_owned(),
                target: target.clone(),
                expected_target: Some(target),
                // Notices name the target as the sender addressed it.
                address: Some(outgoing.target.address.clone()),
            })
            .await?;
        result.watch_id = Some(watch.watch_id);
        Ok(result)
    }

    /// Steps 7–8 before the admission: the chain limit, the rate window and the pending
    /// cap. Their refusals are recorded only in the sender, and nothing reaches the target.
    async fn admit_within_limits(
        &self,
        outgoing: &Outgoing,
        delivery: Delivery,
    ) -> Result<SendResult, RuntimeError> {
        if outgoing.hops > self.max_hops(outgoing) {
            return self.refuse_in_sender(outgoing, CHAIN_LIMIT, None).await;
        }
        if let Err(limit) = self.rate(outgoing).await? {
            return self
                .refuse_in_sender(outgoing, RATE_LIMITED, Some(limit))
                .await;
        }
        self.deliver(outgoing, delivery).await
    }

    /// The longest chain both sides allow: the sender's `messaging.max_hops` and the
    /// target Location's.
    fn max_hops(&self, outgoing: &Outgoing) -> u32 {
        let directory = &outgoing.target.candidate.location.directory;
        self.settings(Some(directory))
            .map_or(outgoing.max_hops, |target| {
                target.max_hops.min(outgoing.max_hops)
            })
    }

    /// The rate limits (design section 7): the sender's accepted messages to the
    /// target in the last minute, then the target's unpromoted message rows. An error
    /// sends nothing.
    async fn rate(&self, outgoing: &Outgoing) -> Result<Result<(), Limit>, RuntimeError> {
        let target = outgoing.target.candidate.session_id.clone();
        let key = outgoing.sender.limit_key();
        let now_ms = chrono::Utc::now().timestamp_millis();
        let probe = target.clone();
        let accepted = self
            .runtime
            .inner
            .store
            .read(move |db| Ok(limits::accepted_in_window(db, &probe, &key, now_ms)?))?;
        let handle = self.runtime.inner.handle(&target).await?;
        let pending = {
            let state = handle.state.lock().await;
            state.held_messages() + state.queued_messages()
        };
        Ok(limits::check_rate(accepted, pending))
    }

    /// A refusal only the sender records (`busy`, `chain_limit`, `rate_limited`).
    async fn refuse_in_sender(
        &self,
        outgoing: &Outgoing,
        reason: &str,
        limit: Option<Limit>,
    ) -> Result<SendResult, RuntimeError> {
        let mut record = outgoing.refused(Direction::Out, reason);
        record.limit = limit.map(|limit| limit.as_str().to_owned());
        self.record_sender(outgoing, event(REFUSED, &record))
            .await?;
        let mut result = outgoing.result(Outcome::Refused, Some(reason));
        result.limit = record.limit;
        Ok(result)
    }

    /// Steps 2–3: the text limits, then the address. Nothing is written.
    fn prepare(&self, request: SendRequest, max_hops: u32) -> Result<Outgoing, RuntimeError> {
        limits::check_text(&request.text)?;
        check_id(request.id.as_deref())?;
        let target = self.resolve(request.from.session_id(), &request.to)?;
        if let Some(expected) = request
            .expected_target
            .as_deref()
            .filter(|expected| *expected != target.candidate.session_id)
        {
            return Err(RuntimeError::Conflict(format!(
                "`{}` now names {} instead of the approved {expected}",
                request.to, target.candidate.session_id
            )));
        }
        let from = match &request.from {
            Sender::Session(id) => {
                let (target, sender) = (target.candidate.session_id.clone(), id.clone());
                self.runtime
                    .inner
                    .store
                    .read(move |db| Ok(address::reply_address(db, &target, &sender)?))?
            }
            Sender::Client(_) | Sender::Mcp(_) => request.from.key(),
            Sender::Peer { .. } => {
                return Err(RuntimeError::Invalid(
                    "a peer's message is received through the peer listener".into(),
                ));
            }
        };
        let mut addresses = self.sender_addresses(&request.from)?;
        // The reply address is a name the target sees, not one the sender owns.
        addresses.add_other(from.clone());
        let run_hops = request
            .from
            .session_id()
            .map_or(0, |id| self.runtime.effective_message_hops(id));
        Ok(Outgoing {
            id: request
                .id
                .unwrap_or_else(|| cyber_core::ids::new_id("msgx")),
            sender: request.from,
            target,
            from,
            addresses,
            text: request.text,
            depth: request.depth,
            hops: run_hops.saturating_add(1),
            max_hops,
        })
    }

    /// Step 5: an ID already recorded returns that send's outcome. A sender record a crash
    /// left unwritten is completed first; the same ID with another text, sender or target
    /// is a conflict.
    async fn replay(&self, outgoing: &Outgoing) -> Result<Option<SendResult>, RuntimeError> {
        let (id, sender) = (
            outgoing.id.clone(),
            outgoing.sender.session_id().map(str::to_owned),
        );
        let row = self
            .runtime
            .inner
            .store
            .read(move |db| Ok(records::recorded(db, &id, sender.as_deref())?))?;
        let Some(row) = row else {
            return Ok(None);
        };
        if !outgoing.matches(&row) {
            return Err(RuntimeError::Conflict(format!(
                "message {} was already sent with another text, sender or target",
                outgoing.id
            )));
        }
        self.runtime.complete_sent(Some(&outgoing.id)).await?;
        let outcome = match row.outcome.as_str() {
            "delivered" => Outcome::Delivered,
            "refused" => Outcome::Refused,
            // A held message keeps its outcome after a release or drop.
            _ => Outcome::Held,
        };
        let mut result = outgoing.result(outcome, row.reason.as_deref());
        result.replayed = true;
        Ok(Some(result))
    }

    /// Step 6, the inbound decision of the target (design section 6). An archived target,
    /// or one whose Location disables messaging, never receives.
    fn decide(&self, outgoing: &Outgoing) -> Verdict {
        let target = &outgoing.target;
        let settings = self.settings(Some(&target.candidate.location.directory));
        super::inbound_verdict(
            target,
            settings.as_ref(),
            &outgoing.addresses,
            outgoing.sender.reach(),
        )
    }

    /// A refusal the target records too, unless this sender already had
    /// [`limits::MAX_REFUSED_PER_TARGET_MINUTE`] refusals recorded there in the rate
    /// window: then only the sender records it, as `rate_limited`, so refused senders
    /// cannot grow the target's log without bound.
    async fn refuse_within_limits(
        &self,
        outgoing: &Outgoing,
        reason: &str,
    ) -> Result<SendResult, RuntimeError> {
        let (target, key) = (
            outgoing.target.candidate.session_id.clone(),
            outgoing.sender.limit_key(),
        );
        let now_ms = chrono::Utc::now().timestamp_millis();
        let refused = self
            .runtime
            .inner
            .store
            .read(move |db| Ok(limits::refused_in_window(db, &target, &key, now_ms)?))?;
        if refused >= limits::MAX_REFUSED_PER_TARGET_MINUTE {
            let limit = Some(Limit::PerTargetMinute);
            return self.refuse_in_sender(outgoing, RATE_LIMITED, limit).await;
        }
        self.refuse(outgoing, reason).await
    }

    /// A refusal the target records too: its metadata (never the text) in the target,
    /// then the sender's record.
    async fn refuse(&self, outgoing: &Outgoing, reason: &str) -> Result<SendResult, RuntimeError> {
        let record = event(REFUSED, &outgoing.refused(Direction::In, reason));
        self.runtime
            .record_message(&outgoing.target.candidate.session_id, record)
            .await?;
        self.record_sender(
            outgoing,
            event(REFUSED, &outgoing.refused(Direction::Out, reason)),
        )
        .await?;
        Ok(outgoing.result(Outcome::Refused, Some(reason)))
    }

    /// Steps 9–10: admission and the `in` record in one target commit, then the `out`
    /// record. A settling child target is retried once, then refused with `busy`.
    async fn deliver(
        &self,
        outgoing: &Outgoing,
        delivery: Delivery,
    ) -> Result<SendResult, RuntimeError> {
        let (kind, outcome) = match delivery {
            Delivery::Hold => (HELD, Outcome::Held),
            _ => (DELIVERED, Outcome::Delivered),
        };
        let mut record = outgoing.accepted(delivery);
        let target = outgoing.target.candidate.session_id.clone();
        let admitted = retry_busy(BUSY_RETRY, || {
            self.runtime.admit_message(
                &target,
                outgoing.admission(delivery),
                vec![event(kind, &record)],
            )
        })
        .await?;
        if admitted.is_none() {
            return self.refuse_in_sender(outgoing, "busy", None).await;
        }
        record.direction = Direction::Out;
        record.peer_session_id = Some(target);
        self.record_sender(outgoing, event(kind, &record)).await?;
        Ok(outgoing.result(outcome, None))
    }

    /// The sender's own record; a client has no Session to record in.
    async fn record_sender(
        &self,
        outgoing: &Outgoing,
        record: NewEvent,
    ) -> Result<(), RuntimeError> {
        match outgoing.sender.session_id() {
            Some(sender) => self.runtime.record_message(sender, record).await,
            None => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    #[tokio::test]
    async fn a_busy_target_is_retried_once_then_reported() {
        let calls = AtomicUsize::new(0);
        let busy_once = retry_busy(Duration::ZERO, || {
            let call = calls.fetch_add(1, Ordering::SeqCst);
            async move {
                if call == 0 {
                    Err(RuntimeError::Busy("ses_child".into()))
                } else {
                    Ok(call)
                }
            }
        })
        .await;
        assert_eq!(busy_once.unwrap(), Some(1));
        calls.store(0, Ordering::SeqCst);
        let always_busy: Result<Option<()>, _> = retry_busy(Duration::ZERO, || {
            calls.fetch_add(1, Ordering::SeqCst);
            async { Err(RuntimeError::Busy("ses_child".into())) }
        })
        .await;
        assert_eq!(always_busy.unwrap(), None);
        assert_eq!(calls.load(Ordering::SeqCst), 2, "one retry, no more");
        let other: Result<Option<()>, _> = retry_busy(Duration::ZERO, || async {
            Err(RuntimeError::Invalid("no".into()))
        })
        .await;
        assert!(matches!(other, Err(RuntimeError::Invalid(_))));
    }

    #[test]
    fn message_ids_are_validated_and_derived_deterministically() {
        assert!(valid_id("msgx_01JABC-retry_2"));
        for bad in [
            "msgx_",
            "msg_1",
            "msgx_a b",
            "msgx_é",
            &format!("msgx_{}", "a".repeat(121)),
        ] {
            assert!(!valid_id(bad), "{bad}");
        }
        let id = idempotent_id("cyber", "key-1");
        assert_eq!(id, idempotent_id("cyber", "key-1"));
        assert_ne!(id, idempotent_id("cyber", "key-2"));
        assert_eq!(id.len(), "msgx_".len() + 20);
        assert!(valid_id(&id));
    }
}
