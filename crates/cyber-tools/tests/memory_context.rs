//! Actual runtime memory baseline and Safe Boundary delivery.
mod support;
use cyber_server::runtime::{ContextObservation, ToolHost, TurnContext};
use serde_json::{Value, json};
use support::Fixture;
use support::flow::{Flow, text};
#[cfg(unix)]
use support::ok;

fn turn(f: &Fixture) -> TurnContext {
    TurnContext {
        session_id: "ses_test".into(),
        directory: f.repo.display().to_string(),
        agent: "build".into(),
        mode: "default".into(),
        prefers_apply_patch: false,
        rules: Value::Null,
    }
}
async fn observe(f: &Fixture) -> ContextObservation {
    f.host
        .context_observations_owned(&turn(f))
        .await
        .remove("core/memory")
        .unwrap()
}
fn value(observation: ContextObservation) -> String {
    let ContextObservation::Value(text) = observation else {
        panic!("{observation:?}")
    };
    text
}
#[cfg(unix)]
fn note(body: &str) -> String {
    format!("---\nname: coding-policy\ndescription: Small patches\ntype: reference\n---\n{body}\n")
}

#[tokio::test]
async fn empty_memory_provides_guidance_without_creating_directories() {
    let f = Fixture::new();
    let context = value(observe(&f).await);
    assert!(context.contains("Save durable user preferences"));
    assert!(context.contains("Check existing names and descriptions"));
    assert!(context.contains("Verify every named file, function or flag"));
    assert!(!f.dir.path().join("memory").exists());
}

#[tokio::test]
async fn disabled_or_denied_memory_is_absent_without_storage_admission() {
    let f = Fixture::new();
    for config in [
        json!({"memory":{"enabled":false}}),
        json!({"permissions":{"memory":"deny"}}),
        json!({"agents":{"build":{"tools":{"deny":["memory"]}}}}),
    ] {
        f.set_config(config);
        assert_eq!(observe(&f).await, ContextObservation::Absent);
        assert!(!f.dir.path().join("memory").exists());
    }
    f.set_config(json!({}));
    f.env.set("CYBER_DISABLE_MEMORY", "1");
    assert_eq!(observe(&f).await, ContextObservation::Absent);
}

#[tokio::test]
async fn readonly_guidance_never_instructs_generation_and_invalid_settings_are_unavailable() {
    let f = Fixture::new();
    f.set_config(json!({"memory":{"generate":false}}));
    let context = value(observe(&f).await);
    assert!(context.contains("Memory is read-only"));
    assert!(!context.contains("Save durable user preferences"));
    f.set_config(json!({"memory":{"enabled":"invalid"}}));
    assert!(matches!(
        observe(&f).await,
        ContextObservation::Unavailable(_)
    ));
}

#[cfg(unix)]
#[tokio::test]
async fn only_bounded_index_is_loaded_without_opening_individual_notes() {
    let f = Fixture::new();
    let store = cyber_core::memory::MemoryStore::open(f.dir.path(), "global").unwrap();
    store
        .claim()
        .unwrap()
        .write(&note("PRIVATE NOTE BODY MUST NOT ENTER BASELINE"))
        .unwrap();
    let index = (0..340)
        .map(|n| format!("Row {n:03}\n"))
        .collect::<String>();
    std::fs::write(store.path().join("MEMORY.md"), index).unwrap();
    std::os::unix::fs::symlink("/does-not-exist", store.path().join("unreadable-note.md")).unwrap();
    let context = value(observe(&f).await);
    assert_eq!(context.matches("Memory index (scope=global)").count(), 1);
    assert!(context.contains("Row 199"));
    assert!(!context.contains("Row 200"));
    assert!(context.contains("[memory index truncated; read MEMORY.md for more]"));
    assert!(!context.contains("PRIVATE NOTE BODY"));
}

#[cfg(unix)]
#[tokio::test]
async fn denied_global_scope_cannot_be_loaded_through_project_fallback() {
    let f = Fixture::new();
    let store = cyber_core::memory::MemoryStore::open(f.dir.path(), "global").unwrap();
    store.claim().unwrap().write(&note("fact")).unwrap();
    f.set_config(json!({"permissions":{"memory":{"project":"allow","global":"deny"}}}));
    assert_eq!(observe(&f).await, ContextObservation::Absent);
}

#[cfg(unix)]
#[tokio::test]
async fn busy_and_pending_scopes_are_unavailable_instead_of_withdrawn() {
    let f = Fixture::new();
    let store = cyber_core::memory::MemoryStore::open(f.dir.path(), "global").unwrap();
    let mut owner = store.claim().unwrap();
    assert!(matches!(
        observe(&f).await,
        ContextObservation::Unavailable(_)
    ));
    drop(owner.prepare_write(&note("fact")).unwrap());
    drop(owner);
    assert!(matches!(
        observe(&f).await,
        ContextObservation::Unavailable(_)
    ));
    assert!(store.path().join(".memory-transaction").exists());
}

#[cfg(unix)]
#[tokio::test]
async fn another_session_receives_index_change_without_rewriting_its_baseline() {
    let flow = Flow::new(vec![text("a"), text("b"), text("b updated")], false);
    let a = flow.session("default").await;
    let b = flow.session("default").await;
    flow.prompt(&a, "first").await;
    flow.settle(&a).await;
    flow.prompt(&b, "first").await;
    flow.settle(&b).await;
    let mut inv = flow.f.invocation(
        "default",
        "memory",
        json!({"operation":"write","scope":"global","content":note("DO NOT LOAD THIS BODY")}),
    );
    inv.session_id = a.clone();
    ok(flow
        .f
        .host
        .execute(inv, tokio_util::sync::CancellationToken::new())
        .await);
    flow.prompt(&b, "next").await;
    flow.settle(&b).await;
    let requests = flow.main.requests();
    assert_eq!(requests.len(), 3);
    assert_eq!(requests[2].system, requests[1].system);
    let messages = serde_json::to_string(&requests[2].messages).unwrap();
    assert!(messages.contains("core/memory context changed"));
    assert!(messages.contains("coding-policy.md"));
    assert!(!messages.contains("DO NOT LOAD THIS BODY"));
    let events = flow.f.store.read_events(&b, -1, 500).unwrap().events;
    let replay = cyber_server::runtime::SessionState::replay(&events).unwrap();
    assert!(replay.epoch.unwrap().snapshot["core/memory"].contains("coding-policy.md"));
}

#[tokio::test]
async fn disabling_memory_withdraws_context_at_next_boundary() {
    let flow = Flow::new(vec![text("first"), text("second")], false);
    let id = flow.session("default").await;
    flow.prompt(&id, "one").await;
    flow.settle(&id).await;
    assert!(
        flow.main.requests()[0]
            .system
            .join("\n")
            .contains("<memory>")
    );
    flow.f.set_config(json!({"memory":{"enabled":false}}));
    flow.prompt(&id, "two").await;
    flow.settle(&id).await;
    let requests = flow.main.requests();
    assert_eq!(requests[1].system, requests[0].system);
    assert!(
        serde_json::to_string(&requests[1].messages)
            .unwrap()
            .contains("core/memory context no longer applies")
    );
    assert!(
        !flow
            .runtime
            .state(&id)
            .await
            .unwrap()
            .epoch
            .unwrap()
            .snapshot
            .contains_key("core/memory")
    );
}

#[cfg(unix)]
#[tokio::test]
async fn unavailable_initial_index_keeps_input_retryable_until_explicit_recovery() {
    let flow = Flow::new(vec![text("recovered")], false);
    let store = cyber_core::memory::MemoryStore::open(flow.f.dir.path(), "global").unwrap();
    {
        let mut owner = store.claim().unwrap();
        drop(owner.prepare_write(&note("fact")).unwrap());
    }
    let id = flow.session("default").await;
    flow.prompt(&id, "keep this prompt").await;
    flow.settle(&id).await;
    assert!(flow.main.requests().is_empty());
    let state = flow.runtime.state(&id).await.unwrap();
    assert!(state.epoch.is_none());
    assert_eq!(
        state.inbox[0].status,
        cyber_server::runtime::InputStatus::Pending
    );
    store.claim().unwrap().recover().unwrap();
    flow.runtime.wake(&id).await.unwrap();
    flow.settle(&id).await;
    assert!(
        flow.main.requests()[0]
            .system
            .join("\n")
            .contains("coding-policy.md")
    );
}

#[cfg(unix)]
#[tokio::test]
async fn child_context_cannot_widen_parent_memory_denials() {
    use cyber_server::runtime::CreateSession;
    let flow = Flow::new(vec![text("child")], false);
    let store = cyber_core::memory::MemoryStore::open(flow.f.dir.path(), "global").unwrap();
    store.claim().unwrap().write(&note("fact")).unwrap();
    let parent = flow
        .runtime
        .create_session(CreateSession {
            directory: flow.f.repo.display().to_string(),
            model: "test/main".into(),
            rules: Some(json!({"memory":"deny"})),
            ..Default::default()
        })
        .await
        .unwrap()
        .id;
    let child = flow
        .runtime
        .create_session(CreateSession {
            directory: flow.f.repo.display().to_string(),
            model: "test/main".into(),
            parent_id: Some(parent),
            agent: Some("general".into()),
            mode: Some("bypass".into()),
            rules: Some(json!({"memory":"allow"})),
            ..Default::default()
        })
        .await
        .unwrap()
        .id;
    flow.prompt(&child, "hello").await;
    flow.settle(&child).await;
    assert!(
        !flow
            .runtime
            .state(&child)
            .await
            .unwrap()
            .epoch
            .unwrap()
            .snapshot
            .contains_key("core/memory")
    );
    assert!(
        !flow.main.requests()[0]
            .system
            .join("\n")
            .contains("coding-policy.md")
    );
}

#[cfg(unix)]
#[tokio::test]
async fn unavailable_existing_index_retains_previous_context_until_recovered() {
    let flow = Flow::new(
        vec![text("first"), text("retained"), text("updated")],
        false,
    );
    let store = cyber_core::memory::MemoryStore::open(flow.f.dir.path(), "global").unwrap();
    store.claim().unwrap().write(&note("fact")).unwrap();
    let id = flow.session("default").await;
    flow.prompt(&id, "one").await;
    flow.settle(&id).await;
    let original = flow
        .runtime
        .state(&id)
        .await
        .unwrap()
        .epoch
        .unwrap()
        .snapshot["core/memory"]
        .clone();
    {
        let mut owner = store.claim().unwrap();
        drop(
            owner
                .prepare_write(&note("new fact").replace("Small patches", "Updated policy"))
                .unwrap(),
        );
    }
    flow.prompt(&id, "two").await;
    flow.settle(&id).await;
    assert_eq!(
        flow.runtime
            .state(&id)
            .await
            .unwrap()
            .epoch
            .unwrap()
            .snapshot["core/memory"],
        original
    );
    let requests = flow.main.requests();
    assert_eq!(requests[1].system, requests[0].system);
    assert!(
        !serde_json::to_string(&requests[1].messages)
            .unwrap()
            .contains("core/memory context no longer applies")
    );
    store.claim().unwrap().recover().unwrap();
    flow.prompt(&id, "three").await;
    flow.settle(&id).await;
    let requests = flow.main.requests();
    assert_eq!(requests[2].system, requests[0].system);
    assert!(
        serde_json::to_string(&requests[2].messages)
            .unwrap()
            .contains("Updated policy")
    );
}

#[cfg(unix)]
#[tokio::test]
async fn repository_baseline_combines_project_and_global_indexes_with_scope_labels() {
    let f = Fixture::new();
    assert!(
        std::process::Command::new("git")
            .args(["init", "--quiet"])
            .current_dir(&f.repo)
            .status()
            .unwrap()
            .success()
    );
    let project = cyber_core::project::identify(&f.repo).id;
    for (id, description) in [
        (project.as_str(), "Project policy"),
        ("global", "Global policy"),
    ] {
        let store = cyber_core::memory::MemoryStore::open(f.dir.path(), id).unwrap();
        store
            .claim()
            .unwrap()
            .write(&note("do not preload this body").replace("Small patches", description))
            .unwrap();
    }
    let context = value(observe(&f).await);
    assert!(context.contains("Memory index (scope=project)"));
    assert!(context.contains("Memory index (scope=global)"));
    assert!(context.contains("Project policy"));
    assert!(context.contains("Global policy"));
    assert!(!context.contains("do not preload this body"));
}

#[tokio::test]
async fn plan_parent_prevents_generation_guidance_in_child_context() {
    use cyber_server::runtime::CreateSession;
    let flow = Flow::new(vec![text("child")], false);
    let parent = flow
        .runtime
        .create_session(CreateSession {
            directory: flow.f.repo.display().to_string(),
            model: "test/main".into(),
            mode: Some("plan".into()),
            ..Default::default()
        })
        .await
        .unwrap()
        .id;
    let child = flow
        .runtime
        .create_session(CreateSession {
            directory: flow.f.repo.display().to_string(),
            model: "test/main".into(),
            parent_id: Some(parent),
            agent: Some("general".into()),
            mode: Some("bypass".into()),
            ..Default::default()
        })
        .await
        .unwrap()
        .id;
    flow.prompt(&child, "hello").await;
    flow.settle(&child).await;
    let source = flow
        .runtime
        .state(&child)
        .await
        .unwrap()
        .epoch
        .unwrap()
        .snapshot["core/memory"]
        .clone();
    assert!(source.contains("Memory is read-only"));
    assert!(!source.contains("Save durable user preferences"));
}
