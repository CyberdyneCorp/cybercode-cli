//! Scripted runtime fixture shared by host and native permission-flow tests.
#![allow(dead_code)]

use super::Fixture;
use cyber_llm::adapters::{ScriptStep, ScriptedAdapter};
use cyber_llm::catalog::ModelRole;
use cyber_llm::{FinishReason, LlmEvent, LlmRequest, RetryPolicy, ToolCall, Usage};
use cyber_server::runtime::*;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

pub fn text(t: &str) -> Vec<ScriptStep> {
    vec![
        ScriptStep::Event(LlmEvent::TextDelta { text: t.into() }),
        ScriptStep::Event(LlmEvent::Usage(Usage {
            input: 100,
            output: 10,
            ..Usage::default()
        })),
        ScriptStep::Event(LlmEvent::Finish {
            reason: FinishReason::Stop,
        }),
    ]
}

pub fn call(id: &str, name: &str, input: serde_json::Value) -> Vec<ScriptStep> {
    vec![
        ScriptStep::Event(LlmEvent::ToolCallDone(ToolCall {
            id: id.into(),
            name: name.into(),
            arguments: input.to_string(),
            input: Some(input),
        })),
        ScriptStep::Event(LlmEvent::Usage(Usage {
            input: 100,
            output: 5,
            ..Usage::default()
        })),
        ScriptStep::Event(LlmEvent::Finish {
            reason: FinishReason::ToolCalls,
        }),
    ]
}

pub type Scripts = Vec<Vec<ScriptStep>>;

struct Models(HashMap<&'static str, Arc<ScriptedAdapter>>);

impl ModelResolver for Models {
    fn resolve(&self, model_ref: &str) -> Result<ResolvedModel, String> {
        let adapter = self
            .0
            .get(model_ref)
            .ok_or_else(|| format!("Model not found: {model_ref}"))?;
        let (provider, model) = model_ref.split_once('/').unwrap();
        Ok(ResolvedModel {
            adapter: Arc::clone(adapter) as Arc<dyn cyber_llm::Adapter>,
            template: LlmRequest {
                model: model.into(),
                max_output_tokens: Some(1000),
                ..LlmRequest::default()
            },
            provider: provider.into(),
            model: model.into(),
            context_limit: 200_000,
            cost: None,
            prefers_apply_patch: false,
        })
    }

    fn role(&self, role: ModelRole) -> Option<String> {
        Some(
            if role.key() == "title" {
                "test/title"
            } else {
                "test/summary"
            }
            .into(),
        )
    }
}

pub struct Flow {
    pub f: Fixture,
    pub main: Arc<ScriptedAdapter>,
    pub runtime: Runtime,
}

impl Flow {
    pub fn new(script: Vec<Vec<ScriptStep>>, interactive: bool) -> Self {
        Self::with(Fixture::new(), script, interactive, Arc::new(NoSnapshots))
    }

    pub fn with(
        f: Fixture,
        script: Vec<Vec<ScriptStep>>,
        interactive: bool,
        snapshots: Arc<dyn Snapshots>,
    ) -> Self {
        Self::with_models(f, script, interactive, snapshots, Vec::new())
    }

    pub fn with_models(
        f: Fixture,
        script: Vec<Vec<ScriptStep>>,
        interactive: bool,
        snapshots: Arc<dyn Snapshots>,
        extra: Vec<(&'static str, Scripts)>,
    ) -> Self {
        let main = Arc::new(ScriptedAdapter::new(script));
        let mut models = Models(HashMap::from([
            ("test/main", Arc::clone(&main)),
            ("test/title", Arc::new(ScriptedAdapter::new(Vec::new()))),
            ("test/summary", Arc::new(ScriptedAdapter::new(Vec::new()))),
        ]));
        for (reference, script) in extra {
            models
                .0
                .insert(reference, Arc::new(ScriptedAdapter::new(script)));
        }
        let runtime = Runtime::new(RuntimeOptions {
            store: Arc::clone(&f.store),
            resolver: Arc::new(models),
            tools: Arc::clone(&f.host) as Arc<dyn ToolHost>,
            global_config_dir: f.dir.path().join("global"),
            shell: "bash".into(),
            claude_compat: false,
            compaction: CompactionConfig::default(),
            retry: RetryPolicy {
                base_delay: Duration::from_millis(1),
                max_delay: Duration::from_millis(5),
                ..RetryPolicy::default()
            },
            max_steps: None,
            today: Some("2026-10-04".into()),
            interactive,
            snapshots,
        });
        f.host.attach(runtime.clone());
        Self { f, main, runtime }
    }

    pub async fn session(&self, mode: &str) -> String {
        let req = CreateSession {
            directory: self.f.repo.display().to_string(),
            model: "test/main".into(),
            mode: Some(mode.into()),
            ..Default::default()
        };
        self.runtime.create_session(req).await.unwrap().id
    }

    pub async fn prompt(&self, id: &str, text: &str) -> String {
        self.runtime
            .admit(id, Admission::text(text, Delivery::Queue))
            .await
            .unwrap()
            .message_id
    }

    pub async fn pending(&self, id: &str) -> PendingRequest {
        for _ in 0..500 {
            if let Some(r) = self.runtime.pending_requests(Some(id)).into_iter().next() {
                return r;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("no request was raised");
    }

    pub async fn settle(&self, id: &str) {
        tokio::time::timeout(Duration::from_secs(10), self.runtime.wait_idle(id))
            .await
            .expect("drain did not settle");
    }

    pub async fn output(&self, id: &str, call_id: &str) -> String {
        let state = self.runtime.state(id).await.unwrap();
        state.calls[call_id].output.clone().unwrap_or_default()
    }
}
