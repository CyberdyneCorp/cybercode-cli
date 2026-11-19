//! Planner and executor models (`provider-catalog` → Planner and executor models):
//! plan-mode Turns of a top-level Session run on `model_roles.plan`, other Turns on the
//! Session's own model.
mod support;

use cyber_server::runtime::*;
use support::*;

async fn session_in(h: &Harness, mode: &str, parent: Option<String>) -> String {
    let req = CreateSession {
        directory: h.repo.display().to_string(),
        model: "test/main".into(),
        mode: Some(mode.into()),
        parent_id: parent,
        ..Default::default()
    };
    h.runtime.create_session(req).await.unwrap().id
}

async fn ask(h: &Harness, id: &str, text: &str) {
    h.runtime
        .admit(id, Admission::text(text, Delivery::Queue))
        .await
        .unwrap();
    h.settle(id).await;
}

/// `(model, model_role)` of every step, in order.
fn steps(h: &Harness, id: &str) -> Vec<(String, Option<String>)> {
    h.store
        .read_events(id, -1, 500)
        .unwrap()
        .events
        .into_iter()
        .filter(|row| row.kind == "session.step.started.1")
        .map(|row| {
            (
                format!(
                    "{}/{}",
                    row.data["provider"].as_str().unwrap(),
                    row.data["model"].as_str().unwrap()
                ),
                row.data["model_role"].as_str().map(str::to_owned),
            )
        })
        .collect()
}

fn planner_setup(scripts: Vec<(&'static str, Vec<Vec<cyber_llm::adapters::ScriptStep>>)>) -> Setup {
    Setup {
        scripts,
        roles: vec![("plan", "other/main")],
        ..Setup::default()
    }
}

#[tokio::test]
async fn plan_mode_runs_on_the_planner_and_leaving_it_returns_to_the_session_model() {
    let h = Harness::new(planner_setup(vec![
        (
            "other/main",
            vec![reasoning_then_text(
                "weighing the private options",
                "1. migrate 2. backfill",
            )],
        ),
        ("test/main", vec![text("built")]),
    ]));
    let id = session_in(&h, "plan", None).await;
    ask(&h, &id, "plan the migration").await;
    let state = h.state(&id).await;
    assert_eq!(state.info.model, "other/main");
    assert_eq!(h.runtime.model_role(&state), Some(PLAN_ROLE));

    h.runtime.switch_mode(&id, "default").await.unwrap();
    ask(&h, &id, "go").await;
    let state = h.state(&id).await;
    assert_eq!(state.info.model, "test/main");
    assert_eq!(h.runtime.model_role(&state), None);
    assert_eq!(
        steps(&h, &id),
        vec![
            ("other/main".into(), Some("plan".into())),
            ("test/main".into(), None)
        ]
    );
    // The executor sees the planner's answer, never its reasoning or signature.
    let executor = transcript(&h.models.requests("test/main")[0]);
    assert!(executor.contains("1. migrate 2. backfill"), "{executor}");
    assert!(
        !executor.contains("weighing the private options"),
        "{executor}"
    );
    assert!(!executor.contains("sig"), "{executor}");
    // Both switches are automatic: the Session's own model never changed.
    let switches: Vec<_> = h
        .store
        .read_events(&id, -1, 500)
        .unwrap()
        .events
        .into_iter()
        .filter(|row| row.kind == "session.model.switched.1")
        .map(|row| {
            (
                row.data["to"].as_str().unwrap().to_owned(),
                row.data["automatic"] == true,
            )
        })
        .collect();
    assert_eq!(
        switches,
        vec![("other/main".into(), true), ("test/main".into(), true)]
    );
}

#[tokio::test]
async fn without_a_plan_role_plan_mode_keeps_the_session_model() {
    let h = Harness::new(Setup {
        scripts: vec![(
            "test/main",
            vec![reasoning_then_text("thinking aloud", "plan"), text("built")],
        )],
        ..Setup::default()
    });
    let id = session_in(&h, "plan", None).await;
    ask(&h, &id, "plan it").await;
    h.runtime.switch_model(&id, "other/main").await.unwrap();
    h.runtime.switch_mode(&id, "default").await.unwrap();
    let state = h.state(&id).await;
    assert_eq!(h.runtime.model_role(&state), None);
    assert_eq!(steps(&h, &id), vec![("test/main".into(), None)]);
    let rows = h.store.read_events(&id, -1, 500).unwrap().events;
    let replayed = SessionState::replay(&rows).unwrap();
    assert_eq!(replayed.info.model, "other/main");
}

/// Without a planner, reasoning another model produced is still lowered to text.
#[tokio::test]
async fn without_a_plan_role_reasoning_is_lowered_as_before() {
    let h = Harness::new(Setup {
        scripts: vec![
            (
                "test/main",
                vec![reasoning_then_text("thinking aloud", "plan")],
            ),
            ("other/main", vec![text("built")]),
        ],
        ..Setup::default()
    });
    let id = session_in(&h, "plan", None).await;
    ask(&h, &id, "plan it").await;
    h.runtime.switch_model(&id, "other/main").await.unwrap();
    ask(&h, &id, "go").await;
    let sent = transcript(&h.models.requests("other/main")[0]);
    assert!(sent.contains("thinking aloud"), "{sent}");
}

#[tokio::test]
async fn a_model_switch_overrides_the_planner_until_the_mode_changes() {
    let h = Harness::new(planner_setup(vec![
        ("other/main", vec![text("first plan"), text("second plan")]),
        ("test/main", vec![text("replanned")]),
    ]));
    let id = session_in(&h, "plan", None).await;
    ask(&h, &id, "plan").await;
    // Switching to the Session's own model is not a no-op while the planner runs.
    h.runtime.switch_model(&id, "test/main").await.unwrap();
    ask(&h, &id, "plan on the session model").await;
    assert_eq!(h.state(&id).await.info.model, "test/main");
    assert_eq!(h.runtime.model_role(&h.state(&id).await), None);

    h.runtime.switch_mode(&id, "default").await.unwrap();
    h.runtime.switch_mode(&id, "plan").await.unwrap();
    ask(&h, &id, "plan again").await;
    assert_eq!(
        steps(&h, &id),
        vec![
            ("other/main".into(), Some("plan".into())),
            ("test/main".into(), None),
            ("other/main".into(), Some("plan".into())),
        ]
    );
    // The suspension is folded from durable events.
    let rows = h.store.read_events(&id, -1, 500).unwrap().events;
    let replayed = SessionState::replay(&rows).unwrap();
    assert_eq!(h.runtime.model_role(&replayed), Some(PLAN_ROLE));
}

#[tokio::test]
async fn sessions_with_a_parent_keep_their_own_model() {
    let h = Harness::new(planner_setup(vec![("test/main", vec![text("child plan")])]));
    let parent = h.session().await;
    let child = session_in(&h, "plan", Some(parent)).await;
    ask(&h, &child, "plan").await;
    assert_eq!(steps(&h, &child), vec![("test/main".into(), None)]);
    assert!(h.models.requests("other/main").is_empty());
}
