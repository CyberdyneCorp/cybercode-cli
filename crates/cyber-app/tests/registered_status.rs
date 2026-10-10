//! Existing-server observation bounds and generation refusal without an application runtime.
use cyber_app::registered_lsp_status;
use cyber_core::paths::Paths;
use serde_json::json;

fn paths(root: &std::path::Path) -> Paths {
    Paths {
        data: root.join("data"),
        config: root.join("config"),
        state: root.join("state"),
        cache: root.join("cache"),
        tmp: root.join("tmp"),
    }
}
fn register(paths: &Paths, url: &str, id: &str) {
    std::fs::create_dir_all(&paths.state).unwrap();
    std::fs::write(
        cyber_app::registration_path(paths),
        json!({
            "id":id,"version":"test","url":url,"socket":null,"pid":std::process::id()
        })
        .to_string(),
    )
    .unwrap();
}

#[tokio::test]
async fn missing_invalid_credentials_and_listener_do_not_create_runtime_storage() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let paths = paths(&root);
    assert!(
        registered_lsp_status(&paths, &root)
            .await
            .unwrap()
            .is_none()
    );
    register(&paths, "http://127.0.0.1:1", "srv_one");
    assert!(
        registered_lsp_status(&paths, &root)
            .await
            .unwrap_err()
            .contains("credentials")
    );
    std::fs::write(paths.state.join("password"), " \n").unwrap();
    assert!(
        registered_lsp_status(&paths, &root)
            .await
            .unwrap_err()
            .contains("empty")
    );
    std::fs::write(paths.state.join("password"), "private-password").unwrap();
    for url in [
        "file:///private",
        "http://user:private-password@localhost",
        "http://localhost/path",
        "http://localhost/?secret=private-password",
        "http://localhost/#private-password",
    ] {
        register(&paths, url, "srv_one");
        let error = registered_lsp_status(&paths, &root).await.unwrap_err();
        assert!(error.contains("listener is invalid"));
        assert!(!error.contains("private-password"));
    }
    std::fs::write(cyber_app::registration_path(&paths), "invalid").unwrap();
    assert!(
        registered_lsp_status(&paths, &root)
            .await
            .unwrap_err()
            .contains("registration is invalid")
    );
    assert!(!paths.data.exists());
    assert!(!paths.tmp.exists());
}

#[tokio::test]
async fn oversized_status_is_refused_without_echoing_body() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let paths = paths(&root);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    register(
        &paths,
        &format!("http://{}", listener.local_addr().unwrap()),
        "srv_one",
    );
    std::fs::write(paths.state.join("password"), "private-password").unwrap();
    let router = axum::Router::new().route(
        "/api/v1/lsp",
        axum::routing::get(|| async { "private-response".repeat(100_000) }),
    );
    let server = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    let result = registered_lsp_status(&paths, &root).await;
    server.abort();
    let _ = server.await;
    let error = result.unwrap_err();
    assert!(error.contains("response budget"));
    assert!(!error.contains("private-response"));
    assert!(!paths.data.exists());
}

#[tokio::test]
async fn registration_replacement_during_observation_refuses_old_generation() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let paths = paths(&root);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    register(&paths, &url, "srv_old");
    std::fs::write(paths.state.join("password"), "private-password").unwrap();
    let changed_paths = paths.clone();
    let location = root.clone();
    let router = axum::Router::new().route(
        "/api/v1/lsp",
        axum::routing::get(move || {
            let changed_paths = changed_paths.clone();
            let url = url.clone();
            let location = location.clone();
            async move {
                register(&changed_paths, &url, "srv_new");
                axum::Json(json!({"location":{"directory":location},"data":[]}))
            }
        }),
    );
    let server = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    let result = registered_lsp_status(&paths, &root).await;
    server.abort();
    let _ = server.await;
    assert!(result.unwrap_err().contains("registration changed"));
    assert!(!paths.data.exists());
}
