//! Schema migrations (`storage-events` → Idempotent migrations).

use std::collections::HashSet;

use rusqlite::{Connection, TransactionBehavior, params};

use crate::error::StoreError;

pub(crate) struct Migration {
    pub id: &'static str,
    pub sql: &'static str,
}

/// Every known migration, in ID order.
pub(crate) const MIGRATIONS: &[Migration] = &[
    Migration {
        id: "20261003000000_event_store",
        sql: include_str!("../migrations/20261003000000_event_store.sql"),
    },
    Migration {
        id: "20261004000000_sessions",
        sql: include_str!("../migrations/20261004000000_sessions.sql"),
    },
    Migration {
        id: "20261005000000_permissions_todos",
        sql: include_str!("../migrations/20261005000000_permissions_todos.sql"),
    },
    Migration {
        id: "20261006000000_idempotency",
        sql: include_str!("../migrations/20261006000000_idempotency.sql"),
    },
    Migration {
        id: "20261007000000_jobs",
        sql: include_str!("../migrations/20261007000000_jobs.sql"),
    },
    Migration {
        id: "20261007010000_subagent_names",
        sql: include_str!("../migrations/20261007010000_subagent_names.sql"),
    },
    Migration {
        id: "20261007020000_job_attempt_names",
        sql: include_str!("../migrations/20261007020000_job_attempt_names.sql"),
    },
    Migration {
        id: "20261007030000_children_usage",
        sql: include_str!("../migrations/20261007030000_children_usage.sql"),
    },
    Migration {
        id: "20261007040000_session_budgets",
        sql: include_str!("../migrations/20261007040000_session_budgets.sql"),
    },
    Migration {
        id: "20261007050000_children_token_classes",
        sql: include_str!("../migrations/20261007050000_children_token_classes.sql"),
    },
    Migration {
        id: "20261008000000_auto_statistics",
        sql: include_str!("../migrations/20261008000000_auto_statistics.sql"),
    },
    Migration {
        id: "20261008010000_hook_executions",
        sql: include_str!("../migrations/20261008010000_hook_executions.sql"),
    },
    Migration {
        id: "20261008020000_hook_once",
        sql: include_str!("../migrations/20261008020000_hook_once.sql"),
    },
    Migration {
        id: "20261008030000_hook_tests",
        sql: include_str!("../migrations/20261008030000_hook_tests.sql"),
    },
    Migration {
        id: "20261008040000_mcp_connections",
        sql: include_str!("../migrations/20261008040000_mcp_connections.sql"),
    },
    Migration {
        id: "20261009000000_memory_mutations",
        sql: include_str!("../migrations/20261009000000_memory_mutations.sql"),
    },
    Migration {
        id: "20261009010000_memory_recovery_requests",
        sql: include_str!("../migrations/20261009010000_memory_recovery_requests.sql"),
    },
    Migration {
        id: "20261015000000_mcp_serve",
        sql: include_str!("../migrations/20261015000000_mcp_serve.sql"),
    },
    Migration {
        id: "20261016000000_workflow_runs",
        sql: include_str!("../migrations/20261016000000_workflow_runs.sql"),
    },
    Migration {
        id: "20261017000000_workflow_budget",
        sql: include_str!("../migrations/20261017000000_workflow_budget.sql"),
    },
    Migration {
        id: "20261018000000_usage_ledger",
        sql: include_str!("../migrations/20261018000000_usage_ledger.sql"),
    },
    Migration {
        id: "20261019000000_messaging",
        sql: include_str!("../migrations/20261019000000_messaging.sql"),
    },
    Migration {
        id: "20261019005000_session_inbound",
        sql: include_str!("../migrations/20261019005000_session_inbound.sql"),
    },
    Migration {
        id: "20261019010000_usage_origin",
        sql: include_str!("../migrations/20261019010000_usage_origin.sql"),
    },
    Migration {
        id: "20261019020000_session_loops",
        sql: include_str!("../migrations/20261019020000_session_loops.sql"),
    },
    Migration {
        id: "20261020000000_peer_grant",
        sql: include_str!("../migrations/20261020000000_peer_grant.sql"),
    },
    Migration {
        id: "20261020010000_peer_messages",
        sql: include_str!("../migrations/20261020010000_peer_messages.sql"),
    },
    Migration {
        id: "20261020020000_peer_outcome_poll",
        sql: include_str!("../migrations/20261020020000_peer_outcome_poll.sql"),
    },
    Migration {
        id: "20261020030000_peer_watch",
        sql: include_str!("../migrations/20261020030000_peer_watch.sql"),
    },
    Migration {
        id: "20261025000000_teams",
        sql: include_str!("../migrations/20261025000000_teams.sql"),
    },
    Migration {
        id: "20261025010000_goals",
        sql: include_str!("../migrations/20261025010000_goals.sql"),
    },
];

pub(crate) enum Outcome {
    Current,
    /// The database was written by a newer version; open read-only.
    TooNew(Vec<String>),
}

pub(crate) fn migrate(
    conn: &mut Connection,
    migrations: &[Migration],
) -> Result<Outcome, StoreError> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS migration (id TEXT PRIMARY KEY, time_completed INTEGER NOT NULL) STRICT",
    )?;
    let applied = applied_ids(conn)?;
    let known: HashSet<&str> = migrations.iter().map(|m| m.id).collect();
    let mut unknown: Vec<String> = applied
        .iter()
        .filter(|id| !known.contains(id.as_str()))
        .cloned()
        .collect();
    if !unknown.is_empty() {
        unknown.sort();
        return Ok(Outcome::TooNew(unknown));
    }
    let pending: Vec<&Migration> = migrations
        .iter()
        .filter(|m| !applied.contains(m.id))
        .collect();
    if applied.is_empty() && is_empty_schema(conn)? {
        apply(conn, &pending)?;
    } else {
        for migration in pending {
            apply(conn, &[migration])?;
        }
    }
    Ok(Outcome::Current)
}

/// Apply migrations and their journal rows in one transaction.
fn apply(conn: &mut Connection, migrations: &[&Migration]) -> Result<(), StoreError> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    for migration in migrations {
        tx.execute_batch(migration.sql)?;
        tx.execute(
            "INSERT INTO migration (id, time_completed) VALUES (?1, ?2)",
            params![migration.id, now],
        )?;
    }
    tx.commit()?;
    Ok(())
}

fn applied_ids(conn: &Connection) -> Result<HashSet<String>, StoreError> {
    let mut stmt = conn.prepare("SELECT id FROM migration")?;
    let ids = stmt
        .query_map([], |r| r.get::<_, String>(0))?
        .collect::<Result<_, _>>()?;
    Ok(ids)
}

fn is_empty_schema(conn: &Connection) -> Result<bool, StoreError> {
    let count: i64 = conn.query_row(
        "SELECT count(*) FROM sqlite_schema WHERE type = 'table' AND name NOT LIKE 'sqlite_%' AND name != 'migration'",
        [],
        |r| r.get(0),
    )?;
    Ok(count == 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migration_ids_are_sorted_and_unique() {
        let ids: Vec<&str> = MIGRATIONS.iter().map(|m| m.id).collect();
        let mut sorted = ids.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(ids, sorted);
    }

    #[test]
    fn migrating_twice_is_a_no_op() {
        let mut conn = Connection::open_in_memory().unwrap();
        migrate(&mut conn, MIGRATIONS).unwrap();
        migrate(&mut conn, MIGRATIONS).unwrap();
        let n: i64 = conn
            .query_row("SELECT count(*) FROM migration", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, MIGRATIONS.len() as i64);
    }

    /// The messaging tables apply over a database holding P1 data, keep that data, and
    /// are removed with their Session.
    #[test]
    fn messaging_applies_over_p1_data_and_cascades_from_session() {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON").unwrap();
        let at = MIGRATIONS
            .iter()
            .position(|m| m.id == "20261019000000_messaging")
            .unwrap();
        migrate(&mut conn, &MIGRATIONS[..at]).unwrap();
        conn.execute_batch(
            "INSERT INTO session (id, title, directory, agent, model, mode, created_at, updated_at, last_seq)
             VALUES ('ses_a', 'backend', '/r', 'build', 'test/main', 'default', 1, 1, 0),
                    ('ses_b', 'frontend', '/r', 'build', 'test/main', 'default', 1, 1, 0);
             INSERT INTO event (id, aggregate_id, seq, type, data, time)
             VALUES ('evt_1', 'ses_a', 0, 'session.created.1', '{}', 1);
             INSERT INTO session_input (message_id, session_id, delivery, status, source, digest, admitted_seq)
             VALUES ('msg_1', 'ses_a', 'queue', 'pending', 'user', 'sha256:x', 0);",
        )
        .unwrap();
        migrate(&mut conn, MIGRATIONS).unwrap();
        let kept: i64 = conn
            .query_row(
                "SELECT (SELECT count(*) FROM session) + (SELECT count(*) FROM event) + (SELECT count(*) FROM session_input)",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(kept, 4);
        conn.execute_batch(
            "INSERT INTO xsession_message (session_id, message_id, direction, peer_address, peer_session_id,
                 origin_kind, sender_key, outcome, size, sha256, text, created_ms, updated_ms, seq)
             VALUES ('ses_a', 'msgx_1', 'in', 'frontend', 'ses_b', 'session', 'ses_b', 'delivered', 2, 'h', 'hi', 1, 1, 1),
                    ('ses_b', 'msgx_1', 'out', 'backend', 'ses_a', 'session', 'ses_b', 'delivered', 2, 'h', 'hi', 1, 1, 1);
             INSERT INTO messaging_watch (id, subscriber_id, target_address, target_id, status, created_ms, expires_ms)
             VALUES ('wch_1', 'ses_a', 'frontend', 'ses_b', 'active', 1, 2);",
        )
        .unwrap();
        let defaults: (i64, i64) = conn
            .query_row(
                "SELECT depth, hops FROM xsession_message WHERE session_id = 'ses_a'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(defaults, (0, 1));
        let orphan = conn.execute(
            "INSERT INTO messaging_watch (id, subscriber_id, target_address, target_id, status, created_ms, expires_ms)
             VALUES ('wch_2', 'ses_gone', 'x', 'ses_b', 'active', 1, 2)",
            [],
        );
        assert!(orphan.is_err(), "a watch needs its subscriber Session");
        conn.execute("DELETE FROM session WHERE id = 'ses_a'", [])
            .unwrap();
        let rows = |sql: &str| -> Vec<String> {
            let mut stmt = conn.prepare(sql).unwrap();
            stmt.query_map([], |r| r.get(0))
                .unwrap()
                .collect::<Result<_, _>>()
                .unwrap()
        };
        assert_eq!(rows("SELECT session_id FROM xsession_message"), ["ses_b"]);
        assert!(rows("SELECT id FROM messaging_watch").is_empty());
    }

    /// The team tables apply over a database holding P1 data and keep it; a team's
    /// members and tasks go with the team, and nothing goes with a deleted Session (a
    /// team outlives its lead).
    #[test]
    fn teams_apply_over_p1_data_and_cascade_from_the_team_only() {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON").unwrap();
        let at = MIGRATIONS
            .iter()
            .position(|m| m.id == "20261025000000_teams")
            .unwrap();
        migrate(&mut conn, &MIGRATIONS[..at]).unwrap();
        conn.execute_batch(
            "INSERT INTO session (id, title, directory, agent, model, mode, created_at, updated_at, last_seq)
             VALUES ('ses_lead', 'lead', '/r', 'build', 'test/main', 'default', 1, 1, 0),
                    ('ses_api', 'feature/api', '/r', 'build', 'test/main', 'default', 1, 1, 0);
             INSERT INTO event (id, aggregate_id, seq, type, data, time)
             VALUES ('evt_1', 'ses_lead', 0, 'session.created.1', '{}', 1);
             INSERT INTO session_loop (loop_id, session_id, status, next_due_ms, inflight, created_ms)
             VALUES ('lop_1', 'ses_lead', 'active', 5, 0, 1);",
        )
        .unwrap();
        migrate(&mut conn, MIGRATIONS).unwrap();
        let count = |sql: &str| -> i64 { conn.query_row(sql, [], |r| r.get(0)).unwrap() };
        assert_eq!(
            count(
                "SELECT (SELECT count(*) FROM session) + (SELECT count(*) FROM event) + (SELECT count(*) FROM session_loop)"
            ),
            4
        );
        conn.execute_batch(
            "INSERT INTO team (id, name, lead_id, directory, status, created_ms, updated_ms)
             VALUES ('team_1', 'feature', 'ses_lead', '/r', 'active', 1, 1);
             INSERT INTO team_member (team_id, name, session_id, role, agent, isolation, status)
             VALUES ('team_1', 'lead', 'ses_lead', 'lead', 'build', 'shared', 'idle'),
                    ('team_1', 'api', 'ses_api', 'teammate', 'build', 'worktree', 'working');
             INSERT INTO team_task (id, team_id, seq, title, status, created_by, created_ms, updated_ms)
             VALUES ('task_1', 'team_1', 1, 'API', 'todo', 'lead', 1, 1);",
        )
        .unwrap();
        assert_eq!(count("SELECT version FROM team_task"), 0);
        assert_eq!(
            count("SELECT count(*) FROM team_task WHERE depends_on = '[]' AND description = ''"),
            1
        );
        let duplicate = conn.execute(
            "INSERT INTO team_member (team_id, name, session_id, role, agent, isolation, status)
             VALUES ('team_1', 'ui', 'ses_api', 'teammate', 'build', 'shared', 'idle')",
            [],
        );
        assert!(duplicate.is_err(), "a Session is a member of one team");
        let orphan = conn.execute(
            "INSERT INTO team_task (id, team_id, seq, title, status, created_by, created_ms, updated_ms)
             VALUES ('task_2', 'team_gone', 1, 'x', 'todo', 'lead', 1, 1)",
            [],
        );
        assert!(orphan.is_err(), "a task needs its team");
        conn.execute("DELETE FROM session WHERE id = 'ses_lead'", [])
            .unwrap();
        assert_eq!(count("SELECT count(*) FROM team_member"), 2);
        conn.execute("DELETE FROM team WHERE id = 'team_1'", [])
            .unwrap();
        assert_eq!(
            count("SELECT (SELECT count(*) FROM team_member) + (SELECT count(*) FROM team_task)"),
            0
        );
    }

    /// Task 9.2: ledger rows and folded days recorded before origins existed read as
    /// `user`, and a folded day keys on its origin.
    #[test]
    fn usage_origin_reads_old_ledger_rows_as_user() {
        let mut conn = Connection::open_in_memory().unwrap();
        let at = MIGRATIONS
            .iter()
            .position(|m| m.id == "20261019010000_usage_origin")
            .unwrap();
        migrate(&mut conn, &MIGRATIONS[..at]).unwrap();
        conn.execute_batch(
            "INSERT INTO usage_step (event_id, session_id, root_id, agent, purpose, provider, model,
                started_ms, ended_ms, input_tokens, output_tokens, reasoning_tokens,
                cache_read_tokens, cache_write_tokens, prompt_tokens, cost, directory)
             VALUES ('evt_1', 'ses_a', 'ses_a', 'build', 'turn', 'test', 'main', 1, 2, 100, 10,
                0, 0, 0, 100, 0.5, '/r');
             INSERT INTO usage_daily VALUES ('2026-10-01', 'ses_a', 'build', 'test', 'main', '/r',
                1, 1, 100, 10, 0, 0, 0, 0.5, 0, 100, 1);",
        )
        .unwrap();
        migrate(&mut conn, MIGRATIONS).unwrap();
        let origin = |sql: &str| -> (String, i64) {
            conn.query_row(sql, [], |r| Ok((r.get(0)?, r.get(1)?)))
                .unwrap()
        };
        assert_eq!(
            origin("SELECT origin, input_tokens FROM usage_step"),
            ("user".into(), 100)
        );
        assert_eq!(
            origin("SELECT origin, input_tokens FROM usage_daily"),
            ("user".into(), 100)
        );
        conn.execute(
            "INSERT INTO usage_daily VALUES ('2026-10-01', 'ses_a', 'build', 'test', 'main',
                'session', '/r', 1, 1, 7, 1, 0, 0, 0, 0.1, 0, 7, 1)",
            [],
        )
        .expect("the same day, root and model with another origin is its own row");
    }
}
