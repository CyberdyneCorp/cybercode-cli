//! Cross-session messages as the TUI shows them (`tui` → Cross-session message
//! surfaces): the Session's message records from `GET /sessions/:id/messaging`, the
//! idle notices in its transcript, the held cards above the composer and the
//! `/messages` view. Everything here is rebuilt from durable routes on every snapshot,
//! so it survives a reconnect.
//!
//! Message text, sender addresses and notice statuses come from other Sessions: they
//! are untrusted and are shown without control characters.

use crossterm::event::{KeyCode, KeyEvent};
use serde_json::Value;

use crate::app::{Action, App, Overlay};
use crate::model::{Item, Queued};

/// Prefix of the notices an idle subscription admits.
const NOTICE_PREFIX: &str = "msgx_watch_";

/// One side of a message (`MessageRecord`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Record {
    pub message_id: String,
    /// `in` (received) or `out` (sent).
    pub incoming: bool,
    pub from: String,
    pub to: String,
    pub peer_session_id: Option<String>,
    /// `delivered`, `held`, `refused`, `released` or `dropped`.
    pub outcome: String,
    pub reason: Option<String>,
    pub size: u64,
    pub sha256: String,
    /// `None` for a refused message, which keeps no text.
    pub text: Option<String>,
    pub created_ms: i64,
    pub updated_ms: i64,
}

impl Record {
    pub fn parse(v: &Value) -> Option<Self> {
        let s = |k: &str| v[k].as_str().unwrap_or_default().to_owned();
        Some(Self {
            message_id: v["message_id"].as_str()?.to_owned(),
            incoming: match v["direction"].as_str()? {
                "in" => true,
                "out" => false,
                _ => return None,
            },
            from: s("from"),
            to: s("to"),
            peer_session_id: v["peer_session_id"].as_str().map(str::to_owned),
            outcome: s("outcome"),
            reason: v["reason"].as_str().map(str::to_owned),
            size: v["size"].as_u64().unwrap_or(0),
            sha256: s("sha256"),
            text: v["text"].as_str().map(str::to_owned),
            created_ms: millis(&v["created_at"]),
            updated_ms: millis(&v["updated_at"]),
        })
    }

    /// The other side's address: the sender of a received message, the target of a
    /// sent one.
    pub fn peer(&self) -> &str {
        if self.incoming { &self.from } else { &self.to }
    }

    /// `delivered`, or `refused: inbound_rule`.
    pub fn outcome_label(&self) -> String {
        outcome_label(&self.outcome, self.reason.as_deref())
    }
}

pub fn outcome_label(outcome: &str, reason: Option<&str>) -> String {
    match reason {
        Some(reason) if outcome == "refused" || outcome == "dropped" => {
            format!("{outcome}: {reason}")
        }
        _ => outcome.to_owned(),
    }
}

/// An RFC 3339 time as Unix milliseconds; 0 when it does not parse.
fn millis(v: &Value) -> i64 {
    v.as_str()
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .map_or(0, |t| t.timestamp_millis())
}

/// An idle notice or an expired subscription in the transcript.
#[derive(Debug, Clone, PartialEq)]
pub struct Notice {
    pub target: String,
    /// When the target's Turn finished; `None` for an expiry.
    pub finished_ms: Option<i64>,
    /// The first line of the target's last run, `failed: …`, `interrupted` or `idle`.
    pub status: String,
    pub expired: bool,
}

impl Notice {
    /// The notice a transcript item is, when it is one: only inbox rows the server names
    /// `msgx_watch_…` and whose text is a notice element.
    pub fn of(item: &Item) -> Option<Self> {
        let Item::User { id, text, .. } = item else {
            return None;
        };
        if !id.starts_with(NOTICE_PREFIX) {
            return None;
        }
        let head = text.lines().next()?;
        if head.starts_with("<session_idle_notice ") {
            return Some(Self {
                target: attribute(head, "target")?,
                finished_ms: attribute(head, "finished_at")
                    .and_then(|t| chrono::DateTime::parse_from_rfc3339(&t).ok())
                    .map(|t| t.timestamp_millis()),
                status: attribute(head, "status").unwrap_or_default(),
                expired: false,
            });
        }
        head.starts_with("<session_watch_expired ").then(|| Self {
            target: attribute(head, "target").unwrap_or_default(),
            finished_ms: None,
            status: "subscription expired".into(),
            expired: true,
        })
    }
}

/// The unescaped value of `name="…"` in a notice element.
fn attribute(element: &str, name: &str) -> Option<String> {
    let start = element.find(&format!(" {name}=\""))? + name.len() + 3;
    let rest = &element[start..];
    let value = &rest[..rest.find('"')?];
    Some(clean(
        &value
            .replace("&quot;", "\"")
            .replace("&lt;", "<")
            .replace("&gt;", ">")
            .replace("&amp;", "&"),
    ))
}

/// One line of untrusted text without control characters; tabs become spaces.
pub fn clean(line: &str) -> String {
    line.chars()
        .map(|c| if c == '\t' { ' ' } else { c })
        .filter(|c| !c.is_control())
        .collect()
}

/// The `/messages` filter, cycled with Tab.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Filter {
    #[default]
    All,
    In,
    Out,
    Held,
}

impl Filter {
    pub const ALL: [Filter; 4] = [Filter::All, Filter::In, Filter::Out, Filter::Held];

    pub fn label(self) -> &'static str {
        match self {
            Filter::All => "all",
            Filter::In => "in",
            Filter::Out => "out",
            Filter::Held => "held",
        }
    }

    fn next(self) -> Self {
        match self {
            Filter::All => Filter::In,
            Filter::In => Filter::Out,
            Filter::Out => Filter::Held,
            Filter::Held => Filter::All,
        }
    }
}

/// One row of the `/messages` view.
#[derive(Debug, Clone, PartialEq)]
pub enum Row<'a> {
    Record(&'a Record),
    Notice { item_id: &'a str, notice: Notice },
}

impl Row<'_> {
    pub fn time_ms(&self) -> i64 {
        match self {
            Row::Record(record) => record.created_ms,
            Row::Notice { notice, .. } => notice.finished_ms.unwrap_or(0),
        }
    }

    pub fn id(&self) -> &str {
        match self {
            Row::Record(record) => &record.message_id,
            Row::Notice { item_id, .. } => item_id,
        }
    }
}

/// The Session's message state and the `/messages` view's selection.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Messages {
    /// Oldest first, both directions.
    pub records: Vec<Record>,
    pub filter: Filter,
    /// The selected row's message ID, kept across reloads.
    pub selected: Option<String>,
    pub expanded: bool,
    /// The held card shown, counted from the newest.
    pub card: usize,
    /// The held cards take the keys instead of the composer.
    pub card_focus: bool,
}

impl Messages {
    pub fn received(&self, message_id: &str) -> Option<&Record> {
        self.records
            .iter()
            .find(|r| r.incoming && r.message_id == message_id)
    }

    pub fn sent(&self, message_id: &str) -> Option<&Record> {
        self.records
            .iter()
            .find(|r| !r.incoming && r.message_id == message_id)
    }

    /// An inbox row is a received message when the Session has its `in` record; a
    /// prompt that only borrows a `msgx_` ID is not.
    pub fn is_message(&self, row: &Queued) -> bool {
        self.received(&row.message_id).is_some()
    }

    /// Held messages, oldest first, with their records.
    pub fn held<'a>(&'a self, queued: &'a [Queued]) -> Vec<(&'a Queued, &'a Record)> {
        queued
            .iter()
            .filter(|row| row.held)
            .filter_map(|row| Some((row, self.received(&row.message_id)?)))
            .collect()
    }

    /// `(queued, held)` received messages in the inbox, for the footer badge.
    pub fn counts(&self, queued: &[Queued]) -> (usize, usize) {
        let rows = queued.iter().filter(|row| self.is_message(row));
        rows.fold(
            (0, 0),
            |(q, h), row| {
                if row.held { (q, h + 1) } else { (q + 1, h) }
            },
        )
    }

    /// The held card shown: the newest unless the user moved.
    pub fn card<'a>(&'a self, queued: &'a [Queued]) -> Option<(usize, usize, &'a Record)> {
        let held = self.held(queued);
        let n = held.len();
        let at = n.checked_sub(1)? - self.card.min(n - 1);
        Some((at, n, held[at].1))
    }

    /// The view's rows under its filter, oldest first.
    pub fn rows<'a>(&'a self, items: &'a [Item], queued: &[Queued]) -> Vec<Row<'a>> {
        let held = |id: &str| queued.iter().any(|q| q.held && q.message_id == id);
        let mut rows: Vec<Row<'a>> = self
            .records
            .iter()
            .filter(|r| match self.filter {
                Filter::All => true,
                Filter::In => r.incoming,
                Filter::Out => !r.incoming,
                Filter::Held => r.incoming && held(&r.message_id),
            })
            .map(Row::Record)
            .collect();
        if matches!(self.filter, Filter::All | Filter::In) {
            rows.extend(items.iter().filter_map(|item| {
                Some(Row::Notice {
                    item_id: item.id(),
                    notice: Notice::of(item)?,
                })
            }));
        }
        rows.sort_by_key(Row::time_ms);
        rows
    }

    /// The selected row's index in `rows`; the last row when none is chosen.
    pub fn selected_index(&self, rows: &[Row]) -> Option<usize> {
        let last = rows.len().checked_sub(1)?;
        Some(
            self.selected
                .as_deref()
                .and_then(|id| rows.iter().position(|row| row.id() == id))
                .unwrap_or(last),
        )
    }

    /// Move the selection by `delta` rows of `ids` (the rows' IDs, in order).
    pub fn select(&mut self, ids: &[String], delta: isize) {
        let Some(last) = ids.len().checked_sub(1) else {
            return;
        };
        let at = self
            .selected
            .as_deref()
            .and_then(|id| ids.iter().position(|row| row == id))
            .unwrap_or(last);
        self.selected = Some(ids[at.saturating_add_signed(delta).min(last)].clone());
        self.expanded = false;
    }

    pub fn next_filter(&mut self) {
        self.filter = self.filter.next();
        self.selected = None;
        self.expanded = false;
    }

    /// Records from a snapshot; a failed read keeps the last ones.
    pub fn set_records(&mut self, records: Option<Vec<Record>>) {
        if let Some(records) = records {
            self.records = records;
        }
    }
}

impl App {
    /// `/messages`, the palette entry and `messages_view`.
    pub(crate) fn open_messages(&mut self) -> Vec<Action> {
        self.messaging.card_focus = false;
        self.overlay = Overlay::Messages;
        vec![Action::Refresh]
    }

    /// Keys over the held cards: Tab on an empty composer selects them; then `r`
    /// releases, `d` drops, Enter views the message, Up and Down move between cards,
    /// and Esc or Tab return to the composer. Any other key goes back to the composer,
    /// so typing never acts on a message.
    pub(crate) fn held_card_key(&mut self, key: KeyEvent) -> Option<Vec<Action>> {
        let card = self
            .messaging
            .card(&self.queued)
            .map(|(_, _, record)| record.message_id.clone());
        let Some(id) = card else {
            self.messaging.card_focus = false;
            return None;
        };
        if !self.messaging.card_focus {
            let select = key.code == KeyCode::Tab && self.composer.is_empty();
            self.messaging.card_focus = select;
            return select.then(Vec::new);
        }
        let actions = match key.code {
            KeyCode::Char('r') => self.settle_held(&id, true),
            KeyCode::Char('d') => self.settle_held(&id, false),
            KeyCode::Enter => {
                self.messaging.filter = Filter::All;
                self.messaging.selected = Some(id);
                self.messaging.expanded = true;
                self.open_messages()
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.messaging.card = self.messaging.card.saturating_add(1);
                Vec::new()
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.messaging.card = self.messaging.card.saturating_sub(1);
                Vec::new()
            }
            KeyCode::Esc | KeyCode::Tab => {
                self.messaging.card_focus = false;
                Vec::new()
            }
            _ => {
                self.messaging.card_focus = false;
                return None;
            }
        };
        Some(actions)
    }

    /// Release or drop a held message through the inbox routes. The row leaves the
    /// inbox at once; the next snapshot shows the server's state.
    fn settle_held(&mut self, id: &str, release: bool) -> Vec<Action> {
        let held = |row: &Queued| row.held && row.message_id == id;
        if !self.queued.iter().any(held) || self.messaging.received(id).is_none() {
            return Vec::new();
        }
        self.queued.retain(|row| !held(row));
        self.messaging.card = 0;
        if self.messaging.held(&self.queued).is_empty() {
            self.messaging.card_focus = false;
        }
        let action = if release {
            Action::ReleaseHeld(id.to_owned())
        } else {
            Action::RemoveQueued(id.to_owned())
        };
        vec![action, Action::Refresh]
    }

    /// Keys in `/messages`.
    pub(crate) fn messages_key(&mut self, key: KeyEvent) -> Vec<Action> {
        let rows = self.messaging.rows(&self.items, &self.queued);
        let ids: Vec<String> = rows.iter().map(|row| row.id().to_owned()).collect();
        let selected = self
            .messaging
            .selected_index(&rows)
            .and_then(|at| match &rows[at] {
                Row::Record(record) => Some(record.message_id.clone()),
                Row::Notice { .. } => None,
            });
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => self.messaging.select(&ids, -1),
            KeyCode::Down | KeyCode::Char('j') => self.messaging.select(&ids, 1),
            KeyCode::Enter => self.messaging.expanded = !self.messaging.expanded,
            KeyCode::Tab => self.messaging.next_filter(),
            KeyCode::Char('r') => {
                return selected.map_or_else(Vec::new, |id| self.settle_held(&id, true));
            }
            KeyCode::Char('d') => {
                return selected.map_or_else(Vec::new, |id| self.settle_held(&id, false));
            }
            KeyCode::Char('a')
                if self.session.inbound_reason.as_deref() == Some("non_interactive") =>
            {
                return vec![Action::AcceptMessages, Action::Refresh];
            }
            KeyCode::Esc | KeyCode::Char('q') => {
                self.messaging.expanded = false;
                self.overlay = Overlay::None;
            }
            _ => {}
        }
        Vec::new()
    }

    /// The footer badge: `✉ 2 queued · 1 held`, only while either is non-zero.
    pub(crate) fn messages_field(&self) -> Option<(String, bool)> {
        let (queued, held) = self.messaging.counts(&self.queued);
        let text = match (queued, held) {
            (0, 0) => return None,
            (q, 0) => format!("✉ {q} queued"),
            (0, h) => format!("✉ {h} held"),
            (q, h) => format!("✉ {q} queued · {h} held"),
        };
        Some((text, held > 0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn record(id: &str, incoming: bool, outcome: &str, ms: i64) -> Record {
        Record {
            message_id: id.into(),
            incoming,
            from: "frontend".into(),
            to: "backend".into(),
            outcome: outcome.into(),
            created_ms: ms,
            ..Record::default()
        }
    }

    fn row(id: &str, held: bool) -> Queued {
        Queued {
            message_id: id.into(),
            text: String::new(),
            delivery: if held { "hold" } else { "queue" }.into(),
            held,
        }
    }

    #[test]
    fn records_parse_from_the_history_route() {
        let v = json!({
            "message_id": "msgx_1", "session_id": "ses_b", "direction": "in",
            "from": "frontend", "to": "ses_b", "peer_session_id": "ses_a",
            "origin_kind": "session", "outcome": "refused", "reason": "inbound_rule",
            "size": 12, "sha256": "ab", "depth": 0, "hops": 1,
            "created_at": "2026-10-16T14:02:00.000Z", "updated_at": "2026-10-16T14:03:00.000Z",
            "seq": 4
        });
        let r = Record::parse(&v).unwrap();
        assert!(r.incoming && r.text.is_none());
        assert_eq!(r.peer(), "frontend");
        assert_eq!(r.outcome_label(), "refused: inbound_rule");
        assert_eq!(r.created_ms, 1_792_159_320_000);
        assert_eq!(r.updated_ms - r.created_ms, 60_000);
        assert!(Record::parse(&json!({ "message_id": "x", "direction": "sideways" })).is_none());
    }

    #[test]
    fn only_server_named_notices_are_notices() {
        let text = "<session_idle_notice target=\"migration\" finished_at=\"2026-10-16T14:40:00.000Z\" status=\"done &quot;12&quot;\x1b[2J\"/>\nThis is an idle notice.";
        let item = |id: &str| Item::User {
            id: id.into(),
            text: text.into(),
            promoted_ms: None,
        };
        let notice = Notice::of(&item("msgx_watch_w1")).unwrap();
        assert_eq!(notice.target, "migration");
        assert_eq!(notice.status, "done \"12\"[2J");
        assert_eq!(notice.finished_ms, Some(1_792_161_600_000));
        assert_eq!(
            Notice::of(&item("msg_user")),
            None,
            "a user prompt is not one"
        );
        let expired = Item::User {
            id: "msgx_watch_w2".into(),
            text: "<session_watch_expired target=\"b\" watch_id=\"w2\"/>\nexpired".into(),
            promoted_ms: None,
        };
        assert!(Notice::of(&expired).unwrap().expired);
    }

    #[test]
    fn counts_cards_and_rows_follow_the_records_and_the_inbox() {
        let messages = Messages {
            records: vec![
                record("msgx_a", true, "held", 1),
                record("msgx_b", true, "held", 2),
                record("msgx_c", true, "delivered", 3),
                record("msgx_d", false, "delivered", 4),
            ],
            ..Messages::default()
        };
        // `msgx_fake` is a prompt that borrowed the prefix: no record, no badge.
        let queued = vec![
            row("msgx_a", true),
            row("msgx_b", true),
            row("msgx_c", false),
            row("msgx_fake", true),
            row("msg_user", false),
        ];
        assert_eq!(messages.counts(&queued), (1, 2));
        let (at, n, newest) = messages.card(&queued).unwrap();
        assert_eq!((at, n, newest.message_id.as_str()), (1, 2, "msgx_b"));
        let older = Messages {
            card: 5,
            ..messages.clone()
        };
        assert_eq!(older.card(&queued).unwrap().2.message_id, "msgx_a");
        let mut view = messages.clone();
        let ids = |m: &Messages| -> Vec<String> {
            m.rows(&[], &queued)
                .iter()
                .map(|r| r.id().to_owned())
                .collect()
        };
        assert_eq!(ids(&view).len(), 4);
        view.next_filter();
        assert_eq!(ids(&view), ["msgx_a", "msgx_b", "msgx_c"]);
        view.next_filter();
        assert_eq!(ids(&view), ["msgx_d"]);
        view.next_filter();
        assert_eq!(ids(&view), ["msgx_a", "msgx_b"]);
        let rows = view.rows(&[], &queued);
        assert_eq!(view.selected_index(&rows), Some(1));
        let held = ids(&view);
        view.select(&held, -5);
        assert_eq!(view.selected.as_deref(), Some("msgx_a"));
    }
}
