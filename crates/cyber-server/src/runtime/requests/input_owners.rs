//! Capture branch-aware input provenance before inference; never infer it later.
use super::super::{InputStatus, SessionState};
use cyber_store::StoreError;
use rusqlite::{Connection, OptionalExtension, params};

const MAX_INPUTS: usize = 32;
const MAX_ID_BYTES: usize = 256;
const MAX_ENCODED: usize = MAX_INPUTS * (MAX_ID_BYTES * 6 + 3) + 2;

fn active_seq(state: &SessionState, id: &str) -> Option<i64> {
    state
        .input(id)
        .filter(|row| row.status == InputStatus::Promoted && !row.forwarded_skill)
        .and_then(|row| row.promoted_seq)
}
fn bounded(inputs: &[String]) -> bool {
    !inputs.is_empty()
        && inputs.len() <= MAX_INPUTS
        && inputs
            .iter()
            .all(|id| !id.is_empty() && id.len() <= MAX_ID_BYTES)
}
pub(crate) fn validate(state: &SessionState, owners: &[String]) -> Result<(), String> {
    if !owners.is_empty() && owners != capture(state) {
        return Err("Step input provenance differs from its active path".into());
    }
    Ok(())
}
/// Active-path rows and step snapshots survive branch changes and compaction.
/// A legacy continuation without recorded provenance remains unknown.
pub(crate) fn current(state: &SessionState) -> Option<(i64, &str)> {
    state
        .inbox
        .iter()
        .filter(|row| row.status == InputStatus::Promoted && !row.forwarded_skill)
        .filter_map(|row| row.promoted_seq.map(|seq| (seq, row.message_id.as_str())))
        .max_by_key(|(seq, _)| *seq)
}
pub(crate) fn capture(state: &SessionState) -> Vec<String> {
    let Some((seq, current)) = current(state) else {
        return Vec::new();
    };
    let previous = state.steps.last();
    let floor = match previous.and_then(|step| step.input_root.as_deref()) {
        Some(id) if id == current => {
            let owners = previous
                .map(|step| step.input_message_ids.as_slice())
                .unwrap_or_default();
            return if bounded(owners) && owners.iter().all(|id| active_seq(state, id).is_some()) {
                owners.to_vec()
            } else {
                Vec::new()
            };
        }
        Some(id) => match active_seq(state, id) {
            Some(value) => value,
            None => return Vec::new(),
        },
        None => -1,
    };
    collect(state, floor, seq)
}
fn collect(state: &SessionState, floor: i64, through: i64) -> Vec<String> {
    let mut owners: Vec<_> = state
        .inbox
        .iter()
        .filter(|row| !row.forwarded_skill && row.status == InputStatus::Promoted)
        .filter_map(|row| row.promoted_seq.map(|seq| (seq, &row.message_id)))
        .filter(|(seq, _)| *seq > floor && *seq <= through)
        .take(MAX_INPUTS + 1)
        .collect();
    if owners.len() > MAX_INPUTS {
        return Vec::new();
    }
    owners.sort_by_key(|(seq, _)| *seq);
    let owners: Vec<_> = owners.into_iter().map(|(_, id)| id.clone()).collect();
    if bounded(&owners) { owners } else { Vec::new() }
}

pub(crate) fn recorded(
    db: &Connection,
    session: &str,
    message: &str,
    call: &str,
) -> Result<Vec<String>, StoreError> {
    let raw: Option<Option<String>> = db
        .query_row(
            "SELECT CASE WHEN json_type(s.data,'$.input_message_ids')='array'
            AND json_array_length(s.data,'$.input_message_ids')<=?4
            AND length(CAST(json_extract(s.data,'$.input_message_ids') AS BLOB))<=?5
            THEN json_extract(s.data,'$.input_message_ids') ELSE NULL END
        FROM event s WHERE s.aggregate_id=?1 AND s.type='session.step.started.1'
        AND json_extract(s.data,'$.message_id')=?2
        AND EXISTS (SELECT 1 FROM event c WHERE c.aggregate_id=s.aggregate_id
            AND c.type='session.tool.called.1' AND c.seq>s.seq
            AND json_extract(c.data,'$.message_id')=?2 AND json_extract(c.data,'$.call_id')=?3)
        ORDER BY s.seq DESC LIMIT 1",
            params![
                session,
                message,
                call,
                MAX_INPUTS as i64,
                MAX_ENCODED as i64
            ],
            |row| row.get(0),
        )
        .optional()?;
    let Some(raw) = raw.flatten() else {
        return Ok(Vec::new());
    };
    let owners: Vec<String> = serde_json::from_str(&raw)
        .map_err(|_| StoreError::Unavailable("request input provenance is malformed".into()))?;
    Ok(if bounded(&owners) { owners } else { Vec::new() })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn database() -> Connection {
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch("CREATE TABLE event(aggregate_id TEXT,seq INTEGER,type TEXT,data TEXT)")
            .unwrap();
        db
    }
    fn insert(db: &Connection, seq: i64, kind: &str, data: serde_json::Value) {
        db.execute(
            "INSERT INTO event VALUES('session',?1,?2,?3)",
            params![seq, kind, data.to_string()],
        )
        .unwrap();
    }
    fn promote(db: &Connection, seq: i64, id: &str) {
        insert(
            db,
            seq,
            "session.prompt.promoted.1",
            json!({"message_id":id}),
        );
    }
    fn step(db: &Connection, seq: i64, id: &str, call: &str, owners: serde_json::Value) {
        insert(
            db,
            seq,
            "session.step.started.1",
            json!({"message_id":id,"input_message_ids":owners}),
        );
        insert(
            db,
            seq + 1,
            "session.tool.called.1",
            json!({"message_id":id,"call_id":call}),
        );
    }
    #[test]
    fn exact_call_keeps_its_cohort_across_continuations_and_later_steering() {
        let db = database();
        promote(&db, 1, "first");
        promote(&db, 2, "second");
        step(&db, 3, "assistant_a", "call_a", json!(["first", "second"]));
        step(&db, 5, "assistant_b", "call_b", json!(["first", "second"]));
        promote(&db, 7, "foreign");
        step(&db, 8, "assistant_c", "call_c", json!(["foreign"]));
        for (message, call) in [("assistant_a", "call_a"), ("assistant_b", "call_b")] {
            assert_eq!(
                recorded(&db, "session", message, call).unwrap(),
                ["first", "second"]
            );
        }
        assert_eq!(
            recorded(&db, "session", "assistant_c", "call_c").unwrap(),
            ["foreign"]
        );
        for (session, message, call) in [
            ("other", "assistant_a", "call_a"),
            ("session", "assistant_a", "call_b"),
            ("session", "first", "call_a"),
        ] {
            assert!(recorded(&db, session, message, call).unwrap().is_empty());
        }
    }
    #[test]
    fn excessive_or_unbound_evidence_never_becomes_partial_ownership() {
        for count in [0, MAX_INPUTS, MAX_INPUTS + 1] {
            let db = database();
            for index in 0..count {
                promote(&db, index as i64, &format!("input_{index}"));
            }
            step(
                &db,
                100,
                "assistant",
                "call",
                json!(
                    (0..count)
                        .map(|index| format!("input_{index}"))
                        .collect::<Vec<_>>()
                ),
            );
            let owners = recorded(&db, "session", "assistant", "call").unwrap();
            assert_eq!(
                owners.len(),
                if count == MAX_INPUTS { MAX_INPUTS } else { 0 }
            );
        }
        let db = database();
        promote(&db, 1, &"x".repeat(MAX_ID_BYTES + 1));
        step(
            &db,
            2,
            "assistant",
            "call",
            json!(["x".repeat(MAX_ID_BYTES + 1)]),
        );
        assert!(
            recorded(&db, "session", "assistant", "call")
                .unwrap()
                .is_empty()
        );
    }
    #[test]
    fn malformed_cohort_is_an_error_and_legacy_requests_remain_unknown() {
        let db = database();
        insert(
            &db,
            1,
            "session.prompt.promoted.1",
            json!({"foreign":"unknown"}),
        );
        step(&db, 2, "assistant", "call", json!([true]));
        assert!(recorded(&db, "session", "assistant", "call").is_err());
        let request: super::super::PendingRequest=serde_json::from_value(json!({"id":"per_old","session_id":"session","message_id":"assistant","call_id":"call","kind":"permission","action":"bash","resources":[],"always_patterns":[]})).unwrap();
        assert!(request.input_message_ids.is_empty());
        assert!(
            serde_json::to_value(request)
                .unwrap()
                .get("input_message_ids")
                .is_none()
        );
    }
}
