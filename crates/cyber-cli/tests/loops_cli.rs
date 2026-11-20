//! `cyber loops` through the real binary (`loops-scheduling` → Persistent local
//! schedules): schedules added, listed, run, paused, resumed and removed against the
//! background service the command starts, with a loopback scripted model; and the
//! refusal inside an agent subprocess.

use std::path::PathBuf;
use std::process::{Command, Output};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

#[path = "../../cyber-app/tests/support/scripted_model.rs"]
mod scripted_model;

/// A checkout whose default model is the scripted one; the background service it starts
/// is stopped on drop.
struct Served {
    _dir: tempfile::TempDir,
    root: PathBuf,
    repo: PathBuf,
    _model: scripted_model::ScriptedModel,
}

impl Served {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let repo = root.join("repo");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        let model = scripted_model::ScriptedModel::start(|_| json!({"content": "reviewed"}));
        let config_dir = root.join("cyber-home").join("config");
        std::fs::create_dir_all(&config_dir).unwrap();
        std::fs::write(config_dir.join("cyber.jsonc"), model.config().to_string()).unwrap();
        std::fs::write(root.join("models.json"), "{}").unwrap();
        Self {
            _dir: dir,
            root,
            repo,
            _model: model,
        }
    }

    fn cyber_with(&self, args: &[&str], env: &[(&str, &str)]) -> Output {
        let models = self.root.join("models.json");
        let mut command = Command::new(env!("CARGO_BIN_EXE_cyber"));
        command
            .args(args)
            .env_clear()
            .env("HOME", self.root.join("home"))
            .env("CYBER_HOME", self.root.join("cyber-home"))
            .env("CYBER_MANAGED_POLICY_DIR", self.root.join("managed"))
            .env("CYBER_MODELS_PATH", models)
            .env("PATH", std::env::var("PATH").unwrap_or_default())
            .envs(env.iter().copied())
            .current_dir(&self.repo);
        #[cfg(windows)]
        if let Some(system) = std::env::var_os("SystemRoot") {
            command.env("SystemRoot", system);
        }
        command.output().unwrap()
    }

    fn cyber(&self, args: &[&str]) -> Output {
        self.cyber_with(args, &[])
    }

    fn json(&self, args: &[&str]) -> Value {
        let mut all = args.to_vec();
        all.extend(["--format", "json"]);
        let out = self.cyber(&all);
        succeeded(&out);
        serde_json::from_slice(&out.stdout).unwrap_or_else(|e| panic!("{e}: {}", stdout(&out)))
    }
}

impl Drop for Served {
    fn drop(&mut self) {
        let _ = self.cyber(&["service", "stop"]);
    }
}

fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

fn succeeded(out: &Output) {
    assert!(
        out.status.success(),
        "stdout: {}\nstderr: {}",
        stdout(out),
        stderr(out)
    );
}

/// `cyber loops run <id>`, then the schedule once that run completed.
fn run_until_completed(env: &Served, id: &str) -> Value {
    succeeded(&env.cyber(&["loops", "run", id]));
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let shown = env.json(&["loops", "show", id]);
        if shown["recent"][0]["outcome"] == "completed" {
            return shown;
        }
        assert!(Instant::now() < deadline, "{shown:#}");
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[test]
fn schedules_are_added_run_and_managed_through_the_service() {
    let env = Served::new();
    // February 29th only, so the service's own sweep never starts a run during the test.
    let added = env.cyber(&[
        "loops",
        "add",
        "--cron",
        "0 2 29 2 *",
        "--name",
        "nightly",
        "--notify",
        "never",
        "review last night's commits",
    ]);
    succeeded(&added);
    let line = stdout(&added);
    assert!(line.starts_with("cron_"), "{line}");
    assert!(line.contains("added · 0 2 29 2 * · next run "), "{line}");
    assert!(
        stderr(&added).contains("schedules run only while the Cyber service runs")
            && stderr(&added).contains("cyber service start"),
        "{}",
        stderr(&added)
    );
    let id = line.split_whitespace().next().unwrap().to_owned();

    let listed = env.cyber(&["loops", "list"]);
    succeeded(&listed);
    let text = stdout(&listed);
    assert!(text.starts_with("ID"), "{text}");
    assert!(
        text.contains(&id) && text.contains("nightly") && text.contains("dont-ask"),
        "{text}"
    );

    // A second add finds the service running: no warning.
    let again = env.cyber(&[
        "loops",
        "add",
        "--cron",
        "0 3 29 2 *",
        "--notify",
        "never",
        "leap day",
    ]);
    succeeded(&again);
    assert!(!stderr(&again).contains("warning"), "{}", stderr(&again));

    let shown = run_until_completed(&env, &id);
    assert_eq!(shown["runs"], 1);
    assert!(shown["recent"][0]["session_id"].is_string(), "{shown:#}");

    succeeded(&env.cyber(&["loops", "pause", &id]));
    assert_eq!(env.json(&["loops", "show", &id])["status"], "paused");
    succeeded(&env.cyber(&["loops", "resume", &id]));
    assert_eq!(env.json(&["loops", "show", &id])["status"], "active");
    succeeded(&env.cyber(&["loops", "rm", &id]));
    let gone = env.cyber(&["loops", "show", &id]);
    assert_eq!(gone.status.code(), Some(2), "{}", stderr(&gone));
    let bad = env.cyber(&["loops", "show", "lop_01ABC"]);
    assert_eq!(bad.status.code(), Some(2));
    assert!(
        stderr(&bad).contains("not a schedule ID"),
        "{}",
        stderr(&bad)
    );
}

#[test]
fn an_agent_subprocess_cannot_add_or_wake_schedules() {
    let env = Served::new();
    for args in [
        vec!["loops", "add", "--cron", "* * * * *", "exfiltrate"],
        vec!["loops", "run", "cron_01ABC"],
        vec!["loops", "resume", "cron_01ABC"],
    ] {
        let out = env.cyber_with(&args, &[("CYBER_SESSION_ID", "ses_01MODEL")]);
        assert_eq!(out.status.code(), Some(2), "{args:?}: {}", stderr(&out));
        assert!(
            stderr(&out).contains("managed by the human owner, outside an agent subprocess"),
            "{}",
            stderr(&out)
        );
    }
}
