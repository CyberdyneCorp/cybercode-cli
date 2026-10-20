//! Agent teams on one server (`agent-teams`): the durable `team.*` records of the lead's
//! stream, their projection and rebuild, and the team tag of member Sessions.
mod support;

use cyber_server::runtime::teams::records;
use cyber_server::runtime::teams::{Role, TeamTag};
use cyber_server::runtime::{CreateSession, RuntimeError};
use cyber_store::{Expected, NewEvent};
use serde_json::{Value, json};
use support::{Harness, Setup};

fn append(h: &Harness, id: &str, kind: &str, data: Value) -> Result<(), String> {
    h.store
        .append(id, Expected::Any, vec![NewEvent::new(kind, data)])
        .map(drop)
        .map_err(|error| error.to_string())
}

fn task(id: &str, seq: i64, status: &str, owner: Option<&str>, version: i64) -> Value {
    json!({"id": id, "seq": seq, "title": format!("task {seq}"), "status": status, "owner": owner,
        "depends_on": if seq > 1 { json!(["task_1"]) } else { json!([]) },
        "created_by": "lead", "created_ms": 10, "updated_ms": 10 + version, "version": version})
}

/// A team as the runtime will record it: created, a lead and two teammates, tasks
/// created, claimed and finished, an idle notice, a budget stop and a resume.
async fn recorded_team(h: &Harness) -> (String, String, String) {
    let lead = h.session().await;
    let api = h.session().await;
    let ui = h.session().await;
    let events = [
        (
            records::CREATED,
            json!({"team_id": "team_1", "name": "feature", "directory": h.repo.display().to_string(),
            "budget": {"max_cost_usd": 20.0}, "definition": "feature", "instructions": "Split the work."}),
        ),
        (
            records::MEMBER_SPAWNED,
            json!({"team_id": "team_1", "name": "lead", "session_id": lead, "role": "lead",
            "agent": "build", "isolation": "shared", "owns": []}),
        ),
        (
            records::MEMBER_SPAWNED,
            json!({"team_id": "team_1", "name": "api", "session_id": api, "role": "teammate",
            "agent": "build", "model": "test/main", "isolation": "worktree", "worktree_id": "wt_api",
            "branch": "cyber/team-1/api", "owns": ["src/api/**"]}),
        ),
        (
            records::MEMBER_SPAWNED,
            json!({"team_id": "team_1", "name": "ui", "session_id": ui, "role": "teammate",
            "agent": "build", "isolation": "shared", "owns": []}),
        ),
        (
            records::TASK_UPDATED,
            json!({"team_id": "team_1", "task": task("task_1", 1, "todo", None, 0), "by": "lead"}),
        ),
        (
            records::TASK_UPDATED,
            json!({"team_id": "team_1", "task": task("task_2", 2, "todo", None, 0), "by": "lead"}),
        ),
        (
            records::TASK_UPDATED,
            json!({"team_id": "team_1", "task": task("task_1", 1, "in_progress", Some("api"), 1),
            "previous_status": "todo", "by": "api"}),
        ),
        (
            records::MEMBER_UPDATED,
            json!({"team_id": "team_1", "name": "api", "status": "working"}),
        ),
        (
            records::TASK_UPDATED,
            json!({"team_id": "team_1", "task": task("task_1", 1, "done", Some("api"), 2),
            "previous_status": "in_progress", "by": "api"}),
        ),
        (
            records::MEMBER_IDLE,
            json!({"team_id": "team_1", "name": "api", "tasks_in_progress": [],
            "last_status": "API done"}),
        ),
        (
            records::MEMBER_UPDATED,
            json!({"team_id": "team_1", "name": "ui", "status": "waiting_input",
            "last_status": "Asking the lead"}),
        ),
        (
            records::BUDGET_EXCEEDED,
            json!({"team_id": "team_1", "budget": {"max_cost_usd": 20.0},
            "spent": {"cost_usd": 20.1}, "members": [{"name": "api", "turns": 3, "tokens": 900, "cost_usd": 12.0}]}),
        ),
        (
            records::STOPPED,
            json!({"team_id": "team_1", "reason": "budget"}),
        ),
        (
            records::RESUMED,
            json!({"team_id": "team_1", "lead_id": lead}),
        ),
    ];
    for (kind, data) in events {
        append(h, &lead, kind, data).unwrap();
    }
    (lead, api, ui)
}

fn rows(h: &Harness) -> Vec<Vec<Value>> {
    let query = |sql: &'static str| {
        h.store
            .read(move |db| {
                let mut stmt = db.prepare(sql)?;
                let width = stmt.column_count();
                let rows = stmt
                    .query_map([], |row| {
                        (0..width)
                            .map(|i| {
                                Ok(match row.get_ref(i)? {
                                    rusqlite::types::ValueRef::Null => Value::Null,
                                    rusqlite::types::ValueRef::Integer(n) => json!(n),
                                    rusqlite::types::ValueRef::Real(f) => json!(f),
                                    rusqlite::types::ValueRef::Text(t)
                                    | rusqlite::types::ValueRef::Blob(t) => {
                                        json!(String::from_utf8_lossy(t))
                                    }
                                })
                            })
                            .collect::<rusqlite::Result<Vec<Value>>>()
                    })?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                Ok(rows)
            })
            .unwrap()
    };
    let mut out = query("SELECT * FROM team ORDER BY id");
    out.extend(query("SELECT * FROM team_member ORDER BY team_id, name"));
    out.extend(query("SELECT * FROM team_task ORDER BY id"));
    out
}

/// Task 1.3: the projection follows the events, and a rebuild (drop the rows,
/// re-project) gives identical rows, twice.
#[tokio::test]
async fn team_projections_follow_the_events_and_rebuild_identically() {
    let h = Harness::new(Setup::default());
    let (lead, api, _) = recorded_team(&h).await;
    let before = rows(&h);
    assert_eq!(before.len(), 1 + 3 + 2, "{before:?}");
    let team = &before[0];
    assert_eq!(team[2], json!(lead));
    assert_eq!(
        (&team[4], &team[5]),
        (&json!("active"), &Value::Null),
        "resumed clears the stop"
    );
    assert_eq!(
        team[6],
        json!(r#"{"max_cost_usd":20.0,"enforcement":"soft"}"#)
    );
    assert_eq!(team[7], json!("Split the work."));
    let member = |name: &str| {
        before
            .iter()
            .find(|r| r.len() == 12 && r[1] == json!(name))
            .unwrap()
            .clone()
    };
    let api_row = member("api");
    assert_eq!(api_row[2], json!(api));
    assert_eq!(
        (&api_row[6], &api_row[7], &api_row[8]),
        (
            &json!("worktree"),
            &json!("wt_api"),
            &json!("cyber/team-1/api")
        )
    );
    assert_eq!(api_row[9], json!(r#"["src/api/**"]"#));
    assert_eq!(
        (&api_row[10], &api_row[11]),
        (&json!("idle"), &json!("API done"))
    );
    assert_eq!(
        member("ui")[10],
        json!("idle"),
        "resume puts every member back to idle"
    );
    let task_1 = before.iter().find(|r| r[0] == json!("task_1")).unwrap();
    assert_eq!(
        (&task_1[5], &task_1[6], &task_1[11]),
        (&json!("done"), &json!("api"), &json!(2))
    );
    let task_2 = before.iter().find(|r| r[0] == json!("task_2")).unwrap();
    assert_eq!(task_2[7], json!(r#"["task_1"]"#));

    h.store
        .transaction(|tx| {
            tx.execute_batch("DELETE FROM team_task; DELETE FROM team_member; DELETE FROM team;")?;
            Ok(())
        })
        .unwrap();
    assert!(rows(&h).is_empty());
    assert_eq!(h.runtime.rebuild_teams().unwrap(), 14);
    assert_eq!(rows(&h), before);
    assert_eq!(h.runtime.rebuild_teams().unwrap(), 14);
    assert_eq!(rows(&h), before);
}

/// Task 6.2: a resume with a new lead after `lead_deleted` gives it the lead's member
/// row and keeps the earlier lead in `former_leads` (its spend still counts); a rebuild
/// gives identical rows.
#[tokio::test]
async fn a_new_lead_takes_the_lead_row_and_the_former_lead_is_kept() {
    let h = Harness::new(Setup::default());
    let (lead, _, _) = recorded_team(&h).await;
    let next = h.session().await;
    append(
        &h,
        &lead,
        records::STOPPED,
        json!({"team_id": "team_1", "reason": "lead_deleted"}),
    )
    .unwrap();
    append(
        &h,
        &next,
        records::RESUMED,
        json!({"team_id": "team_1", "lead_id": next}),
    )
    .unwrap();
    let before = rows(&h);
    let team = &before[0];
    assert_eq!((&team[2], &team[4]), (&json!(next), &json!("active")));
    assert_eq!(team[10], json!(format!("[\"{lead}\"]")));
    let lead_row = before
        .iter()
        .find(|r| r.len() == 12 && r[1] == json!("lead"))
        .unwrap();
    assert_eq!(lead_row[2], json!(next));
    // Resuming with the same lead again adds no former lead.
    append(
        &h,
        &next,
        records::RESUMED,
        json!({"team_id": "team_1", "lead_id": next}),
    )
    .unwrap();
    let after = rows(&h);
    assert_eq!(after[0][10], team[10]);
    h.runtime.rebuild_teams().unwrap();
    assert_eq!(rows(&h), after);
}

#[tokio::test]
async fn stop_phases_project_stopping_then_stopped() {
    let h = Harness::new(Setup::default());
    let (lead, _, _) = recorded_team(&h).await;
    append(
        &h,
        &lead,
        records::STOPPED,
        json!({"team_id": "team_1", "reason": "user", "stopping": true}),
    )
    .unwrap();
    let team = rows(&h)[0].clone();
    assert_eq!((&team[4], &team[5]), (&json!("stopping"), &json!("user")));
    append(
        &h,
        &lead,
        records::STOPPED,
        json!({"team_id": "team_1", "reason": "user"}),
    )
    .unwrap();
    let after = rows(&h);
    assert_eq!(
        (&after[0][4], &after[0][5]),
        (&json!("stopped"), &json!("user"))
    );
    assert!(
        after
            .iter()
            .filter(|r| r.len() == 12 && r[0] == json!("team_1"))
            .all(|m| m[10] == json!("stopped"))
    );
}

/// Records that would corrupt the projection are refused on append, before any row.
#[tokio::test]
async fn invalid_team_records_are_refused() {
    let h = Harness::new(Setup::default());
    let lead = h.session().await;
    let refused = [
        (
            records::CREATED,
            json!({"team_id": "tm_1", "name": "feature", "directory": "/r"}),
        ),
        (
            records::CREATED,
            json!({"team_id": "team_1", "name": "Bad Name", "directory": "/r"}),
        ),
        (
            records::CREATED,
            json!({"team_id": "team_1", "name": "feature", "directory": "/r", "extra": 1}),
        ),
        (
            records::MEMBER_SPAWNED,
            json!({"team_id": "team_1", "name": "api", "session_id": "ses_1", "role": "lead",
            "agent": "build", "isolation": "shared"}),
        ),
        (
            records::MEMBER_SPAWNED,
            json!({"team_id": "team_1", "name": "all", "session_id": "ses_1", "role": "teammate",
            "agent": "build", "isolation": "shared"}),
        ),
        (
            records::MEMBER_SPAWNED,
            json!({"team_id": "team_1", "name": "api", "session_id": "ses_1", "role": "teammate",
            "agent": "build", "isolation": "worktree"}),
        ),
        (
            records::MEMBER_UPDATED,
            json!({"team_id": "team_1", "name": "api", "status": "idle",
            "last_status": "x".repeat(records::MAX_STATUS + 1)}),
        ),
        (
            records::TASK_UPDATED,
            json!({"team_id": "team_1", "by": "lead", "task": {"id": "task_1", "seq": 1,
            "title": "x", "status": "todo", "depends_on": ["task_1"], "created_by": "lead", "created_ms": 1,
            "updated_ms": 1, "version": 0}}),
        ),
        (
            records::STOPPED,
            json!({"team_id": "team_1", "reason": "tired"}),
        ),
        (
            records::RESUMED,
            json!({"team_id": "team_1", "lead_id": "nope"}),
        ),
    ];
    for (kind, data) in refused {
        let error = append(&h, &lead, kind, data.clone()).unwrap_err();
        assert!(error.contains(kind), "{kind} {data}: {error}");
    }
    assert!(rows(&h).is_empty());
}

/// `SessionInfo.team` is set at creation, kept across a restart, refused for a child,
/// and never copied into a fork.
#[tokio::test]
async fn a_member_session_keeps_its_team_tag() {
    let h = Harness::new(Setup::default());
    let tag = TeamTag {
        team_id: "team_1".into(),
        role: Role::Teammate,
        name: "api".into(),
    };
    let create = |team: Option<TeamTag>, parent: Option<String>| CreateSession {
        directory: h.repo.display().to_string(),
        model: "test/main".into(),
        title: Some("feature/api".into()),
        parent_id: parent,
        team,
        ..Default::default()
    };
    let info = h
        .runtime
        .create_session(create(Some(tag.clone()), None))
        .await
        .unwrap();
    assert_eq!(info.team.as_ref(), Some(&tag));
    let restarted = h.restart();
    assert_eq!(
        restarted.state(&info.id).await.unwrap().info.team,
        Some(tag.clone())
    );
    let child = h
        .runtime
        .create_session(create(Some(tag.clone()), Some(info.id.clone())))
        .await;
    assert!(
        matches!(child, Err(RuntimeError::Invalid(ref m)) if m == "A team member is a root Session"),
        "{child:?}"
    );
    let bad = TeamTag {
        role: Role::Lead,
        ..tag
    };
    let refused = h.runtime.create_session(create(Some(bad), None)).await;
    assert!(
        matches!(refused, Err(RuntimeError::Invalid(_))),
        "{refused:?}"
    );
    let fork = h.runtime.fork(&info.id, None).await.unwrap();
    assert_eq!(fork.team, None);
    let plain = h.runtime.create_session(create(None, None)).await.unwrap();
    let data = serde_json::to_value(&plain).unwrap();
    assert!(data.get("team").is_none(), "{data}");
}
