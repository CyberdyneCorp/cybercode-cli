//! End-to-end flows: the built-in host behind a real runtime with a scripted model.

mod support;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use cyber_llm::adapters::{ScriptStep, ScriptedAdapter};
use cyber_llm::catalog::ModelRole;
use cyber_llm::{FinishReason, LlmEvent, LlmRequest, RetryPolicy, ToolCall, Usage};
use cyber_server::runtime::*;
use cyber_tools::permissions::saved;
use serde_json::json;
use support::Fixture;

fn text(t: &str) -> Vec<ScriptStep> {
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

fn call(id: &str, name: &str, input: serde_json::Value) -> Vec<ScriptStep> {
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

struct Flow {
    f: Fixture,
    main: Arc<ScriptedAdapter>,
    runtime: Runtime,
}

impl Flow {
    fn new(script: Vec<Vec<ScriptStep>>, interactive: bool) -> Self {
        Self::with(Fixture::new(), script, interactive, Arc::new(NoSnapshots))
    }

    fn with(
        f: Fixture,
        script: Vec<Vec<ScriptStep>>,
        interactive: bool,
        snapshots: Arc<dyn Snapshots>,
    ) -> Self {
        let main = Arc::new(ScriptedAdapter::new(script));
        let models = Models(HashMap::from([
            ("test/main", Arc::clone(&main)),
            ("test/title", Arc::new(ScriptedAdapter::new(Vec::new()))),
            ("test/summary", Arc::new(ScriptedAdapter::new(Vec::new()))),
        ]));
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

    async fn session(&self, mode: &str) -> String {
        let req = CreateSession {
            directory: self.f.repo.display().to_string(),
            model: "test/main".into(),
            mode: Some(mode.into()),
            ..Default::default()
        };
        self.runtime.create_session(req).await.unwrap().id
    }

    async fn prompt(&self, id: &str, text: &str) -> String {
        self.runtime
            .admit(id, Admission::text(text, Delivery::Queue))
            .await
            .unwrap()
            .message_id
    }

    async fn pending(&self, id: &str) -> PendingRequest {
        for _ in 0..500 {
            if let Some(r) = self.runtime.pending_requests(Some(id)).into_iter().next() {
                return r;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("no request was raised");
    }

    async fn settle(&self, id: &str) {
        tokio::time::timeout(Duration::from_secs(10), self.runtime.wait_idle(id))
            .await
            .expect("drain did not settle");
    }

    async fn output(&self, id: &str, call_id: &str) -> String {
        let state = self.runtime.state(id).await.unwrap();
        state.calls[call_id].output.clone().unwrap_or_default()
    }
}

#[tokio::test]
async fn always_saves_the_approval_and_later_commands_skip_the_prompt() {
    let flow = Flow::new(
        vec![
            call("c1", "bash", json!({"command": "echo first"})),
            text("done"),
            call("c2", "bash", json!({"command": "echo second"})),
            text("done again"),
        ],
        true,
    );
    let id = flow.session("default").await;
    flow.prompt(&id, "run it").await;
    let request = flow.pending(&id).await;
    let PendingKind::Permission(ask) = &request.kind else {
        panic!("expected a permission request")
    };
    assert_eq!(ask.action, "bash");
    assert_eq!(ask.always_patterns, vec!["echo *".to_string()]);
    flow.runtime
        .reply_permission(&request.id, PermissionReply::Always)
        .await
        .unwrap();
    flow.settle(&id).await;
    assert!(flow.output(&id, "c1").await.contains("first"));
    let root = std::fs::canonicalize(&flow.f.repo).unwrap();
    assert_eq!(saved::list(&flow.f.store, Some(&root)).unwrap().len(), 1);

    flow.prompt(&id, "again").await;
    flow.settle(&id).await;
    assert!(flow.output(&id, "c2").await.contains("second"));
    assert!(flow.runtime.pending_requests(Some(&id)).is_empty());
}

#[tokio::test]
async fn reject_with_feedback_reaches_the_model() {
    let flow = Flow::new(
        vec![
            call("c1", "bash", json!({"command": "rm -rf build"})),
            text("ok"),
        ],
        true,
    );
    let id = flow.session("default").await;
    flow.prompt(&id, "clean").await;
    let request = flow.pending(&id).await;
    let reply = PermissionReply::Reject {
        message: Some("use make clean".into()),
    };
    flow.runtime
        .reply_permission(&request.id, reply)
        .await
        .unwrap();
    flow.settle(&id).await;
    assert_eq!(
        flow.output(&id, "c1").await,
        "Rejected by user: use make clean"
    );
    let sent = serde_json::to_string(&flow.main.requests().last().unwrap().messages).unwrap();
    assert!(sent.contains("use make clean"));
}

#[tokio::test]
async fn plan_exit_approval_switches_the_mode() {
    let flow = Flow::new(
        vec![
            call("c1", "plan_exit", json!({"summary": "two steps"})),
            text("building"),
        ],
        true,
    );
    let id = flow.session("plan").await;
    flow.f.write(
        &format!(".cyber/plans/{id}.md"),
        "1. migrate\n2. backfill\n",
    );
    flow.prompt(&id, "plan it").await;
    let request = flow.pending(&id).await;
    let PendingKind::Question { questions } = &request.kind else {
        panic!("expected a question")
    };
    assert!(questions[0].question.contains("2. backfill"));
    let answers = vec![vec!["Approve with accept-edits".to_string()]];
    flow.runtime
        .answer_question(&request.id, QuestionReply::Answers { answers })
        .await
        .unwrap();
    flow.settle(&id).await;
    assert_eq!(
        flow.runtime.state(&id).await.unwrap().info.mode,
        "accept-edits"
    );
    support::golden(&flow.f, "plan_exit", &flow.output(&id, "c1").await);
}

#[tokio::test]
async fn plan_exit_request_changes_returns_feedback_and_stays_in_plan() {
    let flow = Flow::new(
        vec![
            call("c1", "plan_exit", json!({"summary": "one step"})),
            text("revising"),
        ],
        true,
    );
    let id = flow.session("plan").await;
    flow.prompt(&id, "plan it").await;
    let request = flow.pending(&id).await;
    let answers = vec![vec!["split the migration into two steps".to_string()]];
    flow.runtime
        .answer_question(&request.id, QuestionReply::Answers { answers })
        .await
        .unwrap();
    flow.settle(&id).await;
    assert_eq!(flow.runtime.state(&id).await.unwrap().info.mode, "plan");
    assert!(
        flow.output(&id, "c1")
            .await
            .contains("split the migration into two steps")
    );
}

#[tokio::test]
async fn plan_tools_are_denied_without_an_interactive_user() {
    let flow = Flow::new(vec![call("c1", "plan_enter", json!({})), text("ok")], false);
    let id = flow.session("default").await;
    flow.prompt(&id, "plan").await;
    flow.settle(&id).await;
    assert!(flow.output(&id, "c1").await.contains("non-interactive"));
    assert_eq!(flow.runtime.state(&id).await.unwrap().info.mode, "default");
}

#[tokio::test]
async fn plan_enter_switches_to_plan_mode() {
    let flow = Flow::new(
        vec![call("c1", "plan_enter", json!({})), text("exploring")],
        true,
    );
    let id = flow.session("default").await;
    flow.prompt(&id, "plan first").await;
    flow.settle(&id).await;
    assert_eq!(flow.runtime.state(&id).await.unwrap().info.mode, "plan");
    support::golden(
        &flow.f,
        "plan_enter",
        &flow.output(&id, "c1").await.replace(&id, "<session>"),
    );
}

#[tokio::test]
async fn history_search_finds_earlier_messages_of_this_session_only() {
    let flow = Flow::new(
        vec![
            text("noted"),
            call("c1", "history_search", json!({"query": "purple elephant"})),
            text("found"),
        ],
        true,
    );
    let other = flow.session("default").await;
    let id = flow.session("default").await;
    flow.prompt(&id, "remember the purple elephant").await;
    flow.settle(&id).await;
    flow.prompt(&id, "what did I say?").await;
    flow.settle(&id).await;
    let out = flow.output(&id, "c1").await;
    assert!(
        out.contains("[user]") && out.contains("purple elephant"),
        "{out}"
    );
    assert_ne!(other, id);
}

fn git(dir: &std::path::Path, args: &[&str]) {
    let ok = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .unwrap()
        .status
        .success();
    assert!(ok, "git {args:?}");
}

/// A real git repository with `a.txt` committed, snapshots on, edits and bash allowed.
fn rewind_flow(script: Vec<Vec<ScriptStep>>) -> Flow {
    let f = Fixture::new();
    git(&f.repo, &["init", "-q"]);
    f.write("a.txt", "one\ntwo\n");
    git(&f.repo, &["add", "."]);
    git(
        &f.repo,
        &[
            "-c",
            "user.email=t@t",
            "-c",
            "user.name=t",
            "commit",
            "-q",
            "-m",
            "init",
        ],
    );
    f.set_config(json!({"permissions": {"bash": "allow"}}));
    let config = Arc::clone(&f.config);
    let data = f.dir.path().join("data");
    let snapshots =
        cyber_snapshot::GitSnapshots::new(data, Arc::new(move |_| config.lock().unwrap().clone()));
    Flow::with(f, script, true, snapshots)
}

fn edit_a() -> Vec<ScriptStep> {
    call(
        "c1",
        "edit",
        json!({"path": "a.txt", "old_string": "one", "new_string": "ONE"}),
    )
}

fn create_b() -> Vec<ScriptStep> {
    call("c2", "bash", json!({"command": "echo new > b.txt"}))
}

#[tokio::test]
async fn each_message_records_a_diff_of_its_turns() {
    let flow = rewind_flow(vec![edit_a(), text("edited")]);
    let id = flow.session("accept-edits").await;
    let message = flow.prompt(&id, "uppercase it").await;
    flow.settle(&id).await;
    let diffs = flow.runtime.diff(&id, &message).await.unwrap();
    assert_eq!(diffs.len(), 1);
    let d = &diffs[0];
    assert_eq!(
        (d.file.as_str(), d.status.as_str(), d.additions, d.deletions),
        ("a.txt", "modified", 1, 1)
    );
    let state = flow.runtime.state(&id).await.unwrap();
    assert!(
        state
            .steps
            .iter()
            .all(|s| s.pre.is_some() && s.post.is_some()),
        "{:?}",
        state.steps
    );
}

#[tokio::test]
async fn stage_clear_and_commit_follow_the_three_phases() {
    let flow = rewind_flow(vec![edit_a(), text("edited"), create_b(), text("created")]);
    let id = flow.session("accept-edits").await;
    let first = flow.prompt(&id, "uppercase it").await;
    flow.settle(&id).await;
    let second = flow.prompt(&id, "add b").await;
    flow.settle(&id).await;
    assert_eq!(flow.f.read("b.txt"), "new\n");

    let staged = flow
        .runtime
        .revert_stage(&id, &second, RevertTarget::Both)
        .await
        .unwrap();
    assert!(!flow.f.repo.join("b.txt").exists());
    assert_eq!(flow.f.read("a.txt"), "ONE\ntwo\n");
    assert!(staged.diff.contains("-new"), "{}", staged.diff);

    flow.runtime.revert_clear(&id).await.unwrap();
    assert_eq!(flow.f.read("b.txt"), "new\n");
    assert!(flow.runtime.state(&id).await.unwrap().revert.is_none());

    flow.runtime
        .revert_stage(&id, &first, RevertTarget::Both)
        .await
        .unwrap();
    assert_eq!(flow.f.read("a.txt"), "one\ntwo\n");
    assert!(!flow.f.repo.join("b.txt").exists());
    flow.runtime.revert_commit(&id).await.unwrap();
    let state = flow.runtime.state(&id).await.unwrap();
    assert!(state.entries.is_empty(), "{:?}", state.entries);
    assert!(state.inbox.is_empty());
}

#[tokio::test]
async fn code_only_rewind_keeps_the_conversation() {
    let flow = rewind_flow(vec![edit_a(), text("edited")]);
    let id = flow.session("accept-edits").await;
    let message = flow.prompt(&id, "uppercase it").await;
    flow.settle(&id).await;
    let entries = flow.runtime.state(&id).await.unwrap().entries.len();
    flow.runtime
        .revert_stage(&id, &message, RevertTarget::Code)
        .await
        .unwrap();
    flow.runtime.revert_commit(&id).await.unwrap();
    assert_eq!(flow.f.read("a.txt"), "one\ntwo\n");
    assert_eq!(
        flow.runtime.state(&id).await.unwrap().entries.len(),
        entries
    );
}

#[tokio::test]
async fn the_next_prompt_commits_a_staged_conversation_revert() {
    let flow = rewind_flow(vec![text("first answer"), text("second answer")]);
    let id = flow.session("default").await;
    let first = flow.prompt(&id, "hello").await;
    flow.settle(&id).await;
    flow.runtime
        .revert_stage(&id, &first, RevertTarget::Conversation)
        .await
        .unwrap();
    flow.prompt(&id, "hello again").await;
    flow.settle(&id).await;
    let sent = serde_json::to_string(&flow.main.requests().last().unwrap().messages).unwrap();
    assert!(
        !sent.contains("first answer") && sent.contains("hello again"),
        "{sent}"
    );
}

#[tokio::test]
async fn conflicting_user_edits_block_the_rewind() {
    let flow = rewind_flow(vec![edit_a(), text("edited")]);
    let id = flow.session("accept-edits").await;
    let message = flow.prompt(&id, "uppercase it").await;
    flow.settle(&id).await;
    flow.f.write("a.txt", "ONE!\ntwo\n");
    let err = flow
        .runtime
        .revert_stage(&id, &message, RevertTarget::Both)
        .await
        .unwrap_err();
    assert!(
        matches!(&err, RuntimeError::RewindConflict(paths) if paths == &vec!["a.txt".to_string()]),
        "{err}"
    );
    assert_eq!(flow.f.read("a.txt"), "ONE!\ntwo\n");
    assert!(flow.runtime.state(&id).await.unwrap().revert.is_none());
}

#[cfg(target_os = "macos")]
#[tokio::test]
async fn approving_a_domain_lets_sandboxed_commands_reach_it() {
    let base = support::serve(|_, _| (200, vec![], b"hello".to_vec())).await;
    let url = base.replace("127.0.0.1", "localhost");
    let flow = Flow::new(
        vec![
            call(
                "c1",
                "bash",
                json!({"command": format!("curl -s --max-time 10 {url}/")}),
            ),
            text("done"),
        ],
        true,
    );
    flow.f.set_config(json!({"permissions": {"bash": "allow"}}));
    let id = flow.session("default").await;
    flow.prompt(&id, "fetch").await;
    let request = flow.pending(&id).await;
    let PendingKind::Permission(ask) = &request.kind else {
        panic!("expected a permission request")
    };
    assert_eq!(
        (ask.action.as_str(), ask.resources.clone()),
        ("network", vec!["localhost".to_string()])
    );
    flow.runtime
        .reply_permission(&request.id, PermissionReply::Once)
        .await
        .unwrap();
    flow.settle(&id).await;
    assert_eq!(flow.output(&id, "c1").await, "hello");
}

#[tokio::test]
async fn session_rules_deny_tools_and_hide_fully_denied_ones() {
    let q = json!({"questions": [{"question": "Which?", "header": "Pick", "options": [{"label": "a"}, {"label": "b"}]}]});
    let flow = Flow::new(vec![call("c1", "question", q), text("ok")], true);
    let req = CreateSession {
        directory: flow.f.repo.display().to_string(),
        model: "test/main".into(),
        rules: Some(json!({"question": "deny", "plan_enter": "deny"})),
        ..Default::default()
    };
    let id = flow.runtime.create_session(req).await.unwrap().id;
    flow.prompt(&id, "ask me").await;
    flow.settle(&id).await;
    // The model called a tool it was not offered; the call is refused without asking.
    assert!(flow.runtime.pending_requests(Some(&id)).is_empty());
    let offered: Vec<String> = flow.main.requests()[0]
        .tools
        .iter()
        .map(|t| t.name.clone())
        .collect();
    assert!(
        !offered.contains(&"question".to_string()) && !offered.contains(&"plan_enter".to_string()),
        "{offered:?}"
    );
    assert!(offered.contains(&"read".to_string()));
}

#[tokio::test]
async fn shell_commands_are_recorded_for_the_next_turn_without_a_turn() {
    let flow = Flow::new(vec![text("I see the output")], true);
    let id = flow.session("default").await;
    let output = flow
        .runtime
        .shell(&id, "echo from-the-shell")
        .await
        .unwrap();
    assert_eq!(output, "from-the-shell");
    assert!(
        flow.main.requests().is_empty(),
        "no Turn runs for a shell command"
    );
    flow.prompt(&id, "what did it print?").await;
    flow.settle(&id).await;
    let sent = serde_json::to_string(&flow.main.requests()[0].messages).unwrap();
    assert!(
        sent.contains("!echo from-the-shell") && sent.contains("from-the-shell"),
        "{sent}"
    );
}

#[tokio::test]
async fn question_golden() {
    let flow = Flow::new(
        vec![
            call(
                "c1",
                "question",
                json!({"questions":[{"question":"Database?","header":"DB","options":[{"label":"sqlite"},{"label":"postgres"}]}]}),
            ),
            text("done"),
        ],
        true,
    );
    let id = flow.session("default").await;
    flow.prompt(&id, "ask").await;
    let request = flow.pending(&id).await;
    flow.runtime
        .answer_question(
            &request.id,
            QuestionReply::Answers {
                answers: vec![vec!["sqlite".into()]],
            },
        )
        .await
        .unwrap();
    flow.settle(&id).await;
    support::golden(&flow.f, "question", &flow.output(&id, "c1").await);
}

#[tokio::test]
async fn history_search_golden() {
    let flow = Flow::new(
        vec![
            text("noted"),
            call("c1", "history_search", json!({"query":"purple"})),
            text("found"),
        ],
        true,
    );
    let id = flow.session("default").await;
    let message = flow.prompt(&id, "remember purple elephant").await;
    flow.settle(&id).await;
    flow.prompt(&id, "recall it").await;
    flow.settle(&id).await;
    support::golden(
        &flow.f,
        "history_search",
        &flow.output(&id, "c1").await.replace(&message, "msg_fixed"),
    );
}

#[tokio::test]
async fn critical_removal_asks_with_warning_despite_a_saved_approval() {
    let f = Fixture::new();
    f.write("keep.txt", "preserve me");
    saved::save(&f.store, &f.repo, "bash", &["rm *".into()], "previous").unwrap();
    let command = format!("rm -rf '{}'", f.repo.display());
    let flow = Flow::with(
        f,
        vec![
            call("c1", "bash", json!({"command": command})),
            text("done"),
        ],
        true,
        Arc::new(NoSnapshots),
    );
    let id = flow.session("accept-edits").await;
    flow.prompt(&id, "run command").await;
    let request = flow.pending(&id).await;
    let PendingKind::Permission(ask) = request.kind else {
        panic!("missing permission")
    };
    assert_eq!(ask.action, "bash");
    assert_eq!(ask.metadata["severity"], "danger");
    assert!(
        ask.metadata["warning"]
            .as_str()
            .unwrap()
            .starts_with("Danger:")
    );
    flow.runtime
        .reply_permission(
            &request.id,
            PermissionReply::Reject {
                message: Some("Keep the repository".into()),
            },
        )
        .await
        .unwrap();
    flow.settle(&id).await;
    assert_eq!(flow.f.read("keep.txt"), "preserve me");
}

#[cfg(target_os = "macos")]
#[tokio::test]
async fn the_os_sandbox_blocks_inline_writes_after_individual_approval() {
    let f = Fixture::new();
    f.set_config(json!({"permissions": {"bash": "allow"}}));
    let target = f.repo.join(".git/hooks-pre-commit");
    let script = format!(
        "python3 -c \"open('{}', 'w').write('x')\"",
        target.display()
    );
    let flow = Flow::with(
        f,
        vec![call("c1", "bash", json!({"command": script})), text("done")],
        true,
        Arc::new(NoSnapshots),
    );
    let id = flow.session("default").await;
    flow.prompt(&id, "run the probe").await;
    let pending = flow.pending(&id).await;
    let PendingKind::Permission(ask) = &pending.kind else {
        panic!("expected permission request")
    };
    assert_eq!(ask.action, "bash");
    assert_eq!(ask.metadata["requires_confirmation"], true);
    flow.runtime
        .reply_permission(&pending.id, PermissionReply::Once)
        .await
        .unwrap();
    flow.settle(&id).await;
    let out = flow.output(&id, "c1").await;
    assert!(
        out.contains("Operation not permitted") || out.contains("PermissionError"),
        "{out}"
    );
    assert!(!target.exists());
}
