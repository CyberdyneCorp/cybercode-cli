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
