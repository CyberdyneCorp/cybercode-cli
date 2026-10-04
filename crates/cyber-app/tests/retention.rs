//! Retention sweep: old tool output and old archived Sessions go, the rest stays.

use std::time::{Duration, SystemTime};

use cyber_app::retention::{self, Retention};
use cyber_app::{App, AppOptions};
use cyber_core::paths::{DatabaseLocation, Paths};
use cyber_server::runtime::CreateSession;

const DAY: Duration = Duration::from_secs(86_400);

#[tokio::test]
async fn sweeps_remove_only_expired_items() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let paths = Paths {
        data: root.join("data"),
        config: root.join("config"),
        state: root.join("state"),
        cache: root.join("cache"),
        tmp: root.join("tmp"),
    };
    paths.ensure().unwrap();
    let app = App::build(AppOptions {
        paths: paths.clone(),
        home: root.join("home"),
        database: DatabaseLocation::Memory,
        default_directory: root.to_path_buf(),
        sandbox_policy: None,
        snapshots: false,
        interactive: false,
        password: None,
    })
    .await
    .unwrap();
    let out = paths.data.join("tool-output");
    std::fs::write(out.join("tool_old"), "x").unwrap();
    std::fs::write(out.join("tool_new"), "x").unwrap();
    let now = SystemTime::now();
    std::fs::File::options()
        .write(true)
        .open(out.join("tool_old"))
        .unwrap()
        .set_modified(now - 8 * DAY)
        .unwrap();

    let make = |title: &str| CreateSession {
        directory: root.display().to_string(),
        model: "openai/gpt-6-luna".into(),
        title: Some(title.into()),
        ..Default::default()
    };
    let archived = app
        .runtime
        .create_session(make("archived"))
        .await
        .unwrap()
        .id;
    let active = app.runtime.create_session(make("active")).await.unwrap().id;
    app.runtime.archive(&archived, true).await.unwrap();

    let policy = Retention {
        tool_output_days: 7,
        archived_days: 90,
    };
    let soon = retention::sweep(&app.runtime, &paths.data, policy, now).await;
    assert_eq!((soon.tool_output_files, soon.archived_sessions), (1, 0));
    assert!(out.join("tool_new").exists() && !out.join("tool_old").exists());

    let disabled = Retention {
        tool_output_days: 0,
        archived_days: 0,
    };
    assert_eq!(
        retention::sweep(&app.runtime, &paths.data, disabled, now + 365 * DAY)
            .await
            .archived_sessions,
        0
    );

    let later = retention::sweep(&app.runtime, &paths.data, policy, now + 91 * DAY).await;
    assert_eq!(later.archived_sessions, 1);
    assert!(app.runtime.state(&archived).await.is_err());
    assert!(
        app.runtime.state(&active).await.is_ok(),
        "unarchived Sessions are kept"
    );
}

#[test]
fn retention_reads_config_with_defaults() {
    let r = Retention::from_config(
        &serde_json::json!({ "storage": { "retention": { "archived_days": 0 } } }),
    );
    assert_eq!(
        r,
        Retention {
            tool_output_days: 7,
            archived_days: 0
        }
    );
}
