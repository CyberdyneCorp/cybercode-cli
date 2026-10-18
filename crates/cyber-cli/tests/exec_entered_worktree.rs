//! `worktrees` → Cleanup on exit for a plain `cyber exec` whose Session entered a
//! worktree with `enter_worktree` and ended without `exit_worktree`, against the real
//! binary and a local scripted model.
#[path = "../../cyber-app/tests/support/scripted_model.rs"]
mod scripted_model;

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::{Value, json};

/// `ENTER-<NAME>` enters the worktree `<name>`; `DIRTY` then writes a file in it.
fn reply(body: &Value) -> Value {
    let text = body["messages"].to_string();
    let call = |id: &str, name: &str, input: Value| {
        json!({"tool_calls":[{"index":0,"id":id,"type":"function",
            "function":{"name":name,"arguments":input.to_string()}}]})
    };
    let name = if text.contains("ENTER-TIDY") {
        "tidy"
    } else {
        "dirty"
    };
    if !text.contains("call_enter") {
        return call("call_enter", "enter_worktree", json!({"name": name}));
    }
    if text.contains("DIRTY") && !text.contains("call_write") {
        return call(
            "call_write",
            "write",
            json!({"path":"notes.txt","content":"work\n"}),
        );
    }
    json!({"content":"done"})
}

struct Env {
    _dir: tempfile::TempDir,
    root: PathBuf,
    repo: PathBuf,
    _model: scripted_model::ScriptedModel,
}

impl Env {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let model = scripted_model::ScriptedModel::start(reply);
        let mut config = model.config();
        config["permissions"] = json!({"*":"allow"});
        if cfg!(windows) {
            config["sandbox"] = json!({"policy":"full-access"});
        }
        let home = root.join("cyber-home").join("config");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::write(home.join("cyber.json"), config.to_string()).unwrap();
        std::fs::write(root.join("models.json"), "{}").unwrap();
        let repo = root.join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init", "-q", "-b", "main"]);
        git(&repo, &["config", "user.email", "dev@example.com"]);
        git(&repo, &["config", "user.name", "Dev"]);
        git(&repo, &["config", "commit.gpgsign", "false"]);
        std::fs::write(repo.join("app.txt"), "one\n").unwrap();
        git(&repo, &["add", "app.txt"]);
        git(&repo, &["commit", "-q", "-m", "init"]);
        Self {
            _dir: dir,
            root,
            repo,
            _model: model,
        }
    }

    fn exec(&self, prompt: &str) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_cyber"));
        command
            .args(["exec", "--embedded", "--format", "json", prompt])
            .env_clear()
            .env("HOME", self.root.join("home"))
            .env("CYBER_HOME", self.root.join("cyber-home"))
            .env("CYBER_MODELS_PATH", self.root.join("models.json"))
            .env("PATH", std::env::var("PATH").unwrap_or_default())
            .current_dir(&self.repo);
        #[cfg(windows)]
        for name in ["SystemRoot", "ProgramFiles", "USERPROFILE"] {
            if let Some(value) = std::env::var_os(name) {
                command.env(name, value);
            }
        }
        command.output().unwrap()
    }

    /// The managed checkout `name`, wherever the data directory puts it.
    fn checkout(&self, name: &str) -> Option<PathBuf> {
        let root = self.root.join("cyber-home/data/worktrees");
        std::fs::read_dir(root)
            .into_iter()
            .flatten()
            .map(|project| project.unwrap().path().join(name))
            .find(|path| path.exists())
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

fn branch_exists(repo: &Path, branch: &str) -> bool {
    Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["rev-parse", "--verify", "--quiet"])
        .arg(format!("refs/heads/{branch}"))
        .stdout(std::process::Stdio::null())
        .status()
        .unwrap()
        .success()
}

/// Regression: a plain `cyber exec` whose Session entered a worktree it created and
/// ended there removes the clean checkout and its branch on exit, as `exit_worktree`
/// would; a checkout with changes is kept and its path and branch are printed.
#[test]
fn exec_that_ends_in_an_entered_worktree_cleans_it_up() {
    let env = Env::new();
    let tidy = env.exec("ENTER-TIDY");
    let stderr = String::from_utf8_lossy(&tidy.stderr);
    assert!(tidy.status.success(), "{stderr}");
    assert!(!stderr.contains("worktree kept"), "{stderr}");
    assert!(!stderr.contains("cleanup failed"), "{stderr}");
    assert_eq!(env.checkout("tidy"), None);
    assert!(!branch_exists(&env.repo, "cyber/tidy"));

    let dirty = env.exec("ENTER-DIRTY DIRTY");
    let stderr = String::from_utf8_lossy(&dirty.stderr);
    assert!(dirty.status.success(), "{stderr}");
    let kept = env.checkout("dirty").expect("the changed checkout is kept");
    assert!(kept.join("notes.txt").exists());
    assert!(branch_exists(&env.repo, "cyber/dirty"));
    assert!(
        stderr.contains("worktree kept at") && stderr.contains("cyber/dirty"),
        "{stderr}"
    );
    assert!(!env.repo.join("notes.txt").exists());
}
