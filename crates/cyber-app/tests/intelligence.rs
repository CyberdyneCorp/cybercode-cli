//! Actual authenticated TCP formatter discovery through application services.
use cyber_app::{App, AppOptions};
use cyber_core::paths::{DatabaseLocation, Paths};
use serde_json::{Value, json};
use std::path::PathBuf;

struct Fixture {
    _temp: tempfile::TempDir,
    root: PathBuf,
    app: App,
    url: String,
    client: reqwest::Client,
    stop: tokio::sync::oneshot::Sender<()>,
    server: tokio::task::JoinHandle<()>,
}
impl Fixture {
    async fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let paths = Paths {
            data: root.join("data"),
            config: root.join("config"),
            state: root.join("state"),
            cache: root.join("cache"),
            tmp: root.join("tmp"),
        };
        paths.ensure().unwrap();
        std::fs::create_dir_all(paths.cache.join("bin")).unwrap();
        let candidate = paths.cache.join("bin/prettier");
        std::fs::write(&candidate, "Do not execute discovery candidates").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&candidate, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        let app = App::build(AppOptions {
            paths,
            home: root.join("home"),
            database: DatabaseLocation::Memory,
            default_directory: root.clone(),
            sandbox_policy: None,
            snapshots: false,
            interactive: true,
            password: Some("status-password".into()),
        })
        .await
        .unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!(
            "http://{}/api/v1/formatters",
            listener.local_addr().unwrap()
        );
        let (stop, stopped) = tokio::sync::oneshot::channel();
        let router = cyber_server::http::router(app.state.clone());
        let server = tokio::spawn(async move {
            cyber_server::http::serve_tcp(router, listener, async {
                let _ = stopped.await;
            })
            .await
            .unwrap();
        });
        Self {
            _temp: temp,
            root,
            app,
            url,
            client: reqwest::Client::builder().no_proxy().build().unwrap(),
            stop,
            server,
        }
    }
    fn get(&self) -> reqwest::RequestBuilder {
        self.client
            .get(&self.url)
            .basic_auth("cyber", Some("status-password"))
    }
    fn settings(&self, value: Value) {
        std::fs::write(self.root.join("config/cyber.jsonc"), value.to_string()).unwrap();
    }
    async fn close(self) {
        self.stop.send(()).unwrap();
        self.server.await.unwrap();
        self.app.runtime.shutdown().await;
    }
}
fn row<'a>(body: &'a Value, id: &str) -> &'a Value {
    body["data"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["id"] == id)
        .unwrap()
}

#[tokio::test]
async fn formatter_status_authenticates_and_returns_location_without_execution() {
    let f = Fixture::new().await;
    assert_eq!(
        f.client.get(&f.url).send().await.unwrap().status(),
        reqwest::StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        f.client
            .get(&f.url)
            .basic_auth("cyber", Some("wrong"))
            .send()
            .await
            .unwrap()
            .status(),
        reqwest::StatusCode::UNAUTHORIZED
    );
    std::fs::write(
        f.root.join("prettier.config.js"),
        "throw new Error('do not evaluate');",
    )
    .unwrap();
    let response = f.get().send().await.unwrap();
    assert!(response.status().is_success());
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["location"]["directory"], f.root.display().to_string());
    assert_eq!(body["data"].as_array().unwrap().len(), 12);
    assert_eq!(row(&body, "prettier")["enabled"], true);
    assert_eq!(row(&body, "prettier")["detected_by"], "prettier.config.js");
    assert!(
        body["data"]
            .as_array()
            .unwrap()
            .iter()
            .all(|r| r.get("command").is_none() && r.get("env").is_none())
    );
    f.close().await;
}

#[tokio::test]
async fn formatter_status_query_wins_and_project_markers_are_location_scoped() {
    let f = Fixture::new().await;
    let first = f.root.join("first");
    let second = f.root.join("second");
    std::fs::create_dir(&first).unwrap();
    std::fs::create_dir(&second).unwrap();
    std::fs::write(first.join(".prettierrc"), "{}").unwrap();
    let body: Value = f
        .get()
        .header("x-cyber-directory", second.to_str().unwrap())
        .query(&[("location[directory]", first.to_str().unwrap())])
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(body["location"]["directory"], first.display().to_string());
    assert_eq!(row(&body, "prettier")["enabled"], true);
    let body: Value = f
        .get()
        .query(&[("location[directory]", second.to_str().unwrap())])
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(body["location"]["directory"], second.display().to_string());
    assert_eq!(row(&body, "prettier")["enabled"], false);
    let response = f
        .get()
        .header(
            "x-cyber-directory",
            f.root.join("missing").to_str().unwrap(),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);
    let response = f
        .get()
        .query(&[(
            "location[directory]",
            first.join(".prettierrc").to_str().unwrap(),
        )])
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);
    f.close().await;
}

#[tokio::test]
async fn formatter_status_reloads_disabled_and_forced_settings_without_secret_fields() {
    let f = Fixture::new().await;
    f.settings(json!({"formatters":{"taplo":{"command":["uninstalled-status-taplo","fmt","$FILE"],"extensions":[".toml"],"env":{"SECRET":"integration-private-value"}}}}));
    let body: Value = f.get().send().await.unwrap().json().await.unwrap();
    assert_eq!(row(&body, "taplo")["enabled"], true);
    assert_eq!(row(&body, "taplo")["installed"], false);
    assert_eq!(row(&body, "taplo")["detected_by"], "config");
    assert!(!body.to_string().contains("integration-private-value"));
    assert!(!body.to_string().contains("uninstalled-status-taplo"));
    f.settings(json!({"formatters":false}));
    let body: Value = f.get().send().await.unwrap().json().await.unwrap();
    assert_eq!(body["data"].as_array().unwrap().len(), 12);
    assert!(
        body["data"]
            .as_array()
            .unwrap()
            .iter()
            .all(|r| r["enabled"] == false)
    );
    f.close().await;
}

#[tokio::test]
async fn formatter_status_withholds_project_commands_and_returns_safe_configuration_errors() {
    let f = Fixture::new().await;
    std::fs::create_dir(f.root.join(".git")).unwrap();
    std::fs::write(
        f.root.join("cyber.jsonc"),
        r#"{"formatters":{"untrusted":{"command":["do-not-run"],"extensions":[".x"]}}}"#,
    )
    .unwrap();
    let body: Value = f.get().send().await.unwrap().json().await.unwrap();
    assert!(
        body["data"]
            .as_array()
            .unwrap()
            .iter()
            .all(|r| r["id"] != "untrusted")
    );
    f.settings(json!({"formatters":{"invalid":{"command":"integration-private-value"}}}));
    let response = f.get().send().await.unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["_tag"], "InvalidRequestError");
    assert!(!body.to_string().contains("integration-private-value"));
    f.close().await;
}
