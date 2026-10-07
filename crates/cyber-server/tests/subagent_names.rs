//! Named child creation is projected transactionally and survives runtime reconstruction.
mod support;
use cyber_server::runtime::CreateSession;
use support::{Harness, Setup};

#[tokio::test]
async fn names_survive_restart_and_database_uniqueness_is_parent_scoped() {
    let h = Harness::new(Setup::default());
    let parent = h.session().await;
    let req = CreateSession {
        directory: h.repo.display().to_string(),
        model: "test/main".into(),
        parent_id: Some(parent.clone()),
        subagent_name: Some("inspection".into()),
        title: Some("Inspect project files (@general)".into()),
        ..Default::default()
    };
    let child = h.runtime.create_session(req.clone()).await.unwrap();
    assert!(h.runtime.create_session(req.clone()).await.is_err());
    let restarted = h.restart();
    assert_eq!(
        restarted
            .state(&child.id)
            .await
            .unwrap()
            .info
            .subagent_name
            .as_deref(),
        Some("inspection")
    );
    assert_eq!(restarted.subagent_names(&parent).unwrap(), ["inspection"]);
    let other = h.session().await;
    let second = h
        .runtime
        .create_session(CreateSession {
            parent_id: Some(other.clone()),
            ..req
        })
        .await
        .unwrap();
    assert_ne!(child.id, second.id);
    assert_eq!(restarted.subagent_names(&other).unwrap(), ["inspection"]);
    let fork = restarted.fork(&child.id, None).await.unwrap();
    assert!(fork.subagent_name.is_none());
    assert_eq!(restarted.subagent_names(&parent).unwrap(), ["inspection"]);
}

#[tokio::test]
async fn top_level_sessions_cannot_claim_a_subagent_name() {
    let h = Harness::new(Setup::default());
    let result = h
        .runtime
        .create_session(CreateSession {
            directory: h.repo.display().to_string(),
            model: "test/main".into(),
            subagent_name: Some("inspection".into()),
            ..Default::default()
        })
        .await;
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("requires a parent")
    );
}
