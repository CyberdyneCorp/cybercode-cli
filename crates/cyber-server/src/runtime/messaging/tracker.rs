//! Outcome tracking of messages held on a peer (`cross-session-messaging` → Peer outcome
//! tracking; design section 5).
//!
//! A peer usually cannot reach its sender (laptops behind NAT, one-direction grants), so
//! the sending server polls `GET /api/v1/peer/messages/:id` for every `out` record a peer
//! answered `held`: 5 s after the send, then 15 s, 1 min, and every 5 min after that, for
//! up to 7 days, when the outcome becomes `expired`. A release or drop there appends
//! `messaging.message.released.1` or `messaging.message.dropped.1` to the sender Session
//! and publishes the live `messaging.outcome` event. The time of the next poll is
//! `xsession_message.next_poll_ms`, so polling resumes after a restart. A peer that does
//! not answer, refuses the grant or no longer knows the message is polled again later;
//! nothing is sent again.
//!
//! A pass asks the peers in parallel, each with one client and at most [`PER_PEER`] polls
//! (the oldest due first), so a peer that does not answer delays only its own messages
//! and a backlog stays well under the peer's 120 requests a minute per grant. After the
//! first failure to reach a peer, its other due messages are rescheduled unasked; a
//! message whose change cannot be recorded is logged and rescheduled, so it never blocks
//! the messages behind it.

use std::collections::BTreeMap;
use std::sync::PoisonError;
use std::time::Duration;

use cyber_store::NewEvent;
use rusqlite::params;

use super::peer::PeerClient;
use super::receive::PeerOutcome;
use super::records::{
    DROPPED, Direction, EXPIRED, MessageDropped, MessageExpired, MessageReleased, RELEASED,
};
use super::{Messaging, RuntimeError};
use crate::runtime::events::event;
use crate::runtime::{Delivery, LiveEvent, Runtime};

/// How often the tracker looks for polls that are due: the shortest backoff step.
pub const TRACK_EVERY: Duration = Duration::from_secs(5);
/// How long a sender follows a message a peer holds.
pub const TRACK_FOR_MS: i64 = 7 * 24 * 3_600_000;
/// When the first polls happen, after the send: 5 s, then 15 s and 1 min later.
const FIRST_POLLS_MS: [i64; 3] = [5_000, 20_000, 80_000];
/// The interval after the first polls.
const STEADY_MS: i64 = 300_000;
/// Most polls in one pass; the rest wait for the next pass.
const BATCH: i64 = 100;
/// Most polls of one peer in one pass: with a pass every [`TRACK_EVERY`], at most 60 a
/// minute, half the peer's per-grant rate, leaving room for sends, listings and watches.
pub const PER_PEER: i64 = 5;
/// The tracker's lock, among the send locks; no address has this form.
const LOCK: &str = "\0outcomes";

/// The first poll of a message a peer answered `held` at `sent_ms`.
pub(crate) fn first_poll_ms(sent_ms: i64) -> i64 {
    sent_ms.saturating_add(FIRST_POLLS_MS[0])
}

/// The poll after one at `now_ms` of a message sent at `sent_ms`: the next step of the
/// backoff after `now_ms`, never later than the end of tracking (the last poll).
pub fn next_poll_ms(sent_ms: i64, now_ms: i64) -> i64 {
    let age = now_ms.saturating_sub(sent_ms).max(0);
    let last = FIRST_POLLS_MS[FIRST_POLLS_MS.len() - 1];
    let offset = FIRST_POLLS_MS
        .iter()
        .copied()
        .find(|offset| *offset > age)
        .unwrap_or_else(|| last + ((age - last) / STEADY_MS + 1) * STEADY_MS);
    sent_ms.saturating_add(offset.min(TRACK_FOR_MS))
}

/// What one pass of the tracker did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Tracked {
    /// Messages whose poll was due.
    pub polled: usize,
    /// Messages the peer released or dropped.
    pub changed: usize,
    /// Messages still held after 7 days.
    pub expired: usize,
}

/// One `out` record the tracker follows.
#[derive(Debug, Clone)]
struct Followed {
    session_id: String,
    message_id: String,
    to: String,
    peer: String,
    sent_ms: i64,
}

/// What the peer says about a followed message.
enum Answer {
    Released,
    Dropped(Option<String>),
    /// Still held, or no answer (unreachable, refused grant, unknown to the peer).
    Unchanged,
}

/// The record a pass appends for one followed message.
struct Change {
    record: NewEvent,
    outcome: &'static str,
    reason: Option<String>,
}

impl Answer {
    /// What to record for `message` at `now_ms`: a release, a drop, the end of tracking,
    /// or nothing (it is polled again).
    fn change(self, message: &Followed, now_ms: i64) -> Option<Change> {
        let message_id = message.message_id.clone();
        let direction = Direction::Out;
        let (record, outcome, reason) = match self {
            Self::Released => {
                let record = MessageReleased {
                    message_id,
                    direction,
                    delivery: Delivery::Steer,
                };
                (event(RELEASED, &record), "released", None)
            }
            Self::Dropped(reason) => {
                let record = MessageDropped {
                    message_id,
                    direction,
                    reason: reason.clone(),
                };
                (event(DROPPED, &record), "dropped", reason)
            }
            Self::Unchanged if now_ms >= message.sent_ms.saturating_add(TRACK_FOR_MS) => {
                let record = MessageExpired {
                    message_id,
                    direction,
                };
                (event(EXPIRED, &record), "expired", None)
            }
            Self::Unchanged => return None,
        };
        Some(Change {
            record,
            outcome,
            reason,
        })
    }

    fn of(outcome: PeerOutcome) -> Self {
        match outcome.outcome.as_str() {
            "released" => Self::Released,
            // A held message refused on the target's inbox is removed unread there too.
            "dropped" | "refused" => Self::Dropped(outcome.reason),
            _ => Self::Unchanged,
        }
    }
}

impl Tracked {
    fn add(self, other: Self) -> Self {
        Self {
            polled: self.polled + other.polled,
            changed: self.changed + other.changed,
            expired: self.expired + other.expired,
        }
    }
}

/// The due messages, at most [`PER_PEER`] per peer (the oldest due first) and [`BATCH`]
/// in all.
fn due(db: &rusqlite::Connection, now_ms: i64) -> rusqlite::Result<Vec<Followed>> {
    let mut stmt = db.prepare(
        "SELECT session_id, message_id, peer_address, remote_peer, created_ms FROM (
            SELECT session_id, message_id, peer_address, remote_peer, created_ms,
                   next_poll_ms, ROW_NUMBER() OVER (
                       PARTITION BY remote_peer ORDER BY next_poll_ms, message_id) AS rank
              FROM xsession_message
             WHERE next_poll_ms IS NOT NULL AND next_poll_ms <= ?1 AND direction = 'out'
               AND outcome = 'held' AND remote_peer IS NOT NULL)
         WHERE rank <= ?3
         ORDER BY next_poll_ms, message_id LIMIT ?2",
    )?;
    stmt.query_map(params![now_ms, BATCH, PER_PEER], |r| {
        Ok(Followed {
            session_id: r.get(0)?,
            message_id: r.get(1)?,
            to: r.get(2)?,
            peer: r.get(3)?,
            sent_ms: r.get(4)?,
        })
    })?
    .collect()
}

impl Messaging {
    /// Poll the peers for every followed message whose poll is due at `now_ms`, record
    /// the changes in the sender Sessions and schedule the next polls. Runs every
    /// [`TRACK_EVERY`]; `now_ms` is injected so tests can move the clock.
    pub async fn track_outcomes(&self, now_ms: i64) -> Result<Tracked, RuntimeError> {
        let lock = self.runtime.message_lock(LOCK);
        let _tracking = lock.lock().await;
        let followed = self
            .runtime
            .inner
            .store
            .read(move |db| Ok(due(db, now_ms)?))?;
        let mut by_peer: BTreeMap<String, Vec<Followed>> = BTreeMap::new();
        for message in followed {
            by_peer
                .entry(message.peer.clone())
                .or_default()
                .push(message);
        }
        let passes = by_peer
            .into_iter()
            .map(|(peer, messages)| self.track_peer(peer, messages, now_ms));
        let tracked = futures::future::join_all(passes).await;
        Ok(tracked.into_iter().fold(Tracked::default(), Tracked::add))
    }

    /// The due messages of one peer, in order, with one client. After the first failure
    /// to reach the peer, the rest are rescheduled without a request.
    async fn track_peer(&self, peer: String, messages: Vec<Followed>, now_ms: i64) -> Tracked {
        let mut client = self
            .peer_target(&peer)
            .ok()
            .and_then(|target| PeerClient::new(target).ok());
        let mut tracked = Tracked::default();
        for message in messages {
            tracked.polled += 1;
            let answer = match &client {
                Some(client) => poll(client, &message).await,
                None => None,
            };
            if answer.is_none() {
                client = None;
            }
            let change = answer.unwrap_or(Answer::Unchanged).change(&message, now_ms);
            match self.settle(&message, change, now_ms).await {
                Some("expired") => tracked.expired += 1,
                Some(_) => tracked.changed += 1,
                None => {}
            }
        }
        tracked
    }

    /// Record `change` of `message`, or schedule its next poll; the outcome recorded.
    /// A failure is logged and the message rescheduled, so it cannot block the others.
    async fn settle(
        &self,
        message: &Followed,
        change: Option<Change>,
        now_ms: i64,
    ) -> Option<&'static str> {
        let recorded = match change {
            Some(change) => {
                let outcome = change.outcome;
                self.changed(message, change.record, outcome, change.reason)
                    .await
                    .map(|()| Some(outcome))
            }
            None => self.reschedule(message, now_ms).map(|()| None),
        };
        recorded.unwrap_or_else(|error| {
            warn(message, "peer outcome not recorded", &error);
            if let Err(error) = self.reschedule(message, now_ms) {
                warn(message, "peer outcome poll not rescheduled", &error);
            }
            None
        })
    }

    /// Record a change in the sender Session (which ends polling) and publish it live.
    async fn changed(
        &self,
        message: &Followed,
        record: NewEvent,
        outcome: &str,
        reason: Option<String>,
    ) -> Result<(), RuntimeError> {
        self.runtime
            .record_message(&message.session_id, record)
            .await?;
        self.runtime.inner.bus.publish(LiveEvent::MessagingOutcome {
            session_id: message.session_id.clone(),
            message_id: message.message_id.clone(),
            to: message.to.clone(),
            outcome: outcome.into(),
            reason,
        });
        Ok(())
    }

    fn reschedule(&self, message: &Followed, now_ms: i64) -> Result<(), RuntimeError> {
        let next = next_poll_ms(message.sent_ms, now_ms);
        let (session, id) = (message.session_id.clone(), message.message_id.clone());
        self.runtime.inner.store.transaction(move |tx| {
            tx.execute(
                "UPDATE xsession_message SET next_poll_ms = ?3
                 WHERE session_id = ?1 AND message_id = ?2 AND direction = 'out'
                   AND outcome = 'held'",
                params![session, id, next],
            )?;
            Ok(())
        })?;
        Ok(())
    }
}

fn warn(message: &Followed, what: &str, error: &RuntimeError) {
    cyber_core::log::warn(
        "messaging",
        what,
        serde_json::json!({ "message_id": message.message_id, "error": error.to_string() }),
    );
}

/// Ask the peer for the message's outcome: `None` when the peer cannot be reached (or
/// refuses the grant), else what it says. An answer this version cannot read, or an
/// unknown message, is unchanged: the message is polled again until tracking ends.
async fn poll(client: &PeerClient, message: &Followed) -> Option<Answer> {
    let path = format!("/messages/{}", message.message_id);
    match client.request(reqwest::Method::GET, &path, None).await {
        Ok((200, body)) => Some(
            serde_json::from_value::<PeerOutcome>(body)
                .ok()
                .filter(|outcome| outcome.message_id == message.message_id)
                .map_or(Answer::Unchanged, Answer::of),
        ),
        Ok(_) => Some(Answer::Unchanged),
        Err(_) => None,
    }
}

impl Runtime {
    /// Poll for held peer messages every [`TRACK_EVERY`] until shutdown, from the times
    /// the store keeps (so a restart resumes where the last run stopped).
    pub fn spawn_outcome_tracker(&self) {
        let weak = self.downgrade();
        let closed = self.inner.closed.clone();
        let task = tokio::spawn(async move {
            loop {
                tokio::select! {
                    () = closed.cancelled() => return,
                    () = tokio::time::sleep(TRACK_EVERY) => {}
                }
                let Some(runtime) = weak.upgrade() else {
                    return;
                };
                let now = chrono::Utc::now().timestamp_millis();
                if let Err(error) = runtime.messaging().track_outcomes(now).await {
                    cyber_core::log::warn(
                        "messaging",
                        "peer outcome tracking failed",
                        serde_json::json!({ "error": error.to_string() }),
                    );
                }
            }
        });
        let mut owners = self
            .inner
            .background
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        owners.retain(|owner| !owner.is_finished());
        owners.push(task);
    }
}

#[cfg(test)]
mod tests {
    use cyber_core::paths::DatabaseLocation;
    use cyber_store::{Expected, Store, StoreOptions};
    use serde_json::json;

    use super::*;
    use crate::runtime::messaging::records::{HELD, text_sha256};

    /// Regression: a pass polled the 100 oldest due messages one after another, so one
    /// peer's backlog both hogged the pass and went past its 120 requests a minute. A
    /// pass now takes at most [`PER_PEER`] of each peer, oldest due first.
    #[test]
    fn a_pass_takes_a_few_due_messages_of_each_peer() {
        let store = Store::open(StoreOptions::new(
            DatabaseLocation::Memory,
            crate::runtime::Runtime::registry(),
        ))
        .unwrap();
        store
            .transaction(|tx| {
                tx.execute_batch(
                    "INSERT INTO session (id, title, directory, agent, model, mode, created_at, updated_at, last_seq)
                     VALUES ('ses_s', 'review', '/r', 'build', 'test/main', 'default', 1, 1, -1);",
                )?;
                Ok(())
            })
            .unwrap();
        let held = |id: &str, peer: &str| {
            let data = json!({
                "message_id": id, "direction": "out", "from": "ses_s", "to": format!("x@{peer}"),
                "origin_kind": "peer", "sender_key": "ses_s", "size": 2,
                "sha256": text_sha256("hi"), "text": "hi", "depth": 0, "hops": 1,
                "delivery": "hold", "remote_peer": peer, "remote_session_id": "ses_x",
            });
            store
                .append("ses_s", Expected::Any, vec![NewEvent::new(HELD, data)])
                .unwrap();
        };
        for i in 0..7 {
            held(&format!("msgx_d{i}"), "desk");
        }
        held("msgx_n0", "nas");
        held("msgx_n1", "nas");
        let due = store.read(|db| Ok(due(db, i64::MAX)?)).unwrap();
        let ids = |peer: &str| -> Vec<String> {
            due.iter()
                .filter(|m| m.peer == peer)
                .map(|m| m.message_id.clone())
                .collect()
        };
        assert_eq!(
            ids("desk"),
            ["msgx_d0", "msgx_d1", "msgx_d2", "msgx_d3", "msgx_d4"]
        );
        assert_eq!(ids("nas"), ["msgx_n0", "msgx_n1"]);
    }

    #[test]
    fn polls_back_off_from_five_seconds_to_five_minutes_for_seven_days() {
        let sent = 1_000_000;
        assert_eq!(first_poll_ms(sent), sent + 5_000);
        // Each poll schedules the next step: 5 s, 15 s, 1 min, then every 5 min.
        let mut at = first_poll_ms(sent);
        let mut gaps = Vec::new();
        for _ in 0..5 {
            let next = next_poll_ms(sent, at);
            gaps.push(next - at);
            at = next;
        }
        assert_eq!(gaps, [15_000, 60_000, 300_000, 300_000, 300_000]);
        // A late poll (a server that was stopped) joins the schedule after now.
        assert_eq!(next_poll_ms(sent, sent + 100_000), sent + 380_000);
        assert_eq!(next_poll_ms(sent, sent - 10), sent + 5_000);
        // The last poll is at the end of tracking.
        let end = sent + TRACK_FOR_MS;
        assert_eq!(next_poll_ms(sent, end - 1), end);
        assert_eq!(next_poll_ms(sent, end + 600_000), end);
    }
}
