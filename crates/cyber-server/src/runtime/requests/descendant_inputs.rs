//! Root input provenance of descendant (subagent) requests: complete or nothing.
//!
//! A child Session's request inherits its root input cohort only when every link from
//! the asker up to the root Session is proven: a *running* background subagent Job whose
//! immutable `input_origin` is complete, bounded and identical across that link's running
//! Jobs, or a foreground `agent` call of the parent that is running in this server
//! process and lends its call's complete recorded cohort
//! ([`super::super::foreground_children`]). When both prove one link they must agree.
//! Only running work is driving the child: once a Job has settled (or a restart
//! interrupted it) or a foreground call returned, later activity in the child is not
//! that work, so it inherits nothing. A missing link (a detached or settled child), a
//! cycle, a mixed or malformed origin, or an over-deep chain stamps nothing; this is
//! provenance, never live attendance.
use super::super::foreground_children::Lent;
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
pub(crate) fn recorded(
    db: &Connection,
    session: &str,
    foreground: &Lent,
) -> Result<Option<RootCohort>, StoreError> {
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
        let Some(ids) = link_cohort(db, &parent, &child, foreground)? else {
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

/// The one cohort that every running proof of the `parent` → `child` link agrees on.
fn link_cohort(
    db: &Connection,
    parent: &str,
    child: &str,
    foreground: &Lent,
) -> Result<Option<Vec<String>>, StoreError> {
    let Some(origins) = running_job_origins(db, parent, child)? else {
        return Ok(None);
    };
    let lent = foreground.cohort(parent, child).map(<[String]>::to_vec);
    if origins.is_empty() && lent.is_none() {
        return Ok(None);
    }
    let mut cohort = lent;
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

/// The bounded raw origins of the link's running subagent Jobs; `None` when there
/// are more than a bounded proof allows.
fn running_job_origins(
    db: &Connection,
    parent: &str,
    child: &str,
) -> Result<Option<Vec<Option<String>>>, StoreError> {
    let mut statement = db.prepare(
        "SELECT CASE WHEN length(CAST(json_extract(data,'$.input_origin') AS BLOB))<=?3
            THEN json_extract(data,'$.input_origin') ELSE NULL END
        FROM job WHERE session_id=?1 AND child_id=?2 AND status='running'
        AND json_extract(data,'$.kind')='subagent' ORDER BY id LIMIT ?4",
    )?;
    let origins = statement
        .query_map(
            params![parent, child, MAX_ORIGIN_BYTES, (MAX_LINK_JOBS + 1) as i64],
            |row| row.get::<_, Option<String>>(0),
        )?
        .collect::<Result<Vec<_>, _>>()?;
    Ok((origins.len() <= MAX_LINK_JOBS).then_some(origins))
}

fn valid_origin(raw: &str) -> Option<Vec<String>> {
    let origin: JobInputOrigin = serde_json::from_str(raw).ok()?;
    complete(&origin).then_some(origin.input_message_ids)
}

/// Whether an origin names its exact dispatched call and a complete bounded cohort.
pub(crate) fn complete(origin: &JobInputOrigin) -> bool {
    let ids = &origin.input_message_ids;
    !origin.message_id.is_empty()
        && !origin.call_id.is_empty()
        && !origin.operation_key.is_empty()
        && origin.attempt >= 1
        && !ids.is_empty()
        && ids.len() <= MAX_INPUTS
        && ids
            .iter()
            .all(|id| !id.is_empty() && id.len() <= MAX_ID_BYTES)
        && ids.iter().collect::<HashSet<_>>().len() == ids.len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    fn database() -> Connection {
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch(
            "CREATE TABLE session(id TEXT PRIMARY KEY,parent_id TEXT);
            CREATE TABLE job(id TEXT PRIMARY KEY,session_id TEXT,child_id TEXT,status TEXT,
                data TEXT)",
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
        let data = json!({"kind":"subagent","status":"running","input_origin":origin});
        db.execute(
            "INSERT INTO job VALUES(?1,?2,?3,'running',?4)",
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
        assert_eq!(
            recorded(&db, "grandchild", &Lent::default()).unwrap(),
            Some(expected.clone())
        );
        assert_eq!(
            recorded(&db, "child", &Lent::default()).unwrap(),
            Some(expected)
        );
        assert_eq!(
            recorded(&db, "root", &Lent::default()).unwrap(),
            None,
            "a root is no descendant"
        );
        assert_eq!(recorded(&db, "missing", &Lent::default()).unwrap(), None);
        db.execute("DELETE FROM job WHERE id='j2'", []).unwrap();
        assert_eq!(
            recorded(&db, "grandchild", &Lent::default()).unwrap(),
            None,
            "detached link"
        );
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
            assert_eq!(
                recorded(&db, "grandchild", &Lent::default()).unwrap(),
                None,
                "{bad}"
            );
        }
        let db = chain();
        job(&db, "j3", "root", "child", origin(json!(["three"])));
        assert_eq!(
            recorded(&db, "child", &Lent::default()).unwrap(),
            None,
            "mixed cohorts"
        );
        let db = chain();
        job(&db, "j3", "root", "child", origin(json!(["one", "two"])));
        assert!(
            recorded(&db, "child", &Lent::default()).unwrap().is_some(),
            "identical retry"
        );
        let db = chain();
        db.execute(
            "UPDATE session SET parent_id='grandchild' WHERE id='root'",
            [],
        )
        .unwrap();
        job(&db, "j0", "grandchild", "root", origin(json!(["one"])));
        assert_eq!(
            recorded(&db, "child", &Lent::default()).unwrap(),
            None,
            "cycle"
        );
    }

    #[test]
    fn only_the_running_job_driving_a_child_lends_its_cohort() {
        let settle = |db: &Connection, id: &str, status: &str| {
            db.execute("UPDATE job SET status=?2 WHERE id=?1", params![id, status])
                .unwrap();
        };
        for status in ["completed", "error", "cancelled", "interrupted"] {
            let db = chain();
            settle(&db, "j1", status);
            assert_eq!(
                recorded(&db, "child", &Lent::default()).unwrap(),
                None,
                "a {status} Job kept stamping a later resume"
            );
            assert_eq!(
                recorded(&db, "grandchild", &Lent::default()).unwrap(),
                None,
                "{status}"
            );
        }
        // A background resume: the settled Job's old cohort yields to the running one.
        let db = chain();
        settle(&db, "j1", "completed");
        job(&db, "j3", "root", "child", origin(json!(["resumed"])));
        assert_eq!(
            recorded(&db, "grandchild", &Lent::default()).unwrap(),
            Some(RootCohort {
                session_id: "root".into(),
                input_message_ids: vec!["resumed".into()],
            })
        );
    }

    #[test]
    fn a_live_foreground_call_lends_its_cohort_and_must_agree_with_running_jobs() {
        let db = chain();
        db.execute("DELETE FROM job WHERE id='j2'", []).unwrap();
        let none = Lent::default();
        assert_eq!(recorded(&db, "grandchild", &none).unwrap(), None);
        let root = RootCohort {
            session_id: "root".into(),
            input_message_ids: vec!["one".into(), "two".into()],
        };
        // child -> grandchild is a foreground call: the root cohort passes through it.
        let lent = Lent::default().with("child", "grandchild", &["prompt"]);
        assert_eq!(
            recorded(&db, "grandchild", &lent).unwrap(),
            Some(root.clone())
        );
        // A foreground link of another parent proves nothing for this child.
        let foreign = Lent::default().with("root", "grandchild", &["prompt"]);
        assert_eq!(recorded(&db, "grandchild", &foreign).unwrap(), None);
        // A foreground root link alone proves the chain's top.
        db.execute("DELETE FROM job WHERE id='j1'", []).unwrap();
        let both = lent.clone().with("root", "child", &["fg"]);
        assert_eq!(
            recorded(&db, "grandchild", &both).unwrap(),
            Some(RootCohort {
                session_id: "root".into(),
                input_message_ids: vec!["fg".into()],
            })
        );
        // A running Job and a foreground call proving one link must agree.
        job(&db, "j1", "root", "child", origin(json!(["fg"])));
        assert!(recorded(&db, "grandchild", &both).unwrap().is_some());
        let disagree = lent.with("root", "child", &["other"]);
        assert_eq!(recorded(&db, "grandchild", &disagree).unwrap(), None);
    }
}
