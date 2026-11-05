//! Language routes through the production App (`localization` → Language selection,
//! `server-api` → Language routes): `PUT /config/language` writes the user's global
//! default, and a project that sets its own keeps it at its Location.

use std::path::{Path, PathBuf};

use cyber_app::{App, AppOptions};
use cyber_core::paths::{DatabaseLocation, Paths};
use reqwest::{Client, Method};
use serde_json::{Value, json};

const PASSWORD: &str = "language-test";

struct Fixture {
    _temp: tempfile::TempDir,
    config: PathBuf,
    url: String,
    client: Client,
    _app: App,
}

impl Fixture {
    async fn start() -> Self {
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
        let config = paths.config.clone();
        std::fs::write(
            config.join("cyber.jsonc"),
            "{\n  // kept\n  \"mode\": \"default\"\n}\n",
        )
        .unwrap();
        let options = AppOptions {
            database: DatabaseLocation::File(paths.data.join("cyber.db")),
            paths,
            home: root.join("home"),
            default_directory: root.clone(),
            sandbox_policy: None,
            snapshots: false,
            interactive: true,
            password: Some(PASSWORD.into()),
            mcp_credentials: None,
            remote_skill_fetcher: None,
            workflow_cores: None,
            isolation: None,
        };
        let app = App::build(options).await.unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/api/v1", listener.local_addr().unwrap());
        tokio::spawn(cyber_server::http::serve_tcp(
            app.router(),
            listener,
            std::future::pending::<()>(),
        ));
        Self {
            _temp: temp,
            config,
            url,
            client: Client::builder().no_proxy().build().unwrap(),
            _app: app,
        }
    }

    async fn send(&self, method: Method, directory: &Path, body: Option<Value>) -> (u16, Value) {
        let mut request = self
            .client
            .request(method, format!("{}/config/language", self.url))
            .header("x-cyber-directory", directory.display().to_string())
            .basic_auth("cyber", Some(PASSWORD));
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request.send().await.unwrap();
        let status = response.status().as_u16();
        (status, response.json().await.unwrap_or(Value::Null))
    }
}

#[tokio::test]
async fn the_global_default_is_written_and_a_project_keeps_its_own() {
    let f = Fixture::start().await;
    let plain = f.config.parent().unwrap().join("plain");
    std::fs::create_dir_all(plain.join(".git")).unwrap();

    let (status, shown) = f.send(Method::GET, &plain, None).await;
    assert_eq!(status, 200, "{shown}");
    assert_eq!(shown["data"]["language"], "auto");
    assert_eq!(
        shown["data"]["supported"],
        json!(["auto", "en", "pt-BR", "es", "fr"])
    );
    assert!(shown["data"].get("reply").is_none());

    let (status, set) = f
        .send(Method::PUT, &plain, Some(json!({"language": "PT-br"})))
        .await;
    assert_eq!(status, 200, "{set}");
    assert_eq!(set["data"]["language"], "pt-BR");
    assert_eq!(set["data"]["reply"], "pt-BR");
    let written = std::fs::read_to_string(f.config.join("cyber.jsonc")).unwrap();
    assert!(
        written.contains("// kept") && written.contains("\"language\": \"pt-BR\""),
        "{written}"
    );

    // A project's own `language` wins at its Location; the global edit still happens.
    let project = f.config.parent().unwrap().join("project");
    std::fs::create_dir_all(project.join(".git")).unwrap();
    std::fs::write(project.join("cyber.jsonc"), r#"{ "language": "es" }"#).unwrap();
    let (status, set) = f
        .send(Method::PUT, &project, Some(json!({"language": "fr"})))
        .await;
    assert_eq!(status, 200, "{set}");
    assert_eq!(set["data"]["language"], "es");
    let source = set["data"]["source"].as_str().unwrap();
    assert!(source.contains("project"), "{source}");
    let (_, shown) = f.send(Method::GET, &plain, None).await;
    assert_eq!(shown["data"]["language"], "fr");

    let (status, refused) = f
        .send(Method::PUT, &plain, Some(json!({"language": "de"})))
        .await;
    assert_eq!(status, 400, "{refused}");
    let (_, shown) = f.send(Method::GET, &plain, None).await;
    assert_eq!(
        shown["data"]["language"], "fr",
        "a refused value changes nothing"
    );
}
