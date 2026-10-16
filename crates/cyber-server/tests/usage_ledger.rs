//! The usage step ledger (`observability-costs` → Usage step ledger) agrees with the
//! Session totals, including descendants and unpriced calls, after live projection, the
//! upgrade backfill and retention folding.
mod support;

use cyber_server::runtime::{CreateSession, Runtime, SessionUsage};
use cyber_store::{Expected, NewEvent};
use serde_json::json;
use support::{Harness, Setup};

fn usage(input: u64, output: u64) -> serde_json::Value {
    json!({"input":input,"output":output,"reasoning":1,"cache_read":4,"cache_write":2})
}

/// A started and an ended provider step of `provider/model`.
fn step(message: &str, provider: &str, model: &str, cost: Option<f64>) -> Vec<NewEvent> {
    vec![
        NewEvent::new(
            "session.step.started.1",
            json!({"agent":"build","message_id":message,"provider":provider,"model":model,"tools":false}),
        ),
        NewEvent::new(
            "session.step.ended.1",
            json!({"message_id":message,"finish":"stop","usage":usage(100, 20),"cost":cost}),
        ),
    ]
}

async fn child(h: &Harness, parent: &str) -> String {
    h.runtime
        .create_session(CreateSession {
            directory: h.repo.display().to_string(),
            model: "test/main".into(),
            parent_id: Some(parent.into()),
            ..Default::default()
        })
        .await
        .unwrap()
        .id
}

fn append(h: &Harness, id: &str, events: Vec<NewEvent>) {
    h.store.append(id, Expected::Any, events).unwrap();
}

/// The ledger's per-model sums equal the report's own and descendant totals.
fn assert_ledger_matches(usage: &SessionUsage) {
    let total = &usage.report.total;
    let sum =
        |f: fn(&cyber_llm::Usage) -> u64| usage.by_model.iter().map(|m| f(&m.tokens)).sum::<u64>();
    assert_eq!(sum(|t| t.input), total.tokens.input);
    assert_eq!(sum(|t| t.output), total.tokens.output);
    assert_eq!(sum(|t| t.reasoning), total.tokens.reasoning);
    assert_eq!(sum(|t| t.cache_read), total.tokens.cache_read);
    assert_eq!(sum(|t| t.cache_write), total.tokens.cache_write);
    let cost: f64 = usage.by_model.iter().filter_map(|m| m.cost).sum();
    assert!((cost - total.cost).abs() < 1e-9, "{cost} != {}", total.cost);
    let unpriced: u64 = usage.by_model.iter().map(|m| m.unpriced_steps).sum();
    assert_eq!(unpriced, total.unpriced_steps);
}

/// Steps, a failure and auxiliary calls of a root, a child, a nested child and an
/// unpriced child, as the Drain records them.
async fn tree(h: &Harness) -> (String, String) {
    let root = h.session().await;
    let first = child(h, &root).await;
    let nested = child(h, &first).await;
    let unpriced = child(h, &root).await;
    append(h, &root, step("msg_a", "test", "main", Some(0.5)));
    append(h, &root, step("msg_b", "test", "main", Some(0.25)));
    append(
        h,
        &root,
        vec![
            NewEvent::new(
                "session.step.started.1",
                json!({"agent":"build","message_id":"msg_c","provider":"test","model":"main","tools":false}),
            ),
            NewEvent::new(
                "session.step.failed.1",
                json!({"message_id":"msg_c","kind":"interrupted","message":"stopped"}),
            ),
            NewEvent::new(
                "session.title.generated.1",
                json!({"title":"Ledger","usage":usage(10, 2),"cost":0.01,"model":"test/title"}),
            ),
            NewEvent::new(
                "usage.recorded.1",
                json!({"provider":"test","model":"test/title","purpose":"discarded_title","call_id":null,"duration_ms":40,"usage":usage(5, 1),"cost":0.02}),
            ),
        ],
    );
    append(h, &first, step("msg_d", "other", "main", Some(0.125)));
    append(
        h,
        &nested,
        vec![
            NewEvent::new(
                "session.compaction.completed.1",
                json!({"message_id":"msg_s","summary":"s","tail_start_id":"","tokens_before":9,"tokens_after":1,"trigger":"manual","usage":usage(7, 3),"cost":0.03}),
            ),
            NewEvent::new(
                "permission.auto_decided.1",
                json!({"call_id":"c","action":"bash","resources":[],"decision":"allow","reason":"ok","model":"test/main","usage":usage(3, 1),"cost":null}),
            ),
        ],
    );
    append(h, &unpriced, step("msg_e", "ollama", "qwen", None));
    (root, nested)
}

#[tokio::test]
async fn live_ledger_sums_equal_session_totals_with_descendants_and_unpriced_steps() {
    let h = Harness::new(Setup::default());
    let (root, nested) = tree(&h).await;
    let usage = h.runtime.session_usage_breakdown(&root).await.unwrap();
    assert_ledger_matches(&usage);
    assert!(!usage.backfilling);
    let models: Vec<(&str, &str)> = usage
        .by_model
        .iter()
        .map(|m| (m.provider.as_str(), m.model.as_str()))
        .collect();
    assert_eq!(
        models,
        [
            ("test", "main"),
            ("other", "main"),
            ("test", "title"),
            ("ollama", "qwen")
        ],
        "sorted by cost, the title model split from its reference"
    );
    let ollama = &usage.by_model[3];
    assert_eq!((ollama.cost, ollama.unpriced_steps), (None, 1));
    // The failed step records a row with no usage; the evaluator call is unpriced.
    let main = &usage.by_model[0];
    assert_eq!((main.steps, main.unpriced_steps), (4, 1));
    assert_eq!(usage.report.total.unpriced_steps, 2);
    // A descendant scope covers its own subtree only.
    assert_ledger_matches(&h.runtime.session_usage_breakdown(&nested).await.unwrap());
}

#[tokio::test]
async fn deleted_descendants_keep_their_ledger_rows() {
    let h = Harness::new(Setup::default());
    let root = h.session().await;
    let gone = child(&h, &root).await;
    append(&h, &gone, step("msg_x", "test", "main", Some(0.75)));
    h.runtime.delete(&gone).await.unwrap();
    let usage = h.runtime.session_usage_breakdown(&root).await.unwrap();
    assert_ledger_matches(&usage);
    assert_eq!(usage.by_model.len(), 1);
}

/// Remove the ledger from the database file, as before the upgrade, and reopen it.
fn downgrade_and_reopen(h: &mut Harness) -> Runtime {
    let path = h.dir.path().join("legacy.db");
    cyber_store::backup::backup(&h.dir.path().join("cyber.db"), &path).unwrap();
    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute_batch(
        "DROP TABLE usage_step; DROP TABLE usage_daily; DROP TABLE usage_backfill;
         DELETE FROM migration WHERE id = '20261018000000_usage_ledger';",
    )
    .unwrap();
    drop(db);
    h.store = std::sync::Arc::new(support::open_store(&path));
    h.restart()
}

/// Regression: a Session deleted before the backfill reached its events (the retention
/// sweep purging old archived Sessions on the first start after an upgrade) lost its
/// usage for good, since the backfill only projects events that still exist.
#[tokio::test]
async fn sessions_deleted_during_the_backfill_keep_their_ledger_rows() {
    let mut h = Harness::new(Setup::default());
    let root = h.session().await;
    let gone = child(&h, &root).await;
    let nested = child(&h, &gone).await;
    append(&h, &root, step("msg_r", "test", "main", Some(0.5)));
    append(&h, &gone, step("msg_g", "test", "main", Some(0.75)));
    append(&h, &nested, step("msg_n", "other", "main", Some(0.25)));
    let runtime = downgrade_and_reopen(&mut h);
    assert!(runtime.usage_backfilling().unwrap());
    runtime.delete(&gone).await.unwrap();
    while !runtime.backfill_usage_batch().unwrap() {}
    let rows: Vec<(String, String)> = h
        .store
        .read(|db| {
            let mut q = db.prepare("SELECT session_id, root_id FROM usage_step ORDER BY rowid")?;
            let rows = q.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
            Ok(rows.collect::<Result<Vec<_>, _>>()?)
        })
        .unwrap();
    assert_eq!(rows.len(), 3, "{rows:?}");
    assert!(
        rows.iter().all(|(_, r)| *r == root),
        "rooted at the survivor"
    );
    for id in [&root, &gone, &nested] {
        assert!(rows.iter().any(|(s, _)| s == id), "{id} has its row");
    }
    let usage = runtime.session_usage_breakdown(&root).await.unwrap();
    assert_ledger_matches(&usage);
    assert_eq!(usage.by_model.iter().map(|m| m.steps).sum::<u64>(), 3);
}

#[tokio::test]
async fn upgrade_backfills_forty_sessions_in_resumable_batches() {
    let mut h = Harness::new(Setup::default());
    let mut ids = Vec::new();
    for n in 0..40 {
        let id = h.session().await;
        // 66 steps of two events each: 5,280 ledger events, more than one batch.
        let events = (0..66)
            .flat_map(|k| {
                let cost = (k % 3 != 0).then_some(0.001 * f64::from(n + 1));
                step(&format!("msg_{k}"), "test", "main", cost)
            })
            .collect();
        append(&h, &id, events);
        ids.push(id);
    }
    let runtime = downgrade_and_reopen(&mut h);
    assert!(runtime.usage_backfilling().unwrap());
    let partial = runtime.session_usage_breakdown(&ids[0]).await.unwrap();
    assert!(partial.backfilling && partial.by_model.is_empty());

    assert!(!runtime.backfill_usage_batch().unwrap(), "first batch");
    // A restart resumes from the stored cursor.
    let runtime = h.restart();
    assert!(runtime.usage_backfilling().unwrap());
    while !runtime.backfill_usage_batch().unwrap() {}
    assert!(!runtime.usage_backfilling().unwrap());
    let rows: i64 = h
        .store
        .read(|db| Ok(db.query_row("SELECT COUNT(*) FROM usage_step", [], |r| r.get(0))?))
        .unwrap();
    assert_eq!(rows, 40 * 66, "one row per recorded step");
    for id in &ids {
        let usage = runtime.session_usage_breakdown(id).await.unwrap();
        assert!(!usage.backfilling);
        assert_ledger_matches(&usage);
        assert_eq!(usage.by_model[0].steps, 66);
    }
    // A live step after the backfill is counted once.
    append(&h, &ids[0], step("msg_live", "test", "main", Some(1.0)));
    runtime.backfill_usage_batch().unwrap();
    assert_ledger_matches(&runtime.session_usage_breakdown(&ids[0]).await.unwrap());
}

#[tokio::test]
async fn retention_folds_old_rows_into_daily_aggregates_without_losing_totals() {
    let h = Harness::new(Setup::default());
    let (root, _) = tree(&h).await;
    let before = h.runtime.session_usage_breakdown(&root).await.unwrap();
    let folded = h.runtime.fold_usage(i64::MAX).unwrap();
    assert_eq!(folded, 9, "every ledger row of the tree");
    let (steps, days): (i64, i64) = h
        .store
        .read(|db| {
            Ok(db.query_row(
                "SELECT (SELECT COUNT(*) FROM usage_step), (SELECT COUNT(*) FROM usage_daily)",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?)
        })
        .unwrap();
    assert_eq!(steps, 0);
    assert!(days > 0);
    let after = h.runtime.session_usage_breakdown(&root).await.unwrap();
    assert_ledger_matches(&after);
    assert_eq!(after.by_model, before.by_model);
    assert_eq!(after.api_duration_ms, before.api_duration_ms);
    assert_eq!(h.runtime.fold_usage(i64::MAX).unwrap(), 0);
}
