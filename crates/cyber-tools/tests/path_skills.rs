mod support;
use serde_json::json;
use support::flow::{Flow, call, text};

fn skill(flow: &Flow, name: &str, extra: &str) {
    flow.f.write(&format!(".cyber/skills/{name}/SKILL.md"), &format!("---\nname: {name}\ndescription: Migration <instructions>\npaths: ['db/migrations/**']\n{extra}---\nprivate-skill-body\n"));
}

#[tokio::test]
async fn reminders_are_atomic_for_parallel_reads_survive_restart_and_reset_with_epoch() {
    let mut parallel = call("a", "read", json!({"path":"db/migrations/a.sql"}));
    parallel.insert(
        1,
        call("b", "read", json!({"path":"db/migrations/b.sql"})).remove(0),
    );
    let mut flow = Flow::new(
        vec![
            parallel,
            text("done"),
            call("c", "read", json!({"path":"db/migrations/a.sql"})),
            text("again"),
            call(
                "d",
                "edit",
                json!({"path":"db/migrations/a.sql","old_string":"before","new_string":"after"}),
            ),
            text("edited"),
        ],
        false,
    );
    flow.f.write("db/migrations/a.sql", "before\n");
    flow.f.write("db/migrations/b.sql", "other\n");
    skill(&flow, "migrations", "");
    let id = flow.session("bypass").await;
    flow.prompt(&id, "read both").await;
    flow.settle(&id).await;
    let outputs = [flow.output(&id, "a").await, flow.output(&id, "b").await];
    assert_eq!(
        outputs
            .iter()
            .filter(|output| output.contains("<system-reminder>"))
            .count(),
        1
    );
    assert!(outputs[0].contains("migrations: Migration &lt;instructions&gt;"));
    assert!(!outputs[0].contains("private-skill-body"));
    let epoch = flow.runtime.state(&id).await.unwrap().epoch.unwrap().number;
    flow.restart_default_runtime().await;
    let state = flow.runtime.state(&id).await.unwrap();
    assert!(
        state
            .epoch
            .as_ref()
            .unwrap()
            .reminded_skills
            .contains("migrations")
    );
    flow.prompt(&id, "read again").await;
    flow.settle(&id).await;
    assert!(!flow.output(&id, "c").await.contains("<system-reminder>"));
    assert_eq!(
        flow.runtime.state(&id).await.unwrap().epoch.unwrap().number,
        epoch
    );
    flow.runtime.repair_context(&id).await.unwrap();
    flow.prompt(&id, "edit").await;
    flow.settle(&id).await;
    assert!(flow.output(&id, "d").await.contains("<system-reminder>"));
    flow.runtime.shutdown().await;
}

#[tokio::test]
async fn denied_disabled_unmatched_and_directory_reads_do_not_consume_reminders() {
    let flow = Flow::new(
        vec![
            call("dir", "read", json!({"path":"db/migrations"})),
            text("listed"),
            call("outside", "read", json!({"path":"other.sql"})),
            text("read"),
            call(
                "missing",
                "read",
                json!({"path":"db/migrations/missing.sql"}),
            ),
            text("failed"),
            call("match", "read", json!({"path":"db/migrations/a.sql"})),
            text("matched"),
        ],
        false,
    );
    flow.f.write("db/migrations/a.sql", "data");
    flow.f.write("other.sql", "other");
    skill(&flow, "allowed", "");
    skill(&flow, "denied", "");
    skill(&flow, "disabled", "disable-model-invocation: true\n");
    flow.f
        .set_config(json!({"permissions":{"skill":{"denied":"deny"}}}));
    let id = flow.session("bypass").await;
    for call in ["dir", "outside", "missing"] {
        flow.prompt(&id, call).await;
        flow.settle(&id).await;
        assert!(!flow.output(&id, call).await.contains("<system-reminder>"));
        assert!(
            flow.runtime
                .state(&id)
                .await
                .unwrap()
                .epoch
                .unwrap()
                .reminded_skills
                .is_empty()
        );
    }
    flow.prompt(&id, "matching").await;
    flow.settle(&id).await;
    let output = flow.output(&id, "match").await;
    assert!(output.contains("allowed: Migration"));
    assert!(!output.contains("denied: Migration") && !output.contains("disabled: Migration"));
    flow.runtime.shutdown().await;
}

#[tokio::test]
async fn patch_model_suggests_on_successful_add_and_not_again_on_update() {
    use cyber_server::runtime::{CreateSession, NoSnapshots};
    use std::sync::Arc;
    let flow = Flow::with_models(
        support::Fixture::new(),
        Vec::new(),
        false,
        Arc::new(NoSnapshots),
        vec![(
            "test/patch",
            vec![
                call(
                    "add",
                    "apply_patch",
                    json!({"patch":"*** Begin Patch\n*** Add File: db/migrations/a.sql\n+before\n*** End Patch"}),
                ),
                text("added"),
                call(
                    "update",
                    "apply_patch",
                    json!({"patch":"*** Begin Patch\n*** Update File: db/migrations/a.sql\n@@\n-before\n+after\n*** End Patch"}),
                ),
                text("updated"),
            ],
        )],
    );
    skill(&flow, "migrations", "");
    let id = flow
        .runtime
        .create_session(CreateSession {
            directory: flow.f.repo.display().to_string(),
            model: "test/patch".into(),
            mode: Some("bypass".into()),
            ..Default::default()
        })
        .await
        .unwrap()
        .id;
    flow.prompt(&id, "add").await;
    flow.settle(&id).await;
    let output = flow.output(&id, "add").await;
    assert!(output.contains("<system-reminder>"), "{output}");
    flow.prompt(&id, "update").await;
    flow.settle(&id).await;
    let output = flow.output(&id, "update").await;
    assert!(!output.contains("<system-reminder>"));
    assert_eq!(
        std::fs::read_to_string(flow.f.repo.join("db/migrations/a.sql")).unwrap(),
        "after\n"
    );
    flow.runtime.shutdown().await;
}

#[tokio::test]
async fn compaction_starts_a_fresh_reminder_epoch_and_agent_tool_denial_withholds_hints() {
    use cyber_server::runtime::NoSnapshots;
    use std::sync::Arc;
    let flow = Flow::with_models(
        support::Fixture::new(),
        vec![
            call("before", "read", json!({"path":"db/migrations/a.sql"})),
            text("read"),
            call("after", "read", json!({"path":"db/migrations/a.sql"})),
            text("read again"),
            call("denied", "read", json!({"path":"db/migrations/b.sql"})),
            text("read file"),
        ],
        false,
        Arc::new(NoSnapshots),
        vec![("test/summary", vec![text("Earlier file read summary")])],
    );
    skill(&flow, "migrations", "");
    flow.f.write("db/migrations/a.sql", "before");
    flow.f.write("db/migrations/b.sql", "other");
    let id = flow.session("bypass").await;
    flow.prompt(&id, &"Read migrations. ".repeat(3000)).await;
    flow.settle(&id).await;
    assert!(
        flow.output(&id, "before")
            .await
            .contains("<system-reminder>")
    );
    let before_epoch = flow.runtime.state(&id).await.unwrap().epoch.unwrap().number;
    flow.runtime.compact(&id, None).await.unwrap();
    assert!(flow.runtime.state(&id).await.unwrap().epoch_stale);
    flow.prompt(&id, "again").await;
    flow.settle(&id).await;
    assert!(flow.runtime.state(&id).await.unwrap().epoch.unwrap().number > before_epoch);
    assert!(
        flow.output(&id, "after")
            .await
            .contains("<system-reminder>")
    );
    flow.runtime.repair_context(&id).await.unwrap();
    flow.f
        .set_config(json!({"agents":{"build":{"tools":{"deny":["skill"]}}}}));
    flow.prompt(&id, "read b").await;
    flow.settle(&id).await;
    assert!(
        !flow
            .output(&id, "denied")
            .await
            .contains("<system-reminder>")
    );
    assert!(
        flow.runtime
            .state(&id)
            .await
            .unwrap()
            .epoch
            .unwrap()
            .reminded_skills
            .is_empty()
    );
    flow.runtime.shutdown().await;
}

#[tokio::test]
async fn partial_patch_failure_emits_once_for_the_files_that_actually_changed() {
    use cyber_server::runtime::{CallStatus, CreateSession, NoSnapshots};
    use std::sync::Arc;
    let flow = Flow::with_models(
        support::Fixture::new(),
        Vec::new(),
        false,
        Arc::new(NoSnapshots),
        vec![(
            "test/patch",
            vec![
                call(
                    "partial",
                    "apply_patch",
                    json!({"patch":"*** Begin Patch\n*** Add File: db/migrations/created.sql\n+created\n*** Add File: blocked/child.sql\n+blocked\n*** End Patch"}),
                ),
                text("failed"),
                call("read", "read", json!({"path":"db/migrations/created.sql"})),
                text("read"),
            ],
        )],
    );
    skill(&flow, "migrations", "");
    flow.f.write("blocked", "A file, not a directory");
    let id = flow
        .runtime
        .create_session(CreateSession {
            directory: flow.f.repo.display().to_string(),
            model: "test/patch".into(),
            mode: Some("bypass".into()),
            ..Default::default()
        })
        .await
        .unwrap()
        .id;
    flow.prompt(&id, "patch").await;
    flow.settle(&id).await;
    let output = flow.output(&id, "partial").await;
    assert!(output.contains("Patch partially applied"), "{output}");
    assert_eq!(
        flow.runtime.state(&id).await.unwrap().calls["partial"].status,
        CallStatus::Error
    );
    assert_eq!(
        std::fs::read_to_string(flow.f.repo.join("db/migrations/created.sql")).unwrap(),
        "created\n"
    );
    assert!(output.contains("<system-reminder>"), "{output}");
    assert!(
        flow.runtime
            .state(&id)
            .await
            .unwrap()
            .epoch
            .unwrap()
            .reminded_skills
            .contains("migrations")
    );
    flow.prompt(&id, "read created file").await;
    flow.settle(&id).await;
    assert!(!flow.output(&id, "read").await.contains("<system-reminder>"));
    flow.runtime.shutdown().await;
}

#[tokio::test]
async fn post_hook_failure_keeps_the_mutation_failure_and_durable_reminder() {
    use cyber_core::config::{Resolved, TrustReport};
    use cyber_core::trust::TrustStore;
    use cyber_server::runtime::CallStatus;
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };
    let mut flow = Flow::new(
        vec![
            call(
                "write",
                "write",
                json!({"path":"db/migrations/created.sql","content":"created"}),
            ),
            text("hook failed"),
            call("read", "read", json!({"path":"db/migrations/created.sql"})),
            text("read"),
        ],
        false,
    );
    skill(&flow, "migrations", "");
    let target = flow.f.repo.join("db/migrations/created.sql");
    let root = flow.f.repo.clone();
    let pending = AtomicBool::new(true);
    flow.f
        .host
        .attach_hook_config(
            Arc::new(move |_| {
                if target.exists() && pending.swap(false, Ordering::SeqCst) {
                    return Err("post-hook resolver fixture failed".into());
                }
                Ok(Resolved {
                    value: json!({}),
                    sources: Default::default(),
                    warnings: Vec::new(),
                    layers: Vec::new(),
                    trust: TrustReport {
                        checkout_root: root.clone(),
                        trusted: true,
                        digest: None,
                        definitions: Vec::new(),
                    },
                })
            }),
            TrustStore::new(flow.f.dir.path().join("trust.json")),
        )
        .unwrap();
    let id = flow.session("bypass").await;
    flow.prompt(&id, "write").await;
    flow.settle(&id).await;
    let output = flow.output(&id, "write").await;
    assert!(output.contains("post-hook failed"), "{output}");
    assert_eq!(
        flow.runtime.state(&id).await.unwrap().calls["write"].status,
        CallStatus::Error
    );
    assert_eq!(
        std::fs::read_to_string(flow.f.repo.join("db/migrations/created.sql")).unwrap(),
        "created"
    );
    assert!(output.contains("<system-reminder>"), "{output}");
    flow.restart_default_runtime().await;
    assert!(
        flow.runtime
            .state(&id)
            .await
            .unwrap()
            .epoch
            .unwrap()
            .reminded_skills
            .contains("migrations")
    );
    flow.prompt(&id, "read").await;
    flow.settle(&id).await;
    assert!(!flow.output(&id, "read").await.contains("<system-reminder>"));
    flow.runtime.shutdown().await;
}

#[tokio::test]
async fn reminder_reserves_payload_budget_and_preserves_complete_overflow() {
    let flow = Flow::new(
        vec![
            call("read", "read", json!({"path":"db/migrations/a.sql"})),
            text("done"),
        ],
        false,
    );
    flow.f
        .set_config(json!({"tool_output":{"max_lines":8,"max_bytes":400}}));
    flow.f
        .write("db/migrations/a.sql", &("migration line\n".repeat(30)));
    skill(&flow, "migrations", "");
    let id = flow.session("bypass").await;
    flow.prompt(&id, "read").await;
    flow.settle(&id).await;
    let output = flow.output(&id, "read").await;
    let payload = output
        .lines()
        .filter(|line| !line.starts_with("[output truncated:"))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(payload.lines().count() <= 8, "{output}");
    assert!(payload.len() <= 400, "{output}");
    assert!(output.ends_with("</system-reminder>"), "{output}");
    let path = output
        .rsplit("full output at ")
        .next()
        .unwrap()
        .split(']')
        .next()
        .unwrap();
    let complete = std::fs::read_to_string(path).unwrap();
    assert!(complete.contains("<system-reminder>"), "{complete}");
    assert!(complete.contains("migration line"), "{complete}");
    flow.runtime.shutdown().await;
}

#[tokio::test]
async fn undeliverable_reminder_is_not_consumed_and_can_be_delivered_later() {
    use cyber_server::runtime::CallStatus;
    let mut flow = Flow::new(
        vec![
            call("first", "read", json!({"path":"db/migrations/a.sql"})),
            text("failed"),
            call("second", "read", json!({"path":"db/migrations/a.sql"})),
            text("done"),
        ],
        false,
    );
    flow.f
        .set_config(json!({"tool_output":{"max_lines":2,"max_bytes":400}}));
    flow.f.write("db/migrations/a.sql", "migration");
    skill(&flow, "migrations", "");
    let id = flow.session("bypass").await;
    flow.prompt(&id, "read").await;
    flow.settle(&id).await;
    let state = flow.runtime.state(&id).await.unwrap();
    assert_eq!(state.calls["first"].status, CallStatus::Error);
    assert!(state.epoch.unwrap().reminded_skills.is_empty());
    assert!(
        !flow
            .output(&id, "first")
            .await
            .contains("<system-reminder>")
    );
    let stored = std::fs::read_dir(flow.f.dir.path().join("tool-output"))
        .unwrap()
        .map(|entry| std::fs::read_to_string(entry.unwrap().path()).unwrap())
        .collect::<Vec<_>>();
    assert!(
        stored
            .iter()
            .any(|output| output.contains("migrations: Migration &lt;instructions&gt;"))
    );
    flow.f.set_config(json!({}));
    flow.restart_default_runtime().await;
    assert!(
        flow.runtime
            .state(&id)
            .await
            .unwrap()
            .epoch
            .unwrap()
            .reminded_skills
            .is_empty()
    );
    flow.prompt(&id, "read after budget increase").await;
    flow.settle(&id).await;
    assert!(
        flow.output(&id, "second")
            .await
            .contains("<system-reminder>")
    );
    flow.runtime.shutdown().await;
}

#[tokio::test]
async fn reminder_overflow_storage_failure_does_not_consume_the_skill() {
    use cyber_server::runtime::CallStatus;
    let flow = Flow::new(
        vec![
            call("first", "read", json!({"path":"db/migrations/a.sql"})),
            text("failed"),
            call("second", "read", json!({"path":"db/migrations/a.sql"})),
            text("done"),
        ],
        false,
    );
    flow.f
        .set_config(json!({"tool_output":{"max_lines":8,"max_bytes":220}}));
    flow.f.write("db/migrations/a.sql", &"🦀".repeat(25));
    skill(&flow, "migrations", "");
    let output_dir = flow.f.dir.path().join("tool-output");
    std::fs::write(&output_dir, "not a directory").unwrap();
    let id = flow.session("bypass").await;
    flow.prompt(&id, "read").await;
    flow.settle(&id).await;
    let state = flow.runtime.state(&id).await.unwrap();
    assert_eq!(state.calls["first"].status, CallStatus::Error);
    assert!(state.epoch.unwrap().reminded_skills.is_empty());
    std::fs::remove_file(&output_dir).unwrap();
    flow.prompt(&id, "retry after storage repair").await;
    flow.settle(&id).await;
    let output = flow.output(&id, "second").await;
    assert!(output.contains("<system-reminder>"), "{output}");
    assert_eq!(
        flow.runtime.state(&id).await.unwrap().calls["second"].status,
        CallStatus::Ok
    );
    flow.runtime.shutdown().await;
}
