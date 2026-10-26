use super::*;
use cyber_core::autofix::{Config, Trigger, TriggerKind};
use cyber_core::paths::DatabaseLocation;
use cyber_store::StoreOptions;
fn store(path: &std::path::Path) -> Arc<Store> {
    Arc::new(
        Store::open(StoreOptions::new(
            DatabaseLocation::File(path.to_owned()),
            crate::runtime::Runtime::registry(),
        ))
        .unwrap(),
    )
}
fn fixture() -> Subscription {
    Subscription::new(
        "afx_test".into(),
        "/project".into(),
        Config {
            repository: "team/repo".into(),
            pr: 1,
            session_id: "ses_owner".into(),
            model: "test/model".into(),
            checks: vec![vec!["test".into()]],
            required_ci: vec!["check:1:test".into()],
            verification_paths: vec!["tests".into()],
            max_attempts: 3,
            max_tokens: 100,
            max_seconds: 30,
        },
        7,
        "fix".into(),
        0,
    )
    .unwrap()
}
#[test]
fn revision_race_admits_only_one_worker() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = Repository::new(store(&tmp.path().join("db")));
    repo.create(fixture()).unwrap();
    let mut workers = vec![];
    for i in 0..8 {
        let repo = repo.clone();
        workers.push(std::thread::spawn(move || {
            repo.update("afx_test", 1, 1, move |s| {
                s.enqueue(
                    vec![Trigger {
                        key: "check:1".into(),
                        head: "1".repeat(40),
                        kind: TriggerKind::CheckFailure,
                        text: "failure".into(),
                    }],
                    &"1".repeat(40),
                )?;
                s.claim(format!("attempt_{i}"), &"1".repeat(40), 1)?;
                Ok(())
            })
            .is_ok()
        }))
    }
    assert_eq!(
        workers
            .into_iter()
            .filter_map(|h| h.join().ok())
            .filter(|v| *v)
            .count(),
        1
    );
    assert_eq!(repo.get("afx_test").unwrap().attempts.len(), 1);
}
#[test]
fn binding_and_dedup_survive_reopen_and_projection_rebuild() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("db");
    let db = store(&path);
    let repo = Repository::new(db.clone());
    let s = fixture();
    repo.create(s.clone()).unwrap();
    assert_eq!(repo.create(s).unwrap().revision, 1);
    repo.update("afx_test", 1, 1, |s| {
        s.enqueue(
            vec![Trigger {
                key: "check:1".into(),
                head: "1".repeat(40),
                kind: TriggerKind::CheckFailure,
                text: "failure".into(),
            }],
            &"1".repeat(40),
        )?;
        s.claim("attempt".into(), &"1".repeat(40), 1)?;
        Ok(())
    })
    .unwrap();
    repo.bind_session("afx_test", "attempt", "ses_fix", "/isolated")
        .unwrap();
    repo.bind_session("afx_test", "attempt", "ses_fix", "/isolated")
        .unwrap();
    assert!(
        repo.bind_session("afx_test", "attempt", "ses_fix", "/different")
            .is_err()
    );
    drop(repo);
    drop(db);
    let db = store(&path);
    let repo = Repository::new(db.clone());
    assert!(repo.get("afx_test").unwrap().seen.contains("check:1"));
    assert_eq!(
        db.read(|c| Ok(
            c.query_row("SELECT COUNT(*) FROM autofix_session", [], |r| r
                .get::<_, i64>(0))?
        ))
        .unwrap(),
        1
    );
    db.reproject(&["autofix_session","autofix_subscription"],"autofix.",Arc::new(project)).unwrap();
    assert!(repo.get("afx_test").unwrap().seen.contains("check:1"));
    assert_eq!(db.read(|c|Ok(c.query_row("SELECT COUNT(*) FROM autofix_session",[],|r|r.get::<_,i64>(0))?)).unwrap(),1);
    // Audit event independently contains the capability marker needed by replay.
    assert!(
        db.read_events("afx_test", -1, 100)
            .unwrap()
            .events
            .iter()
            .any(|e| e.kind == BOUND && e.data["session_id"] == "ses_fix")
    );
}
#[test]
fn failed_transition_rolls_back_both_revision_and_audit() {
    let tmp = tempfile::tempdir().unwrap();
    let db = store(&tmp.path().join("db"));
    let repo = Repository::new(db.clone());
    repo.create(fixture()).unwrap();
    let before = db.aggregate_seq("afx_test").unwrap();
    assert!(
        repo.update("afx_test", 1, 1, |s| {
            s.failures = 9;
            Err("abort".into())
        })
        .is_err()
    );
    assert_eq!(repo.get("afx_test").unwrap().failures, 0);
    assert_eq!(db.aggregate_seq("afx_test").unwrap(), before);
}
