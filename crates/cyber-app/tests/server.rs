//! Server lifecycle: password, lock, listeners and registration.

use std::sync::Arc;
use std::time::Duration;

use cyber_app::{App, AppOptions, ServeOptions};
use cyber_core::paths::{DatabaseLocation, Paths};
use tokio::sync::Notify;

fn paths(root: &std::path::Path) -> Paths {
    Paths {
        data: root.join("data"),
        config: root.join("config"),
        state: root.join("state"),
        cache: root.join("cache"),
        tmp: root.join("tmp"),
    }
}

async fn app(root: &std::path::Path) -> App {
    let p = paths(root);
    p.ensure().unwrap();
    App::build(AppOptions {
        paths: p,
        home: root.join("home"),
        database: DatabaseLocation::Memory,
        default_directory: root.to_path_buf(),
        sandbox_policy: None,
        snapshots: false,
        interactive: true,
        password: Some("test-password-123456".into()),
    })
    .await
    .unwrap()
}

#[cfg(unix)]
#[test]
fn the_password_is_generated_once_and_private() {
    use std::os::unix::fs::PermissionsExt;
    let tmp = tempfile::tempdir().unwrap();
    let p = paths(tmp.path());
    let first = cyber_app::password(&p).unwrap();
    assert_eq!(first.len(), 43, "32 bytes in base64url");
    assert_eq!(cyber_app::password(&p).unwrap(), first);
    let mode = std::fs::metadata(p.state.join("password"))
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o600);
}

#[tokio::test]
async fn a_registered_server_is_healthy_and_unregisters_on_stop() {
    let tmp = tempfile::tempdir().unwrap();
    let p = paths(tmp.path());
    let stop = Arc::new(Notify::new());
    let opts = ServeOptions {
        port: Some(0),
        socket: Some(tmp.path().join("s.sock")),
        register: true,
        ..ServeOptions::default()
    };
    let server = tokio::spawn(cyber_app::run_server_until(
        app(tmp.path()).await,
        opts,
        |_| {},
        Arc::clone(&stop),
    ));
    let reg = loop {
        if let Some(r) = cyber_app::read_registration(&p) {
            break r;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    assert!(cyber_app::health(&reg.url, cyber_app::version()).await);
    assert!(
        !cyber_app::health(&reg.url, "9.9.9").await,
        "a version mismatch is not healthy"
    );
    assert!(tmp.path().join("s.sock").exists());

    // A second server for the same user is refused by the lock.
    let second = cyber_app::run_server_until(
        app(tmp.path()).await,
        ServeOptions {
            port: Some(0),
            no_tcp: true,
            socket: Some(tmp.path().join("t.sock")),
            ..ServeOptions::default()
        },
        |_| {},
        Arc::new(Notify::new()),
    )
    .await;
    assert!(second.unwrap_err().contains("already running"));

    stop.notify_one();
    tokio::time::timeout(Duration::from_secs(10), server)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(cyber_app::read_registration(&p).is_none());
    assert!(!tmp.path().join("s.sock").exists());
}

/// Regression: `GET /tools` listed only built-in tools, not client-registered ones.
#[tokio::test]
async fn registered_tools_are_listed_with_the_built_ins() {
    let tmp = tempfile::tempdir().unwrap();
    let app = app(tmp.path()).await;
    let (tx, mut rx) = tokio::sync::mpsc::channel(8);
    let session = cyber_server::http::rpc::RpcSession::new(&app.state, tx);
    session.handle(r#"{"jsonrpc":"2.0","id":1,"method":"v1.tool.register","params":{"name":"lookup_ticket","description":"Find a ticket"}}"#).await;
    assert_eq!(
        rx.recv().await.unwrap()["result"]["registered"],
        "lookup_ticket"
    );
    session
        .handle(r#"{"jsonrpc":"2.0","id":2,"method":"v1.tool.list","params":{}}"#)
        .await;
    let listed = rx.recv().await.unwrap();
    let names: Vec<&str> = listed["result"]["data"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|t| t["name"].as_str())
        .collect();
    assert!(
        names.contains(&"lookup_ticket") && names.contains(&"read"),
        "{names:?}"
    );
    drop(session);
    assert!(
        app.state.remote_tools.definitions().is_empty(),
        "registrations end with the channel"
    );
}
