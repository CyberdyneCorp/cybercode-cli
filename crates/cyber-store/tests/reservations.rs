//! Durable reservation admission, uncertainty and transactional accounting regressions.

use std::sync::Arc;

use cyber_core::paths::DatabaseLocation;
use cyber_store::reservations::{
    self as ledger, Estimate, Filter, Receipt, ReconcileAction, Reconciliation, Request, Scope,
    State, TokenAmounts,
};
use cyber_store::{EventRegistry, Expected, NewEvent, Store, StoreOptions};
use serde_json::json;

fn store() -> Store {
    Store::open(StoreOptions::new(
        DatabaseLocation::Memory,
        EventRegistry::default(),
    ))
    .unwrap()
}

fn request(id: &str, tokens: u64, cost: Option<u64>) -> Request {
    Request {
        request_id: id.into(),
        session_id: "ses_owner".into(),
        provider: "test".into(),
        model: "main".into(),
        purpose: "turn".into(),
        call_id: None,
        bound_source: None,
        normalized_sha256: None,
        deadline_ms: None,
        estimate: Estimate {
            tokens: TokenAmounts {
                input: tokens,
                ..Default::default()
            },
            total_tokens: tokens,
            turns: 1,
            cost_microusd: cost,
        },
    }
}

fn scope(id: &str, tokens: u64, cost: Option<u64>) -> Scope {
    Scope {
        id: id.into(),
        max_tokens: Some(tokens),
        max_turns: Some(100),
        max_cost_microusd: cost,
        spent_tokens: 0,
        spent_turns: 0,
        spent_cost_microusd: 0,
        uncertain: false,
    }
}

fn admit(
    store: &Store,
    request: Request,
    scopes: Vec<Scope>,
) -> Result<ledger::Admission, cyber_store::StoreError> {
    store.transaction(move |tx| Ok(ledger::reserve(tx, &request, &scopes, 1)?))
}

fn observed(store: &Store, id: &str) -> ledger::Record {
    let id = id.to_owned();
    store
        .read(move |db| Ok(ledger::get(db, &id)?.unwrap()))
        .unwrap()
}

fn dispatched(store: &Store, id: &str) {
    let id = id.to_owned();
    store
        .transaction(move |tx| Ok(ledger::mark_dispatched(tx, &id, 2)?))
        .unwrap();
}

fn receipt(tokens: u64, cost: Option<u64>, complete: bool) -> Receipt {
    Receipt {
        tokens: Some(TokenAmounts {
            input: tokens,
            ..Default::default()
        }),
        turns: u64::from(complete),
        cost_microusd: cost,
        complete,
    }
}

#[test]
fn concurrent_last_budget_admission_reserves_every_scope_atomically() {
    let store = Arc::new(store());
    let attempts: Vec<_> = (0..8)
        .map(|i| {
            let store = Arc::clone(&store);
            std::thread::spawn(move || {
                admit(
                    &store,
                    request(&format!("req_{i}"), 60, Some(600_000)),
                    vec![
                        scope("run", 1000, Some(1_000_000)),
                        scope("daily", 1000, Some(1_000_000)),
                    ],
                )
            })
        })
        .collect();
    assert_eq!(
        attempts
            .into_iter()
            .map(|t| t.join().unwrap())
            .filter(Result::is_ok)
            .count(),
        1
    );
    let page = store
        .read(|db| Ok(ledger::list(db, &Filter::default())?))
        .unwrap();
    assert_eq!(page.records.len(), 1);
    assert_eq!(page.records[0].scopes.len(), 2);
}

#[test]
fn rejecting_one_ancestor_scope_leaves_no_partial_allocations() {
    let store = store();
    let failed = admit(
        &store,
        request("req", 10, Some(100)),
        vec![
            scope("child", 100, Some(1000)),
            scope("ancestor", 5, Some(1000)),
        ],
    );
    assert!(failed.unwrap_err().to_string().contains("ancestor"));
    assert!(
        store
            .read(|db| Ok(ledger::get(db, "req")?))
            .unwrap()
            .is_none()
    );
    assert_eq!(
        store
            .read(|db| Ok(db.query_row(
                "SELECT count(*) FROM budget_reservation_allocation",
                [],
                |r| r.get::<_, i64>(0)
            )?))
            .unwrap(),
        0
    );
}

#[test]
fn main_requests_reserve_turns_while_auxiliary_requests_reserve_only_their_usage() {
    let store = store();
    let mut cap = scope("session", 1000, None);
    cap.max_turns = Some(1);
    admit(&store, request("main", 1, None), vec![cap.clone()]).unwrap();
    let mut auxiliary = request("title", 1, None);
    auxiliary.estimate.turns = 0;
    admit(&store, auxiliary, vec![cap.clone()]).unwrap();
    let error = admit(&store, request("concurrent_child", 1, None), vec![cap]).unwrap_err();
    let cyber_store::StoreError::Projector { kind, reason } = error else {
        panic!("expected structured admission refusal")
    };
    assert_eq!(kind, "budget.reservation.exceeded");
    let detail: serde_json::Value = serde_json::from_str(&reason).unwrap();
    assert_eq!(detail["limit"], "max_turns");
    assert_eq!(detail["scope"], "session");
}

#[test]
fn correlated_class_envelopes_and_duplicate_admission_do_not_grant_more_dispatches() {
    let store = store();
    let mut r = request("req", 20, Some(100));
    r.estimate.tokens = TokenAmounts {
        input: 12,
        cache_read: 12,
        cache_write: 12,
        output: 8,
        reasoning: 8,
    };
    assert!(
        admit(&store, r.clone(), vec![scope("run", 20, Some(100))])
            .unwrap()
            .new
    );
    assert!(
        !admit(&store, r.clone(), vec![scope("run", 20, Some(100))])
            .unwrap()
            .new
    );
    dispatched(&store, "req");
    assert!(
        store
            .transaction(|tx| Ok(ledger::mark_dispatched(tx, "req", 3)?))
            .is_err()
    );
    r.model = "other".into();
    assert!(admit(&store, r, vec![scope("run", 20, Some(100))]).is_err());
}

#[test]
fn unpriced_work_can_use_token_only_caps_but_cannot_enter_a_usd_scope() {
    let store = store();
    admit(
        &store,
        request("tokens", 10, None),
        vec![scope("run", 100, None)],
    )
    .unwrap();
    assert!(
        admit(
            &store,
            request("usd", 10, None),
            vec![scope("dollars", 100, Some(1_000_000))]
        )
        .unwrap_err()
        .to_string()
        .contains("BudgetUnpricedError")
    );
    assert!(
        admit(
            &store,
            request("priced", 10, Some(100)),
            vec![scope("run", 100, Some(1_000_000))]
        )
        .unwrap_err()
        .to_string()
        .contains("BudgetUnpricedError")
    );
}

#[test]
fn incomplete_spend_and_integer_overflow_fail_before_any_admission() {
    let store = store();
    let mut uncertain = scope("run", 100, None);
    uncertain.uncertain = true;
    assert!(admit(&store, request("unknown", 10, None), vec![uncertain]).is_err());
    assert!(
        admit(
            &store,
            request("overflow", u64::MAX, None),
            vec![scope("run", 100, None)]
        )
        .is_err()
    );
    assert!(
        store
            .read(|db| Ok(ledger::list(db, &Filter::default())?))
            .unwrap()
            .records
            .is_empty()
    );
}

#[test]
fn only_confirmed_predispatch_cancellation_releases_without_owner_audit() {
    let store = store();
    admit(
        &store,
        request("early", 10, None),
        vec![scope("run", 10, None)],
    )
    .unwrap();
    store
        .transaction(|tx| Ok(ledger::release_before_dispatch(tx, "early", 2)?))
        .unwrap();
    store
        .transaction(|tx| Ok(ledger::release_before_dispatch(tx, "early", 3)?))
        .unwrap();
    admit(
        &store,
        request("late", 10, None),
        vec![scope("run", 10, None)],
    )
    .unwrap();
    dispatched(&store, "late");
    assert!(
        store
            .transaction(|tx| Ok(ledger::release_before_dispatch(tx, "late", 3)?))
            .is_err()
    );
    assert!(
        admit(
            &store,
            request("blocked", 1, None),
            vec![scope("run", 10, None)]
        )
        .is_err()
    );
}

#[test]
fn uncertain_holds_survive_database_restart_and_session_deletion() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("store.db");
    let open = || {
        Store::open(StoreOptions::new(
            DatabaseLocation::File(path.clone()),
            EventRegistry::default(),
        ))
        .unwrap()
    };
    {
        let store = open();
        store.transaction(|tx|{tx.execute("INSERT INTO session(id,title,directory,agent,model,mode,created_at,updated_at,last_seq)
            VALUES ('ses_owner','x','/r','build','test/main','default',1,1,0)",[])?;Ok(())}).unwrap();
        admit(
            &store,
            request("req", 100, Some(1_000_000)),
            vec![scope("run", 100, Some(1_000_000))],
        )
        .unwrap();
        dispatched(&store, "req");
        store
            .transaction(|tx| {
                tx.execute("DELETE FROM session WHERE id='ses_owner'", [])?;
                Ok(())
            })
            .unwrap();
    }
    let store = open();
    assert_eq!(observed(&store, "req").state, State::Uncertain);
    assert!(
        admit(
            &store,
            request("next", 1, Some(1)),
            vec![scope("run", 100, Some(1_000_000))]
        )
        .is_err()
    );
}

#[test]
fn partial_actuals_hold_only_the_remaining_bound_and_final_settlement_is_a_delta() {
    let store = store();
    admit(
        &store,
        request("first", 100, Some(1_000_000)),
        vec![scope("run", 150, Some(1_500_000))],
    )
    .unwrap();
    dispatched(&store, "first");
    let partial = store
        .transaction(|tx| {
            Ok(ledger::settle(
                tx,
                "first",
                "receipt_partial",
                &receipt(40, Some(400_000), false),
                3,
            )?)
        })
        .unwrap();
    assert_eq!(partial.record.state, State::Uncertain);
    assert_eq!(partial.delta.tokens.unwrap().input, 40);
    let mut after = scope("run", 150, Some(1_500_000));
    after.spent_tokens = 40;
    after.spent_cost_microusd = 400_000;
    admit(
        &store,
        request("second", 50, Some(500_000)),
        vec![after.clone()],
    )
    .unwrap();
    assert!(admit(&store, request("too_much", 1, Some(1)), vec![after]).is_err());
    let final_ = store
        .transaction(|tx| {
            Ok(ledger::settle(
                tx,
                "first",
                "receipt_final",
                &receipt(70, Some(700_000), true),
                4,
            )?)
        })
        .unwrap();
    assert_eq!(final_.record.state, State::Settled);
    assert_eq!(final_.delta.tokens.unwrap().input, 30);
    assert_eq!(final_.delta.cost_microusd, Some(300_000));
    let duplicate = store
        .transaction(|tx| {
            Ok(ledger::settle(
                tx,
                "first",
                "receipt_final",
                &receipt(70, Some(700_000), true),
                5,
            )?)
        })
        .unwrap();
    assert!(!duplicate.applied);
}

#[test]
fn all_five_actual_token_classes_are_preserved_and_must_not_decrease() {
    let store = store();
    admit(
        &store,
        request("req", 100, Some(1000)),
        vec![scope("run", 1000, Some(10_000))],
    )
    .unwrap();
    dispatched(&store, "req");
    let actual = Receipt {
        tokens: Some(TokenAmounts {
            input: 1,
            output: 2,
            reasoning: 3,
            cache_read: 4,
            cache_write: 5,
        }),
        turns: 0,
        cost_microusd: Some(100),
        complete: false,
    };
    let copy = actual.clone();
    store
        .transaction(move |tx| Ok(ledger::settle(tx, "req", "partial", &copy, 3)?))
        .unwrap();
    assert_eq!(observed(&store, "req").actual, Some(actual));
    assert!(
        store
            .transaction(|tx| Ok(ledger::settle(
                tx,
                "req",
                "bad",
                &receipt(0, Some(0), true),
                4
            )?))
            .is_err()
    );
}

#[test]
fn owner_reconciliation_is_revision_fenced_idempotent_and_preserves_late_overruns() {
    let store = store();
    admit(
        &store,
        request("req", 100, Some(1_000_000)),
        vec![scope("run", 100, Some(1_000_000))],
    )
    .unwrap();
    dispatched(&store, "req");
    let command = Reconciliation {
        operation_id: "op_release".into(),
        expected_revision: 2,
        owner: "local-owner".into(),
        reason: "provider confirms the disconnected request was not billed".into(),
        action: ReconcileAction::Release,
    };
    let copy = command.clone();
    let released = store
        .transaction(move |tx| Ok(ledger::reconcile(tx, "req", &copy, 4)?))
        .unwrap();
    assert_eq!(released.record.state, State::Released);
    assert!(released.delta.is_none());
    let copy = command.clone();
    assert!(
        !store
            .transaction(move |tx| Ok(ledger::reconcile(tx, "req", &copy, 5)?))
            .unwrap()
            .applied
    );
    let mut stale = command.clone();
    stale.operation_id = "op_stale".into();
    assert!(
        store
            .transaction(move |tx| Ok(ledger::reconcile(tx, "req", &stale, 5)?))
            .is_err()
    );
    let late = store
        .transaction(|tx| {
            Ok(ledger::settle(
                tx,
                "req",
                "late_provider_receipt",
                &receipt(150, Some(1_300_000), true),
                6,
            )?)
        })
        .unwrap();
    assert_eq!(late.record.state, State::Settled);
    assert_eq!(late.record.released_ms, Some(4));
    assert_eq!(late.record.overrun_tokens, 50);
    assert_eq!(late.record.overrun_cost_microusd, Some(300_000));
    assert_eq!(
        store
            .read(|db| Ok(ledger::audit(db, "req", 100)?))
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn diagnostics_paginate_by_stable_identity_and_keep_closed_history() {
    let store = store();
    for id in ["a", "b", "c"] {
        admit(&store, request(id, 1, None), vec![scope("run", 10, None)]).unwrap();
    }
    store
        .transaction(|tx| Ok(ledger::release_before_dispatch(tx, "b", 2)?))
        .unwrap();
    let first = store
        .read(|db| {
            Ok(ledger::list(
                db,
                &Filter {
                    limit: 1,
                    ..Default::default()
                },
            )?)
        })
        .unwrap();
    assert_eq!(first.records[0].request.request_id, "a");
    let next = store
        .read(move |db| {
            Ok(ledger::list(
                db,
                &Filter {
                    after: first.next,
                    limit: 1,
                    ..Default::default()
                },
            )?)
        })
        .unwrap();
    assert_eq!(next.records[0].request.request_id, "c");
    assert!(next.next.is_none());
    assert_eq!(
        store
            .read(|db| Ok(ledger::list(
                db,
                &Filter {
                    open_only: false,
                    ..Default::default()
                }
            )?))
            .unwrap()
            .records
            .len(),
        3
    );
}

#[test]
fn usage_projection_failure_rolls_back_settlement_and_duplicate_callback_charges_nothing() {
    let mut registry = EventRegistry::default();
    registry.register("test.usage.1").unwrap();
    registry.projector(|tx, event| {
        let actual: Receipt =
            serde_json::from_value(event.data["actual"].clone()).map_err(|e| e.to_string())?;
        let settled = ledger::settle(tx, "req", "stable_receipt", &actual, event.time_ms)
            .map_err(|e| e.to_string())?;
        if settled.applied {
            tx.execute(
                "INSERT INTO native_charge(tokens,cost) VALUES (?1,?2)",
                rusqlite::params![
                    i64::try_from(settled.delta.tokens.unwrap().total().unwrap()).unwrap(),
                    settled
                        .delta
                        .cost_microusd
                        .map(|n| i64::try_from(n).unwrap())
                ],
            )
            .map_err(|e| e.to_string())?;
        }
        if event.data["fail"] == true {
            return Err("injected native projection failure".into());
        }
        Ok(())
    });
    let store = Store::open(StoreOptions::new(DatabaseLocation::Memory, registry)).unwrap();
    store
        .transaction(|tx| {
            tx.execute(
                "CREATE TABLE native_charge(tokens INTEGER,cost INTEGER)",
                [],
            )?;
            Ok(())
        })
        .unwrap();
    admit(
        &store,
        request("req", 100, Some(1000)),
        vec![scope("run", 100, Some(1000))],
    )
    .unwrap();
    dispatched(&store, "req");
    let actual = receipt(50, Some(500), true);
    assert!(
        store
            .append(
                "ses_owner",
                Expected::Any,
                vec![NewEvent::new(
                    "test.usage.1",
                    json!({"actual":actual,"fail":true})
                )]
            )
            .is_err()
    );
    assert_eq!(observed(&store, "req").state, State::Uncertain);
    for _ in 0..2 {
        store
            .append(
                "ses_owner",
                Expected::Any,
                vec![NewEvent::new("test.usage.1", json!({"actual":actual}))],
            )
            .unwrap();
    }
    assert_eq!(
        store
            .read(
                |db| Ok(db.query_row("SELECT count(*) FROM native_charge", [], |r| r
                    .get::<_, i64>(0))?)
            )
            .unwrap(),
        1
    );
    assert_eq!(observed(&store, "req").state, State::Settled);
}

#[test]
fn deadline_and_normalized_provenance_are_durable_and_block_late_dispatch() {
    let store = store();
    let mut req = request("deadline", 5, None);
    req.deadline_ms = Some(10);
    req.call_id = Some("message-1".into());
    req.bound_source = Some("declared provider limits".into());
    req.normalized_sha256 = Some("a".repeat(64));
    admit(&store, req.clone(), vec![scope("owner", 10, None)]).unwrap();
    assert_eq!(observed(&store, "deadline").request, req);
    store
        .transaction(|tx| {
            assert!(matches!(
                ledger::mark_dispatched(tx, "deadline", 10),
                Err(ledger::LedgerError::Exceeded { .. })
            ));
            assert_eq!(ledger::get(tx, "deadline")?.unwrap().state, State::Reserved);
            ledger::release_before_dispatch(tx, "deadline", 11)?;
            Ok(())
        })
        .unwrap();
    req.request_id = "already-expired".into();
    assert!(
        store
            .transaction(move |tx| Ok(ledger::reserve(tx, &req, &[scope("owner", 10, None)], 10)?))
            .is_err()
    );
    assert!(
        store
            .read(|conn| Ok(ledger::get(conn, "already-expired")?))
            .unwrap()
            .is_none()
    );
}

#[test]
fn retries_share_one_turn_hold_and_confirmed_success_cancels_earlier_unknown_hold() {
    let store = store();
    store.transaction(|tx| {
        let mut ceiling=scope("owner",100,None);
        ceiling.max_turns=Some(1);
        let mut first=request("first-attempt",10,None);
        first.call_id=Some("canonical-message".into());
        ledger::reserve(tx,&first,std::slice::from_ref(&ceiling),1)?;
        ledger::mark_dispatched(tx,&first.request_id,2)?;
        let mut retry=first.clone();
        retry.request_id="retry".into();
        retry.estimate.turns=0;
        ledger::reserve(tx,&retry,std::slice::from_ref(&ceiling),3)?;
        ledger::mark_dispatched(tx,"retry",4)?;
        ledger::settle(tx,"retry","success",&Receipt{tokens:Some(TokenAmounts{input:1,..Default::default()}),turns:1,cost_microusd:None,complete:true},5)?;
        assert_eq!(ledger::get(tx,"first-attempt")?.unwrap().state,State::Uncertain);
        ceiling.spent_turns=1;
        // A zero-Turn follow-up under this canonical claim does not inherit the
        // earlier failed attempt's hold as a second Turn.
        retry.request_id="late-same-message".into();
        assert!(ledger::reserve(tx,&retry,std::slice::from_ref(&ceiling),6)?.new);
        let unrelated=request("next-message",1,None);
        assert!(matches!(ledger::reserve(tx,&unrelated,&[ceiling],7),Err(ledger::LedgerError::Exceeded{limit,..}) if limit=="max_turns"));
        Ok(())
    }).unwrap();
}
