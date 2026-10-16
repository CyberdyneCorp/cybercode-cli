//! End-to-end tests of the `cyber` binary (`cli-commands`, `workspace-trust`).

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::Value;

#[path = "../../cyber-app/tests/support/scripted_model.rs"]
mod scripted_model;

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
    let o = Env::new().cyber(&["goals", "list"]);
    assert_eq!(o.status.code(), Some(2));
    assert!(
        stderr(&o).contains("`cyber goals` is not available in this build yet"),
        "{}",
        stderr(&o)
    );
}

fn saved_workflow(path: &Path, name: &str, description: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        path,
        format!(
            "export const meta = {{ name: {name:?}, description: {description:?}, phases: [{{ title: 'Scan' }}] }};\n\
             export default async function () {{ return await agent('scan'); }}\n"
        ),
    )
    .unwrap();
}

#[test]
fn workflows_list_shows_saved_workflows_with_project_overrides_and_parse_errors() {
    let env = Env::new();
    let repo = env.repo("{}");
    let global = env.root.join("cyber-home/config/workflows");
    saved_workflow(&global.join("audit.js"), "audit", "Global audit");
    saved_workflow(&global.join("plan.js"), "plan", "Plan from angles");
    saved_workflow(
        &repo.join(".cyber/workflows/audit.js"),
        "audit",
        "Audit auth endpoints",
    );
    saved_workflow(
        &repo.join(".cyber/workflows/nested/review.js"),
        "review",
        "Multi-angle review",
    );
    std::fs::write(
        repo.join(".cyber/workflows/computed.js"),
        "const v = 1;\nexport const meta = { name: `audit-${v}`, description: 'x' };\n",
    )
    .unwrap();

    let o = env
        .command(&["workflows", "list"])
        .current_dir(&repo)
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", stderr(&o));
    let text = stdout(&o);
    let lines: Vec<&str> = text.lines().collect();
    assert!(
        lines.iter().any(|l| l.starts_with("audit")
            && l.contains("[project]")
            && l.contains("Audit auth endpoints")),
        "{text}"
    );
    assert!(
        lines.iter().any(|l| l.starts_with("plan")
            && l.contains("[global]")
            && l.contains("Plan from angles")),
        "{text}"
    );
    assert!(
        lines
            .iter()
            .any(|l| l.starts_with("review") && l.contains("Multi-angle review")),
        "{text}"
    );
    assert!(!text.contains("Global audit"), "{text}");
    assert!(
        text.contains("computed.js: ParseError: meta must be a pure literal (2:29)"),
        "{text}"
    );

    let o = env
        .command(&["workflows", "list", "--format", "json"])
        .current_dir(&repo)
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", stderr(&o));
    let listing = json(&o);
    let names: Vec<&str> = listing["workflows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|w| w["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["audit", "plan", "review"]);
    let audit = &listing["workflows"][0];
    assert_eq!(audit["scope"], "project");
    assert_eq!(audit["phases"][0]["title"], "Scan");
    assert!(audit["digest"].as_str().unwrap().starts_with("sha256:"));
    assert_eq!(audit["shadowed"].as_array().unwrap().len(), 1);
    assert_eq!(listing["issues"][0]["diagnostic"]["kind"], "ParseError");
    assert_eq!(listing["issues"][0]["diagnostic"]["position"]["line"], 2);
}

/// A shared server the test started through the CLI, stopped when the test ends.
struct StartedService<'a>(&'a Env, PathBuf);

impl StartedService<'_> {
    fn cyber(&self, args: &[&str]) -> Output {
        self.0
            .command(args)
            .env("CYBER_MODELS_PATH", &self.1)
            .output()
            .unwrap()
    }
}

impl Drop for StartedService<'_> {
    fn drop(&mut self) {
        let _ = self.cyber(&["service", "stop"]);
    }
}

/// Assert an exit code, showing both output streams on failure.
fn exits(output: &Output, code: i32) -> String {
    assert_eq!(
        output.status.code(),
        Some(code),
        "{}{}",
        stdout(output),
        stderr(output)
    );
    format!("{}{}", stdout(output), stderr(output))
}

/// A saved Workflow `x` (one agent) and a throwing `boom.js`, with the scripted model as
/// the global default.
fn workflow_env(env: &Env, model: &scripted_model::ScriptedModel) -> PathBuf {
    let catalog = env.root.join("models.json");
    std::fs::write(&catalog, "{}").unwrap();
    let config = env.root.join("cyber-home/config");
    std::fs::create_dir_all(config.join("workflows")).unwrap();
    std::fs::write(config.join("cyber.jsonc"), model.config().to_string()).unwrap();
    std::fs::write(
        config.join("workflows/x.js"),
        "export const meta = { name: 'x', description: 'One agent', phases: [{ title: 'Ask' }] };\nexport default async () => {\n  phase('Ask');\n  const answer = await agent('say hello', { label: 'greeter' });\n  log('answer', answer.text);\n  return { text: answer.text };\n};\n",
    )
    .unwrap();
    std::fs::write(
        env.root.join("boom.js"),
        "export const meta = { name: 'boom', description: 'Throws' };\nexport default async () => { throw new Error('boom'); };\n",
    )
    .unwrap();
    catalog
}

/// The completed run's ID, after checking the Location lists all three runs.
fn listed_runs(service: &StartedService<'_>) -> String {
    let runs = json(&service.cyber(&["workflows", "runs", "--format", "json"]));
    let runs = runs.as_array().unwrap();
    let mut statuses: Vec<&str> = runs
        .iter()
        .map(|run| run["status"].as_str().unwrap())
        .collect();
    statuses.sort_unstable();
    assert_eq!(statuses, ["budget_exceeded", "completed", "failed"]);
    let id = runs
        .iter()
        .find(|run| run["status"] == "completed")
        .and_then(|run| run["id"].as_str())
        .unwrap()
        .to_owned();
    assert!(exits(&service.cyber(&["workflows", "runs"]), 0).contains(&id));
    id
}

fn inspect_run(service: &StartedService<'_>, id: &str) {
    let shown = exits(&service.cyber(&["workflows", "show", id]), 0);
    assert!(shown.contains("1 done, 0 running, 0 failed"), "{shown}");
    assert!(shown.contains("greeter"), "{shown}");
    let shown = json(&service.cyber(&["workflows", "show", id, "--format", "json"]));
    assert_eq!(shown["agents"][0]["status"], "completed");
    let logs = service.cyber(&["workflows", "logs", id]);
    exits(&logs, 0);
    assert_eq!(stdout(&logs).trim(), "answer agent done");
    let resumed = exits(&service.cyber(&["workflows", "resume", id]), 1);
    assert!(resumed.contains("completed"), "{resumed}");
}

#[test]
fn workflows_run_against_the_shared_server_and_wait_maps_exit_codes() {
    let model =
        scripted_model::ScriptedModel::start(|_| serde_json::json!({"content": "agent done"}));
    let env = Env::new();
    let service = StartedService(&env, workflow_env(&env, &model));

    let done = service.cyber(&["workflows", "run", "x", "--wait"]);
    let text = exits(&done, 0);
    assert!(stdout(&done).contains("agent done"), "{text}");
    assert!(
        text.contains("phase: Ask") && text.contains("agent greeter completed"),
        "{text}"
    );
    let failed = exits(
        &service.cyber(&["workflows", "run", "boom.js", "--wait"]),
        1,
    );
    assert!(failed.contains("boom"), "{failed}");
    let capped = service.cyber(&["workflows", "run", "x", "--max-agents", "0", "--wait"]);
    assert!(exits(&capped, 4).contains("budget_exceeded"));
    exits(
        &service.cyber(&["workflows", "run", "x", "--args", "[1]"]),
        2,
    );

    let id = listed_runs(&service);
    inspect_run(&service, &id);

    let remote = service.cyber(&["workflows", "run", "x", "--runner", "r1", "--wait"]);
    let remote = exits(&remote, 2);
    assert!(
        remote.contains("`cyber workflows run --runner` is not available in this build yet"),
        "{remote}"
    );
}

#[test]
fn a_workflow_run_deny_rule_refuses_cyber_workflows_run() {
    let model =
        scripted_model::ScriptedModel::start(|_| serde_json::json!({"content": "agent done"}));
    let env = Env::new();
    let catalog = workflow_env(&env, &model);
    let mut config = model.config();
    config["permissions"] = serde_json::json!({"workflow.run": {"x": "deny"}});
    std::fs::write(
        env.root.join("cyber-home/config/cyber.jsonc"),
        config.to_string(),
    )
    .unwrap();
    let service = StartedService(&env, catalog);
    let denied = exits(&service.cyber(&["workflows", "run", "x", "--wait"]), 1);
    assert!(denied.contains("workflow.run is denied for x"), "{denied}");
    assert!(model.requests().is_empty(), "no agent may start");
    // Other workflows still start.
    exits(
        &service.cyber(&["workflows", "run", "boom.js", "--wait"]),
        1,
    );
    let runs = json(&service.cyber(&["workflows", "runs", "--format", "json"]));
    let names: Vec<&str> = runs
        .as_array()
        .unwrap()
        .iter()
        .map(|run| run["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["boom"]);
}

/// Ctrl-C while `--wait` sleeps between two polls stops the run and exits 130.
#[cfg(unix)]
#[test]
fn ctrl_c_between_polls_stops_a_waited_run() {
    use std::process::Stdio;
    use std::time::{Duration, Instant};
    let model = scripted_model::ScriptedModel::start(|_| serde_json::json!({"hold": true}));
    let env = Env::new();
    let service = StartedService(&env, workflow_env(&env, &model));
    let mut child = service
        .0
        .command(&["workflows", "run", "x", "--wait"])
        .env("CYBER_MODELS_PATH", &service.1)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(60);
    while model.requests().is_empty() {
        assert!(
            Instant::now() < deadline,
            "the agent never reached the model"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    // Polls take a few milliseconds and the pause between them 200: the signal almost
    // always lands while the command sleeps.
    std::thread::sleep(Duration::from_millis(700));
    let pid = child.id().to_string();
    assert!(
        Command::new("kill")
            .args(["-INT", &pid])
            .status()
            .unwrap()
            .success()
    );
    let deadline = Instant::now() + Duration::from_secs(30);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            model.release();
            panic!("Ctrl-C did not end `cyber workflows run --wait`");
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let output = child.wait_with_output().unwrap();
    model.release();
    assert_eq!(status.code(), Some(130), "{}", stderr(&output));
    assert!(
        stderr(&output).contains("interrupted; stopped"),
        "{}",
        stderr(&output)
    );
    let runs = json(&service.cyber(&["workflows", "runs", "--format", "json"]));
    assert_eq!(runs[0]["status"], "stopped", "{runs}");
}

/// The first agent answers; the second waits for [`ScriptedModel::release`].
#[cfg(unix)]
fn first_then_held(body: &Value) -> Value {
    if body.to_string().contains("second step") {
        return serde_json::json!({"hold": true});
    }
    serde_json::json!({"content": "first done"})
}

#[cfg(unix)]
fn requests_with(model: &scripted_model::ScriptedModel, needle: &str) -> usize {
    let requests = model.requests();
    requests
        .iter()
        .filter(|r| r.to_string().contains(needle))
        .count()
}

/// Poll `cyber workflows show` until `done` accepts the run.
#[cfg(unix)]
fn await_run(service: &StartedService<'_>, id: &str, done: impl Fn(&Value) -> bool) -> Value {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    loop {
        let run = json(&service.cyber(&["workflows", "show", id, "--format", "json"]));
        if done(&run) {
            return run;
        }
        assert!(std::time::Instant::now() < deadline, "{run}");
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

/// A server killed with SIGKILL while a workflow agent runs never releases that agent's
/// child ownership. The next server releases it, so resuming the run replays the
/// settled agent without a model call and continues the interrupted child.
#[cfg(unix)]
#[test]
fn a_run_resumes_after_the_server_is_killed_mid_agent() {
    let model = scripted_model::ScriptedModel::start(first_then_held);
    let env = Env::new();
    let service = StartedService(&env, workflow_env(&env, &model));
    std::fs::write(
        env.root.join("cyber-home/config/workflows/steps.js"),
        "export const meta = { name: 'steps', description: 'Two steps' };\nexport default async () => {\n  const a = await agent('first step');\n  const b = await agent('second step');\n  return { a: a.text, b: b.text };\n};\n",
    )
    .unwrap();
    let started = service.cyber(&["workflows", "run", "steps"]);
    exits(&started, 0);
    let id = stdout(&started).trim().to_owned();
    await_run(&service, &id, |run| {
        run["agents"]
            .as_array()
            .is_some_and(|agents| agents.len() == 2)
    });
    let registration: Value = serde_json::from_slice(
        &std::fs::read(env.root.join("cyber-home/state/server.json")).unwrap(),
    )
    .unwrap();
    let pid = registration["pid"].to_string();
    assert!(
        Command::new("kill")
            .args(["-KILL", &pid])
            .status()
            .unwrap()
            .success()
    );
    // Let the held request end; the resumed child asks again.
    model.release();

    let paused = await_run(&service, &id, |run| run["status"] == "paused");
    assert_eq!(paused["agents"][1]["status"], "interrupted", "{paused}");
    exits(&service.cyber(&["workflows", "resume", &id]), 0);
    let done = await_run(&service, &id, |run| run["status"] != "running");
    assert_eq!(done["status"], "completed", "{done}");
    assert_eq!(done["result"]["a"], "first done", "{done}");
    assert_eq!(done["result"]["b"], "released answer", "{done}");
    assert_eq!(
        requests_with(&model, "first step"),
        1,
        "replayed, not asked again"
    );
    // The interrupted child continued: no replacement child was spawned.
    let owner = done["session_id"].as_str().unwrap();
    let children = json(&service.cyber(&[
        "api",
        "v1.session.children",
        "--param",
        &format!("id={owner}"),
    ]));
    assert_eq!(
        children["data"]["data"].as_array().unwrap().len(),
        2,
        "{children}"
    );
}

#[test]
fn review_is_a_shipped_command_that_documents_its_target_grammar() {
    let env = Env::new();
    let help = env.cyber(&["review", "--help"]);
    assert!(help.status.success(), "{}", stderr(&help));
    for text in [
        "<base>..<head>",
        "pr <number>",
        "--fail-on-deny",
        "--worktree",
    ] {
        assert!(stdout(&help).contains(text), "{text}: {}", stdout(&help));
    }
    // The subcommand wins over a directory named `review`; `./review` opens the TUI.
    std::fs::create_dir_all(env.root.join("review")).unwrap();
    let invalid = env.cyber(&["review", "a...b"]);
    assert_eq!(invalid.status.code(), Some(2));
    assert!(
        stderr(&invalid).contains("three-dot"),
        "{}",
        stderr(&invalid)
    );
    assert!(!stderr(&invalid).contains("not available in this build"));
    let tui = env.cyber(&["./review"]);
    assert_eq!(tui.status.code(), Some(2));
    assert!(
        stderr(&tui).contains("the TUI needs a terminal"),
        "{}",
        stderr(&tui)
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
        names
            .iter()
            .copied()
            .filter(|name| !name.starts_with("lsp:"))
            .collect::<Vec<_>>(),
        vec![
            "config",
            "skills",
            "commands",
            "instructions",
            "catalog",
            "credentials",
            "database",
            "sandbox",
            "git",
            "rg",
            "server"
        ]
    );
    assert!(names.contains(&"lsp:gopls"));
    assert_eq!(
        names.iter().filter(|name| name.starts_with("lsp:")).count(),
        14
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
    let absent = env.root.join("absent-review.db");
    let output = env
        .command(&["hooks", "list", "--format", "json"])
        .env("CYBER_DB", &absent)
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", stderr(&output));
    let value = json(&output);
    assert!(
        !absent.exists(),
        "review must not create a missing database"
    );
    assert!(value["hooks"][0]["last_run"].is_null());
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

fn status_candidate(env: &Env, name: &str) {
    let path = env.root.join("cyber-home/cache/bin").join(name);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, "Discovery must not execute candidates").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
}

fn status_row<'a>(rows: &'a Value, id: &str) -> &'a Value {
    rows.as_array()
        .unwrap()
        .iter()
        .find(|row| row["id"] == id)
        .unwrap()
}

#[test]
fn lsp_status_lists_local_integrations_without_database_or_process_startup() {
    let env = Env::new();
    status_candidate(&env, "rust-analyzer");
    let run = |args: &[&str]| {
        env.command(args)
            .env("PATH", env.root.join("empty-path"))
            .output()
            .unwrap()
    };
    let output = run(&["lsp", "status", "--format", "json"]);
    assert!(output.status.success(), "{}", stderr(&output));
    let rows = json(&output);
    assert_eq!(rows.as_array().unwrap().len(), 14);
    let rust = status_row(&rows, "rust-analyzer");
    assert_eq!(rust["enabled"], true);
    assert_eq!(rust["installed"], true);
    assert!(rust["running"].is_null());
    assert!(rust["roots"].is_null());
    let human = run(&["lsp", "status"]);
    assert!(stdout(&human).contains("ENABLED"));
    assert!(stdout(&human).contains("INSTALLED"));
    assert!(stdout(&human).contains("RUNNING"));
    assert!(stdout(&human).contains("unknown"));
    assert!(!env.root.join("cyber-home/data/cyber-dev.db").exists());
}

fn register_status_listener(env: &Env, url: &str, socket: Option<&Path>) {
    let state = env.root.join("cyber-home/state");
    std::fs::create_dir_all(&state).unwrap();
    std::fs::write(state.join("password"), "status-password\n").unwrap();
    std::fs::write(
        state.join("server.json"),
        serde_json::json!({
            "id":"srv_status","version":"test","url":url,"socket":socket,"pid":std::process::id()
        })
        .to_string(),
    )
    .unwrap();
}

fn status_reply(
    mut stream: impl std::io::Read + std::io::Write,
    body: Value,
    status: &str,
) -> String {
    let mut request = Vec::new();
    let mut buffer = [0; 1024];
    while !request.windows(4).any(|bytes| bytes == b"\r\n\r\n") {
        let count = stream.read(&mut buffer).unwrap();
        assert!(count > 0 && request.len() < 16384);
        request.extend_from_slice(&buffer[..count]);
    }
    let body = body.to_string();
    write!(stream, "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
    String::from_utf8(request).unwrap()
}

fn tcp_status(env: &Env, body: Value, status: &'static str) -> std::thread::JoinHandle<String> {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    register_status_listener(
        env,
        &format!("http://{}", listener.local_addr().unwrap()),
        None,
    );
    listener.set_nonblocking(true).unwrap();
    std::thread::spawn(move || {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            if let Ok((stream, _)) = listener.accept() {
                stream.set_nonblocking(false).unwrap();
                stream
                    .set_read_timeout(Some(std::time::Duration::from_secs(2)))
                    .unwrap();
                return status_reply(stream, body, status);
            }
            if std::time::Instant::now() >= deadline {
                return String::new();
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    })
}

#[test]
fn lsp_status_joins_authenticated_live_roots_and_retains_removed_server_ids() {
    let env = Env::new();
    let child = env.root.join("nested");
    std::fs::create_dir(&child).unwrap();
    let server = tcp_status(
        &env,
        serde_json::json!({"location":{"directory":env.root},"data":[
            {"id":"rust-analyzer","root":env.root,"status":"connected"},
            {"id":"rust-analyzer","root":child,"status":"starting"},
            {"id":"removed-custom","root":env.root,"status":"broken"}
        ]}),
        "200 OK",
    );
    let output = env.cyber(&["lsp", "status", "--format", "json"]);
    let request = server.join().unwrap();
    assert!(output.status.success(), "{}", stderr(&output));
    let rows = json(&output);
    assert_eq!(status_row(&rows, "rust-analyzer")["running"], true);
    assert_eq!(
        status_row(&rows, "rust-analyzer")["roots"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    let removed = status_row(&rows, "removed-custom");
    assert!(
        removed["running"].is_null()
            && removed["enabled"].is_null()
            && removed["installed"].is_null()
    );
    assert_eq!(removed["roots"][0]["status"], "broken");
    let request = request.to_ascii_lowercase();
    assert!(request.starts_with("get /api/v1/lsp?"));
    assert!(request.contains("location%5bdirectory%5d="));
    assert!(request.contains("authorization: basic y3lizxi6c3rhdhvzlxbhc3n3b3jk"));
    assert!(!env.root.join("cyber-home/data/cyber-dev.db").exists());
}

#[test]
fn lsp_status_live_empty_snapshot_is_stopped_and_human_roots_are_visible() {
    let env = Env::new();
    let server = tcp_status(
        &env,
        serde_json::json!({"location":{"directory":env.root},"data":[]}),
        "200 OK",
    );
    let output = env.cyber(&["lsp", "status", "--format", "json"]);
    server.join().unwrap();
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(
        status_row(&json(&output), "rust-analyzer")["running"],
        false
    );
    let server = tcp_status(
        &env,
        serde_json::json!({"location":{"directory":env.root},"data":[{"id":"rust-analyzer","root":env.root,"status":"starting"}]}),
        "200 OK",
    );
    let output = env.cyber(&["lsp", "status"]);
    server.join().unwrap();
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(stdout(&output).contains("unknown"));
    assert!(
        stdout(&output).contains("starting:")
            && stdout(&output).contains(
                &env.root
                    .display()
                    .to_string()
                    .chars()
                    .flat_map(char::escape_debug)
                    .collect::<String>(),
            )
    );
}

#[test]
fn lsp_status_refuses_unavailable_foreign_and_invalid_live_snapshots() {
    let env = Env::new();
    let cases = [
        serde_json::json!({"location":{"directory":"/another-location"},"data":[]}),
        serde_json::json!({"location":{"directory":env.root},"data":[{"id":"fixture","root":env.root.join("../outside"),"status":"connected"}]}),
        serde_json::json!({"location":{"directory":env.root},"data":[{"id":"fixture","root":env.root,"status":"invented"}]}),
    ];
    for body in cases {
        let server = tcp_status(&env, body, "200 OK");
        let output = env.cyber(&["lsp", "status", "--format", "json"]);
        server.join().unwrap();
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert!(!stderr(&output).contains("status-password"));
    }
    let server = tcp_status(
        &env,
        serde_json::json!({"message":"private-server-error"}),
        "401 Unauthorized",
    );
    let output = env.cyber(&["lsp", "status"]);
    server.join().unwrap();
    assert!(!output.status.success());
    assert!(!stderr(&output).contains("private-server-error"));
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    register_status_listener(
        &env,
        &format!("http://{}", listener.local_addr().unwrap()),
        None,
    );
    drop(listener);
    let output = env.cyber(&["lsp", "status"]);
    assert!(!output.status.success());
    assert!(stderr(&output).contains("unavailable"));
}

#[cfg(unix)]
fn prepare_status_socket(stream: std::os::unix::net::UnixStream) -> std::os::unix::net::UnixStream {
    stream.set_nonblocking(false).unwrap();
    stream
        .set_read_timeout(Some(std::time::Duration::from_secs(2)))
        .unwrap();
    stream
}

#[cfg(unix)]
#[test]
fn unix_status_fixture_waits_for_a_delayed_request_on_a_nonblocking_accepted_stream() {
    use std::io::{Read, Write};
    let env = Env::new();
    let listener = std::os::unix::net::UnixListener::bind(env.root.join("delayed.sock")).unwrap();
    let mut client =
        std::os::unix::net::UnixStream::connect(env.root.join("delayed.sock")).unwrap();
    let (stream, _) = listener.accept().unwrap();
    stream.set_nonblocking(true).unwrap();
    let stream = prepare_status_socket(stream);
    let (ready, waiting) = std::sync::mpsc::channel();
    let server = std::thread::spawn(move || {
        ready.send(()).unwrap();
        status_reply(stream, serde_json::json!({"data":[]}), "200 OK")
    });
    waiting.recv().unwrap();
    std::thread::sleep(std::time::Duration::from_millis(20));
    client
        .write_all(b"GET /api/v1/lsp HTTP/1.1\r\nHost: local\r\n\r\n")
        .unwrap();
    let mut response = String::new();
    client.read_to_string(&mut response).unwrap();
    assert!(server.join().unwrap().starts_with("GET /api/v1/lsp"));
    assert!(response.contains("200 OK"));
}

#[cfg(unix)]
#[test]
fn lsp_status_reads_a_registered_unix_socket_without_tcp_startup() {
    let env = Env::new();
    let socket = env.root.join("server.sock");
    let listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
    register_status_listener(&env, "", Some(&socket));
    let root = env.root.clone();
    listener.set_nonblocking(true).unwrap();
    let server = std::thread::spawn(move || {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let stream = loop {
            if let Ok((stream, _)) = listener.accept() {
                break stream;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "Unix status listener received no request"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        };
        let stream = prepare_status_socket(stream);
        status_reply(
            stream,
            serde_json::json!({"location":{"directory":root},"data":[]}),
            "200 OK",
        )
    });
    let output = env.cyber(&["lsp", "status", "--format", "json"]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(server.join().unwrap().contains("/api/v1/lsp?"));
    assert_eq!(
        status_row(&json(&output), "rust-analyzer")["running"],
        false
    );
}

#[test]
fn formatter_status_observes_project_markers_without_evaluating_configuration() {
    let env = Env::new();
    status_candidate(&env, "prettier");
    let run = |args: &[&str]| {
        env.command(args)
            .env("PATH", env.root.join("empty-path"))
            .output()
            .unwrap()
    };
    let output = run(&["fmt", "status", "--format", "json"]);
    assert!(output.status.success(), "{}", stderr(&output));
    let rows = json(&output);
    assert_eq!(rows.as_array().unwrap().len(), 12);
    assert_eq!(status_row(&rows, "prettier")["enabled"], false);
    std::fs::write(
        env.root.join("prettier.config.js"),
        "throw new Error('must not evaluate');",
    )
    .unwrap();
    let rows = json(&run(&["fmt", "status", "--format", "json"]));
    let prettier = status_row(&rows, "prettier");
    assert_eq!(prettier["enabled"], true);
    assert_eq!(prettier["detected_by"], "prettier.config.js");
    assert!(!env.root.join("cyber-home/data/cyber-dev.db").exists());
}

#[test]
fn intelligence_status_honors_disabling_and_withholds_untrusted_commands() {
    let env = Env::new();
    status_candidate(&env, "rust-analyzer");
    let disabled = env
        .command(&["lsp", "status", "--format", "json", "--config", "lsp=false"])
        .env("PATH", env.root.join("empty-path"))
        .output()
        .unwrap();
    assert!(disabled.status.success(), "{}", stderr(&disabled));
    assert!(
        json(&disabled)
            .as_array()
            .unwrap()
            .iter()
            .all(|r| r["enabled"] == false)
    );
    let repo=env.repo(r#"{"lsp":{"untrusted":{"command":["malicious"],"extensions":[".x"]}},"formatters":{"untrusted":{"command":["malicious","$FILE"],"extensions":[".x"]}}}"#);
    for verb in ["lsp", "fmt"] {
        let output = env
            .command(&[verb, "status", "--format", "json"])
            .current_dir(&repo)
            .output()
            .unwrap();
        assert!(output.status.success(), "{}", stderr(&output));
        assert!(
            json(&output)
                .as_array()
                .unwrap()
                .iter()
                .all(|r| r["id"] != "untrusted")
        );
    }
    let forced=env.command(&["fmt","status","--format","json","--config",r#"formatters.taplo={"command":["missing-taplo","fmt","$FILE"],"extensions":[".toml"]}"#]).output().unwrap();
    assert!(forced.status.success(), "{}", stderr(&forced));
    let rows = json(&forced);
    let taplo = rows
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["id"] == "taplo")
        .unwrap();
    assert_eq!(taplo["enabled"], true);
    assert_eq!(taplo["detected_by"], "config");
}

/// `cyber doctor` lists the instruction files in load order, CYBER.md last in its
/// directory, and warns when one exceeds `instructions.max_bytes`.
#[test]
fn doctor_lists_cyber_md_and_warns_when_it_is_truncated() {
    let env = Env::new();
    let repo = env.root.join("repo");
    std::fs::create_dir_all(repo.join(".git")).unwrap();
    std::fs::write(repo.join("AGENTS.md"), "agents").unwrap();
    std::fs::write(repo.join("CYBER.md"), "cyber").unwrap();
    let doctor = |dir: &Path| {
        let output = env
            .command(&["doctor", "--format", "json"])
            .current_dir(dir)
            .output()
            .unwrap();
        let rows = json(&output);
        rows["checks"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["check"] == "instructions")
            .cloned()
            .unwrap()
    };
    // Paths are compared by file name: on Windows the reported Location may lack the
    // `\\?\` prefix of the canonical temp directory.
    let row = doctor(&repo);
    assert_eq!(row["status"], "pass", "{row}");
    let detail = row["detail"].as_str().unwrap();
    let files: Vec<&str> = detail
        .strip_prefix("2 loaded: ")
        .unwrap()
        .split(", ")
        .collect();
    let names: Vec<_> = files
        .iter()
        .map(|file| Path::new(file).file_name().unwrap().to_str().unwrap())
        .collect();
    assert_eq!(names, ["AGENTS.md", "CYBER.md"], "{detail}");
    assert!(
        files
            .iter()
            .all(|file| Path::new(file).starts_with(Path::new(files[0]).parent().unwrap()))
    );
    std::fs::write(repo.join("CYBER.md"), "x".repeat(40_000)).unwrap();
    let row = doctor(&repo);
    assert_eq!(row["status"], "warn", "{row}");
    let detail = row["detail"].as_str().unwrap();
    let warning = format!(
        "{}CYBER.md exceeds instructions.max_bytes (32768 bytes) and was truncated",
        std::path::MAIN_SEPARATOR
    );
    assert!(detail.contains(&warning), "{detail}");
}

/// The `instructions` row of `cyber doctor` run in `dir`.
fn doctor_instructions(env: &Env, dir: &Path) -> serde_json::Value {
    let output = env
        .command(&["doctor", "--format", "json"])
        .current_dir(dir)
        .output()
        .unwrap();
    json(&output)["checks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["check"] == "instructions")
        .cloned()
        .unwrap()
}

/// Regression: doctor read `compat.claude` from the Location's config while the runtime
/// read it once from the server's default directory, so the two disagreed about whether
/// a project's `CLAUDE.md` reaches the model. Both now read the Location's config (the
/// runtime side is covered by `cyber-app` `instructions`
/// `a_project_compat_claude_false_keeps_its_claude_md_out_of_the_request`).
#[test]
fn doctor_honors_a_project_compat_claude_false() {
    let env = Env::new();
    let repo = env.root.join("repo");
    std::fs::create_dir_all(repo.join(".git")).unwrap();
    std::fs::write(repo.join("CLAUDE.md"), "claude").unwrap();
    std::fs::write(repo.join("CYBER.md"), "cyber").unwrap();
    let names = |row: &serde_json::Value| -> Vec<String> {
        let detail = row["detail"].as_str().unwrap();
        let (_, files) = detail.split_once(" loaded: ").unwrap();
        files
            .split(", ")
            .map(|file| {
                let name = Path::new(file).file_name().unwrap();
                name.to_string_lossy().into_owned()
            })
            .collect()
    };
    assert_eq!(
        names(&doctor_instructions(&env, &repo)),
        ["CLAUDE.md", "CYBER.md"]
    );
    std::fs::write(repo.join("cyber.json"), r#"{"compat":{"claude":false}}"#).unwrap();
    assert_eq!(names(&doctor_instructions(&env, &repo)), ["CYBER.md"]);
}

/// With `AGENTS.md`, `CLAUDE.md` and `CYBER.md` in one directory, doctor lists `AGENTS.md`
/// then `CYBER.md`: `CLAUDE.md` only loads where a directory has no `AGENTS.md`.
#[test]
fn doctor_leaves_claude_md_out_next_to_agents_md_and_keeps_cyber_md() {
    let env = Env::new();
    let repo = env.root.join("repo");
    std::fs::create_dir_all(repo.join(".git")).unwrap();
    for name in ["AGENTS.md", "CLAUDE.md", "CYBER.md"] {
        std::fs::write(repo.join(name), name).unwrap();
    }
    let row = doctor_instructions(&env, &repo);
    let detail = row["detail"].as_str().unwrap();
    let (_, files) = detail.split_once(" loaded: ").unwrap();
    let names: Vec<_> = files
        .split(", ")
        .map(|file| {
            Path::new(file)
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    assert_eq!(names, ["AGENTS.md", "CYBER.md"], "{detail}");
}

/// Regression: a project `CYBER.md` symlinked outside the checkout was loaded; doctor now
/// shows it refused.
#[cfg(unix)]
#[test]
fn doctor_warns_about_an_instruction_file_outside_the_project() {
    let env = Env::new();
    let repo = env.root.join("repo");
    std::fs::create_dir_all(repo.join(".git")).unwrap();
    std::fs::write(env.root.join("secret.txt"), "secret").unwrap();
    std::os::unix::fs::symlink("../secret.txt", repo.join("CYBER.md")).unwrap();
    let row = doctor_instructions(&env, &repo);
    assert_eq!(row["status"], "warn", "{row}");
    let detail = row["detail"].as_str().unwrap();
    assert!(detail.starts_with("no instruction files"), "{detail}");
    assert!(
        detail.contains("CYBER.md resolves outside the project and was not loaded"),
        "{detail}"
    );
}

/// Regression: doctor (like the runtime) loaded `@.env` imports of project instruction
/// files without the read rules; it now shows them refused, and a project config that
/// fails to parse keeps every project file out (CLAUDE.md included) instead of reading
/// `compat.claude` from `Null` as `true`.
#[test]
fn doctor_refuses_protected_imports_and_project_files_under_a_broken_config() {
    let env = Env::new();
    let repo = env.root.join("repo");
    std::fs::create_dir_all(repo.join(".git")).unwrap();
    std::fs::write(repo.join(".env"), "TOKEN=hunter2").unwrap();
    std::fs::write(repo.join("CYBER.md"), "cyber\n@.env").unwrap();
    let row = doctor_instructions(&env, &repo);
    assert_eq!(row["status"], "warn", "{row}");
    let detail = row["detail"].as_str().unwrap();
    assert!(detail.starts_with("1 loaded: "), "{detail}");
    assert!(
        detail.contains("import @.env is not allowed by the read permission rules"),
        "{detail}"
    );
    std::fs::write(repo.join("CLAUDE.md"), "claude").unwrap();
    std::fs::write(repo.join("cyber.json"), "{ not json").unwrap();
    let row = doctor_instructions(&env, &repo);
    let detail = row["detail"].as_str().unwrap();
    assert!(detail.starts_with("no instruction files"), "{detail}");
    assert!(!detail.contains("CLAUDE.md"), "{detail}");
}

#[test]
fn doctor_disabled_lsp_does_not_report_missing_servers_as_warnings() {
    let env = Env::new();
    let output = env
        .command(&["doctor", "--format", "json", "--config", "lsp=false"])
        .env("PATH", env.root.join("empty-path"))
        .output()
        .unwrap();
    let rows = json(&output);
    let lsp: Vec<_> = rows["checks"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|r| r["check"].as_str().unwrap().starts_with("lsp:"))
        .collect();
    assert_eq!(lsp.len(), 14);
    assert!(lsp.iter().all(|r| r["status"] == "pass"));
    assert!(lsp.iter().all(|r| r["detail"] == "disabled; not installed"));
}

#[test]
fn import_detection_is_read_only_and_outputs_counts_without_configuration_secrets() {
    let e = Env::new();
    std::fs::create_dir_all(e.root.join("home")).unwrap();
    std::fs::create_dir_all(e.root.join(".claude/agents")).unwrap();
    std::fs::write(e.root.join(".claude/agents/check.md"), "private-secret").unwrap();
    std::fs::write(
        e.root.join(".claude/settings.json"),
        r#"{"env":{"TOKEN":"private-secret"},"hooks":{}}"#,
    )
    .unwrap();
    let o = e.cyber(&["import", "--detect", "--format", "json"]);
    assert!(o.status.success(), "{}", stderr(&o));
    let v = json(&o);
    assert_eq!(v["tools"][0]["tool"], "claude");
    assert_eq!(v["tools"][0]["counts"]["agents"], 1);
    assert!(v["tools"][0]["counts"]["sessions"].is_null());
    assert_eq!(v["complete"], false);
    assert!(!stdout(&o).contains("private-secret"));
    assert!(!e.root.join("cyber-home").exists());
    assert!(!e.root.join(".cyber").exists());
    let text = e.cyber(&["import", "--detect"]);
    assert!(text.status.success(), "{}", stderr(&text));
    assert!(stdout(&text).contains("claude: 1 agents"));
    assert!(stdout(&text).contains("sessions: unknown"));
    assert!(!e.root.join("cyber-home").exists());
}

#[test]
fn import_preview_is_read_only_and_preserves_diff_escaping() {
    let e = Env::new();
    std::fs::create_dir_all(e.root.join("home")).unwrap();
    std::fs::create_dir_all(e.root.join(".codex")).unwrap();
    let native = r#"{ "description": "line\nquoted\"value" }"#;
    std::fs::write(e.root.join("cyber.jsonc"), native).unwrap();
    std::fs::write(e.root.join(".codex/config.toml"), "model='coder'\nmodel_provider='local'\n[model_providers.local]\nbase_url='http://localhost:8000/v1'\napi_key='private-secret'\n").unwrap();
    let preview = e.cyber(&["import", "auto", "--dry-run", "--format", "json"]);
    assert!(preview.status.success(), "{}", stderr(&preview));
    let view = json(&preview);
    assert_eq!(view["complete"], false);
    assert_eq!(view["required_environment"].as_array().unwrap().len(), 1);
    assert_eq!(
        view["required_environment"][0]["sources"][0]["field"],
        "/model_providers/local/api_key"
    );
    assert!(!stdout(&preview).contains("private-secret"));
    let text = e.cyber(&["import", "auto", "--dry-run"]);
    assert!(text.status.success(), "{}", stderr(&text));
    assert!(stdout(&text).contains(view["diff"].as_str().unwrap()));
    assert_eq!(
        std::fs::read_to_string(e.root.join("cyber.jsonc")).unwrap(),
        native
    );
    assert!(!e.root.join("cyber-home").exists());
}

#[test]
fn mcp_import_preview_is_secret_safe_source_linked_and_does_not_create_runtime_state() {
    let e = Env::new();
    std::fs::create_dir_all(e.root.join("home")).unwrap();
    std::fs::write(e.root.join(".mcp.json"), r#"{"mcpServers":{"audit":{"command":"never-execute-source-server","env":{"API_KEY":"private-mcp-secret"},"disabled":true}}}"#).unwrap();
    let preview = e.cyber(&["import", "claude", "--dry-run", "--format", "json"]);
    assert!(preview.status.success(), "{}", stderr(&preview));
    let view = json(&preview);
    assert_eq!(view["complete"], false);
    assert!(
        view["diff"]
            .as_str()
            .unwrap()
            .contains("never-execute-source-server")
    );
    assert_eq!(
        view["required_environment"][0]["sources"][0]["field"],
        "/mcpServers/audit/env/API_KEY"
    );
    assert!(!stdout(&preview).contains("private-mcp-secret"));
    let text = e.cyber(&["import", "claude", "--dry-run"]);
    assert!(text.status.success(), "{}", stderr(&text));
    assert!(!stdout(&text).contains("private-mcp-secret"));
    assert!(!e.root.join("cyber.jsonc").exists());
    assert!(!e.root.join(".cyber").exists());
    assert!(!e.root.join("cyber-home").exists());
}

#[test]
fn import_merged_reports_include_comparisons_in_json_and_text_without_writing() {
    let e = Env::new();
    std::fs::create_dir_all(e.root.join("home")).unwrap();
    let native = r#"{"model":"native/kept"}"#;
    std::fs::write(e.root.join("cyber.jsonc"), native).unwrap();
    std::fs::write(
        e.root.join("opencode.json"),
        r#"{"model":"source/ignored"}"#,
    )
    .unwrap();
    let preview = e.cyber(&["import", "opencode", "--dry-run", "--format", "json"]);
    assert!(preview.status.success(), "{}", stderr(&preview));
    let view = json(&preview);
    let model = view["report"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["field"] == "/model" && r["status"] == "merged")
        .unwrap();
    assert_eq!(model["comparison"]["kept"], "native/kept");
    assert_eq!(model["comparison"]["ignored"], "source/ignored");
    let text = e.cyber(&["import", "opencode", "--dry-run"]);
    assert!(text.status.success(), "{}", stderr(&text));
    assert!(stdout(&text).contains("kept: \"native/kept\""));
    assert!(stdout(&text).contains("ignored: \"source/ignored\""));
    assert_eq!(
        std::fs::read_to_string(e.root.join("cyber.jsonc")).unwrap(),
        native
    );
    assert!(!e.root.join("cyber-home").exists());
}

#[test]
fn import_kept_credentials_do_not_ask_to_set_ignored_variables() {
    let e = Env::new();
    std::fs::create_dir_all(e.root.join("home")).unwrap();
    let native = r#"{"mcp":{"audit":{"type":"local","command":"kept","args":[],"env":{"API_KEY":"{env:KEPT_KEY}"}}}}"#;
    std::fs::write(e.root.join("cyber.jsonc"), native).unwrap();
    std::fs::write(
        e.root.join(".mcp.json"),
        r#"{"mcpServers":{"audit":{"command":"ignored","env":{"API_KEY":"private-ignored-key"}}}}"#,
    )
    .unwrap();
    let preview = e.cyber(&["import", "claude", "--dry-run", "--format", "json"]);
    assert!(preview.status.success(), "{}", stderr(&preview));
    let view = json(&preview);
    assert!(view["required_environment"].as_array().unwrap().is_empty());
    assert_eq!(view["diff"], "");
    let text = e.cyber(&["import", "claude", "--dry-run"]);
    assert!(text.status.success(), "{}", stderr(&text));
    assert!(!stdout(&text).contains("Set CYBER_IMPORT_"));
    assert!(!stdout(&text).contains("private-ignored-key"));
    assert_eq!(
        std::fs::read_to_string(e.root.join("cyber.jsonc")).unwrap(),
        native
    );
    assert!(!e.root.join("cyber-home").exists());
}

#[test]
fn opencode_provider_import_reports_credentials_without_writing_or_loading_packages() {
    let e = Env::new();
    std::fs::create_dir_all(e.root.join("home")).unwrap();
    std::fs::write(e.root.join("opencode.json"), r#"{"model":"corp/coder","providers":{"corp":{"package":"@opencode/ai/providers/openai/responses","settings":{"baseURL":"https://example.com/v1","apiKey":"private-provider-secret"},"models":{"coder":{"modelID":"wire-model","limit":{"context":8192},"body":{"store":false},"variants":[{"id":"deep","body":{"metadata":{"lane":"review"}}}]}}},"custom":{"package":"file:never-load-source-package"}}}"#).unwrap();
    let preview = e.cyber(&["import", "opencode", "--dry-run", "--format", "json"]);
    assert!(preview.status.success(), "{}", stderr(&preview));
    let view = json(&preview);
    assert_eq!(view["complete"], false);
    assert_eq!(view["required_environment"].as_array().unwrap().len(), 1);
    assert_eq!(
        view["required_environment"][0]["sources"][0]["field"],
        "/providers/corp/settings/apiKey"
    );
    let diff = view["diff"].as_str().unwrap();
    assert!(
        ["openai-responses", "wire-model", "review", "deep"]
            .iter()
            .all(|field| diff.contains(field))
    );
    assert!(
        view["report"]
            .as_array()
            .unwrap()
            .iter()
            .any(|record| record["field"] == "/providers/custom"
                && record["status"] == "not imported")
    );
    assert!(!stdout(&preview).contains("private-provider-secret"));
    assert!(!stdout(&preview).contains("never-load-source-package"));
    let text = e.cyber(&["import", "opencode", "--dry-run"]);
    assert!(text.status.success(), "{}", stderr(&text));
    assert!(!stdout(&text).contains("private-provider-secret"));
    assert!(!e.root.join("cyber.jsonc").exists());
    assert!(!e.root.join(".cyber").exists());
    assert!(!e.root.join("cyber-home").exists());
}

#[test]
fn inline_agent_and_compaction_imports_are_source_linked_without_runtime_state() {
    let e = Env::new();
    std::fs::create_dir_all(e.root.join("home")).unwrap();
    std::fs::write(e.root.join("opencode.json"),r#"{"agent":{"audit":{"prompt":"Review code only.","mode":"subagent","permission":{"edit":"deny"}}},"compaction":{"preserve_recent_tokens":12000},"instructions":["never-read-source-instruction"],"commands":{"audit":{"template":"never-execute-source-command"}}}"#).unwrap();
    let preview = e.cyber(&["import", "opencode", "--dry-run", "--format", "json"]);
    assert!(preview.status.success(), "{}", stderr(&preview));
    let view = json(&preview);
    let report = view["report"].as_array().unwrap();
    assert!(report.iter().any(|r| r["field"] == "/agents/audit/system"
        && r["sources"][0]["field"] == "/agent/audit/prompt"));
    assert!(
        report
            .iter()
            .any(|r| r["field"] == "/compaction/keep/tokens"
                && r["sources"][0]["field"] == "/compaction/preserve_recent_tokens")
    );
    let diff = view["diff"].as_str().unwrap();
    assert!(diff.contains("12000"));
    assert!(!diff.contains("never-read-source-instruction"));
    assert!(diff.contains("never-execute-source-command"));
    assert!(
        report
            .iter()
            .any(|r| r["field"] == "/commands/audit/template"
                && r["sources"][0]["field"] == "/commands/audit/template")
    );
    assert!(
        ["cyber.jsonc", ".cyber", "cyber-home"]
            .iter()
            .all(|path| !e.root.join(path).exists())
    );
}

#[test]
fn markdown_command_import_is_read_only_and_links_body_and_frontmatter() {
    let e = Env::new();
    std::fs::create_dir_all(e.root.join("home")).unwrap();
    let commands = e.root.join(".opencode/commands/team");
    std::fs::create_dir_all(&commands).unwrap();
    let path = commands.join("review.md");
    let original =
        "---\ndescription: Review code\nargument-hint: '<paths>'\n---\nReview $ARGUMENTS";
    std::fs::write(&path, original).unwrap();
    std::fs::write(commands.join("shell.md"), "!`touch never-created-sentinel`").unwrap();
    let legacy = e.root.join(".opencode/command");
    std::fs::create_dir_all(&legacy).unwrap();
    std::fs::write(legacy.join("legacy.md"), "Legacy review").unwrap();
    let output = e.cyber(&["import", "opencode", "--dry-run", "--format", "json"]);
    assert!(output.status.success(), "{}", stderr(&output));
    let view = json(&output);
    let report = view["report"].as_array().unwrap();
    assert!(
        report.iter().any(
            |r| r["field"] == "/commands/legacy/template" && r["sources"][0]["field"] == "body"
        )
    );
    assert!(
        report
            .iter()
            .any(|r| r["field"] == "/commands/team~1review/template"
                && r["sources"][0]["field"] == "body")
    );
    assert!(
        report
            .iter()
            .any(|r| r["field"] == "/commands/team~1review/argument_hint"
                && r["sources"][0]["field"] == "frontmatter:argument-hint")
    );
    assert!(report.iter().any(|r| {
        r["source"]
            .as_str()
            .is_some_and(|s| s.ends_with("shell.md"))
            && r["status"] == "not imported"
    }));
    assert!(!stdout(&output).contains("never-created-sentinel"));
    assert_eq!(std::fs::read_to_string(path).unwrap(), original);
    for path in [
        "cyber.jsonc",
        ".cyber",
        "cyber-home",
        "never-created-sentinel",
    ] {
        assert!(!e.root.join(path).exists());
    }
}

#[test]
fn api_reconciliation_uses_the_registered_service_without_creating_a_job() {
    use std::process::Stdio;
    use std::time::{Duration, Instant};
    let env = Env::new();
    let mut command = env.command(&["serve", "--register", "--port", "0"]);
    #[cfg(unix)]
    command.arg("--socket").arg(env.root.join("r.sock"));
    let mut owned = OwnedService(
        command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let registration = env.root.join("cyber-home/state/server.json");
    let deadline = Instant::now() + Duration::from_secs(10);
    while !registration.is_file() {
        assert!(
            owned.0.try_wait().unwrap().is_none(),
            "owned server exited before registration"
        );
        assert!(Instant::now() < deadline, "owned server did not register");
        std::thread::sleep(Duration::from_millis(10));
    }
    let created = env.cyber(&[
        "api",
        "v1.session.create",
        "--data",
        r#"{"model":"openai/gpt-6-luna"}"#,
    ]);
    assert!(
        created.status.success(),
        "{}: {}",
        stderr(&created),
        stdout(&created)
    );
    let session = json(&created)["data"]["id"].as_str().unwrap().to_owned();
    let session_parameter = format!("sessionID={session}");
    let request_parameter = "requestID=op_cli_reconcile";
    let stopped = env.cyber(&[
        "api",
        "v1.session.stopDelegation",
        "--param",
        &session_parameter,
        "--param",
        request_parameter,
    ]);
    assert!(stopped.status.success(), "{}", stderr(&stopped));
    for _ in 0..2 {
        let recovered = env.cyber(&[
            "api",
            "v1.session.reconcileDelegation",
            "--param",
            &session_parameter,
            "--param",
            request_parameter,
        ]);
        assert!(
            recovered.status.success(),
            "{}: {}",
            stderr(&recovered),
            stdout(&recovered)
        );
        let recovered = json(&recovered);
        assert_eq!(recovered["data"]["session_id"], session);
        assert_eq!(recovered["data"]["id"], "op_cli_reconcile");
        assert_eq!(recovered["data"]["status"], "cancelled");
        assert!(recovered["data"]["job_id"].is_null());
    }
    let jobs = env.cyber(&["api", "v1.job.list"]);
    assert!(jobs.status.success(), "{}", stderr(&jobs));
    assert!(json(&jobs)["data"].as_array().unwrap().is_empty());
    let stop = env.cyber(&["service", "stop", "--format", "json"]);
    assert!(stop.status.success(), "{}", stderr(&stop));
}
