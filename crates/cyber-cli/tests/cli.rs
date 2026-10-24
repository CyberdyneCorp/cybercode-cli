//! End-to-end tests of the `cyber` binary (`cli-commands`, `workspace-trust`).

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::{Value, json};

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
    assert_list_rows(&stdout(&o), &global.join("audit.js"));

    let o = env
        .command(&["workflows", "list", "--format", "json"])
        .current_dir(&repo)
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", stderr(&o));
    assert_list_json(&json(&o));
}

/// The cells of `name`'s row in `workflows list` output.
fn list_row(text: &str, name: &str) -> Vec<String> {
    text.lines()
        .find(|l| l.starts_with(&format!("{name} ")))
        .unwrap_or_else(|| panic!("no row for {name}: {text}"))
        .split("  ")
        .map(str::trim)
        .filter(|cell| !cell.is_empty())
        .map(str::to_owned)
        .collect()
}

fn assert_list_rows(text: &str, global_audit: &Path) {
    assert!(text.starts_with("NAME"), "{text}");
    let audit = list_row(text, "audit");
    assert_eq!(audit[1], "project", "{text}");
    assert!(audit[2].starts_with("Audit auth endpoints"), "{text}");
    assert_eq!(
        audit[3],
        format!("(shadows bundled:audit, {})", global_audit.display())
    );
    for (name, origin) in [
        ("plan", "global"),
        ("review", "project"),
        ("migrate", "bundled"),
        ("research", "bundled"),
    ] {
        assert_eq!(list_row(text, name)[1], origin, "{text}");
    }
    assert!(!text.contains("Global audit"), "{text}");
    assert!(
        text.contains("computed.js: ParseError: meta must be a pure literal (2:29)"),
        "{text}"
    );
}

fn listed_names(listing: &Value) -> Vec<(String, String)> {
    listing["workflows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|w| {
            (
                w["name"].as_str().unwrap().to_owned(),
                w["origin"].as_str().unwrap().to_owned(),
            )
        })
        .collect()
}

fn assert_list_json(listing: &Value) {
    let names: Vec<String> = listed_names(listing).into_iter().map(|(n, _)| n).collect();
    assert_eq!(
        names,
        ["audit", "migrate", "plan", "research", "review", "solve"]
    );
    let audit = &listing["workflows"][0];
    assert_eq!(audit["origin"], "project");
    assert_eq!(audit["scope"], "project");
    assert_eq!(audit["phases"][0]["title"], "Scan");
    assert!(audit["digest"].as_str().unwrap().starts_with("sha256:"));
    assert_eq!(audit["shadowed"].as_array().unwrap().len(), 2);
    assert_eq!(listing["workflows"][1]["origin"], "bundled");
    assert_eq!(listing["workflows"][1]["path"], Value::Null);
    assert_eq!(listing["issues"][0]["diagnostic"]["kind"], "ParseError");
    assert_eq!(listing["issues"][0]["diagnostic"]["position"]["line"], 2);
}

/// `workflows` → Saved and bundled workflows: a fresh install lists the bundled
/// Workflows, and `show` prints one with its estimate and args (the source with
/// `--source`).
#[test]
fn bundled_workflows_are_listed_and_shown() {
    let env = Env::new();
    let repo = env.repo("{}");
    let run = |args: &[&str]| env.command(args).current_dir(&repo).output().unwrap();

    let o = run(&["workflows", "list", "--format", "json"]);
    assert!(o.status.success(), "{}", stderr(&o));
    let entries = listed_names(&json(&o));
    let expected: Vec<(String, String)> =
        ["audit", "migrate", "plan", "research", "review", "solve"]
            .iter()
            .map(|name| ((*name).to_owned(), "bundled".to_owned()))
            .collect();
    assert_eq!(entries, expected);

    let o = run(&["workflows", "show", "review"]);
    assert!(o.status.success(), "{}", stderr(&o));
    let shown = stdout(&o);
    assert!(shown.starts_with("review  (bundled)"), "{shown}");
    assert!(shown.contains("estimate: lenses × diff groups"), "{shown}");
    assert!(shown.contains("\"max_findings\""), "{shown}");
    assert!(!shown.contains("function dedupFindings"), "{shown}");
    let o = run(&["workflows", "show", "review", "--source"]);
    assert!(stdout(&o).contains("function dedupFindings"));
}

/// `eject` copies a bundled Workflow into the project, where the copy then overrides the
/// bundled one; it refuses to overwrite an existing file and names it.
#[test]
fn an_ejected_workflow_overrides_the_bundled_one_and_is_never_overwritten() {
    let env = Env::new();
    let repo = env.repo("{}");
    let run = |args: &[&str]| env.command(args).current_dir(&repo).output().unwrap();

    let o = run(&["workflows", "eject", "audit"]);
    assert!(o.status.success(), "{}", stderr(&o));
    let ejected = repo.join(".cyber/workflows/audit.js");
    let source = std::fs::read_to_string(&ejected).unwrap();
    std::fs::write(&ejected, source.replace("Codebase-wide bug", "Edited bug")).unwrap();
    let listing = json(&run(&["workflows", "list", "--format", "json"]));
    let audit = &listing["workflows"][0];
    assert_eq!(audit["origin"], "project");
    assert!(
        audit["description"]
            .as_str()
            .unwrap()
            .starts_with("Edited bug")
    );
    assert_eq!(audit["shadowed"], json!(["bundled:audit"]));

    let o = run(&["workflows", "eject", "audit"]);
    assert_eq!(o.status.code(), Some(1));
    assert!(
        stderr(&o).contains(&ejected.display().to_string()),
        "{}",
        stderr(&o)
    );
    assert!(
        std::fs::read_to_string(&ejected)
            .unwrap()
            .contains("Edited bug")
    );
    assert_eq!(run(&["workflows", "eject", "nope"]).status.code(), Some(1));
}

/// `workflows.bundled: false` and `CYBER_DISABLE_BUNDLED_WORKFLOWS=1` hide the bundled
/// Workflows.
#[test]
fn bundled_workflows_can_be_turned_off() {
    let env = Env::new();
    let repo = env.repo("{}");
    let count = |command: &mut Command| {
        let o = command.current_dir(&repo).output().unwrap();
        json(&o)["workflows"].as_array().unwrap().len()
    };
    std::fs::create_dir_all(repo.join(".cyber")).unwrap();
    std::fs::write(
        repo.join(".cyber/cyber.jsonc"),
        r#"{"workflows": {"bundled": false}}"#,
    )
    .unwrap();
    assert_eq!(
        count(&mut env.command(&["workflows", "list", "--format", "json"])),
        0
    );
    std::fs::remove_file(repo.join(".cyber/cyber.jsonc")).unwrap();
    let mut disabled = env.command(&["workflows", "list", "--format", "json"]);
    disabled.env("CYBER_DISABLE_BUNDLED_WORKFLOWS", "1");
    assert_eq!(count(&mut disabled), 0);
    assert_eq!(
        count(&mut env.command(&["workflows", "list", "--format", "json"])),
        6
    );
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
    assert_eq!(shown["agents_detail"][0]["status"], "completed");
    assert_eq!(shown["agents"]["completed"], 1, "{shown}");
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

/// `exec-mode` → Workflow exec: `cyber exec --workflow` streams `workflow_progress`,
/// prints the return value as the result and maps the run's end to the exit code.
#[test]
fn exec_runs_a_workflow_and_maps_its_end_to_exit_codes() {
    let model =
        scripted_model::ScriptedModel::start(|_| serde_json::json!({"content": "agent done"}));
    let env = Env::new();
    let service = StartedService(&env, workflow_env(&env, &model));

    let streamed = service.cyber(&["exec", "--workflow", "x", "--format", "stream-json"]);
    exits(&streamed, 0);
    let events: Vec<Value> = stdout(&streamed)
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(events[0]["type"], "system", "{events:?}");
    let progress: Vec<&Value> = events
        .iter()
        .filter(|event| event["type"] == "workflow_progress")
        .collect();
    assert!(!progress.is_empty(), "{events:?}");
    assert!(progress.iter().all(|event| event["run_id"].is_string()));
    let result = events.last().unwrap();
    assert_eq!(result["type"], "result", "{events:?}");
    assert_eq!(result["result"], serde_json::json!({"text": "agent done"}));
    assert_eq!(result["status"], "completed");
    assert_eq!(result["exit_code"], 0);

    let text = service.cyber(&["exec", "--workflow", "x", "--args", "{}"]);
    let shown = exits(&text, 0);
    assert!(
        stdout(&text).contains("\"text\": \"agent done\""),
        "{shown}"
    );
    assert!(shown.contains("phase: Ask"), "{shown}");

    let failed = exits(&service.cyber(&["exec", "--workflow", "boom.js"]), 1);
    assert!(failed.contains("boom"), "{failed}");
    let capped = service.cyber(&[
        "exec",
        "--workflow",
        "x",
        "--timeout",
        "0s",
        "--format",
        "json",
    ]);
    exits(&capped, 4);
    assert_eq!(json(&capped)["stop_reason"], "budget_exceeded");
    exits(
        &service.cyber(&["exec", "--workflow", "x", "--args", "[1]"]),
        2,
    );
    exits(&service.cyber(&["exec", "--workflow", "x", "a prompt"]), 2);
}

/// Request bodies of provider Turns, as text. Session title requests carry the first
/// prompt too and arrive concurrently, so they are left out of any count.
fn turn_requests(model: &scripted_model::ScriptedModel) -> Vec<String> {
    model
        .requests()
        .iter()
        .map(Value::to_string)
        .filter(|text| !text.contains("Write a short title"))
        .collect()
}

/// Poll `done` for up to 30 seconds.
fn poll_until(what: &str, mut done: impl FnMut() -> bool) {
    for _ in 0..600 {
        if done() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    panic!("{what} did not happen");
}

/// [`poll_until`] while `child` runs, handing it back once `done`. If it exits first, or
/// the wait times out, fail with its output instead of hiding why it never got there.
fn poll_while_running(
    what: &str,
    mut child: std::process::Child,
    mut done: impl FnMut() -> bool,
) -> std::process::Child {
    for _ in 0..1200 {
        if done() {
            return child;
        }
        if child.try_wait().unwrap().is_some() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    let _ = child.kill();
    let output = child.wait_with_output().unwrap();
    panic!(
        "{what} did not happen ({}): {}{}",
        output.status,
        stdout(&output),
        stderr(&output)
    );
}

/// `workflows` → Run queue and Workflow CLI with the real binary: with
/// `workflows.max_running_runs: 1`, a second run waits `queued`; `runs` and `show` report
/// its queue position, and `run --wait` keeps waiting through the queue until it ends.
#[test]
fn workflows_run_wait_keeps_waiting_while_the_run_is_queued() {
    let model = scripted_model::ScriptedModel::start(|body| {
        if body.to_string().contains("GATE-s1") {
            return serde_json::json!({"gate": "s1"});
        }
        serde_json::json!({"content": "agent done"})
    });
    let env = Env::new();
    let catalog = workflow_env(&env, &model);
    let mut config = model.config();
    config["workflows"] = serde_json::json!({"max_running_runs": 1});
    std::fs::write(
        env.root.join("cyber-home/config/cyber.jsonc"),
        config.to_string(),
    )
    .unwrap();
    std::fs::write(
        env.root.join("slow.js"),
        "export const meta = { name: 'slow', description: 'Gated' };\nexport default async () => (await agent('GATE-s1')).text;\n",
    )
    .unwrap();
    let service = StartedService(&env, catalog);
    let slow = exits(&service.cyber(&["workflows", "run", "slow.js"]), 0);
    let slow = slow.trim().to_owned();
    poll_until("s1 at the gate", || model.held() == ["s1"]);

    let waiting = env
        .command(&["workflows", "run", "x", "--wait"])
        .env("CYBER_MODELS_PATH", &service.1)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let mut queued = String::new();
    poll_until("the queued run", || {
        let runs = json(&service.cyber(&["workflows", "runs", "--format", "json"]));
        let found = runs
            .as_array()
            .unwrap()
            .iter()
            .find(|run| run["status"] == "queued");
        if let Some(run) = found {
            assert_eq!(run["queue_position"], 1, "{run}");
            queued = run["id"].as_str().unwrap().to_owned();
        }
        found.is_some()
    });
    let table = exits(&service.cyber(&["workflows", "runs"]), 0);
    assert!(table.contains("#1 in queue"), "{table}");
    let shown = exits(&service.cyber(&["workflows", "show", &queued]), 0);
    assert!(shown.contains("queued (#1 in queue)"), "{shown}");
    // Regression: a queued run has no agents, and its cost printed as `-0.0000 USD`.
    assert!(shown.contains("cost     0.0000 USD"), "{shown}");
    std::thread::sleep(std::time::Duration::from_millis(500));
    let mut waiting = waiting;
    assert!(
        waiting.try_wait().unwrap().is_none(),
        "--wait returned while queued"
    );

    model.open_gate();
    let output = waiting.wait_with_output().unwrap();
    let text = exits(&output, 0);
    assert!(text.contains("queued: #1 in queue"), "{text}");
    assert!(stdout(&output).contains("agent done"), "{text}");
    let shown = json(&service.cyber(&["workflows", "show", &slow, "--format", "json"]));
    assert_eq!(shown["status"], "completed", "{shown}");
}

/// First attempts of `HOLD-` agents never answer; a resumed child's continuation does.
fn recovery_reply(body: &Value) -> Value {
    let text = body.to_string();
    if text.contains("was interrupted") {
        return serde_json::json!({"content": "continued"});
    }
    if text.contains("HOLD-") {
        return serde_json::json!({"hold": true});
    }
    serde_json::json!({"content": "agent done"})
}

/// `workflows` → Recovery of several runs, "Resume every interrupted run", with the real
/// binary: three runs in flight when the shared server stops come back interrupted, one
/// run the user paused before does not; `cyber workflows resume --interrupted` resumes
/// the three (each child continues once) and leaves the paused one paused.
#[test]
fn workflows_resume_interrupted_resumes_runs_a_restart_paused_and_not_a_user_paused_one() {
    let model = scripted_model::ScriptedModel::start(recovery_reply);
    let env = Env::new();
    let service = StartedService(&env, workflow_env(&env, &model));
    let mut ids = Vec::new();
    for name in ["ra", "rb", "rc", "rp"] {
        std::fs::write(
            env.root.join(format!("{name}.js")),
            format!("export const meta = {{ name: '{name}', description: 'Held' }};\nexport default async () => (await agent('HOLD-{name}')).text;\n"),
        )
        .unwrap();
        let id = exits(
            &service.cyber(&["workflows", "run", &format!("{name}.js")]),
            0,
        );
        ids.push(id.trim().to_owned());
    }
    // The server caps agents at the core count (3 on CI's macOS runners), so only the runs
    // started first are guaranteed an agent at the model; `rp` may still wait for a slot.
    poll_until("the first three runs' agents at the model", || {
        let turns = turn_requests(&model);
        ["ra", "rb", "rc"].iter().all(|name| {
            turns
                .iter()
                .any(|text| text.contains(&format!("HOLD-{name}")))
        })
    });
    let paused = ids.pop().unwrap();
    exits(&service.cyber(&["workflows", "pause", &paused]), 0);
    exits(&service.cyber(&["service", "stop"]), 0);

    let resumed = exits(&service.cyber(&["workflows", "resume", "--interrupted"]), 0);
    let lines: Vec<String> = ids.iter().map(|id| format!("resumed {id}")).collect();
    assert!(resumed.contains(&lines.join("\n")), "{resumed}");
    for id in &ids {
        poll_until("the resumed run's end", || {
            json(&service.cyber(&["workflows", "show", id, "--format", "json"]))["status"]
                == "completed"
        });
    }
    let shown = json(&service.cyber(&["workflows", "show", &paused, "--format", "json"]));
    assert_eq!(shown["status"], "paused", "{shown}");
    assert_eq!(shown["reason"], "paused by the user", "{shown}");
    for name in ["ra", "rb", "rc", "rp"] {
        let continued = turn_requests(&model)
            .iter()
            .filter(|text| {
                text.contains("was interrupted") && text.contains(&format!("HOLD-{name}"))
            })
            .count();
        assert_eq!(continued, usize::from(name != "rp"), "{name}");
    }
    let none = exits(
        &service.cyber(&["workflows", "resume", "--interrupted", "--all"]),
        0,
    );
    assert!(none.contains("No interrupted Workflow Runs."), "{none}");
    let usage = service.cyber(&["workflows", "resume", "--interrupted", &paused]);
    assert_eq!(usage.status.code(), Some(2), "{}", stderr(&usage));
    model.release();
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
    turn_requests(model)
        .iter()
        .filter(|text| text.contains(needle))
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
        run["agents_detail"]
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
    assert_eq!(
        paused["agents_detail"][1]["status"], "interrupted",
        "{paused}"
    );
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

/// Agents of `g<x>.js` wait at gate `<x>`; gates open oldest first.
fn gated_reply(body: &Value) -> Value {
    let text = body.to_string();
    for name in ["a", "b", "c"] {
        if text.contains(&format!("GATE-{name}")) {
            return serde_json::json!({"gate": name});
        }
    }
    serde_json::json!({"content": "agent done"})
}

/// A shared server with `workflows.max_running_runs: 2` and three gated runs started in
/// order: `a` and `b` running with their agent at the model, `c` queued. Returns the
/// run IDs in that order.
fn two_running_one_queued<'a>(
    env: &'a Env,
    model: &scripted_model::ScriptedModel,
) -> (StartedService<'a>, Vec<String>) {
    let catalog = workflow_env(env, model);
    let mut config = model.config();
    config["workflows"] = serde_json::json!({"max_running_runs": 2});
    std::fs::write(
        env.root.join("cyber-home/config/cyber.jsonc"),
        config.to_string(),
    )
    .unwrap();
    let service = StartedService(env, catalog);
    let mut ids = Vec::new();
    for (name, held) in [
        ("a", &["a"][..]),
        ("b", &["a", "b"][..]),
        ("c", &["a", "b"][..]),
    ] {
        std::fs::write(
            env.root.join(format!("g{name}.js")),
            format!("export const meta = {{ name: 'g{name}', description: 'Gated', phases: [{{ title: 'Wait' }}] }};\nexport default async () => {{ phase('Wait'); return (await agent('GATE-{name}', {{ label: '{name}' }})).text; }};\n"),
        )
        .unwrap();
        let id = exits(
            &service.cyber(&["workflows", "run", &format!("g{name}.js")]),
            0,
        );
        ids.push(id.trim().to_owned());
        poll_until("the agents at the gate", || model.held() == held);
    }
    poll_until("c queued", || {
        let run = json(&service.cyber(&["workflows", "show", &ids[2], "--format", "json"]));
        run["status"] == "queued"
    });
    (service, ids)
}

fn table_line<'t>(table: &'t str, id: &str) -> &'t str {
    table
        .lines()
        .find(|line| line.starts_with(id))
        .unwrap_or_else(|| panic!("{id} not listed:\n{table}"))
}

/// The `runs` table: two running runs with their agent counts, one queued one.
fn assert_two_running_one_queued(service: &StartedService<'_>, ids: &[String]) {
    let table = exits(&service.cyber(&["workflows", "runs"]), 0);
    let header = table.lines().next().unwrap();
    for title in [
        "ID",
        "NAME",
        "STATUS",
        "PHASE",
        "AGENTS d/r/q/f",
        "TOKENS",
        "COST",
        "ELAPSED",
    ] {
        assert!(header.contains(title), "{table}");
    }
    for id in &ids[..2] {
        let line = table_line(&table, id);
        assert!(line.contains("running") && line.contains("Wait"), "{table}");
        assert!(line.contains("0/1/0/0") && line.contains('$'), "{table}");
    }
    let queued = table_line(&table, &ids[2]);
    assert!(
        queued.contains("queued") && queued.contains("#1 in queue"),
        "{table}"
    );
}

/// `--status` keeps the named statuses and refuses an unknown one.
fn assert_status_filter(service: &StartedService<'_>, ids: &[String]) {
    let only = json(&service.cyber(&[
        "workflows",
        "runs",
        "--status",
        "queued",
        "--format",
        "json",
    ]));
    let only = only.as_array().unwrap();
    assert_eq!(only.len(), 1, "{only:?}");
    assert_eq!(only[0]["id"], ids[2].as_str());
    assert_eq!(only[0]["queue_position"], 1);
    let running = exits(
        &service.cyber(&["workflows", "runs", "--status", "running"]),
        0,
    );
    assert!(
        running.contains(&ids[0]) && running.contains(&ids[1]),
        "{running}"
    );
    assert!(!running.contains(&ids[2]), "{running}");
    let bad = service.cyber(&["workflows", "runs", "--status", "sleeping"]);
    assert_eq!(bad.status.code(), Some(2), "{}", stderr(&bad));
}

/// `workflows` → Workflow CLI with the real binary: the `runs` table shows two running
/// runs with their agent counts and a queued one with its place, `--status` filters,
/// `--all` lists every Location, and `run --max-concurrent` records the run's share.
#[test]
fn workflows_runs_lists_running_and_queued_runs_and_filters_by_status() {
    let model = scripted_model::ScriptedModel::start(gated_reply);
    let env = Env::new();
    let (service, ids) = two_running_one_queued(&env, &model);

    assert_two_running_one_queued(&service, &ids);
    assert_status_filter(&service, &ids);

    // Another Location's runs are listed with `--all` only.
    let elsewhere = env.root.join("elsewhere");
    std::fs::create_dir_all(&elsewhere).unwrap();
    let mut other = env.command(&["workflows", "runs", "--format", "json"]);
    other
        .env("CYBER_MODELS_PATH", &service.1)
        .current_dir(&elsewhere);
    assert_eq!(json(&other.output().unwrap()), serde_json::json!([]));
    let mut all = env.command(&["workflows", "runs", "--all", "--format", "json"]);
    all.env("CYBER_MODELS_PATH", &service.1)
        .current_dir(&elsewhere);
    assert_eq!(json(&all.output().unwrap()).as_array().unwrap().len(), 3);

    let shared = json(&service.cyber(&[
        "workflows",
        "run",
        "x",
        "--max-concurrent",
        "3",
        "--format",
        "json",
    ]));
    assert_eq!(shared["share"], 3, "{shared}");
    let zero = service.cyber(&["workflows", "run", "x", "--max-concurrent", "0"]);
    assert_eq!(zero.status.code(), Some(2), "{}", stderr(&zero));
    model.release();
    for _ in 0..4 {
        model.open_gate();
    }
}

/// A running `cyber workflows runs --watch` whose stdout lines are collected.
#[cfg(unix)]
struct Watcher {
    child: std::process::Child,
    lines: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
}

#[cfg(unix)]
impl Watcher {
    fn spawn(service: &StartedService<'_>, args: &[&str]) -> Self {
        use std::io::BufRead;
        let mut child = service
            .0
            .command(args)
            .env("CYBER_MODELS_PATH", &service.1)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let lines = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let (out, sink) = (child.stdout.take().unwrap(), std::sync::Arc::clone(&lines));
        std::thread::spawn(move || {
            for line in std::io::BufReader::new(out).lines().map_while(Result::ok) {
                sink.lock().unwrap().push(line);
            }
        });
        Self { child, lines }
    }

    fn lines(&self) -> Vec<String> {
        self.lines.lock().unwrap().clone()
    }

    /// Ctrl-C, then the exit status and stderr.
    fn interrupt(mut self) -> (Option<i32>, String) {
        let pid = self.child.id().to_string();
        assert!(
            Command::new("kill")
                .args(["-INT", &pid])
                .status()
                .unwrap()
                .success()
        );
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        while self.child.try_wait().unwrap().is_none() {
            if std::time::Instant::now() > deadline {
                let _ = self.child.kill();
                panic!("Ctrl-C did not end `runs --watch`");
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let output = self.child.wait_with_output().unwrap();
        (output.status.code(), stderr(&output))
    }
}

/// `workflows` → Workflow CLI, "Watch several runs", with the real binary and no
/// terminal: `runs --watch` prints each run once, then one line per change as a run
/// ends and the queued run is dispatched; `--format json --watch` prints one JSON object
/// per change. Ctrl-C exits 0, and the runs it watched stay active.
#[cfg(unix)]
#[test]
fn workflows_runs_watch_prints_changes_and_ctrl_c_leaves_the_runs_active() {
    let model = scripted_model::ScriptedModel::start(gated_reply);
    let env = Env::new();
    let (service, ids) = two_running_one_queued(&env, &model);
    let line_of = |lines: &[String], id: &str, status: &str| {
        lines
            .iter()
            .any(|line| line.starts_with(id) && line.contains(&format!("  {status}  ")))
    };

    let watch = Watcher::spawn(&service, &["workflows", "runs", "--watch"]);
    poll_until("the first listing", || watch.lines().len() >= 3);
    let first = watch.lines();
    assert!(line_of(&first, &ids[0], "running"), "{first:?}");
    assert!(
        first
            .iter()
            .any(|l| l.starts_with(&ids[2]) && l.contains("#1 in queue")),
        "{first:?}"
    );
    model.open_gate();
    poll_until("a's end and c's dispatch", || {
        let lines = watch.lines();
        line_of(&lines, &ids[0], "completed") && line_of(&lines, &ids[2], "running")
    });
    let lines = watch.lines();
    let a_lines = lines.iter().filter(|l| l.starts_with(&ids[0])).count();
    assert!(a_lines <= 3, "a is printed once per change: {lines:?}");
    let (code, err) = watch.interrupt();
    assert_eq!(code, Some(0), "{err}");
    assert!(!err.contains("live events unavailable"), "{err}");
    for id in &ids[1..] {
        let run = json(&service.cyber(&["workflows", "show", id, "--format", "json"]));
        assert_eq!(run["status"], "running", "Ctrl-C changed no run: {run}");
    }

    let watch = Watcher::spawn(
        &service,
        &["workflows", "runs", "--watch", "--format", "json"],
    );
    let objects = |watch: &Watcher| -> Vec<Value> {
        watch
            .lines()
            .iter()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    };
    poll_until("the first objects", || objects(&watch).len() >= 3);
    model.open_gate();
    poll_until("b's end", || {
        objects(&watch)
            .iter()
            .any(|run| run["id"] == ids[1].as_str() && run["status"] == "completed")
    });
    let (code, err) = watch.interrupt();
    assert_eq!(code, Some(0), "{err}");
    assert!(!err.contains("live events unavailable"), "{err}");
    let run = json(&service.cyber(&["workflows", "show", &ids[2], "--format", "json"]));
    assert_eq!(run["status"], "running", "{run}");
    assert_eq!(run["agents"]["running"], 1, "{run}");
    model.open_gate();
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

/// `lsp` omitted and `gopls` absent from PATH and `<cache>/bin`: doctor lists it as not
/// installed and nothing is downloaded, with or without `lsp.auto_install`.
#[test]
fn doctor_lists_missing_gopls_as_not_installed_without_download() {
    let env = Env::new();
    let bin = env.root.join("empty-bin");
    std::fs::create_dir_all(&bin).unwrap();
    let cache_bin = env.root.join("cyber-home/cache/bin");
    let row = |format: &str| {
        let output = env
            .command(&["doctor", "--format", format])
            .env("PATH", &bin)
            .output()
            .unwrap();
        stdout(&output)
    };
    let rows: Value = serde_json::from_str(&row("json")).unwrap();
    let gopls = rows["checks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["check"] == "lsp:gopls")
        .cloned()
        .unwrap();
    assert_eq!(gopls["status"], "warn");
    assert_eq!(gopls["detail"], "not installed");
    assert!(row("text").contains("lsp:gopls: warn not installed"));
    let config = env.root.join("cyber-home/config/cyber.jsonc");
    std::fs::create_dir_all(config.parent().unwrap()).unwrap();
    std::fs::write(&config, r#"{"lsp":{"auto_install":true}}"#).unwrap();
    assert!(
        row("text")
            .contains("lsp:gopls: warn not installed; auto_install does not download servers"),
    );
    let downloaded = std::fs::read_dir(&cache_bin)
        .map(|entries| entries.count())
        .unwrap_or(0);
    assert_eq!(downloaded, 0, "doctor must never install servers");
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
            "server",
            "peers:listener",
            "peers:remotes",
            "account"
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

/// A Git repository with one committed file, `tracked` = `base`.
fn git_repo(env: &Env) -> PathBuf {
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
    repo
}

/// The Location the embedded server recorded for Session `id`.
fn session_directory(env: &Env, id: &str) -> PathBuf {
    let embedded = env.root.join("cyber-home").join("data").join("embedded");
    std::fs::read_dir(&embedded)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "db"))
        .find_map(|path| {
            let db = rusqlite::Connection::open(path).unwrap();
            db.query_row("SELECT directory FROM session WHERE id = ?1", [id], |row| {
                row.get::<_, String>(0)
            })
            .ok()
        })
        .map(PathBuf::from)
        .unwrap_or_else(|| panic!("no Session {id} under {}", embedded.display()))
}

fn assert_branch(repo: &Path, branch: &str) {
    let verified = Command::new("git")
        .current_dir(repo)
        .args([
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("refs/heads/{branch}"),
        ])
        .stdout(std::process::Stdio::null())
        .status()
        .unwrap();
    assert!(verified.success(), "missing branch {branch}");
}

#[cfg(unix)]
#[test]
fn exec_worktree_starts_in_owned_checkout_and_keeps_json_stdout_clean() {
    let env = Env::new();
    let repo = git_repo(&env);
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
    assert_eq!(record["branch"], "cyber/from-cli");
    assert_branch(&repo, "cyber/from-cli");
    let target = Path::new(record["path"].as_str().unwrap());
    // Default root: `<data>/worktrees/<project_id>/<name>`.
    let project = target.parent().unwrap();
    assert_eq!(
        project.parent().unwrap(),
        env.root.join("cyber-home").join("data").join("worktrees")
    );
    assert_eq!(target.file_name().unwrap(), "from-cli");
    let session = result["session_id"].as_str().unwrap();
    assert_eq!(
        session_directory(&env, session).canonicalize().unwrap(),
        target.canonicalize().unwrap()
    );
    assert_eq!(
        std::fs::read_to_string(target.join("tracked")).unwrap(),
        "base"
    );
    assert_eq!(
        std::fs::read_to_string(target.join("setup-result")).unwrap(),
        "setup"
    );
    assert!(!repo.join("setup-result").exists());
    // The setup output makes the checkout dirty, so exit keeps it and says where.
    assert!(
        stderr(&output).contains(&format!(
            "worktree kept at {} (branch cyber/from-cli)",
            target.display()
        )),
        "{}",
        stderr(&output)
    );
    verify_worktree_list_command(&env, &repo, target);
}

#[test]
fn exec_worktree_without_name_generates_adjective_noun_hex() {
    let env = Env::new();
    let repo = git_repo(&env);
    keep_worktrees(&env);
    let output = env.cyber(&[
        "--cwd",
        repo.to_str().unwrap(),
        "exec",
        "--embedded",
        "--worktree",
        "--model",
        "missing/model",
        "--format",
        "json",
        "prompt",
    ]);
    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
    let records: Vec<_> = std::fs::read_dir(repo.join(".git").join("cyber-worktrees"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
        .collect();
    assert_eq!(records.len(), 1, "{records:?}");
    let record: Value = serde_json::from_slice(&std::fs::read(&records[0]).unwrap()).unwrap();
    let name = record["name"].as_str().unwrap();
    let parts: Vec<_> = name.split('-').collect();
    assert!(
        parts.len() == 3
            && parts[..2]
                .iter()
                .all(|word| !word.is_empty() && word.bytes().all(|b| b.is_ascii_lowercase()))
            && parts[2].len() == 4
            && parts[2]
                .bytes()
                .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')),
        "generated name {name:?} is not <adjective>-<noun>-<4 hex>"
    );
    assert_eq!(record["branch"], format!("cyber/{name}"));
    assert_branch(&repo, &format!("cyber/{name}"));
    let target = Path::new(record["path"].as_str().unwrap());
    assert_eq!(target.file_name().unwrap(), name);
    let session = json(&output)["session_id"].as_str().unwrap().to_string();
    assert_eq!(
        session_directory(&env, &session).canonicalize().unwrap(),
        target.canonicalize().unwrap()
    );
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
    assert_eq!(v["tools"][0]["counts"]["sessions"], 0);
    assert_eq!(v["tools"][0]["read_time"]["agents"], 0);
    assert_eq!(v["complete"], false);
    assert!(!stdout(&o).contains("private-secret"));
    assert!(!e.root.join("cyber-home").exists());
    assert!(!e.root.join(".cyber").exists());
    let text = e.cyber(&["import", "--detect"]);
    assert!(text.status.success(), "{}", stderr(&text));
    assert!(stdout(&text).contains("claude: 1 agents; read-time: no"));
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

fn put(root: &Path, path: &str, text: &str) {
    let file = root.join(path);
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(file, text).unwrap();
}

/// A path as `cyber import` prints it: one line, control characters escaped.
fn shown(path: &Path) -> String {
    path.to_string_lossy()
        .chars()
        .flat_map(char::escape_debug)
        .collect()
}

fn import_reports(root: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(root.join(".cyber"))
        .map(|entries| {
            entries
                .flatten()
                .map(|e| e.path())
                .filter(|p| {
                    p.file_name()
                        .is_some_and(|n| n.to_string_lossy().starts_with("import-report-"))
                })
                .collect()
        })
        .unwrap_or_default()
}

const CODEX_SOURCE: &str = "model='coder'\nmodel_provider='local'\n[model_providers.local]\nbase_url='http://localhost:8000/v1'\napi_key='sk-private-literal-key'\n[mcp_servers.first]\ncommand='do-not-run'\n";

#[test]
fn import_claude_dry_run_prints_a_file_diff_and_writes_nothing() {
    let e = Env::new();
    std::fs::create_dir_all(e.root.join("home")).unwrap();
    put(
        &e.root,
        ".claude/settings.json",
        r#"{"model":"claude-sonnet-4-5","permissions":{"allow":["Bash(git status)"]}}"#,
    );
    let o = e.cyber(&["import", "claude", "--dry-run"]);
    assert!(o.status.success(), "{}", stderr(&o));
    let out = stdout(&o);
    let target = e.root.join("cyber.jsonc");
    assert!(out.contains("--- /dev/null\n"), "{out}");
    assert!(
        out.contains(&format!("+++ {}\n", target.display())),
        "{out}"
    );
    assert!(
        out.contains("+  \"model\": \"anthropic/claude-sonnet-4-5\""),
        "{out}"
    );
    let source = Path::new(".claude").join("settings.json");
    assert!(
        out.contains(&format!("+  // x-imported-from: {}", shown(&source))),
        "{out}"
    );
    assert!(out.contains("imported: "), "{out}");
    let report = e.root.join(".cyber").join("import-report-<timestamp>.md");
    assert!(
        out.contains(&format!(
            "New file: {} (the mapping report below)",
            shown(&report)
        )),
        "{out}"
    );
    assert!(out.ends_with("Dry run: no files written.\n"), "{out}");
    assert!(!target.exists());
    assert!(!e.root.join(".cyber").exists());
    assert!(!e.root.join("cyber-home").exists());
    existing_document_is_diffed_byte_for_byte(&e, &target);
}

/// An existing document is diffed byte for byte and keeps its comments.
fn existing_document_is_diffed_byte_for_byte(e: &Env, target: &Path) {
    let native = "{\n  // keep this comment\n  \"mode\": \"plan\"\n}\n";
    std::fs::write(target, native).unwrap();
    let o = e.cyber(&["import", "claude", "--dry-run"]);
    assert!(o.status.success(), "{}", stderr(&o));
    let out = stdout(&o);
    assert!(
        out.contains(&format!("--- {}\n", target.display())),
        "{out}"
    );
    assert!(out.contains("   // keep this comment\n"), "{out}");
    assert_eq!(std::fs::read_to_string(target).unwrap(), native);
    assert!(!e.root.join(".cyber").exists());
}

#[test]
fn import_with_no_sources_prints_none_found_and_exits_zero() {
    let e = Env::new();
    std::fs::create_dir_all(e.root.join("home")).unwrap();
    let o = e.cyber(&["import", "claude"]);
    assert!(o.status.success(), "{}", stderr(&o));
    assert_eq!(
        stdout(&o),
        format!(
            "no claude configuration found in {} or ~/.claude\n",
            shown(&e.root)
        )
    );
    let o = e.cyber(&["import", "auto", "--yes"]);
    assert!(o.status.success(), "{}", stderr(&o));
    assert!(
        stdout(&o).starts_with("no opencode, codex or claude configuration found in "),
        "{}",
        stdout(&o)
    );
    let v = json(&e.cyber(&["import", "codex", "--dry-run", "--format", "json"]));
    assert_eq!(v["found"], false);
    assert!(v["message"].as_str().unwrap().ends_with("or ~/.codex"));
    assert!(!e.root.join("cyber.jsonc").exists());
    assert!(!e.root.join(".cyber").exists());
}

#[test]
fn import_codex_yes_twice_writes_once_then_reports_nothing_to_do() {
    let e = Env::new();
    std::fs::create_dir_all(e.root.join("home")).unwrap();
    put(&e.root, ".codex/config.toml", CODEX_SOURCE);
    let first = e.cyber(&["import", "codex", "--yes"]);
    assert!(first.status.success(), "{}", stderr(&first));
    let target = e.root.join("cyber.jsonc");
    assert!(
        stdout(&first).contains(&format!("Wrote {}\n", shown(&target))),
        "{}",
        stdout(&first)
    );
    let written = std::fs::read_to_string(&target).unwrap();
    let source = Path::new(".codex").join("config.toml");
    assert_eq!(
        written
            .matches(&format!("// x-imported-from: {} sha256:", shown(&source)))
            .count(),
        3,
        "{written}"
    );
    assert_eq!(import_reports(&e.root).len(), 1);
    written_codex_config_loads_untrusted(&e);

    let second = e.cyber(&["import", "codex", "--yes"]);
    assert!(second.status.success(), "{}", stderr(&second));
    assert!(
        stdout(&second).ends_with("nothing to do\n"),
        "{}",
        stdout(&second)
    );
    assert!(!stdout(&second).contains("+++"), "{}", stdout(&second));
    assert_eq!(std::fs::read_to_string(&target).unwrap(), written);
    assert_eq!(import_reports(&e.root).len(), 1);
    let v = json(&e.cyber(&["import", "codex", "--yes", "--format", "json"]));
    assert_eq!(v["nothing_to_do"], true);
    assert!(v["written"].is_null());
}

/// Regression: a global import never created the global configuration directory, so on
/// a machine without one the write failed with "native configuration could not be
/// written".
#[test]
fn import_global_scope_creates_the_missing_global_configuration_directory() {
    let e = Env::new();
    put(&e.root, "home/.codex/config.toml", CODEX_SOURCE);
    let global = e.root.join("cyber-home").join("config");
    assert!(!global.exists());
    let o = e.cyber(&["import", "codex", "--scope", "global", "--yes"]);
    assert!(o.status.success(), "{}", stderr(&o));
    let written = std::fs::read_to_string(global.join("cyber.jsonc")).unwrap();
    assert!(written.contains("x-imported-from:"), "{written}");
    let reports: Vec<_> = std::fs::read_dir(&global)
        .unwrap()
        .flatten()
        .filter(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with("import-report-")
        })
        .collect();
    assert_eq!(reports.len(), 1);
}

/// Regression: the target was replaced before the mapping report was created, so a
/// report that could not be written failed the import after `cyber.jsonc` had already
/// changed. (A linked or non-directory `.cyber` is refused earlier, by the preview.)
#[cfg(unix)]
#[test]
fn import_whose_report_cannot_be_written_leaves_the_target_untouched() {
    use std::os::unix::fs::PermissionsExt;
    let e = Env::new();
    std::fs::create_dir_all(e.root.join("home")).unwrap();
    put(
        &e.root,
        ".claude/settings.json",
        r#"{"model":"claude-sonnet-4-5","permissions":{"allow":["Bash(git status)"]}}"#,
    );
    let native = "{\n  // keep this comment\n  \"mode\": \"plan\"\n}\n";
    let target = e.root.join("cyber.jsonc");
    std::fs::write(&target, native).unwrap();
    let reports = e.root.join(".cyber");
    std::fs::create_dir(&reports).unwrap();
    std::fs::set_permissions(&reports, std::fs::Permissions::from_mode(0o555)).unwrap();
    if std::fs::write(reports.join("probe"), "").is_ok() {
        return; // Permissions are not enforced for this user (root).
    }
    let o = e.cyber(&["import", "claude", "--yes"]);
    std::fs::set_permissions(&reports, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(!o.status.success(), "{}", stdout(&o));
    assert!(
        stderr(&o).contains("import report could not be written"),
        "{}",
        stderr(&o)
    );
    assert_eq!(std::fs::read_to_string(&target).unwrap(), native);
}

#[test]
fn import_without_yes_writes_only_after_an_explicit_yes() {
    use std::io::Write;
    let e = Env::new();
    std::fs::create_dir_all(e.root.join("home")).unwrap();
    put(&e.root, ".codex/config.toml", CODEX_SOURCE);
    let declined = e.cyber(&["import", "codex"]);
    assert_eq!(declined.status.code(), Some(1), "{}", stderr(&declined));
    assert!(stderr(&declined).contains("[y/N]"), "{}", stderr(&declined));
    assert!(stderr(&declined).contains("Import cancelled; no files written."));
    assert!(stdout(&declined).contains("+++"), "{}", stdout(&declined));
    assert!(!e.root.join("cyber.jsonc").exists());
    assert!(!e.root.join(".cyber").exists());

    let mut child = e
        .command(&["import", "codex"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(b"y\n").unwrap();
    let accepted = child.wait_with_output().unwrap();
    assert!(accepted.status.success(), "{}", stderr(&accepted));
    assert!(e.root.join("cyber.jsonc").exists());
    assert_eq!(import_reports(&e.root).len(), 1);
}

#[test]
fn import_reimport_updates_its_own_annotated_entries_and_keeps_edited_ones() {
    let e = Env::new();
    std::fs::create_dir_all(e.root.join("home")).unwrap();
    put(&e.root, ".codex/config.toml", CODEX_SOURCE);
    assert!(e.cyber(&["import", "codex", "--yes"]).status.success());
    let target = e.root.join("cyber.jsonc");
    put(
        &e.root,
        ".codex/config.toml",
        &CODEX_SOURCE.replace("do-not-run", "still-not-run"),
    );
    let updated = e.cyber(&["import", "codex", "--yes"]);
    assert!(updated.status.success(), "{}", stderr(&updated));
    assert!(
        stdout(&updated).contains("entry of an earlier import updated from its source"),
        "{}",
        stdout(&updated)
    );
    let text = std::fs::read_to_string(&target).unwrap();
    assert!(text.contains("\"still-not-run\"") && !text.contains("\"do-not-run\""));
    assert_eq!(text.matches("\"mcp\"").count(), 1, "{text}");

    // A hand-edited entry is the user's now: it is kept and reported as merged.
    std::fs::write(&target, text.replace("still-not-run", "my-own-server")).unwrap();
    put(
        &e.root,
        ".codex/config.toml",
        &CODEX_SOURCE.replace("do-not-run", "third-server"),
    );
    let kept = e.cyber(&["import", "codex", "--yes"]);
    assert!(kept.status.success(), "{}", stderr(&kept));
    assert!(
        stdout(&kept).contains("kept: \"my-own-server\""),
        "{}",
        stdout(&kept)
    );
    assert!(
        stdout(&kept).ends_with("nothing to do\n"),
        "{}",
        stdout(&kept)
    );
    assert!(
        std::fs::read_to_string(&target)
            .unwrap()
            .contains("my-own-server")
    );
}

#[test]
fn import_writes_report_file_listing_merged_model_with_both_values() {
    let e = Env::new();
    std::fs::create_dir_all(e.root.join("home")).unwrap();
    std::fs::write(e.root.join("cyber.jsonc"), r#"{"model":"native/kept"}"#).unwrap();
    put(&e.root, ".codex/config.toml", CODEX_SOURCE);
    let o = e.cyber(&["import", "codex", "--yes", "--format", "json"]);
    assert!(o.status.success(), "{}", stderr(&o));
    let v = json(&o);
    let report = PathBuf::from(v["written"]["report"].as_str().unwrap());
    assert_eq!(import_reports(&e.root), vec![report.clone()]);
    let name = report.file_name().unwrap().to_string_lossy().into_owned();
    assert!(
        name.len() == "import-report-20260101T000000Z.md".len() && name.ends_with("Z.md"),
        "{name}"
    );
    let text = std::fs::read_to_string(&report).unwrap();
    let merged = text.split("\n## merged\n").nth(1).unwrap();
    let merged = merged.split("\n## ").next().unwrap();
    assert!(merged.contains("- `/model` from `"), "{text}");
    assert!(merged.contains("  - kept: `\"native/kept\"`"), "{text}");
    assert!(merged.contains("  - ignored: `\"local/coder\"`"), "{text}");
    assert!(text.contains("\n## imported\n"), "{text}");
    // The report is Markdown: the path as written, not JSON-escaped (on Windows, with or
    // without its `\\?\` prefix).
    let source = e
        .root
        .join(".codex")
        .join("config.toml")
        .display()
        .to_string();
    assert!(
        text.contains(&source) || text.contains(source.trim_start_matches(r"\\?\")),
        "{source} not in {text}"
    );
    assert_eq!(
        crate_config(&e)["model"],
        "native/kept",
        "the existing key is kept"
    );
}

/// The real configuration loader accepts the written document, and the imported provider
/// and MCP server stay inactive until the project is trusted.
fn written_codex_config_loads_untrusted(e: &Env) {
    let config = e.cyber(&["debug", "config", "--format", "json"]);
    assert!(config.status.success(), "{}", stderr(&config));
    assert_eq!(json(&config)["model"], "local/coder");
    assert!(json(&config).get("mcp").is_none());
    assert!(
        stderr(&config).contains("2 project definition(s) are inactive until approved"),
        "{}",
        stderr(&config)
    );
}

fn crate_config(e: &Env) -> Value {
    json(&e.cyber(&["debug", "config", "--format", "json"]))
}

#[test]
fn import_literal_codex_api_key_never_reaches_cyber_jsonc() {
    let e = Env::new();
    std::fs::create_dir_all(e.root.join("home")).unwrap();
    // The user's own literal key in the existing document is redacted from the diff too.
    std::fs::write(
        e.root.join("cyber.jsonc"),
        "{\n  \"providers\": {\"own\": {\"api\": {\"settings\": {\"api_key\": \"sk-native-own-key\"}}}}\n}\n",
    )
    .unwrap();
    put(&e.root, ".codex/config.toml", CODEX_SOURCE);
    let o = e.cyber(&["import", "codex", "--yes"]);
    assert!(o.status.success(), "{}", stderr(&o));
    let out = stdout(&o);
    for text in [&out, &stderr(&o)] {
        assert!(!text.contains("sk-private-literal-key"), "{text}");
        assert!(!text.contains("sk-native-own-key"), "{text}");
    }
    let written = std::fs::read_to_string(e.root.join("cyber.jsonc")).unwrap();
    assert!(!written.contains("sk-private-literal-key"), "{written}");
    let variable = "CYBER_IMPORT_CODEX_6C6F63616C_API_KEY";
    assert!(
        written.contains(&format!("\"api_key\": \"{{env:{variable}}}\"")),
        "{written}"
    );
    let report = std::fs::read_to_string(&import_reports(&e.root)[0]).unwrap();
    assert!(!report.contains("sk-private-literal-key"), "{report}");
    assert!(!report.contains("sk-native-own-key"), "{report}");
    assert!(
        report.contains(&format!(
            "- `{variable}` from the source field `/model_providers/local/api_key`"
        )),
        "{report}"
    );
    assert!(out.contains(&format!("Set {variable} from the source field")));
    // No other file the import touched holds the key either.
    assert!(!e.root.join("cyber-home").exists());
}

/// Copy the checked-in tree at `from` into `to`.
fn copy_tree(from: &Path, to: &Path) {
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            std::fs::create_dir_all(&target).unwrap();
            copy_tree(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}

/// The M1.5 exit criterion (ROADMAP): `cyber import claude` reproduces a reference Claude
/// Code setup — agents, skills, a command, hooks, MCP servers, permissions and settings —
/// with nothing left to port by hand. After the workspace-trust approval every security
/// review requires, the binary's own inspection commands show each item live.
#[test]
fn import_claude_reproduces_reference_setup_without_manual_edits() {
    let e = Env::new();
    std::fs::create_dir_all(e.root.join("home")).unwrap();
    std::fs::create_dir_all(e.root.join(".git")).unwrap();
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/claude-reference");
    copy_tree(&fixture, &e.root);
    let o = e.cyber(&["import", "claude", "--yes"]);
    assert!(o.status.success(), "{}", stderr(&o));
    reference_report_leaves_nothing_to_port(&e);
    let cwd = e.root.to_str().unwrap();
    approve_checkout(&e);

    reference_settings_are_live(&e);
    reference_extensions_are_live(&e, cwd);
    // A second import finds nothing left to do.
    let again = e.cyber(&["import", "claude", "--yes"]);
    assert!(
        stdout(&again).ends_with("nothing to do\n"),
        "{}",
        stdout(&again)
    );
}

/// The mapping report imports every reference item and lists nothing to port or set.
fn reference_report_leaves_nothing_to_port(e: &Env) {
    let reports = import_reports(&e.root);
    assert_eq!(reports.len(), 1);
    let report = std::fs::read_to_string(&reports[0]).unwrap();
    for section in [
        "## requires manual port",
        "## not imported",
        "## merged",
        "## Environment to set",
    ] {
        assert!(!report.contains(section), "{section}:\n{report}");
    }
    for item in [
        "`/agents/reviewer`",
        "`/hooks/PreToolUse/0`",
        "`/mcp/docs/command`",
        "`/commands/ship/template`",
        "`/fallback_models`",
    ] {
        assert!(report.contains(item), "{item}:\n{report}");
    }
    assert!(
        report.contains("SKILL.md`: read in place by read-time skill compatibility"),
        "{report}"
    );
}

/// Model, fallback, mode, environment, permission rules and the agent, as loaded.
fn reference_settings_are_live(e: &Env) {
    let config = crate_config(e);
    assert_eq!(config["model"], "anthropic/claude-sonnet-5-5");
    assert_eq!(
        config["fallback_models"],
        serde_json::json!(["anthropic/claude-haiku-4-5"])
    );
    assert_eq!(config["mode"], "accept-edits");
    assert_eq!(config["sandbox"]["env"]["set"]["NODE_ENV"], "development");
    let rules = config["permissions"]["rules"].to_string();
    for rule in [
        r#"{"action":"bash","effect":"allow","resource":"npm run test *"}"#,
        r#"{"action":"read","effect":"deny","resource":".env"}"#,
    ] {
        let rule: Value = serde_json::from_str(rule).unwrap();
        assert!(
            config["permissions"]["rules"]
                .as_array()
                .unwrap()
                .contains(&rule),
            "{rules}"
        );
    }
    let reviewer = &config["agents"]["reviewer"];
    assert_eq!(reviewer["model"], "anthropic/claude-opus-5-5");
    assert_eq!(reviewer["mode"], "subagent");
    assert!(
        reviewer["system"]
            .as_str()
            .unwrap()
            .starts_with("You are a careful code reviewer.")
    );
}

/// Hooks, MCP servers, the skill and the command, through the binary's inspection commands.
fn reference_extensions_are_live(e: &Env, cwd: &str) {
    let hooks = json(&e.cyber(&["hooks", "list", "--cwd", cwd, "--format", "json"]));
    let events: Vec<_> = hooks["hooks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|h| {
            (
                h["event"].as_str().unwrap().to_owned(),
                h["matcher"].clone(),
            )
        })
        .collect();
    for (event, matcher) in [
        ("PreToolUse", serde_json::json!("bash")),
        ("PostToolUse", serde_json::json!("{edit,write}")),
        ("SessionStart", Value::Null),
    ] {
        assert!(
            events.contains(&(event.to_owned(), matcher.clone())),
            "{event} {matcher}: {hooks}"
        );
    }
    assert!(
        hooks
            .to_string()
            .contains("$CYBER_PROJECT_DIR/.claude/hooks/guard.sh"),
        "{hooks}"
    );

    let mcp = json(&e.cyber(&["mcp", "definitions", "--cwd", cwd, "--format", "json"]));
    let names: Vec<_> = mcp["servers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["name"].as_str().unwrap())
        .collect();
    assert!(
        names.contains(&"docs") && names.contains(&"tracker"),
        "{mcp}"
    );

    let skills = json(&e.cyber(&["skills", "list", "--cwd", cwd, "--format", "json"]));
    assert!(skills.to_string().contains("\"release-notes\""), "{skills}");
    let commands = json(&e.cyber(&["commands", "list", "--cwd", cwd, "--format", "json"]));
    assert!(commands.to_string().contains("\"ship\""), "{commands}");
}

/// Approve the checkout's project configuration, as `cyber trust approve` does.
fn approve_checkout(e: &Env) {
    let cwd = e.root.to_str().unwrap();
    let inspect = json(&e.cyber(&["trust", "inspect", "--cwd", cwd, "--format", "json"]));
    if let Some(digest) = inspect["digest"].as_str() {
        assert!(
            e.cyber(&["trust", "approve", "--cwd", cwd, "--digest", digest])
                .status
                .success()
        );
    }
}

const CODEX_PROFILES: &str = r#"profile = "careful"
notify = ["notify-send", "Codex"]
default_permissions = "build"
approval_policy = { granular = { sandbox_approval = false } }
[profiles.careful]
model = "gpt-6"
approval_policy = "on-request"
sandbox_mode = "read-only"
[permissions.build]
extends = ":workspace"
filesystem = { "secrets/**" = "none" }
network = { enabled = false }
[shell_environment_policy]
inherit = "core"
set = { CI = "1" }
"#;

/// `compat-import` → Codex sources through the binary: profiles, a named permission
/// profile, the environment policy, granular approvals and `notify` load as native
/// settings, and `requirements.toml` is reported with its org-policy equivalents.
#[test]
fn import_codex_profiles_permissions_and_notify_load_through_the_binary() {
    let e = Env::new();
    std::fs::create_dir_all(e.root.join("home")).unwrap();
    std::fs::create_dir_all(e.root.join(".git")).unwrap();
    put(&e.root, ".codex/config.toml", CODEX_PROFILES);
    put(
        &e.root,
        ".codex/requirements.toml",
        "allowed_approval_policies = [\"on-request\"]\n",
    );
    put(
        &e.root,
        ".codex/rules/git.rules",
        "prefix_rule(pattern=[\"git\",\"push\"], decision=\"forbidden\")\n",
    );
    let o = e.cyber(&["import", "codex", "--yes"]);
    assert!(o.status.success(), "{}", stderr(&o));
    let report = std::fs::read_to_string(&import_reports(&e.root)[0]).unwrap();
    assert!(
        report.contains("`/allowed_approval_policies`")
            && report.contains("the org-policy equivalent is modes.disable"),
        "{report}"
    );
    approve_checkout(&e);
    let config = crate_config(&e);
    // `profile = "careful"` is the default profile, so it applies.
    assert_eq!(config["default_profile"], "careful");
    assert_eq!(config["model"], "openai/gpt-6");
    assert_eq!(config["sandbox"]["policy"], "read-only");
    assert_eq!(config["sandbox"]["profile"], "build");
    let rules = config["permissions"]["rules"].as_array().unwrap();
    assert!(
        rules
            .iter()
            .any(|r| r["action"] == "sandbox.escalate" && r["effect"] == "deny"),
        "{rules:?}"
    );
    // "Execpolicy rule converted".
    assert!(
        rules.iter().any(|r| r["action"] == "bash"
            && r["resource"] == "git push *"
            && r["effect"] == "deny"),
        "{rules:?}"
    );
    let explain = json(&e.cyber(&[
        "sandbox",
        "explain",
        "--profile",
        "build",
        "--format",
        "json",
    ]));
    assert_eq!(explain["network"], "off", "{explain}");
    assert_eq!(explain["env_mode"], "none", "{explain}");
    assert_eq!(explain["env_set"]["CI"], "1", "{explain}");
    assert!(
        explain["deny_read"].to_string().contains("secrets/**"),
        "{explain}"
    );
    let cwd = e.root.to_str().unwrap();
    let hooks = json(&e.cyber(&["hooks", "list", "--cwd", cwd, "--format", "json"]));
    assert!(
        hooks["hooks"]
            .as_array()
            .unwrap()
            .iter()
            .any(|h| h["event"] == "Notification"),
        "{hooks}"
    );
}

/// `compat-import` → OpenCode sources through the binary: Markdown agents and modes,
/// `instructions` and skills, with plugins reported for a manual port with the guide link.
#[test]
fn import_opencode_agents_modes_and_plugins_through_the_binary() {
    let e = Env::new();
    std::fs::create_dir_all(e.root.join("home")).unwrap();
    std::fs::create_dir_all(e.root.join(".git")).unwrap();
    put(
        &e.root,
        "opencode.json",
        r#"{"instructions":["CONTRIBUTING.md"]}"#,
    );
    put(
        &e.root,
        ".opencode/agents/docs.md",
        "---\ndescription: Writes docs\nmodel: anthropic/claude-sonnet-5-5\n---\nWrite the docs.\n",
    );
    put(&e.root, ".opencode/modes/focus.md", "Stay on the task.\n");
    put(
        &e.root,
        ".opencode/plugins/notify.ts",
        "export default {}\n",
    );
    put(
        &e.root,
        ".opencode/skills/lint/SKILL.md",
        "---\nname: lint\ndescription: Lint the code\n---\nRun the linter.\n",
    );
    let o = e.cyber(&["import", "opencode", "--yes"]);
    assert!(o.status.success(), "{}", stderr(&o));
    let report = std::fs::read_to_string(&import_reports(&e.root)[0]).unwrap();
    assert!(report.contains("## requires manual port"), "{report}");
    assert!(
        report.contains("plugin kit migration guide: https://"),
        "{report}"
    );
    assert!(
        report.contains("SKILL.md`: read in place by read-time skill compatibility"),
        "{report}"
    );
    approve_checkout(&e);
    let config = crate_config(&e);
    assert_eq!(config["agents"]["docs"]["mode"], "all");
    assert_eq!(config["agents"]["docs"]["system"], "Write the docs.\n");
    assert_eq!(config["agents"]["focus"]["mode"], "primary");
    assert_eq!(
        config["instructions"],
        serde_json::json!(["CONTRIBUTING.md"])
    );
    let cwd = e.root.to_str().unwrap();
    let skills = json(&e.cyber(&["skills", "list", "--cwd", cwd, "--format", "json"]));
    assert!(skills.to_string().contains("\"lint\""), "{skills}");
}

#[test]
fn import_detect_reports_read_time_coverage_per_item() {
    let e = Env::new();
    let home = e.root.join("home");
    for agent in ["a", "b", "c"] {
        put(
            &e.root,
            &format!(".claude/agents/{agent}.md"),
            "private-agent",
        );
    }
    put(
        &e.root,
        ".claude/skills/check/SKILL.md",
        "---\nname: check\ndescription: d\n---\n",
    );
    put(&e.root, ".claude/commands/test.md", "private-command");
    put(
        &e.root,
        ".mcp.json",
        r#"{"mcpServers":{"docs":{"command":"never-run"}}}"#,
    );
    put(
        &home,
        ".codex/config.toml",
        "[mcp_servers.one]\ncommand='x'\n[mcp_servers.two]\ncommand='y'\n",
    );
    let plain = e.root.to_string_lossy().replace(r"\\?\", "");
    let encoded: String = plain
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    for session in ["one", "two"] {
        put(
            &home,
            &format!(".claude/projects/{encoded}/{session}.jsonl"),
            "{}\n",
        );
    }
    put(&home, ".claude/projects/-elsewhere/three.jsonl", "{}\n");
    let meta = |cwd: &str| {
        format!(
            "{}\n",
            serde_json::json!({"type":"session_meta","payload":{"cwd":cwd}})
        )
    };
    put(
        &home,
        ".codex/sessions/2026/10/01/rollout-here.jsonl",
        &meta(&plain),
    );
    put(
        &home,
        ".codex/sessions/2026/10/01/rollout-elsewhere.jsonl",
        &meta("/elsewhere"),
    );
    let o = e.cyber(&["import", "--detect"]);
    assert!(o.status.success(), "{}", stderr(&o));
    let out = stdout(&o);
    for line in [
        "claude: 3 agents; read-time: no",
        "claude: 1 commands; read-time: yes",
        "claude: 1 skills; read-time: yes",
        "claude: 1 mcp servers; read-time: yes",
        "claude: 2 sessions; read-time: no",
        "codex: 2 mcp servers; read-time: no",
        "codex: 1 sessions; read-time: no",
    ] {
        assert!(out.contains(&format!("{line}\n")), "{line}: {out}");
    }
    let v = json(&e.cyber(&["import", "--detect", "--format", "json"]));
    let claude = v["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["tool"] == "claude")
        .unwrap();
    assert_eq!(claude["counts"]["sessions"], 2);
    assert_eq!(claude["read_time"]["skills"], 1);
    assert_eq!(claude["read_time"]["agents"], 0);
    assert!(!out.contains("private-agent"));
    assert!(!e.root.join("cyber-home").exists());
    assert!(!e.root.join(".cyber").exists());
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
    // The instruction entry is imported as a path; the file it names is never read.
    assert!(diff.contains("never-read-source-instruction"));
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

/// Reads `notes.txt` once when asked to, then answers.
fn stats_reply(body: &Value) -> Value {
    let asked = body["messages"].to_string().contains("read the notes");
    let answered = body["messages"]
        .as_array()
        .is_some_and(|m| m.iter().any(|m| m["role"] == "tool"));
    if asked && !answered {
        let arguments = serde_json::json!({ "path": "notes.txt" }).to_string();
        return serde_json::json!({ "tool_calls": [{ "index": 0, "id": "call_read",
            "type": "function", "function": { "name": "read", "arguments": arguments } }] });
    }
    serde_json::json!({ "content": "done" })
}

/// The value of a `cyber stats` text row.
fn stats_row(text: &str, label: &str) -> String {
    text.lines()
        .find_map(|line| line.strip_prefix(label))
        .unwrap_or_else(|| panic!("no {label:?} row in\n{text}"))
        .trim()
        .to_owned()
}

/// Two answered Sessions through the shared server: one in project `a` that reads a
/// file, one in project `b`.
fn stats_sessions(service: &StartedService<'_>, root: &Path) -> (PathBuf, PathBuf) {
    let (a, b) = (root.join("a"), root.join("b"));
    for dir in [&a, &b] {
        std::fs::create_dir_all(dir.join(".git")).unwrap();
    }
    std::fs::write(a.join("notes.txt"), "notes").unwrap();
    for (dir, prompt) in [(&a, "read the notes"), (&b, "hello")] {
        let ran = service.cyber(&[
            "--cwd",
            dir.to_str().unwrap(),
            "exec",
            "--model",
            "test/main",
            prompt,
        ]);
        exits(&ran, 0);
    }
    (a, b)
}

/// `--format json` is the route's answer unchanged.
fn stats_json_is_the_route(service: &StartedService<'_>) {
    let month = json(&exits_ok(
        &service.cyber(&["stats", "--days", "30", "--format", "json"]),
    ));
    let route = json(&exits_ok(&service.cyber(&[
        "api",
        "v1.usage.stats",
        "--param",
        "period=30d",
    ])));
    assert_eq!(month, route["data"]);
    assert_eq!(month["period"], "30d");
    assert_eq!(month["days"].as_array().unwrap().len(), 30);
    assert_eq!(month["totals"]["sessions"], 2, "{month}");
    assert_eq!(month["current_streak"], 1);
    assert_eq!(month["favorite_model"]["model"], "main");
}

/// The text shows the same numbers as the JSON.
fn stats_text_matches_json(service: &StartedService<'_>) {
    let week = exits_ok(&service.cyber(&["stats", "--days", "7", "--models"]));
    let text = stdout(&week);
    let json7 = json(&exits_ok(
        &service.cyber(&["stats", "--days", "7", "--format", "json"]),
    ));
    assert!(text.starts_with("Last 7 days · all projects · "), "{text}");
    assert_eq!(stats_row(&text, "Sessions"), "2");
    assert_eq!(
        stats_row(&text, "Total tokens"),
        json7["totals"]["total_tokens"].to_string()
    );
    assert_eq!(
        stats_row(&text, "Turns"),
        json7["totals"]["turns"].to_string()
    );
    assert_eq!(stats_row(&text, "Active days"), "1/7");
    // Regression: one day was printed as `1 days`.
    assert_eq!(stats_row(&text, "Current streak"), "1 day");
    let tokens = &json7["totals"]["tokens"];
    let spoken = tokens["input"].as_u64().unwrap() + tokens["output"].as_u64().unwrap();
    let book = cyber_tui::books::comparison(spoken).unwrap();
    assert!(
        text.lines().any(|line| line == book),
        "{book:?} not in\n{text}"
    );
    let day = stdout(&exits_ok(&service.cyber(&["stats", "--days", "1"])));
    assert!(day.starts_with("Last 1 day · all projects · "), "{day}");
    assert!(stats_row(&text, "Total cost").starts_with('$'), "{text}");
    let row = text
        .lines()
        .find(|l| l.starts_with("test/main "))
        .unwrap_or_else(|| panic!("no model row in\n{text}"));
    assert!(row.contains("100%"), "{row}");
}

/// Only the project of `a`, and tool calls by name.
fn stats_scope_and_tools(service: &StartedService<'_>, a: &Path) {
    let scoped = json(&exits_ok(&service.cyber(&[
        "stats",
        "--project",
        a.to_str().unwrap(),
        "--tools",
        "--format",
        "json",
    ])));
    assert_eq!(scoped["totals"]["sessions"], 1, "{scoped}");
    assert_eq!(
        scoped["tools"],
        serde_json::json!([{ "name": "read", "calls": 1 }])
    );
    let all = json(&exits_ok(
        &service.cyber(&["stats", "--tools", "--format", "json"]),
    ));
    assert_eq!(all["period"], "all");
    assert_eq!(all["tools"][0]["name"], "read");
}

#[test]
fn stats_render_the_usage_stats_route_for_days_projects_models_and_tools() {
    let model = scripted_model::ScriptedModel::start(stats_reply);
    let env = Env::new();
    let catalog = env.root.join("models.json");
    std::fs::write(&catalog, "{}").unwrap();
    let mut config = model.config();
    config["providers"]["test"]["models"]["main"] =
        serde_json::json!({ "cost": { "input": 10.0, "output": 20.0 } });
    config["permissions"] = serde_json::json!("allow");
    let home = env.root.join("cyber-home/config");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::write(home.join("cyber.jsonc"), config.to_string()).unwrap();
    let service = StartedService(&env, catalog);
    let (a, _b) = stats_sessions(&service, &env.root);
    stats_json_is_the_route(&service);
    stats_text_matches_json(&service);
    stats_scope_and_tools(&service, &a);
    exits(&service.cyber(&["stats", "--days", "0"]), 2);
    exits(&service.cyber(&["stats", "--project", "missing-dir"]), 2);
}

fn exits_ok(output: &Output) -> Output {
    exits(output, 0);
    output.clone()
}

/// Global `worktrees.cleanup: keep`, so exec leaves its clean checkouts in place.
fn keep_worktrees(env: &Env) {
    write_global_config(env, serde_json::json!({"worktrees": {"cleanup": "keep"}}));
}

/// Write the global config, with explicit `full-access` on Windows: worktree Sessions
/// run git tools, and these tests cover worktrees, not the AppContainer launch.
fn write_global_config(env: &Env, mut config: Value) {
    if cfg!(windows) {
        config["sandbox"] = serde_json::json!({"policy": "full-access"});
    }
    let path = env.root.join("cyber-home/config/cyber.jsonc");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, config.to_string()).unwrap();
}

/// `cyber exec --worktree <name>` against an unavailable model: the Session starts in
/// the checkout and ends without a provider call. Returns the checkout path.
fn exec_in_worktree(env: &Env, repo: &Path, name: &str) -> PathBuf {
    let output = env.cyber(&[
        "--cwd",
        repo.to_str().unwrap(),
        "exec",
        "--embedded",
        "--worktree",
        name,
        "--model",
        "missing/model",
        "--format",
        "json",
        "prompt",
    ]);
    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
    worktree_path(repo, name)
}

fn worktree_path(repo: &Path, name: &str) -> PathBuf {
    let record: Value = serde_json::from_slice(
        &std::fs::read(
            repo.join(".git/cyber-worktrees")
                .join(format!("{name}.json")),
        )
        .unwrap(),
    )
    .unwrap();
    PathBuf::from(record["path"].as_str().unwrap())
}

fn branch_exists(repo: &Path, branch: &str) -> bool {
    Command::new("git")
        .current_dir(repo)
        .args([
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("refs/heads/{branch}"),
        ])
        .stdout(std::process::Stdio::null())
        .status()
        .unwrap()
        .success()
}

#[test]
fn exec_worktree_without_changes_is_removed_on_exit() {
    let env = Env::new();
    let repo = git_repo(&env);
    write_global_config(&env, serde_json::json!({}));
    let output = env.cyber(&[
        "--cwd",
        repo.to_str().unwrap(),
        "exec",
        "--embedded",
        "--worktree",
        "tidy",
        "--model",
        "missing/model",
        "--format",
        "json",
        "prompt",
    ]);
    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
    assert!(
        json(&output)["session_id"]
            .as_str()
            .unwrap()
            .starts_with("ses_")
    );
    assert!(
        !stderr(&output).contains("worktree kept"),
        "{}",
        stderr(&output)
    );
    assert!(!repo.join(".git/cyber-worktrees/tidy.json").exists());
    assert!(!branch_exists(&repo, "cyber/tidy"));
    let root = env.root.join("cyber-home/data/worktrees");
    let leftover: Vec<_> = std::fs::read_dir(&root)
        .into_iter()
        .flatten()
        .flat_map(|project| std::fs::read_dir(project.unwrap().path()).unwrap())
        .map(|entry| entry.unwrap().path())
        .collect();
    assert!(leftover.is_empty(), "{leftover:?}");
}

#[test]
fn worktree_remove_refuses_dirty_without_force() {
    let env = Env::new();
    let repo = git_repo(&env);
    keep_worktrees(&env);
    let spike = exec_in_worktree(&env, &repo, "spike");
    std::fs::write(spike.join("notes.txt"), "work in progress").unwrap();
    let cwd = repo.to_str().unwrap();
    let refused = env.cyber(&["--cwd", cwd, "worktree", "remove", "spike"]);
    let forced = if refused.status.success() {
        None
    } else {
        assert!(spike.join("notes.txt").exists());
        assert!(branch_exists(&repo, "cyber/spike"));
        Some(env.cyber(&["--cwd", cwd, "worktree", "remove", "spike", "--force"]))
    };
    let missing = env.cyber(&["--cwd", cwd, "worktree", "remove", "nothing"]);
    let stopped = env.cyber(&["service", "stop"]);
    assert!(stopped.status.success(), "{}", stderr(&stopped));
    assert!(!refused.status.success(), "{}", stdout(&refused));
    assert!(
        stderr(&refused).contains("uncommitted changes"),
        "{}",
        stderr(&refused)
    );
    let forced = forced.unwrap();
    assert!(forced.status.success(), "{}", stderr(&forced));
    assert!(
        stdout(&forced).starts_with("removed spike"),
        "{}",
        stdout(&forced)
    );
    assert!(!spike.exists());
    assert!(!branch_exists(&repo, "cyber/spike"));
    assert!(!missing.status.success());
    assert!(
        stderr(&missing).contains("No managed worktree named nothing"),
        "{}",
        stderr(&missing)
    );
}

#[test]
fn worktree_prune_removes_only_older_clean_worktrees() {
    let env = Env::new();
    let repo = git_repo(&env);
    keep_worktrees(&env);
    let old_clean = exec_in_worktree(&env, &repo, "old-clean");
    let old_dirty = exec_in_worktree(&env, &repo, "old-dirty");
    let recent = exec_in_worktree(&env, &repo, "recent");
    std::fs::write(old_dirty.join("draft.txt"), "draft").unwrap();
    let month_ago = std::time::SystemTime::now() - std::time::Duration::from_secs(30 * 86_400);
    for name in ["old-clean", "old-dirty"] {
        std::fs::File::options()
            .write(true)
            .open(
                repo.join(".git/cyber-worktrees")
                    .join(format!("{name}.json")),
            )
            .unwrap()
            .set_modified(month_ago)
            .unwrap();
    }
    let cwd = repo.to_str().unwrap();
    let pruned = env.cyber(&["--cwd", cwd, "worktree", "prune", "--format", "json"]);
    let stopped = env.cyber(&["service", "stop"]);
    assert!(stopped.status.success(), "{}", stderr(&stopped));
    assert!(pruned.status.success(), "{}", stderr(&pruned));
    assert_eq!(
        json(&pruned),
        serde_json::json!({"removed": ["old-clean"], "skipped": []})
    );
    assert!(!old_clean.exists());
    assert!(!branch_exists(&repo, "cyber/old-clean"));
    assert!(old_dirty.join("draft.txt").exists());
    assert!(recent.exists());
    assert!(branch_exists(&repo, "cyber/recent"));
}

/// The scripted model holds the Turn, so the exec Session is running in `spike`.
fn held(_: &Value) -> Value {
    serde_json::json!({"hold": true})
}

#[test]
fn worktree_remove_while_session_active_fails_in_use() {
    let model = scripted_model::ScriptedModel::start(held);
    let env = Env::new();
    let repo = git_repo(&env);
    let catalog = env.root.join("models.json");
    std::fs::write(&catalog, "{}").unwrap();
    write_global_config(&env, model.config());
    let service = StartedService(&env, catalog);
    let cwd = repo.to_str().unwrap();
    let exec = env
        .command(&[
            "--cwd",
            cwd,
            "exec",
            "--worktree",
            "spike",
            "--format",
            "json",
            "hello",
        ])
        .env("CYBER_MODELS_PATH", &service.1)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    // The concurrent title request carries the prompt too, before the Turn holds the
    // Location, so only the Turn's own request proves the Session uses the checkout.
    let exec = poll_while_running("the Turn at the model", exec, || {
        turn_requests(&model)
            .iter()
            .any(|body| body.contains("hello"))
    });
    let refused = service.cyber(&["--cwd", cwd, "worktree", "remove", "spike", "--force"]);
    model.release();
    let output = exec.wait_with_output().unwrap();
    assert!(
        !refused.status.success(),
        "{} exec: {}",
        stdout(&refused),
        stderr(&output)
    );
    let message = stderr(&refused);
    assert!(message.contains("Worktree in use by ses_"), "{message}");
    let session = json(&output)["session_id"].as_str().unwrap().to_owned();
    assert!(message.contains(&session), "{message} {session}");
    // Released, the Session finished without changes and its exit cleanup removed it.
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(!repo.join(".git/cyber-worktrees/spike.json").exists());
    assert!(!branch_exists(&repo, "cyber/spike"));
}

/// The global configuration file of an `Env`.
fn global_config(env: &Env, config: Value) {
    let dir = env.root.join("cyber-home/config");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("cyber.jsonc"), config.to_string()).unwrap();
}

/// `sandbox` → Sandbox CLI: `cyber sandbox explain` prints the writable roots, network
/// mode, allowed domains and the names (never values) of masked variables.
#[test]
fn sandbox_explain_prints_roots_network_domains_and_masked_names() {
    let env = Env::new();
    let repo = env.repo("{}");
    global_config(
        &env,
        serde_json::json!({"sandbox": {"writable_roots": ["~/extra"], "allowed_domains": ["example.com"]}}),
    );
    let o = env
        .command(&["sandbox", "explain"])
        .current_dir(&repo)
        .env("OPENAI_API_KEY", "sk-secret-value")
        .output()
        .unwrap();
    let text = exits(&o, 0);
    assert!(text.contains("profile: workspace"), "{text}");
    assert!(text.contains("policy: workspace-write"), "{text}");
    assert!(text.contains(&format!("  {}", repo.display())), "{text}");
    assert!(
        text.contains(&format!(
            "  {}",
            env.root.join("home").join("extra").display()
        )),
        "{text}"
    );
    assert!(text.contains("network: proxy"), "{text}");
    assert!(text.contains("allowed domains: example.com"), "{text}");
    let masked = text
        .lines()
        .find(|line| line.starts_with("masked variables:"))
        .unwrap();
    assert!(masked.contains("OPENAI_API_KEY"), "{text}");
    assert!(!masked.contains("PATH"), "{text}");
    assert!(!text.contains("sk-secret-value"), "{text}");
}

/// `sandbox` → Named sandbox profiles: `--profile` resolves an `extends` chain, takes the
/// root's policy and unions every `deny_read`, anchored at the Location and home.
#[test]
fn sandbox_explain_profile_resolves_extends_and_unions_deny_read() {
    let env = Env::new();
    let repo = env.repo("{}");
    global_config(
        &env,
        serde_json::json!({"sandbox": {"profiles": {
            "base": {"extends": "read-only", "deny_read": ["~/.aws/**"], "network": {"mode": "off"}},
            "ci": {"extends": "base", "deny_read": ["**/*.pem"], "env": {"mode": "none", "set": {"CI": "1"}}}
        }}}),
    );
    let o = env
        .command(&["sandbox", "explain", "--profile", "ci", "--format", "json"])
        .current_dir(&repo)
        .output()
        .unwrap();
    exits(&o, 0);
    let v = json(&o);
    assert_eq!(v["profile"], "ci");
    assert_eq!(v["policy"], "read-only");
    assert_eq!(v["network"], "off");
    assert_eq!(v["env_mode"], "none");
    assert_eq!(v["env_set"]["CI"], "1");
    let deny: Vec<&str> = v["deny_read"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(Value::as_str)
        .collect();
    assert_eq!(deny.len(), 2, "{v}");
    assert!(
        deny[0].ends_with("/home/.aws/**") && deny[1].ends_with("/repo/**/*.pem"),
        "{v}"
    );
    #[cfg(unix)]
    assert_eq!(
        deny,
        [
            format!("{}/.aws/**", env.root.join("home").display()),
            format!("{}/**/*.pem", repo.display())
        ]
    );
    let unknown = env
        .command(&["sandbox", "explain", "--profile", "missing"])
        .current_dir(&repo)
        .output()
        .unwrap();
    assert!(
        stdout(&unknown).contains("error: unknown sandbox profile missing"),
        "{}",
        stdout(&unknown)
    );
}

/// Regression (`sandbox` → Named sandbox profiles, org `min_profile`): a trusted
/// repository that redefines the profile the organization names as its minimum cannot
/// lower it; the floor fails closed to `read-only`.
#[test]
fn repository_cannot_redefine_the_managed_minimum_profile() {
    let env = Env::new();
    let repo = env.repo(
        &serde_json::json!({"sandbox": {"profiles": {"corp": {"policy": "full-access", "deny_read": []}}}})
            .to_string(),
    );
    global_config(
        &env,
        serde_json::json!({"sandbox": {"policy": "full-access", "profiles": {
            "corp": {"extends": "workspace", "deny_read": ["**/*.pem"]}
        }}}),
    );
    let managed = env.root.join("managed");
    std::fs::create_dir_all(&managed).unwrap();
    std::fs::write(
        managed.join("cyber-policy.json"),
        serde_json::json!({"policy": {"sandbox": {"min_profile": "corp"}}}).to_string(),
    )
    .unwrap();
    let cwd = repo.to_str().unwrap();
    let report = json(&env.cyber(&["trust", "inspect", "--cwd", cwd, "--format", "json"]));
    let digest = report["digest"].as_str().unwrap().to_string();
    assert!(
        env.cyber(&["trust", "approve", "--cwd", cwd, "--digest", &digest])
            .status
            .success()
    );
    let o = env
        .command(&["sandbox", "explain", "--format", "json"])
        .env(cyber_core::config::MANAGED_POLICY_DIR_ENV, &managed)
        .current_dir(&repo)
        .output()
        .unwrap();
    exits(&o, 0);
    let v = json(&o);
    assert_eq!(v["policy"], "read-only", "{v}");
}

/// `sandbox` → Sandbox CLI: `cyber sandbox run` enforces the effective policy on one
/// command, masks credentials and passes its exit code through.
#[cfg(any(target_os = "macos", target_os = "linux"))]
#[test]
fn sandbox_run_executes_under_effective_policy() {
    let env = Env::new();
    let repo = env.repo("{}");
    global_config(&env, serde_json::json!({"sandbox": {"network": "off"}}));
    let outside = format!("/usr/cyber-sandbox-run-{}", std::process::id());
    let script = format!(
        "echo inside > inside.txt; echo ${{OPENAI_API_KEY:-masked}}; echo outside > {outside}; exit 7"
    );
    let run = |extra: &[&str]| {
        let mut args = vec!["sandbox", "run"];
        args.extend_from_slice(extra);
        args.extend(["--", "/bin/sh", "-c", script.as_str()]);
        env.command(&args)
            .current_dir(&repo)
            .env("OPENAI_API_KEY", "sk-secret-value")
            .output()
            .unwrap()
    };
    let o = run(&[]);
    let text = exits(&o, 7);
    assert!(
        text.contains("masked") && !text.contains("sk-secret-value"),
        "{text}"
    );
    assert_eq!(
        std::fs::read_to_string(repo.join("inside.txt")).unwrap(),
        "inside\n"
    );
    assert!(!Path::new(&outside).exists());
    std::fs::remove_file(repo.join("inside.txt")).unwrap();
    let read_only = run(&["--policy", "read-only"]);
    exits(&read_only, 7);
    assert!(
        !repo.join("inside.txt").exists(),
        "read-only refuses Location writes"
    );
}

/// Regression: the credential files in the data directory (the file-backed plugin secret
/// store, the MCP OAuth file and the peer token file) were hidden only when an App was built, so `cyber
/// sandbox run` could read them.
#[cfg(any(target_os = "macos", target_os = "linux"))]
#[test]
fn sandbox_run_cannot_read_the_credential_files_of_the_data_directory() {
    let env = Env::new();
    let repo = env.repo("{}");
    let data = env.root.join("cyber-home/data");
    std::fs::create_dir_all(&data).unwrap();
    for name in ["plugin-secrets.json", "mcp-auth.json", "peers-secrets.json"] {
        let file = data.join(name);
        std::fs::write(&file, "{\"token\": \"credential-canary\"}").unwrap();
        let o = env
            .command(&[
                "sandbox",
                "run",
                "--",
                "/bin/cat",
                &file.display().to_string(),
            ])
            .current_dir(&repo)
            .output()
            .unwrap();
        assert!(!o.status.success(), "{name}: {}", stdout(&o));
        assert!(!stdout(&o).contains("credential-canary"), "{name}");
    }
}

/// A checkout whose database holds the saved approval `bash: npm test *`.
#[cfg(unix)]
fn saved_approval_env() -> (Env, PathBuf, PathBuf) {
    let env = Env::new();
    let repo = env.repo("{}");
    let db = env.root.join("approvals.db");
    let store = cyber_store::Store::open(cyber_store::StoreOptions::new(
        cyber_core::paths::DatabaseLocation::File(db.clone()),
        cyber_server::runtime::Runtime::registry(),
    ))
    .unwrap();
    cyber_tools::permissions::saved::save(&store, &repo, "bash", &["npm test *".into()], "ses_x")
        .unwrap();
    (env, repo, db)
}

/// `permissions-modes` → Rule dry run: each command of a shell line names its effect and
/// the layer that decided it, under the Session Mode. Unix only: the saved-approval
/// fixture keys the checkout by its canonical path, which differs from the lookup key on
/// Windows (`\\?\` prefix).
#[cfg(unix)]
#[test]
fn permissions_test_lists_saved_approval_and_default_rule_per_command() {
    let (env, repo, db) = saved_approval_env();
    let o = env
        .command(&["permissions", "test", "--", "npm test && git push"])
        .current_dir(&repo)
        .env("CYBER_DB", &db)
        .output()
        .unwrap();
    let text = exits(&o, 0);
    assert_eq!(
        stdout(&o),
        "npm test \u{2192} allow (saved approval, project)\ngit push \u{2192} ask (default rules)\n",
        "{text}"
    );
    let dont_ask = env
        .command(&[
            "permissions",
            "test",
            "--mode",
            "dont-ask",
            "--",
            "git push",
        ])
        .current_dir(&repo)
        .env("CYBER_DB", &db)
        .output()
        .unwrap();
    exits(&dont_ask, 0);
    assert_eq!(
        stdout(&dont_ask),
        "git push \u{2192} deny (dont-ask mode)\n"
    );
    let tool = env
        .command(&[
            "permissions",
            "test",
            "--tool",
            "edit",
            "--resource",
            ".cyber/hooks.jsonc",
            "--mode",
            "bypass",
        ])
        .current_dir(&repo)
        .env("CYBER_DB", &db)
        .output()
        .unwrap();
    exits(&tool, 0);
    assert_eq!(
        stdout(&tool),
        ".cyber/hooks.jsonc \u{2192} ask (protected path)\n"
    );
}

/// Regression (`permissions-modes` → Rule dry run): a tool is evaluated under the action
/// a Session authorizes it with, so an `edit` rule decides `--tool write` and a `bash`
/// rule decides `--tool monitor`.
#[test]
fn permissions_test_evaluates_tools_under_their_action() {
    let env = Env::new();
    let repo = env.repo("{}");
    global_config(
        &env,
        serde_json::json!({"permissions": {"edit": {"src/**": "deny"}, "bash": {"git push*": "deny"}}}),
    );
    let test = |tool: &str, resource: &str| {
        let o = env
            .command(&[
                "permissions",
                "test",
                "--tool",
                tool,
                "--resource",
                resource,
            ])
            .current_dir(&repo)
            .output()
            .unwrap();
        exits(&o, 0);
        stdout(&o)
    };
    for tool in ["write", "apply_patch", "edit"] {
        let out = test(tool, "src/a.rs");
        assert!(out.starts_with("src/a.rs \u{2192} deny"), "{tool}: {out}");
    }
    let out = test("monitor", "git push --force");
    assert!(out.starts_with("git push --force \u{2192} deny"), "{out}");
}

/// `permissions-modes` → Rule dry run: `GET /api/v1/permissions/test` on the shared server
/// returns the evaluation the CLI prints, without creating a Session.
#[cfg(unix)]
#[test]
fn permissions_test_route_matches_cli_evaluation_without_session() {
    let (env, repo, db) = saved_approval_env();
    let catalog = env.root.join("models.json");
    std::fs::write(&catalog, "{}").unwrap();
    let cli = env
        .command(&[
            "permissions",
            "test",
            "--format",
            "json",
            "--",
            "npm test && git push",
        ])
        .current_dir(&repo)
        .env("CYBER_DB", &db)
        .output()
        .unwrap();
    exits(&cli, 0);
    let service = StartedService(&env, catalog);
    let api = service
        .0
        .command(&[
            "api",
            "GET",
            "/permissions/test?command=npm%20test%20%26%26%20git%20push",
        ])
        .current_dir(&repo)
        .env("CYBER_DB", &db)
        .env("CYBER_MODELS_PATH", &service.1)
        .output()
        .unwrap();
    exits(&api, 0);
    let routed: Value = serde_json::from_slice(&api.stdout).unwrap();
    assert_eq!(routed["data"], json(&cli));
    assert_eq!(
        routed["data"]["results"][0]["reason"],
        "saved approval, project"
    );
    let sessions = service
        .0
        .command(&["api", "GET", "/sessions"])
        .current_dir(&repo)
        .env("CYBER_DB", &db)
        .env("CYBER_MODELS_PATH", &service.1)
        .output()
        .unwrap();
    exits(&sessions, 0);
    let listed: Value = serde_json::from_slice(&sessions.stdout).unwrap();
    assert_eq!(listed["data"]["data"], serde_json::json!([]), "{listed}");
}

/// `sandbox` → Sandbox CLI: `cyber sandbox test` probes reads, writes inside and outside
/// the roots, a credential directory and the network, and every probe matches the policy.
#[cfg(any(target_os = "macos", target_os = "linux"))]
#[test]
fn sandbox_test_probes_match_the_effective_policy() {
    let env = Env::new();
    let repo = env.repo("{}");
    std::fs::create_dir_all(env.root.join("home/.ssh")).unwrap();
    // Keep home out of the per-user temporary root macOS commands may write.
    let tmpdir = env.root.join("tmpdir");
    std::fs::create_dir_all(&tmpdir).unwrap();
    let o = env
        .command(&["sandbox", "test", "--format", "json"])
        .current_dir(&repo)
        .env("TMPDIR", &tmpdir)
        .output()
        .unwrap();
    exits(&o, 0);
    let probes = json(&o);
    let summary: Vec<(String, String)> = probes
        .as_array()
        .unwrap()
        .iter()
        .map(|p| {
            (
                p["operation"].as_str().unwrap().to_string(),
                p["expected"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    let expected: Vec<(String, String)> = [
        ("read", "allowed"),
        ("write", "allowed"),
        ("write", "denied"),
        ("read", "denied"),
        ("network", "denied"),
    ]
    .iter()
    .map(|(a, b)| (a.to_string(), b.to_string()))
    .collect();
    assert_eq!(summary, expected, "{probes}");
    assert!(
        probes
            .as_array()
            .unwrap()
            .iter()
            .all(|p| p["passed"] == true),
        "{probes}"
    );
    let leftovers: Vec<_> = std::fs::read_dir(&repo)
        .unwrap()
        .chain(std::fs::read_dir(env.root.join("home")).unwrap())
        .filter_map(Result::ok)
        .filter(|e| {
            e.file_name()
                .to_string_lossy()
                .starts_with(".cyber-sandbox-probe")
        })
        .collect();
    assert!(leftovers.is_empty(), "probe files are removed");
}

/// `--pure` (and `CYBER_PURE=1`) refuses the commands that can only start Turns on the
/// shared server, whose configuration still carries plugins, hooks and MCP servers,
/// before any server starts.
#[test]
fn pure_refuses_commands_that_run_turns_on_the_shared_server() {
    let env = Env::new();
    for args in [
        &["--pure", "api", "GET", "/sessions"][..],
        &["--pure", "pr", "3"],
        &["--pure", "messages", "list"],
        &["--pure", "mcp", "serve"],
        &["--pure", "workflows", "run", "x"],
        &["--pure", "workflows", "resume", "wr_1"],
        &["--pure", "review", "--attach", "http://127.0.0.1:9"],
    ] {
        let o = env.cyber(args);
        assert_eq!(o.status.code(), Some(2), "{args:?}: {}", stderr(&o));
        assert!(
            stderr(&o).contains("--pure cannot"),
            "{args:?}: {}",
            stderr(&o)
        );
    }
    let o = env
        .command(&["api", "GET", "/sessions"])
        .env("CYBER_PURE", "1")
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(2), "{}", stderr(&o));
    assert!(
        stderr(&o).contains("--pure cannot run `cyber api`"),
        "{}",
        stderr(&o)
    );
}

/// `installation-upgrade` → Container and CI usage, "CI defaults": with `CI=true`,
/// `cyber` with no subcommand refuses the TUI with exit 2 before touching a terminal.
#[test]
fn ci_true_refuses_the_tui() {
    let env = Env::new();
    let output = env.command(&[]).env("CI", "true").output().unwrap();
    assert_eq!(output.status.code(), Some(2), "{output:?}");
    assert!(
        stderr(&output).contains("interactive TUI unavailable in CI; use \"cyber exec\""),
        "{}",
        stderr(&output)
    );
    assert!(stdout(&output).is_empty(), "{}", stdout(&output));
}

/// Regression: `cyber attach` opens the TUI too, so with `CI=true` it is refused the
/// same way, before any peer is looked up or contacted.
#[test]
fn ci_true_refuses_attach() {
    let env = Env::new();
    let output = env
        .command(&["attach", "s1@laptop"])
        .env("CI", "true")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2), "{output:?}");
    assert!(
        stderr(&output).contains("interactive TUI unavailable in CI; use \"cyber exec\""),
        "{}",
        stderr(&output)
    );
}

/// With `CI=true`, `cyber exec` keeps its default `--format text`: the scripted local
/// model's answer is printed as plain text, not JSON.
#[test]
fn ci_true_exec_defaults_to_text() {
    let model = scripted_model::ScriptedModel::start(|_| json!({"content": "ci answer"}));
    let env = Env::new();
    let config = env.root.join("cyber-home/config");
    std::fs::create_dir_all(&config).unwrap();
    std::fs::write(config.join("cyber.jsonc"), model.config().to_string()).unwrap();
    let catalog = env.root.join("models.json");
    std::fs::write(&catalog, "{}").unwrap();
    let output = env
        .command(&["exec", "--embedded", "say hi"])
        .env("CI", "true")
        .env("CYBER_MODELS_PATH", &catalog)
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(stdout(&output).trim(), "ci answer");
    assert!(serde_json::from_slice::<Value>(&output.stdout).is_err());
}
