//! `tool-registry` → Tool output redaction at settlement: a host without configuration
//! still hides credentials, and the model's history equals the stored settlement.
mod support;
use cyber_server::runtime::*;
use support::{Behavior, Harness, Setup, text, tools, transcript};

#[tokio::test]
async fn settled_output_is_redacted_for_the_model_and_the_stored_call() {
    let h = Harness::new(Setup {
        scripts: vec![(
            "test/main",
            vec![tools(&[("c1", "read", "{}")]), text("done")],
        )],
        ..Setup::default()
    });
    h.tools.set(
        "read",
        Behavior::Return("GITHUB_TOKEN=ghp_abcdefgh12345678\nDB_PASSWORD=Hunter2Hunter2".into()),
    );
    let id = h.session().await;
    h.runtime
        .admit(&id, Admission::text("read it", Delivery::Queue))
        .await
        .unwrap();
    h.settle(&id).await;
    let requests = h.models.requests("test/main");
    let seen = transcript(&requests[1]);
    assert!(!seen.contains("ghp_abcdefgh12345678") && !seen.contains("Hunter2Hunter2"));
    let expected = "GITHUB_TOKEN=[REDACTED:github_token]\nDB_PASSWORD=[REDACTED:assignment]";
    let state = h.runtime.state(&id).await.unwrap();
    assert_eq!(state.calls["c1"].output.as_deref(), Some(expected));
    let redactions = state.calls["c1"].redactions.clone().unwrap();
    assert_eq!(redactions.count, 2);
    assert_eq!(redactions.kinds, ["assignment", "github_token"]);
    for line in expected.lines() {
        assert!(seen.contains(line), "{seen}");
    }
}
