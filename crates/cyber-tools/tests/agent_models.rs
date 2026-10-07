//! Agent model selection uses trusted profiles without overriding explicit choices.
mod support;

use cyber_llm::LlmRequest;
use cyber_llm::adapters::ScriptedAdapter;
use cyber_llm::catalog::{ModelRef, ModelRole};
use cyber_server::runtime::*;
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::Duration;
use support::Fixture;
use support::flow::text;

struct Models(Arc<ScriptedAdapter>);

impl ModelResolver for Models {
    fn resolve(&self, reference: &str) -> Result<ResolvedModel, String> {
        let model = ModelRef::parse(reference).map_err(|e| e.to_string())?;
        if !matches!(model.model.as_str(), "main" | "other") || model.provider != "test" {
            return Err(format!("ModelUnavailableError: {reference}"));
        }
        if model
            .variant
            .as_deref()
            .is_some_and(|v| !matches!(v, "low" | "high"))
        {
            return Err(format!("VariantUnavailableError: {reference}"));
        }
        Ok(ResolvedModel {
            adapter: self.0.clone(),
            template: LlmRequest {
                model: model.model.clone(),
                body: json!({"variant": model.variant}),
                ..Default::default()
            },
            provider: model.provider,
            model: model.model,
            context_limit: 200_000,
            cost: None,
            prefers_apply_patch: false,
        })
    }

    fn role(&self, _: ModelRole) -> Option<String> {
        None
    }
}

struct Case {
    f: Fixture,
    adapter: Arc<ScriptedAdapter>,
    runtime: Runtime,
}

impl Case {
    fn new(replies: usize, config: Value) -> Self {
        let f = Fixture::new();
        f.set_config(config);
        let adapter = Arc::new(ScriptedAdapter::new(
            (0..replies).map(|_| text("done")).collect(),
        ));
        let runtime = runtime(&f, adapter.clone());
        f.host.attach(runtime.clone());
        Self {
            f,
            adapter,
            runtime,
        }
    }

    async fn session(&self, model: &str) -> String {
        self.create(model, false).await.id
    }

    async fn create(&self, model: &str, model_is_default: bool) -> SessionInfo {
        self.runtime
            .create_session(CreateSession {
                directory: self.f.repo.display().to_string(),
                model: model.into(),
                model_is_default,
                title: Some("Model selection".into()),
                ..Default::default()
            })
            .await
            .unwrap()
    }

    async fn restart(&mut self) {
        self.runtime.shutdown().await;
        self.runtime = runtime(&self.f, self.adapter.clone());
        self.f.host.attach(self.runtime.clone());
    }

    async fn prompt(&self, id: &str) {
        self.runtime
            .admit(id, Admission::text("continue", Delivery::Queue))
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(5), self.runtime.wait_idle(id))
            .await
            .unwrap();
    }
}

#[tokio::test]
async fn agent_variant_is_applied_to_an_explicit_bare_model() {
    let c = Case::new(
        1,
        json!({"agents":{"build":{"model":"test/other","variant":"low"}}}),
    );
    let id = c.session("test/main").await;
    c.prompt(&id).await;
    let requests = c.adapter.requests();
    assert_eq!(
        requests[0].model, "main",
        "explicit model overrides the agent model"
    );
    assert_eq!(requests[0].body["variant"], "low");
    assert_eq!(
        c.runtime.state(&id).await.unwrap().info.model,
        "test/main#low"
    );
}

#[tokio::test]
async fn variant_changes_and_removal_do_not_turn_a_bare_choice_into_an_explicit_variant() {
    let c = Case::new(3, json!({"agents":{"build":{"variant":"low"}}}));
    let id = c.session("test/main").await;
    c.prompt(&id).await;
    c.f.set_config(json!({"agents":{"build":{"variant":"high"}}}));
    c.prompt(&id).await;
    c.f.set_config(json!({}));
    c.prompt(&id).await;
    let requests = c.adapter.requests();
    let variants: Vec<_> = requests.iter().map(|r| r.body["variant"].clone()).collect();
    assert_eq!(variants, vec![json!("low"), json!("high"), Value::Null]);
    assert_eq!(c.runtime.state(&id).await.unwrap().info.model, "test/main");
}

fn runtime(f: &Fixture, adapter: Arc<ScriptedAdapter>) -> Runtime {
    Runtime::new(RuntimeOptions {
        store: f.store.clone(),
        resolver: Arc::new(Models(adapter)),
        tools: f.host.clone(),
        global_config_dir: f.dir.path().join("global"),
        shell: "bash".into(),
        claude_compat: false,
        compaction: CompactionConfig {
            auto: false,
            ..Default::default()
        },
        retry: cyber_llm::RetryPolicy::default(),
        max_steps: None,
        today: None,
        interactive: false,
        snapshots: Arc::new(NoSnapshots),
    })
}

#[tokio::test]
async fn explicit_variant_wins_over_agent_model_and_variant_changes() {
    let c = Case::new(
        2,
        json!({"agents":{"build":{"model":"test/other#low","variant":"low"}}}),
    );
    let id = c.session("test/main#high").await;
    c.prompt(&id).await;
    c.f.set_config(json!({"agents":{"build":{"model":"test/other#high","variant":"low"}}}));
    c.prompt(&id).await;
    let requests = c.adapter.requests();
    assert!(
        requests
            .iter()
            .all(|r| r.model == "main" && r.body["variant"] == "high")
    );
    let rows = c.f.store.read_events(&id, -1, 500).unwrap().events;
    assert!(
        rows[0].data.get("selection").is_none(),
        "explicit creation retains historical payload shape"
    );
    assert!(!rows.iter().any(|r| r.kind == "session.model.switched.1"));
}

#[tokio::test]
async fn inherited_agent_models_follow_profile_changes_and_return_to_the_original_default() {
    let c = Case::new(
        3,
        json!({"agents":{"build":{"model":"test/other#high","variant":"low"}}}),
    );
    let session = c.create("test/main#low", true).await;
    assert_eq!(session.model, "test/other#low");
    c.prompt(&session.id).await;
    c.f.set_config(json!({"agents":{"build":{"model":"test/main","variant":"high"}}}));
    c.prompt(&session.id).await;
    c.f.set_config(json!({}));
    c.prompt(&session.id).await;
    let requests = c.adapter.requests();
    let choices: Vec<_> = requests
        .iter()
        .map(|r| (r.model.clone(), r.body["variant"].clone()))
        .collect();
    assert_eq!(
        choices,
        vec![
            ("other".into(), json!("low")),
            ("main".into(), json!("high")),
            ("main".into(), json!("low"))
        ]
    );
    let rows = c.f.store.read_events(&session.id, -1, 500).unwrap().events;
    assert_eq!(
        rows[0].data["selection"],
        json!({"reference":"test/main#low","agent_default":true})
    );
    assert!(
        rows.iter()
            .filter(|r| r.kind == "session.model.switched.1")
            .all(|r| r.data["automatic"] == true)
    );
    assert_eq!(
        c.runtime.list(&ListFilter::default()).unwrap().sessions[0].model,
        "test/main#low"
    );
}

#[tokio::test]
async fn selecting_the_visible_inherited_model_makes_it_explicit_once() {
    let mut c = Case::new(
        2,
        json!({"agents":{"build":{"model":"test/other","variant":"low"}}}),
    );
    let session = c.create("test/main", true).await;
    c.runtime
        .switch_model(&session.id, "test/other#low")
        .await
        .unwrap();
    c.runtime
        .switch_model(&session.id, "test/other#low")
        .await
        .unwrap();
    let rows = c.f.store.read_events(&session.id, -1, 500).unwrap().events;
    let switches: Vec<_> = rows
        .iter()
        .filter(|r| r.kind == "session.model.switched.1")
        .collect();
    assert_eq!(switches.len(), 1);
    assert_eq!(
        switches[0].data,
        json!({"from":"test/other#low","to":"test/other#low"})
    );
    let fork = c.runtime.fork(&session.id, None).await.unwrap();
    c.restart().await;
    c.f.set_config(json!({"agents":{"build":{"model":"test/main","variant":"high"}}}));
    c.prompt(&session.id).await;
    c.prompt(&fork.id).await;
    assert!(
        c.adapter
            .requests()
            .iter()
            .all(|r| r.model == "other" && r.body["variant"] == "low")
    );
}

#[tokio::test]
async fn replay_and_fork_preserve_explicit_bare_model_variant_defaults() {
    let mut c = Case::new(3, json!({"agents":{"build":{"variant":"low"}}}));
    let id = c.session("test/main").await;
    c.prompt(&id).await;
    let fork = c.runtime.fork(&id, None).await.unwrap();
    let fork_rows = c.f.store.read_events(&fork.id, -1, 500).unwrap().events;
    assert_eq!(
        fork_rows[0].data["selection"],
        json!({"reference":"test/main","agent_default":false})
    );
    c.restart().await;
    c.f.set_config(json!({"agents":{"build":{"variant":"high"}}}));
    c.prompt(&id).await;
    c.prompt(&fork.id).await;
    let requests = c.adapter.requests();
    assert_eq!(requests.len(), 3);
    assert!(requests[1..].iter().all(|r| r.body["variant"] == "high"));
}

#[tokio::test]
async fn replay_and_fork_keep_inherited_models_inherited() {
    let mut c = Case::new(
        2,
        json!({"agents":{"build":{"model":"test/other","variant":"low"}}}),
    );
    let session = c.create("test/main", true).await;
    let fork = c.runtime.fork(&session.id, None).await.unwrap();
    c.restart().await;
    c.f.set_config(json!({"agents":{"build":{"model":"test/main","variant":"high"}}}));
    c.prompt(&session.id).await;
    c.prompt(&fork.id).await;
    assert!(
        c.adapter
            .requests()
            .iter()
            .all(|r| r.model == "main" && r.body["variant"] == "high")
    );
}

#[tokio::test]
async fn an_agent_model_can_be_used_without_a_location_default() {
    let c = Case::new(1, json!({"agents":{"build":{"model":"test/other"}}}));
    let session = c.create("", true).await;
    assert_eq!(session.model, "test/other");
    c.prompt(&session.id).await;
    assert_eq!(c.adapter.requests()[0].model, "other");
}

#[tokio::test]
async fn unavailable_variants_leave_input_unpromoted_until_configuration_repair() {
    let c = Case::new(1, json!({"agents":{"build":{"variant":"missing"}}}));
    let id = c.session("test/main").await;
    c.prompt(&id).await;
    let state = c.runtime.state(&id).await.unwrap();
    assert!(state.epoch.is_none());
    assert_eq!(state.info.model, "test/main");
    assert_eq!(state.inbox[0].status, InputStatus::Pending);
    assert!(c.adapter.requests().is_empty());
    c.f.set_config(json!({"agents":{"build":{"variant":"low"}}}));
    c.runtime.wake(&id).await.unwrap();
    c.runtime.wait_idle(&id).await;
    assert_eq!(c.adapter.requests()[0].body["variant"], "low");
}

#[tokio::test]
async fn unavailable_inherited_models_are_refused_before_session_creation() {
    let c = Case::new(0, json!({"agents":{"build":{"model":"test/missing"}}}));
    let result = c
        .runtime
        .create_session(CreateSession {
            id: Some("ses_unavailable".into()),
            directory: c.f.repo.display().to_string(),
            model: "test/main".into(),
            model_is_default: true,
            ..Default::default()
        })
        .await;
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("ModelUnavailableError")
    );
    assert!(
        c.f.store
            .read_events("ses_unavailable", -1, 500)
            .unwrap()
            .events
            .is_empty()
    );
}

#[tokio::test]
async fn agent_switches_change_inherited_models_at_the_next_safe_boundary() {
    let c = Case::new(
        2,
        json!({"agents":{
            "build":{"model":"test/other","variant":"low"},
            "docs":{"model":"test/main","variant":"high"}
        }}),
    );
    let session = c.create("test/main", true).await;
    c.prompt(&session.id).await;
    c.runtime.switch_agent(&session.id, "docs").await.unwrap();
    c.prompt(&session.id).await;
    let requests = c.adapter.requests();
    assert_eq!(
        (requests[0].model.as_str(), &requests[0].body["variant"]),
        ("other", &json!("low"))
    );
    assert_eq!(
        (requests[1].model.as_str(), &requests[1].body["variant"]),
        ("main", &json!("high"))
    );
}

#[tokio::test]
async fn historical_creation_and_manual_switch_payloads_remain_explicit() {
    let c = Case::new(
        1,
        json!({"agents":{"build":{"model":"test/other","variant":"low"}}}),
    );
    let source = c.session("test/main#low").await;
    let mut created = c.f.store.read_events(&source, -1, 1).unwrap().events[0]
        .data
        .clone();
    created.as_object_mut().unwrap().remove("selection");
    created["info"]["id"] = json!("ses_legacy");
    c.f.store
        .append(
            "ses_legacy",
            cyber_store::Expected::Seq(-1),
            vec![
                cyber_store::NewEvent::new("session.created.1", created),
                cyber_store::NewEvent::new(
                    "session.model.switched.1",
                    json!({"from":"test/main#low","to":"test/main#high"}),
                ),
            ],
        )
        .unwrap();
    c.prompt("ses_legacy").await;
    let requests = c.adapter.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].model, "main");
    assert_eq!(requests[0].body["variant"], "high");
}

#[tokio::test]
async fn malformed_explicit_references_keep_the_model_error_class() {
    let c = Case::new(0, json!({}));
    let mut events = c.runtime.subscribe();
    let id = c.session("invalid-reference").await;
    c.prompt(&id).await;
    let mut kinds = Vec::new();
    while let Ok(event) = events.try_recv() {
        if let LiveEvent::Error { kind, .. } = event {
            kinds.push(kind);
        }
    }
    assert_eq!(kinds, vec!["model"]);
    assert!(c.adapter.requests().is_empty());
    assert_eq!(
        c.runtime.state(&id).await.unwrap().inbox[0].status,
        InputStatus::Pending
    );
}
