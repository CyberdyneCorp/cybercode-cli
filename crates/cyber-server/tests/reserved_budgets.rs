//! Reserved-mode boundaries use the real runtime, store and finite offline adapter.
mod support;
use cyber_core::budget::{Budget, Enforcement};
use cyber_llm::{ErrorKind, FinishReason, LlmEvent, Usage, adapters::ScriptStep};
use cyber_server::runtime::{
    Admission, CreateSession, Delivery, ReservationAction, ReservationActual, ReservationQuery,
    ReservationReconcile,
};
use support::{Harness, Setup, text};

async fn create(h: &Harness, parent: Option<&str>, tokens: u64) -> String {
    h.runtime
        .create_session(CreateSession {
            directory: h.repo.display().to_string(),
            model: "test/main".into(),
            parent_id: parent.map(str::to_string),
            title: Some("Budget test".into()),
            budget: Some(Budget {
                max_tokens: Some(tokens),
                enforcement: Enforcement::Reserved,
                ..Default::default()
            }),
            ..Default::default()
        })
        .await
        .unwrap()
        .id
}
async fn prompt(h: &Harness, id: &str) {
    h.runtime
        .admit(id, Admission::text("continue", Delivery::Steer))
        .await
        .unwrap();
    h.settle(id).await;
}
fn setup(turns: Vec<Vec<ScriptStep>>) -> Setup {
    Setup {
        scripts: vec![("test/main", turns)],
        ..Default::default()
    }
}

#[tokio::test]
async fn actuals_keep_ui_evidence_and_charge_subtree_once() {
    let h = Harness::new(setup(vec![text("one"), text("two")]));
    let root = create(&h, None, 110).await;
    let child = create(&h, Some(&root), 1000).await;
    prompt(&h, &child).await;
    assert_eq!(h.models.requests("test/main").len(), 1);
    let state = h.state(&child).await;
    assert_eq!(state.totals.usage.input, 100);
    assert_eq!(state.totals.usage.output, 10);
    let records = h
        .runtime
        .budget_reservations(ReservationQuery::default())
        .unwrap()
        .reservations;
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].state, "settled");
    assert_eq!(records[0].held_tokens, 0);
    let ended = h
        .store
        .read_events(&child, -1, 100)
        .unwrap()
        .events
        .into_iter()
        .find(|e| e.kind == "session.step.ended.1")
        .unwrap();
    assert_eq!(ended.data["usage"]["input"], 100);
    assert_eq!(ended.data["usage_accounted"], true);
    prompt(&h, &root).await;
    assert_eq!(h.models.requests("test/main").len(), 1);
    let restarted = h.restart();
    assert_eq!(
        restarted.state(&child).await.unwrap().totals.usage.input,
        100
    );
}

#[tokio::test]
async fn transport_error_after_finish_retains_hold_until_audited_settlement() {
    let usage = Usage {
        input: 20,
        output: 4,
        reasoning: 3,
        cache_read: 2,
        cache_write: 1,
    };
    let h = Harness::new(setup(vec![
        vec![
            ScriptStep::Event(LlmEvent::Usage(usage)),
            ScriptStep::Event(LlmEvent::Finish {
                reason: FinishReason::Stop,
            }),
            ScriptStep::Error {
                error: ErrorKind::InvalidRequest,
                message: "connection lost".into(),
            },
        ],
        text("later"),
    ]));
    let id = create(&h, None, 1000).await;
    prompt(&h, &id).await;
    let page = h
        .runtime
        .budget_reservations(ReservationQuery::default())
        .unwrap();
    let record = &page.reservations[0];
    assert_eq!(record.state, "uncertain");
    assert!(!record.actual.as_ref().unwrap().complete);
    assert_eq!(h.state(&id).await.totals.usage, usage);
    let report = h.runtime.session_usage_breakdown(&id).await.unwrap();
    assert_eq!(
        report.by_model[0].tokens, usage,
        "partial usage remains visible even without a completed call"
    );
    assert_eq!(
        report.by_model[0].cost,
        record.actual.as_ref().unwrap().cost_usd,
        "known priced partial charges remain visible with zero finished calls"
    );
    let restarted = h.restart();
    let command = ReservationReconcile {
        operation_id: "owner-tested".into(),
        expected_revision: record.revision,
        reason: "Checked offline provider receipt".into(),
        action: ReservationAction::Settle {
            actual: ReservationActual {
                tokens: Some(usage),
                turns: 0,
                cost_usd: Some(0.00005),
                complete: true,
            },
        },
    };
    let result = restarted
        .reconcile_budget_reservation(&record.request_id, command.clone())
        .await
        .unwrap();
    assert_eq!(result.reservation.state, "settled");
    restarted
        .reconcile_budget_reservation(&record.request_id, command)
        .await
        .unwrap();
    assert_eq!(restarted.state(&id).await.unwrap().totals.usage, usage);
    assert_eq!(
        restarted
            .budget_reservation(&record.request_id)
            .unwrap()
            .unwrap()
            .audit
            .len(),
        1
    );
    let after = restarted.session_usage_breakdown(&id).await.unwrap();
    assert_eq!(after.by_model[0].steps, 1);
    assert!((after.by_model[0].cost.unwrap() - 0.00005).abs() < 1e-12);
    // A later cumulative complete correction adds its financial delta, not a
    // second successful provider request or canonical Turn.
    restarted
        .reconcile_budget_reservation(
            &record.request_id,
            ReservationReconcile {
                operation_id: "owner-later-correction".into(),
                expected_revision: result.reservation.revision,
                reason: "later fixture provider cost correction".into(),
                action: ReservationAction::Settle {
                    actual: ReservationActual {
                        tokens: Some(usage),
                        turns: 0,
                        cost_usd: Some(0.00006),
                        complete: true,
                    },
                },
            },
        )
        .await
        .unwrap();
    let after = restarted.session_usage_breakdown(&id).await.unwrap();
    assert_eq!(after.by_model[0].steps, 1);
    assert!((after.by_model[0].cost.unwrap() - 0.00006).abs() < 1e-12);
    let stats = restarted
        .usage_stats(&cyber_server::runtime::StatsRequest {
            period: cyber_server::runtime::Period::All,
            project: None,
            zone: Some("UTC".into()),
        })
        .unwrap();
    assert_eq!(stats.totals.steps, 1);
    assert_eq!(stats.totals.turns, 0);
    assert_eq!(stats.models[0].steps, 1);
    assert_eq!(stats.origins[0].steps, 1);
    let insights = restarted
        .usage_insights(&cyber_server::runtime::InsightsRequest {
            window: cyber_server::runtime::InsightWindow::Day,
            project: None,
        })
        .unwrap();
    assert_eq!(insights.steps, 1);
    assert_eq!(
        insights.total_tokens,
        usage.input + usage.output + usage.reasoning + usage.cache_read + usage.cache_write
    );
}

#[tokio::test]
async fn unpriced_usd_refuses_without_a_provider_call() {
    let h = Harness::new(Setup {
        scripts: vec![("other/main", vec![text("unused")])],
        ..Default::default()
    });
    let id = h
        .runtime
        .create_session(CreateSession {
            directory: h.repo.display().to_string(),
            model: "other/main".into(),
            title: Some("Test".into()),
            budget: Some(Budget {
                max_cost_usd: Some(1.0),
                enforcement: Enforcement::Reserved,
                ..Default::default()
            }),
            ..Default::default()
        })
        .await
        .unwrap()
        .id;
    prompt(&h, &id).await;
    assert!(h.models.requests("other/main").is_empty());
    assert!(
        h.runtime
            .budget_reservations(ReservationQuery::default())
            .unwrap()
            .reservations
            .is_empty()
    );
    assert!(
        h.store
            .read_events(&id, -1, 100)
            .unwrap()
            .events
            .iter()
            .any(|e| e.kind == "session.step.failed.1"
                && e.data.to_string().contains("BudgetUnpricedError"))
    );
}

struct Unbounded(std::sync::Arc<cyber_llm::adapters::ScriptedAdapter>);
impl cyber_llm::Adapter for Unbounded {
    fn stream(
        &self,
        request: cyber_llm::LlmRequest,
    ) -> futures::future::BoxFuture<'_, Result<cyber_llm::EventStream, cyber_llm::LlmError>> {
        cyber_llm::Adapter::stream(&*self.0, request)
    }
}
struct UnboundedModels {
    models: std::sync::Arc<support::Models>,
    priced: bool,
}
impl cyber_server::runtime::ModelResolver for UnboundedModels {
    fn resolve(&self, name: &str) -> Result<cyber_server::runtime::ResolvedModel, String> {
        let mut resolved = self.models.resolve(name)?;
        resolved.adapter = std::sync::Arc::new(Unbounded(self.models.adapter(name)));
        if !self.priced {
            resolved.cost = None;
        }
        Ok(resolved)
    }
    fn role(&self, _: cyber_llm::catalog::ModelRole) -> Option<String> {
        None
    }
}
fn unbounded_runtime(h: &Harness, priced: bool) -> cyber_server::runtime::Runtime {
    use cyber_server::runtime::*;
    Runtime::new(RuntimeOptions {
        store: h.store.clone(),
        resolver: std::sync::Arc::new(UnboundedModels {
            models: h.models.clone(),
            priced,
        }),
        tools: h.tools.clone(),
        global_config_dir: h.dir.path().join("global"),
        shell: "zsh".into(),
        claude_compat: true,
        compaction: Default::default(),
        retry: Default::default(),
        max_steps: None,
        today: None,
        interactive: true,
        snapshots: std::sync::Arc::new(NoSnapshots),
    })
}
#[tokio::test]
async fn unsupported_bounds_refuse_and_usd_unpriced_error_has_precedence() {
    for priced in [true, false] {
        let h = Harness::new(setup(vec![text("unused")]));
        let runtime = unbounded_runtime(&h, priced);
        let id = runtime
            .create_session(CreateSession {
                directory: h.repo.display().to_string(),
                model: "test/main".into(),
                title: Some("Offline".into()),
                budget: Some(Budget {
                    max_tokens: Some(1000),
                    max_cost_usd: (!priced).then_some(1.0),
                    enforcement: Enforcement::Reserved,
                    ..Default::default()
                }),
                ..Default::default()
            })
            .await
            .unwrap()
            .id;
        runtime
            .admit(&id, Admission::text("continue", Delivery::Steer))
            .await
            .unwrap();
        runtime.wait_idle(&id).await;
        assert!(h.models.requests("test/main").is_empty());
        let expected = if priced {
            "BudgetBoundUnavailableError"
        } else {
            "BudgetUnpricedError"
        };
        assert!(h.store.read_events(&id,-1,100).unwrap().events.iter().any(|e|e.kind=="session.step.failed.1"&&e.data.to_string().contains(expected)),"expected {expected}");
    }
}

#[tokio::test]
async fn concurrent_children_share_the_last_request_allowance() {
    use cyber_llm::Adapter;
    let h = Harness::new(setup(vec![text("only one"), text("must not run")]));
    let root = create(&h, None, 110).await;
    let a = create(&h, Some(&root), 1000).await;
    let b = create(&h, Some(&root), 1000).await;
    let resolved = cyber_server::runtime::ModelResolver::resolve(&*h.models, "test/main").unwrap();
    let first = h
        .runtime
        .operation_asker(&a)
        .await
        .unwrap()
        .guard_model(&resolved, "title", None)
        .await
        .unwrap();
    let second = h
        .runtime
        .operation_asker(&b)
        .await
        .unwrap()
        .guard_model(&resolved, "title", None)
        .await
        .unwrap();
    let (left, right) = tokio::join!(
        first.stream(resolved.template.clone()),
        second.stream(resolved.template.clone())
    );
    assert_eq!(usize::from(left.is_ok()) + usize::from(right.is_ok()), 1);
    assert_eq!(h.models.requests("test/main").len(), 1);
    drop((left, right));
    first.flush().await.ok();
    second.flush().await.ok();
    let records = h
        .runtime
        .budget_reservations(ReservationQuery::default())
        .unwrap()
        .reservations;
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].state, "uncertain");
}

struct SlowBounded {
    bounded: std::sync::Arc<cyber_llm::adapters::ScriptedAdapter>,
    calls: std::sync::Arc<std::sync::atomic::AtomicUsize>,
}
impl cyber_llm::Adapter for SlowBounded {
    fn prepare_reserved(
        &self,
        request: cyber_llm::LlmRequest,
        context: u64,
    ) -> Result<
        cyber_llm::reservations::PreparedReservation,
        cyber_llm::reservations::BoundUnavailable,
    > {
        cyber_llm::Adapter::prepare_reserved(&*self.bounded, request, context)
    }
    fn stream(
        &self,
        _: cyber_llm::LlmRequest,
    ) -> futures::future::BoxFuture<'_, Result<cyber_llm::EventStream, cyber_llm::LlmError>> {
        Box::pin(async move {
            self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            futures::future::pending().await
        })
    }
}
#[tokio::test]
async fn cancellation_during_opening_and_wall_deadline_keep_unknown_allocations() {
    use cyber_llm::Adapter;
    for wall in [false, true] {
        let h = Harness::new(setup(vec![text("not consumed")]));
        let id = h
            .runtime
            .create_session(CreateSession {
                directory: h.repo.display().to_string(),
                model: "test/main".into(),
                title: Some("Offline".into()),
                budget: Some(Budget {
                    max_tokens: Some(1000),
                    max_wall_seconds: wall.then_some(0.02),
                    enforcement: Enforcement::Reserved,
                    ..Default::default()
                }),
                ..Default::default()
            })
            .await
            .unwrap()
            .id;
        let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mut resolved =
            cyber_server::runtime::ModelResolver::resolve(&*h.models, "test/main").unwrap();
        resolved.adapter = std::sync::Arc::new(SlowBounded {
            bounded: h.models.adapter("test/main"),
            calls: calls.clone(),
        });
        let owner = h
            .runtime
            .operation_asker(&id)
            .await
            .unwrap()
            .guard_model(&resolved, "title", None)
            .await
            .unwrap();
        if wall {
            assert!(owner.stream(resolved.template.clone()).await.is_err());
        } else {
            assert!(
                tokio::time::timeout(
                    std::time::Duration::from_millis(20),
                    owner.stream(resolved.template.clone())
                )
                .await
                .is_err()
            );
        }
        owner.flush().await.ok();
        assert!(owner.reservation_owned());
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
        let record = &h
            .runtime
            .budget_reservations(ReservationQuery::default())
            .unwrap()
            .reservations[0];
        assert_eq!(record.state, "uncertain");
        assert_eq!(record.held_tokens, 110);
        assert!(!h.runtime.session_usage(&id).unwrap().total.usage_complete);
        assert!(h.runtime.delete(&id).await.is_err());
    }
}

#[tokio::test]
async fn opening_retry_claims_one_canonical_turn_and_keeps_unknown_token_hold() {
    let h = Harness::new(setup(vec![
        vec![ScriptStep::Error {
            error: ErrorKind::RateLimit,
            message: "retry before response".into(),
        }],
        text("recovered"),
        text("must not run"),
    ]));
    let id = h
        .runtime
        .create_session(CreateSession {
            directory: h.repo.display().to_string(),
            model: "test/main".into(),
            title: Some("Retry".into()),
            budget: Some(Budget {
                max_tokens: Some(1000),
                max_turns: Some(1),
                enforcement: Enforcement::Reserved,
                ..Default::default()
            }),
            ..Default::default()
        })
        .await
        .unwrap()
        .id;
    prompt(&h, &id).await;
    assert_eq!(h.models.requests("test/main").len(), 2);
    let records = h
        .runtime
        .budget_reservations(ReservationQuery::default())
        .unwrap()
        .reservations;
    assert_eq!(records.len(), 2);
    assert_eq!(
        records.iter().map(|r| r.estimated.turns_upper).sum::<u64>(),
        1
    );
    assert_eq!(
        records
            .iter()
            .filter_map(|r| r.actual.as_ref())
            .map(|r| r.turns)
            .sum::<u64>(),
        1
    );
    assert!(
        records
            .iter()
            .any(|r| r.state == "uncertain" && r.held_tokens > 0)
    );
    assert_eq!(h.state(&id).await.totals.steps, 1);
    prompt(&h, &id).await;
    assert_eq!(h.models.requests("test/main").len(), 2);
}

fn record_mcp_input(h: &Harness, session: &str, message: &str) {
    cyber_store::mcp_serve::record_input(
        &h.store,
        &cyber_store::mcp_serve::McpServeInput {
            message_id: message.into(),
            session_id: session.into(),
            mcp_client: "origin-fixture-client".into(),
            depth: 0,
            admitted_ms: 1,
            idempotency_key: None,
        },
    )
    .unwrap();
}
async fn origin_prompt(h: &Harness, session: &str, message: &str, source: &str) {
    let mut input = Admission::text("origin check", Delivery::Queue);
    input.message_id = Some(message.into());
    input.source = source.into();
    h.runtime.admit(session, input).await.unwrap();
    h.settle(session).await;
}
#[tokio::test]
async fn reserved_receipts_keep_channel_and_mcp_origin_after_later_input_and_reconciliation() {
    for source in ["channel", "mcp"] {
        let usage = Usage {
            input: 20,
            output: 4,
            ..Usage::default()
        };
        let h = Harness::new(setup(vec![
            vec![
                ScriptStep::Event(LlmEvent::Usage(usage)),
                ScriptStep::Event(LlmEvent::Finish {
                    reason: FinishReason::Stop,
                }),
                ScriptStep::Error {
                    error: ErrorKind::InvalidRequest,
                    message: "fixture lost final transport".into(),
                },
            ],
            text("later user turn"),
        ]));
        let id = create(&h, None, 2000).await;
        let message = format!("msg_origin_{source}");
        if source == "mcp" {
            record_mcp_input(&h, &id, &message);
        }
        origin_prompt(
            &h,
            &id,
            &message,
            if source == "mcp" {
                "mcp_prompt"
            } else {
                source
            },
        )
        .await;
        let reservation = h
            .runtime
            .budget_reservations(ReservationQuery {
                session_id: Some(id.clone()),
                ..Default::default()
            })
            .unwrap()
            .reservations
            .remove(0);
        assert_eq!(reservation.state, "uncertain");
        prompt(&h, &id).await;
        let restarted = h.restart();
        restarted
            .reconcile_budget_reservation(
                &reservation.request_id,
                ReservationReconcile {
                    operation_id: format!("origin-settle-{source}"),
                    expected_revision: reservation.revision,
                    reason: "offline fixture receipt confirmed".into(),
                    action: ReservationAction::Settle {
                        actual: ReservationActual {
                            tokens: Some(usage),
                            turns: 0,
                            cost_usd: Some(0.001),
                            complete: true,
                        },
                    },
                },
            )
            .await
            .unwrap();
        let request = reservation.request_id;
        let rows=h.store.read(move|db|{
            let mut q=db.prepare("SELECT origin,mcp_client,SUM(input_tokens),SUM(output_tokens),COUNT(*) FROM usage_step WHERE reservation_id=?1 GROUP BY origin,mcp_client")?;
            Ok(q.query_map([request],|row|Ok((row.get::<_,String>(0)?,row.get::<_,Option<String>>(1)?,row.get::<_,i64>(2)?,row.get::<_,i64>(3)?,row.get::<_,i64>(4)?)))?.collect::<Result<Vec<_>,_>>()?)
        }).unwrap();
        assert_eq!(
            rows,
            vec![(
                source.to_owned(),
                (source == "mcp").then(|| "origin-fixture-client".into()),
                20,
                4,
                2
            )]
        );
        let user_rows = h
            .store
            .read(move |db| {
                Ok(db.query_row(
                    "SELECT COUNT(*) FROM usage_step WHERE session_id=?1 AND origin='user'",
                    [id],
                    |row| row.get::<_, i64>(0),
                )?)
            })
            .unwrap();
        assert_eq!(
            user_rows, 1,
            "the later user turn keeps its own attribution"
        );
    }
}
#[tokio::test]
async fn owned_auxiliary_request_uses_its_message_origin_after_a_new_input_is_promoted() {
    use cyber_llm::{Adapter, collect};
    use cyber_server::runtime::ModelResolver;
    let h = Harness::new(setup(vec![
        text("first MCP"),
        text("later user"),
        text("auxiliary result"),
    ]));
    let id = create(&h, None, 2000).await;
    let message = "msg_auxiliary_origin";
    record_mcp_input(&h, &id, message);
    // Capture from an admitted queued input before StepStarted is available.
    let mut input = Admission::text("first MCP", Delivery::Queue);
    input.message_id = Some(message.into());
    input.source = "mcp_prompt".into();
    input.resume = false;
    h.runtime.admit(&id, input).await.unwrap();
    let mut asker = h.runtime.operation_asker(&id).await.unwrap();
    asker.message_id = message.into();
    let resolved = h.models.resolve("test/main").unwrap();
    let guarded = asker
        .guard_model(&resolved, "web_summary", Some("call_owned_origin".into()))
        .await
        .unwrap();
    // First actually drain the queued MCP input, then promote independent user work.
    h.runtime.resume(&id).await.unwrap();
    h.settle(&id).await;
    prompt(&h, &id).await;
    let stream = guarded.stream(resolved.template.clone()).await.unwrap();
    let result = collect(stream, |_| {}).await.unwrap();
    guarded.flush().await.unwrap();
    assert_eq!(result.text, "auxiliary result");
    let request = guarded.request_ids().pop().unwrap();
    let (origin,client,tokens)=h.store.read(move|db|Ok(db.query_row("SELECT origin,mcp_client,SUM(input_tokens+output_tokens) FROM usage_step WHERE reservation_id=?1",[request],|row|Ok((row.get::<_,String>(0)?,row.get::<_,Option<String>>(1)?,row.get::<_,i64>(2)?)))?)).unwrap();
    assert_eq!(origin, "mcp");
    assert_eq!(client.as_deref(), Some("origin-fixture-client"));
    assert_eq!(tokens, 110);
}
