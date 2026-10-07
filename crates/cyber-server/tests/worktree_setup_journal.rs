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

#[test]
fn explicit_retry_preserves_success_and_rejects_stale_attempt_acknowledgements() {
    use cyber_server::worktrees::CommandStatus;
    let store = open(DatabaseLocation::Memory);
    let commands = vec!["once".into(), "retry".into(), "later".into()];
    let journal =
        SetupJournal::new(Arc::clone(&store), &managed(), &commands, "ses_owner").unwrap();
    let first = journal.start_attempt(0).unwrap().started_revision.unwrap();
    journal
        .finish_attempt(0, first, CommandResult::Exited { code: Some(0) })
        .unwrap();
    let original = journal.start_attempt(1).unwrap().started_revision.unwrap();
    journal
        .finish_attempt(1, original, CommandResult::Exited { code: Some(7) })
        .unwrap();
    let snapshot = journal.snapshot().unwrap();
    journal
        .retry_failed(
            snapshot.revision,
            &snapshot.digest,
            1,
            "Dependency service restored",
        )
        .unwrap();
    assert!(
        journal
            .retry_failed(snapshot.revision, &snapshot.digest, 1, "Duplicate request")
            .is_err()
    );
    assert_eq!(
        journal.start(0).unwrap(),
        CommandDecision::Recorded(CommandResult::Exited { code: Some(0) })
    );
    let retry = journal.start_attempt(1).unwrap();
    assert_eq!(retry.decision, CommandDecision::Dispatch);
    let revision = retry.started_revision.unwrap();
    assert!(revision > original);
    assert!(
        journal
            .finish_attempt(1, original, CommandResult::Exited { code: Some(0) })
            .is_err()
    );
    assert!(
        journal
            .finish(1, CommandResult::Exited { code: Some(0) })
            .is_err()
    );
    assert_eq!(
        journal.snapshot().unwrap().commands[1],
        CommandStatus::Pending
    );
    journal
        .finish_attempt(1, revision, CommandResult::Exited { code: Some(0) })
        .unwrap();
    assert_eq!(journal.start(2).unwrap(), CommandDecision::Dispatch);
    let replay = SetupJournal::new(store, &managed(), &commands, "ses_other").unwrap();
    assert_eq!(
        replay.start(1).unwrap(),
        CommandDecision::Recorded(CommandResult::Exited { code: Some(0) })
    );
}

#[test]
fn recovery_refuses_unknown_successful_error_unstarted_and_changed_recipe_states() {
    for result in [
        None,
        Some(CommandResult::Exited { code: Some(0) }),
        Some(CommandResult::Failed {
            message: "unproven settlement".into(),
        }),
    ] {
        let store = open(DatabaseLocation::Memory);
        let journal = SetupJournal::new(
            Arc::clone(&store),
            &managed(),
            &["step".into()],
            "ses_owner",
        )
        .unwrap();
        let unstarted = journal.snapshot().unwrap();
        assert!(
            journal
                .retry_failed(unstarted.revision, &unstarted.digest, 0, "reviewed")
                .is_err()
        );
        journal.start(0).unwrap();
        if let Some(result) = result {
            journal.finish(0, result).unwrap();
        }
        let current = journal.snapshot().unwrap();
        assert!(
            journal
                .retry_failed(current.revision, &current.digest, 0, "reviewed")
                .is_err()
        );
        assert_eq!(journal.snapshot().unwrap(), current);
    }
    let store = open(DatabaseLocation::Memory);
    let journal = SetupJournal::new(
        Arc::clone(&store),
        &managed(),
        &["step".into()],
        "ses_owner",
    )
    .unwrap();
    journal.start(0).unwrap();
    journal
        .finish(0, CommandResult::Exited { code: None })
        .unwrap();
    let snapshot = journal.snapshot().unwrap();
    for (revision, digest, reason) in [
        (snapshot.revision - 1, snapshot.digest.as_str(), "reviewed"),
        (snapshot.revision, "foreign", "reviewed"),
        (snapshot.revision, snapshot.digest.as_str(), " "),
    ] {
        assert!(journal.retry_failed(revision, digest, 0, reason).is_err());
        assert_eq!(journal.snapshot().unwrap(), snapshot);
    }
    let changed =
        SetupJournal::new(store, &managed(), &["replacement".into()], "ses_owner").unwrap();
    assert!(changed.snapshot().is_err());
    journal
        .retry_failed(
            snapshot.revision,
            &snapshot.digest,
            0,
            "Signal acknowledged; retry explicitly approved",
        )
        .unwrap();
}

#[test]
fn racing_retry_reviews_authorize_only_one_new_attempt() {
    let store = open(DatabaseLocation::Memory);
    let journal = SetupJournal::new(
        Arc::clone(&store),
        &managed(),
        &["step".into()],
        "ses_owner",
    )
    .unwrap();
    journal.start(0).unwrap();
    journal
        .finish(0, CommandResult::Exited { code: Some(1) })
        .unwrap();
    let review = journal.snapshot().unwrap();
    let barrier = Arc::new(std::sync::Barrier::new(2));
    let threads: Vec<_> = (0..2)
        .map(|_| {
            let journal = SetupJournal::new(
                Arc::clone(&store),
                &managed(),
                &["step".into()],
                "ses_owner",
            )
            .unwrap();
            let review = review.clone();
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                barrier.wait();
                journal.retry_failed(review.revision, &review.digest, 0, "Explicit review")
            })
        })
        .collect();
    assert_eq!(
        threads
            .into_iter()
            .map(|thread| thread.join().unwrap())
            .filter(Result::is_ok)
            .count(),
        1
    );
    assert_eq!(journal.start(0).unwrap(), CommandDecision::Dispatch);
    assert!(journal.start(0).is_err());
}

#[test]
fn reviewed_prelaunch_failure_retries_survive_restart_and_fence_old_acknowledgements() {
    use cyber_server::worktrees::CommandStatus;
    let temp = tempfile::tempdir().unwrap();
    let location = DatabaseLocation::File(temp.path().join("preparation.db"));
    let commands = vec!["setup".into()];
    let original;
    {
        let journal =
            SetupJournal::new(open(location.clone()), &managed(), &commands, "ses_owner").unwrap();
        original = journal.start_attempt(0).unwrap().started_revision.unwrap();
        journal
            .finish_attempt(
                0,
                original,
                CommandResult::NotDispatched {
                    message: "Sandbox preparation refused launch".into(),
                },
            )
            .unwrap();
    }
    let journal = SetupJournal::new(open(location), &managed(), &commands, "ses_owner").unwrap();
    let reviewed = journal.snapshot().unwrap();
    assert!(matches!(
        reviewed.commands[0],
        CommandStatus::Finished {
            result: CommandResult::NotDispatched { .. },
        }
    ));
    assert!(
        journal
            .retry_failed(reviewed.revision - 1, &reviewed.digest, 0, "stale")
            .is_err()
    );
    assert_eq!(journal.snapshot().unwrap(), reviewed);
    journal
        .retry_failed(
            reviewed.revision,
            &reviewed.digest,
            0,
            "Preparation repaired and reviewed",
        )
        .unwrap();
    let next = journal.start_attempt(0).unwrap().started_revision.unwrap();
    assert!(
        journal
            .finish_attempt(
                0,
                original,
                CommandResult::NotDispatched {
                    message: "late".into()
                }
            )
            .is_err()
    );
    assert_eq!(
        journal.snapshot().unwrap().commands[0],
        CommandStatus::Pending
    );
    journal
        .finish_attempt(0, next, CommandResult::Exited { code: Some(0) })
        .unwrap();
}
