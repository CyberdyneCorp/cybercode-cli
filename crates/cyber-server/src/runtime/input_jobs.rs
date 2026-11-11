//! Bounded settlement lookup using complete durable Job input provenance.
use super::{Runtime, RuntimeError};
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InputJobQuery {
    pub session_id: String,
    pub input_message_ids: Vec<String>,
}

/// One small Job record per supplied input; no logs/results/history. Handback
/// admission is separate from the parent Turn processing that queued input.
#[derive(Debug, Serialize, Deserialize, schemars::JsonSchema)]
pub struct InputJobState {
    pub message_id: String,
    pub running: bool,
    /// An ended Job has not yet admitted its completion notice. False does not
    /// establish that the parent has processed an already admitted handback.
    pub awaiting_handback: bool,
    pub oldest_unnotified_ms: Option<i64>,
}

impl Runtime {
    pub fn input_job_state(
        &self,
        query: InputJobQuery,
    ) -> Result<Vec<InputJobState>, RuntimeError> {
        if query.input_message_ids.is_empty()
            || query.input_message_ids.len() > 32
            || query
                .input_message_ids
                .iter()
                .any(|id| id.is_empty() || id.len() > 256)
            || query
                .input_message_ids
                .iter()
                .collect::<std::collections::HashSet<_>>()
                .len()
                != query.input_message_ids.len()
        {
            return Err(RuntimeError::Invalid(
                "Expected 1–32 unique input identities of at most 256 bytes".into(),
            ));
        }
        let ids = serde_json::to_string(&query.input_message_ids)
            .map_err(|error| RuntimeError::Invalid(error.to_string()))?;
        self.inner
            .store
            .read(move |db| states(db, &query.session_id, &ids))
            .map_err(Into::into)
    }
}

fn states(
    db: &Connection,
    session: &str,
    ids: &str,
) -> Result<Vec<InputJobState>, cyber_store::StoreError> {
    let mut statement = db.prepare(
        "SELECT input.value,
            coalesce(max(j.status='running'),0),
            coalesce(max(j.status<>'running' AND json_extract(j.data,'$.notified')=0),0),
            min(CASE WHEN j.status<>'running' AND json_extract(j.data,'$.notified')=0
                THEN coalesce(json_extract(j.data,'$.ended_ms'),json_extract(j.data,'$.started_ms')) END)
        FROM json_each(?2) input LEFT JOIN job j ON j.session_id=?1
            AND json_type(j.data,'$.input_origin.input_message_ids')='array'
            AND json_array_length(j.data,'$.input_origin.input_message_ids') BETWEEN 1 AND 32
            AND json_type(j.data,'$.input_origin.message_id')='text'
            AND length(json_extract(j.data,'$.input_origin.message_id'))>0
            AND json_type(j.data,'$.input_origin.call_id')='text'
            AND length(json_extract(j.data,'$.input_origin.call_id'))>0
            AND json_type(j.data,'$.input_origin.operation_key')='text'
            AND length(json_extract(j.data,'$.input_origin.operation_key'))>0
            AND json_type(j.data,'$.input_origin.attempt')='integer'
            AND json_extract(j.data,'$.input_origin.attempt') BETWEEN 1 AND 4294967295
            AND EXISTS (SELECT 1 FROM json_each(j.data,'$.input_origin.input_message_ids') owner
                WHERE owner.type='text' AND owner.value=input.value)
            AND NOT EXISTS (SELECT 1 FROM json_each(j.data,'$.input_origin.input_message_ids') owner
                WHERE owner.type<>'text' OR length(CAST(owner.value AS BLOB)) NOT BETWEEN 1 AND 256
                OR NOT EXISTS (SELECT 1 FROM json_each(?2) supplied WHERE supplied.value=owner.value))
            AND (SELECT count(DISTINCT owner.value) FROM json_each(j.data,'$.input_origin.input_message_ids') owner)
                =json_array_length(j.data,'$.input_origin.input_message_ids')
        GROUP BY input.key ORDER BY input.key",
    )?;
    let rows = statement.query_map(params![session, ids], |row| {
        Ok(InputJobState {
            message_id: row.get(0)?,
            running: row.get(1)?,
            awaiting_handback: row.get(2)?,
            oldest_unnotified_ms: row.get(3)?,
        })
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    fn database() -> Connection {
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch(
            "CREATE TABLE job(id TEXT PRIMARY KEY,session_id TEXT,status TEXT,data TEXT);
            CREATE INDEX job_session ON job(session_id)",
        )
        .unwrap();
        db
    }
    fn origin(ids: Value) -> Value {
        json!({"message_id":"assistant", "call_id":"call", "attempt":1,
            "operation_key":"session:background", "input_message_ids":ids})
    }
    fn insert(
        db: &Connection,
        id: &str,
        session: &str,
        status: &str,
        notified: bool,
        origin: Value,
    ) {
        let data = json!({"input_origin":origin,"notified":notified,"started_ms":10,"ended_ms":20,
            "result":{"text":"large output".repeat(10_000)}});
        db.execute(
            "INSERT INTO job VALUES(?1,?2,?3,?4)",
            params![id, session, status, data.to_string()],
        )
        .unwrap();
    }
    fn lookup(db: &Connection, ids: Value) -> Vec<InputJobState> {
        states(db, "session", &ids.to_string()).unwrap()
    }

    #[test]
    fn full_cohort_and_session_are_required_before_any_job_is_awaited() {
        let db = database();
        insert(
            &db,
            "own",
            "session",
            "running",
            false,
            origin(json!(["one", "two"])),
        );
        insert(
            &db,
            "foreign-session",
            "foreign",
            "running",
            false,
            origin(json!(["three"])),
        );
        insert(
            &db,
            "mixed",
            "session",
            "running",
            false,
            origin(json!(["three", "foreign"])),
        );
        insert(&db, "legacy", "session", "running", false, Value::Null);
        for state in lookup(&db, json!(["one", "three"])) {
            assert!(!state.running && !state.awaiting_handback);
        }
        let states = lookup(&db, json!(["three", "two", "one"]));
        assert_eq!(
            states
                .iter()
                .map(|state| state.message_id.as_str())
                .collect::<Vec<_>>(),
            ["three", "two", "one"]
        );
        assert!(!states[0].running);
        assert!(states[1].running && states[2].running);
    }

    #[test]
    fn malformed_or_oversized_provenance_never_becomes_partial_ownership() {
        let db = database();
        let valid = origin(json!(["one"]));
        let mut origins = vec![
            origin(json!([])),
            origin(json!(["one", 1])),
            origin(json!(["one", "one"])),
            origin(json!(["one", "x".repeat(257)])),
            origin(json!(vec!["one"; 33])),
        ];
        for (field, value) in [
            ("message_id", Value::Null),
            ("call_id", json!("")),
            ("attempt", json!(0)),
            ("attempt", json!(1.5)),
            ("operation_key", json!(false)),
        ] {
            let mut malformed = valid.clone();
            malformed[field] = value;
            origins.push(malformed);
        }
        for (index, origin) in origins.into_iter().enumerate() {
            insert(
                &db,
                &format!("bad-{index}"),
                "session",
                "running",
                false,
                origin,
            );
        }
        let result = lookup(&db, json!(["one"]));
        assert_eq!(result.len(), 1);
        assert!(!result[0].running && !result[0].awaiting_handback);
    }

    #[test]
    fn settlement_lookup_is_small_and_preserves_unadmitted_handback_time() {
        let db = database();
        for index in 0..40 {
            insert(
                &db,
                &format!("done-{index}"),
                "session",
                "completed",
                true,
                origin(json!(["one"])),
            );
        }
        insert(
            &db,
            "waiting",
            "session",
            "error",
            false,
            origin(json!(["two"])),
        );
        let result = lookup(&db, json!(["one", "two", "three"]));
        assert!(!result[0].running && !result[0].awaiting_handback);
        assert!(!result[1].running && result[1].awaiting_handback);
        assert_eq!(result[1].oldest_unnotified_ms, Some(20));
        assert_eq!(result[2].oldest_unnotified_ms, None);
        assert!(serde_json::to_vec(&result).unwrap().len() < 512);
        db.execute(
            "UPDATE job SET data=json_set(data,'$.notified',json('true')) WHERE id='waiting'",
            [],
        )
        .unwrap();
        assert!(!lookup(&db, json!(["two"]))[0].awaiting_handback);
    }
}
