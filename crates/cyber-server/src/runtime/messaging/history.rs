//! A Session's message history (`GET /api/v1/sessions/:id/messaging`).
//!
//! Pages walk `(created_ms, message_id, direction)`, a key no later outcome changes, so
//! a release or drop between two pages neither repeats nor skips a record. The cursor
//! carries the order and the direction filter it was made for; any other use is an
//! `InvalidCursorError`.

use rusqlite::params;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::records::{self, Direction, MessageRow};
use super::{Messaging, RuntimeError};

/// Records per page without `limit`.
pub const DEFAULT_HISTORY_LIMIT: u32 = 50;
/// The largest `limit` (`server-api` → Pagination).
pub const MAX_HISTORY_LIMIT: u32 = 200;

/// Page order by creation time.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Order {
    Asc,
    /// Newest first (the default).
    #[default]
    Desc,
}

impl Order {
    fn tag(self) -> &'static str {
        match self {
            Self::Asc => "a",
            Self::Desc => "d",
        }
    }
}

/// One history request.
#[derive(Debug, Clone, Default)]
pub struct HistoryQuery {
    /// Only the records of this side.
    pub direction: Option<Direction>,
    pub order: Order,
    /// 1 to [`MAX_HISTORY_LIMIT`]; `None` is [`DEFAULT_HISTORY_LIMIT`].
    pub limit: Option<u32>,
    /// `next` of the previous page.
    pub cursor: Option<String>,
}

/// One side of a message as the history shows it (`cross-session-messaging` → Messaging
/// API). A refused message carries no `text`.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct MessageRecord {
    pub message_id: String,
    /// The Session whose record this is.
    pub session_id: String,
    pub direction: Direction,
    /// The sender's address: the peer for `in`, this Session for `out`.
    pub from: String,
    /// The target's address: this Session for `in`, the peer for `out`.
    pub to: String,
    /// The other side's local Session, when it has one (none for a client).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub peer_session_id: Option<String>,
    /// `session` or `client`; an open set.
    pub origin_kind: String,
    /// `delivered`, `held`, `refused`, `released` or `dropped`.
    pub outcome: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// UTF-8 bytes of the text.
    pub size: u32,
    pub sha256: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// The sender's MCP recursion depth.
    pub depth: u32,
    /// The message's position in its chain.
    pub hops: u32,
    /// RFC 3339, milliseconds.
    pub created_at: String,
    /// When the outcome last changed (RFC 3339, milliseconds).
    pub updated_at: String,
    /// The event that last changed this record.
    pub seq: i64,
}

impl From<MessageRow> for MessageRecord {
    fn from(row: MessageRow) -> Self {
        let (from, to) = match row.direction {
            Direction::In => (row.peer_address, row.session_id.clone()),
            Direction::Out => (row.session_id.clone(), row.peer_address),
        };
        Self {
            message_id: row.message_id,
            session_id: row.session_id,
            direction: row.direction,
            from,
            to,
            peer_session_id: row.peer_session_id,
            origin_kind: row.origin_kind,
            outcome: row.outcome,
            reason: row.reason,
            size: row.size,
            sha256: row.sha256,
            text: row.text,
            depth: row.depth,
            hops: row.hops,
            created_at: rfc3339_ms(row.created_ms),
            updated_at: rfc3339_ms(row.updated_ms),
            seq: row.seq,
        }
    }
}

/// One page and the cursor of the next, if any.
#[derive(Debug, Clone)]
pub struct HistoryPage {
    pub records: Vec<MessageRecord>,
    pub next: Option<String>,
}

fn rfc3339_ms(ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(ms)
        .unwrap_or_default()
        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

/// Where a page starts: after this record's key.
#[derive(Debug, Clone, PartialEq, Eq)]
struct After {
    created_ms: i64,
    message_id: String,
    direction: &'static str,
}

/// The filter part of a cursor: the order and the direction it pages.
fn scope(order: Order, direction: Option<Direction>) -> String {
    format!(
        "{}{}",
        order.tag(),
        direction.map_or("*", Direction::as_str)
    )
}

fn encode(order: Order, direction: Option<Direction>, last: &MessageRow) -> String {
    format!(
        "{}:{}:{}:{}",
        scope(order, direction),
        last.created_ms,
        last.direction.as_str(),
        last.message_id
    )
}

/// Parse a cursor made for exactly this order and direction filter.
fn decode(cursor: &str, order: Order, direction: Option<Direction>) -> Result<After, RuntimeError> {
    let invalid =
        || RuntimeError::Invalid("InvalidCursorError: malformed or foreign cursor".into());
    let mut parts = cursor.splitn(4, ':');
    let (Some(made_for), Some(created), Some(side), Some(message_id)) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err(invalid());
    };
    if made_for != scope(order, direction) || message_id.is_empty() {
        return Err(invalid());
    }
    let direction = match side {
        "in" => Direction::In.as_str(),
        "out" => Direction::Out.as_str(),
        _ => return Err(invalid()),
    };
    Ok(After {
        created_ms: created.parse().map_err(|_| invalid())?,
        message_id: message_id.to_owned(),
        direction,
    })
}

/// Up to `limit` rows of `session` after `after`, in `order`, filtered by `direction`.
fn page_rows(
    db: &rusqlite::Connection,
    session: &str,
    direction: Option<Direction>,
    order: Order,
    after: Option<&After>,
    limit: u32,
) -> rusqlite::Result<Vec<MessageRow>> {
    let (cmp, sort) = match order {
        Order::Asc => (">", "ASC"),
        Order::Desc => ("<", "DESC"),
    };
    let sql = format!(
        "SELECT {columns} FROM xsession_message
         WHERE session_id = ?1 AND (?2 IS NULL OR direction = ?2)
           AND (?3 IS NULL OR (created_ms, message_id, direction) {cmp} (?3, ?4, ?5))
         ORDER BY created_ms {sort}, message_id {sort}, direction {sort}
         LIMIT ?6",
        columns = records::MESSAGE_COLUMNS,
    );
    let mut stmt = db.prepare(&sql)?;
    let rows = stmt.query_map(
        params![
            session,
            direction.map(Direction::as_str),
            after.map(|a| a.created_ms),
            after.map(|a| a.message_id.as_str()),
            after.map(|a| a.direction),
            limit,
        ],
        records::message_row,
    )?;
    rows.collect()
}

impl Messaging {
    /// One page of `session`'s sent and received messages. An unknown Session is
    /// `SessionNotFoundError`; a bad limit or cursor is rejected before reading.
    pub fn history(&self, session: &str, query: HistoryQuery) -> Result<HistoryPage, RuntimeError> {
        let limit = query.limit.unwrap_or(DEFAULT_HISTORY_LIMIT);
        if !(1..=MAX_HISTORY_LIMIT).contains(&limit) {
            return Err(RuntimeError::Invalid(format!(
                "limit must be between 1 and {MAX_HISTORY_LIMIT}"
            )));
        }
        let after = query
            .cursor
            .as_deref()
            .map(|cursor| decode(cursor, query.order, query.direction))
            .transpose()?;
        self.caller_exists(Some(session))?;
        let id = session.to_owned();
        let (direction, order) = (query.direction, query.order);
        // One row more than the page tells whether another page follows.
        let mut rows = self.runtime.inner.store.read(move |db| {
            Ok(page_rows(
                db,
                &id,
                direction,
                order,
                after.as_ref(),
                limit + 1,
            )?)
        })?;
        let more = rows.len() > limit as usize;
        rows.truncate(limit as usize);
        let next = more
            .then(|| rows.last().map(|last| encode(order, direction, last)))
            .flatten();
        Ok(HistoryPage {
            records: rows.into_iter().map(MessageRecord::from).collect(),
            next,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(created_ms: i64, message_id: &str, direction: Direction) -> MessageRow {
        MessageRow {
            session_id: "ses_a".into(),
            message_id: message_id.into(),
            direction,
            peer_address: "peer".into(),
            peer_session_id: None,
            origin_kind: "client".into(),
            sender_key: "client:api".into(),
            outcome: "refused".into(),
            reason: Some("busy".into()),
            size: 3,
            sha256: "x".into(),
            text: None,
            depth: 0,
            hops: 1,
            created_ms,
            updated_ms: created_ms,
            seq: 1,
        }
    }

    #[test]
    fn a_cursor_round_trips_only_for_its_own_order_and_filter() {
        let last = row(1_700_000_000_123, "msgx_a:b", Direction::Out);
        let cursor = encode(Order::Desc, Some(Direction::Out), &last);
        assert_eq!(
            decode(&cursor, Order::Desc, Some(Direction::Out)).unwrap(),
            After {
                created_ms: 1_700_000_000_123,
                message_id: "msgx_a:b".into(),
                direction: "out",
            }
        );
        for (order, direction) in [
            (Order::Asc, Some(Direction::Out)),
            (Order::Desc, None),
            (Order::Desc, Some(Direction::In)),
        ] {
            assert!(
                decode(&cursor, order, direction).is_err(),
                "{order:?} {direction:?}"
            );
        }
        for bad in [
            "",
            "d*",
            "d*:x:in:msgx_a",
            "d*:1:sideways:msgx_a",
            "d*:1:in:",
        ] {
            assert!(decode(bad, Order::Desc, None).is_err(), "{bad}");
        }
    }

    #[test]
    fn a_record_names_both_sides_and_formats_times() {
        let mut received = row(0, "msgx_in", Direction::In);
        received.updated_ms = 1_500;
        let record = MessageRecord::from(received);
        assert_eq!(
            (record.from.as_str(), record.to.as_str()),
            ("peer", "ses_a")
        );
        assert_eq!(record.created_at, "1970-01-01T00:00:00.000Z");
        assert_eq!(record.updated_at, "1970-01-01T00:00:01.500Z");
        let sent = MessageRecord::from(row(0, "msgx_out", Direction::Out));
        assert_eq!((sent.from.as_str(), sent.to.as_str()), ("ses_a", "peer"));
        let json = serde_json::to_value(&sent).unwrap();
        assert!(
            json.get("text").is_none(),
            "a refused record has no text: {json}"
        );
    }
}
