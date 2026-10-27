//! Usage attribution (`cross-session-messaging` → Usage attribution): each provider step
//! carries the origin of the input its Session promoted last, so cost reports can separate
//! user-driven usage from message-, client-, MCP- and Job-driven usage.

use cyber_store::{Store, StoreError};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

use super::model::{InputStatus, SessionState};

/// The origin of a step and of the usage recorded inside its Turn. Both fields are absent
/// on records made before origins existed; those read as `user`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StepOrigin {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<String>,
    /// The MCP client label, for origin `mcp`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mcp_client: Option<String>,
}

impl StepOrigin {
    fn of(origin: &str) -> Self {
        Self {
            origin: Some(origin.to_owned()),
            mcp_client: None,
        }
    }
}

/// Handback rows the runtime admits with source `session` (Job and forked-skill results).
const HANDBACKS: [&str; 2] = ["msg_handback_", "msg_skill_handback_"];

/// The promoted input a step answers.
struct Input {
    session_id: String,
    parent_id: Option<String>,
    message_id: String,
    source: String,
}

/// The origin of the next step of `state`: that of its most recently promoted input.
/// A Session that promoted nothing (a fork answering copied history) is `user`.
pub(crate) fn step_origin(store: &Store, state: &SessionState) -> Result<StepOrigin, StoreError> {
    let latest = state
        .inbox
        .iter()
        .filter(|row| row.status == InputStatus::Promoted)
        .filter_map(|row| row.promoted_seq.map(|seq| (seq, row)))
        .max_by_key(|(seq, _)| *seq)
        .map(|(_, row)| row);
    let Some(row) = latest else {
        return Ok(StepOrigin::of("user"));
    };
    let input = Input {
        session_id: state.info.id.clone(),
        parent_id: state.info.parent_id.clone(),
        message_id: row.message_id.clone(),
        source: row.source.clone(),
    };
    store.read(move |db| Ok(classify(db, &input)?))
}

/// Recover an input's immutable origin before or after its first step starts.
/// A known message never borrows attribution from a later foreground input.
pub(crate) fn recorded_message_origin(
    db: &Connection,
    session: &str,
    message: &str,
) -> Result<Option<StepOrigin>, StoreError> {
    let started: Option<String> = db.query_row(
        "SELECT data FROM event WHERE aggregate_id=?1 AND type='session.step.started.1' AND json_extract(data,'$.message_id')=?2 ORDER BY seq DESC LIMIT 1",
        params![session,message], |row|row.get(0),
    ).optional()?;
    if let Some(started) = started {
        return serde_json::from_str(&started).map(Some).map_err(|_| {
            StoreError::Unavailable("reserved request origin evidence is corrupt".into())
        });
    }
    let row: Option<(Option<String>,String)> = db.query_row(
        "SELECT s.parent_id,i.source FROM session_input i JOIN session s ON s.id=i.session_id WHERE i.session_id=?1 AND i.message_id=?2",
        params![session,message], |row|Ok((row.get(0)?,row.get(1)?)),
    ).optional()?;
    if let Some((parent_id, source)) = row {
        return Ok(Some(classify(
            db,
            &Input {
                session_id: session.into(),
                parent_id,
                message_id: message.into(),
                source,
            },
        )?));
    }
    let client: Option<String> = db
        .query_row(
            "SELECT mcp_client FROM mcp_serve_input WHERE session_id=?1 AND message_id=?2",
            params![session, message],
            |row| row.get(0),
        )
        .optional()?;
    Ok(client.map(|client| StepOrigin {
        origin: Some("mcp".into()),
        mcp_client: Some(client),
    }))
}

/// MCP input wins over the messaging kinds; any source but `session` is its own origin.
fn classify(db: &Connection, input: &Input) -> rusqlite::Result<StepOrigin> {
    let mcp: Option<String> = db
        .query_row(
            "SELECT mcp_client FROM mcp_serve_input WHERE message_id = ?1 AND session_id = ?2",
            params![input.message_id, input.session_id],
            |r| r.get(0),
        )
        .optional()?;
    if let Some(client) = mcp {
        return Ok(StepOrigin {
            origin: Some("mcp".into()),
            mcp_client: Some(client),
        });
    }
    if input.source != "session" {
        return Ok(StepOrigin::of(&input.source));
    }
    session_input(db, input)
}

/// A `session` row is a message when the Session holds an `in` record for it (`client`
/// when sent through the Messaging API or the CLI), a watch notice when one of its
/// watches fired it, a background handback (`job`) by its ID, and otherwise the prompt a
/// parent delegated to its subagent, which inherits the parent's current step origin.
fn session_input(db: &Connection, input: &Input) -> rusqlite::Result<StepOrigin> {
    let (session_id, message_id) = (&input.session_id, &input.message_id);
    let kind: Option<String> = db
        .query_row(
            "SELECT origin_kind FROM xsession_message
             WHERE session_id = ?1 AND message_id = ?2 AND direction = 'in'",
            params![session_id, message_id],
            |r| r.get(0),
        )
        .optional()?;
    match kind.as_deref() {
        Some("client") => return Ok(StepOrigin::of("client")),
        // `session`, and the open set of Session-sent kinds (direct peers add `peer`).
        Some(_) => return Ok(StepOrigin::of("session")),
        None => {}
    }
    if notice(db, session_id, message_id)? {
        return Ok(StepOrigin::of("session"));
    }
    let handback = HANDBACKS.iter().any(|p| message_id.starts_with(p));
    match &input.parent_id {
        Some(parent) if !handback => parent_origin(db, parent),
        _ => Ok(StepOrigin::of("job")),
    }
}

/// Whether `message_id` is the idle notice of one of the Session's watches.
fn notice(db: &Connection, session_id: &str, message_id: &str) -> rusqlite::Result<bool> {
    db.query_row(
        "SELECT EXISTS(SELECT 1 FROM messaging_watch WHERE subscriber_id = ?1 AND notice_id = ?2)",
        params![session_id, message_id],
        |r| r.get(0),
    )
}

/// The origin of the parent's latest started step; `user` before origins existed.
fn parent_origin(db: &Connection, parent: &str) -> rusqlite::Result<StepOrigin> {
    let found: Option<(Option<String>, Option<String>)> = db
        .query_row(
            "SELECT json_extract(data, '$.origin'), json_extract(data, '$.mcp_client')
             FROM event WHERE aggregate_id = ?1 AND type = ?2 ORDER BY seq DESC LIMIT 1",
            params![parent, super::events::STEP_STARTED],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    Ok(match found {
        Some((Some(origin), mcp_client)) => StepOrigin {
            origin: Some(origin),
            mcp_client,
        },
        _ => StepOrigin::of("user"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db() -> Connection {
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch(
            "CREATE TABLE mcp_serve_input (message_id TEXT, session_id TEXT, mcp_client TEXT);
             CREATE TABLE xsession_message (session_id TEXT, message_id TEXT, direction TEXT,
                origin_kind TEXT);
             CREATE TABLE messaging_watch (subscriber_id TEXT, notice_id TEXT);
             INSERT INTO mcp_serve_input VALUES ('msg_mcp', 'ses_b', 'claude-code'),
                ('msgx_mcp', 'ses_b', 'claude-code');
             INSERT INTO xsession_message VALUES ('ses_b', 'msgx_s', 'in', 'session'),
                ('ses_b', 'msgx_c', 'in', 'client'), ('ses_b', 'msgx_p', 'in', 'peer'),
                ('ses_b', 'msgx_mcp', 'in', 'client'), ('ses_a', 'msgx_o', 'out', 'session');
             INSERT INTO messaging_watch VALUES ('ses_b', 'msgx_watch_w1');
             CREATE TABLE event (aggregate_id TEXT, seq INTEGER, type TEXT, data TEXT);
             INSERT INTO event VALUES
                ('ses_p', 1, 'session.step.started.1', '{\"origin\":\"user\"}'),
                ('ses_p', 2, 'session.step.started.1',
                    '{\"origin\":\"mcp\",\"mcp_client\":\"zed\"}'),
                ('ses_old', 1, 'session.step.started.1', '{}');",
        )
        .unwrap();
        db
    }

    fn origin_in(
        parent: Option<&str>,
        message_id: &str,
        source: &str,
    ) -> (Option<String>, Option<String>) {
        let input = Input {
            session_id: "ses_b".into(),
            parent_id: parent.map(str::to_owned),
            message_id: message_id.into(),
            source: source.into(),
        };
        let o = classify(&db(), &input).unwrap();
        (o.origin, o.mcp_client)
    }

    fn origin(message_id: &str, source: &str) -> (Option<String>, Option<String>) {
        origin_in(None, message_id, source)
    }

    fn just(origin: &str) -> (Option<String>, Option<String>) {
        (Some(origin.into()), None)
    }

    #[test]
    fn promoted_inputs_map_to_their_origin() {
        assert_eq!(origin("msg_u", "user"), just("user"));
        assert_eq!(origin("msgx_s", "session"), just("session"));
        assert_eq!(origin("msgx_p", "session"), just("session"));
        assert_eq!(origin("msgx_c", "session"), just("client"));
        assert_eq!(origin("msgx_watch_w1", "session"), just("session"));
        assert_eq!(origin("msg_job", "session"), just("job"));
        // Another Session's `out` record is not this Session's message.
        assert_eq!(origin("msgx_o", "session"), just("job"));
        for source in ["workflow", "hook", "loop", "goal", "channel"] {
            assert_eq!(origin("msg_x", source), just(source));
        }
        // MCP input wins over the messaging kinds.
        let mcp = (Some("mcp".into()), Some("claude-code".into()));
        assert_eq!(origin("msg_mcp", "user"), mcp);
        assert_eq!(origin("msgx_mcp", "session"), mcp);
    }

    /// A subagent's delegated prompt carries its parent's current step origin; its Job
    /// and forked-skill handbacks stay `job`, and a parent without origins reads `user`.
    #[test]
    fn delegated_prompts_inherit_the_parent_origin() {
        let mcp = (Some("mcp".into()), Some("zed".into()));
        assert_eq!(origin_in(Some("ses_p"), "msg_task", "session"), mcp);
        assert_eq!(
            origin_in(Some("ses_old"), "msg_task", "session"),
            just("user")
        );
        assert_eq!(
            origin_in(Some("ses_none"), "msg_task", "session"),
            just("user")
        );
        for handback in ["msg_handback_job_1", "msg_skill_handback_dlg_1"] {
            assert_eq!(origin_in(Some("ses_p"), handback, "session"), just("job"));
        }
        assert_eq!(
            origin_in(Some("ses_p"), "msgx_c", "session"),
            just("client")
        );
    }
}
