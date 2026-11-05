//! The reply language through the real tool host and runtime (`localization` → Reply
//! language): a configured `language` reaches the model as the `core/language` source,
//! and returning to `auto` withdraws it.

mod support;

use std::collections::HashMap;
use std::sync::Arc;

use cyber_core::config::{self, LoadRequest};
use cyber_core::paths::Paths;
use cyber_core::trust::TrustStore;
use cyber_server::runtime::CreateSession;
use support::flow::{Flow, text};

/// Resolve every model through the real tool host's configuration loader.
fn attach_config(flow: &Flow) {
    let root = flow.f.dir.path().to_path_buf();
    let env = HashMap::from([(
        "CYBER_HOME".into(),
        root.join("cyber").display().to_string(),
    )]);
    let paths = Paths::resolve(&env, &root);
    paths.ensure().unwrap();
    let trust = TrustStore::new(paths.trust_file());
    flow.f
        .host
        .attach_hook_config(
            Arc::new(move |location| {
                config::load(&LoadRequest {
                    location,
                    paths: &paths,
                    env: &env,
                    home: &root,
                    profile: None,
                    overrides: &[],
                    flags: serde_json::json!({}),
                })
                .map_err(|error| error.to_string())
            }),
            trust,
        )
        .unwrap();
}

#[tokio::test]
async fn a_configured_language_asks_the_model_to_reply_in_it() {
    let flow = Flow::new(vec![text("olá"), text("hi")], false);
    attach_config(&flow);
    std::fs::write(
        flow.f.repo.join("cyber.jsonc"),
        r#"{ "language": "pt-BR" }"#,
    )
    .unwrap();
    let request = CreateSession {
        directory: flow.f.repo.display().to_string(),
        model: "test/main".into(),
        mode: Some("default".into()),
        ..Default::default()
    };
    let id = flow.runtime.create_session(request).await.unwrap().id;
    flow.prompt(&id, "one").await;
    flow.settle(&id).await;
    let epoch = flow.runtime.state(&id).await.unwrap().epoch.unwrap();
    assert!(
        epoch.snapshot["core/language"].contains("Brazilian Portuguese (pt-BR)"),
        "{:?}",
        epoch.snapshot
    );
    let first = flow.main.requests()[0].system.concat();
    assert!(first.contains("in Brazilian Portuguese (pt-BR)"), "{first}");

    std::fs::write(flow.f.repo.join("cyber.jsonc"), r#"{ "language": "auto" }"#).unwrap();
    flow.prompt(&id, "two").await;
    flow.settle(&id).await;
    let epoch = flow.runtime.state(&id).await.unwrap().epoch.unwrap();
    assert!(!epoch.snapshot.contains_key("core/language"));
    let requests = flow.main.requests();
    let second = format!("{:?}", requests[1]);
    assert!(
        second.contains("The core/language context no longer applies."),
        "{second}"
    );
}
