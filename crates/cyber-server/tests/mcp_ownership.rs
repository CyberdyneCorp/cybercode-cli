//! Independent MCP receipts, rotating native-owner capabilities and restart fences.
use cyber_core::paths::DatabaseLocation;
use cyber_server::runtime::{McpConnectionOwner, McpConnectionPhase, Runtime, mcp_connections};
use cyber_store::{Expected, NewEvent, Store, StoreOptions};
use serde_json::json;
use std::sync::Arc;

const DIGEST: &str = "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
fn store() -> Arc<Store> {
    Arc::new(
        Store::open(StoreOptions::new(
            DatabaseLocation::Memory,
            Runtime::registry(),
        ))
        .unwrap(),
    )
}
fn admit(store: &Arc<Store>, root: &tempfile::TempDir) -> McpConnectionOwner {
    McpConnectionOwner::admit(store.clone(), root.path(), root.path(), "audit", DIGEST).unwrap()
}

#[test]
fn receipts_never_create_or_borrow_sessions_and_settlement_releases_the_name() {
    let store = store();
    let root = tempfile::tempdir().unwrap();
    let mut owner = admit(&store, &root);
    assert!(cyber_core::ids::has_prefix(&owner.record().id, "mcs"));
    owner.preparing().unwrap();
    owner.launching(vec!["wt_verified".into()]).unwrap();
    owner.connected().unwrap();
    assert!(
        McpConnectionOwner::admit(store.clone(), root.path(), root.path(), "audit", DIGEST)
            .is_err()
    );
    assert_eq!(
        store
            .read(|connection| connection
                .query_row("SELECT COUNT(*) FROM session", [], |row| row
                    .get::<_, i64>(0))
                .map_err(Into::into))
            .unwrap(),
        0
    );
    let record = owner.finish(true, "Location closed".into()).unwrap();
    assert_eq!(record.phase, McpConnectionPhase::Settled);
    assert_eq!(record.worktree_ids, vec!["wt_verified"]);
    let next = admit(&store, &root);
    assert_ne!(next.record().id, record.id);
    assert_eq!(mcp_connections(&store, root.path()).unwrap().len(), 2);
}

#[test]
fn disposal_is_settled_before_native_preparation_and_unknown_after_it() {
    let store = store();
    let root = tempfile::tempdir().unwrap();
    drop(admit(&store, &root));
    let mut owner = admit(&store, &root);
    owner.preparing().unwrap();
    drop(owner);
    let records = mcp_connections(&store, root.path()).unwrap();
    assert_eq!(
        records
            .iter()
            .filter(|record| record.phase == McpConnectionPhase::Settled)
            .count(),
        1
    );
    assert_eq!(
        records
            .iter()
            .filter(|record| record.phase == McpConnectionPhase::Unknown)
            .count(),
        1
    );
    assert!(
        McpConnectionOwner::admit(store.clone(), root.path(), root.path(), "audit", DIGEST)
            .is_err()
    );
}

#[test]
fn consumed_event_key_cannot_forge_acknowledgement_or_steal_a_current_owner() {
    let store = store();
    let root = tempfile::tempdir().unwrap();
    let mut owner = admit(&store, &root);
    owner.preparing().unwrap();
    let events = store
        .read_events(&owner.record().id, -1, 10)
        .unwrap()
        .events;
    let mut forged = events[1].data.clone();
    assert!(forged["previous_owner_key"].is_string());
    forged["record"]["phase"] = json!("settled");
    forged["record"]["status"] = json!("failed");
    forged["record"]["acknowledged"] = json!(true);
    forged["record"]["error"] = json!("forged native stop");
    forged["next_owner_hash"] = json!(DIGEST);
    let error = store
        .append(
            &owner.record().id,
            Expected::Seq(1),
            vec![NewEvent::new("mcp.status.changed.1", forged)],
        )
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("current native owner capability")
    );
    assert_eq!(store.aggregate_seq(&owner.record().id).unwrap(), Some(1));
    owner.launching(vec![]).unwrap();
    owner.connected().unwrap();
    owner.finish(true, "owned shutdown".into()).unwrap();
}

#[test]
fn late_acknowledgement_requires_the_same_retained_owner_and_pinned_identity() {
    let store = store();
    let root = tempfile::tempdir().unwrap();
    let mut owner = admit(&store, &root);
    owner.preparing().unwrap();
    owner.launching(vec![]).unwrap();
    owner.connected().unwrap();
    owner
        .finish(false, "shutdown observation expired".into())
        .unwrap();
    assert!(
        McpConnectionOwner::admit(store.clone(), root.path(), root.path(), "audit", DIGEST)
            .is_err()
    );
    owner
        .finish(true, "retained native owner acknowledged shutdown".into())
        .unwrap();
    drop(admit(&store, &root));
}

#[test]
fn invalid_transitions_roll_back_and_do_not_consume_owner_authority() {
    let store = store();
    let root = tempfile::tempdir().unwrap();
    let mut owner = admit(&store, &root);
    assert!(owner.connected().is_err());
    assert_eq!(store.aggregate_seq(&owner.record().id).unwrap(), Some(0));
    owner.preparing().unwrap();
    assert!(owner.launching(vec!["ses_not_worktree".into()]).is_err());
    assert_eq!(owner.record().phase, McpConnectionPhase::Preparing);
    owner.launching(vec!["wt_verified".into()]).unwrap();
    owner.connected().unwrap();
    owner.finish(true, "owned shutdown".into()).unwrap();
    assert!(owner.connected().is_err());
}

#[test]
fn failed_terminal_commit_keeps_the_name_fenced() {
    let mut registry = Runtime::registry();
    registry.projector(|_, event| {
        if event.kind == "mcp.status.changed.1" && event.data["record"]["phase"] == "settled" {
            return Err("terminal commit rejected".into());
        }
        Ok(())
    });
    let store =
        Arc::new(Store::open(StoreOptions::new(DatabaseLocation::Memory, registry)).unwrap());
    let root = tempfile::tempdir().unwrap();
    let mut owner = admit(&store, &root);
    owner.preparing().unwrap();
    owner.launching(vec![]).unwrap();
    owner.connected().unwrap();
    assert!(owner.finish(true, "owned shutdown".into()).is_err());
    assert_eq!(owner.record().phase, McpConnectionPhase::Running);
    drop(owner);
    assert_eq!(
        mcp_connections(&store, root.path()).unwrap()[0].phase,
        McpConnectionPhase::Unknown
    );
    assert!(
        McpConnectionOwner::admit(store.clone(), root.path(), root.path(), "audit", DIGEST)
            .is_err()
    );
}

#[test]
fn independent_locations_and_names_do_not_contend_but_competing_runtimes_do() {
    let store = store();
    let root = tempfile::tempdir().unwrap();
    let barrier = Arc::new(std::sync::Barrier::new(2));
    let workers: Vec<_> = (0..2)
        .map(|_| {
            let store = store.clone();
            let directory = root.path().to_path_buf();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                McpConnectionOwner::admit(store, &directory, &directory, "audit", DIGEST)
            })
        })
        .collect();
    let results: Vec<_> = workers
        .into_iter()
        .map(|worker| worker.join().unwrap())
        .collect();
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    let other = tempfile::tempdir().unwrap();
    let _other = admit(&store, &other);
    let _name =
        McpConnectionOwner::admit(store.clone(), root.path(), root.path(), "different", DIGEST)
            .unwrap();
}

#[test]
fn unknown_ownership_survives_database_restart_without_claiming_a_live_server() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("cyber.db");
    let store = Arc::new(
        Store::open(StoreOptions::new(
            DatabaseLocation::File(path.clone()),
            Runtime::registry(),
        ))
        .unwrap(),
    );
    let mut owner = admit(&store, &root);
    owner.preparing().unwrap();
    owner.launching(vec![]).unwrap();
    owner.connected().unwrap();
    drop(owner);
    drop(store);
    let store = Arc::new(
        Store::open(StoreOptions::new(
            DatabaseLocation::File(path),
            Runtime::registry(),
        ))
        .unwrap(),
    );
    let record = &mcp_connections(&store, root.path()).unwrap()[0];
    assert_eq!(record.phase, McpConnectionPhase::Unknown);
    assert_eq!(record.acknowledged, Some(false));
    assert!(
        McpConnectionOwner::admit(store.clone(), root.path(), root.path(), "audit", DIGEST)
            .is_err()
    );
}
