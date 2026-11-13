//! `cyber doctor --network` (`cli-commands` → Doctor network check) against local
//! listeners: reachable and unreachable providers, the update channel, the proxy line,
//! and probes that carry no credentials.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::process::Command;
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};

/// A server answering every request `404`, keeping each request's head.
fn listener() -> (String, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let seen = Arc::new(Mutex::new(Vec::new()));
    let kept = seen.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut buffer = [0; 8192];
            let n = stream.read(&mut buffer).unwrap_or(0);
            kept.lock()
                .unwrap()
                .push(String::from_utf8_lossy(&buffer[..n]).to_ascii_lowercase());
            let _ = stream.write_all(
                b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            );
        }
    });
    (origin, seen)
}

/// A local port nothing listens on.
fn closed_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn provider(url: &str, settings: Value) -> String {
    json!({ "api": { "type": "openai-chat", "url": url, "settings": settings } }).to_string()
}

fn doctor(
    root: &std::path::Path,
    configs: &[(&str, String)],
    envs: &[(&str, &str)],
) -> std::process::Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_cyber"));
    command.args(["doctor", "--network", "--format", "json"]);
    for (key, value) in configs {
        command.args(["--config", &format!("providers.{key}={value}")]);
    }
    command
        .env_clear()
        .envs(envs.iter().copied())
        .env("HOME", root.join("home"))
        .env("CYBER_HOME", root.join("cyber"))
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .current_dir(root)
        .output()
        .unwrap()
}

fn checks(output: &std::process::Output) -> Value {
    serde_json::from_slice(&output.stdout).unwrap_or_else(|e| {
        panic!(
            "{e}: {}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    })
}

fn line<'a>(report: &'a Value, name: &str) -> &'a Value {
    report["checks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["check"] == name)
        .unwrap_or_else(|| panic!("no {name} in {report:#}"))
}

#[test]
fn network_checks_probe_usable_providers_without_credentials() {
    let root = tempfile::tempdir().unwrap();
    let (up, seen) = listener();
    let (updates, _) = listener();
    let down = format!("http://127.0.0.1:{}/v1", closed_port());
    let configs = [
        (
            "up",
            provider(&format!("{up}/v1"), json!({"api_key": "sk-doctor-secret"})),
        ),
        (
            "down",
            provider(&down, json!({"api_key": "sk-doctor-secret"})),
        ),
        ("idle", provider(&down, json!({"auth": "none"}))),
    ];
    let release = ("CYBER_TEST_RELEASE_BASE_URL", updates.as_str());
    let output = doctor(root.path(), &configs, &[release]);
    let report = checks(&output);

    // A keyed provider that cannot be reached fails the run.
    assert_eq!(output.status.code(), Some(1), "{report:#}");
    assert_eq!(report["ok"], false);
    let reached = line(&report, "network up");
    assert_eq!(reached["status"], "pass", "{report:#}");
    assert!(
        reached["detail"]
            .as_str()
            .unwrap()
            .contains("reachable: HTTP 404"),
        "{report:#}"
    );
    let failed = line(&report, "network down");
    assert_eq!(failed["status"], "fail", "{report:#}");
    assert!(
        failed["detail"]
            .as_str()
            .unwrap()
            .contains("unreachable: cannot connect"),
        "{report:#}"
    );
    // A configured endpoint that needs no credential only warns: a local server may not
    // be running.
    assert_eq!(
        line(&report, "network idle")["status"],
        "warn",
        "{report:#}"
    );
    assert_eq!(
        line(&report, "network updates")["status"],
        "pass",
        "{report:#}"
    );
    assert_eq!(line(&report, "proxy")["detail"], "none set");

    // Probes carry no credential, and no detail shows a URL path or the key.
    let requests = seen.lock().unwrap().clone();
    assert!(!requests.is_empty());
    assert!(
        requests
            .iter()
            .all(|r| !r.contains("authorization") && !r.contains("sk-doctor-secret")),
        "{requests:#?}"
    );
    assert!(!report.to_string().contains("sk-doctor-secret"));
    assert!(!report.to_string().contains("/v1"), "{report:#}");
}

/// Only network lines run, and a proxy URL's credentials are hidden.
#[test]
fn network_checks_hide_proxy_credentials_and_skip_other_checks() {
    let root = tempfile::tempdir().unwrap();
    let (up, _) = listener();
    let (updates, _) = listener();
    let configs = [(
        "up",
        provider(&format!("{up}/v1"), json!({"api_key": "sk-doctor-secret"})),
    )];
    let proxied = checks(&doctor(
        root.path(),
        &configs,
        &[
            ("CYBER_TEST_RELEASE_BASE_URL", updates.as_str()),
            ("NO_PROXY", "*"),
            ("HTTPS_PROXY", "http://me:hunter2@proxy.test:3128"),
        ],
    ));
    assert_eq!(
        line(&proxied, "proxy")["detail"],
        "HTTPS_PROXY=http://***@proxy.test:3128, NO_PROXY=*",
        "{proxied:#}"
    );
    let names: Vec<&str> = proxied["checks"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|c| c["check"].as_str())
        .collect();
    assert!(
        names
            .iter()
            .all(|name| *name == "proxy" || name.starts_with("network ")),
        "{names:?}"
    );
    assert!(!proxied.to_string().contains("hunter2"));
}
