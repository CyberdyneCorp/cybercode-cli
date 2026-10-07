//! Forks preserve durable context with fresh identities and independent billing.
mod support;
use cyber_server::runtime::{Admission, CreateSession, Delivery};
use support::{Harness, Setup, text};

#[tokio::test]
async fn a_child_fork_is_independent_and_preserves_epoch_and_task_on_restart() {
    let h = Harness::new(Setup {
        scripts: vec![("test/main", vec![text("original answer")])],
        ..Default::default()
    });
    let parent = h.session().await;
    let child = h
        .runtime
        .create_session(CreateSession {
            directory: h.repo.display().to_string(),
            model: "test/main".into(),
            parent_id: Some(parent),
            agent: Some("general".into()),
            subagent_name: Some("inspection".into()),
            ..Default::default()
        })
        .await
        .unwrap();
    h.runtime
        .admit(
            &child.id,
            Admission::text("original objective", Delivery::Queue),
        )
        .await
        .unwrap();
    h.runtime.wait_idle(&child.id).await;
    let source = h.state(&child.id).await;
    let fork = h.runtime.fork(&child.id, None).await.unwrap();
    assert!(fork.parent_id.is_none());
    assert!(fork.subagent_name.is_none());
    let copied = h.state(&fork.id).await;
    assert_eq!(copied.epoch, source.epoch);
    assert_eq!(copied.info.agent, source.info.agent);
    assert_eq!(copied.info.model, source.info.model);
    assert_eq!(copied.task.instructions.len(), 1);
    assert_eq!(
        copied.task.objective.as_ref().unwrap().message_id,
        copied.entries[0].id()
    );
    assert_ne!(copied.entries[0].id(), source.entries[0].id());
    assert_eq!(copied.totals.cost, 0.0);
    assert_eq!(copied.totals.steps, 0);
    let restored = h.restart().state(&fork.id).await.unwrap();
    assert_eq!(restored.epoch, copied.epoch);
    assert_eq!(restored.task, copied.task);
}

#[tokio::test]
async fn a_fork_cut_does_not_copy_later_task_instructions() {
    let h = Harness::new(Setup {
        scripts: vec![("test/main", vec![text("first"), text("second")])],
        ..Default::default()
    });
    let id = h.session().await;
    h.runtime
        .admit(&id, Admission::text("first objective", Delivery::Queue))
        .await
        .unwrap();
    h.runtime.wait_idle(&id).await;
    let second = h
        .runtime
        .admit(&id, Admission::text("later instruction", Delivery::Queue))
        .await
        .unwrap();
    h.runtime.wait_idle(&id).await;
    let fork = h.runtime.fork(&id, Some(&second.message_id)).await.unwrap();
    let copied = h.state(&fork.id).await;
    assert_eq!(copied.task.instructions.len(), 1);
    assert_eq!(copied.task.instructions[0].text, "first objective");
    assert_eq!(
        copied.task.instructions[0].message_id,
        copied.entries[0].id()
    );
    assert_eq!(copied.entries.len(), 2);
    assert!(copied.epoch.is_some());
}

#[tokio::test]
async fn copying_an_active_tool_does_not_redispatch_or_cancel_the_source() {
    use support::{Behavior, tools};
    let h = Harness::new(Setup {
        scripts: vec![(
            "test/main",
            vec![
                tools(&[("pending", "write", "{}")]),
                text("fork answer"),
                text("source answer"),
            ],
        )],
        ..Default::default()
    });
    h.tools.set("write", Behavior::Gated);
    let id = h.session().await;
    h.runtime
        .admit(&id, Admission::text("original objective", Delivery::Queue))
        .await
        .unwrap();
    h.tools.started.notified().await;
    let source = h.state(&id).await;
    let fork = h.runtime.fork(&id, None).await.unwrap();
    let copied = h.state(&fork.id).await;
    assert_eq!(copied.calls.len(), 1);
    let call = copied.calls.values().next().unwrap();
    assert_ne!(call.call_id, "pending");
    assert!(call.status.is_settled());
    assert!(call.output.as_ref().unwrap().contains("source call"));
    h.runtime
        .admit(
            &fork.id,
            Admission::text("try another approach", Delivery::Queue),
        )
        .await
        .unwrap();
    h.runtime.wait_idle(&fork.id).await;
    assert!(h.runtime.is_running(&id));
    assert_eq!(h.tools.executed.lock().unwrap().len(), 1);
    assert_eq!(h.state(&id).await.calls["pending"], source.calls["pending"]);
    h.tools.release.notify_one();
    h.runtime.wait_idle(&id).await;
}
