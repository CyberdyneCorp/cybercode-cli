//! `cyber mcp list|add|remove` (`mcp` → MCP CLI and UI) through the real binary: edits
//! keep JSONC comments, scopes pick the document, duplicates and ambiguity are refused,
//! and `list` reports a configured server that no Session opened yet as `idle`.

use std::path::PathBuf;
use std::process::{Command, Output};

use serde_json::Value;

struct Env {
    _dir: tempfile::TempDir,
    root: PathBuf,
}

impl Env {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        std::fs::create_dir_all(root.join("cyber-home/config")).unwrap();
        std::fs::create_dir_all(root.join("repo/.git")).unwrap();
        Self { _dir: dir, root }
    }

    fn global(&self) -> PathBuf {
        self.root.join("cyber-home/config/cyber.jsonc")
    }

    fn cyber(&self, args: &[&str]) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_cyber"));
        command
            .args(args)
            .env_clear()
            .env("HOME", self.root.join("home"))
            .env("CYBER_HOME", self.root.join("cyber-home"))
            .env("PATH", std::env::var("PATH").unwrap_or_default())
            .current_dir(self.root.join("repo"));
        #[cfg(windows)]
        if let Some(system) = std::env::var_os("SystemRoot") {
            command.env("SystemRoot", system);
        }
        command.output().unwrap()
    }

    fn ok(&self, args: &[&str]) -> String {
        let out = self.cyber(args);
        assert!(
            out.status.success(),
            "{args:?}: {}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    fn fails(&self, args: &[&str]) -> String {
        let out = self.cyber(args);
        assert!(!out.status.success(), "{args:?} should fail");
        String::from_utf8_lossy(&out.stderr).into_owned()
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        let _ = self.cyber(&["service", "stop"]);
    }
}

fn parse(path: &std::path::Path) -> Value {
    cyber_core::config::parse_jsonc("t", &std::fs::read_to_string(path).unwrap()).unwrap()
}

#[test]
fn add_keeps_comments_refuses_duplicates_and_remove_deletes_the_entry() {
    let env = Env::new();
    std::fs::write(
        env.global(),
        "{\n  // my settings\n  \"model\": \"x/y\" // trailing\n}\n",
    )
    .unwrap();
    env.ok(&[
        "mcp",
        "add",
        "docs",
        "--url",
        "https://docs.example/mcp",
        "--header",
        "Authorization=Bearer {env:DOCS}",
    ]);
    env.ok(&[
        "mcp",
        "add",
        "files",
        "--env",
        "ROOT=.",
        "--",
        "files-server",
        "--read-only",
    ]);
    let text = std::fs::read_to_string(env.global()).unwrap();
    assert!(
        text.contains("// my settings") && text.contains("// trailing"),
        "{text}"
    );
    let config = parse(&env.global());
    assert_eq!(config["model"], "x/y");
    assert_eq!(config["mcp"]["docs"]["type"], "remote");
    assert_eq!(
        config["mcp"]["docs"]["headers"]["Authorization"],
        "Bearer {env:DOCS}"
    );
    assert_eq!(config["mcp"]["files"]["command"], "files-server");
    assert_eq!(
        config["mcp"]["files"]["args"],
        serde_json::json!(["--read-only"])
    );
    assert_eq!(config["mcp"]["files"]["env"]["ROOT"], ".");

    env.ok(&["mcp", "remove", "docs"]);
    let config = parse(&env.global());
    assert!(config["mcp"].get("docs").is_none());
    assert_eq!(config["mcp"]["files"]["command"], "files-server");
    assert!(
        std::fs::read_to_string(env.global())
            .unwrap()
            .contains("// my settings")
    );
    let missing = env.fails(&["mcp", "remove", "docs"]);
    assert!(missing.contains("is not defined"), "{missing}");
}

#[test]
fn add_refuses_duplicates_mixed_or_missing_targets_and_bad_names() {
    let env = Env::new();
    env.ok(&["mcp", "add", "docs", "--url", "https://docs.example/mcp"]);
    let duplicate = env.fails(&["mcp", "add", "docs", "--url", "https://other/mcp"]);
    assert!(duplicate.contains("already defined"), "{duplicate}");
    let both = env.fails(&["mcp", "add", "x", "--url", "https://a", "--", "cmd"]);
    assert!(
        both.contains("not both") || both.contains("cannot be used"),
        "{both}"
    );
    let neither = env.fails(&["mcp", "add", "x"]);
    assert!(neither.contains("--url"), "{neither}");
    let name = env.fails(&["mcp", "add", "bad name", "--url", "https://a"]);
    assert!(name.contains("invalid MCP server name"), "{name}");
}

#[test]
fn project_and_local_scopes_pick_their_documents_and_remove_needs_a_scope_when_ambiguous() {
    let env = Env::new();
    let project = env.ok(&[
        "mcp",
        "add",
        "shared",
        "--scope",
        "project",
        "--url",
        "https://s/mcp",
    ]);
    assert!(
        project.contains("cyber mcp trust shared --digest sha256:"),
        "{project}"
    );
    env.ok(&[
        "mcp",
        "add",
        "mine",
        "--scope",
        "local",
        "--url",
        "https://m/mcp",
    ]);
    env.ok(&["mcp", "add", "shared", "--url", "https://user/mcp"]);
    let repo = env.root.join("repo/.cyber");
    assert_eq!(
        parse(&repo.join("cyber.jsonc"))["mcp"]["shared"]["url"],
        "https://s/mcp"
    );
    assert_eq!(
        parse(&repo.join("cyber.local.jsonc"))["mcp"]["mine"]["url"],
        "https://m/mcp"
    );
    assert_eq!(
        parse(&env.global())["mcp"]["shared"]["url"],
        "https://user/mcp"
    );

    let ambiguous = env.fails(&["mcp", "remove", "shared"]);
    assert!(ambiguous.contains("--scope"), "{ambiguous}");
    env.ok(&["mcp", "remove", "shared", "--scope", "project"]);
    assert!(
        parse(&repo.join("cyber.jsonc"))["mcp"]
            .get("shared")
            .is_none()
    );
    assert_eq!(
        parse(&env.global())["mcp"]["shared"]["url"],
        "https://user/mcp"
    );
}

#[test]
fn list_reports_a_configured_server_that_has_not_started_as_idle() {
    let env = Env::new();
    let empty = env.ok(&["mcp", "list"]);
    assert!(empty.contains("no MCP servers configured"), "{empty}");
    // A loopback URL nothing listens on: no Session opens the Location, so nothing connects.
    env.ok(&["mcp", "add", "later", "--url", "http://127.0.0.1:9/mcp"]);
    let listed: Value =
        serde_json::from_str(&env.ok(&["--format", "json", "mcp", "list"])).unwrap();
    let server = listed
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["name"] == "later")
        .unwrap_or_else(|| panic!("{listed}"));
    assert_eq!(server["status"], "idle", "{server}");
    assert_eq!(server["transport"], "remote");
    let table = env.ok(&["mcp", "list"]);
    assert!(table.contains("later") && table.contains("idle"), "{table}");
}
