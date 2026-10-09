//! Actual authenticated memory review over the application's shared storage.
use cyber_app::{App, AppOptions};
use cyber_core::paths::{DatabaseLocation, Paths};
use reqwest::{Client, StatusCode};
use serde_json::{Value, json};

struct Fixture {
    root: tempfile::TempDir,
    directory: std::path::PathBuf,
    app: App,
    url: String,
    client: Client,
    stop: tokio::sync::oneshot::Sender<()>,
    server: tokio::task::JoinHandle<()>,
}
impl Fixture {
    async fn new(repository: bool) -> Self {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().canonicalize().unwrap();
        if repository {
            assert!(
                std::process::Command::new("git")
                    .args(["init", "--quiet"])
                    .arg(&directory)
                    .status()
                    .unwrap()
                    .success()
            );
        }
        let paths = Paths {
            data: directory.join("data"),
            config: directory.join("config"),
            state: directory.join("state"),
            cache: directory.join("cache"),
            tmp: directory.join("tmp"),
        };
        paths.ensure().unwrap();
        let app = App::build(AppOptions {
            paths,
            home: directory.join("home"),
            database: DatabaseLocation::Memory,
            default_directory: directory.clone(),
            sandbox_policy: None,
            snapshots: false,
            interactive: true,
            password: Some("memory-test".into()),
        })
        .await
        .unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/api/v1", listener.local_addr().unwrap());
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
            root,
            directory,
            app,
            url,
            client: Client::builder().no_proxy().build().unwrap(),
            stop,
            server,
        }
    }
    fn get(&self, route: &str) -> reqwest::RequestBuilder {
        self.client
            .get(format!("{}{}", self.url, route))
            .basic_auth("cyber", Some("memory-test"))
    }
    async fn close(self) {
        self.stop.send(()).unwrap();
        self.server.await.unwrap();
        self.app.runtime.shutdown().await;
        drop(self.root);
    }
    #[cfg(unix)]
    fn store(&self, scope: &str) -> cyber_core::memory::MemoryStore {
        let project = if scope == "global" {
            "global".into()
        } else {
            cyber_core::project::identify(&self.directory).id
        };
        cyber_core::memory::MemoryStore::open(&self.app.paths.data, &project).unwrap()
    }
}

#[tokio::test]
async fn empty_review_authentication_and_invalid_routing_create_no_scope() {
    let f = Fixture::new(true).await;
    let response = f
        .client
        .get(format!("{}/memory", f.url))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let response = f.get("/memory").send().await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let value: Value = response.json().await.unwrap();
    assert_eq!(value["data"], json!({"memories":[],"invalid":[]}));
    assert_eq!(
        value["location"]["directory"],
        f.directory.to_str().unwrap()
    );
    for route in [
        "/memory?scope=other",
        "/memory/other/note",
        "/memory/project/Bad_Name",
    ] {
        assert_eq!(
            f.get(route).send().await.unwrap().status(),
            StatusCode::BAD_REQUEST
        );
    }
    let missing = f.get("/memory/project/absent").send().await.unwrap();
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);
    assert_eq!(
        missing.json::<Value>().await.unwrap()["_tag"],
        "MemoryNotFoundError"
    );
    assert_eq!(
        f.get("/memory")
            .header(
                "x-cyber-directory",
                f.directory.join("missing").to_str().unwrap()
            )
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        std::fs::read_dir(f.app.paths.data.join("memory"))
            .unwrap()
            .count(),
        0
    );
    f.close().await;
}

#[cfg(unix)]
fn text(name: &str, body: &str) -> String {
    format!("---\nname: {name}\ndescription: Durable policy\ntype: reference\n---\n\n{body}\n")
}

#[cfg(unix)]
#[tokio::test]
async fn scope_and_location_review_match_cli_storage_without_model_settings() {
    let f = Fixture::new(true).await;
    for (scope, body) in [("project", "repository policy"), ("global", "user policy")] {
        let store = f.store(scope);
        store.claim().unwrap().write(&text("policy", body)).unwrap();
    }
    std::fs::write(
        f.app.paths.config.join("cyber.json"),
        r#"{"memory":{"enabled":false}}"#,
    )
    .unwrap();
    for (scope, body) in [("project", "repository policy"), ("global", "user policy")] {
        let response = f
            .get(&format!("/memory/{scope}/policy"))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let value: Value = response.json().await.unwrap();
        assert_eq!(value["data"]["body"], body);
        assert_eq!(value["data"]["metadata"]["type"], "reference");
        let catalog: Value = f
            .get(&format!("/memory?scope={scope}"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(catalog["data"]["memories"][0]["name"], "policy");
    }
    let outside = tempfile::tempdir().unwrap();
    let sibling = outside.path().canonicalize().unwrap();
    let response = f
        .get("/memory/project/policy")
        .header("x-cyber-directory", f.directory.to_str().unwrap())
        .query(&[("location[directory]", sibling.to_str().unwrap())])
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.json::<Value>().await.unwrap()["data"]["body"],
        "user policy"
    );
    f.close().await;
}

#[cfg(unix)]
#[tokio::test]
async fn busy_pending_invalid_and_alias_review_preserve_evidence() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let f = Fixture::new(false).await;
    let store = f.store("global");
    let mut owner = store.claim().unwrap();
    owner.write(&text("policy", "durable fact")).unwrap();
    assert_eq!(
        f.get("/memory").send().await.unwrap().status(),
        StatusCode::CONFLICT
    );
    drop(owner);
    let malformed = store.path().join("malformed.md");
    std::fs::write(&malformed, "private invalid text").unwrap();
    std::fs::set_permissions(&malformed, std::fs::Permissions::from_mode(0o600)).unwrap();
    let response: Value = f.get("/memory").send().await.unwrap().json().await.unwrap();
    assert_eq!(response["data"]["memories"].as_array().unwrap().len(), 1);
    assert_eq!(response["data"]["invalid"][0]["filename"], "malformed.md");
    assert!(!response.to_string().contains("private invalid text"));
    assert_eq!(
        f.get("/memory/global/malformed")
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::BAD_REQUEST
    );
    symlink(
        store.path().join("policy.md"),
        store.path().join("alias.md"),
    )
    .unwrap();
    assert_eq!(
        f.get("/memory/global/alias").send().await.unwrap().status(),
        StatusCode::CONFLICT
    );
    let mut owner = store.claim().unwrap();
    let prepared = owner
        .prepare_write(&text("policy", "updated fact"))
        .unwrap();
    drop(prepared);
    drop(owner);
    for route in ["/memory", "/memory/global/policy"] {
        assert_eq!(
            f.get(route).send().await.unwrap().status(),
            StatusCode::CONFLICT
        );
    }
    assert!(store.path().join(".memory-transaction").exists());
    assert!(
        std::fs::read_to_string(store.path().join("policy.md"))
            .unwrap()
            .contains("durable fact")
    );
    f.close().await;
}
