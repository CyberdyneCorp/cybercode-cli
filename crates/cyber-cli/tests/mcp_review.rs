use serde_json::{Value, json};
use std::path::PathBuf;
use std::process::{Command, Output};

struct Env {
    _dir: tempfile::TempDir,
    root: PathBuf,
}
impl Env {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        std::fs::create_dir(root.join(".git")).unwrap();
        std::fs::create_dir_all(root.join("cyber/config")).unwrap();
        Self { _dir: dir, root }
    }
    fn run(&self, args: &[&str]) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_cyber"));
        command
            .args(["--format", "json"])
            .args(args)
            .env_clear()
            .env("HOME", self.root.join("home"))
            .env("CYBER_HOME", self.root.join("cyber"))
            .env("CYBER_DB", "review-test.db")
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .current_dir(&self.root);
        #[cfg(windows)]
        if let Some(root) = std::env::var_os("SystemRoot") {
            command.env("SystemRoot", root);
        }
        command.output().unwrap()
    }
    fn project(&self, value: Value) {
        std::fs::write(self.root.join("cyber.jsonc"), value.to_string()).unwrap();
    }
    fn approve_workspace(&self) {
        let inspected = body(&self.run(&["trust", "inspect"]));
        assert!(
            self.run(&[
                "trust",
                "approve",
                "--digest",
                inspected["digest"].as_str().unwrap()
            ])
            .status
            .success()
        );
    }
    fn assert_no_database(&self) {
        assert!(!self.root.join("cyber/data/review-test.db").exists());
    }
}
fn body(output: &Output) -> Value {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
fn definitions_redact_environment_headers_oauth_and_url_query_without_execution() {
    let env = Env::new();
    let marker = env.root.join("must-not-run");
    std::fs::write(env.root.join("cyber/config/cyber.jsonc"),json!({"mcp":{
        "local":{"type":"local","command":"unused-server","args":[marker],"env":{"PUBLIC":"private-env-secret"}},
        "remote":{"type":"remote","url":"https://example.test/mcp?access=private-url-secret","headers":{"custom":"private-header-secret"},"oauth":{"client_secret":"private-oauth-secret"},"enabled":false}
    }}).to_string()).unwrap();
    let output = env.run(&["mcp", "definitions"]);
    let value = body(&output);
    let printed = String::from_utf8(output.stdout).unwrap();
    for secret in [
        "private-env-secret",
        "private-url-secret",
        "private-header-secret",
        "private-oauth-secret",
    ] {
        assert!(!printed.contains(secret));
    }
    assert_eq!(value["servers"].as_array().unwrap().len(), 2);
    assert_eq!(value["servers"][0]["authorized"], true);
    assert_eq!(value["servers"][1]["authorized"], false);
    assert_eq!(
        env.run(&[
            "mcp",
            "trust",
            "local",
            "--digest",
            value["servers"][0]["digest"].as_str().unwrap()
        ])
        .status
        .code(),
        Some(2)
    );
    assert!(!marker.exists());
    env.assert_no_database();
}

#[test]
fn project_review_approves_exact_digest_and_revokes_obsolete_invalid_configuration() {
    let env = Env::new();
    env.project(json!({"mcp":{"audit":{"type":"local","command":"unused-server"}}}));
    let withheld = body(&env.run(&["mcp", "definitions"]));
    assert!(withheld["servers"].as_array().unwrap().is_empty());
    assert!(
        !withheld["withheld_definitions"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    env.approve_workspace();
    let listed = body(&env.run(&["mcp", "definitions"]));
    let digest = listed["servers"][0]["digest"].as_str().unwrap();
    assert_eq!(listed["servers"][0]["authorized"], false);
    assert_eq!(
        env.run(&["mcp", "trust", "audit", "--digest", "sha256:bad"])
            .status
            .code(),
        Some(2)
    );
    assert!(
        env.run(&["mcp", "trust", "audit", "--digest", digest])
            .status
            .success()
    );
    assert_eq!(
        body(&env.run(&["mcp", "definitions"]))["servers"][0]["authorized"],
        true
    );
    std::fs::write(env.root.join("cyber.jsonc"), "invalid json").unwrap();
    assert_eq!(
        body(&env.run(&["mcp", "untrust", "--digest", digest]))["revoked"],
        true
    );
    assert_eq!(
        body(&env.run(&["mcp", "untrust", "--digest", digest]))["revoked"],
        false
    );
    env.assert_no_database();
}

#[test]
fn changed_definition_and_another_checkout_cannot_reuse_inspected_digest() {
    let env = Env::new();
    env.project(json!({"mcp":{"audit":{"type":"local","command":"first"}}}));
    env.approve_workspace();
    let listed = body(&env.run(&["mcp", "definitions"]));
    let digest = listed["servers"][0]["digest"].as_str().unwrap();
    assert!(
        env.run(&["mcp", "trust", "audit", "--digest", digest])
            .status
            .success()
    );
    env.project(json!({"mcp":{"audit":{"type":"local","command":"changed"}}}));
    env.approve_workspace();
    assert_eq!(
        env.run(&["mcp", "trust", "audit", "--digest", digest])
            .status
            .code(),
        Some(2)
    );
    assert_eq!(
        body(&env.run(&["mcp", "definitions"]))["servers"][0]["authorized"],
        false
    );
    let other = Env::new();
    other.project(json!({"mcp":{"audit":{"type":"local","command":"first"}}}));
    other.approve_workspace();
    assert_eq!(
        body(&other.run(&["mcp", "definitions"]))["servers"][0]["authorized"],
        false
    );
}

#[test]
fn get_displays_one_resolved_server_with_redacted_headers_and_no_effects() {
    let env = Env::new();
    std::fs::write(env.root.join("cyber/config/cyber.jsonc"),json!({
        "tool_output":{"max_lines":42,"max_bytes":1234},
        "mcp":{
            "db":{"type":"remote","url":"https://example.test/mcp?key=private-url-secret","headers":{"custom":"private-header-secret"},"required":true,"output_token_limit":10},
            "local":{"type":"local","command":"not-installed"}
        }
    }).to_string()).unwrap();
    let output = env.run(&["mcp", "get", "db"]);
    let value = body(&output);
    assert_eq!(value["name"], "db");
    assert_eq!(value["call_timeout_seconds"], 300);
    assert_eq!(value["definition"]["headers"]["custom"], "***");
    assert_eq!(value["definition"]["required"], true);
    assert_eq!(
        value["output_budget"],
        json!({"output_token_limit":10,"max_lines":42,"max_bytes":1234})
    );
    assert!(
        !String::from_utf8(output.stdout)
            .unwrap()
            .contains("private-")
    );
    let local = body(&env.run(&["mcp", "get", "local"]));
    assert_eq!(local["definition"]["required"], false);
    assert_eq!(local["output_budget"]["output_token_limit"], Value::Null);
    assert_eq!(env.run(&["mcp", "get", "missing"]).status.code(), Some(2));
    env.assert_no_database();
}
