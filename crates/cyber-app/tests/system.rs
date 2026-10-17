//! `GET /api/v1/system` (`observability-costs` → Machine stats) through the production
//! App's HTTP router: the shape, the empty first CPU reading, the 2 s sample cache and
//! the Location's disk.

use std::time::Duration;

use cyber_app::{App, AppOptions};
use cyber_core::paths::{DatabaseLocation, Paths};
use serde_json::Value;

const PASSWORD: &str = "system-password";

async fn app(root: &std::path::Path) -> (App, String) {
    let paths = Paths {
        data: root.join("data"),
        config: root.join("config"),
        state: root.join("state"),
        cache: root.join("cache"),
        tmp: root.join("tmp"),
    };
    paths.ensure().unwrap();
    let app = App::build(AppOptions {
        paths,
        home: root.join("home"),
        database: DatabaseLocation::Memory,
        default_directory: root.to_path_buf(),
        sandbox_policy: None,
        snapshots: false,
        interactive: true,
        password: Some(PASSWORD.into()),
        mcp_credentials: None,
        remote_skill_fetcher: None,
        workflow_cores: None,
    })
    .await
    .unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(cyber_server::http::serve_tcp(
        app.router(),
        listener,
        std::future::pending::<()>(),
    ));
    (app, base)
}

async fn system(http: &reqwest::Client, base: &str, directory: &std::path::Path) -> Value {
    let response = http
        .get(format!("{base}/api/v1/system"))
        .basic_auth("cyber", Some(PASSWORD))
        .header("x-cyber-directory", directory.display().to_string())
        .send()
        .await
        .unwrap();
    assert!(response.status().is_success(), "{}", response.status());
    response.json().await.unwrap()
}

#[tokio::test]
async fn system_reports_cpu_memory_and_the_location_disk_with_a_two_second_cache() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let (_app, base) = app(&root).await;
    let http = reqwest::Client::builder().no_proxy().build().unwrap();

    let unauthorized = http
        .get(format!("{base}/api/v1/system"))
        .send()
        .await
        .unwrap();
    assert_eq!(unauthorized.status(), 401);

    let first = system(&http, &base, &root).await;
    let data = &first["data"];
    // One CPU reading cannot give usage: idle is unknown, never a made-up number.
    assert_eq!(data["cpu_idle_percent"], Value::Null, "{first}");
    assert!(data["cores_logical"].as_u64().unwrap() >= 1, "{first}");
    let total = data["memory_total"].as_u64().unwrap();
    let available = data["memory_available"].as_u64().unwrap();
    assert!(total > 0 && available <= total, "{first}");
    let disk = &data["disk"];
    let (disk_total, disk_free) = (
        disk["total"].as_u64().unwrap(),
        disk["available"].as_u64().unwrap(),
    );
    assert!(disk_total > 0 && disk_free <= disk_total, "{first}");
    let mount = disk["mount"].as_str().unwrap().to_lowercase();
    let location = root.display().to_string().to_lowercase();
    let location = location.trim_start_matches(r"\\?\");
    assert!(
        location.starts_with(mount.trim_end_matches(['/', '\\'])),
        "{mount} does not hold {location}"
    );
    if cfg!(windows) {
        assert_eq!(data["load_avg"], Value::Null);
    } else {
        assert_eq!(
            data["load_avg"].as_array().map(Vec::len),
            Some(3),
            "{first}"
        );
    }
    // The sample names the OS, never the user.
    let text = first.to_string();
    if let Ok(user) = std::env::var("USER").or_else(|_| std::env::var("USERNAME"))
        && user.len() > 2
    {
        let without_paths = text.replace(&root.display().to_string(), "");
        assert!(!without_paths.contains(&user), "{text}");
    }

    // Within 2 s the cached sample is served again.
    let again = system(&http, &base, &root).await;
    assert_eq!(again["data"]["sampled_at"], data["sampled_at"]);

    tokio::time::sleep(Duration::from_millis(2_100)).await;
    let later = system(&http, &base, &root).await;
    assert_ne!(later["data"]["sampled_at"], data["sampled_at"]);
    let idle = later["data"]["cpu_idle_percent"].as_f64().unwrap();
    assert!((0.0..=100.0).contains(&idle), "{later}");
}
