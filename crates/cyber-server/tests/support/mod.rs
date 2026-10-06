//! Test harness: a scripted model resolver, a fake tool host and a file-backed store.

#![allow(dead_code)]

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use cyber_core::paths::DatabaseLocation;
use cyber_llm::adapters::{ScriptStep, ScriptedAdapter};
use cyber_llm::catalog::{Cost, ModelRole};
use cyber_llm::{
    ErrorKind, FinishReason, LlmEvent, LlmRequest, RetryPolicy, ToolCall, ToolSpec, Usage,
};
use cyber_server::runtime::*;
use cyber_store::{Store, StoreOptions};
use futures::future::BoxFuture;
use serde_json::{Value, json};
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

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

pub fn tools(calls: &[(&str, &str, &str)]) -> Vec<ScriptStep> {
    let mut steps: Vec<ScriptStep> = calls
        .iter()
        .map(|(id, name, args)| {
            ScriptStep::Event(LlmEvent::ToolCallDone(ToolCall {
                id: (*id).into(),
                name: (*name).into(),
                arguments: (*args).into(),
                input: serde_json::from_str(args).ok(),
            }))
        })
        .collect();
    steps.push(ScriptStep::Event(LlmEvent::Usage(Usage {
        input: 100,
        output: 5,
        ..Usage::default()
    })));
    steps.push(ScriptStep::Event(LlmEvent::Finish {
        reason: FinishReason::ToolCalls,
    }));
    steps
}

pub fn reasoning_then_text(r: &str, t: &str) -> Vec<ScriptStep> {
    let mut steps = vec![
        ScriptStep::Event(LlmEvent::ReasoningDelta { text: r.into() }),
        ScriptStep::Event(LlmEvent::ReasoningSignature {
            signature: "sig".into(),
        }),
    ];
    steps.extend(text(t));
    steps
}

pub fn error(kind: ErrorKind, message: &str) -> Vec<ScriptStep> {
    vec![ScriptStep::Error {
        error: kind,
        message: message.into(),
    }]
}

/// Models by reference: `test/main`, `other/main`, `test/title`, `test/summary`.
pub struct Models {
    pub adapters: HashMap<String, Arc<ScriptedAdapter>>,
    pub context_limit: u64,
    pub roles: HashMap<&'static str, String>,
}

impl Models {
    pub fn adapter(&self, model_ref: &str) -> Arc<ScriptedAdapter> {
        Arc::clone(&self.adapters[model_ref])
    }

    pub fn requests(&self, model_ref: &str) -> Vec<LlmRequest> {
        self.adapters[model_ref].requests()
    }
}

impl ModelResolver for Models {
    fn resolve(&self, model_ref: &str) -> Result<ResolvedModel, String> {
        let adapter = self
            .adapters
            .get(model_ref)
            .ok_or_else(|| format!("Model not found: {model_ref}"))?;
        let (provider, model) = model_ref.split_once('/').unwrap();
        Ok(ResolvedModel {
            adapter: Arc::clone(adapter) as Arc<dyn cyber_llm::Adapter>,
            template: LlmRequest {
                model: model.into(),
                max_output_tokens: Some(1000),
                cache: true,
                ..LlmRequest::default()
            },
            provider: provider.into(),
            model: model.into(),
            context_limit: self.context_limit,
            cost: (provider == "test").then(|| Cost {
                input: 1.0,
                output: 2.0,
                ..Cost::default()
            }),
            prefers_apply_patch: false,
        })
    }

    fn role(&self, role: ModelRole) -> Option<String> {
        self.roles.get(role.key()).cloned()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Behavior {
    Return(String),
    /// Wait for the abort signal, then report abortion.
    UntilCancelled,
    /// Signal `started`, then wait for `release`.
    Gated,
    Crash,
    /// A defect in the host: the future panics.
    Panic,
    /// Ask for permission on `resource`, then report the reply.
    Ask(String),
    /// Review a request through the real auto-mode evaluator boundary.
    Auto(String),
    /// Ask one question, then report the answer.
    AskQuestion,
}

pub struct Tools {
    pub defs: Mutex<Vec<ToolDef>>,
    pub behavior: Mutex<HashMap<String, Behavior>>,
    pub executed: Mutex<Vec<Invocation>>,
    pub reconcile: Mutex<Reconciliation>,
    pub started: Notify,
    pub release: Notify,
}

pub fn def(name: &str, safety: RetrySafety, parallel: bool) -> ToolDef {
    ToolDef {
        spec: ToolSpec {
            name: name.into(),
            description: format!("{name} tool"),
            input_schema: json!({"type": "object"}),
        },
        retry_safety: safety,
        concurrency_safe: parallel,
    }
}

impl Tools {
    pub fn new() -> Arc<Self> {
        let defs = vec![
            def("clock", RetrySafety::ReadOnly, true),
            def("read", RetrySafety::ReadOnly, true),
            def("write", RetrySafety::Never, false),
            def("shell", RetrySafety::Never, false),
            def("ask", RetrySafety::ReadOnly, false),
        ];
        let behavior = HashMap::from([
            ("clock".to_string(), Behavior::Return("12:00".into())),
            ("read".to_string(), Behavior::Return("contents".into())),
            ("write".to_string(), Behavior::Return("written".into())),
        ]);
        Arc::new(Self {
            defs: Mutex::new(defs),
            behavior: Mutex::new(behavior),
            executed: Mutex::default(),
            reconcile: Mutex::new(Reconciliation::Unknown),
            started: Notify::new(),
            release: Notify::new(),
        })
    }

    pub fn set(&self, name: &str, behavior: Behavior) {
        self.behavior.lock().unwrap().insert(name.into(), behavior);
    }

    pub fn executed_names(&self) -> Vec<String> {
        self.executed
            .lock()
            .unwrap()
            .iter()
            .map(|i| i.name.clone())
            .collect()
    }
}

impl ToolHost for Tools {
    fn definitions(&self, _turn: &TurnContext) -> Vec<ToolDef> {
        self.defs.lock().unwrap().clone()
    }

    fn execute(&self, call: Invocation, cancel: CancellationToken) -> BoxFuture<'_, ToolOutcome> {
        let behavior = self.behavior.lock().unwrap().get(&call.name).cloned();
        let asker = call.asker.clone();
        let name = call.name.clone();
        let input = call.input.clone();
        self.executed.lock().unwrap().push(call);
        Box::pin(async move {
            match behavior {
                Some(Behavior::Return(out)) => ToolOutcome::Ok(out),
                Some(Behavior::UntilCancelled) => {
                    self.started.notify_one();
                    cancel.cancelled().await;
                    ToolOutcome::Aborted
                }
                Some(Behavior::Gated) => {
                    self.started.notify_one();
                    self.release.notified().await;
                    ToolOutcome::Ok("done".into())
                }
                Some(Behavior::Crash) => ToolOutcome::Crashed("panic: index out of bounds".into()),
                Some(Behavior::Panic) => panic!("tool host defect"),
                Some(Behavior::Auto(resource)) => {
                    let review = AutoReview {
                        action: "bash".into(),
                        resources: vec![resource],
                        tool: name,
                        input,
                        policy: "Only modify this repository".into(),
                    };
                    match asker.review_auto(review, cancel).await {
                        Ok(decision) => ToolOutcome::Ok(serde_json::to_string(&decision).unwrap()),
                        Err(_) => ToolOutcome::Failed("Auto decision could not be recorded".into()),
                    }
                }
                Some(Behavior::Ask(resource)) => {
                    let ask = PermissionAsk {
                        action: "bash".into(),
                        resources: vec![resource.clone()],
                        always_patterns: vec![format!(
                            "{} *",
                            resource.split(' ').next().unwrap_or_default()
                        )],
                        metadata: json!({}),
                    };
                    match asker.permission(ask).await {
                        PermissionReply::Once | PermissionReply::Always => {
                            ToolOutcome::Ok(format!("ran {resource}"))
                        }
                        PermissionReply::Reject { message: Some(m) } => {
                            ToolOutcome::Failed(format!("Rejected by user: {m}"))
                        }
                        PermissionReply::Reject { message: None } => {
                            ToolOutcome::Failed("Rejected by user".into())
                        }
                        PermissionReply::Unattended => {
                            ToolOutcome::Failed("Denied: no approver".into())
                        }
                    }
                }
                Some(Behavior::AskQuestion) => {
                    let q = Question {
                        question: "Database?".into(),
                        header: "DB".into(),
                        options: vec![
                            QuestionOption {
                                label: "postgres".into(),
                                description: String::new(),
                            },
                            QuestionOption {
                                label: "sqlite".into(),
                                description: String::new(),
                            },
                        ],
                        multi_select: false,
                        allow_custom: true,
                    };
                    match asker.question(vec![q]).await {
                        QuestionReply::Answers { answers } => ToolOutcome::Ok(answers[0].join(",")),
                        QuestionReply::Dismissed => ToolOutcome::Failed("dismissed".into()),
                        QuestionReply::Unattended => ToolOutcome::Failed("no user".into()),
                    }
                }
                None => ToolOutcome::Failed("no behavior".into()),
            }
        })
    }

    fn reconcile(&self, _directory: &str, _call: &CallState) -> BoxFuture<'_, Reconciliation> {
        let r = self.reconcile.lock().unwrap().clone();
        Box::pin(async move { r })
    }
}

pub struct Harness {
    pub dir: tempfile::TempDir,
    pub store: Arc<Store>,
    pub models: Arc<Models>,
    pub tools: Arc<Tools>,
    pub runtime: Runtime,
    pub repo: PathBuf,
}

pub struct Setup {
    pub scripts: Vec<(&'static str, Vec<Vec<ScriptStep>>)>,
    pub context_limit: u64,
    pub compaction: CompactionConfig,
    pub max_steps: Option<u32>,
    pub roles: Vec<(&'static str, &'static str)>,
    pub interactive: bool,
}

impl Default for Setup {
    fn default() -> Self {
        Self {
            scripts: Vec::new(),
            context_limit: 200_000,
            compaction: CompactionConfig {
                auto: true,
                buffer: 1_000,
                keep_tokens: 8_000,
                keep_turns: None,
                model: None,
            },
            max_steps: None,
            roles: Vec::new(),
            interactive: true,
        }
    }
}

impl Harness {
    pub fn new(setup: Setup) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        let store = Arc::new(open_store(&dir.path().join("cyber.db")));
        let models = models(setup.scripts, setup.context_limit, &setup.roles);
        let tools = Tools::new();
        let runtime = runtime_with(
            &store,
            &models,
            &tools,
            dir.path(),
            setup.compaction,
            setup.max_steps,
            setup.interactive,
        );
        Self {
            dir,
            store,
            models,
            tools,
            runtime,
            repo,
        }
    }

    /// A second runtime over the same store, as after a process restart.
    pub fn restart(&self) -> Runtime {
        runtime(
            &self.store,
            &self.models,
            &self.tools,
            self.dir.path(),
            CompactionConfig::default(),
            None,
        )
    }

    pub async fn session(&self) -> String {
        let req = cyber_server::runtime::CreateSession {
            directory: self.repo.display().to_string(),
            model: "test/main".into(),
            ..Default::default()
        };
        self.runtime.create_session(req).await.unwrap().id
    }

    pub async fn state(&self, id: &str) -> SessionState {
        self.runtime.state(id).await.unwrap()
    }

    pub async fn settle(&self, id: &str) {
        tokio::time::timeout(Duration::from_secs(10), self.runtime.wait_idle(id))
            .await
            .expect("drain did not settle");
    }
}

pub fn open_store(path: &std::path::Path) -> Store {
    Store::open(StoreOptions::new(
        DatabaseLocation::File(path.to_path_buf()),
        Runtime::registry(),
    ))
    .unwrap()
}

pub fn models(
    scripts: Vec<(&'static str, Vec<Vec<ScriptStep>>)>,
    context_limit: u64,
    roles: &[(&'static str, &'static str)],
) -> Arc<Models> {
    let mut adapters: HashMap<String, Arc<ScriptedAdapter>> = HashMap::new();
    for name in ["test/main", "other/main", "test/title", "test/summary"] {
        adapters.insert(name.into(), Arc::new(ScriptedAdapter::new(Vec::new())));
    }
    for (name, turns) in scripts {
        adapters.insert(name.into(), Arc::new(ScriptedAdapter::new(turns)));
    }
    // Hidden calls use their own scripted models so they never consume the main script.
    let mut defaults: HashMap<&'static str, String> = HashMap::from([
        ("title", "test/title".to_string()),
        ("compaction", "test/summary".to_string()),
    ]);
    defaults.extend(roles.iter().map(|(k, v)| (*k, v.to_string())));
    Arc::new(Models {
        adapters,
        context_limit,
        roles: defaults,
    })
}

pub fn runtime(
    store: &Arc<Store>,
    models: &Arc<Models>,
    tools: &Arc<Tools>,
    dir: &std::path::Path,
    compaction: CompactionConfig,
    max_steps: Option<u32>,
) -> Runtime {
    runtime_with(store, models, tools, dir, compaction, max_steps, true)
}

pub fn runtime_with(
    store: &Arc<Store>,
    models: &Arc<Models>,
    tools: &Arc<Tools>,
    dir: &std::path::Path,
    compaction: CompactionConfig,
    max_steps: Option<u32>,
    interactive: bool,
) -> Runtime {
    Runtime::new(RuntimeOptions {
        store: Arc::clone(store),
        resolver: Arc::clone(models) as Arc<dyn ModelResolver>,
        tools: Arc::clone(tools) as Arc<dyn ToolHost>,
        global_config_dir: dir.join("global"),
        shell: "zsh".into(),
        claude_compat: true,
        compaction,
        retry: RetryPolicy {
            base_delay: Duration::from_millis(1),
            max_delay: Duration::from_millis(5),
            ..RetryPolicy::default()
        },
        max_steps,
        today: Some("2026-10-03".into()),
        interactive,
        snapshots: Arc::new(NoSnapshots),
    })
}

/// Text of every message in a request, for assertions.
pub fn transcript(request: &LlmRequest) -> String {
    serde_json::to_string(&request.messages).unwrap()
}

pub fn last_user_text(request: &LlmRequest) -> Value {
    serde_json::to_value(request.messages.last().unwrap()).unwrap()
}

pub async fn next_error(rx: &mut tokio::sync::broadcast::Receiver<LiveEvent>) -> (String, String) {
    loop {
        if let LiveEvent::Error { kind, message, .. } = rx.recv().await.unwrap() {
            return (kind, message);
        }
    }
}
