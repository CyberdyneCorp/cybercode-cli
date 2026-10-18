//! `cyber git commit-msg` and `cyber git pr-description` (`vcs-integration` → Commit and PR
//! text generation) against the real binary and a local scripted model.
#[path = "../../cyber-app/tests/support/scripted_model.rs"]
mod scripted_model;

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::{Value, json};

/// The small model answers every request; it even tries to add its own trailer.
fn reply(body: &Value) -> Value {
    if body.to_string().contains("Draft a pull request") {
        return json!({"content":"Add a greeting\n\nAdds `greet()`.\n\nCo-authored-by: Model <model@example.com>"});
    }
    json!({"content":"```\nfeat(app): add a greeting\n\nAdds greet().\n\nCo-authored-by: Model <model@example.com>\n```"})
}

struct Repo {
    _dir: tempfile::TempDir,
    root: PathBuf,
    repo: PathBuf,
    model: scripted_model::ScriptedModel,
}

impl Repo {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let model = scripted_model::ScriptedModel::start(reply);
        let mut config = model.config();
        config["providers"]["test"]["models"]["small"] = json!({});
        config["model_roles"] = json!({"small":"test/small"});
        let home = root.join("cyber-home").join("config");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::write(home.join("cyber.json"), config.to_string()).unwrap();
        std::fs::write(root.join("models.json"), "{}").unwrap();
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
            model,
        }
    }

    fn cyber(&self, args: &[&str]) -> Output {
        self.cyber_in(&self.repo, args)
    }

    fn cyber_in(&self, location: &Path, args: &[&str]) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_cyber"));
        command
            .args(args)
            .env_clear()
            .env("HOME", self.root.join("home"))
            .env("CYBER_HOME", self.root.join("cyber-home"))
            .env("CYBER_MODELS_PATH", self.root.join("models.json"))
            .env("PATH", std::env::var("PATH").unwrap_or_default())
            .current_dir(location);
        #[cfg(windows)]
        if let Some(system) = std::env::var_os("SystemRoot") {
            command.env("SystemRoot", system);
        }
        command.output().unwrap()
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

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

/// "No co-author by default": the message carries no trailer, even one the model wrote,
/// while a configured `git.co_author` ends the message with exactly that trailer. The
/// small model receives the staged diff.
#[test]
fn commit_msg_has_no_co_author_by_default() {
    let f = Repo::new();
    std::fs::write(f.repo.join("app.txt"), "one\ngreet\n").unwrap();
    // Nothing staged yet: refused without a model request.
    let refused = f.cyber(&["git", "commit-msg"]);
    assert!(!refused.status.success());
    assert!(
        String::from_utf8_lossy(&refused.stderr).contains("nothing is staged"),
        "{}",
        String::from_utf8_lossy(&refused.stderr)
    );
    assert!(f.model.requests().is_empty());
    git(&f.repo, &["add", "app.txt"]);
    let output = f.cyber(&["git", "commit-msg"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let message = stdout(&output);
    assert_eq!(
        message.trim_end(),
        "feat(app): add a greeting\n\nAdds greet()."
    );
    assert!(!message.to_ascii_lowercase().contains("co-authored-by"));
    let requests = f.model.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0]["model"], "small");
    assert!(requests[0]["messages"].to_string().contains("+greet"));
    assert!(
        requests[0]
            .get("tools")
            .is_none_or(|tools| tools.as_array().is_none_or(Vec::is_empty))
    );

    let configured = f.cyber(&[
        "git",
        "commit-msg",
        "--format",
        "json",
        "--config",
        "git.co_author=Cyber Code <bot@cyber.dev>",
    ]);
    assert!(
        configured.status.success(),
        "{}",
        String::from_utf8_lossy(&configured.stderr)
    );
    let message: Value = serde_json::from_slice(&configured.stdout).unwrap();
    let message = message["message"].as_str().unwrap();
    assert!(
        message.ends_with("\n\nCo-authored-by: Cyber Code <bot@cyber.dev>"),
        "{message}"
    );
    assert_eq!(message.matches("Co-authored-by").count(), 1);
}

/// `git pr-description` drafts a title and body for `base..HEAD` without trailers.
#[test]
fn pr_description_drafts_title_and_body_from_the_branch() {
    let f = Repo::new();
    git(&f.repo, &["checkout", "-q", "-b", "feature"]);
    std::fs::write(f.repo.join("app.txt"), "one\ngreet\n").unwrap();
    git(&f.repo, &["commit", "-q", "-am", "feat: greet"]);
    let output = f.cyber(&["git", "pr-description", "--format", "json"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let pr: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(pr["base"], "main");
    assert_eq!(pr["title"], "Add a greeting");
    assert_eq!(pr["body"], "Adds `greet()`.");
    let requests = f.model.requests();
    let prompt = requests[0]["messages"].to_string();
    assert!(
        prompt.contains("main..HEAD") && prompt.contains("feat: greet"),
        "{prompt}"
    );
}

/// Regression: the staged diff follows the reviewer's read policy before it reaches the
/// model. A staged `.env` (`ask` by default) is named but its contents are withheld, and a
/// credential in another staged file is redacted.
#[test]
fn commit_msg_withholds_unreadable_files_and_redacts_credentials() {
    let f = Repo::new();
    std::fs::write(
        f.repo.join(".env"),
        "DATABASE_URL=postgres://db-secret-value\n",
    )
    .unwrap();
    std::fs::write(
        f.repo.join("deploy.txt"),
        "use ghp_abcdefghij0123456789abcdef to deploy\n",
    )
    .unwrap();
    std::fs::write(f.repo.join("app.txt"), "one\ngreet\n").unwrap();
    git(&f.repo, &["add", ".env", "deploy.txt", "app.txt"]);
    let output = f.cyber(&["git", "commit-msg"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let requests = f.model.requests();
    assert_eq!(requests.len(), 1);
    let sent = requests[0]["messages"].to_string();
    assert!(sent.contains("+greet"), "{sent}");
    assert!(!sent.contains("db-secret-value"), "{sent}");
    assert!(!sent.contains("ghp_abcdefghij0123456789abcdef"), "{sent}");
    assert!(sent.contains("Withheld by read policy"), "{sent}");
    assert!(sent.contains("- .env"), "{sent}");
}

/// Regression: with `diff.relative=true` in the repository configuration, Git would list
/// paths relative to the Location, so a withheld file in a subdirectory escaped its
/// `:(top)` exclusion and its contents reached the model. Paths stay repository-relative.
#[test]
fn commit_msg_from_a_subdirectory_withholds_files_despite_relative_diff_config() {
    let f = Repo::new();
    git(&f.repo, &["config", "diff.relative", "true"]);
    let sub = f.repo.join("sub");
    std::fs::create_dir_all(&sub).unwrap();
    std::fs::write(sub.join(".env"), "TOKEN=relative-secret-value\n").unwrap();
    std::fs::write(sub.join("app.txt"), "greet\n").unwrap();
    git(&f.repo, &["add", "sub/.env", "sub/app.txt"]);
    let output = f.cyber_in(&sub, &["git", "commit-msg"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let requests = f.model.requests();
    assert_eq!(requests.len(), 1);
    let sent = requests[0]["messages"].to_string();
    assert!(sent.contains("+greet"), "{sent}");
    assert!(!sent.contains("relative-secret-value"), "{sent}");
    assert!(!sent.contains("TOKEN="), "{sent}");
    let (_, withheld) = sent.split_once("Withheld by read policy").expect(&sent);
    assert!(withheld.contains("- sub/.env"), "{sent}");
}
