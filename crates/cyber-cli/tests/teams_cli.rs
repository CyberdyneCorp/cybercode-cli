//! `cyber features` and `cyber teams` through the real binary (`configuration` → Feature
//! flags registry, `agent-teams` → Feature flag, Team definitions and Team view); `cyber
//! teams list|show|stop|resume` against the background server it starts, with a loopback
//! scripted model.

use std::path::PathBuf;
use std::process::{Command, Output};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

#[path = "../../cyber-app/tests/support/scripted_model.rs"]
mod scripted_model;

struct Env {
    _dir: tempfile::TempDir,
    root: PathBuf,
    repo: PathBuf,
}

impl Env {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let repo = root.join("repo");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        Self {
            _dir: dir,
            root,
            repo,
        }
    }

    fn cyber_with(&self, args: &[&str], env: &[(&str, &str)]) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_cyber"));
        command
            .args(args)
            .env_clear()
            .env("HOME", self.root.join("home"))
            .env("CYBER_HOME", self.root.join("cyber-home"))
            .env("CYBER_MANAGED_POLICY_DIR", self.root.join("managed"))
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

    fn user_config_dir(&self) -> PathBuf {
        self.root.join("cyber-home").join("config")
    }

    fn write(&self, path: PathBuf, text: &str) -> PathBuf {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, text).unwrap();
        path
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

/// Whitespace-collapsed lines, so column padding does not matter.
fn lines(o: &Output) -> Vec<String> {
    stdout(o)
        .lines()
        .map(|line| line.split_whitespace().collect::<Vec<_>>().join(" "))
        .collect()
}

fn teams(o: &Output) -> Value {
    json(o)
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["name"] == "teams")
        .cloned()
        .unwrap()
}

#[track_caller]
fn succeeded(out: &Output) {
    assert!(out.status.success(), "{}", stderr(out));
}

#[track_caller]
fn has_line(out: &Output, line: &str) {
    assert!(
        lines(out).iter().any(|l| l == line),
        "{line}: {}",
        stdout(out)
    );
}

#[track_caller]
fn first_line_starts(out: &Output, start: &str) {
    let first = lines(out).first().cloned().unwrap_or_default();
    assert!(first.starts_with(start), "{start}: {}", stdout(out));
}

#[track_caller]
fn fails_with(out: &Output, code: i32, message: &str) {
    assert_eq!(out.status.code(), Some(code), "{}", stderr(out));
    assert!(stderr(out).contains(message), "{message}: {}", stderr(out));
}

/// In the table of `out` whose header starts with `header`, the row starting with `row`
/// has a cell starting with `cell` under `column`; a `$` cost cell (right-aligned) ends
/// where its header ends.
#[track_caller]
fn aligned(out: &Output, header: &str, row: &str, column: &str, cell: &str) {
    let text = stdout(out);
    let find = |start: &str| {
        text.lines()
            .find(|line| line.starts_with(start))
            .unwrap_or_else(|| panic!("no line starting {start}: {text}"))
    };
    let (head, line) = (find(header), find(row));
    let at = head.find(column).unwrap();
    let ok = if cell == "$" {
        let end = at + column.len();
        let start = line
            .get(..end)
            .and_then(|l| l.rfind(' '))
            .map_or(0, |s| s + 1);
        line[start..end].starts_with('$') && line[end..].starts_with(' ')
    } else {
        line.get(at..).is_some_and(|rest| rest.starts_with(cell))
    };
    assert!(ok, "{column} out of line:\n{text}");
}

fn names(list: &Value, key: &str) -> Vec<String> {
    list[key]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["name"].as_str().unwrap().to_owned())
        .collect()
}

/// Scenarios "List features" and "Unknown feature rejected", plus every source of the
/// resolution order and `enable`/`disable` edits.
#[test]
fn features_list_shows_teams_off_and_unknown_feature_fails() {
    let env = Env::new();
    let out = env.cyber(&["features", "list"]);
    succeeded(&out);
    has_line(&out, "teams experimental off (default)");
    let listed = teams(&env.cyber(&["features", "list", "--json"]));
    assert_eq!(listed["maturity"], "experimental");
    assert_eq!(
        (&listed["default"], &listed["enabled"]),
        (&Value::Bool(false), &Value::Bool(false))
    );
    sources_rank_flag_env_and_documents(&env);
    policy_lock_wins(&env);
    unknown_features_fail(&env);
    deprecated_alias_warns(&env);
}

/// `enable` writes the user document; CYBER_FEATURES outranks it, --features outranks
/// both; `disable --scope project` writes the project document, which outranks the user one.
fn sources_rank_flag_env_and_documents(env: &Env) {
    succeeded(&env.cyber(&["features", "enable", "teams"]));
    let user = env.user_config_dir().join("cyber.jsonc");
    let listed = teams(&env.cyber(&["features", "list", "--json"]));
    assert_eq!(listed["enabled"], true);
    assert_eq!(listed["source"], format!("global:{}", user.display()));
    let listed = teams(&env.cyber_with(
        &["features", "list", "--format", "json"],
        &[("CYBER_FEATURES", "-teams")],
    ));
    assert_eq!(
        (&listed["enabled"], &listed["source"]),
        (&Value::Bool(false), &Value::from("env:CYBER_FEATURES"))
    );
    let out = env.cyber_with(
        &["--features", "teams", "features", "list"],
        &[("CYBER_FEATURES", "-teams")],
    );
    has_line(&out, "teams experimental on (cli:--features; default off)");
    succeeded(&env.cyber(&["features", "disable", "teams", "--scope", "project"]));
    let listed = teams(&env.cyber(&["features", "list", "--json"]));
    assert_eq!(listed["enabled"], false);
    let source = listed["source"].as_str().unwrap();
    assert!(source.starts_with("project:"), "{listed}");
}

/// Organization policy locks the value over every other source.
fn policy_lock_wins(env: &Env) {
    let policy = env.write(
        env.root.join("managed/cyber-policy.json"),
        r#"{"policy": {"features": {"lock": {"teams": true}}}}"#,
    );
    let out = env.cyber(&["--features", "-teams", "features", "list"]);
    first_line_starts(&out, "teams experimental on (managed:");
    assert!(
        stdout(&out).contains("locked by policy"),
        "{}",
        stdout(&out)
    );
    std::fs::remove_file(policy).unwrap();
}

/// Scenario "Unknown feature rejected", from config, the flag and the environment.
fn unknown_features_fail(env: &Env) {
    let config = env.write(
        env.repo.join("cyber.jsonc"),
        r#"{"features": {"telepathy": true}}"#,
    );
    let out = env.cyber(&["features", "list"]);
    fails_with(&out, 2, "ConfigInvalidError: unknown feature \"telepathy\"");
    std::fs::remove_file(config).unwrap();
    let out = env.cyber(&["--features", "telepathy", "features", "list"]);
    fails_with(&out, 2, "--features: unknown feature \"telepathy\"");
    let out = env.cyber_with(&["features", "list"], &[("CYBER_FEATURES", "telepathy")]);
    fails_with(&out, 2, "CYBER_FEATURES: unknown feature \"telepathy\"");
    let out = env.cyber(&["features", "enable", "telepathy"]);
    fails_with(&out, 2, "unknown feature \"telepathy\"");
}

/// The deprecated alias still works and warns, below `features.<name>` of any layer.
fn deprecated_alias_warns(env: &Env) {
    std::fs::remove_file(env.repo.join(".cyber").join("cyber.jsonc")).unwrap();
    env.write(
        env.repo.join("cyber.jsonc"),
        r#"{"experimental": {"teams": false}}"#,
    );
    let out = env.cyber(&["features", "list"]);
    let warning = "warning: experimental.teams is deprecated; use features.teams";
    assert!(stderr(&out).contains(warning), "{}", stderr(&out));
    first_line_starts(&out, "teams experimental on (global:");
    succeeded(&env.cyber(&["features", "disable", "teams"]));
    env.write(
        env.repo.join("cyber.jsonc"),
        r#"{"experimental": {"teams": true}}"#,
    );
    first_line_starts(
        &env.cyber(&["features", "list"]),
        "teams experimental off (global:",
    );
}

const FEATURE: &str = "---\nname: feature\ndescription: Build a feature end to end\nteammates:\n  - name: api\n    agent: build\n    owns: [\"src/api/**\"]\n  - name: ui\n    agent: build\n  - name: tests\n    agent: general\nbudget:\n  max_cost_usd: 20\n---\nSplit the work into api, ui and tests tasks.\n";

struct Written {
    user: PathBuf,
    remote: PathBuf,
    reserved: PathBuf,
}

/// User `feature` and `review`, project `feature`, a `runner` one and a reserved name.
fn write_definitions(env: &Env) -> Written {
    let user = env.user_config_dir().join("teams");
    env.write(
        user.join("feature.md"),
        &FEATURE.replace("Build a feature end to end", "user version"),
    );
    env.write(
        user.join("review.md"),
        "---\ndescription: Review\nteammates: [{name: reviewer, agent: reviewer}]\n---\nReview.\n",
    );
    let project = env.repo.join(".cyber").join("teams");
    env.write(project.join("feature.md"), FEATURE);
    let remote = env.write(
        project.join("remote.md"),
        "---\nname: remote\nteammates:\n  - name: api\n    agent: build\n    runner: desk\n---\n",
    );
    let reserved = env.write(
        project.join("reserved.md"),
        "---\nname: reserved\nteammates:\n  - name: lead\n    agent: build\n---\n",
    );
    Written {
        user,
        remote,
        reserved,
    }
}

/// Scenario "Start a defined team" (definition half): project definitions need trust and
/// then win over user ones; invalid definitions name their field and block nothing.
#[test]
fn team_definitions_load_with_project_precedence_and_name_the_invalid_field() {
    let env = Env::new();
    let out = env.cyber(&["teams", "definitions"]);
    fails_with(
        &out,
        2,
        "Agent teams are disabled. Enable them with \"cyber features enable teams\".",
    );
    let written = write_definitions(&env);
    untrusted_project_definitions_are_held(&env);
    approve_checkout(&env);
    let out = env.cyber_with(
        &["teams", "definitions", "--format", "json"],
        &[("CYBER_FEATURES", "teams")],
    );
    succeeded(&out);
    let listed = json(&out);
    project_definition_wins(&listed, &written);
    invalid_definitions_name_the_field(&listed, &written);
    let out = env.cyber(&["--features", "teams", "teams", "definitions"]);
    has_line(
        &out,
        "feature project api,ui,tests Build a feature end to end",
    );
    has_line(&out, "review user reviewer Review");
    has_line(
        &out,
        &format!(
            "{}: teammates[0].runner: Distributed teammates are not available yet (runner)",
            written.remote.display()
        ),
    );
}

/// Untrusted: only the user definitions, and the project ones are reported held.
fn untrusted_project_definitions_are_held(env: &Env) {
    let out = env.cyber(&[
        "--features",
        "teams",
        "teams",
        "definitions",
        "--format",
        "json",
    ]);
    succeeded(&out);
    let listed = json(&out);
    assert_eq!(names(&listed, "definitions"), ["feature", "review"]);
    assert_eq!(listed["definitions"][0]["description"], "user version");
    let message = listed["issues"][0]["message"].as_str().unwrap();
    assert!(
        message.contains("held until this checkout is trusted"),
        "{message}"
    );
}

/// The definitions are in the trust digest; approve them as `cyber trust` shows them.
fn approve_checkout(env: &Env) {
    let inspect = json(&env.cyber(&["trust", "inspect", "--format", "json"]));
    let listed = inspect["definitions"].as_array().unwrap();
    assert!(
        listed
            .iter()
            .any(|d| d.as_str().unwrap().ends_with("feature.md")),
        "{inspect}"
    );
    let digest = inspect["digest"].as_str().unwrap();
    succeeded(&env.cyber(&["trust", "approve", "--digest", digest]));
}

fn project_definition_wins(listed: &Value, written: &Written) {
    let all = listed["definitions"].as_array().unwrap();
    let feature = all.iter().find(|d| d["name"] == "feature").unwrap();
    assert_eq!(feature["scope"], "project");
    assert_eq!(feature["description"], "Build a feature end to end");
    assert_eq!(
        feature["instructions"],
        "Split the work into api, ui and tests tasks."
    );
    assert_eq!(feature["budget"]["max_cost_usd"], 20.0);
    assert_eq!(names(feature, "teammates"), ["api", "ui", "tests"]);
    assert_eq!(feature["teammates"][0]["owns"][0], "src/api/**");
    let shadowed = written.user.join("feature.md").display().to_string();
    assert_eq!(feature["shadows"][0], shadowed);
}

fn invalid_definitions_name_the_field(listed: &Value, written: &Written) {
    let issues = listed["issues"].as_array().unwrap();
    let message = |path: &PathBuf| {
        issues
            .iter()
            .find(|i| i["path"] == path.display().to_string())
            .map(|i| i["message"].as_str().unwrap().to_owned())
    };
    assert_eq!(
        message(&written.remote).as_deref(),
        Some("teammates[0].runner: Distributed teammates are not available yet (runner)")
    );
    assert_eq!(
        message(&written.reserved).as_deref(),
        Some("teammates[0].name: \"lead\" is reserved")
    );
    assert_eq!(issues.len(), 2, "{issues:?}");
}

/// The scripted model runs the tool call the newest user prompt asks for
/// (`TOOL<<name>>{json}`), answers a tool result with `done`, and anything else with `ok`.
fn prompted_call(body: &Value) -> Value {
    let messages = body["messages"].as_array().cloned().unwrap_or_default();
    let last = messages.last().cloned().unwrap_or(Value::Null);
    if last["role"] == "tool" {
        return json!({"content": "done"});
    }
    let text = match &last["content"] {
        Value::String(text) => text.clone(),
        Value::Array(parts) => parts
            .iter()
            .filter_map(|part| part["text"].as_str())
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    };
    let Some(start) = text.find("TOOL<<").map(|at| at + 6) else {
        return json!({"content": "ok"});
    };
    let Some(end) = text[start..].find(">>").map(|at| start + at) else {
        return json!({"content": "ok"});
    };
    let input = text[end + 2..].lines().next().unwrap_or("{}").trim();
    json!({"tool_calls": [{"index": 0, "id": format!("call_{:04}", messages.len()),
        "type": "function", "function": {"name": &text[start..end], "arguments": input}}]})
}

/// A checkout with `teams` on whose default model is the scripted one, priced; the
/// background server it starts is stopped on drop.
struct Served {
    env: Env,
    _model: scripted_model::ScriptedModel,
}

impl Served {
    fn new() -> Self {
        Self::with_teams(true)
    }

    /// A checkout whose configuration turns `teams` on or leaves it off.
    fn with_teams(teams: bool) -> Self {
        let model = scripted_model::ScriptedModel::start(prompted_call);
        let env = Env::new();
        let mut config = model.config();
        config["providers"]["test"]["models"] =
            json!({"main": {"cost": {"input": 10.0, "output": 20.0}}});
        if teams {
            config["features"] = json!({"teams": true});
        }
        env.write(
            env.user_config_dir().join("cyber.jsonc"),
            &config.to_string(),
        );
        env.write(env.root.join("models.json"), "{}");
        Self { env, _model: model }
    }

    fn cyber(&self, args: &[&str]) -> Output {
        let models = self.env.root.join("models.json");
        let models = models.display().to_string();
        self.env
            .cyber_with(args, &[("CYBER_MODELS_PATH", models.as_str())])
    }

    /// `cyber api METHOD path -d body`, its JSON.
    fn api(&self, method: &str, path: &str, body: &Value) -> Value {
        let out = self.cyber(&["api", method, path, "-d", &body.to_string()]);
        succeeded(&out);
        json(&out)
    }

    /// `cyber api GET path`, its JSON.
    fn get(&self, path: &str) -> Value {
        let out = self.cyber(&["api", "GET", path]);
        succeeded(&out);
        json(&out)
    }

    fn show(&self, team: &str) -> Value {
        let out = self.cyber(&["teams", "show", team, "--format", "json"]);
        succeeded(&out);
        json(&out)
    }

    /// The team view once `done` holds for it.
    fn show_when(&self, team: &str, what: &str, done: impl Fn(&Value) -> bool) -> Value {
        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            let view = self.show(team);
            if done(&view) {
                return view;
            }
            assert!(Instant::now() < deadline, "never {what}: {view:#}");
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

impl Drop for Served {
    fn drop(&mut self) {
        let _ = self.cyber(&["service", "stop"]);
    }
}

fn member<'a>(view: &'a Value, name: &str) -> &'a Value {
    view["members"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["name"] == name)
        .unwrap_or_else(|| panic!("no member {name}: {view:#}"))
}

/// Every teammate idle with a recorded cost.
fn settled(view: &Value) -> bool {
    view["members"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|m| m["role"] == "teammate")
        .all(|m| m["status"] == "idle" && m["cost_usd"].as_f64().unwrap_or_default() > 0.0)
}

/// Scenario "Team view" and task 7.2 through the real binary and the server it starts: a
/// team started with `POST /teams` (through `cyber api`) whose `api` teammate creates
/// and claims a task and whose `tests` teammate messages the lead; `cyber teams show`
/// lists each member with status, current task, cost and last message plus the board by
/// status (table and JSON); `cyber teams list` counts teammates and tasks; `stop` then
/// `resume` round-trips the team, and the claimed task's owner is told to continue it.
#[test]
fn teams_show_lists_members_tasks_cost_and_last_message() {
    let served = Served::new();
    let lead = served.api("POST", "/sessions", &json!({"title": "lead"}));
    let lead = lead["data"]["id"].as_str().unwrap().to_owned();
    let create = json!({"lead_session_id": lead, "teammates": [
        {"name": "api", "isolation": "shared",
            "prompt": r#"TOOL<<team_task_create>>{"title": "schema", "owner": "api"}"#},
        {"name": "tests", "isolation": "shared",
            "prompt": r#"TOOL<<send_message>>{"to": "lead", "text": "tests are green"}"#}]});
    let created = served.api("POST", "/teams", &create);
    let team = created["data"]["team"]["team"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(created["data"]["spawned"].as_array().unwrap().len(), 2);

    let view = served.show_when(&team, "settled with a task", |view| {
        settled(view) && view["tasks"].as_array().is_some_and(|t| t.len() == 1)
    });
    let task = view["tasks"][0]["id"].as_str().unwrap().to_owned();
    let api = member(&view, "api")["session_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let claim = format!(r#"TOOL<<team_task_update>>{{"id": "{task}", "status": "in_progress"}}"#);
    served.api(
        "POST",
        &format!("/sessions/{api}/prompt"),
        &json!({"parts": [{"type": "text", "text": claim}], "delivery": "queue"}),
    );
    let view = served.show_when(&team, "claimed", |view| {
        settled(view) && member(view, "api")["current_task"]["id"] == task.as_str()
    });
    shown_as_json(&view, &task);
    shown_as_table(&served, &team, &task);
    listed(&served, &team);
    stop_then_resume(&served, &team, &task);
}

fn shown_as_json(view: &Value, task: &str) {
    assert_eq!(view["team"]["name"], "adhoc");
    assert_eq!(view["team"]["status"], "active");
    let names: Vec<&str> = view["members"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["lead", "api", "tests"]);
    let api = member(view, "api");
    assert_eq!(api["current_task"]["title"], "schema");
    assert_eq!(view["tasks"][0]["id"], task);
    assert_eq!(view["tasks"][0]["status"], "in_progress");
    assert_eq!(view["tasks"][0]["owner"], "api");
    let tests = member(view, "tests");
    assert!(tests.get("current_task").is_none(), "{tests}");
    let message = &tests["last_message"];
    assert_eq!(
        (&message["direction"], &message["text"]),
        (&json!("out"), &json!("tests are green")),
        "{tests}"
    );
    assert_eq!(member(view, "lead")["last_message"]["direction"], "in");
    let spent = view["spent"]["cost_usd"].as_f64().unwrap();
    let shares: f64 = view["members"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["cost_usd"].as_f64().unwrap())
        .sum();
    assert!(spent > 0.0 && (spent - shares).abs() < 1e-9, "{view:#}");
}

fn shown_as_table(served: &Served, team: &str, task: &str) {
    let out = served.cyber(&["teams", "show", "adhoc"]);
    succeeded(&out);
    let all = lines(&out);
    assert!(
        all[0].starts_with(&format!("{team} adhoc active (lead ses_")),
        "{all:#?}"
    );
    assert!(all[1].starts_with("spent $0.0"), "{all:#?}");
    let row = |name: &str| {
        all.iter()
            .find(|line| line.starts_with(&format!("{name} ")))
            .unwrap_or_else(|| panic!("no {name} row: {all:#?}"))
            .clone()
    };
    let api = row("api");
    assert!(
        api.starts_with(&format!("api teammate idle {task} schema $0.0")),
        "{api}"
    );
    let tests = row("tests");
    assert!(
        tests.starts_with("tests teammate idle - $0.0")
            && tests.ends_with("to lead: tests are green"),
        "{tests}"
    );
    assert!(row("lead").ends_with(": tests are green"), "{all:#?}");
    // Regression: a task cell longer than its fixed column pushed COST out of line.
    aligned(&out, "MEMBER", "api ", "COST", "$");
    has_line(&out, "in_progress (1)");
    has_line(&out, &format!("{task} schema @api"));
}

fn listed(served: &Served, team: &str) {
    let out = served.cyber(&["teams", "list"]);
    succeeded(&out);
    let row = lines(&out)
        .into_iter()
        .find(|line| line.starts_with(team))
        .unwrap_or_else(|| panic!("{}", stdout(&out)));
    assert!(
        row.starts_with(&format!("{team} adhoc active 2 0/1 $0.0")),
        "{row}"
    );
    // Regression: team IDs are wider than the ID column was, which shifted every cell.
    aligned(&out, "ID", team, "NAME", "adhoc");
    aligned(&out, "ID", team, "LEAD", "ses_");
    let out = served.cyber(&["teams", "list", "--status", "stopped", "--format", "json"]);
    succeeded(&out);
    assert_eq!(json(&out), json!([]));
}

/// `stop` stops the idle team at once; `resume` makes it active again and the owner of
/// the `in_progress` task is told to continue it.
fn stop_then_resume(served: &Served, team: &str, task: &str) {
    let out = served.cyber(&["teams", "stop", team]);
    succeeded(&out);
    assert_eq!(lines(&out), [format!("{team} stopped (user)")]);
    let out = served.cyber(&["teams", "list", "--status", "stopped", "--format", "json"]);
    let stopped = json(&out);
    assert_eq!(stopped[0]["id"], team, "{stopped}");
    assert_eq!(stopped[0]["stop_reason"], "user");
    let view = served.show(team);
    assert!(
        view["members"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|m| m["role"] == "teammate")
            .all(|m| m["status"] == "stopped"),
        "{view:#}"
    );
    let out = served.cyber(&["teams", "resume", "adhoc", "--format", "json"]);
    succeeded(&out);
    let resumed = json(&out);
    assert_eq!(
        (&resumed["id"], &resumed["status"]),
        (&json!(team), &json!("active"))
    );
    let continued = format!("The team resumed. Continue {task}: schema");
    let api = member(&served.show(team), "api")["session_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let history = format!("/sessions/{api}/history?limit=500");
    let deadline = Instant::now() + Duration::from_secs(60);
    while !served.get(&history).to_string().contains(&continued) {
        assert!(
            Instant::now() < deadline,
            "the api teammate was never told to continue"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
    let view = served.show_when(team, "idle again", settled);
    assert_eq!(view["team"]["status"], "active");
    let out = served.cyber(&["teams", "resume", team]);
    fails_with(&out, 1, "only a stopped team resumes");
}

/// Regression: `--features` reaches only the CLI process, never the background server,
/// so a server-backed team command follows the server's configuration. With `teams`
/// off there, `cyber --features teams teams list` fails with the server's
/// `FeatureDisabledError` and says why, instead of passing a local check first; the
/// local `features list` still reports the flag.
#[test]
fn features_flag_does_not_turn_teams_on_in_the_background_server() {
    let served = Served::with_teams(false);
    let out = served.cyber(&["--features", "teams", "teams", "list"]);
    fails_with(&out, 2, "Agent teams are disabled");
    fails_with(
        &out,
        2,
        "--features applies to this process only; the background server reads its own configuration",
    );
    let out = served.cyber(&["teams", "list"]);
    fails_with(&out, 2, "Agent teams are disabled");
    assert!(
        !stderr(&out).contains("--features applies"),
        "{}",
        stderr(&out)
    );
    let out = served.cyber(&["--features", "teams", "features", "list"]);
    succeeded(&out);
    has_line(&out, "teams experimental on (cli:--features; default off)");
    // Definitions are read by this process, so the flag counts there.
    succeeded(&served.cyber(&["--features", "teams", "teams", "definitions"]));
}
