//! The five bundled Workflows on the real binary (`workflows` → Bundled workflow
//! contracts): `cyber exec --embedded --workflow <name>` against a scripted
//! OpenAI-compatible model that answers each agent by the `Task:` line of its prompt.
//! The review command (`cyber review`) and eject/override are covered in
//! `review_command.rs` and `cli.rs`. Unix-only: the fixture repository uses
//! `GIT_CONFIG_GLOBAL=/dev/null` and the migrate checks run the POSIX `true` command.
#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::{Value, json};

#[path = "../../cyber-app/tests/support/scripted_model.rs"]
mod scripted_model;

/// A structured result through the `return_result` tool.
fn result(value: Value) -> Value {
    json!({"tool_calls":[{"index":0,"id":"call_result","type":"function",
        "function":{"name":"return_result","arguments":value.to_string()}}]})
}

fn last_tool_call(body: &Value) -> Option<String> {
    let last = body["messages"].as_array()?.last()?;
    (last["role"] == "tool").then(|| last["tool_call_id"].as_str().unwrap_or_default().to_owned())
}

/// The task named on the first line of the agent's prompt.
fn task(body: &Value) -> String {
    let text = body["messages"].to_string();
    text.find("Task: ")
        .map(|at| {
            text[at + 6..]
                .chars()
                .take_while(|c| *c != '\\' && *c != '"')
                .collect()
        })
        .unwrap_or_default()
}

fn finding(file: &str) -> Value {
    json!({"file": file, "line": 1, "severity": "high", "summary": "unchecked input",
        "failure_scenario": "an empty input panics"})
}

fn reply(body: &Value) -> Value {
    if body.to_string().contains("Write a short title") {
        return json!({"content": "Title"});
    }
    let task = task(body);
    match last_tool_call(body).as_deref() {
        Some("call_result") => return json!({"content": "done"}),
        Some("call_edit") => return migrated(&task),
        _ => {}
    }
    answer(&task).unwrap_or_else(|| json!({"content": "noted"}))
}

fn answer(task: &str) -> Option<Value> {
    let verdict = || result(json!({"refuted": false, "evidence": "confirmed by the code"}));
    let value = match task {
        t if t.starts_with("review ") => {
            result(json!({"findings": [finding("a.rs")], "limitations": []}))
        }
        t if t.starts_with("verify batch") => result(json!({"ok": true, "problems": []})),
        t if t.starts_with("verify ") || t.starts_with("refute ") => verdict(),
        "find chunk 1" => result(json!({"findings": [finding("a.rs")], "limitations": []})),
        t if t.starts_with("find chunk") => result(json!({"findings": [], "limitations": []})),
        t if t.starts_with("migrate batch") => edit(t),
        _ => return research_or_plan(task),
    };
    Some(value)
}

fn research_or_plan(task: &str) -> Option<Value> {
    let value = match task {
        t if t.starts_with("search ") => result(json!({"sources": [
            {"ref": "README.md", "title": "Readme", "why": "overview"},
            {"ref": "a.rs", "title": "Code", "why": "implementation"}
        ]})),
        t if t.starts_with("read source") => result(json!({"claims": [
            {"text": format!("claim of {t}"), "location": "line 1"}
        ]})),
        t if t.starts_with("cross-check") => result(json!({
            "supported_by": ["README.md", "a.rs"], "contradicted_by": [], "confidence": "high"
        })),
        "synthesize answer" => json!({"content": "The answer [1]."}),
        t if t.starts_with("draft ") => result(json!({"title": t, "steps": [
            {"step": "change a.rs", "files": ["a.rs"], "risk": "low"}
        ], "assumptions": [], "risks": [], "tests": ["cargo test"]})),
        t if t.starts_with("judge") => {
            let row = |angle: &str, n: u64| {
                json!({"angle": angle, "reasons": "r", "correctness": n,
                "completeness": n, "risk": n, "effort": n, "testability": n})
            };
            result(
                json!({"scores": [row("mvp-first", 3), row("risk-first", 5), row("user-first", 2)]}),
            )
        }
        "merge plans" => result(json!({"plan_markdown": "# Plan", "grafts": [
            {"from": "user-first", "what": "a usability check"}
        ]})),
        _ => return None,
    };
    Some(value)
}

/// The file a migrate batch edits: batch 1 is `a.rs`, batch 2 `b.rs`.
fn batch_file(task: &str) -> &'static str {
    if task.ends_with('1') { "a.rs" } else { "b.rs" }
}

/// A migrate batch edits its file and commits on its branch with one shell command.
fn edit(task: &str) -> Value {
    let file = batch_file(task);
    let command = format!(
        "printf 'fn migrated() {{}}\\n' > {file} && git add {file} && git -c user.name=M -c user.email=m@example.invalid commit -qm 'migrate {file}'"
    );
    json!({"tool_calls":[{"index":0,"id":"call_edit","type":"function",
        "function":{"name":"bash","arguments":json!({"command": command}).to_string()}}]})
}

fn migrated(task: &str) -> Value {
    result(json!({"status": "done", "changed": [batch_file(task)],
        "check": [{"command": "true", "exit_code": 0, "tail": ""}], "notes": ""}))
}

struct Env {
    _dir: tempfile::TempDir,
    root: PathBuf,
    repo: PathBuf,
    _model: scripted_model::ScriptedModel,
}

fn git(repo: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(args)
        .current_dir(repo)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .output()
        .unwrap();
    assert!(output.status.success(), "git {args:?}: {output:?}");
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

impl Env {
    /// A committed repository with `a.rs` and `b.rs`, both changed since; every action
    /// is allowed and commands run unsandboxed (the migrate agents commit in worktrees).
    fn new() -> Self {
        let model = scripted_model::ScriptedModel::start(reply);
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let repo = root.join("repo");
        std::fs::create_dir_all(root.join("cyber/config")).unwrap();
        std::fs::create_dir_all(root.join("home")).unwrap();
        std::fs::create_dir_all(&repo).unwrap();
        let mut config = model.config();
        config["permissions"] = json!("allow");
        config["sandbox"] = json!({"policy": "full-access"});
        std::fs::write(root.join("cyber/config/cyber.jsonc"), config.to_string()).unwrap();
        std::fs::write(root.join("models.json"), "{}").unwrap();
        git(&repo, &["init", "--quiet", "-b", "main"]);
        git(&repo, &["config", "user.name", "Test"]);
        git(&repo, &["config", "user.email", "test@example.invalid"]);
        std::fs::write(repo.join("a.rs"), "fn a() {}\n").unwrap();
        std::fs::write(repo.join("b.rs"), "fn b() {}\n").unwrap();
        std::fs::write(repo.join("README.md"), "# Demo\n").unwrap();
        git(&repo, &["add", "-A"]);
        git(&repo, &["commit", "--quiet", "-m", "baseline"]);
        Self {
            _dir: dir,
            root,
            repo,
            _model: model,
        }
    }

    fn exec(&self, workflow: &str, args: Value, extra: &[&str]) -> Output {
        let exe = PathBuf::from(env!("CARGO_BIN_EXE_cyber"));
        let helper = exe.with_file_name(if cfg!(windows) {
            "cyber-sandbox-exec.exe"
        } else {
            "cyber-sandbox-exec"
        });
        let args = args.to_string();
        Command::new(&exe)
            .args([
                "exec",
                "--embedded",
                "--workflow",
                workflow,
                "--args",
                &args,
                "--format",
                "json",
            ])
            .args(extra)
            .env_clear()
            .env("HOME", self.root.join("home"))
            .env("CYBER_HOME", self.root.join("cyber"))
            .env("CYBER_MODELS_PATH", self.root.join("models.json"))
            .env("CYBER_SANDBOX_HELPER", helper)
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .current_dir(&self.repo)
            .output()
            .unwrap()
    }
}

/// The run's result value, after checking the exit code.
fn ran(output: &Output, code: i32) -> Value {
    let detail = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.status.code(), Some(code), "{detail}");
    let report: Value =
        serde_json::from_slice(&output.stdout).unwrap_or_else(|e| panic!("{e}: {detail}"));
    report["result"].clone()
}

#[test]
fn review_verifies_its_findings_on_a_two_file_diff() {
    let env = Env::new();
    std::fs::write(env.repo.join("a.rs"), "fn a(x: u8) {}\n").unwrap();
    std::fs::write(env.repo.join("b.rs"), "fn b(y: u8) {}\n").unwrap();
    let result = ran(
        &env.exec("review", json!({"lenses": ["correctness", "tests"]}), &[]),
        0,
    );
    assert_eq!(result["findings"], json!([finding("a.rs")]), "{result}");
    assert_eq!(result["refuted"], 0);
    assert_eq!(
        result["lenses"]["correctness"],
        json!({"raised": 1, "confirmed": 1})
    );
    assert_eq!(result["partial"], false);
}

#[test]
fn audit_reports_coverage_and_a_turn_budget_stops_it_with_exit_4() {
    let env = Env::new();
    let result = ran(&env.exec("audit", json!({"chunk_files": 1}), &[]), 0);
    assert_eq!(result["coverage"]["files_scanned"], 3, "{result}");
    assert_eq!(result["coverage"]["chunks"], 3);
    assert_eq!(result["findings"].as_array().unwrap().len(), 1);
    assert_eq!(result["partial"], false);

    let stopped = ran(
        &env.exec("audit", json!({"chunk_files": 1}), &["--max-turns", "1"]),
        4,
    );
    assert_eq!(stopped["partial"], true, "{stopped}");
    assert!(
        stopped["coverage"]["files_skipped"]
            .as_array()
            .unwrap()
            .iter()
            .any(|skipped| skipped["reason"] == "budget"),
        "{stopped}"
    );
}

#[test]
fn migrate_commits_each_batch_on_its_own_branch_and_merges_nothing() {
    let env = Env::new();
    let args = json!({"instructions": "rename to migrated", "files": ["a.rs", "b.rs"],
        "batch_size": 1, "check": ["true"]});
    let result = ran(&env.exec("migrate", args, &[]), 0);
    let batches = result["batches"].as_array().unwrap();
    assert_eq!(batches.len(), 2, "{result}");
    for batch in batches {
        assert_eq!(batch["status"], "done", "{batch}");
        assert_eq!(batch["verified"], true, "{batch}");
        let branch = batch["branch"].as_str().unwrap();
        let file = batch["files"][0].as_str().unwrap();
        let shown = git(&env.repo, &["show", &format!("{branch}:{file}")]);
        assert_eq!(shown, "fn migrated() {}", "{branch}");
    }
    assert_eq!(result["merged"], false);
    assert!(
        result["integrate"]
            .as_str()
            .unwrap()
            .starts_with("git merge ")
    );
    // The owner's checkout is unchanged.
    assert_eq!(
        std::fs::read_to_string(env.repo.join("a.rs")).unwrap(),
        "fn a() {}\n"
    );
    assert_eq!(git(&env.repo, &["status", "--porcelain"]), "");
}

#[test]
fn research_answers_from_cross_checked_claims() {
    let env = Env::new();
    let args = json!({"question": "what does a.rs do?", "sources": ["repo"], "angles": 1});
    let result = ran(&env.exec("research", args, &[]), 0);
    assert_eq!(result["answer"], "The answer [1].", "{result}");
    assert_eq!(result["sources"].as_array().unwrap().len(), 2);
    assert!(
        result["claims"]
            .as_array()
            .unwrap()
            .iter()
            .all(|c| c["verified"] == true)
    );
}

#[test]
fn plan_merges_the_highest_scoring_draft() {
    let env = Env::new();
    let result = ran(&env.exec("plan", json!({"task": "add a cache"}), &[]), 0);
    assert_eq!(result["chosen"], "risk-first", "{result}");
    assert_eq!(
        result["grafts"],
        json!([{"from": "user-first", "what": "a usability check"}])
    );
    assert_eq!(result["plan_markdown"], "# Plan");
}
