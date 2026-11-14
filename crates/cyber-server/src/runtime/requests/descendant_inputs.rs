//! Root input provenance of descendant (subagent) requests: complete or nothing.
//!
//! A child Session's request inherits its root input cohort only when every link from
//! the asker up to the root Session is a background subagent Job whose immutable
//! `input_origin` is complete, bounded and identical across that link's Jobs. A missing
//! link (a foreground or detached child), a cycle, a mixed or malformed origin, or an
//! over-deep chain stamps nothing; this is provenance, never live attendance.
use super::super::jobs::JobInputOrigin;
use cyber_store::StoreError;
use rusqlite::{Connection, OptionalExtension, params};
use std::collections::HashSet;

const MAX_DEPTH: usize = 32;
/// More Jobs for one parent/child link than this is not a bounded proof.
const MAX_LINK_JOBS: usize = 8;
const MAX_INPUTS: usize = 32;
const MAX_ID_BYTES: usize = 256;
const MAX_ORIGIN_BYTES: i64 = 64 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RootCohort {
    pub session_id: String,
    pub input_message_ids: Vec<String>,
}

/// The root cohort of `session`'s subagent lineage, or `None` when any link is unproven.
pub(crate) fn recorded(db: &Connection, session: &str) -> Result<Option<RootCohort>, StoreError> {
    let mut child = session.to_owned();
    let mut seen = HashSet::from([child.clone()]);
    let mut cohort = None;
    for _ in 0..=MAX_DEPTH {
        let parent = match parent_of(db, &child)? {
            None => return Ok(None),
            Some(None) => return Ok(cohort),
            Some(Some(parent)) => parent,
        };
        if !seen.insert(parent.clone()) {
            return Ok(None);
        }
        let Some(ids) = link_cohort(db, &parent, &child)? else {
            return Ok(None);
        };
        cohort = Some(RootCohort {
            session_id: parent.clone(),
            input_message_ids: ids,
        });
        child = parent;
    }
    Ok(None)
}

/// `None` for a missing Session row, `Some(None)` for a root Session.
fn parent_of(db: &Connection, session: &str) -> Result<Option<Option<String>>, StoreError> {
    Ok(db
        .query_row(
            "SELECT parent_id FROM session WHERE id=?1",
            [session],
            |row| row.get(0),
        )
        .optional()?)
}

fn link_cohort(
    db: &Connection,
    parent: &str,
    child: &str,
) -> Result<Option<Vec<String>>, StoreError> {
    let mut statement = db.prepare(
        "SELECT CASE WHEN length(CAST(json_extract(data,'$.input_origin') AS BLOB))<=?3
            THEN json_extract(data,'$.input_origin') ELSE NULL END
        FROM job WHERE session_id=?1 AND child_id=?2
        AND json_extract(data,'$.kind')='subagent' ORDER BY id LIMIT ?4",
    )?;
    let origins = statement
        .query_map(
            params![parent, child, MAX_ORIGIN_BYTES, (MAX_LINK_JOBS + 1) as i64],
            |row| row.get::<_, Option<String>>(0),
        )?
        .collect::<Result<Vec<_>, _>>()?;
    if origins.is_empty() || origins.len() > MAX_LINK_JOBS {
        return Ok(None);
    }
    let mut cohort: Option<Vec<String>> = None;
    for raw in origins {
        let Some(ids) = raw.as_deref().and_then(valid_origin) else {
            return Ok(None);
        };
        if cohort.as_ref().is_some_and(|previous| previous != &ids) {
            return Ok(None);
        }
        cohort = Some(ids);
    }
    Ok(cohort)
}

fn valid_origin(raw: &str) -> Option<Vec<String>> {
    let origin: JobInputOrigin = serde_json::from_str(raw).ok()?;
    let ids = origin.input_message_ids;
    let complete = !origin.message_id.is_empty()
        && !origin.call_id.is_empty()
        && !origin.operation_key.is_empty()
        && origin.attempt >= 1
        && !ids.is_empty()
        && ids.len() <= MAX_INPUTS
        && ids
            .iter()
            .all(|id| !id.is_empty() && id.len() <= MAX_ID_BYTES)
        && ids.iter().collect::<HashSet<_>>().len() == ids.len();
    complete.then_some(ids)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    fn database() -> Connection {
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch(
            "CREATE TABLE session(id TEXT PRIMARY KEY,parent_id TEXT);
            CREATE TABLE job(id TEXT PRIMARY KEY,session_id TEXT,child_id TEXT,data TEXT)",
        )
        .unwrap();
        db
    }
    fn session(db: &Connection, id: &str, parent: Option<&str>) {
        db.execute("INSERT INTO session VALUES(?1,?2)", params![id, parent])
            .unwrap();
    }
    fn origin(ids: Value) -> Value {
        json!({"message_id":"assistant","call_id":"call","attempt":1,
            "operation_key":"session:background","input_message_ids":ids})
    }
    fn job(db: &Connection, id: &str, parent: &str, child: &str, origin: Value) {
        let data = json!({"kind":"subagent","input_origin":origin});
        db.execute(
            "INSERT INTO job VALUES(?1,?2,?3,?4)",
            params![id, parent, child, data.to_string()],
        )
        .unwrap();
    }
    fn chain() -> Connection {
        let db = database();
        session(&db, "root", None);
        session(&db, "child", Some("root"));
        session(&db, "grandchild", Some("child"));
        job(&db, "j1", "root", "child", origin(json!(["one", "two"])));
        job(&db, "j2", "child", "grandchild", origin(json!(["prompt"])));
        db
    }

    #[test]
    fn every_link_must_be_a_complete_subagent_job_and_the_root_cohort_wins() {
        let db = chain();
        let expected = RootCohort {
            session_id: "root".into(),
            input_message_ids: vec!["one".into(), "two".into()],
        };
        assert_eq!(recorded(&db, "grandchild").unwrap(), Some(expected.clone()));
        assert_eq!(recorded(&db, "child").unwrap(), Some(expected));
        assert_eq!(
            recorded(&db, "root").unwrap(),
            None,
            "a root is no descendant"
        );
        assert_eq!(recorded(&db, "missing").unwrap(), None);
        db.execute("DELETE FROM job WHERE id='j2'", []).unwrap();
        assert_eq!(recorded(&db, "grandchild").unwrap(), None, "detached link");
    }

    #[test]
    fn mixed_malformed_oversized_or_cyclic_provenance_stamps_nothing() {
        let mut cases = vec![
            origin(json!([])),
            origin(json!(["one", "one"])),
            origin(json!(["x".repeat(257)])),
            origin(json!(vec!["one"; 33])),
            origin(json!(["one", 1])),
            Value::Null,
        ];
        for (field, value) in [
            ("message_id", json!("")),
            ("call_id", Value::Null),
            ("attempt", json!(0)),
            ("operation_key", json!("")),
        ] {
            let mut bad = origin(json!(["one"]));
            bad[field] = value;
            cases.push(bad);
        }
        for bad in cases {
            let db = chain();
            db.execute("DELETE FROM job WHERE id='j1'", []).unwrap();
            job(&db, "j1", "root", "child", bad.clone());
            assert_eq!(recorded(&db, "grandchild").unwrap(), None, "{bad}");
        }
        let db = chain();
        job(&db, "j3", "root", "child", origin(json!(["three"])));
        assert_eq!(recorded(&db, "child").unwrap(), None, "mixed cohorts");
        let db = chain();
        job(&db, "j3", "root", "child", origin(json!(["one", "two"])));
        assert!(recorded(&db, "child").unwrap().is_some(), "identical retry");
        let db = chain();
        db.execute(
            "UPDATE session SET parent_id='grandchild' WHERE id='root'",
            [],
        )
        .unwrap();
        job(&db, "j0", "grandchild", "root", origin(json!(["one"])));
        assert_eq!(recorded(&db, "child").unwrap(), None, "cycle");
    }
}
