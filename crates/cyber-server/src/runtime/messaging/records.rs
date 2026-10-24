//! Durable messaging records (`cross-session-messaging` → Transcript and audit): the
//! `messaging.*` events of the Session streams, their projector into `xsession_message`
//! and `messaging_watch`, and the Session state fold that promotion and the inbox badge
//! read. Every record is validated on append; a refused record never carries text.

use std::collections::BTreeMap;

use rusqlite::{Transaction, params};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use cyber_store::{EventRegistry, StoredEvent};

use crate::runtime::Delivery;

pub const DELIVERED: &str = "messaging.message.delivered.1";
pub const HELD: &str = "messaging.message.held.1";
pub const REFUSED: &str = "messaging.message.refused.1";
/// A send to a peer that failed on the network or was refused by the peer's TLS or guard
/// (`PeerUnreachableError`, `PeerAuthError`): the sender's `out` record only, outcome
/// `failed`. A retry with the same ID replaces it.
pub const FAILED: &str = "messaging.message.failed.1";
pub const RELEASED: &str = "messaging.message.released.1";
pub const DROPPED: &str = "messaging.message.dropped.1";
/// A message a peer still held 7 days after it was sent: the sender's `out` record only,
/// outcome `expired`, after which the outcome tracker stops polling the peer.
pub const EXPIRED: &str = "messaging.message.expired.1";
pub const WATCH_CREATED: &str = "messaging.watch.created.1";
pub const WATCH_FIRED: &str = "messaging.watch.fired.1";
pub const WATCH_EXPIRED: &str = "messaging.watch.expired.1";
pub const CHAIN_INHERITED: &str = "messaging.chain.inherited.1";

/// The projection tables, in the order a rebuild clears them.
pub const TABLES: [&str; 2] = ["xsession_message", "messaging_watch"];
/// The event type prefix every messaging record shares.
pub const PREFIX: &str = "messaging.";

/// The Session side of a message: `in` in the target, `out` in the sender.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Direction {
    In,
    Out,
}

impl Direction {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::In => "in",
            Self::Out => "out",
        }
    }
}

/// `messaging.message.delivered.1` and `messaging.message.held.1`: an accepted message.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MessageAccepted {
    pub message_id: String,
    pub direction: Direction,
    pub from: String,
    pub to: String,
    /// The other side's local Session, when it has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub peer_session_id: Option<String>,
    /// `session`, `client` or `peer`; an open set.
    pub origin_kind: String,
    pub sender_key: String,
    pub size: u32,
    pub sha256: String,
    pub text: String,
    pub depth: u32,
    pub hops: u32,
    pub delivery: Delivery,
    /// The peer of a message between machines: the remote's name on the sender, the
    /// grant's name on the target.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote_peer: Option<String>,
    /// The Session on the other machine.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote_session_id: Option<String>,
}

/// `messaging.message.refused.1`: metadata only; the text is never stored.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MessageRefused {
    pub message_id: String,
    pub direction: Direction,
    pub from: String,
    pub to: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub peer_session_id: Option<String>,
    pub origin_kind: String,
    pub sender_key: String,
    pub size: u32,
    pub sha256: String,
    pub reason: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<String>,
    /// The chain hops the message would have carried (a `chain_limit` refusal shows the
    /// hop it stopped at).
    #[serde(default = "first_hop")]
    pub hops: u32,
    /// The peer of a message between machines: the remote's name on the sender, the
    /// grant's name on the target.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote_peer: Option<String>,
    /// The Session on the other machine.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote_session_id: Option<String>,
}

fn first_hop() -> u32 {
    1
}

/// `messaging.message.released.1`: a held message promoted with `delivery`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MessageReleased {
    pub message_id: String,
    pub direction: Direction,
    pub delivery: Delivery,
}

/// `messaging.message.dropped.1`: a held message removed unread.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MessageDropped {
    pub message_id: String,
    pub direction: Direction,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// `messaging.message.expired.1`: the sender stopped following a message a peer holds.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MessageExpired {
    pub message_id: String,
    pub direction: Direction,
}

/// `messaging.watch.created.1`, in the subscriber.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WatchCreated {
    pub watch_id: String,
    /// The target's address as the subscriber gave it.
    pub target: String,
    pub target_id: String,
    pub expires_ms: i64,
    /// The peer of a watch on a peer Session (`X@P`); its `target_id` is `<ses_>@P`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_peer: Option<String>,
}

/// `messaging.watch.fired.1`, after the notice `notice_id` was admitted.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WatchFired {
    pub watch_id: String,
    pub notice_id: String,
    pub status: String,
    pub finished_at: String,
    /// The chain hops the notice carries into the run that promotes it.
    #[serde(default)]
    pub hops: u32,
}

/// `messaging.watch.expired.1`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WatchExpired {
    pub watch_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// The expiry notice admitted with this record, if one was.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notice_id: Option<String>,
}

/// `messaging.chain.inherited.1`, in the Session that will promote input `message_id`:
/// input the model caused (a background Job's handback or Monitor line, a subagent's
/// prompt) carries the hops of the run that caused it, so only user input restarts a
/// message chain at 0 (`cross-session-messaging` → Message chain limit).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChainInherited {
    pub message_id: String,
    pub hops: u32,
}

/// What promotion of a received message reads: the sender's MCP depth and chain hops.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MessageChain {
    pub depth: u32,
    pub hops: u32,
}

pub(crate) fn register(registry: &mut EventRegistry) {
    registry
        .register_with(DELIVERED, validate_accepted)
        .and_then(|r| r.register_with(HELD, validate_accepted))
        .and_then(|r| r.register_with(REFUSED, validate_refused))
        .and_then(|r| r.register_with(FAILED, validate_failed))
        .and_then(|r| {
            r.register_with(RELEASED, |v| {
                message_id(&decode::<MessageReleased>(v)?.message_id)
            })
        })
        .and_then(|r| {
            r.register_with(DROPPED, |v| {
                message_id(&decode::<MessageDropped>(v)?.message_id)
            })
        })
        .and_then(|r| r.register_with(EXPIRED, validate_expired))
        .and_then(|r| r.register_with(WATCH_CREATED, |v| decode::<WatchCreated>(v).map(drop)))
        .and_then(|r| r.register_with(WATCH_FIRED, |v| decode::<WatchFired>(v).map(drop)))
        .and_then(|r| r.register_with(WATCH_EXPIRED, |v| decode::<WatchExpired>(v).map(drop)))
        .and_then(|r| {
            r.register_with(CHAIN_INHERITED, |v| {
                let inherited: ChainInherited = decode(v)?;
                if inherited.message_id.is_empty() || inherited.hops == 0 {
                    return Err("an inherited chain names its input and at least one hop".into());
                }
                Ok(())
            })
        })
        .expect("valid messaging events");
}

fn decode<T: DeserializeOwned>(data: &Value) -> Result<T, String> {
    serde_json::from_value(data.clone()).map_err(|error| error.to_string())
}

fn message_id(id: &str) -> Result<(), String> {
    if id.len() > "msgx_".len() && id.starts_with("msgx_") {
        Ok(())
    } else {
        Err(format!("message ID {id:?} is not a msgx_ ID"))
    }
}

fn digest(sha256: &str) -> Result<(), String> {
    if sha256.len() == 64
        && sha256
            .bytes()
            .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
    {
        Ok(())
    } else {
        Err("sha256 must be 64 lowercase hex characters".into())
    }
}

/// The SHA-256 of a message text, as records carry it.
pub fn text_sha256(text: &str) -> String {
    format!("{:x}", Sha256::digest(text.as_bytes()))
}

fn validate_accepted(data: &Value) -> Result<(), String> {
    let accepted: MessageAccepted = decode(data)?;
    message_id(&accepted.message_id)?;
    if accepted.origin_kind.is_empty() || accepted.sender_key.is_empty() {
        return Err("origin_kind and sender_key are required".into());
    }
    if usize::try_from(accepted.size).ok() != Some(accepted.text.len()) {
        return Err("size differs from the text's UTF-8 length".into());
    }
    if accepted.sha256 != text_sha256(&accepted.text) {
        return Err("sha256 differs from the text".into());
    }
    if accepted.origin_kind == "collaboration" {
        let origin = super::CollaborationOrigin::from_key(&accepted.sender_key)
            .ok_or("collaboration provenance is invalid")?;
        if accepted.delivery != Delivery::Hold
            || accepted.peer_session_id.is_some()
            || accepted.remote_peer.is_some()
            || accepted.remote_session_id.is_some()
            || accepted.depth != origin.depth
            || accepted.hops != origin.hops
        {
            return Err("collaboration admission must be held with matching provenance and no remote Session".into());
        }
    }
    if accepted.delivery == Delivery::Queue {
        return Err("a message is delivered with steer or hold".into());
    }
    Ok(())
}

fn validate_refused(data: &Value) -> Result<(), String> {
    if data.get("text").is_some() {
        return Err("a refused message stores no text".into());
    }
    let refused: MessageRefused = decode(data)?;
    message_id(&refused.message_id)?;
    digest(&refused.sha256)?;
    if refused.origin_kind == "collaboration"
        && (super::CollaborationOrigin::from_key(&refused.sender_key).is_none()
            || refused.peer_session_id.is_some()
            || refused.remote_peer.is_some()
            || refused.remote_session_id.is_some())
    {
        return Err("collaboration refusal has invalid provenance".into());
    }
    if refused.reason.is_empty() || refused.sender_key.is_empty() {
        return Err("reason and sender_key are required".into());
    }
    Ok(())
}

fn validate_failed(data: &Value) -> Result<(), String> {
    validate_refused(data)?;
    let failed: MessageRefused = decode(data)?;
    if failed.direction != Direction::Out || failed.remote_peer.is_none() {
        return Err("a failed send is the sender's record of a message to a peer".into());
    }
    Ok(())
}

fn validate_expired(data: &Value) -> Result<(), String> {
    let expired: MessageExpired = decode(data)?;
    message_id(&expired.message_id)?;
    if expired.direction != Direction::Out {
        return Err("an expiry is the sender's record of a message to a peer".into());
    }
    Ok(())
}

/// Write `xsession_message` and `messaging_watch` in the event transaction.
pub(crate) fn project(tx: &Transaction<'_>, e: &StoredEvent) -> Result<(), String> {
    if !e.kind.starts_with(PREFIX) {
        return Ok(());
    }
    match e.kind.as_str() {
        DELIVERED => insert_accepted(tx, e, "delivered"),
        HELD => insert_accepted(tx, e, "held"),
        REFUSED => insert_refused(tx, e, "refused"),
        FAILED => insert_refused(tx, e, "failed"),
        RELEASED => {
            let released: MessageReleased = decode(&e.data)?;
            update_outcome(
                tx,
                e,
                &released.message_id,
                released.direction,
                "released",
                None,
            )
        }
        DROPPED => {
            let dropped: MessageDropped = decode(&e.data)?;
            let reason = dropped.reason.as_deref();
            update_outcome(
                tx,
                e,
                &dropped.message_id,
                dropped.direction,
                "dropped",
                reason,
            )
        }
        EXPIRED => {
            let expired: MessageExpired = decode(&e.data)?;
            update_outcome(
                tx,
                e,
                &expired.message_id,
                expired.direction,
                "expired",
                None,
            )
        }
        _ => project_watch(tx, e),
    }
}

fn sql(error: rusqlite::Error) -> String {
    error.to_string()
}

/// The other side's address as this side shows it.
fn peer(direction: Direction, from: &str, to: &str) -> String {
    match direction {
        Direction::In => from.to_string(),
        Direction::Out => to.to_string(),
    }
}

fn insert_accepted(tx: &Transaction<'_>, e: &StoredEvent, outcome: &str) -> Result<(), String> {
    let m: MessageAccepted = decode(&e.data)?;
    // The sender follows a message a peer holds (`super::tracker`).
    let next_poll = (outcome == "held" && m.direction == Direction::Out && m.remote_peer.is_some())
        .then(|| super::tracker::first_poll_ms(e.time_ms));
    let changed = tx
        .execute(
            &format!(
                "INSERT INTO xsession_message (session_id, message_id, direction, peer_address, peer_session_id,
                     origin_kind, sender_key, outcome, size, sha256, text, depth, hops, created_ms, updated_ms, seq,
                     remote_peer, remote_session_id, next_poll_ms)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?14, ?15, ?16, ?17, ?18)
                 {RETRY_FAILED}"
            ),
            params![
                e.aggregate_id,
                m.message_id,
                m.direction.as_str(),
                peer(m.direction, &m.from, &m.to),
                m.peer_session_id,
                m.origin_kind,
                m.sender_key,
                outcome,
                m.size,
                m.sha256,
                m.text,
                m.depth,
                m.hops,
                e.time_ms,
                e.seq,
                m.remote_peer,
                m.remote_session_id,
                next_poll,
            ],
        )
        .map_err(sql)?;
    inserted(changed, &m.message_id)
}

/// A record of a message whose earlier send failed (outcome `failed`) replaces that
/// record; any other existing record of the message on this side is a defect.
const RETRY_FAILED: &str = "ON CONFLICT (session_id, message_id, direction) DO UPDATE SET
     peer_address = excluded.peer_address, peer_session_id = excluded.peer_session_id,
     origin_kind = excluded.origin_kind, sender_key = excluded.sender_key,
     outcome = excluded.outcome, reason = excluded.reason, size = excluded.size,
     sha256 = excluded.sha256, text = excluded.text, depth = excluded.depth,
     hops = excluded.hops, updated_ms = excluded.updated_ms, seq = excluded.seq,
     remote_peer = excluded.remote_peer, remote_session_id = excluded.remote_session_id,
     next_poll_ms = excluded.next_poll_ms
     WHERE xsession_message.outcome = 'failed'";

fn inserted(changed: usize, message_id: &str) -> Result<(), String> {
    if changed == 1 {
        Ok(())
    } else {
        Err(format!(
            "message {message_id} is already recorded on this side"
        ))
    }
}

fn insert_refused(tx: &Transaction<'_>, e: &StoredEvent, outcome: &str) -> Result<(), String> {
    let m: MessageRefused = decode(&e.data)?;
    let changed = tx
        .execute(
            &format!(
                "INSERT INTO xsession_message (session_id, message_id, direction, peer_address, peer_session_id,
                     origin_kind, sender_key, outcome, reason, size, sha256, hops, created_ms, updated_ms, seq,
                     remote_peer, remote_session_id)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?13, ?14, ?15, ?16)
                 {RETRY_FAILED}"
            ),
            params![
                e.aggregate_id,
                m.message_id,
                m.direction.as_str(),
                peer(m.direction, &m.from, &m.to),
                m.peer_session_id,
                m.origin_kind,
                m.sender_key,
                outcome,
                m.reason,
                m.size,
                m.sha256,
                m.hops,
                e.time_ms,
                e.seq,
                m.remote_peer,
                m.remote_session_id,
            ],
        )
        .map_err(sql)?;
    inserted(changed, &m.message_id)
}

/// Release, drop and expiry change a recorded message, which then needs no more polls;
/// one without a record is a defect.
fn update_outcome(
    tx: &Transaction<'_>,
    e: &StoredEvent,
    message_id: &str,
    direction: Direction,
    outcome: &str,
    reason: Option<&str>,
) -> Result<(), String> {
    let changed = tx
        .execute(
            "UPDATE xsession_message SET outcome = ?4, reason = COALESCE(?5, reason), updated_ms = ?6, seq = ?7,
                 next_poll_ms = NULL
             WHERE session_id = ?1 AND message_id = ?2 AND direction = ?3",
            params![
                e.aggregate_id,
                message_id,
                direction.as_str(),
                outcome,
                reason,
                e.time_ms,
                e.seq
            ],
        )
        .map_err(sql)?;
    if changed == 1 {
        Ok(())
    } else {
        Err(format!(
            "message {message_id} has no {} record",
            direction.as_str()
        ))
    }
}

fn project_watch(tx: &Transaction<'_>, e: &StoredEvent) -> Result<(), String> {
    let changed = match e.kind.as_str() {
        WATCH_CREATED => {
            let w: WatchCreated = decode(&e.data)?;
            tx.execute(
                "INSERT INTO messaging_watch (id, subscriber_id, target_address, target_id, status, created_ms, expires_ms, target_peer)
                 VALUES (?1, ?2, ?3, ?4, 'active', ?5, ?6, ?7)",
                params![w.watch_id, e.aggregate_id, w.target, w.target_id, e.time_ms, w.expires_ms, w.target_peer],
            )
        }
        WATCH_FIRED => {
            let w: WatchFired = decode(&e.data)?;
            tx.execute(
                "UPDATE messaging_watch SET status = 'fired', fired_ms = ?3, notice_id = ?4
                 WHERE id = ?1 AND subscriber_id = ?2 AND status = 'active'",
                params![w.watch_id, e.aggregate_id, e.time_ms, w.notice_id],
            )
        }
        WATCH_EXPIRED => {
            let w: WatchExpired = decode(&e.data)?;
            tx.execute(
                "UPDATE messaging_watch SET status = 'expired', notice_id = ?3
                 WHERE id = ?1 AND subscriber_id = ?2 AND status = 'active'",
                params![w.watch_id, e.aggregate_id, w.notice_id],
            )
        }
        _ => return Ok(()),
    }
    .map_err(sql)?;
    if changed == 1 {
        Ok(())
    } else {
        Err(format!("{} names no active watch of this Session", e.kind))
    }
}

/// Fold a messaging event into a Session's `message_id → (depth, hops)` map, where
/// received messages enter and dropped ones leave, and its `notice_id → hops` map of fired
/// idle notices and inherited chains.
pub(crate) fn fold(
    chains: &mut BTreeMap<String, MessageChain>,
    notices: &mut BTreeMap<String, u32>,
    e: &StoredEvent,
) -> Result<(), String> {
    match e.kind.as_str() {
        DELIVERED | HELD => {
            let m: MessageAccepted = decode(&e.data)?;
            if m.direction == Direction::In {
                let chain = MessageChain {
                    depth: m.depth,
                    hops: m.hops,
                };
                chains.insert(m.message_id, chain);
            }
        }
        DROPPED => {
            let m: MessageDropped = decode(&e.data)?;
            if m.direction == Direction::In {
                chains.remove(&m.message_id);
            }
        }
        WATCH_FIRED => {
            let fired: WatchFired = decode(&e.data)?;
            notices.insert(fired.notice_id, fired.hops);
        }
        CHAIN_INHERITED => {
            let inherited: ChainInherited = decode(&e.data)?;
            notices.insert(inherited.message_id, inherited.hops);
        }
        _ => {}
    }
    Ok(())
}

/// One `xsession_message` row.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MessageRow {
    pub session_id: String,
    pub message_id: String,
    pub direction: Direction,
    pub peer_address: String,
    pub peer_session_id: Option<String>,
    pub origin_kind: String,
    pub sender_key: String,
    pub outcome: String,
    pub reason: Option<String>,
    pub size: u32,
    pub sha256: String,
    pub text: Option<String>,
    pub depth: u32,
    pub hops: u32,
    pub created_ms: i64,
    pub updated_ms: i64,
    pub seq: i64,
    /// The peer of a message between machines (the remote's name on the sender, the
    /// grant's on the target).
    pub remote_peer: Option<String>,
    /// The Session on the other machine.
    pub remote_session_id: Option<String>,
}

/// One `messaging_watch` row.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct WatchRow {
    pub id: String,
    pub subscriber_id: String,
    pub target_address: String,
    pub target_id: String,
    pub status: String,
    pub created_ms: i64,
    pub expires_ms: i64,
    pub fired_ms: Option<i64>,
    pub notice_id: Option<String>,
    /// The peer of a watch on a peer Session.
    pub target_peer: Option<String>,
}

pub(crate) const MESSAGE_COLUMNS: &str =
    "session_id, message_id, direction, peer_address, peer_session_id, origin_kind, sender_key,
     outcome, reason, size, sha256, text, depth, hops, created_ms, updated_ms, seq,
     remote_peer, remote_session_id";

pub(crate) fn message_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<MessageRow> {
    let direction: String = r.get(2)?;
    Ok(MessageRow {
        session_id: r.get(0)?,
        message_id: r.get(1)?,
        direction: if direction == "in" {
            Direction::In
        } else {
            Direction::Out
        },
        peer_address: r.get(3)?,
        peer_session_id: r.get(4)?,
        origin_kind: r.get(5)?,
        sender_key: r.get(6)?,
        outcome: r.get(7)?,
        reason: r.get(8)?,
        size: r.get(9)?,
        sha256: r.get(10)?,
        text: r.get(11)?,
        depth: r.get(12)?,
        hops: r.get(13)?,
        created_ms: r.get(14)?,
        updated_ms: r.get(15)?,
        seq: r.get(16)?,
        remote_peer: r.get(17)?,
        remote_session_id: r.get(18)?,
    })
}

/// The message rows of a Session, in record order.
pub(crate) fn message_rows(
    db: &rusqlite::Connection,
    session_id: &str,
) -> rusqlite::Result<Vec<MessageRow>> {
    let mut stmt = db.prepare(&format!(
        "SELECT {MESSAGE_COLUMNS} FROM xsession_message WHERE session_id = ?1 ORDER BY seq, direction"
    ))?;
    stmt.query_map([session_id], message_row)?.collect()
}

/// The first record of `message_id`: its `in` record in any Session, else the `out`
/// record of `sender` (a message refused before it reached a target has only that).
pub(crate) fn recorded(
    db: &rusqlite::Connection,
    message_id: &str,
    sender: Option<&str>,
) -> rusqlite::Result<Option<MessageRow>> {
    use rusqlite::OptionalExtension;
    db.query_row(
        &format!(
            "SELECT {MESSAGE_COLUMNS} FROM xsession_message
             WHERE message_id = ?1 AND (direction = 'in' OR session_id IS ?2)
             ORDER BY direction = 'in' DESC LIMIT 1"
        ),
        params![message_id, sender],
        message_row,
    )
    .optional()
}

/// A target's `in` record whose sender has no `out` record yet.
#[derive(Debug, Clone)]
pub(crate) struct Unrecorded {
    pub sender: String,
    pub target: String,
    /// The `in` event's type and payload, which the `out` record mirrors.
    pub kind: String,
    pub data: Value,
}

impl Unrecorded {
    /// The sender's record: the same event, seen from the sender. A release or drop
    /// names only the message.
    pub fn out_record(mut self) -> (String, cyber_store::NewEvent) {
        self.data["direction"] = Value::from(Direction::Out.as_str());
        if self.kind != RELEASED && self.kind != DROPPED {
            self.data["peer_session_id"] = Value::from(self.target);
        }
        (
            self.sender,
            cyber_store::NewEvent::new(self.kind, self.data),
        )
    }
}

/// The sender records a crash left unwritten: every `in` record from a local Session that
/// still exists and has no `out` record of the message (`message_id` alone when given),
/// in commit order.
pub(crate) fn unrecorded(
    db: &rusqlite::Connection,
    message_id: Option<&str>,
) -> rusqlite::Result<Vec<Unrecorded>> {
    let mut stmt = db.prepare(
        "SELECT m.peer_session_id, m.session_id, e.type, e.data FROM xsession_message m
           JOIN event e ON e.aggregate_id = m.session_id
            AND e.type IN (?2, ?3, ?4)
            AND json_extract(e.data, '$.message_id') = m.message_id
            AND json_extract(e.data, '$.direction') = 'in'
         WHERE m.direction = 'in' AND m.origin_kind = 'session'
           AND (?1 IS NULL OR m.message_id = ?1)
           AND m.peer_session_id IN (SELECT id FROM session)
           AND NOT EXISTS (SELECT 1 FROM xsession_message o
                WHERE o.session_id = m.peer_session_id AND o.message_id = m.message_id
                  AND o.direction = 'out')
         ORDER BY e.rowid",
    )?;
    stmt.query_map(
        params![message_id, DELIVERED, HELD, REFUSED],
        unrecorded_row,
    )?
    .collect()
}

/// The releases and drops of received messages whose local sender's `out` record still
/// shows them `held` (`message_id` alone when given), in commit order: each mirrors the
/// `in` event that last changed the target's record.
pub(crate) fn unrecorded_outcomes(
    db: &rusqlite::Connection,
    message_id: Option<&str>,
) -> rusqlite::Result<Vec<Unrecorded>> {
    let mut stmt = db.prepare(
        "SELECT m.peer_session_id, m.session_id, e.type, e.data FROM xsession_message m
           JOIN xsession_message o ON o.session_id = m.peer_session_id
            AND o.message_id = m.message_id AND o.direction = 'out' AND o.outcome = 'held'
           JOIN event e ON e.aggregate_id = m.session_id AND e.seq = m.seq AND e.type IN (?2, ?3)
         WHERE m.direction = 'in' AND m.outcome IN ('released', 'dropped')
           AND (?1 IS NULL OR m.message_id = ?1)
         ORDER BY e.rowid",
    )?;
    stmt.query_map(params![message_id, RELEASED, DROPPED], unrecorded_row)?
        .collect()
}

fn unrecorded_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Unrecorded> {
    let data: String = r.get(3)?;
    let data = serde_json::from_str(&data).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(3, rusqlite::types::Type::Text, Box::new(e))
    })?;
    Ok(Unrecorded {
        sender: r.get(0)?,
        target: r.get(1)?,
        kind: r.get(2)?,
        data,
    })
}

pub(crate) const WATCH_COLUMNS: &str = "id, subscriber_id, target_address, target_id, status, created_ms, expires_ms, fired_ms, notice_id, target_peer";

pub(crate) fn watch_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<WatchRow> {
    Ok(WatchRow {
        id: r.get(0)?,
        subscriber_id: r.get(1)?,
        target_address: r.get(2)?,
        target_id: r.get(3)?,
        status: r.get(4)?,
        created_ms: r.get(5)?,
        expires_ms: r.get(6)?,
        fired_ms: r.get(7)?,
        notice_id: r.get(8)?,
        target_peer: r.get(9)?,
    })
}

/// The watches a Session subscribed to, oldest first.
pub(crate) fn watch_rows(
    db: &rusqlite::Connection,
    subscriber_id: &str,
) -> rusqlite::Result<Vec<WatchRow>> {
    let mut stmt = db.prepare(&format!(
        "SELECT {WATCH_COLUMNS} FROM messaging_watch WHERE subscriber_id = ?1 ORDER BY created_ms, id"
    ))?;
    stmt.query_map([subscriber_id], watch_row)?.collect()
}

#[cfg(test)]
#[path = "records_tests.rs"]
mod tests;
