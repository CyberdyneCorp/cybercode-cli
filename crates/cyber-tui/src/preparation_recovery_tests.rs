use super::*;
use crate::{app::Action, model::Session};
use serde_json::json;
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
    app.attachments.chips.push(crate::attachments::Chip {
        kind: crate::attachments::Kind::Text,
        number: 1,
        payload: crate::attachments::Payload::Text {
            text: "attachment owner data".into(),
        },
    });
    app
}
fn request(actions: Vec<Action>) -> rewind::Request {
    let [Action::Rewind(request)] = actions.as_slice() else {
        panic!("one guarded owner request")
    };
    request.clone()
}
fn deliver(app: &mut App, request: rewind::Request, reply: Reply) -> Vec<Action> {
    rewind::apply(
        app,
        &mut Default::default(),
        request,
        Ok(rewind::Reply::Recovery(Box::new(
            recovery::Reply::Preparation(Box::new(reply)),
        ))),
    )
}
fn review() -> Review {
    serde_json::from_value(json!({"activity_id":"op_one","lifetime":{"version":2,"owner":{"activity_id":"op_one","session_id":"ses_one","session_created_id":"created","directory":"/project","worktree_id":null},"root_identity":"root","namespace_identity":"namespace","directory_identity":"directory","file_identity":"file","source_phase":"phase"},"native_sequence":0,"session_sequence":1,"confirmation":"a".repeat(64),"warning":"independent uncertainty remains","origin":null,"checkout_resources_digest":"d".repeat(64)})).unwrap()
}
fn receipt(id: &str) -> Receipt {
    let review = review();
    Receipt {
        activity_id: review.activity_id,
        request_id: id.into(),
        confirmation: review.confirmation,
        lifetime: review.lifetime,
        native_sequence: 0,
        session_sequence: 1,
        origin: None,
        source_admitted: Some(false),
        checkout_resources_digest: review.checkout_resources_digest,
    }
}
fn open(app: &mut App) {
    let req = request(app.open_snapshot_recovery());
    rewind::apply(
        app,
        &mut Default::default(),
        req,
        Ok(rewind::Reply::Recovery(Box::new(
            recovery::Reply::Journals(vec![]),
        ))),
    );
    let req = request(app.recovery_key(KeyCode::Char('p')));
    deliver(
        app,
        req,
        Reply::Candidates(vec![Candidate {
            activity_id: "op_one".into(),
            status: "unknown".into(),
            lifetime_available: true,
        }]),
    );
}
#[test]
fn cancellation_retains_key_before_admission_and_never_replays_after_reply_loss() {
    let mut app = app();
    open(&mut app);
    let req = request(app.recovery_key(KeyCode::Enter));
    let actions = deliver(&mut app, req, Reply::Discovered(None));
    let req = request(actions);
    assert!(matches!(
        req.command,
        rewind::Command::Recovery(recovery::Command::Preparation(Command::Review(_)))
    ));
    deliver(&mut app, req, Reply::Review(Box::new(review())));
    let confirm = request(app.recovery_key(KeyCode::Enter));
    let retained = app.rewind.recovery.preparation.request.clone().unwrap();
    assert!(
        matches!(&confirm.command, rewind::Command::Recovery(recovery::Command::Preparation(Command::Confirm(_, expected))) if expected == &retained)
    );
    assert!(app.rewind.recovery.preparation.review.is_none());
    assert!(app.recovery_key(KeyCode::Enter).is_empty());
    rewind::apply(
        &mut app,
        &mut Default::default(),
        confirm,
        Err("lost reply".into()),
    );
    assert!(app.recovery_key(KeyCode::Enter).is_empty());
    lookup_and_reconcile(&mut app, &retained);
    assert_eq!(app.composer.text(), "owner draft");
    assert!(app.items.is_empty());
    assert_eq!(app.attachments.chips.len(), 1);
    assert_eq!(
        app.attachments.chips[0].payload,
        crate::attachments::Payload::Text {
            text: "attachment owner data".into()
        }
    );
}
fn lookup_and_reconcile(app: &mut App, retained: &Confirmation) {
    let lookup = request(app.recovery_key(KeyCode::Char('r')));
    assert!(
        matches!(&lookup.command, rewind::Command::Recovery(recovery::Command::Preparation(Command::Lookup(_, expected))) if expected == retained)
    );
    deliver(
        app,
        lookup,
        Reply::Receipt(Box::new(receipt(&retained.request_id))),
    );
    assert!(!app.rewind.recovery.preparation.reconciled);
    let ack = request(app.recovery_key(KeyCode::Char('a')));
    deliver(
        app,
        ack,
        Reply::Receipt(Box::new(receipt(&retained.request_id))),
    );
    assert!(app.rewind.recovery.preparation.reconciled);
}

#[test]
fn fresh_terminal_discovers_committed_cancellation_and_only_reconciles_explicitly() {
    let mut app = app();
    open(&mut app);
    let req = request(app.recovery_key(KeyCode::Enter));
    assert!(
        deliver(
            &mut app,
            req,
            Reply::Discovered(Some(Box::new(receipt("original / #?%"))))
        )
        .is_empty()
    );
    assert_eq!(app.rewind.recovery.preparation.phase, Phase::Outcome);
    assert_eq!(
        app.rewind
            .recovery
            .preparation
            .request
            .as_ref()
            .unwrap()
            .request_id,
        "original / #?%"
    );
    assert!(!app.rewind.recovery.preparation.reconciled);
    assert!(app.recovery_key(KeyCode::Enter).is_empty());
    assert!(
        lines(&app)
            .iter()
            .any(|line| line.to_string().contains("not established"))
    );
    assert_eq!(app.composer.text(), "owner draft");
}
#[test]
fn missing_phase_foreign_location_and_stale_reply_never_grant_confirmation() {
    let mut app = app();
    open(&mut app);
    app.rewind.recovery.preparation.phase = Phase::Review;
    for mode in 0..3 {
        let req = request(app.preparation_request(Command::Review("op_one".into())));
        let mut invalid = review();
        match mode {
            0 => invalid.lifetime.source_phase = None,
            1 => invalid.lifetime.owner.directory = "/foreign".into(),
            _ => invalid.confirmation = "bad".into(),
        }
        deliver(&mut app, req, Reply::Review(Box::new(invalid)));
        assert!(app.rewind.recovery.preparation.review.is_none());
        assert!(app.recovery_key(KeyCode::Enter).is_empty());
    }
    let stale = request(app.preparation_request(Command::Review("op_one".into())));
    let current = request(app.preparation_request(Command::List));
    deliver(&mut app, stale, Reply::Review(Box::new(review())));
    assert!(app.rewind.recovery.busy);
    assert!(app.rewind.recovery.preparation.review.is_none());
    deliver(&mut app, current, Reply::Candidates(vec![]));
    assert_eq!(app.composer.text(), "owner draft");
}

#[tokio::test]
async fn actual_transport_reads_history_and_absence_still_requires_positive_review() {
    let server = crate::screens::Server::start().await;
    let created = server
        .client
        .post("/sessions", json!({"model":"test/main"}))
        .await
        .unwrap();
    let id = created["data"]["id"].as_str().unwrap();
    let mut app = server.load(id).await;
    app.composer.set_text("transport owner draft");
    let mut actions = app.open_snapshot_recovery();
    while let Some(action) = actions.pop() {
        let reply = crate::perform::perform_owned(&server.client, &app.session, action, None).await;
        actions.extend(crate::runner::apply(
            &mut app,
            reply,
            &mut Default::default(),
        ));
    }
    let list = app.recovery_key(KeyCode::Char('p')).pop().unwrap();
    let reply = crate::perform::perform_owned(&server.client, &app.session, list, None).await;
    crate::runner::apply(&mut app, reply, &mut Default::default());
    assert!(app.rewind.recovery.preparation.candidates.is_empty());
    let absent = request(app.preparation_request(Command::Discover("op_missing".into())));
    let reply = recovery::perform(
        &server.client,
        &absent,
        match &absent.command {
            rewind::Command::Recovery(command) => command,
            _ => unreachable!(),
        },
    )
    .await
    .unwrap();
    let recovery::Reply::Preparation(reply) = reply else {
        panic!("preparation reply")
    };
    let review = request(deliver(&mut app, absent, *reply));
    let command = match &review.command {
        rewind::Command::Recovery(command) => command,
        _ => unreachable!(),
    };
    let error = recovery::perform(&server.client, &review, command)
        .await
        .unwrap_err();
    rewind::apply(&mut app, &mut Default::default(), review, Err(error));
    assert!(app.rewind.recovery.preparation.review.is_none());
    assert!(app.rewind.recovery.preparation.request.is_none());
    assert_eq!(app.composer.text(), "transport owner draft");
    assert!(server.model.requests().is_empty());
    assert!(
        server
            .client
            .get(&format!(
                "/sessions/{id}/rewind/preparations/op_missing/receipt"
            ))
            .await
            .unwrap()["data"]
            .is_null()
    );
}
