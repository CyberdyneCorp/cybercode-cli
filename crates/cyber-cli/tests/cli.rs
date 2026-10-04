//! End-to-end tests of the `cyber` binary (`cli-commands`, `workspace-trust`).

use std::path::{Path, PathBuf};
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
        Self { _dir: dir, root }
    }

    fn cyber(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_cyber"))
            .args(args)
            .env_clear()
            .env("HOME", self.root.join("home"))
            .env("CYBER_HOME", self.root.join("cyber-home"))
            .env("PATH", std::env::var("PATH").unwrap_or_default())
            .current_dir(&self.root)
            .output()
            .unwrap()
    }

    fn repo(&self, config: &str) -> PathBuf {
        let repo = self.root.join("repo");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        std::fs::write(repo.join("cyber.jsonc"), config).unwrap();
        repo
    }
}

fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

fn json(o: &Output) -> Value {
    serde_json::from_slice(&o.stdout).unwrap_or_else(|e| panic!("{e}: {}", stdout(o)))
}

#[test]
fn version_has_semver_channel_sha_and_target() {
    let o = Env::new().cyber(&["--version"]);
    assert!(o.status.success());
    let line = stdout(&o);
    let parts: Vec<&str> = line.trim().splitn(3, ' ').collect();
    assert_eq!(parts[0], "cyber");
    assert_eq!(parts[1].split('.').count(), 3, "{line}");
    assert!(
        parts[2].starts_with('(') && parts[2].ends_with(')') && parts[2].matches(", ").count() == 2,
        "{line}"
    );
}

#[test]
fn version_as_json() {
    let o = Env::new().cyber(&["--version", "--format", "json"]);
    let v = json(&o);
    assert_eq!(v["name"], "cyber");
    assert!(v["target"].as_str().is_some_and(|t| !t.is_empty()));
}

#[test]
fn unknown_command_suggests_and_exits_2() {
    let o = Env::new().cyber(&["sesions", "list"]);
    assert_eq!(o.status.code(), Some(2));
    assert!(
        stderr(&o).contains("Error: unknown command \"sesions\". Did you mean \"sessions\"?"),
        "{}",
        stderr(&o)
    );
}

#[test]
fn unshipped_command_is_explicitly_unavailable() {
    let o = Env::new().cyber(&["workflows", "list"]);
    assert_eq!(o.status.code(), Some(2));
    assert!(
        stderr(&o).contains("`cyber workflows` is not available in this build yet"),
        "{}",
        stderr(&o)
    );
}

#[test]
fn unknown_flag_exits_2() {
    let o = Env::new().cyber(&["debug", "paths", "--nope"]);
    assert_eq!(o.status.code(), Some(2));
}

#[test]
fn json_errors_use_the_error_envelope() {
    let o = Env::new().cyber(&["exec", "--format", "json"]);
    assert_eq!(o.status.code(), Some(2));
    let v: Value = serde_json::from_str(stderr(&o).trim()).unwrap();
    assert_eq!(v["error"]["code"], 2);
    assert!(v["error"]["hint"].is_string());
}

#[test]
fn paths_follow_cyber_home_and_are_created() {
    let env = Env::new();
    let o = env.cyber(&["debug", "paths", "--format", "json"]);
    assert!(o.status.success(), "{}", stderr(&o));
    let v = json(&o);
    let home = env.root.join("cyber-home");
    assert_eq!(v["data"], home.join("data").display().to_string());
    assert_eq!(v["config"], home.join("config").display().to_string());
    assert!(home.join("data/tool-output").is_dir());
    assert!(
        home.join("config/cyber.jsonc").is_file(),
        "global config bootstrapped"
    );
}

#[test]
fn db_path_uses_the_channel_name() {
    let env = Env::new();
    let o = env.cyber(&["db", "path"]);
    let path = stdout(&o);
    assert!(path.trim().ends_with(".db"), "{path}");
    assert!(Path::new(path.trim()).starts_with(env.root.join("cyber-home/data")));
}

#[test]
fn db_query_is_read_only() {
    let env = Env::new();
    let db = env.root.join("cyber-home/data/test.db");
    std::fs::create_dir_all(db.parent().unwrap()).unwrap();
    {
        let conn = rusqlite::Connection::open(&db).unwrap();
        conn.execute_batch("CREATE TABLE session (id TEXT); INSERT INTO session VALUES ('ses_1');")
            .unwrap();
    }
    let run = |sql: &str, extra: &[&str]| {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_cyber"));
        cmd.args(["db", "query", sql])
            .args(extra)
            .env("HOME", env.root.join("home"))
            .env("CYBER_HOME", env.root.join("cyber-home"))
            .env("CYBER_DB", &db);
        cmd.output().unwrap()
    };
    let denied = run("DELETE FROM session", &[]);
    assert_eq!(denied.status.code(), Some(1));
    assert!(
        stderr(&denied).contains("attempt to write a readonly database"),
        "{}",
        stderr(&denied)
    );
    let tsv = run("SELECT id FROM session", &[]);
    assert_eq!(stdout(&tsv), "id\nses_1\n");
    let rows = run("SELECT id FROM session", &["--format", "json"]);
    assert_eq!(json(&rows), serde_json::json!([{"id": "ses_1"}]));
}

#[test]
fn config_flags_and_overrides_reach_resolved_config() {
    let env = Env::new();
    let o = env.cyber(&[
        "debug",
        "config",
        "--sources",
        "-m",
        "openai/gpt-6",
        "--config",
        "compaction.auto=false",
    ]);
    assert!(o.status.success(), "{}", stderr(&o));
    let v = json(&o);
    assert_eq!(v["value"]["model"], "openai/gpt-6");
    assert_eq!(v["value"]["compaction"]["auto"], false);
    assert_eq!(v["sources"]["/model"], "cli:flags");
}

#[test]
fn invalid_config_exits_2() {
    let env = Env::new();
    let repo = env.repo("{ \"mode\": \"plan\"\n \"model\": 1 }");
    let o = env.cyber(&["debug", "config", "--cwd", repo.to_str().unwrap()]);
    assert_eq!(o.status.code(), Some(2));
    assert!(stderr(&o).contains("ConfigParseError"), "{}", stderr(&o));
}

#[test]
fn trust_inspect_approve_and_revoke() {
    let env = Env::new();
    let repo = env.repo(
        r#"{ "model": "openai/gpt-6", "mcp": { "db": { "type": "local", "command": "x" } } }"#,
    );
    let cwd = repo.to_str().unwrap();

    let report = json(&env.cyber(&["trust", "inspect", "--cwd", cwd, "--format", "json"]));
    assert_eq!(report["trusted"], false);
    let digest = report["digest"].as_str().unwrap().to_string();

    let before = env.cyber(&["debug", "config", "--cwd", cwd]);
    assert!(json(&before).get("mcp").is_none());
    assert!(stderr(&before).contains("inactive until approved"));

    let wrong = env.cyber(&["trust", "approve", "--cwd", cwd, "--digest", "sha256:0"]);
    assert_eq!(wrong.status.code(), Some(2));

    assert!(
        env.cyber(&["trust", "approve", "--cwd", cwd, "--digest", &digest])
            .status
            .success()
    );
    let after = json(&env.cyber(&["debug", "config", "--cwd", cwd]));
    assert_eq!(after["mcp"]["db"]["command"], "x");

    assert!(stdout(&env.cyber(&["trust", "revoke", "--cwd", cwd])).starts_with("revoked trust"));
    assert!(
        json(&env.cyber(&["debug", "config", "--cwd", cwd]))
            .get("mcp")
            .is_none()
    );
}

#[test]
fn bare_cyber_without_a_terminal_points_to_exec() {
    let o = Env::new().cyber(&[]);
    assert_eq!(o.status.code(), Some(2));
    assert!(
        stderr(&o).contains("the TUI needs a terminal"),
        "{}",
        stderr(&o)
    );
}

#[test]
fn invalid_launch_combinations_fail_before_the_ui() {
    let o = Env::new().cyber(&["--fork"]);
    assert_eq!(o.status.code(), Some(2));
    assert!(stderr(&o).contains("--fork needs"), "{}", stderr(&o));
}
