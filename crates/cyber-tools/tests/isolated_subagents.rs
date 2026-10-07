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
async fn completed_clean_checkout_requires_fresh_child_instead_of_reusing_missing_identity() {
    let flow = flow(vec![text("first")], false);
    let parent = flow.session("bypass").await;
    let first = invoke(
        &flow,
        &parent,
        json!({"prompt":"inspect","name":"removed","isolation":"worktree"}),
    )
    .await
    .unwrap();
    assert_eq!(first["worktree"]["kept"], false);
    let error = invoke(
        &flow,
        &parent,
        json!({"prompt":"continue","resume":"removed"}),
    )
    .await
    .unwrap_err();
    assert!(error.contains("was removed"), "{error}");
    assert_eq!(flow.main.requests().len(), 1);
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
