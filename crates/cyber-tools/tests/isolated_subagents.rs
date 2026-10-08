//! Actual child inference against native managed checkouts. Full-access selects process
//! availability explicitly; these cases do not claim Windows sandbox confinement.
mod support;
use cyber_server::runtime::{NoSnapshots, ToolHost};
use serde_json::{Value, json};
use std::process::Command;
use std::sync::Arc;
use support::flow::{Flow, call, text};
use tokio_util::sync::CancellationToken;

fn flow(script: Vec<Vec<cyber_llm::adapters::ScriptStep>>, interactive: bool) -> Flow {
    let f = support::Fixture::with_policy(
        "bash",
        cyber_sandbox::find_helper(),
        Some("full-access".into()),
    );
    for args in [
        vec!["init", "--quiet"],
        vec!["config", "user.name", "Isolated test"],
        vec!["config", "user.email", "test@example.invalid"],
    ] {
        git(&f.repo, &args);
    }
    f.write("tracked.txt", "original\n");
    git(&f.repo, &["add", "tracked.txt"]);
    git(&f.repo, &["commit", "--quiet", "-m", "initial"]);
    let flow = Flow::with(f, script, interactive, Arc::new(NoSnapshots));
    flow.f
        .set_config(json!({"permissions":{"agent":"allow","worktree":"allow","edit":"allow"}}));
    flow
}
fn git(directory: &std::path::Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .current_dir(directory)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().into()
}
async fn invoke(flow: &Flow, parent: &str, input: Value) -> Result<Value, String> {
    let mut inv = flow.f.invocation("bypass", "agent", input);
    inv.session_id = parent.into();
    let info = flow.runtime.state(parent).await.unwrap().info;
    inv.agent = info.agent;
    inv.mode = info.mode;
    inv.rules = info.rules;
    match flow.f.host.execute(inv, CancellationToken::new()).await {
        cyber_server::runtime::ToolOutcome::Ok(output)
        | cyber_server::runtime::ToolOutcome::Structured { output, .. } => {
            Ok(serde_json::from_str(&output).unwrap())
        }
        cyber_server::runtime::ToolOutcome::Failed(error) => Err(error),
        other => panic!("unexpected {other:?}"),
    }
}
async fn done(flow: &Flow, job: &str) -> cyber_server::runtime::Job {
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        while !flow.runtime.job(job).unwrap().notified {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    flow.runtime.job(job).unwrap()
}

#[tokio::test]
async fn isolated_model_edits_stay_in_the_child_and_report_diff_stats() {
    let flow = flow(
        vec![
            call(
                "write",
                "write",
                json!({"path":"generated.txt","content":"child change\n"}),
            ),
            text("child findings"),
        ],
        false,
    );
    let parent = flow.session("bypass").await;
    let result=invoke(&flow,&parent,json!({"prompt":"implement change","name":"patch","isolation":"worktree","model":"test/main"})).await.unwrap();
    let worktree = &result["worktree"];
    let path = std::path::Path::new(worktree["path"].as_str().unwrap());
    assert_eq!(result["text"], "child findings");
    assert_eq!(worktree["kept"], true);
    assert!(worktree["branch"].as_str().unwrap().ends_with("/patch"));
    assert_eq!(
        std::fs::read_to_string(path.join("generated.txt")).unwrap(),
        "child change\n"
    );
    assert!(!flow.f.repo.join("generated.txt").exists());
    assert_eq!(worktree["changes"]["additions"], 1);
    assert_eq!(worktree["changes"]["files"][0]["file"], "generated.txt");
    let child = flow
        .runtime
        .state(result["id"].as_str().unwrap())
        .await
        .unwrap();
    assert_eq!(child.info.worktree_id.as_deref(), worktree["id"].as_str());
    assert_eq!(
        child.child_worktree().unwrap().branch,
        worktree["branch"].as_str().unwrap()
    );
}

#[tokio::test]
async fn a_clean_isolated_child_removes_its_checkout_and_branch() {
    let flow = flow(vec![text("no changes")], false);
    let parent = flow.session("bypass").await;
    let result = invoke(
        &flow,
        &parent,
        json!({"prompt":"inspect only","isolation":"worktree","model":"test/main"}),
    )
    .await
    .unwrap();
    let worktree = &result["worktree"];
    assert_eq!(worktree["kept"], false, "{result}");
    assert!(!std::path::Path::new(worktree["path"].as_str().unwrap()).exists());
    assert_eq!(
        git(
            &flow.f.repo,
            &["branch", "--list", worktree["branch"].as_str().unwrap()]
        ),
        ""
    );
    assert_eq!(worktree["changes"]["files"], json!([]));
}

#[tokio::test]
async fn keep_policy_preserves_a_clean_child_and_resume_reuses_its_identity() {
    let flow = flow(vec![text("first"), text("follow up")], false);
    flow.f.set_config(
        json!({"permissions":{"agent":"allow","worktree":"allow"},"worktrees":{"keep":"always"}}),
    );
    let parent = flow.session("bypass").await;
    let first = invoke(
        &flow,
        &parent,
        json!({"prompt":"inspect only","name":"review","isolation":"worktree","model":"test/main"}),
    )
    .await
    .unwrap();
    assert_eq!(first["worktree"]["kept"], true);
    let resumed = invoke(
        &flow,
        &parent,
        json!({"prompt":"follow up","resume":"review"}),
    )
    .await
    .unwrap();
    assert_eq!(resumed["id"], first["id"]);
    assert_eq!(resumed["worktree"]["id"], first["worktree"]["id"]);
    assert_eq!(resumed["text"], "follow up");
    assert!(
        invoke(
            &flow,
            &parent,
            json!({"prompt":"follow up","resume":"review","isolation":"none"})
        )
        .await
        .unwrap_err()
        .contains("cannot change child isolation")
    );
}

#[tokio::test]
async fn a_forked_structured_background_child_has_worktree_metadata_in_its_handback() {
    let flow = flow(
        vec![
            text("parent context"),
            call("result", "return_result", json!({"answer":1})),
            text("notice handled"),
        ],
        false,
    );
    let parent = flow.session("bypass").await;
    flow.prompt(&parent, "original objective").await;
    flow.settle(&parent).await;
    let started=invoke(&flow,&parent,json!({"prompt":"inspect independently","fork":true,"background":true,"isolation":"worktree","output_schema":{"type":"object","properties":{"answer":{"type":"integer"}},"required":["answer"]}})).await.unwrap();
    let job = done(&flow, started["job_id"].as_str().unwrap()).await;
    let result = job.result.unwrap();
    assert_eq!(result["result"], json!({"answer":1}));
    assert_eq!(result["worktree"]["kept"], false);
    let child = flow.runtime.state(&job.child_id).await.unwrap();
    assert_eq!(
        child.task.objective.as_ref().unwrap().text,
        "original objective"
    );
    assert!(format!("{:?}", flow.main.requests()[1].messages).contains("parent context"));
    assert_eq!(job.tokens, 105);
}

#[tokio::test]
async fn worktree_permission_denial_creates_no_child_or_native_ownership() {
    let flow = flow(vec![], false);
    flow.f
        .set_config(json!({"permissions":{"agent":"allow","worktree":"deny"}}));
    let parent = flow.session("bypass").await;
    assert!(
        invoke(
            &flow,
            &parent,
            json!({"prompt":"inspect only","isolation":"worktree"})
        )
        .await
        .unwrap_err()
        .contains("Permission denied")
    );
    assert_eq!(
        flow.runtime
            .list(&Default::default())
            .unwrap()
            .sessions
            .len(),
        1
    );
    assert!(!flow.f.repo.join(".git/cyber-worktrees").exists());
    assert!(flow.main.requests().is_empty());
}

#[tokio::test]
async fn retained_checkout_binding_survives_restart_and_resume() {
    let flow = flow(vec![text("first")], false);
    flow.f.set_config(
        json!({"permissions":{"agent":"allow","worktree":"allow"},"worktrees":{"keep":"always"}}),
    );
    let parent = flow.session("bypass").await;
    let first = invoke(
        &flow,
        &parent,
        json!({"prompt":"inspect","name":"durable","isolation":"worktree"}),
    )
    .await
    .unwrap();
    let id = first["id"].as_str().unwrap().to_owned();
    let binding = flow
        .runtime
        .state(&id)
        .await
        .unwrap()
        .child_worktree()
        .unwrap()
        .clone();
    flow.runtime.shutdown().await;
    let mut fixture = flow.f;
    fixture.renew_host(Some("full-access".into()));
    let restored = Flow::with(fixture, vec![text("resumed")], false, Arc::new(NoSnapshots));
    assert_eq!(
        restored.runtime.state(&id).await.unwrap().child_worktree(),
        Some(&binding)
    );
    let resumed = invoke(
        &restored,
        &parent,
        json!({"prompt":"continue","resume":"durable"}),
    )
    .await
    .unwrap();
    assert_eq!(resumed["id"], first["id"]);
    assert_eq!(resumed["worktree"]["id"], first["worktree"]["id"]);
    assert_eq!(resumed["text"], "resumed");
}

#[tokio::test]
async fn concurrent_children_edit_the_same_filename_in_separate_checkouts() {
    let initial = flow(vec![], false);
    initial.runtime.shutdown().await;
    let scripts = |content: &str| {
        vec![
            call(
                "edit",
                "write",
                json!({"path":"shared.txt","content":content}),
            ),
            text(content),
        ]
    };
    let mut fixture = initial.f;
    fixture.renew_host(Some("full-access".into()));
    let flow = Flow::with_models(
        fixture,
        vec![],
        false,
        Arc::new(NoSnapshots),
        vec![
            ("test/left", scripts("left\n")),
            ("test/right", scripts("right\n")),
        ],
    );
    let parent = flow.session("bypass").await;
    let (left, right) = tokio::join!(
        invoke(
            &flow,
            &parent,
            json!({"prompt":"left","name":"left","isolation":"worktree","model":"test/left"})
        ),
        invoke(
            &flow,
            &parent,
            json!({"prompt":"right","name":"right","isolation":"worktree","model":"test/right"})
        )
    );
    let left = left.unwrap();
    let right = right.unwrap();
    assert_ne!(left["worktree"]["id"], right["worktree"]["id"]);
    assert_ne!(left["worktree"]["branch"], right["worktree"]["branch"]);
    for (result, expected) in [(&left, "left\n"), (&right, "right\n")] {
        let path = std::path::Path::new(result["worktree"]["path"].as_str().unwrap());
        assert_eq!(
            std::fs::read_to_string(path.join("shared.txt")).unwrap(),
            expected
        );
        assert_eq!(result["worktree"]["kept"], true);
    }
    assert_eq!(
        std::fs::read_to_string(flow.f.repo.join("tracked.txt")).unwrap(),
        "original\n"
    );
}

#[tokio::test]
async fn setup_failure_preserves_owned_checkout_without_child_inference() {
    let flow = flow(vec![], false);
    flow.f.set_config(json!({"permissions":{"agent":"allow","worktree":"allow"},"worktrees":{"setup":["exit 17"]}}));
    let parent = flow.session("bypass").await;
    let error = invoke(
        &flow,
        &parent,
        json!({"prompt":"inspect","name":"setup-failure","isolation":"worktree"}),
    )
    .await
    .unwrap_err();
    assert!(error.contains("17"), "{error}");
    assert!(flow.main.requests().is_empty());
    let sessions = flow.runtime.list(&Default::default()).unwrap().sessions;
    let child = sessions
        .iter()
        .find(|session| session.parent_id.as_deref() == Some(&parent))
        .unwrap();
    let state = flow.runtime.state(&child.id).await.unwrap();
    let binding = state.child_worktree().unwrap();
    assert!(binding.path.exists());
    assert_eq!(
        git(
            &flow.f.repo,
            &[
                "branch",
                "--list",
                "--format=%(refname:short)",
                &binding.branch
            ]
        ),
        binding.branch
    );
}

#[tokio::test]
async fn completed_clean_checkout_recreates_for_resume_with_new_binding() {
    let flow = flow(vec![text("first"), text("second")], false);
    let parent = flow.session("bypass").await;
    let first = invoke(
        &flow,
        &parent,
        json!({"prompt":"inspect","name":"removed","isolation":"worktree"}),
    )
    .await
    .unwrap();
    assert_eq!(first["worktree"]["kept"], false);
    let resumed = invoke(
        &flow,
        &parent,
        json!({"prompt":"continue","resume":"removed"}),
    )
    .await
    .unwrap();
    assert_eq!(resumed["id"], first["id"]);
    assert_ne!(resumed["worktree"]["id"], first["worktree"]["id"]);
    assert_eq!(resumed["worktree"]["branch"], first["worktree"]["branch"]);
    assert_eq!(resumed["worktree"]["path"], first["worktree"]["path"]);
    assert_eq!(resumed["text"], "second");
    assert_eq!(resumed["worktree"]["kept"], false);
    let state = flow
        .runtime
        .state(resumed["id"].as_str().unwrap())
        .await
        .unwrap();
    assert!(!state.child_worktree_setup_pending());
    assert_eq!(
        state.info.worktree_id.as_deref(),
        resumed["worktree"]["id"].as_str()
    );
    assert!(format!("{:?}", flow.main.requests()[1].messages).contains("first"));
}

#[tokio::test]
async fn cleanup_ask_policy_preserves_checkout_without_implicit_deletion() {
    let flow = flow(vec![text("first")], false);
    flow.f.set_config(
        json!({"permissions":{"agent":"allow","worktree":"allow"},"worktrees":{"cleanup":"ask"}}),
    );
    let parent = flow.session("bypass").await;
    let result = invoke(
        &flow,
        &parent,
        json!({"prompt":"inspect","isolation":"worktree"}),
    )
    .await
    .unwrap();
    assert_eq!(result["worktree"]["kept"], true);
    assert!(std::path::Path::new(result["worktree"]["path"].as_str().unwrap()).exists());
}

#[tokio::test]
async fn worktree_deny_added_during_approval_prevents_checkout_creation() {
    use cyber_server::runtime::PermissionReply;
    let flow = flow(
        vec![
            call(
                "spawn",
                "agent",
                json!({"prompt":"inspect","name":"approved","isolation":"worktree"}),
            ),
            text("finished"),
        ],
        true,
    );
    flow.f
        .set_config(json!({"permissions":{"agent":"allow","worktree":"ask"}}));
    let parent = flow.session("default").await;
    flow.prompt(&parent, "delegate").await;
    let request = flow.pending(&parent).await;
    assert!(
        matches!(&request.kind, cyber_server::runtime::PendingKind::Permission(ask) if ask.action == "worktree")
    );
    flow.f
        .set_config(json!({"permissions":{"agent":"allow","worktree":"deny"}}));
    flow.runtime
        .reply_permission(&request.id, PermissionReply::Once)
        .await
        .unwrap();
    flow.settle(&parent).await;
    assert!(flow.output(&parent, "spawn").await.contains("denied"));
    assert_eq!(
        flow.runtime
            .list(&Default::default())
            .unwrap()
            .sessions
            .len(),
        1
    );
    assert!(!flow.f.repo.join(".git/cyber-worktrees").exists());
}

#[tokio::test]
async fn stopping_a_child_awaiting_permission_keeps_its_checkout() {
    let flow = flow(
        vec![
            call("read", "read", json!({"path":".env"})),
            text("notice handled"),
        ],
        true,
    );
    flow.f
        .set_config(json!({"permissions":{"agent":"allow","worktree":"allow","read":"ask"}}));
    let parent = flow.session("default").await;
    let started = invoke(
        &flow,
        &parent,
        json!({"prompt":"inspect","name":"waiting","isolation":"worktree","background":true}),
    )
    .await
    .unwrap();
    let request = flow.pending(&parent).await;
    let child = flow.runtime.state(&request.session_id).await.unwrap();
    let binding = child.child_worktree().unwrap().clone();
    flow.runtime
        .cancel_job(started["job_id"].as_str().unwrap())
        .await
        .unwrap();
    let job = done(&flow, started["job_id"].as_str().unwrap()).await;
    assert_eq!(job.status, cyber_server::runtime::JobStatus::Cancelled);
    assert!(binding.path.exists());
    assert!(flow.runtime.pending_requests(Some(&parent)).is_empty());
}

async fn cleanup_job(flow: &Flow, parent: &str) -> (String, cyber_server::runtime::PendingRequest) {
    let started = invoke(
        flow,
        parent,
        json!({"prompt":"inspect","isolation":"worktree","background":true}),
    )
    .await
    .unwrap();
    let request = flow.pending(parent).await;
    assert!(!flow.runtime.is_running(&request.session_id));
    assert!(
        matches!(&request.kind, cyber_server::runtime::PendingKind::Permission(ask) if ask.action == "worktree" && ask.metadata["operation"] == "cleanup" && ask.metadata["requires_confirmation"] == true && ask.always_patterns.is_empty())
    );
    if let cyber_server::runtime::PendingKind::Permission(ask) = &request.kind {
        assert!(ask.resources[0].contains(ask.metadata["path"].as_str().unwrap()));
        assert!(ask.resources[1].contains(ask.metadata["branch"].as_str().unwrap()));
    }
    (started["job_id"].as_str().unwrap().into(), request)
}
fn ask_cleanup(flow: &Flow) {
    flow.f.set_config(
        json!({"permissions":{"agent":"allow","worktree":"allow"},"worktrees":{"cleanup":"ask"}}),
    );
}

#[tokio::test]
async fn cleanup_approval_removes_only_the_child_checkout() {
    let flow = flow(vec![text("child"), text("notice")], true);
    ask_cleanup(&flow);
    let parent = flow.session("bypass").await;
    let (job, request) = cleanup_job(&flow, &parent).await;
    flow.runtime
        .reply_permission(&request.id, cyber_server::runtime::PermissionReply::Once)
        .await
        .unwrap();
    let result = done(&flow, &job).await.result.unwrap();
    assert_eq!(result["worktree"]["kept"], false, "{result}");
    assert!(!std::path::Path::new(result["worktree"]["path"].as_str().unwrap()).exists());
    assert!(flow.f.repo.join("tracked.txt").exists());
}

#[tokio::test]
async fn declining_cleanup_keeps_checkout_and_completes_the_child() {
    let flow = flow(vec![text("child"), text("notice")], true);
    ask_cleanup(&flow);
    let parent = flow.session("bypass").await;
    let (job, request) = cleanup_job(&flow, &parent).await;
    flow.runtime
        .reply_permission(
            &request.id,
            cyber_server::runtime::PermissionReply::Reject { message: None },
        )
        .await
        .unwrap();
    let result = done(&flow, &job).await.result.unwrap();
    assert_eq!(result["worktree"]["kept"], true);
    assert!(std::path::Path::new(result["worktree"]["path"].as_str().unwrap()).exists());
    assert_eq!(result["text"], "child");
}

#[tokio::test]
async fn stopping_background_cleanup_clears_idle_child_requests_and_keeps_checkout() {
    let flow = flow(vec![text("child"), text("notice")], true);
    ask_cleanup(&flow);
    let parent = flow.session("bypass").await;
    let (job, request) = cleanup_job(&flow, &parent).await;
    let binding = flow
        .runtime
        .state(&request.session_id)
        .await
        .unwrap()
        .child_worktree()
        .unwrap()
        .clone();
    flow.runtime.cancel_job(&job).await.unwrap();
    assert_eq!(
        done(&flow, &job).await.status,
        cyber_server::runtime::JobStatus::Cancelled
    );
    assert!(flow.runtime.pending_requests(Some(&parent)).is_empty());
    assert!(binding.path.exists());
}

#[tokio::test]
async fn cleanup_approval_rechecks_new_edits_policy_and_denies() {
    use cyber_server::runtime::PermissionReply;
    for change in ["edits", "keep", "deny"] {
        let flow = flow(vec![text("child"), text("notice")], true);
        ask_cleanup(&flow);
        let parent = flow.session("bypass").await;
        let (job, request) = cleanup_job(&flow, &parent).await;
        let binding = flow
            .runtime
            .state(&request.session_id)
            .await
            .unwrap()
            .child_worktree()
            .unwrap()
            .clone();
        match change {
            "edits" => std::fs::write(binding.path.join("user.txt"), "user edit\n").unwrap(),
            "keep" => flow.f.set_config(json!({"permissions":{"agent":"allow","worktree":"allow"},"worktrees":{"cleanup":"keep"}})),
            "deny" => flow.f.set_config(json!({"permissions":{"agent":"allow","worktree":"deny"},"worktrees":{"cleanup":"ask"}})),
            _ => unreachable!(),
        }
        flow.runtime
            .reply_permission(&request.id, PermissionReply::Once)
            .await
            .unwrap();
        let result = done(&flow, &job).await.result.unwrap();
        assert!(binding.path.exists(), "{change}: {result}");
        if change == "deny" {
            assert!(
                result["worktree"]["cleanup_error"]
                    .as_str()
                    .unwrap()
                    .contains("denied")
            );
        } else {
            assert_eq!(result["worktree"]["kept"], true);
        }
        if change == "edits" {
            assert_eq!(
                result["worktree"]["changes"]["files"][0]["file"],
                "user.txt"
            );
        }
    }
}

#[tokio::test]
async fn foreground_interrupt_clears_cleanup_request_and_preserves_checkout() {
    let flow = flow(
        vec![
            call(
                "spawn",
                "agent",
                json!({"prompt":"inspect","isolation":"worktree"}),
            ),
            text("child"),
        ],
        true,
    );
    ask_cleanup(&flow);
    let parent = flow.session("bypass").await;
    flow.prompt(&parent, "delegate").await;
    let request = flow.pending(&parent).await;
    assert!(
        matches!(&request.kind, cyber_server::runtime::PendingKind::Permission(ask) if ask.metadata["operation"] == "cleanup")
    );
    let binding = flow
        .runtime
        .state(&request.session_id)
        .await
        .unwrap()
        .child_worktree()
        .unwrap()
        .clone();
    flow.runtime.interrupt(&parent).await.unwrap();
    assert!(flow.runtime.pending_requests(Some(&parent)).is_empty());
    assert!(binding.path.exists());
    assert!(!flow.runtime.is_running(&request.session_id));
}

#[tokio::test]
async fn waiting_cleanup_does_not_hold_the_repository_lifecycle_lock() {
    let flow = flow(
        vec![
            text("first"),
            text("second"),
            text("notice"),
            text("notice"),
        ],
        true,
    );
    ask_cleanup(&flow);
    let parent = flow.session("bypass").await;
    let (first, first_request) = cleanup_job(&flow, &parent).await;
    let second = invoke(
        &flow,
        &parent,
        json!({"prompt":"inspect too","isolation":"worktree","background":true}),
    )
    .await
    .unwrap();
    let second_request = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            if let Some(request) = flow
                .runtime
                .pending_requests(Some(&parent))
                .into_iter()
                .find(|request| request.id != first_request.id)
            {
                break request;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("cleanup prompt blocked another child checkout");
    assert_ne!(first_request.session_id, second_request.session_id);
    flow.runtime
        .reply_permission(
            &first_request.id,
            cyber_server::runtime::PermissionReply::Once,
        )
        .await
        .unwrap();
    flow.runtime
        .reply_permission(
            &second_request.id,
            cyber_server::runtime::PermissionReply::Once,
        )
        .await
        .unwrap();
    assert_eq!(
        done(&flow, &first).await.result.unwrap()["worktree"]["kept"],
        false
    );
    assert_eq!(
        done(&flow, second["job_id"].as_str().unwrap())
            .await
            .result
            .unwrap()["worktree"]["kept"],
        false
    );
}

fn isolated_user_profile(flow: &Flow) {
    flow.f
        .set_config(json!({"agents":{"build":{"isolation":"worktree"}}}));
}

#[tokio::test]
async fn explicit_user_isolated_subtasks_work_in_all_modes_without_spawn_approval() {
    for mode in [
        "default",
        "accept-edits",
        "plan",
        "auto",
        "dont-ask",
        "bypass",
    ] {
        let flow = flow(vec![text("child"), text("notice")], false);
        isolated_user_profile(&flow);
        let parent = flow.session(mode).await;
        let job = flow
            .runtime
            .subtask(&parent, "inspect separately")
            .await
            .unwrap();
        let child = flow.runtime.state(&job.child_id).await.unwrap();
        assert_eq!(child.info.mode, mode);
        assert_eq!(child.info.agent, "build");
        assert!(child.child_worktree().is_some());
        assert_ne!(child.info.directory, flow.f.repo.display().to_string());
        let result = done(&flow, &job.id).await.result.unwrap();
        assert_eq!(result["text"], "child");
        assert_eq!(result["worktree"]["kept"], false, "{mode}: {result}");
        assert!(flow.runtime.pending_requests(None).is_empty());
    }
}

#[tokio::test]
async fn explicit_user_isolation_does_not_authorize_plan_child_model_writes() {
    let flow = flow(
        vec![
            call(
                "write",
                "write",
                json!({"path":"forbidden.txt","content":"must not write"}),
            ),
            text("read-only findings"),
            text("notice"),
        ],
        false,
    );
    isolated_user_profile(&flow);
    let parent = flow.session("plan").await;
    let job = flow
        .runtime
        .subtask(&parent, "inspect separately")
        .await
        .unwrap();
    let result = done(&flow, &job.id).await.result.unwrap();
    let child = flow.runtime.state(&job.child_id).await.unwrap();
    assert_eq!(child.info.mode, "plan");
    assert!(
        child.calls["write"]
            .output
            .as_deref()
            .unwrap()
            .contains("Plan mode is read-only")
    );
    assert_eq!(result["worktree"]["changes"]["files"], json!([]));
    assert!(!flow.f.repo.join("forbidden.txt").exists());
}

#[tokio::test]
async fn explicit_user_isolation_retains_child_tool_approval_routing() {
    let flow = flow(
        vec![
            call(
                "write",
                "write",
                json!({"path":"approved.txt","content":"approved change\n"}),
            ),
            text("child"),
            text("notice"),
        ],
        true,
    );
    isolated_user_profile(&flow);
    let parent = flow.session("default").await;
    let job = flow
        .runtime
        .subtask(&parent, "make a change separately")
        .await
        .unwrap();
    let request = flow.pending(&parent).await;
    assert_eq!(request.session_id, job.child_id);
    assert!(
        matches!(&request.kind, cyber_server::runtime::PendingKind::Permission(ask) if ask.action == "edit")
    );
    flow.runtime
        .reply_permission(&request.id, cyber_server::runtime::PermissionReply::Once)
        .await
        .unwrap();
    let result = done(&flow, &job.id).await.result.unwrap();
    assert_eq!(result["worktree"]["kept"], true);
    assert_eq!(
        result["worktree"]["changes"]["files"][0]["file"],
        "approved.txt"
    );
    assert!(!flow.f.repo.join("approved.txt").exists());
}

#[tokio::test]
async fn explicit_user_isolation_runs_trusted_setup_in_plan_without_widening_model_tools() {
    let flow = flow(vec![text("child"), text("notice")], false);
    flow.f.set_config(json!({"agents":{"build":{"isolation":"worktree"}},"worktrees":{"setup":["printf 'setup\\n' > initialized.txt"]}}));
    let parent = flow.session("plan").await;
    let job = flow
        .runtime
        .subtask(&parent, "inspect separately")
        .await
        .unwrap();
    let result = done(&flow, &job.id).await.result.unwrap();
    let path = std::path::Path::new(result["worktree"]["path"].as_str().unwrap());
    assert_eq!(
        std::fs::read_to_string(path.join("initialized.txt")).unwrap(),
        "setup\n"
    );
    assert_eq!(result["worktree"]["kept"], true);
    assert!(!flow.f.repo.join("initialized.txt").exists());
    assert_eq!(
        flow.runtime.state(&job.child_id).await.unwrap().info.mode,
        "plan"
    );
}

#[tokio::test]
async fn explicit_user_isolation_denies_before_any_native_checkout_or_model_request() {
    for permissions in [json!({"worktree":"deny"}), json!({"agent":"deny"})] {
        let flow = flow(vec![], false);
        flow.f.set_config(json!({"agents":{"build":{"isolation":"worktree","permissions":{"worktree":"allow","agent":"allow"}}},"permissions":permissions}));
        let parent = flow.session("bypass").await;
        assert!(
            flow.runtime
                .subtask(&parent, "inspect")
                .await
                .unwrap_err()
                .to_string()
                .contains("denied")
        );
        assert_eq!(
            flow.runtime
                .list(&Default::default())
                .unwrap()
                .sessions
                .len(),
            1
        );
        assert!(!flow.f.repo.join(".git/cyber-worktrees").exists());
        assert!(flow.main.requests().is_empty());
    }
}

#[tokio::test]
async fn explicit_user_isolation_respects_read_only_sandbox_and_ancestor_denies() {
    let original = flow(vec![], false);
    original.runtime.shutdown().await;
    let mut fixture = original.f;
    fixture.renew_host(Some("read-only".into()));
    let readonly = Flow::with(fixture, vec![], false, Arc::new(NoSnapshots));
    isolated_user_profile(&readonly);
    let parent = readonly.session("bypass").await;
    assert!(
        readonly
            .runtime
            .subtask(&parent, "inspect")
            .await
            .unwrap_err()
            .to_string()
            .contains("Read-only sandbox")
    );
    assert!(!readonly.f.repo.join(".git/cyber-worktrees").exists());
    assert!(readonly.main.requests().is_empty());

    let flow = flow(vec![], false);
    isolated_user_profile(&flow);
    let root = flow
        .runtime
        .create_session(cyber_server::runtime::CreateSession {
            directory: flow.f.repo.display().to_string(),
            model: "test/main".into(),
            mode: Some("bypass".into()),
            rules: Some(json!({"worktree":"deny"})),
            ..Default::default()
        })
        .await
        .unwrap();
    let parent = flow
        .runtime
        .create_session(cyber_server::runtime::CreateSession {
            directory: flow.f.repo.display().to_string(),
            model: "test/main".into(),
            mode: Some("bypass".into()),
            parent_id: Some(root.id),
            rules: Some(json!({"worktree":"allow"})),
            ..Default::default()
        })
        .await
        .unwrap();
    assert!(
        flow.runtime
            .subtask(&parent.id, "inspect")
            .await
            .unwrap_err()
            .to_string()
            .contains("denied")
    );
    assert!(!flow.f.repo.join(".git/cyber-worktrees").exists());
    assert_eq!(
        flow.runtime
            .list(&Default::default())
            .unwrap()
            .sessions
            .len(),
        2
    );
    assert!(flow.main.requests().is_empty());
}

#[tokio::test]
async fn model_selected_plan_child_can_verify_empty_setup_without_model_writes() {
    let flow = flow(vec![text("findings")], false);
    flow.f.set_config(json!({"permissions":{"agent":"allow","worktree":"allow"},"agents":{"general":{"permission_mode":"plan"}}}));
    let parent = flow.session("bypass").await;
    let result = invoke(
        &flow,
        &parent,
        json!({"prompt":"inspect","isolation":"worktree"}),
    )
    .await
    .unwrap();
    assert_eq!(
        flow.runtime
            .state(result["id"].as_str().unwrap())
            .await
            .unwrap()
            .info
            .mode,
        "plan"
    );
    assert_eq!(result["worktree"]["kept"], false);
    assert_eq!(result["text"], "findings");
}

#[tokio::test]
async fn isolated_names_use_git_branch_validation_independently_of_storage_names() {
    let flow = flow(vec![text("findings")], false);
    flow.f.set_config(
        json!({"permissions":{"agent":"allow","worktree":"allow"},"worktrees":{"keep":"always"}}),
    );
    let parent = flow.session("bypass").await;
    let name = format!("Review_{}", "x".repeat(90));
    let result = invoke(
        &flow,
        &parent,
        json!({"prompt":"inspect","name":name,"isolation":"worktree"}),
    )
    .await
    .unwrap();
    assert!(
        result["worktree"]["branch"]
            .as_str()
            .unwrap()
            .ends_with(&name)
    );
    let child = flow
        .runtime
        .state(result["id"].as_str().unwrap())
        .await
        .unwrap();
    assert_eq!(child.info.subagent_name.as_deref(), Some(name.as_str()));
    assert!(child.child_worktree().unwrap().name.starts_with("child-"));
    let error = invoke(
        &flow,
        &parent,
        json!({"prompt":"inspect","name":"invalid..branch","isolation":"worktree"}),
    )
    .await
    .unwrap_err();
    assert!(error.contains("not a valid branch name"), "{error}");
    assert_eq!(
        flow.runtime
            .list(&Default::default())
            .unwrap()
            .sessions
            .len(),
        2
    );
    assert_eq!(flow.main.requests().len(), 1);
}

#[tokio::test]
async fn recreated_binding_replays_and_repeated_resume_keeps_original_base_and_history() {
    let flow = flow(
        vec![
            text("first"),
            call("read", "read", json!({"path":"tracked.txt"})),
            text("second"),
        ],
        false,
    );
    let parent = flow.session("bypass").await;
    let first = invoke(
        &flow,
        &parent,
        json!({"prompt":"inspect","name":"durable-clean","isolation":"worktree"}),
    )
    .await
    .unwrap();
    let old = flow
        .runtime
        .state(first["id"].as_str().unwrap())
        .await
        .unwrap()
        .child_worktree()
        .unwrap()
        .clone();
    flow.f.write("tracked.txt", "new source\n");
    git(&flow.f.repo, &["add", "tracked.txt"]);
    git(&flow.f.repo, &["commit", "--quiet", "-m", "new source"]);
    let second = invoke(
        &flow,
        &parent,
        json!({"prompt":"inspect again","resume":"durable-clean"}),
    )
    .await
    .unwrap();
    let state = flow
        .runtime
        .state(second["id"].as_str().unwrap())
        .await
        .unwrap();
    assert_eq!(state.child_worktree().unwrap().base, old.base);
    assert!(
        state.calls["read"]
            .output
            .as_deref()
            .unwrap()
            .contains("original")
    );
    assert_eq!(
        std::fs::read_to_string(flow.f.repo.join("tracked.txt")).unwrap(),
        "new source\n"
    );
    flow.runtime.shutdown().await;
    let mut fixture = flow.f;
    fixture.renew_host(Some("full-access".into()));
    let restored = Flow::with(fixture, vec![text("third")], false, Arc::new(NoSnapshots));
    assert_eq!(
        restored
            .runtime
            .state(&state.info.id)
            .await
            .unwrap()
            .child_worktree(),
        state.child_worktree()
    );
    let third = invoke(
        &restored,
        &parent,
        json!({"prompt":"continue","resume":"durable-clean"}),
    )
    .await
    .unwrap();
    assert_eq!(third["id"], first["id"]);
    assert_ne!(third["worktree"]["id"], second["worktree"]["id"]);
    assert_eq!(third["text"], "third");
    assert!(format!("{:?}", restored.main.requests()[0].messages).contains("second"));
}

#[tokio::test]
async fn recreation_clears_read_proofs_from_the_removed_checkout() {
    let flow = flow(
        vec![
            call("read", "read", json!({"path":"tracked.txt"})),
            text("first"),
            call(
                "write",
                "write",
                json!({"path":"tracked.txt","content":"not allowed without a fresh read"}),
            ),
            text("second"),
        ],
        false,
    );
    let parent = flow.session("bypass").await;
    let first = invoke(
        &flow,
        &parent,
        json!({"prompt":"inspect","name":"read-cache","isolation":"worktree"}),
    )
    .await
    .unwrap();
    let resumed = invoke(
        &flow,
        &parent,
        json!({"prompt":"continue","resume":"read-cache"}),
    )
    .await
    .unwrap();
    let state = flow
        .runtime
        .state(first["id"].as_str().unwrap())
        .await
        .unwrap();
    assert_eq!(
        state.calls["write"].status,
        cyber_server::runtime::CallStatus::Error
    );
    assert_eq!(resumed["worktree"]["kept"], false);
}

#[tokio::test]
async fn rebound_setup_failure_is_durable_and_blocks_all_inference_and_idle_work() {
    let flow = flow(vec![text("first")], false);
    let parent = flow.session("bypass").await;
    let first = invoke(
        &flow,
        &parent,
        json!({"prompt":"inspect","name":"setup-gate","isolation":"worktree"}),
    )
    .await
    .unwrap();
    flow.f.set_config(json!({"permissions":{"agent":"allow","worktree":"allow"},"worktrees":{"setup":["exit 17"]}}));
    let error = invoke(
        &flow,
        &parent,
        json!({"prompt":"continue","resume":"setup-gate"}),
    )
    .await
    .unwrap_err();
    assert!(error.contains("17"), "{error}");
    let id = first["id"].as_str().unwrap();
    let state = flow.runtime.state(id).await.unwrap();
    assert!(state.child_worktree_setup_pending());
    assert!(state.child_worktree().unwrap().path.exists());
    assert_ne!(
        state.info.worktree_id.as_deref(),
        first["worktree"]["id"].as_str()
    );
    assert!(flow.runtime.resume(id).await.is_err());
    assert!(flow.runtime.wake(id).await.is_err());
    assert!(
        flow.runtime
            .admit(
                id,
                cyber_server::runtime::Admission::text(
                    "bypass setup",
                    cyber_server::runtime::Delivery::Queue
                )
            )
            .await
            .is_err()
    );
    assert!(flow.runtime.shell(id, "echo must-not-run").await.is_err());
    assert!(flow.runtime.repair_context(id).await.is_err());
    assert!(
        invoke(
            &flow,
            &parent,
            json!({"prompt":"continue","resume":"setup-gate"})
        )
        .await
        .unwrap_err()
        .contains("setup is incomplete")
    );
    assert_eq!(flow.main.requests().len(), 1);
    flow.runtime.shutdown().await;
    let mut fixture = flow.f;
    fixture.renew_host(Some("full-access".into()));
    let restored = Flow::with(fixture, vec![], false, Arc::new(NoSnapshots));
    assert!(
        restored
            .runtime
            .state(id)
            .await
            .unwrap()
            .child_worktree_setup_pending()
    );
    assert!(restored.runtime.resume(id).await.is_err());
}

#[tokio::test]
async fn recreation_refuses_replaced_path_and_missing_removal_record_before_model_access() {
    for replaced in [false, true] {
        let flow = flow(vec![text("first")], false);
        let parent = flow.session("bypass").await;
        let first = invoke(
            &flow,
            &parent,
            json!({"prompt":"inspect","name":"proof","isolation":"worktree"}),
        )
        .await
        .unwrap();
        let managed = flow
            .runtime
            .state(first["id"].as_str().unwrap())
            .await
            .unwrap()
            .child_worktree()
            .unwrap()
            .clone();
        if replaced {
            std::fs::create_dir_all(&managed.path).unwrap();
            std::fs::write(managed.path.join("user.txt"), "preserve").unwrap();
        } else {
            std::fs::remove_file(
                managed
                    .common_dir
                    .join("cyber-worktree-removals")
                    .join(format!("{}.json", managed.name)),
            )
            .unwrap();
        }
        assert!(
            invoke(
                &flow,
                &parent,
                json!({"prompt":"continue","resume":"proof"})
            )
            .await
            .is_err()
        );
        assert_eq!(flow.main.requests().len(), 1);
        if replaced {
            assert_eq!(
                std::fs::read_to_string(managed.path.join("user.txt")).unwrap(),
                "preserve"
            );
        }
    }
}

#[tokio::test]
async fn rebound_location_updates_sql_and_context_before_inference() {
    let flow = flow(vec![text("first"), text("second")], false);
    let parent = flow.session("bypass").await;
    let first = invoke(
        &flow,
        &parent,
        json!({"prompt":"inspect","name":"relocated","isolation":"worktree"}),
    )
    .await
    .unwrap();
    let target = flow.f.dir.path().join(r"relocated\checkouts");
    flow.f.set_config(json!({"permissions":{"agent":"allow","worktree":"allow"},"worktrees":{"root":target,"keep":"always"}}));
    let second = invoke(
        &flow,
        &parent,
        json!({"prompt":"continue","resume":"relocated"}),
    )
    .await
    .unwrap();
    assert_ne!(second["worktree"]["path"], first["worktree"]["path"]);
    let id = second["id"].as_str().unwrap();
    let state = flow.runtime.state(id).await.unwrap();
    assert_eq!(
        state.info.directory,
        second["worktree"]["path"].as_str().unwrap()
    );
    let record_id = id.to_owned();
    let recorded: String = flow
        .f
        .store
        .read(move |db| {
            Ok(db.query_row(
                "SELECT directory FROM session WHERE id=?1",
                [record_id],
                |row| row.get(0),
            )?)
        })
        .unwrap();
    assert_eq!(recorded, state.info.directory);
    assert!(
        flow.main.requests()[1]
            .messages
            .iter()
            .flat_map(|message| &message.content)
            .any(|content| matches!(content, cyber_llm::Content::Text { text } if text.contains(&state.info.directory)))
    );
    assert_eq!(second["worktree"]["kept"], true);
}

#[tokio::test]
async fn structured_background_resume_recreates_clean_child_before_a_fresh_attempt() {
    let flow = flow(
        vec![
            call("result1", "return_result", json!({"answer":1})),
            call("result2", "return_result", json!({"answer":2})),
            text("notice"),
        ],
        false,
    );
    let parent = flow.session("bypass").await;
    let first = invoke(&flow, &parent, json!({"prompt":"inspect","name":"typed-recreate","isolation":"worktree","output_schema":{"type":"object","properties":{"answer":{"type":"integer"}},"required":["answer"]}})).await.unwrap();
    let started = invoke(
        &flow,
        &parent,
        json!({"prompt":"continue","resume":"typed-recreate","background":true}),
    )
    .await
    .unwrap();
    let job = done(&flow, started["job_id"].as_str().unwrap()).await;
    let result = job.result.unwrap();
    assert_eq!(result["result"], json!({"answer":2}));
    assert_eq!(result["id"], first["id"]);
    assert_ne!(result["worktree"]["id"], first["worktree"]["id"]);
    assert_eq!(result["worktree"]["kept"], false);
    assert_eq!(job.tokens, 105);
}

#[tokio::test]
async fn fresh_setup_failure_also_fences_resume_and_prompt_admission() {
    let flow = flow(vec![], false);
    flow.f.set_config(json!({"permissions":{"agent":"allow","worktree":"allow"},"worktrees":{"setup":["exit 17"]}}));
    let parent = flow.session("bypass").await;
    assert!(
        invoke(
            &flow,
            &parent,
            json!({"prompt":"inspect","name":"fresh-failure","isolation":"worktree"})
        )
        .await
        .is_err()
    );
    let child = flow
        .runtime
        .resolve_subagent(&parent, "fresh-failure")
        .await
        .unwrap();
    assert!(
        flow.runtime
            .state(&child.id)
            .await
            .unwrap()
            .child_worktree_setup_pending()
    );
    assert!(
        flow.runtime
            .admit(
                &child.id,
                cyber_server::runtime::Admission::text(
                    "skip setup",
                    cyber_server::runtime::Delivery::Queue
                )
            )
            .await
            .is_err()
    );
    assert!(
        invoke(
            &flow,
            &parent,
            json!({"prompt":"continue","resume":"fresh-failure"})
        )
        .await
        .unwrap_err()
        .contains("setup is incomplete")
    );
    assert!(flow.main.requests().is_empty());
}

#[tokio::test]
async fn primary_profile_fork_resumes_as_existing_child_without_authorizing_new_primary_spawns() {
    let flow = flow(vec![text("first"), text("second")], false);
    let parent = flow.session("bypass").await;
    let first = invoke(
        &flow,
        &parent,
        json!({"prompt":"inspect","name":"primary-fork","fork":true,"isolation":"worktree"}),
    )
    .await
    .unwrap();
    let second = invoke(
        &flow,
        &parent,
        json!({"prompt":"continue","resume":"primary-fork"}),
    )
    .await
    .unwrap();
    assert_eq!(second["id"], first["id"]);
    assert_ne!(second["worktree"]["id"], first["worktree"]["id"]);
    assert_eq!(
        flow.runtime
            .state(second["id"].as_str().unwrap())
            .await
            .unwrap()
            .info
            .agent,
        "build"
    );
    assert!(invoke(&flow, &parent, json!({"prompt":"new unauthorized primary spawn","agent":"build","isolation":"worktree"})).await.unwrap_err().contains("cannot run a subagent"));
    assert_eq!(flow.main.requests().len(), 2);
}

#[tokio::test]
async fn explicit_user_primary_subtask_can_resume_after_clean_removal() {
    let flow = flow(vec![text("first"), text("notice"), text("second")], false);
    isolated_user_profile(&flow);
    let parent = flow.session("bypass").await;
    let job = flow
        .runtime
        .subtask(&parent, "inspect separately")
        .await
        .unwrap();
    let result = done(&flow, &job.id).await.result.unwrap();
    let resumed = invoke(
        &flow,
        &parent,
        json!({"prompt":"continue","resume":job.name}),
    )
    .await
    .unwrap();
    assert_eq!(resumed["id"], result["id"]);
    assert_ne!(resumed["worktree"]["id"], result["worktree"]["id"]);
    assert_eq!(resumed["text"], "second");
}

#[tokio::test]
async fn trusted_source_setup_initializes_a_plan_child_without_widening_its_model_tools() {
    let flow = flow(
        vec![
            call(
                "write",
                "write",
                json!({"path":"forbidden.txt","content":"must not write"}),
            ),
            text("findings"),
        ],
        false,
    );
    flow.f.set_config(json!({"permissions":{"agent":"allow","worktree":"allow"},"agents":{"general":{"permission_mode":"plan"}},"worktrees":{"setup":["printf 'initialized\\n' > setup.txt"]}}));
    let parent = flow.session("bypass").await;
    let result = invoke(
        &flow,
        &parent,
        json!({"prompt":"inspect","isolation":"worktree"}),
    )
    .await
    .unwrap();
    let state = flow
        .runtime
        .state(result["id"].as_str().unwrap())
        .await
        .unwrap();
    assert_eq!(state.info.mode, "plan");
    assert!(
        state.calls["write"]
            .output
            .as_deref()
            .unwrap()
            .contains("Plan mode is read-only")
    );
    let managed = state.child_worktree().unwrap();
    assert_eq!(
        std::fs::read_to_string(managed.path.join("setup.txt")).unwrap(),
        "initialized\n"
    );
    assert!(!managed.path.join("forbidden.txt").exists());
    assert!(!flow.f.repo.join("setup.txt").exists());
}

#[cfg(unix)]
#[tokio::test]
async fn explicit_setup_retry_survives_restart_preserves_success_and_does_not_start_inference() {
    use cyber_server::worktrees::SetupRecoveryRequest;
    let flow = flow(vec![], false);
    let setup = json!([
        "printf once >> prefix.txt",
        "printf attempt >> attempts.txt; test -f allow-retry",
        "printf later > later.txt"
    ]);
    flow.f.set_config(json!({"permissions":{"agent":"allow","worktree":"allow"},"worktrees":{"setup":setup,"cleanup":"keep"}}));
    let parent = flow.session("bypass").await;
    assert!(
        invoke(
            &flow,
            &parent,
            json!({"prompt":"inspect","name":"retry-setup","isolation":"worktree"})
        )
        .await
        .is_err()
    );
    let child = flow
        .runtime
        .resolve_subagent(&parent, "retry-setup")
        .await
        .unwrap();
    let original = flow.runtime.state(&child.id).await.unwrap();
    assert!(flow.main.requests().is_empty());
    flow.runtime.shutdown().await;
    let mut fixture = flow.f;
    fixture.renew_host(Some("full-access".into()));
    let restored = Flow::with(
        fixture,
        vec![text("after recovery")],
        false,
        Arc::new(NoSnapshots),
    );
    let snapshot = restored
        .f
        .host
        .inspect_child_worktree_setup(&parent, &child.id, CancellationToken::new())
        .await
        .unwrap();
    assert!(snapshot.setup_pending);
    let continued = restored
        .f
        .host
        .recover_child_worktree_setup(
            &parent,
            &child.id,
            SetupRecoveryRequest {
                revision: snapshot.journal.revision,
                digest: snapshot.journal.digest.clone(),
                retry_index: None,
                reason: "Inspect acknowledged results without authorizing retry".into(),
            },
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(
        continued.setup,
        Ok(cyber_core::worktrees::SetupOutcome::Failed {
            index: 1,
            code: Some(1)
        })
    );
    assert!(
        restored
            .runtime
            .state(&child.id)
            .await
            .unwrap()
            .child_worktree_setup_pending()
    );
    assert_eq!(
        restored
            .f
            .host
            .inspect_child_worktree_setup(&parent, &child.id, CancellationToken::new())
            .await
            .unwrap()
            .journal,
        snapshot.journal
    );
    std::fs::write(
        std::path::Path::new(&child.directory).join("allow-retry"),
        "approved",
    )
    .unwrap();
    let review = SetupRecoveryRequest {
        revision: snapshot.journal.revision,
        digest: snapshot.journal.digest,
        retry_index: Some(1),
        reason: "Prerequisite restored; retry acknowledged failed exit".into(),
    };
    let result = restored
        .f
        .host
        .recover_child_worktree_setup(
            &parent,
            "retry-setup",
            review.clone(),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(
        result.setup,
        Ok(cyber_core::worktrees::SetupOutcome::Completed)
    );
    assert!(restored.main.requests().is_empty());
    let recovered = restored.runtime.state(&child.id).await.unwrap();
    assert!(!recovered.child_worktree_setup_pending());
    assert_eq!(recovered.child_worktree(), original.child_worktree());
    assert_eq!(recovered.entries, original.entries);
    for (name, content) in [
        ("prefix.txt", "once"),
        ("attempts.txt", "attemptattempt"),
        ("later.txt", "later"),
    ] {
        assert_eq!(
            std::fs::read_to_string(std::path::Path::new(&child.directory).join(name)).unwrap(),
            content
        );
        assert!(!restored.f.repo.join(name).exists());
    }
    assert!(
        restored
            .f
            .host
            .recover_child_worktree_setup(&parent, &child.id, review, CancellationToken::new())
            .await
            .is_err()
    );
    let resumed = invoke(
        &restored,
        &parent,
        json!({"prompt":"continue","resume":"retry-setup"}),
    )
    .await
    .unwrap();
    assert_eq!(resumed["id"], child.id);
    assert_eq!(resumed["text"], "after recovery");
}

#[cfg(unix)]
#[tokio::test]
async fn setup_recovery_refuses_foreign_stale_denied_replaced_and_unknown_attempts() {
    use cyber_server::worktrees::{SetupJournal, SetupRecoveryRequest};
    let flow = flow(vec![], false);
    let config = json!({"permissions":{"agent":"allow","worktree":"allow"},"worktrees":{"setup":["printf once >> attempts.txt; exit 7"]}});
    flow.f.set_config(config.clone());
    let parent = flow.session("bypass").await;
    assert!(
        invoke(
            &flow,
            &parent,
            json!({"prompt":"inspect","name":"refuse-recovery","isolation":"worktree"})
        )
        .await
        .is_err()
    );
    let child = flow
        .runtime
        .resolve_subagent(&parent, "refuse-recovery")
        .await
        .unwrap();
    let snapshot = flow
        .f
        .host
        .inspect_child_worktree_setup(&parent, &child.id, CancellationToken::new())
        .await
        .unwrap();
    let review = SetupRecoveryRequest {
        revision: snapshot.journal.revision,
        digest: snapshot.journal.digest,
        retry_index: Some(0),
        reason: "reviewed".into(),
    };
    let foreign = flow.session("bypass").await;
    assert!(
        flow.f
            .host
            .recover_child_worktree_setup(
                &foreign,
                &child.id,
                review.clone(),
                CancellationToken::new()
            )
            .await
            .is_err()
    );
    let owner = flow
        .runtime
        .claim_child_execution(&parent, &child.id)
        .unwrap();
    assert!(
        flow.f
            .host
            .recover_child_worktree_setup(
                &parent,
                &child.id,
                review.clone(),
                CancellationToken::new()
            )
            .await
            .is_err()
    );
    drop(owner);
    let mut stale = review.clone();
    stale.revision -= 1;
    assert!(
        flow.f
            .host
            .recover_child_worktree_setup(&parent, &child.id, stale, CancellationToken::new())
            .await
            .is_err()
    );
    let mut changed = config.clone();
    changed["worktrees"]["setup"] = json!([]);
    flow.f.set_config(changed);
    assert!(
        flow.f
            .host
            .recover_child_worktree_setup(
                &parent,
                &child.id,
                review.clone(),
                CancellationToken::new()
            )
            .await
            .is_err()
    );
    let mut denied = config.clone();
    denied["permissions"]["worktree"] = json!("deny");
    flow.f.set_config(denied);
    assert!(
        flow.f
            .host
            .recover_child_worktree_setup(
                &parent,
                &child.id,
                review.clone(),
                CancellationToken::new()
            )
            .await
            .is_err()
    );
    let mut child_denied = config.clone();
    child_denied["agents"] = json!({"general":{"permissions":{"worktree":"deny"}}});
    flow.f.set_config(child_denied);
    assert!(
        flow.f
            .host
            .recover_child_worktree_setup(
                &parent,
                &child.id,
                review.clone(),
                CancellationToken::new()
            )
            .await
            .is_err()
    );
    flow.f.set_config(config);
    let state = flow.runtime.state(&child.id).await.unwrap();
    let managed = state.child_worktree().unwrap();
    let record = managed
        .common_dir
        .join("cyber-worktrees")
        .join(format!("{}.json", managed.name));
    let original_record = std::fs::read(&record).unwrap();
    let mut replaced = managed.clone();
    replaced.id = "wt_replaced".into();
    std::fs::write(&record, serde_json::to_vec(&replaced).unwrap()).unwrap();
    assert!(
        flow.f
            .host
            .recover_child_worktree_setup(
                &parent,
                &child.id,
                review.clone(),
                CancellationToken::new()
            )
            .await
            .is_err()
    );
    assert_eq!(
        std::fs::read(&record).unwrap(),
        serde_json::to_vec(&replaced).unwrap()
    );
    std::fs::write(&record, original_record).unwrap();
    let journal = SetupJournal::new(
        Arc::clone(&flow.f.store),
        managed,
        &["printf once >> attempts.txt; exit 7".into()],
        &child.id,
    )
    .unwrap();
    assert_eq!(journal.snapshot().unwrap().revision, review.revision);
    // An authorized retry whose dispatch acknowledgement is lost remains unknown.
    journal
        .retry_failed(review.revision, &review.digest, 0, "Explicit retry")
        .unwrap();
    journal.start(0).unwrap();
    let unknown = journal.snapshot().unwrap();
    let unknown_review = SetupRecoveryRequest {
        revision: unknown.revision,
        digest: unknown.digest,
        retry_index: Some(0),
        reason: "must refuse unknown".into(),
    };
    assert!(
        flow.f
            .host
            .recover_child_worktree_setup(
                &parent,
                &child.id,
                unknown_review.clone(),
                CancellationToken::new()
            )
            .await
            .is_err()
    );
    let mut continuation = unknown_review;
    continuation.retry_index = None;
    assert!(
        flow.f
            .host
            .recover_child_worktree_setup(
                &parent,
                &child.id,
                continuation,
                CancellationToken::new()
            )
            .await
            .is_err()
    );
    assert!(
        flow.runtime
            .state(&child.id)
            .await
            .unwrap()
            .child_worktree_setup_pending()
    );
    assert_eq!(
        std::fs::read_to_string(managed.path.join("attempts.txt")).unwrap(),
        "once"
    );
    assert!(flow.main.requests().is_empty());
}

#[cfg(unix)]
#[tokio::test]
async fn read_only_sandbox_refuses_setup_retry_without_changing_its_journal() {
    use cyber_server::worktrees::SetupRecoveryRequest;
    let flow = flow(vec![], false);
    flow.f.set_config(json!({"permissions":{"agent":"allow","worktree":"allow"},"worktrees":{"setup":["printf once > attempts.txt; exit 7"]}}));
    let parent = flow.session("bypass").await;
    assert!(
        invoke(
            &flow,
            &parent,
            json!({"prompt":"inspect","name":"readonly-recovery","isolation":"worktree"})
        )
        .await
        .is_err()
    );
    let child = flow
        .runtime
        .resolve_subagent(&parent, "readonly-recovery")
        .await
        .unwrap();
    let original = flow
        .f
        .host
        .inspect_child_worktree_setup(&parent, &child.id, CancellationToken::new())
        .await
        .unwrap();
    flow.runtime.shutdown().await;
    let mut fixture = flow.f;
    fixture.renew_host(Some("read-only".into()));
    let restored = Flow::with(fixture, vec![], false, Arc::new(NoSnapshots));
    let result = restored
        .f
        .host
        .recover_child_worktree_setup(
            &parent,
            &child.id,
            SetupRecoveryRequest {
                revision: original.journal.revision,
                digest: original.journal.digest.clone(),
                retry_index: Some(0),
                reason: "Explicit retry cannot widen sandbox".into(),
            },
            CancellationToken::new(),
        )
        .await;
    assert!(result.err().unwrap().to_string().contains("Read-only"));
    assert_eq!(
        restored
            .f
            .host
            .inspect_child_worktree_setup(&parent, &child.id, CancellationToken::new())
            .await
            .unwrap()
            .journal,
        original.journal
    );
    assert_eq!(
        std::fs::read_to_string(std::path::Path::new(&child.directory).join("attempts.txt"))
            .unwrap(),
        "once"
    );
    assert!(
        restored
            .runtime
            .state(&child.id)
            .await
            .unwrap()
            .child_worktree_setup_pending()
    );
    assert!(restored.main.requests().is_empty());
}

#[cfg(unix)]
#[tokio::test]
async fn completed_setup_journal_recovers_a_lost_readiness_acknowledgement_without_redispatch() {
    use cyber_server::worktrees::{CommandResult, SetupJournal, SetupRecoveryRequest};
    let flow = flow(vec![], false);
    let command = "printf once > attempts.txt; exit 7";
    flow.f.set_config(
        json!({"permissions":{"agent":"allow","worktree":"allow"},"worktrees":{"setup":[command]}}),
    );
    let parent = flow.session("bypass").await;
    assert!(
        invoke(
            &flow,
            &parent,
            json!({"prompt":"inspect","name":"lost-ready","isolation":"worktree"})
        )
        .await
        .is_err()
    );
    let child = flow
        .runtime
        .resolve_subagent(&parent, "lost-ready")
        .await
        .unwrap();
    let state = flow.runtime.state(&child.id).await.unwrap();
    let managed = state.child_worktree().unwrap();
    // Simulate the durable state after an acknowledged retry but before Session readiness.
    let journal = SetupJournal::new(
        Arc::clone(&flow.f.store),
        managed,
        &[command.into()],
        &child.id,
    )
    .unwrap();
    let failed = journal.snapshot().unwrap();
    journal
        .retry_failed(failed.revision, &failed.digest, 0, "Prior explicit retry")
        .unwrap();
    let attempt = journal.start_attempt(0).unwrap();
    journal
        .finish_attempt(
            0,
            attempt.started_revision.unwrap(),
            CommandResult::Exited { code: Some(0) },
        )
        .unwrap();
    let complete = journal.snapshot().unwrap();
    let result = flow
        .f
        .host
        .recover_child_worktree_setup(
            &parent,
            &child.id,
            SetupRecoveryRequest {
                revision: complete.revision,
                digest: complete.digest.clone(),
                retry_index: None,
                reason: "Reconcile completed setup with missing readiness".into(),
            },
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(
        result.setup,
        Ok(cyber_core::worktrees::SetupOutcome::Completed)
    );
    assert_eq!(journal.snapshot().unwrap(), complete);
    assert!(
        !flow
            .runtime
            .state(&child.id)
            .await
            .unwrap()
            .child_worktree_setup_pending()
    );
    assert_eq!(
        std::fs::read_to_string(managed.path.join("attempts.txt")).unwrap(),
        "once"
    );
    assert!(flow.main.requests().is_empty());
}

#[tokio::test]
async fn named_user_delegation_honors_profile_isolation_and_parent_mode() {
    let flow = flow(
        vec![text("isolated findings"), text("notice handled")],
        false,
    );
    flow.f.set_config(json!({"agents":{"general":{"isolation":"worktree","permission_mode":"bypass"}},"worktrees":{"keep":"always"}}));
    let parent = flow.session("plan").await;
    let job = flow
        .runtime
        .subtask_with_agent(&parent, "inspect in isolation", Some("general".into()))
        .await
        .unwrap();
    let completed = done(&flow, &job.id).await;
    assert_eq!(
        completed.status,
        cyber_server::runtime::JobStatus::Completed
    );
    let child = flow.runtime.state(&job.child_id).await.unwrap();
    assert_eq!(child.info.agent, "general");
    assert_eq!(child.info.mode, "plan");
    assert!(!child.child_worktree_setup_pending());
    let binding = child.child_worktree().unwrap();
    assert_eq!(child.info.directory, binding.path.display().to_string());
    assert_ne!(binding.path, flow.f.repo);
    assert_eq!(
        completed.result.as_ref().unwrap()["text"],
        "isolated findings"
    );
    assert_eq!(
        completed.result.as_ref().unwrap()["worktree"]["id"],
        binding.id
    );
    assert_eq!(completed.result.as_ref().unwrap()["worktree"]["kept"], true);
    assert!(binding.path.join("tracked.txt").exists());
}

#[tokio::test]
async fn long_root_child_reports_changes_lists_status_and_resumes_through_owned_execution() {
    let flow = flow(
        vec![
            call(
                "write",
                "write",
                json!({"path":"generated.txt","content":"child change\n"}),
            ),
            text("first findings"),
            text("resumed findings"),
        ],
        false,
    );
    let root = flow
        .f
        .dir
        .path()
        .join("segment".repeat(15))
        .join("another".repeat(15))
        .join("third-segment".repeat(10));
    flow.f.set_config(
        json!({"permissions":{"agent":"allow","worktree":"allow","edit":"allow"},
        "worktrees":{"root":root,"keep":"always"}}),
    );
    let parent = flow.session("bypass").await;
    let first = invoke(
        &flow,
        &parent,
        json!({"prompt":"implement","name":"long-root",
        "isolation":"worktree","model":"test/main"}),
    )
    .await
    .unwrap();
    let path = std::path::Path::new(first["worktree"]["path"].as_str().unwrap());
    assert!(path.as_os_str().len() > 260);
    assert_eq!(first["worktree"]["changes"]["additions"], 1);
    assert_eq!(
        first["worktree"]["changes"]["files"][0]["file"],
        "generated.txt"
    );
    let listing = flow
        .f
        .host
        .list_worktrees(&flow.f.repo, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(listing.len(), 1);
    assert!(listing[0].status.as_ref().unwrap().dirty);
    let resumed = invoke(
        &flow,
        &parent,
        json!({"prompt":"continue","resume":"long-root"}),
    )
    .await
    .unwrap();
    assert_eq!(resumed["worktree"]["id"], first["worktree"]["id"]);
    assert_eq!(resumed["text"], "resumed findings");
    assert_eq!(
        std::fs::read_to_string(path.join("generated.txt")).unwrap(),
        "child change\n"
    );
    assert!(!flow.f.repo.join("generated.txt").exists());
}

#[tokio::test]
async fn public_child_continuation_recreates_clean_checkout_before_inference_and_keeps_edits() {
    use cyber_server::runtime::{Admission, Delivery};
    let flow = flow(
        vec![
            text("first"),
            call(
                "write2",
                "write",
                json!({"path":"followup.txt","content":"follow up\n"}),
            ),
            text("second"),
        ],
        false,
    );
    let parent = flow.session("bypass").await;
    let first = invoke(
        &flow,
        &parent,
        json!({"prompt":"inspect","name":"public-resume","isolation":"worktree"}),
    )
    .await
    .unwrap();
    let child = first["id"].as_str().unwrap();
    let old = flow
        .runtime
        .state(child)
        .await
        .unwrap()
        .child_worktree()
        .unwrap()
        .clone();
    assert!(!old.path.exists());
    flow.runtime
        .admit_user(child, Admission::text("continue", Delivery::Steer))
        .await
        .unwrap();
    flow.runtime.wait_idle(child).await;
    let state = flow.runtime.state(child).await.unwrap();
    let managed = state.child_worktree().unwrap();
    assert_ne!(managed.id, old.id);
    assert_eq!(managed.branch, old.branch);
    assert_eq!(managed.base, old.base);
    assert!(!state.child_worktree_setup_pending());
    assert_eq!(
        std::fs::read_to_string(managed.path.join("followup.txt")).unwrap(),
        "follow up\n"
    );
    assert!(!flow.f.repo.join("followup.txt").exists());
    assert_eq!(flow.main.requests().len(), 3);
    let events = flow.f.store.read_events(child, -1, 200).unwrap().events;
    let settled = events
        .iter()
        .find(|event| event.kind == "session.child.continuation_settled.1")
        .unwrap();
    assert_eq!(settled.data["worktree"]["kept"], true);
    assert_eq!(settled.data["worktree"]["changes"]["additions"], 1);
}

#[tokio::test]
async fn public_child_continuation_runs_source_setup_in_every_mode_without_widening_tools() {
    use cyber_server::runtime::{Admission, Delivery};
    for mode in [
        "default",
        "accept-edits",
        "plan",
        "auto",
        "dont-ask",
        "bypass",
    ] {
        let flow = flow(vec![text("first"), text("second")], false);
        let parent = flow.session("bypass").await;
        let first = invoke(
            &flow,
            &parent,
            json!({"prompt":"inspect","name":"public-mode","isolation":"worktree"}),
        )
        .await
        .unwrap();
        let child = first["id"].as_str().unwrap();
        flow.runtime.switch_mode(child, mode).await.unwrap();
        flow.f.set_config(json!({"permissions":{"agent":"allow","worktree":"ask"},"worktrees":{"setup":["printf 'trusted\\n' > setup.txt"]}}));
        flow.runtime
            .admit_user(child, Admission::text("continue", Delivery::Steer))
            .await
            .unwrap();
        flow.runtime.wait_idle(child).await;
        let state = flow.runtime.state(child).await.unwrap();
        let managed = state.child_worktree().unwrap();
        assert_eq!(state.info.mode, mode);
        assert_eq!(
            std::fs::read_to_string(managed.path.join("setup.txt")).unwrap(),
            "trusted\n"
        );
        assert!(!flow.f.repo.join("setup.txt").exists());
        assert!(!state.child_worktree_setup_pending());
        assert!(flow.runtime.pending_requests(None).is_empty());
        assert_eq!(flow.main.requests().len(), 2);
    }
}

#[tokio::test]
async fn public_child_continuation_refuses_foreign_removal_evidence_and_current_denies() {
    use cyber_server::runtime::{Admission, Delivery};
    for case in ["missing", "replaced", "deny", "child-deny"] {
        let flow = flow(vec![text("first"), text("must not run")], false);
        let parent = flow.session("bypass").await;
        let first = invoke(
            &flow,
            &parent,
            json!({"prompt":"inspect","name":"public-proof","isolation":"worktree"}),
        )
        .await
        .unwrap();
        let child = first["id"].as_str().unwrap();
        let before = flow.runtime.state(child).await.unwrap();
        let managed = before.child_worktree().unwrap();
        match case {
            "missing" => std::fs::remove_file(managed.common_dir.join("cyber-worktree-removals").join(format!("{}.json", managed.name))).unwrap(),
            "replaced" => {std::fs::create_dir_all(&managed.path).unwrap(); std::fs::write(managed.path.join("user.txt"), "preserve").unwrap();},
            "deny" => flow.f.set_config(json!({"permissions":{"worktree":"deny"}})),
            "child-deny" => flow.f.set_config(json!({"permissions":{"worktree":"allow"},"agents":{"general":{"permissions":{"worktree":"deny"}}}})),
            _ => unreachable!(),
        }
        let result = flow
            .runtime
            .admit_user(child, Admission::text("continue", Delivery::Steer))
            .await;
        assert!(result.is_err(), "{case}: {result:?}");
        assert_eq!(flow.runtime.state(child).await.unwrap(), before, "{case}");
        assert_eq!(flow.main.requests().len(), 1, "{case}");
        if case == "replaced" {
            assert_eq!(
                std::fs::read_to_string(managed.path.join("user.txt")).unwrap(),
                "preserve"
            );
        } else {
            assert!(!managed.path.exists(), "{case}");
        }
    }
}

#[tokio::test]
async fn public_child_continuation_setup_failure_stays_fenced_without_fresh_input() {
    use cyber_server::runtime::{Admission, Delivery};
    let flow = flow(vec![text("first"), text("must not run")], false);
    let parent = flow.session("bypass").await;
    let first = invoke(
        &flow,
        &parent,
        json!({"prompt":"inspect","name":"public-setup-failure","isolation":"worktree"}),
    )
    .await
    .unwrap();
    let child = first["id"].as_str().unwrap();
    let before = flow.runtime.state(child).await.unwrap();
    flow.f
        .set_config(json!({"permissions":{"worktree":"allow"},"worktrees":{"setup":["exit 17"]}}));
    assert!(
        flow.runtime
            .admit_user(child, Admission::text("continue", Delivery::Steer))
            .await
            .unwrap_err()
            .to_string()
            .contains("17")
    );
    let failed = flow.runtime.state(child).await.unwrap();
    assert!(failed.child_worktree_setup_pending());
    assert!(failed.child_worktree().unwrap().path.exists());
    assert_ne!(failed.info.worktree_id, before.info.worktree_id);
    assert_eq!(failed.inbox, before.inbox);
    assert!(
        flow.runtime
            .admit_user(child, Admission::text("retry", Delivery::Steer))
            .await
            .is_err()
    );
    assert_eq!(flow.runtime.state(child).await.unwrap(), failed);
    assert_eq!(flow.main.requests().len(), 1);
}

#[tokio::test]
async fn public_child_continuation_owns_cleanup_until_approval_or_cancellation_settles() {
    use cyber_server::runtime::{Admission, Delivery, PermissionReply, RuntimeError};
    for outcome in ["approve", "interrupt", "shutdown"] {
        let approve = outcome == "approve";
        let flow = flow(vec![text("first"), text("second")], true);
        let parent = flow.session("bypass").await;
        let first = invoke(
            &flow,
            &parent,
            json!({"prompt":"inspect","name":"public-cleanup","isolation":"worktree"}),
        )
        .await
        .unwrap();
        let child = first["id"].as_str().unwrap();
        flow.f.set_config(json!({"permissions":{"agent":"allow","worktree":"allow"},"worktrees":{"cleanup":"ask"}}));
        flow.runtime
            .admit_user(child, Admission::text("continue", Delivery::Steer))
            .await
            .unwrap();
        let request = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                if let Some(request) = flow
                    .runtime
                    .pending_requests(Some(child))
                    .into_iter()
                    .find(|r| r.session_id == child)
                {
                    break request;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        let before = flow.runtime.state(child).await.unwrap();
        let managed = before.child_worktree().unwrap();
        assert!(managed.path.exists());
        assert!(flow.runtime.claim_child_execution(&parent, child).is_err());
        assert!(matches!(
            flow.runtime
                .admit_user(child, Admission::text("late prompt", Delivery::Steer))
                .await,
            Err(RuntimeError::Busy(_))
        ));
        assert!(matches!(
            flow.runtime
                .admit(
                    child,
                    Admission::text("internal late input", Delivery::Queue)
                )
                .await,
            Err(RuntimeError::Busy(_))
        ));
        assert!(matches!(
            flow.runtime.wake(child).await,
            Err(RuntimeError::Busy(_))
        ));
        assert!(matches!(
            flow.runtime.resume(child).await,
            Err(RuntimeError::Busy(_))
        ));
        assert_eq!(flow.runtime.state(child).await.unwrap(), before);
        if approve {
            flow.runtime
                .reply_permission(&request.id, PermissionReply::Once)
                .await
                .unwrap();
        } else if outcome == "interrupt" {
            flow.runtime.interrupt(child).await.unwrap();
        } else {
            tokio::time::timeout(std::time::Duration::from_secs(5), flow.runtime.shutdown())
                .await
                .unwrap();
        }
        flow.runtime.wait_idle(child).await;
        assert_eq!(managed.path.exists(), !approve);
        assert!(flow.runtime.pending_requests(Some(child)).is_empty());
        let _next = flow.runtime.claim_child_execution(&parent, child).unwrap();
        let events = flow.f.store.read_events(child, -1, 200).unwrap().events;
        let settlement = events
            .iter()
            .find(|e| e.kind == "session.child.continuation_settled.1")
            .unwrap();
        assert_eq!(settlement.data["unknown"], false);
        assert_eq!(settlement.data["worktree"]["kept"], !approve);
    }
}

#[tokio::test]
async fn public_child_continuation_read_only_sandbox_refuses_recreation_before_effects() {
    use cyber_server::runtime::{Admission, Delivery};
    let flow = flow(vec![text("first")], false);
    let parent = flow.session("bypass").await;
    let first = invoke(
        &flow,
        &parent,
        json!({"prompt":"inspect","name":"public-read-only","isolation":"worktree"}),
    )
    .await
    .unwrap();
    let child = first["id"].as_str().unwrap().to_owned();
    flow.runtime.shutdown().await;
    let mut fixture = flow.f;
    fixture.renew_host(Some("read-only".into()));
    let restored = Flow::with(
        fixture,
        vec![text("must not run")],
        false,
        Arc::new(NoSnapshots),
    );
    let before = restored.runtime.state(&child).await.unwrap();
    let error = restored
        .runtime
        .admit_user(&child, Admission::text("continue", Delivery::Steer))
        .await
        .unwrap_err();
    assert!(error.to_string().contains("Read-only sandbox"), "{error}");
    assert_eq!(restored.runtime.state(&child).await.unwrap(), before);
    assert!(!before.child_worktree().unwrap().path.exists());
    assert!(restored.main.requests().is_empty());
}

#[tokio::test]
async fn public_child_continuation_plan_mode_keeps_model_writes_denied() {
    use cyber_server::runtime::{Admission, CallStatus, Delivery};
    let flow = flow(
        vec![
            text("first"),
            call(
                "forbidden",
                "write",
                json!({"path":"forbidden.txt","content":"must not write"}),
            ),
            text("read-only findings"),
        ],
        false,
    );
    let parent = flow.session("bypass").await;
    let first = invoke(
        &flow,
        &parent,
        json!({"prompt":"inspect","name":"public-plan","isolation":"worktree"}),
    )
    .await
    .unwrap();
    let child = first["id"].as_str().unwrap();
    flow.runtime.switch_mode(child, "plan").await.unwrap();
    flow.f.set_config(json!({"permissions":{"agent":"allow","worktree":"allow","edit":"allow"},"worktrees":{"setup":["printf 'trusted\\n' > setup.txt"]}}));
    flow.runtime
        .admit_user(child, Admission::text("continue", Delivery::Steer))
        .await
        .unwrap();
    flow.runtime.wait_idle(child).await;
    let state = flow.runtime.state(child).await.unwrap();
    let managed = state.child_worktree().unwrap();
    assert_eq!(state.info.mode, "plan");
    assert_eq!(
        std::fs::read_to_string(managed.path.join("setup.txt")).unwrap(),
        "trusted\n"
    );
    assert!(!managed.path.join("forbidden.txt").exists());
    assert!(!flow.f.repo.join("setup.txt").exists());
    assert!(!flow.f.repo.join("forbidden.txt").exists());
    assert_ne!(
        state
            .calls
            .values()
            .find(|call| call.name == "write")
            .unwrap()
            .status,
        CallStatus::Ok
    );
    assert_eq!(flow.main.requests().len(), 3);
}

#[tokio::test]
async fn deferred_child_input_replays_and_wakes_through_verified_checkout_recreation() {
    use cyber_server::runtime::{Admission, Delivery, InputStatus};
    let flow = flow(vec![text("first")], false);
    let parent = flow.session("bypass").await;
    let first = invoke(
        &flow,
        &parent,
        json!({"prompt":"inspect","name":"deferred-replay","isolation":"worktree"}),
    )
    .await
    .unwrap();
    let child = first["id"].as_str().unwrap().to_owned();
    let original = flow
        .runtime
        .state(&child)
        .await
        .unwrap()
        .child_worktree()
        .unwrap()
        .clone();
    let mut input = Admission::text("obsolete text", Delivery::Queue);
    input.resume = false;
    let receipt = flow.runtime.admit_user(&child, input).await.unwrap();
    flow.runtime
        .edit_input(
            &child,
            &receipt.message_id,
            Some(vec![cyber_llm::Content::Text {
                text: "edited followup".into(),
            }]),
            None,
        )
        .await
        .unwrap();
    assert!(!original.path.exists());
    flow.runtime.shutdown().await;
    let mut fixture = flow.f;
    fixture.renew_host(Some("full-access".into()));
    let restored = Flow::with(
        fixture,
        vec![text("resumed findings")],
        false,
        Arc::new(NoSnapshots),
    );
    restored.runtime.wake(&child).await.unwrap();
    restored.runtime.wait_idle(&child).await;
    let state = restored.runtime.state(&child).await.unwrap();
    let managed = state.child_worktree().unwrap();
    assert_ne!(managed.id, original.id);
    assert_eq!(managed.branch, original.branch);
    assert_eq!(managed.base, original.base);
    assert!(!state.child_worktree_setup_pending());
    assert!(!managed.path.exists());
    let row = state.input(&receipt.message_id).unwrap();
    assert_eq!(row.status, InputStatus::Promoted);
    assert_eq!(row.admitted_seq, receipt.admitted_seq);
    assert_eq!(restored.main.requests().len(), 1);
    let request = serde_json::to_string(&restored.main.requests()[0]).unwrap();
    assert!(request.contains("edited followup"));
    assert!(!request.contains("obsolete text"));
    let events = restored
        .f
        .store
        .read_events(&child, -1, 200)
        .unwrap()
        .events;
    let settled = events
        .iter()
        .rev()
        .find(|event| event.kind == "session.child.continuation_settled.1")
        .unwrap();
    assert_eq!(settled.data["worktree"]["kept"], false);
}

#[tokio::test]
async fn child_wake_without_promotable_input_keeps_removed_checkout_and_terminal_state() {
    use cyber_server::runtime::{Admission, Delivery};
    let flow = flow(vec![text("first"), text("must not run")], false);
    let parent = flow.session("bypass").await;
    let first = invoke(
        &flow,
        &parent,
        json!({"prompt":"inspect","name":"noop-wake","isolation":"worktree"}),
    )
    .await
    .unwrap();
    let child = first["id"].as_str().unwrap();
    // Held input is deliberately excluded from wake until explicitly released.
    flow.runtime
        .admit_user(child, Admission::text("held followup", Delivery::Hold))
        .await
        .unwrap();
    let before = flow.runtime.state(child).await.unwrap();
    assert!(!before.child_worktree().unwrap().path.exists());
    flow.f
        .set_config(json!({"permissions":{"worktree":"deny"}}));
    flow.runtime.wake(child).await.unwrap();
    flow.runtime.wait_idle(child).await;
    assert_eq!(flow.runtime.state(child).await.unwrap(), before);
    assert!(!before.child_worktree().unwrap().path.exists());
    assert_eq!(flow.main.requests().len(), 1);
}

#[tokio::test]
async fn held_child_release_preserves_input_when_checkout_preparation_is_refused_or_fails() {
    use cyber_server::runtime::{Admission, Delivery, InputStatus};
    for failure in ["deny", "setup"] {
        let flow = flow(vec![text("first"), text("must not run")], false);
        let parent = flow.session("bypass").await;
        let first = invoke(
            &flow,
            &parent,
            json!({"prompt":"inspect","name":"held-refusal","isolation":"worktree"}),
        )
        .await
        .unwrap();
        let child = first["id"].as_str().unwrap();
        let receipt = flow
            .runtime
            .admit_user(child, Admission::text("held followup", Delivery::Hold))
            .await
            .unwrap();
        let before = flow.runtime.state(child).await.unwrap();
        flow.f.set_config(match failure {
            "deny" => json!({"permissions":{"worktree":"deny"}}),
            _ => json!({"permissions":{"worktree":"allow"},"worktrees":{"setup":["exit 17"]}}),
        });
        assert!(
            flow.runtime
                .release(child, &receipt.message_id, Delivery::Queue)
                .await
                .is_err()
        );
        let after = flow.runtime.state(child).await.unwrap();
        assert_eq!(after.inbox, before.inbox);
        assert_eq!(
            after.input(&receipt.message_id).unwrap().status,
            InputStatus::Held
        );
        assert_eq!(flow.main.requests().len(), 1);
        if failure == "deny" {
            assert_eq!(after, before);
            assert!(!after.child_worktree().unwrap().path.exists());
        } else {
            assert!(after.child_worktree_setup_pending());
            assert!(after.child_worktree().unwrap().path.exists());
            assert_ne!(after.info.worktree_id, before.info.worktree_id);
        }
    }
}

#[tokio::test]
async fn structured_foreground_result_is_collected_before_automatic_queued_attempts() {
    use cyber_server::runtime::{Admission, Delivery, InputStatus, PermissionReply};
    for cleanup in ["keep", "auto"] {
        let flow = flow(
            vec![
                call("read", "read", json!({"path":".env"})),
                call("first", "return_result", json!(1)),
                call("second", "return_result", json!(2)),
                call("third", "return_result", json!(3)),
            ],
            true,
        );
        flow.f.set_config(json!({"permissions":{"agent":"allow","worktree":"allow","read":"ask"},"worktrees":{"cleanup":cleanup}}));
        let parent = flow.session("default").await;
        let (result, receipts) = tokio::join!(
            invoke(
                &flow,
                &parent,
                json!({"prompt":"inspect","name":"automatic-queue","isolation":"worktree","output_schema":{"type":"integer"}})
            ),
            async {
                let request = flow.pending(&parent).await;
                let child = &request.session_id;
                let mut receipts = Vec::new();
                for text in ["obsolete queued prompt", "last queued prompt"] {
                    receipts.push(
                        flow.runtime
                            .admit_user(child, Admission::text(text, Delivery::Queue))
                            .await
                            .unwrap(),
                    );
                }
                flow.runtime
                    .edit_input(
                        child,
                        &receipts[0].message_id,
                        Some(vec![cyber_llm::Content::Text {
                            text: "edited queued prompt".into(),
                        }]),
                        None,
                    )
                    .await
                    .unwrap();
                assert!(flow.runtime.claim_child_execution(&parent, child).is_err());
                flow.runtime
                    .reply_permission(&request.id, PermissionReply::Once)
                    .await
                    .unwrap();
                receipts
            }
        );
        let result = result.unwrap();
        assert_eq!(result["result"], 1);
        assert_eq!(result["worktree"]["kept"], cleanup == "keep");
        let child = result["id"].as_str().unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(3), async {
            loop {
                if flow.runtime.state(child).await.unwrap().structured_result() == Some(&json!(3)) {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("queued structured attempts should dispatch automatically after result collection");
        flow.runtime.wait_idle(child).await;
        let state = flow.runtime.state(child).await.unwrap();
        for receipt in &receipts {
            let row = state.input(&receipt.message_id).unwrap();
            assert_eq!(row.status, InputStatus::Promoted);
            assert_eq!(row.admitted_seq, receipt.admitted_seq);
        }
        assert!(
            state.input(&receipts[0].message_id).unwrap().promoted_seq
                < state.input(&receipts[1].message_id).unwrap().promoted_seq
        );
        let requests = flow.main.requests();
        assert_eq!(requests.len(), 4);
        let request = serde_json::to_string(&requests[2]).unwrap();
        assert!(request.contains("edited queued prompt"));
        assert!(!request.contains("obsolete queued prompt"));
    }
}

#[tokio::test]
async fn interrupt_cancels_public_child_preparation_before_late_setup_or_input() {
    use cyber_server::runtime::{Admission, Delivery};
    for stop in ["interrupt", "dispose", "shutdown"] {
        let flow = flow(vec![text("first"), text("must not run")], false);
        let parent = flow.session("bypass").await;
        let first = invoke(
            &flow,
            &parent,
            json!({"prompt":"inspect","name":"interrupt-preparation","isolation":"worktree"}),
        )
        .await
        .unwrap();
        let child = first["id"].as_str().unwrap().to_owned();
        let before = flow.runtime.state(&child).await.unwrap();
        flow.f.set_config(json!({"permissions":{"agent":"allow","worktree":"allow"},"worktrees":{"setup":["printf started > setup-started; while [ ! -f release-setup ]; do sleep 0.02; done; printf late > late-effect"]}}));
        let runtime = flow.runtime.clone();
        let id = child.clone();
        let mut continuation = tokio::spawn(async move {
            runtime
                .admit_user(&id, Admission::text("continue", Delivery::Steer))
                .await
        });
        let checkout = tokio::time::timeout(std::time::Duration::from_secs(10), async {
            loop {
                if continuation.is_finished() {
                    let result = (&mut continuation).await;
                    panic!("{stop}: preparation finished before its setup-started marker: {result:?}");
                }
                let state = flow.runtime.state(&child).await.unwrap();
                let path = &state.child_worktree().unwrap().path;
                if path.join("setup-started").exists() {
                    break path.clone();
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap_or_else(|error| {
            let events = flow.f.store.read_events(&child, -1, 100).map(|page| {
                page.events.into_iter().rev().take(8)
                    .map(|event| (event.seq, event.kind, event.data)).collect::<Vec<_>>()
            });
            panic!("{stop}: setup-started marker deadline expired: {error}; continuation finished: {}; recent child events: {events:?}", continuation.is_finished());
        });
        if stop == "dispose" {
            continuation.abort();
        }
        if stop == "shutdown" {
            tokio::time::timeout(std::time::Duration::from_secs(5), flow.runtime.shutdown())
                .await
                .unwrap();
        } else {
            tokio::time::timeout(
                std::time::Duration::from_secs(5),
                flow.runtime.interrupt(&child),
            )
            .await
            .unwrap()
            .unwrap();
        }
        // Release the old process if interruption failed to terminate it, proving late admission.
        std::fs::write(checkout.join("release-setup"), "released\n").unwrap();
        let result = tokio::time::timeout(std::time::Duration::from_secs(5), continuation)
            .await
            .unwrap();
        if stop == "dispose" {
            assert!(result.unwrap_err().is_cancelled());
        } else {
            let result = result.unwrap();
            assert!(
                result.is_err(),
                "interrupted preparation admitted new input: {result:?}"
            );
        }
        flow.runtime.wait_idle(&child).await;
        let state = flow.runtime.state(&child).await.unwrap();
        assert_eq!(state.inbox, before.inbox);
        assert!(state.child_worktree_setup_pending());
        assert!(!checkout.join("late-effect").exists());
        assert_eq!(flow.main.requests().len(), 1);
        assert!(flow.runtime.claim_child_execution(&parent, &child).is_ok());
    }
}
