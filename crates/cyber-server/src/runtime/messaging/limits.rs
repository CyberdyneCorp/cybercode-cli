//! Message limits (`cross-session-messaging` → Message content limits, Rate limits;
//! design section 7). The text is plain UTF-8, non-empty, without NUL and at most
//! [`MAX_MESSAGE_BYTES`], checked before the address is resolved, so an oversized message
//! never leaves the sender. The rate window and the pending cap are checked under the
//! target's send lock: the window counts the target's durable `in` records, so a restart
//! does not reset it.

use rusqlite::Connection;

use crate::runtime::RuntimeError;

/// The largest message text, in UTF-8 bytes.
pub const MAX_MESSAGE_BYTES: usize = 16 * 1024;
/// Accepted messages one sender key may send one target per [`RATE_WINDOW_MS`].
pub const MAX_PER_TARGET_MINUTE: usize = 10;
/// Refusals one sender key may have recorded in one target per [`RATE_WINDOW_MS`]; past
/// it a refusal is recorded only in the sender, as `rate_limited`, so no sender grows
/// another Session's log without bound.
pub const MAX_REFUSED_PER_TARGET_MINUTE: usize = 10;
/// The sliding rate window.
pub const RATE_WINDOW_MS: i64 = 60_000;
/// Unpromoted message rows (`pending` or `held`) a target inbox holds at most.
pub const MAX_PENDING: usize = 30;

/// Which limit a `rate_limited` refusal hit (its `limit` field).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Limit {
    PerTargetMinute,
    InboxPending,
}

impl Limit {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::PerTargetMinute => "per_target_minute",
            Self::InboxPending => "inbox_pending",
        }
    }
}

/// The sender keys whose records the limits count together as one sender.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LimitKey {
    /// One sender key (a local Session, a client).
    Exact(String),
    /// Every key starting with this prefix: all Sessions of one peer grant
    /// (`peer:<grant>:`), whose Session labels the peer chooses.
    Prefix(String),
}

/// The messages of `key` that `target` accepted (delivered or held, whatever became of
/// them since) in the window ending at `now_ms`. Refusals never count.
pub fn accepted_in_window(
    db: &Connection,
    target: &str,
    key: &LimitKey,
    now_ms: i64,
) -> rusqlite::Result<usize> {
    in_window(db, target, key, now_ms, false)
}

/// The refusals of `key` that `target` recorded in the window ending at `now_ms`.
pub fn refused_in_window(
    db: &Connection,
    target: &str,
    key: &LimitKey,
    now_ms: i64,
) -> rusqlite::Result<usize> {
    in_window(db, target, key, now_ms, true)
}

/// The target's `in` records of `key` in the window, refused or not.
fn in_window(
    db: &Connection,
    target: &str,
    key: &LimitKey,
    now_ms: i64,
    refused: bool,
) -> rusqlite::Result<usize> {
    let (key, prefix) = match key {
        LimitKey::Exact(key) => (key, false),
        LimitKey::Prefix(prefix) => (prefix, true),
    };
    db.query_row(
        "SELECT COUNT(*) FROM xsession_message
         WHERE session_id = ?1 AND direction = 'in'
           AND (sender_key = ?2 OR (?5 AND substr(sender_key, 1, length(?2)) = ?2))
           AND (outcome = 'refused') = ?4 AND created_ms > ?3",
        rusqlite::params![target, key, now_ms - RATE_WINDOW_MS, refused, prefix],
        |row| row.get::<_, i64>(0),
    )
    .map(|count| usize::try_from(count).unwrap_or(usize::MAX))
}

/// The rate limits of one send: `accepted` messages of this sender in the window and
/// `pending` unpromoted message rows in the target, both before this message.
pub fn check_rate(accepted: usize, pending: usize) -> Result<(), Limit> {
    if accepted >= MAX_PER_TARGET_MINUTE {
        Err(Limit::PerTargetMinute)
    } else if pending >= MAX_PENDING {
        Err(Limit::InboxPending)
    } else {
        Ok(())
    }
}

/// The chain hops of a message sent from a run at `run_hops`, or `None` when it would
/// exceed `max_hops` (`chain_limit`).
pub fn next_hop(run_hops: u32, max_hops: u32) -> Option<u32> {
    Some(run_hops.saturating_add(1)).filter(|hops| *hops <= max_hops)
}

/// Why a message text is not sendable. Nothing is recorded for either.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TextError {
    #[error("MessageTooLargeError: the message is {size} bytes; the limit is {limit}")]
    TooLarge { size: usize, limit: usize },
    #[error("InvalidRequestError: {0}")]
    Invalid(&'static str),
}

impl From<TextError> for RuntimeError {
    fn from(error: TextError) -> Self {
        match error {
            TextError::TooLarge { size, limit } => Self::MessageTooLarge { size, limit },
            TextError::Invalid(reason) => Self::Invalid(reason.into()),
        }
    }
}

/// Check a message text against the content limits.
pub fn check_text(text: &str) -> Result<(), TextError> {
    if text.len() > MAX_MESSAGE_BYTES {
        return Err(TextError::TooLarge {
            size: text.len(),
            limit: MAX_MESSAGE_BYTES,
        });
    }
    if text.is_empty() {
        return Err(TextError::Invalid("a message needs text"));
    }
    if text.contains('\0') {
        return Err(TextError::Invalid(
            "a message cannot contain NUL characters",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    use cyber_core::paths::DatabaseLocation;
    use cyber_store::{Expected, NewEvent, Store, StoreOptions};
    use serde_json::json;

    use crate::runtime::messaging::records::{self, text_sha256};

    fn open(location: DatabaseLocation) -> Store {
        Store::open(StoreOptions::new(
            location,
            crate::runtime::Runtime::registry(),
        ))
        .unwrap()
    }

    fn store(location: DatabaseLocation) -> Store {
        let store = open(location);
        store
            .transaction(|tx| {
                tx.execute_batch(
                    "INSERT INTO session (id, title, directory, agent, model, mode, created_at, updated_at, last_seq)
                     VALUES ('ses_t', 'target', '/r', 'build', 'test/main', 'default', 1, 1, -1);",
                )?;
                Ok(())
            })
            .unwrap();
        store
    }

    /// An `in` record of `ses_t` from `key`, accepted or refused; returns its `created_ms`.
    fn record(store: &Store, id: &str, key: &str, kind: &str) -> i64 {
        let mut data = json!({
            "message_id": id, "direction": "in", "from": key, "to": "target",
            "origin_kind": "client", "sender_key": key, "size": 2,
            "sha256": text_sha256("hi"),
        });
        let extra = if kind == records::REFUSED {
            json!({"reason": "inbound_rule"})
        } else {
            json!({"text": "hi", "depth": 0, "hops": 1, "delivery": "hold"})
        };
        data.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        let stored = store
            .append("ses_t", Expected::Any, vec![NewEvent::new(kind, data)])
            .unwrap();
        stored[0].time_ms
    }

    fn count(store: &Store, key: &str, now_ms: i64) -> usize {
        count_as(store, LimitKey::Exact(key.to_owned()), now_ms)
    }

    fn count_as(store: &Store, key: LimitKey, now_ms: i64) -> usize {
        store
            .read(move |db| Ok(accepted_in_window(db, "ses_t", &key, now_ms)?))
            .unwrap()
    }

    /// Regression: a peer's limits were keyed on the Session label it sends, so a new
    /// `from_session` per message escaped them. A grant's Sessions count together.
    #[test]
    fn a_prefix_counts_every_session_of_one_grant() {
        let store = store(DatabaseLocation::Memory);
        let first = record(&store, "msgx_1", "peer:laptop:ses_a", records::HELD);
        record(&store, "msgx_2", "peer:laptop:ses_b", records::HELD);
        record(&store, "msgx_3", "peer:laptop2:ses_a", records::HELD);
        let grant = |prefix: &str| LimitKey::Prefix(prefix.to_owned());
        assert_eq!(count_as(&store, grant("peer:laptop:"), first), 2);
        assert_eq!(count_as(&store, grant("peer:laptop2:"), first), 1);
        assert_eq!(count(&store, "peer:laptop:ses_a", first), 1);
        // `%` and `_` are not patterns here.
        assert_eq!(count_as(&store, grant("peer:lap%"), first), 0);
        assert_eq!(count_as(&store, grant("peer:l_ptop:"), first), 0);
    }

    #[test]
    fn the_rate_window_slides_and_refusals_do_not_count() {
        let store = store(DatabaseLocation::Memory);
        let first = record(&store, "msgx_1", "client:a", records::HELD);
        record(&store, "msgx_2", "client:a", records::DELIVERED);
        record(&store, "msgx_3", "client:a", records::REFUSED);
        record(&store, "msgx_4", "client:b", records::HELD);
        assert_eq!(
            count(&store, "client:a", first),
            2,
            "a refusal never counts"
        );
        assert_eq!(count(&store, "client:b", first), 1, "per sender key");
        // The window is the 60 s before `now`: a record leaves it once it is 60 s old.
        assert_eq!(count(&store, "client:a", first + RATE_WINDOW_MS - 1), 2);
        assert_eq!(
            count(&store, "client:a", first + RATE_WINDOW_MS + 60_000),
            0
        );
    }

    #[test]
    fn a_restart_does_not_reset_the_rate_window() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("cyber.db");
        let store = store(DatabaseLocation::File(path.clone()));
        let sent = record(&store, "msgx_1", "ses_a", records::DELIVERED);
        drop(store);
        let reopened = open(DatabaseLocation::File(path));
        assert_eq!(count(&reopened, "ses_a", sent + 30_000), 1);
    }

    #[test]
    fn the_eleventh_message_and_the_thirty_first_pending_row_are_limited() {
        assert_eq!(check_rate(9, 29), Ok(()));
        assert_eq!(check_rate(10, 0), Err(Limit::PerTargetMinute));
        assert_eq!(check_rate(0, 30), Err(Limit::InboxPending));
        assert_eq!(check_rate(10, 30), Err(Limit::PerTargetMinute));
        assert_eq!(Limit::InboxPending.as_str(), "inbox_pending");
    }

    #[test]
    fn a_chain_stops_above_max_hops() {
        assert_eq!(next_hop(0, 8), Some(1));
        assert_eq!(next_hop(7, 8), Some(8));
        assert_eq!(next_hop(8, 8), None);
        assert_eq!(next_hop(u32::MAX, 64), None);
    }

    #[test]
    fn texts_up_to_sixteen_kibibytes_pass_and_larger_ones_report_their_size() {
        assert_eq!(check_text("which port?"), Ok(()));
        assert_eq!(check_text(&"x".repeat(MAX_MESSAGE_BYTES)), Ok(()));
        assert_eq!(
            check_text(&"x".repeat(MAX_MESSAGE_BYTES + 1)),
            Err(TextError::TooLarge {
                size: MAX_MESSAGE_BYTES + 1,
                limit: 16_384
            })
        );
        // The limit counts UTF-8 bytes, not characters.
        let wide = "é".repeat(MAX_MESSAGE_BYTES / 2 + 1);
        assert!(matches!(
            check_text(&wide),
            Err(TextError::TooLarge { size, .. }) if size == MAX_MESSAGE_BYTES + 2
        ));
    }

    #[test]
    fn empty_texts_and_nul_characters_are_rejected() {
        assert!(matches!(check_text(""), Err(TextError::Invalid(_))));
        assert!(matches!(check_text("a\0b"), Err(TextError::Invalid(_))));
    }
}
