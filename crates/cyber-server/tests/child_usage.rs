//! Descendant billing must agree with hidden usage and remain separate from own usage.
mod support;
use cyber_store::{Expected, NewEvent};
use serde_json::json;
use support::{Harness, Setup};

#[tokio::test]
async fn hidden_usage_sql_projection_matches_replayed_session_usage() {
    let h = Harness::new(Setup::default());
    let id = h.session().await;
    let usage = json!({"input":10,"output":2,"reasoning":3,"cache_read":4,"cache_write":5});
    h.store.append(&id,Expected::Any,vec![
        NewEvent::new("session.title.generated.1",json!({"title":"Billed title","usage":usage,"cost":0.10})),
        NewEvent::new("session.compaction.completed.1",json!({"message_id":"msg_summary","summary":"retained context","tail_start_id":"","tokens_before":10,"tokens_after":2,"trigger":"manual","usage":usage,"cost":0.15})),
    ]).unwrap();
    let replay = h.restart().state(&id).await.unwrap();
    let key = id.clone();
    let (cost,tokens)=h.store.read(move|conn|Ok(conn.query_row("SELECT cost,input_tokens+output_tokens+reasoning_tokens+cache_read_tokens+cache_write_tokens FROM session WHERE id=?1",[key],|row|Ok((row.get::<_,f64>(0)?,row.get::<_,i64>(1)?)))?)).unwrap();
    assert!((replay.totals.cost - 0.25).abs() < 1e-12);
    assert!(
        (cost - replay.totals.cost).abs() < 1e-12,
        "Hidden SQL usage omitted billing: {cost}"
    );
    assert_eq!(tokens, 48);
}

async fn child(h: &Harness, parent: &str) -> cyber_server::runtime::SessionInfo {
    h.runtime
        .create_session(cyber_server::runtime::CreateSession {
            directory: h.repo.display().to_string(),
            model: "test/main".into(),
            parent_id: Some(parent.into()),
            ..Default::default()
        })
        .await
        .unwrap()
}
fn billed(cost: Option<f64>) -> NewEvent {
    NewEvent::new(
        "session.step.ended.1",
        json!({"message_id":"msg_charge","finish":"stop","usage":{"input":10,"output":2,"reasoning":3,"cache_read":4,"cache_write":5},"cost":cost}),
    )
}
fn charge(h: &Harness, id: &str, cost: Option<f64>) {
    h.store
        .append(id, Expected::Any, vec![billed(cost)])
        .unwrap();
}
#[tokio::test]
async fn nested_live_usage_is_attributed_once_to_each_ancestor_without_changing_own_totals() {
    let h = Harness::new(Setup::default());
    let root = h.session().await;
    let first = child(&h, &root).await;
    let nested = child(&h, &first.id).await;
    let second = child(&h, &root).await;
    let unrelated = h.session().await;
    charge(&h, &root, Some(0.10));
    charge(&h, &first.id, Some(0.20));
    charge(&h, &nested.id, Some(0.05));
    charge(&h, &second.id, None);
    charge(&h, &unrelated, Some(1.0));
    let total = h.runtime.children_usage(&root).unwrap();
    assert!((total.children_cost - 0.25).abs() < 1e-12);
    assert_eq!(total.children_tokens, 72);
    assert_eq!(total.children_unpriced_steps, 1);
    assert!(total.children_usage_complete);
    assert_eq!(
        h.runtime.children_usage(&first.id).unwrap().children_tokens,
        24
    );
    assert_eq!(
        h.runtime
            .children_usage(&nested.id)
            .unwrap()
            .children_tokens,
        0
    );
    let replay = h.restart();
    assert_eq!(replay.state(&root).await.unwrap().children_usage, total);
    assert!((replay.state(&root).await.unwrap().totals.cost - 0.10).abs() < 1e-12);
    let rows = h
        .runtime
        .list(&cyber_server::runtime::ListFilter::default())
        .unwrap();
    assert_eq!(
        rows.sessions
            .iter()
            .find(|row| row.id == root)
            .unwrap()
            .children_usage,
        total
    );
    let receipts = h
        .store
        .read(|conn| {
            Ok(
                conn.query_row("SELECT count(*) FROM session_children_charge", [], |r| {
                    r.get::<_, i64>(0)
                })?,
            )
        })
        .unwrap();
    assert_eq!(receipts, 4);
}
#[tokio::test]
async fn child_conversation_deletion_preserves_billing_on_surviving_ancestors() {
    let h = Harness::new(Setup::default());
    let root = h.session().await;
    let direct = child(&h, &root).await;
    let nested = child(&h, &direct.id).await;
    charge(&h, &direct.id, Some(0.20));
    charge(&h, &nested.id, Some(0.30));
    let total = h.runtime.children_usage(&root).unwrap();
    h.runtime.delete(&direct.id).await.unwrap();
    assert_eq!(h.runtime.children_usage(&root).unwrap(), total);
    assert_eq!(
        h.restart().state(&root).await.unwrap().children_usage,
        total
    );
    let receipt_counts = h
        .store
        .read(move |conn| {
            Ok(conn.query_row(
                "SELECT count(*) FROM session_children_charge WHERE parent_id=?1",
                [direct.id],
                |r| r.get::<_, i64>(0),
            )?)
        })
        .unwrap();
    assert_eq!(receipt_counts, 0);
    h.runtime.delete(&root).await.unwrap();
    assert_eq!(
        h.store
            .read(|conn| Ok(conn.query_row(
                "SELECT count(*) FROM session_children_charge",
                [],
                |r| r.get::<_, i64>(0)
            )?))
            .unwrap(),
        0
    );
}
#[tokio::test]
async fn usage_and_ancestor_receipts_roll_back_together_if_a_later_projector_fails() {
    let h = Harness::new(Setup::default());
    let root = h.session().await;
    let direct = child(&h, &root).await;
    let seq = h.store.aggregate_seq(&direct.id).unwrap();
    let failure = h.store.append(
        &direct.id,
        Expected::Any,
        vec![
            billed(Some(0.50)),
            NewEvent::new("session.created.1", json!({"info":direct})),
        ],
    );
    assert!(failure.is_err());
    assert_eq!(h.store.aggregate_seq(&direct.id).unwrap(), seq);
    assert_eq!(h.runtime.children_usage(&root).unwrap().children_tokens, 0);
    assert_eq!(
        h.restart().state(&direct.id).await.unwrap().totals.cost,
        0.0
    );
    assert_eq!(
        h.store
            .read(|conn| Ok(conn.query_row(
                "SELECT count(*) FROM session_children_charge",
                [],
                |r| r.get::<_, i64>(0)
            )?))
            .unwrap(),
        0
    );
}
#[tokio::test]
async fn copied_fork_history_has_no_billing_until_new_usage_is_committed() {
    use cyber_server::runtime::{Admission, Delivery};
    let h = Harness::new(Setup {
        scripts: vec![(
            "test/main",
            vec![
                support::text("original billed history"),
                support::text("fresh fork answer"),
            ],
        )],
        ..Default::default()
    });
    let root = h.session().await;
    let direct = child(&h, &root).await;
    h.runtime
        .admit(&direct.id, Admission::text("first task", Delivery::Steer))
        .await
        .unwrap();
    h.runtime.wait_idle(&direct.id).await;
    let original = h.state(&direct.id).await;
    assert!(original.totals.cost > 0.0);
    let total = h.runtime.children_usage(&root).unwrap();
    let fork = h.runtime.fork(&direct.id, None).await.unwrap();
    let copied = h.state(&fork.id).await;
    assert!(fork.parent_id.is_none());
    assert!(!copied.entries.is_empty());
    assert_eq!(copied.totals.cost, 0.0);
    assert_eq!(copied.children_usage.children_tokens, 0);
    assert_eq!(h.runtime.children_usage(&root).unwrap(), total);
    h.runtime
        .admit(&fork.id, Admission::text("follow-up task", Delivery::Steer))
        .await
        .unwrap();
    h.runtime.wait_idle(&fork.id).await;
    assert!(h.state(&fork.id).await.totals.cost > 0.0);
    assert_eq!(h.runtime.children_usage(&root).unwrap(), total);
}

#[tokio::test]
async fn hidden_descendant_usage_counts_title_compaction_and_evaluator_billing() {
    let h = Harness::new(Setup::default());
    let root = h.session().await;
    let direct = child(&h, &root).await;
    let usage = json!({"input":10,"output":2,"reasoning":3,"cache_read":4,"cache_write":5});
    h.store.append(&direct.id,Expected::Any,vec![
        NewEvent::new("session.title.generated.1",json!({"title":"Charged title","usage":usage,"cost":0.10})),
        NewEvent::new("session.compaction.completed.1",json!({"message_id":"msg_summary","summary":"retained context","tail_start_id":"","tokens_before":10,"tokens_after":2,"trigger":"manual","usage":usage,"cost":0.15})),
        NewEvent::new("permission.auto_decided.1",json!({"request_id":"per_review","effect":"allow","reason":"safe","evaluator":"test/main","usage":usage,"cost":null})),
    ]).unwrap();
    let total = h.runtime.children_usage(&root).unwrap();
    assert!((total.children_cost - 0.25).abs() < 1e-12);
    assert_eq!(total.children_tokens, 72);
    assert_eq!(total.children_unpriced_steps, 1);
}
#[tokio::test]
async fn older_database_backfills_surviving_usage_without_claiming_deleted_history_is_complete() {
    let mut h = Harness::new(Setup::default());
    let root = h.session().await;
    let direct = child(&h, &root).await;
    let purged = child(&h, &root).await;
    charge(&h, &direct.id, Some(0.20));
    charge(&h, &purged.id, Some(0.70));
    h.runtime.delete(&purged.id).await.unwrap();
    h.store
        .append(
            &direct.id,
            Expected::Any,
            vec![NewEvent::new(
                "session.title.generated.1",
                json!({"title":"Legacy hidden cost","usage":{"input":1,"output":2,"reasoning":0,"cache_read":0,"cache_write":0},"cost":0.10}),
            )],
        )
        .unwrap();
    let backup = h.dir.path().join("legacy.db");
    cyber_store::backup::backup(&h.dir.path().join("cyber.db"), &backup).unwrap();
    {
        let conn = rusqlite::Connection::open(&backup).unwrap();
        conn.execute_batch(
            "DROP TABLE session_children_charge;
            ALTER TABLE session DROP COLUMN children_cost;
            ALTER TABLE session DROP COLUMN children_tokens;
            ALTER TABLE session DROP COLUMN children_unpriced_steps;
            ALTER TABLE session DROP COLUMN children_usage_complete;
            DELETE FROM migration WHERE id='20261007030000_children_usage';",
        )
        .unwrap();
        conn.execute(
            "UPDATE session SET cost=0.20,input_tokens=10,output_tokens=2 WHERE id=?1",
            [&direct.id],
        )
        .unwrap();
    }
    h.store = std::sync::Arc::new(support::open_store(&backup));
    let migrated = h.restart();
    let total = migrated.children_usage(&root).unwrap();
    assert!((total.children_cost - 0.30).abs() < 1e-12);
    assert_eq!(total.children_tokens, 27);
    assert!(!total.children_usage_complete);
    assert!((migrated.state(&direct.id).await.unwrap().totals.cost - 0.30).abs() < 1e-12);
    assert_eq!(migrated.children_usage(&root).unwrap(), total);
    let fresh = migrated
        .create_session(cyber_server::runtime::CreateSession {
            directory: h.repo.display().to_string(),
            model: "test/main".into(),
            ..Default::default()
        })
        .await
        .unwrap();
    assert!(
        migrated
            .children_usage(&fresh.id)
            .unwrap()
            .children_usage_complete
    );
}
