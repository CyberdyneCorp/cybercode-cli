use super::*;
use crate::model::Session;
#[cfg(any(target_os = "macos", target_os = "linux"))]
use crate::perform::Msg;
use serde_json::json;

fn request(actions: Vec<Action>) -> rewind::Request {
    let [Action::Rewind(request)] = actions.as_slice() else {
        panic!("one owner request")
    };
    request.clone()
}
fn deliver(app: &mut App, request: rewind::Request, reply: Reply) {
    rewind::apply(
        app,
        &mut Default::default(),
        request,
        Ok(rewind::Reply::Recovery(Box::new(reply))),
    );
}
fn origin() -> cyber_server::runtime::SnapshotOrigin {
    serde_json::from_value(json!({"version":1,"session_id":"ses_one","session_created_id":"created","activity_id":"act_one","worktree_id":null,"binding":{"directory":"/project","root_identity":"root","namespace_identity":"namespace","journal_id":"restore_one","journal_identity":"journal"}})).unwrap()
}
fn review() -> Review {
    Review {
        journal_id: "restore_one".into(),
        action: RecoveryAction::Rollback,
        origin: Some(origin()),
        filesystem_complete: false,
        acknowledged: false,
        confirmation: Some("a".repeat(64)),
        paths: vec![],
        exclusions: None,
        recorded_created_parents: None,
        conflicts: vec![],
    }
}
fn app() -> App {
    let mut app = App::new(
        Session {
            id: "ses_one".into(),
            directory: "/project".into(),
            ..Default::default()
        },
        vec![],
        "cyber",
    );
    app.composer.set_text("owner draft");
    app
}
#[test]
fn recovery_drops_stale_generations_and_foreign_sessions_without_changing_drafts() {
    let mut app = app();
    let old = request(app.open_snapshot_recovery());
    let current = request(app.open_snapshot_recovery());
    deliver(&mut app, old, Reply::Journals(vec![]));
    assert!(app.rewind.recovery.busy);
    deliver(&mut app, current.clone(), Reply::Journals(vec![]));
    assert!(!app.rewind.recovery.busy);
    app.set_session(Session {
        id: "ses_foreign".into(),
        directory: "/foreign".into(),
        ..Default::default()
    });
    app.composer.set_text("foreign draft");
    deliver(&mut app, current, Reply::Journals(vec![]));
    assert_eq!(app.composer.text(), "foreign draft");
    assert_ne!(app.rewind.phase, rewind::Phase::Recovery);
}
#[test]
fn ambiguous_confirmation_retains_exact_lookup_and_never_reuses_enter_to_mutate() {
    let mut app = app();
    let initial = request(app.open_snapshot_recovery());
    deliver(&mut app, initial, Reply::Journals(vec![]));
    app.rewind.recovery.phase = Phase::Requests;
    app.rewind.recovery.discovered = true;
    app.rewind.recovery.journal = Some("restore_one".into());
    let selection = request(app.recovery_key(KeyCode::Char('b')));
    deliver(&mut app, selection, Reply::Review(Box::new(review())));
    let confirmation = request(app.recovery_key(KeyCode::Enter));
    let rewind::Command::Recovery(Command::Confirm(body)) = confirmation.command.clone() else {
        panic!("explicit confirmation")
    };
    assert!(app.rewind.recovery.review.is_none());
    assert!(app.recovery_key(KeyCode::Enter).is_empty());
    rewind::apply(
        &mut app,
        &mut Default::default(),
        confirmation,
        Err("reply lost".into()),
    );
    assert!(app.recovery_key(KeyCode::Enter).is_empty());
    let lookup = request(app.recovery_key(KeyCode::Char('r')));
    assert_eq!(
        lookup.command,
        rewind::Command::Recovery(Command::Lookup(body.clone()))
    );
    let mut outcome = Outcome {
        request: body,
        original: origin(),
        resolution: None,
        state: SnapshotRecoveryState::Acknowledged,
        receipt: None,
        journal_acknowledged: None,
    };
    outcome.original.session_id = "ses_foreign".into();
    deliver(&mut app, lookup, Reply::Outcome(Box::new(outcome)));
    assert!(app.rewind.recovery.outcome.is_none());
    assert!(app.recovery_key(KeyCode::Char('a')).is_empty());
    assert_eq!(app.composer.text(), "owner draft");
}

#[test]
fn failed_discovery_does_not_license_a_fresh_attempt() {
    let mut app = app();
    let load = request(app.open_snapshot_recovery());
    deliver(
        &mut app,
        load,
        Reply::Journals(vec![SnapshotRecovery {
            id: "restore_one".into(),
            filesystem_complete: false,
            paths: vec![],
            applied: vec![],
        }]),
    );
    let discovery = request(app.recovery_key(KeyCode::Enter));
    deliver(&mut app, discovery, Reply::Requests(vec![]));
    let discovery = request(app.recovery_key(KeyCode::Char('r')));
    rewind::apply(
        &mut app,
        &mut Default::default(),
        discovery,
        Err("discovery unavailable".into()),
    );
    assert!(app.recovery_key(KeyCode::Char('k')).is_empty());
    assert!(app.recovery_key(KeyCode::Char('b')).is_empty());
    assert_eq!(app.composer.text(), "owner draft");
}

#[test]
fn recovery_review_escapes_paths_and_legacy_evidence_has_no_confirm_action() {
    let mut app = app();
    let _ = app.open_snapshot_recovery();
    let mut review = review();
    review
        .paths
        .push(cyber_server::runtime::SnapshotRecoveryPath {
            path: "one\ntwo\x1b[31m".into(),
            state: "unapplied".into(),
            current: None,
            recorded_before: None,
            recorded_after: None,
            retained: None,
            proposed: None,
            changes: false,
        });
    review.origin = None;
    review.confirmation = None;
    app.rewind.recovery.busy = false;
    app.rewind.recovery.phase = Phase::Review;
    app.rewind.recovery.review = Some(review);
    let shown = lines(&app)
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(shown.contains("one\\ntwo"));
    assert!(!shown.contains('\x1b'));
    assert!(shown.contains("Confirmation unavailable"));
    assert!(app.recovery_key(KeyCode::Enter).is_empty());
    assert_eq!(app.composer.text(), "owner draft");
}

#[test]
fn only_positively_settled_attempts_allow_a_new_review_without_replay() {
    for state in [
        SnapshotRecoveryState::Pending,
        SnapshotRecoveryState::Unknown,
        SnapshotRecoveryState::CancelledBeforeOrigin,
        SnapshotRecoveryState::CancelledBeforeEffects,
        SnapshotRecoveryState::ResolvedByAnotherRequest,
    ] {
        let mut app = app();
        let _ = app.open_snapshot_recovery();
        let body = Confirmation {
            journal_id: "restore_one".into(),
            action: RecoveryAction::Keep,
            confirmation: "a".repeat(64),
            request_id: "retained".into(),
        };
        app.rewind.recovery.busy = false;
        app.rewind.recovery.phase = Phase::Outcome;
        app.rewind.recovery.journal = Some(body.journal_id.clone());
        app.rewind.recovery.request = Some(body.clone());
        app.rewind.recovery.outcome = Some(Outcome {
            request: body,
            original: origin(),
            resolution: None,
            state,
            receipt: None,
            journal_acknowledged: None,
        });
        let actions = app.recovery_key(KeyCode::Char('b'));
        if matches!(
            state,
            SnapshotRecoveryState::Pending | SnapshotRecoveryState::Unknown
        ) {
            assert!(actions.is_empty());
        } else {
            assert!(matches!(
                request(actions).command,
                rewind::Command::Recovery(Command::Review(_))
            ));
        }
        assert_eq!(app.composer.text(), "owner draft");
    }
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
async fn run(server: &crate::screens::Server, app: &mut App, mut actions: Vec<Action>) {
    while let Some(action) = actions.pop() {
        let message =
            crate::perform::perform_owned(&server.client, &app.session, action, None).await;
        actions.extend(crate::runner::apply(app, message, &mut Default::default()));
    }
}
#[tokio::test]
#[cfg(any(target_os = "macos", target_os = "linux"))]
async fn terminal_recovery_handles_real_partial_interruption_and_discovers_a_lost_reply_after_reopen()
 {
    for action in [KeyCode::Char('k'), KeyCode::Char('b')] {
        native_recovery(action).await;
    }
}
#[cfg(any(target_os = "macos", target_os = "linux"))]
async fn native_recovery(action: KeyCode) {
    let mut fixture = native_fixture().await;
    interrupt_restore(&mut fixture).await;
    let inference = fixture.server.model.requests().len();
    confirm_without_applying_reply(&mut fixture, action).await;
    reopen_and_acknowledge(&fixture, inference).await;
}
#[cfg(any(target_os = "macos", target_os = "linux"))]
struct NativeFixture {
    server: crate::screens::Server,
    client: cyber_client::Client,
    directory: std::path::PathBuf,
    id: String,
    messages: serde_json::Value,
    app: App,
}
#[cfg(any(target_os = "macos", target_os = "linux"))]
async fn native_fixture() -> NativeFixture {
    let server = crate::screens::Server::start_config(json!({}), json!({}), true).await;
    let directory = server.root().join("recovery-work");
    std::fs::create_dir(&directory).unwrap();
    bulk(&directory, "before");
    let client = server.client.at(directory.to_str().unwrap());
    let created = client
        .post(
            "/sessions",
            json!({"model":"test/main","mode":"accept-edits"}),
        )
        .await
        .unwrap();
    let id = created["data"]["id"].as_str().unwrap().to_owned();
    server.prompt(&id, "before checkpoint").await;
    server.wait_answered(&id).await;
    bulk(&directory, "after");
    server.prompt(&id, "after checkpoint").await;
    server.wait_answered(&id).await;
    let messages = client
        .get(&format!("/sessions/{id}/messages"))
        .await
        .unwrap();
    let mut app = server.load(&id).await;
    app.composer.set_text("owner draft");
    NativeFixture {
        server,
        client,
        directory,
        id,
        messages,
        app,
    }
}
#[cfg(any(target_os = "macos", target_os = "linux"))]
async fn interrupt_restore(fixture: &mut NativeFixture) {
    let (server, client, directory, id, app) = (
        &fixture.server,
        &fixture.client,
        &fixture.directory,
        &fixture.id,
        &mut fixture.app,
    );
    let actions = app.open_rewind(rewind::Intent::Menu);
    run(server, app, actions).await;
    app.rewind.selected = 0;
    app.rewind.target = cyber_server::runtime::RevertTarget::Code;
    let actions = app.rewind_key(crossterm::event::KeyEvent::from(KeyCode::Enter));
    run(server, app, actions).await;
    assert_eq!(app.rewind.phase, rewind::Phase::Review);
    let pending = app
        .rewind_key(crossterm::event::KeyEvent::from(KeyCode::Enter))
        .pop()
        .unwrap();
    let (owned, session) = ((*client).clone(), app.session.clone());
    let task = tokio::spawn(async move {
        crate::perform::perform_owned(&owned, &session, pending, None).await
    });
    wait_for_partial(directory, &task).await;
    assert!(
        server.host.runtime.interrupt_for_owner(id).await.is_err(),
        "partial native effects must retain uncertainty"
    );
    let reply = task.await.unwrap();
    assert!(matches!(&reply, Ok(Msg::Rewind { result, .. }) if result.is_err()));
    crate::runner::apply(app, reply, &mut Default::default());
    assert_eq!(app.composer.text(), "owner draft");
}
#[cfg(any(target_os = "macos", target_os = "linux"))]
async fn wait_for_partial(
    directory: &std::path::Path,
    task: &tokio::task::JoinHandle<Result<Msg, String>>,
) {
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(45);
    while std::fs::read_to_string(directory.join("bulk_0000")).unwrap() != "before" {
        assert!(
            !task.is_finished(),
            "restore finished without a partial replacement"
        );
        assert!(
            tokio::time::Instant::now() < deadline,
            "source exchange was not observed"
        );
        tokio::time::sleep(std::time::Duration::from_millis(1)).await;
    }
    assert_eq!(
        std::fs::read_to_string(directory.join("bulk_1279")).unwrap(),
        "after"
    );
}
#[cfg(any(target_os = "macos", target_os = "linux"))]
async fn confirm_without_applying_reply(fixture: &mut NativeFixture, action: KeyCode) {
    let (server, client, app) = (&fixture.server, &fixture.client, &mut fixture.app);
    std::fs::write(fixture.directory.join("unrelated-owner"), "owner data").unwrap();
    let actions = app.open_snapshot_recovery();
    run(server, app, actions).await;
    let actions = app.recovery_key(KeyCode::Enter);
    run(server, app, actions).await;
    assert!(app.rewind.recovery.requests.is_empty());
    let actions = app.recovery_key(action);
    run(server, app, actions).await;
    assert!(
        app.rewind
            .recovery
            .review
            .as_ref()
            .unwrap()
            .confirmation
            .is_some()
    );
    let shown = crate::screens::render(app, 120, 35, &Default::default());
    assert!(shown.contains("SNAPSHOT RECOVERY") && shown.contains("REVIEW"));
    let confirm = app.recovery_key(KeyCode::Enter).pop().unwrap();
    let acknowledgement = crate::perform::perform_owned(client, &app.session, confirm, None)
        .await
        .unwrap();
    let Msg::Rewind { result, .. } = acknowledgement else {
        panic!("recovery response")
    };
    assert!(result.is_ok()); // Deliberately discard the successful reply.
    let expected = if action == KeyCode::Char('k') {
        "before"
    } else {
        "after"
    };
    assert_eq!(
        std::fs::read_to_string(fixture.directory.join("bulk_0000")).unwrap(),
        expected
    );
    assert_eq!(
        std::fs::read_to_string(fixture.directory.join("unrelated-owner")).unwrap(),
        "owner data"
    );
}
#[cfg(any(target_os = "macos", target_os = "linux"))]
async fn reopen_and_acknowledge(fixture: &NativeFixture, inference: usize) {
    let NativeFixture {
        server,
        client,
        directory,
        id,
        messages,
        ..
    } = fixture;
    let mut fresh = server.load(id).await;
    fresh.composer.set_text("new terminal draft");
    let actions = fresh.open_snapshot_recovery();
    run(server, &mut fresh, actions).await;
    let actions = fresh.recovery_key(KeyCode::Enter);
    run(server, &mut fresh, actions).await;
    assert_eq!(fresh.rewind.recovery.requests.len(), 1);
    let actions = fresh.recovery_key(KeyCode::Enter);
    run(server, &mut fresh, actions).await;
    assert_eq!(
        fresh.rewind.recovery.outcome.as_ref().unwrap().state,
        SnapshotRecoveryState::Acknowledged
    );
    assert!(
        fresh
            .rewind
            .recovery
            .outcome
            .as_ref()
            .unwrap()
            .journal_acknowledged
            .is_none()
    );
    std::fs::write(directory.join("bulk_0000"), "later owner edit").unwrap();
    let actions = fresh.recovery_key(KeyCode::Char('a'));
    run(server, &mut fresh, actions).await;
    assert_eq!(
        fresh
            .rewind
            .recovery
            .outcome
            .as_ref()
            .unwrap()
            .journal_acknowledged,
        Some(true)
    );
    assert_eq!(
        std::fs::read_to_string(directory.join("bulk_0000")).unwrap(),
        "later owner edit"
    );
    assert_eq!(fresh.composer.text(), "new terminal draft");
    assert_eq!(
        client
            .get(&format!("/sessions/{id}/messages"))
            .await
            .unwrap(),
        *messages
    );
    assert_eq!(server.model.requests().len(), inference);
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn bulk(directory: &std::path::Path, contents: &str) {
    for index in 0..1280 {
        std::fs::write(directory.join(format!("bulk_{index:04}")), contents).unwrap();
    }
}
