use std::sync::Arc;

use cyber_core::{paths::DatabaseLocation, worktrees::Managed};
use cyber_server::{
    runtime::Runtime,
    worktrees::{CommandDecision, CommandResult, SetupJournal},
};
use cyber_store::{Store, StoreOptions};

fn managed() -> Managed {
    Managed {
        id: "wt_original".into(),
        name: "journal".into(),
        path: "/work/worktrees/journal".into(),
        common_dir: "/work/repo/.git".into(),
        branch: "cyber/journal".into(),
        base: "base".into(),
        ready: true,
        included: vec![],
    }
}

fn open(location: DatabaseLocation) -> Arc<Store> {
    Arc::new(Store::open(StoreOptions::new(location, Runtime::registry())).unwrap())
}

#[test]
fn unfinished_intent_survives_reopening_and_cannot_be_hidden_by_config_changes() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("journal.db");
    let commands = vec!["side effects".into()];
    {
        let store = open(DatabaseLocation::File(path.clone()));
        let journal = SetupJournal::new(store, &managed(), &commands, "ses_owner").unwrap();
        assert_eq!(journal.start(0).unwrap(), CommandDecision::Dispatch);
    }
    let store = open(DatabaseLocation::File(path));
    let journal =
        SetupJournal::new(Arc::clone(&store), &managed(), &commands, "ses_other").unwrap();
    assert!(
        journal
            .start(0)
            .unwrap_err()
            .to_string()
            .contains("outcome unknown")
    );
    for commands in [vec!["different".into()], vec![]] {
        let changed =
            SetupJournal::new(Arc::clone(&store), &managed(), &commands, "ses_other").unwrap();
        assert!(
            changed
                .validate()
                .unwrap_err()
                .to_string()
                .contains("configuration or ownership changed")
        );
    }
}

#[test]
fn settled_commands_replay_without_redispatch_and_settlement_is_idempotent() {
    let store = open(DatabaseLocation::Memory);
    let commands = vec!["first".into(), "second".into()];
    let journal =
        SetupJournal::new(Arc::clone(&store), &managed(), &commands, "ses_owner").unwrap();
    assert!(journal.start(1).is_err());
    assert!(
        journal
            .finish(0, CommandResult::Exited { code: Some(0) })
            .is_err()
    );
    assert_eq!(journal.start(0).unwrap(), CommandDecision::Dispatch);
    let success = CommandResult::Exited { code: Some(0) };
    journal.finish(0, success.clone()).unwrap();
    journal.finish(0, success.clone()).unwrap();
    assert!(
        journal
            .finish(0, CommandResult::Exited { code: Some(7) })
            .is_err()
    );
    let replay = SetupJournal::new(store, &managed(), &commands, "ses_other").unwrap();
    assert_eq!(replay.start(0).unwrap(), CommandDecision::Recorded(success));
    assert_eq!(replay.start(1).unwrap(), CommandDecision::Dispatch);
    let failure = CommandResult::Failed {
        message: "delivery failed".into(),
    };
    replay.finish(1, failure.clone()).unwrap();
    assert_eq!(replay.start(1).unwrap(), CommandDecision::Recorded(failure));
}

#[test]
fn failed_or_signalled_command_does_not_allow_later_commands() {
    for code in [Some(7), None] {
        let store = open(DatabaseLocation::Memory);
        let journal = SetupJournal::new(
            store,
            &managed(),
            &["first".into(), "second".into()],
            "ses_owner",
        )
        .unwrap();
        assert_eq!(journal.start(0).unwrap(), CommandDecision::Dispatch);
        journal.finish(0, CommandResult::Exited { code }).unwrap();
        assert!(journal.start(1).is_err());
    }
}

#[test]
fn journal_replays_beyond_one_event_page() {
    let store = open(DatabaseLocation::Memory);
    let commands = vec!["command".into(); 251];
    let journal =
        SetupJournal::new(Arc::clone(&store), &managed(), &commands, "ses_owner").unwrap();
    for index in 0..commands.len() {
        assert_eq!(journal.start(index).unwrap(), CommandDecision::Dispatch);
        journal
            .finish(index, CommandResult::Exited { code: Some(0) })
            .unwrap();
    }
    let replay = SetupJournal::new(store, &managed(), &commands, "ses_other").unwrap();
    assert_eq!(
        replay.start(250).unwrap(),
        CommandDecision::Recorded(CommandResult::Exited { code: Some(0) })
    );
}

#[test]
fn concurrent_admission_authorizes_only_one_dispatch() {
    let store = open(DatabaseLocation::Memory);
    let barrier = Arc::new(std::sync::Barrier::new(2));
    let jobs: Vec<_> = (0..2)
        .map(|_| {
            let journal = SetupJournal::new(
                Arc::clone(&store),
                &managed(),
                &["command".into()],
                "ses_owner",
            )
            .unwrap();
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                barrier.wait();
                journal.start(0)
            })
        })
        .collect();
    let decisions: Vec<_> = jobs.into_iter().map(|job| job.join().unwrap()).collect();
    assert_eq!(
        decisions
            .iter()
            .filter(|decision| matches!(decision, Ok(CommandDecision::Dispatch)))
            .count(),
        1
    );
    assert_eq!(
        decisions
            .iter()
            .filter(|decision| decision.is_err())
            .count(),
        1
    );
}

#[test]
fn recreated_worktree_has_independent_setup_while_legacy_ownership_requires_recovery() {
    let store = open(DatabaseLocation::Memory);
    let mut owned = managed();
    let commands = vec!["command".into()];
    let journal = SetupJournal::new(Arc::clone(&store), &owned, &commands, "ses_owner").unwrap();
    assert_eq!(journal.start(0).unwrap(), CommandDecision::Dispatch);
    journal
        .finish(0, CommandResult::Exited { code: Some(0) })
        .unwrap();
    owned.id = "wt_recreated".into();
    let recreated = SetupJournal::new(Arc::clone(&store), &owned, &commands, "ses_owner").unwrap();
    assert_eq!(recreated.start(0).unwrap(), CommandDecision::Dispatch);
    owned.id.clear();
    assert!(SetupJournal::new(store, &owned, &commands, "ses_owner").is_err());
}
