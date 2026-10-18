//! `cyber pr <number>` and `cyber --from-pr` (`vcs-integration` → PR checkout and linked
//! sessions) against the real binary, the shared background server, a local scripted
//! model and a fake `gh` on `PATH`. Unix-only: the fake `gh` is a shell script.
#![cfg(unix)]

#[path = "../../cyber-app/tests/support/scripted_model.rs"]
mod scripted_model;

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::{Value, json};

/// `OPEN-PR` creates a PR with `gh pr create`; `PUSH-PR` pushes a branch whose PR `gh pr
/// view` reports. Everything else is a plain answer.
fn reply(body: &Value) -> Value {
    let text = body["messages"].to_string();
    let answered = text.contains("\"role\":\"tool\"");
    let bash = |command: &str| {
        json!({"tool_calls":[{"index":0,"id":"call_bash","type":"function",
            "function":{"name":"bash","arguments":json!({"command":command}).to_string()}}]})
    };
    if text.contains("OPEN-PR") && !answered {
        return bash("gh pr create --fill");
    }
    if text.contains("PUSH-PR") && !answered {
        return bash("git push origin HEAD; true");
    }
    json!({"content":"done"})
}

const FAKE_GH: &str = r#"#!/bin/sh
echo "$@" >> "$(dirname "$0")/gh.log"
case "$1 $2" in
  "pr create") echo "Creating pull request"; echo "https://github.com/acme/app/pull/42" ;;
  "pr view") echo '{"number":43,"url":"https://github.com/acme/app/pull/43"}' ;;
  "pr checkout") git checkout -q -b "pr-$3" ;;
esac
"#;

struct Env {
    _dir: tempfile::TempDir,
    root: PathBuf,
    repo: PathBuf,
    bin: PathBuf,
    _model: scripted_model::ScriptedModel,
}

impl Env {
    fn new() -> Self {
        Self::with_permissions(json!({"bash":"allow"}))
    }

    fn with_permissions(permissions: Value) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let model = scripted_model::ScriptedModel::start(reply);
        let mut config = model.config();
        config["sandbox"] = json!({"policy":"full-access"});
        config["permissions"] = permissions;
        let home = root.join("cyber-home").join("config");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::write(home.join("cyber.json"), config.to_string()).unwrap();
        std::fs::write(root.join("models.json"), "{}").unwrap();
        let bin = root.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(bin.join("gh"), FAKE_GH).unwrap();
        std::fs::set_permissions(bin.join("gh"), std::fs::Permissions::from_mode(0o755)).unwrap();
        let repo = root.join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init", "-q", "-b", "main"]);
        git(&repo, &["config", "user.email", "dev@example.com"]);
        git(&repo, &["config", "user.name", "Dev"]);
        std::fs::write(repo.join("app.txt"), "one\n").unwrap();
        git(&repo, &["add", "app.txt"]);
        git(&repo, &["commit", "-q", "-m", "init"]);
        Self {
            _dir: dir,
            root,
            repo,
            bin,
            _model: model,
        }
    }

    fn cyber(&self, args: &[&str]) -> Output {
        let path = format!(
            "{}:{}",
            self.bin.display(),
            std::env::var("PATH").unwrap_or_default()
        );
        Command::new(env!("CARGO_BIN_EXE_cyber"))
            .args(args)
            .env_clear()
            .env("HOME", self.root.join("home"))
            .env("CYBER_HOME", self.root.join("cyber-home"))
            .env("CYBER_MODELS_PATH", self.root.join("models.json"))
            .env("PATH", path)
            .current_dir(&self.repo)
            .output()
            .unwrap()
    }

    fn json(&self, args: &[&str]) -> Value {
        let output = self.cyber(args);
        assert!(
            output.status.success(),
            "{args:?}: {}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout)
            .unwrap_or_else(|e| panic!("{e}: {}", String::from_utf8_lossy(&output.stdout)))
    }

    fn gh_log(&self) -> String {
        std::fs::read_to_string(self.bin.join("gh.log")).unwrap_or_default()
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        let _ = self.cyber(&["service", "stop"]);
    }
}

fn git(repo: &Path, args: &[&str]) {
    let status = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .status()
        .unwrap();
    assert!(status.success(), "git {args:?}");
}

/// "Resume the linked session": a Session that opened PR #42 is resumed by `cyber pr 42`
/// after `gh pr checkout 42`; a pushed branch links its PR through `gh pr view`; an
/// unlinked PR starts a new Session; `--from-pr` lists the linked Sessions.
#[test]
fn pr_checkout_resumes_the_linked_session() {
    let env = Env::new();
    let opened = env.json(&["exec", "--format", "json", "OPEN-PR"]);
    let opener = opened["session_id"].as_str().unwrap().to_owned();
    assert!(
        env.gh_log().contains("pr create --fill"),
        "{}",
        env.gh_log()
    );
    let pushed = env.json(&["exec", "--format", "json", "PUSH-PR"]);
    let pusher = pushed["session_id"].as_str().unwrap().to_owned();
    assert!(
        env.gh_log().contains("pr view --json number,url"),
        "{}",
        env.gh_log()
    );

    let resumed = env.json(&["pr", "42", "--format", "json"]);
    assert_eq!(resumed["pr"], 42);
    assert_eq!(resumed["checked_out"], true);
    assert_eq!(resumed["session_id"], opener.as_str());
    assert_eq!(resumed["resumed"], true);
    assert!(env.gh_log().contains("pr checkout 42"), "{}", env.gh_log());
    let head = Command::new("git")
        .arg("-C")
        .arg(&env.repo)
        .args(["branch", "--show-current"])
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&head.stdout).trim(), "pr-42");

    let by_url = env.json(&[
        "pr",
        "https://github.com/acme/app/pull/43",
        "--format",
        "json",
    ]);
    assert_eq!(by_url["session_id"], pusher.as_str());

    let unlinked = env.json(&["pr", "7", "--format", "json"]);
    assert_eq!(unlinked["session_id"], Value::Null);
    assert_eq!(unlinked["resumed"], false);

    let picker = env.json(&["--from-pr", "#42", "--format", "json"]);
    assert_eq!(picker["sessions"], json!([opener]));

    // A failed checkout resumes nothing.
    std::fs::write(env.bin.join("gh"), "#!/bin/sh\nexit 1\n").unwrap();
    let failed = env.cyber(&["pr", "42", "--format", "json"]);
    assert!(!failed.status.success());
    assert!(failed.stdout.is_empty());
}

/// Regression: the `gh pr view` lookup after an allowed `git push` is itself a `bash`
/// command the Session's rules must allow; a `gh *` deny rule means no `gh` runs.
#[test]
fn a_gh_deny_rule_stops_the_lookup_after_a_push() {
    let env = Env::with_permissions(json!({"bash":{"*":"allow","gh *":"deny"}}));
    let pushed = env.json(&["exec", "--format", "json", "PUSH-PR"]);
    assert!(pushed["session_id"].is_string(), "{pushed}");
    assert_eq!(env.gh_log(), "", "gh ran despite the deny rule");
    let unlinked = env.json(&["--from-pr", "#43", "--format", "json"]);
    assert_eq!(unlinked["sessions"], json!([]));
}
