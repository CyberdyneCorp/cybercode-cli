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

    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_cyber"));
        command
            .args(args)
            .env_clear()
            .env("HOME", self.root.join("home"))
            .env("CYBER_HOME", self.root.join("cyber-home"))
            .env("PATH", std::env::var("PATH").unwrap_or_default())
            .current_dir(&self.root);
        #[cfg(windows)]
        if let Some(system) = std::env::var_os("SystemRoot") {
            command.env("SystemRoot", system);
        }
        command
    }

    fn cyber(&self, args: &[&str]) -> Output {
        self.command(args).output().unwrap()
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

#[test]
fn doctor_reports_every_check_as_json() {
    let o = Env::new().cyber(&["doctor", "--format", "json"]);
    let v: Value = serde_json::from_str(&String::from_utf8_lossy(&o.stdout)).unwrap();
    let names: Vec<&str> = v["checks"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|c| c["check"].as_str())
        .collect();
    assert_eq!(
        names,
        vec![
            "config",
            "catalog",
            "credentials",
            "database",
            "sandbox",
            "git",
            "rg",
            "server"
        ]
    );
    let failed = v["checks"]
        .as_array()
        .unwrap()
        .iter()
        .any(|c| c["status"] == "fail");
    assert_eq!(o.status.code(), Some(if failed { 1 } else { 0 }));
}

#[test]
fn db_backup_without_a_database_is_a_usage_error() {
    let env = Env::new();
    assert!(
        env.cyber(&["db", "backup", "x.db"]).status.code() == Some(2),
        "no database yet is a usage error"
    );
}

struct OwnedService(std::process::Child);

impl Drop for OwnedService {
    fn drop(&mut self) {
        // This is our actual child handle, never a PID from a registration file.
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn service_stop_uses_registered_http_shutdown() {
    use std::process::Stdio;
    use std::time::{Duration, Instant};
    let env = Env::new();
    let mut command = env.command(&["serve", "--register", "--port", "0"]);
    #[cfg(unix)]
    command.arg("--socket").arg(env.root.join("s.sock"));
    let diagnostic = env.root.join("server-stderr.log");
    let mut server = OwnedService(
        command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(std::fs::File::create(&diagnostic).unwrap())
            .spawn()
            .unwrap(),
    );
    let registration = env.root.join("cyber-home/state/server.json");
    let deadline = Instant::now() + Duration::from_secs(10);
    while !registration.is_file() {
        if let Some(status) = server.0.try_wait().unwrap() {
            panic!(
                "server exited before registration ({status}): {}",
                std::fs::read_to_string(&diagnostic).unwrap_or_default()
            );
        }
        assert!(Instant::now() < deadline, "server did not register");
        std::thread::sleep(Duration::from_millis(10));
    }
    let output = env.cyber(&["service", "stop", "--format", "json"]);
    assert!(output.status.success(), "{}", stderr(&output));
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(status) = server.0.try_wait().unwrap() {
            assert!(status.success());
            break;
        }
        assert!(
            Instant::now() < deadline,
            "CLI stop left its owned server running"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(!registration.exists());
}

#[cfg(unix)]
#[test]
fn exec_worktree_starts_in_owned_checkout_and_keeps_json_stdout_clean() {
    let env = Env::new();
    let repo = env.root.join("repo");
    std::fs::create_dir(&repo).unwrap();
    for args in [
        vec!["init", "--quiet"],
        vec!["config", "user.name", "Worktree"],
        vec!["config", "user.email", "test@example.invalid"],
    ] {
        assert!(
            Command::new("git")
                .current_dir(&repo)
                .args(args)
                .status()
                .unwrap()
                .success()
        );
    }
    std::fs::write(repo.join("tracked"), "base").unwrap();
    for args in [
        vec!["add", "tracked"],
        vec!["commit", "--quiet", "-m", "initial"],
    ] {
        assert!(
            Command::new("git")
                .current_dir(&repo)
                .args(args)
                .status()
                .unwrap()
                .success()
        );
    }
    let config = env.root.join("cyber-home/config/cyber.jsonc");
    std::fs::create_dir_all(config.parent().unwrap()).unwrap();
    std::fs::write(config, serde_json::json!({"worktrees": {"setup": ["printf setup-ready; sleep 0.15; printf setup > setup-result"]}}).to_string()).unwrap();
    // An unavailable model ends the prompt locally after startup, without a provider call.
    let output = env.cyber(&[
        "--cwd",
        repo.to_str().unwrap(),
        "exec",
        "--embedded",
        "--worktree",
        "from-cli",
        "--model",
        "missing/model",
        "--format",
        "json",
        "prompt",
    ]);
    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
    let result = json(&output);
    assert!(result["session_id"].as_str().unwrap().starts_with("ses_"));
    assert!(
        stderr(&output).contains("setup-ready"),
        "{}",
        stderr(&output)
    );
    let record: Value = serde_json::from_slice(
        &std::fs::read(repo.join(".git/cyber-worktrees/from-cli.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(record["ready"], true);
    let target = Path::new(record["path"].as_str().unwrap());
    assert_eq!(
        std::fs::read_to_string(target.join("tracked")).unwrap(),
        "base"
    );
    assert_eq!(
        std::fs::read_to_string(target.join("setup-result")).unwrap(),
        "setup"
    );
    assert!(!repo.join("setup-result").exists());
    verify_worktree_list_command(&env, &repo, target);
}

#[cfg(unix)]
fn verify_worktree_list_command(env: &Env, repo: &Path, target: &Path) {
    std::fs::write(repo.join(".git/cyber-worktrees/broken.json"), "not JSON").unwrap();
    let listed = env.cyber(&[
        "--cwd",
        repo.to_str().unwrap(),
        "worktree",
        "list",
        "--format",
        "json",
    ]);
    let text = env.cyber(&["--cwd", repo.to_str().unwrap(), "worktree", "list"]);
    let stopped = env.cyber(&["service", "stop"]);
    assert!(stopped.status.success(), "{}", stderr(&stopped));
    assert!(listed.status.success(), "{}", stderr(&listed));
    let entries = json(&listed);
    assert_eq!(entries.as_array().unwrap().len(), 2);
    assert_eq!(entries[0]["status"], "invalid");
    assert_eq!(entries[0]["name"], "broken");
    assert_eq!(entries[1]["worktree"]["path"], target.to_str().unwrap());
    assert_eq!(entries[1]["dirty"], true);
    assert_eq!(entries[1]["ahead"], 0);
    assert!(text.status.success(), "{}", stderr(&text));
    assert!(stdout(&text).contains("ahead=0 behind=0 dirty"));
    assert!(stdout(&text).contains("broken  invalid:"));
    assert_eq!(
        std::fs::read_to_string(target.join("setup-result")).unwrap(),
        "setup"
    );
}

#[test]
fn auto_statistics_show_and_reset_are_checkout_scoped_and_persisted() {
    let env = Env::new();
    let repo = env.repo("{}");
    let show = env
        .command(&["--format", "json", "permissions", "auto", "show"])
        .current_dir(&repo)
        .output()
        .unwrap();
    assert!(
        show.status.success(),
        "{}",
        String::from_utf8_lossy(&show.stderr)
    );
    let before: Value = serde_json::from_slice(&show.stdout).unwrap();
    assert_eq!(
        before["checkout_root"],
        repo.canonicalize().unwrap().display().to_string()
    );
    assert_eq!(before["allowed"], 0);
    assert!(before["recorded_since"].is_null());
    let reset = env
        .command(&["--format", "json", "permissions", "auto", "reset"])
        .current_dir(&repo)
        .output()
        .unwrap();
    assert!(
        reset.status.success(),
        "{}",
        String::from_utf8_lossy(&reset.stderr)
    );
    let value: Value = serde_json::from_slice(&reset.stdout).unwrap();
    assert!(value["reset_at"].is_i64());
    let sub = repo.join("src");
    std::fs::create_dir(&sub).unwrap();
    let after = env
        .command(&["--format", "json", "permissions", "auto", "show"])
        .current_dir(sub)
        .output()
        .unwrap();
    assert!(after.status.success());
    assert_eq!(
        serde_json::from_slice::<Value>(&after.stdout).unwrap(),
        value
    );
}

#[test]
fn hook_review_approves_exact_digests_and_revokes_obsolete_definitions() {
    let env = Env::new();
    let repo = env.repo(
        r#"{"hooks":{"PreToolUse":[{"hooks":[{"type":"command","command":"echo reviewed"}]}]}}"#,
    );
    let cwd = repo.to_str().unwrap();
    let list = json(&env.cyber(&["hooks", "list", "--cwd", cwd, "--format", "json"]));
    assert!(list["hooks"].as_array().unwrap().is_empty());
    assert!(!list["withheld_definitions"].as_array().unwrap().is_empty());
    let report = json(&env.cyber(&["trust", "inspect", "--cwd", cwd, "--format", "json"]));
    assert!(
        env.cyber(&[
            "trust",
            "approve",
            "--cwd",
            cwd,
            "--digest",
            report["digest"].as_str().unwrap()
        ])
        .status
        .success()
    );
    let list = json(&env.cyber(&["hooks", "list", "--cwd", cwd, "--format", "json"]));
    let digest = list["hooks"][0]["digest"].as_str().unwrap();
    assert_eq!(list["hooks"][0]["trusted"], false);
    assert_eq!(list["hooks"][0]["scope"], "project");
    assert_eq!(list["hooks"][0]["sandbox_required"], true);
    assert!(
        env.cyber(&["hooks", "trust", "--cwd", cwd, "--digest", digest])
            .status
            .success()
    );
    let trusted = json(&env.cyber(&["hooks", "list", "--cwd", cwd, "--format", "json"]));
    assert_eq!(trusted["hooks"][0]["trusted"], true);
    std::fs::write(
        repo.join("cyber.jsonc"),
        r#"{"hooks":{"PreToolUse":[{"hooks":[{"type":"command","command":"echo changed"}]}]}}"#,
    )
    .unwrap();
    assert!(
        !env.cyber(&["hooks", "trust", "--cwd", cwd, "--digest", digest])
            .status
            .success()
    );
    let changed = json(&env.cyber(&["trust", "inspect", "--cwd", cwd, "--format", "json"]));
    assert!(
        env.cyber(&[
            "trust",
            "approve",
            "--cwd",
            cwd,
            "--digest",
            changed["digest"].as_str().unwrap()
        ])
        .status
        .success()
    );
    let changed_list = json(&env.cyber(&["hooks", "list", "--cwd", cwd, "--format", "json"]));
    assert_eq!(changed_list["hooks"][0]["trusted"], false);
    assert_ne!(changed_list["hooks"][0]["digest"], digest);
    std::fs::write(repo.join("cyber.jsonc"), "malformed").unwrap();
    let revoke = json(&env.cyber(&[
        "hooks", "untrust", "--cwd", cwd, "--digest", digest, "--format", "json",
    ]));
    assert_eq!(revoke["revoked"], true);
}

#[test]
fn hook_listing_redacts_credentials_and_reports_global_scope() {
    let env = Env::new();
    let config = env.root.join("cyber-home/config");
    std::fs::create_dir_all(&config).unwrap();
    std::fs::write(config.join("cyber.jsonc"),r#"{"hooks":{"PreToolUse":[{"hooks":[{"type":"http","url":"http://127.0.0.1:9/hook","headers":{"Authorization":"secret-credential"}}]}]}}"#).unwrap();
    let output = env.cyber(&["hooks", "list", "--format", "json"]);
    assert!(output.status.success(), "{}", stderr(&output));
    let value = json(&output);
    assert_eq!(value["hooks"][0]["scope"], "global");
    assert_eq!(value["hooks"][0]["trusted"], true);
    assert_eq!(value["hooks"][0]["sandbox_required"], false);
    assert_eq!(
        value["hooks"][0]["handler"]["headers"]["Authorization"],
        "***"
    );
    assert!(!stdout(&output).contains("secret-credential"));
}

#[test]
fn hook_listing_reviews_withheld_literals_without_approval_or_execution() {
    let env = Env::new();
    let repo = env.repo(r#"{"hooks":{"PreToolUse":[{"hooks":[{"type":"invalid","command":"{env:UNREAD_SECRET} {file:/unreadable-secret}","headers":{"Authorization":"private-token"}}]}]}}"#);
    let cwd = repo.to_str().unwrap();
    let list = json(&env.cyber(&["hooks", "list", "--cwd", cwd, "--format", "json"]));
    assert!(list["hooks"].as_array().unwrap().is_empty());
    assert_eq!(list["withheld_hooks"][0]["scope"], "project");
    assert_eq!(list["withheld_hooks"][0]["pointer"], "/hooks");
    assert_eq!(
        list["withheld_hooks"][0]["value"]["PreToolUse"][0]["hooks"][0]["command"],
        "{env:UNREAD_SECRET} {file:/unreadable-secret}"
    );
    assert_eq!(
        list["withheld_hooks"][0]["value"]["PreToolUse"][0]["hooks"][0]["headers"]["Authorization"],
        "***"
    );
    assert!(!list.to_string().contains("private-token"));
    let text = env.cyber(&["hooks", "list", "--cwd", cwd]);
    assert!(text.status.success());
    let text = String::from_utf8(text.stdout).unwrap();
    assert!(text.contains("literal, inactive") && text.contains("UNREAD_SECRET"));
    assert!(!text.contains("private-token"));
    let report = json(&env.cyber(&["trust", "inspect", "--cwd", cwd, "--format", "json"]));
    assert_eq!(report["trusted"], false);
    assert!(
        !env.cyber(&[
            "hooks",
            "trust",
            "--cwd",
            cwd,
            "--digest",
            report["digest"].as_str().unwrap()
        ])
        .status
        .success()
    );
}
