//! Agent teams on the production App (`agent-teams`, `configuration` → Feature flags
//! registry): the feature and team definition routes over authenticated TCP, reading
//! the Location's configuration on every request.

#[path = "support/scripted_model.rs"]
mod scripted_model;
#[path = "support/mode_app.rs"]
mod support;

use serde_json::{Value, json};
use support::{ModeApp, Setup};

fn write(path: std::path::PathBuf, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

/// `GET /features` reports `teams` off by default and follows config edits; team routes
/// answer `404 FeatureDisabledError` until it is on; untrusted project definitions are
/// held while user ones load; an unknown feature fails the request.
#[tokio::test]
async fn feature_and_team_definition_routes_follow_the_location_configuration() {
    let app = ModeApp::start(Setup::default()).await;
    let (status, body) = app.send("GET", "/features", None).await;
    assert_eq!(status, 200, "{body}");
    let teams = body["data"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["name"] == "teams")
        .unwrap()
        .clone();
    assert_eq!(
        (&teams["maturity"], &teams["enabled"], &teams["source"]),
        (&json!("experimental"), &json!(false), &json!("default"))
    );
    let (status, body) = app.send("GET", "/team-definitions", None).await;
    assert_eq!(status, 404, "{body}");
    assert_eq!(body["_tag"], "FeatureDisabledError");
    assert_eq!(
        body["message"],
        "Agent teams are disabled. Enable them with \"cyber features enable teams\"."
    );

    let project_config = app.repo.join("cyber.jsonc");
    write(project_config.clone(), r#"{"features": {"teams": true}}"#);
    write(
        app.root.join("config/teams/review.md"),
        "---\ndescription: Review a change\nteammates:\n  - name: security\n    agent: reviewer\n---\nReview it.\n",
    );
    write(
        app.repo.join(".cyber/teams/remote.md"),
        "---\nteammates:\n  - name: api\n    agent: build\n    runner: desk\n---\n",
    );
    let (_, body) = app.send("GET", "/features", None).await;
    let feature = body["data"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["name"] == "teams")
        .unwrap();
    assert_eq!(feature["enabled"], true, "{body}");
    assert_eq!(
        feature["source"],
        format!("project:{}", project_config.display())
    );
    let (status, body) = app.send("GET", "/team-definitions", None).await;
    assert_eq!(status, 200, "{body}");
    let definitions = &body["data"]["definitions"];
    assert_eq!(definitions.as_array().unwrap().len(), 1, "{body}");
    assert_eq!(definitions[0]["name"], "review");
    assert_eq!(definitions[0]["scope"], "user");
    assert_eq!(definitions[0]["teammates"][0]["agent"], "reviewer");
    assert_eq!(definitions[0]["instructions"], "Review it.");
    let issue = &body["data"]["issues"][0];
    assert!(
        issue["message"]
            .as_str()
            .unwrap()
            .contains("held until this checkout is trusted"),
        "{issue}"
    );

    write(
        project_config,
        r#"{"features": {"teams": true, "telepathy": true}}"#,
    );
    let (status, body) = app.send("GET", "/features", None).await;
    assert_eq!(status, 400, "{body}");
    assert!(
        body["message"]
            .as_str()
            .unwrap()
            .contains("unknown feature \"telepathy\""),
        "{body}"
    );
    let (status, _) = app.send("GET", "/team-definitions", None).await;
    assert_eq!(status, 400, "an invalid configuration fails closed");
}

fn git(repo: &std::path::Path, args: &[&str]) -> String {
    let output = std::process::Command::new("git")
        .current_dir(repo)
        .args(args)
        .output()
        .unwrap();
    assert!(output.status.success(), "git {args:?}: {output:?}");
    String::from_utf8(output.stdout).unwrap()
}

/// A git checkout with one commit.
fn init_repository(repo: &std::path::Path) {
    git(repo, &["init", "--quiet", "-b", "main"]);
    git(repo, &["config", "user.name", "Teams"]);
    git(repo, &["config", "user.email", "teams@example.invalid"]);
    std::fs::write(repo.join("README.md"), "team\n").unwrap();
    git(repo, &["add", "README.md"]);
    git(repo, &["commit", "--quiet", "-m", "initial"]);
}

fn tool_names(body: &Value) -> Vec<String> {
    body["tools"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|tool| tool["function"]["name"].as_str().map(str::to_owned))
        .collect()
}

/// The newest model request whose text contains `needle`.
fn request_with(app: &ModeApp, needle: &str) -> Value {
    app.model
        .requests()
        .into_iter()
        .rev()
        .find(|body| body.to_string().contains(needle))
        .unwrap_or_else(|| panic!("no request contains {needle:?}"))
}

/// The model requests with one prompt cache key, oldest first: a root Session's ID, or
/// `<root>:<agent>` for its subagents.
fn requests_of(app: &ModeApp, cache_key: &str) -> Vec<Value> {
    app.model
        .requests()
        .into_iter()
        .filter(|body| body["prompt_cache_key"] == cache_key)
        .collect()
}

fn spawn_call(teammates: Value) -> String {
    format!("TOOL<<team_spawn>>{}", json!({ "teammates": teammates }))
}

/// The output of the newest settled tool call of a Session.
async fn last_output(app: &ModeApp, id: &str) -> String {
    app.outputs(id).await.pop().unwrap_or_default()
}

/// `agent-teams` on the production App over authenticated TCP: with `teams` off no team
/// tool is offered and nothing is created ("Disabled by default"); once on, the lead's
/// `team_spawn` creates `api`, `ui` and `tests` as root teammate Sessions titled
/// `adhoc/<name>`, each in its own worktree `team-<ulid>-<name>` on
/// `cyber/team-<ulid>/<name>` ("Spawn teammates"); the spawn that would make a seventh
/// teammate fails with `Team size limit reached (6)` and creates nothing ("Teammate
/// limit"); teammates see the `core/teams` context but no `team_spawn`, cannot lead a
/// team, inherit the lead's deny rules, and with the lead in `plan` mode their edits are
/// denied and their Mode cannot rise above it ("Teammate approval prompt", ceiling half).
#[tokio::test]
async fn lead_spawns_three_teammates_in_worktrees_and_the_seventh_is_refused() {
    let app = ModeApp::start(Setup {
        config: json!({"permissions": {"bash": "allow"}}),
        ..Setup::default()
    })
    .await;
    init_repository(&app.repo);
    let (status, body) = app
        .send(
            "POST",
            "/sessions",
            Some(json!({"mode": "default", "rules": {"bash": {"git push*": "deny"}}})),
        )
        .await;
    assert_eq!(status, 201, "{body}");
    let lead = body["data"]["id"].as_str().unwrap().to_owned();
    let trio = json!([{"name": "api", "prompt": "hello api"}, {"name": "ui"}, {"name": "tests"}]);

    // Disabled by default: no team tool is offered, a call is refused, nothing is created.
    app.run(&lead, &spawn_call(trio.clone())).await;
    let offered = tool_names(&request_with(&app, "TOOL<<team_spawn>>"));
    assert!(offered.contains(&"send_message".to_owned()), "{offered:?}");
    assert!(
        !offered.iter().any(|name| name.starts_with("team_")),
        "{offered:?}"
    );
    assert_eq!(last_output(&app, &lead).await, "Unknown tool: team_spawn");
    assert!(app.app.runtime.team_membership(&lead).unwrap().is_none());
    let (_, body) = app.send("GET", "/team-definitions", None).await;
    assert_eq!(body["_tag"], "FeatureDisabledError", "{body}");

    // Spawn teammates: three editors, so each gets its own worktree.
    write(
        app.repo.join("cyber.jsonc"),
        r#"{"features": {"teams": true}}"#,
    );
    app.run(&lead, &spawn_call(trio)).await;
    let text = last_output(&app, &lead).await;
    let output: Value = serde_json::from_str(&text).unwrap_or_else(|_| panic!("{text}"));
    let spawned = output["spawned"].as_array().unwrap();
    assert_eq!(spawned.len(), 3, "{output}");
    let (team, me) = app.app.runtime.team_membership(&lead).unwrap().unwrap();
    assert_eq!(
        (team.name.as_str(), team.status.as_str()),
        ("adhoc", "active")
    );
    assert_eq!(
        (me.name.as_str(), me.role),
        ("lead", cyber_server::runtime::teams::Role::Lead)
    );
    let ulid = team.id.strip_prefix("team_").unwrap().to_ascii_lowercase();
    let branches = git(&app.repo, &["branch", "--list", "cyber/team-*"]);
    let mut ids = std::collections::BTreeMap::new();
    for (member, name) in spawned.iter().zip(["api", "ui", "tests"]) {
        assert_eq!(member["name"], name, "{member}");
        assert_eq!(member["agent"], "build");
        assert_eq!(member["isolation"], "worktree");
        let branch = format!("cyber/team-{ulid}/{name}");
        assert_eq!(member["branch"], branch.as_str());
        assert!(branches.contains(&branch), "{branches}");
        let id = member["session_id"].as_str().unwrap().to_owned();
        let info = app.app.runtime.state(&id).await.unwrap().info;
        assert_eq!(info.title, format!("adhoc/{name}"));
        assert!(info.parent_id.is_none(), "teammates are root Sessions");
        let tag = info.team.clone().unwrap();
        assert_eq!(
            (tag.team_id.as_str(), tag.name.as_str()),
            (team.id.as_str(), name)
        );
        let directory = std::path::Path::new(&info.directory);
        assert!(
            directory.is_dir() && directory != app.repo,
            "{}",
            info.directory
        );
        assert!(
            directory.ends_with(format!("team-{ulid}-{name}")),
            "{}",
            info.directory
        );
        ids.insert(name, id);
    }
    // Worktree teammates are listed in the lead's checkout too.
    let (_, listed) = app.send("GET", "/sessions", None).await;
    for id in ids.values() {
        assert!(listed.to_string().contains(id.as_str()), "{listed}");
    }
    let members = app.app.runtime.team_members(&team.id).unwrap();
    let names: Vec<&str> = members.iter().map(|m| m.name.as_str()).collect();
    assert_eq!(names, ["lead", "api", "ui", "tests"]);
    // The prompt is the teammate's first input; it sees the team, not team_spawn.
    app.app.runtime.wait_idle(&ids["api"]).await;
    let first = requests_of(&app, &ids["api"]).remove(0);
    let text = first.to_string();
    assert!(text.contains("hello api"), "{text}");
    assert!(
        text.contains("you are teammate api of team adhoc"),
        "{text}"
    );
    assert!(!tool_names(&first).contains(&"team_spawn".to_owned()));
    let led = requests_of(&app, &lead).pop().unwrap();
    assert!(led.to_string().contains("you lead team adhoc"));
    assert!(tool_names(&led).contains(&"team_spawn".to_owned()));

    // Teammate limit: three more fit, four do not; the refused spawn creates nothing.
    let shared = |names: &[&str]| {
        json!(
            names
                .iter()
                .map(|name| json!({"name": name, "isolation": "shared"}))
                .collect::<Vec<_>>()
        )
    };
    app.run(&lead, &spawn_call(shared(&["d1", "d2", "d3", "d4"])))
        .await;
    let refused = last_output(&app, &lead).await;
    assert!(refused.contains("Team size limit reached (6)"), "{refused}");
    assert_eq!(app.app.runtime.team_members(&team.id).unwrap().len(), 4);
    app.run(&lead, &spawn_call(shared(&["d1", "d2", "d3"])))
        .await;
    assert!(last_output(&app, &lead).await.contains("\"spawned\""));
    app.run(&lead, &spawn_call(shared(&["seventh"]))).await;
    let refused = last_output(&app, &lead).await;
    assert!(refused.contains("Team size limit reached (6)"), "{refused}");
    assert_eq!(app.app.runtime.team_members(&team.id).unwrap().len(), 7);

    // Teammates cannot lead a team: not offered, and refused by the runtime.
    let tests = ids["tests"].clone();
    app.run(&tests, &spawn_call(json!([{"name": "nested"}])))
        .await;
    assert_eq!(last_output(&app, &tests).await, "Unknown tool: team_spawn");
    let nested = app
        .app
        .runtime
        .team_spawn(
            &tests,
            vec![cyber_server::runtime::teams::TeamSpawn {
                name: "nested".into(),
                ..Default::default()
            }],
        )
        .await
        .unwrap_err();
    assert!(
        nested.to_string().contains("Teammates cannot lead a team"),
        "{nested}"
    );

    // The lead's deny rules hold for its teammates.
    app.run(&tests, "RUN<<echo teammate>>").await;
    assert!(last_output(&app, &tests).await.contains("teammate"));
    app.run(&tests, "RUN<<git push --dry-run>>").await;
    let denied = last_output(&app, &tests).await;
    assert!(denied.contains("denied by rule"), "{denied}");

    // The ceiling: with the lead in `plan` mode a teammate's edit is denied and its Mode
    // cannot rise above the lead's.
    let (status, body) = app
        .send(
            "POST",
            &format!("/sessions/{lead}/mode"),
            Some(json!({"mode": "plan"})),
        )
        .await;
    assert_eq!(status, 200, "{body}");
    let ui = ids["ui"].clone();
    app.run(
        &ui,
        r#"TOOL<<write>>{"path": "src/ui.rs", "content": "fn ui() {}\n"}"#,
    )
    .await;
    let denied = last_output(&app, &ui).await;
    assert!(denied.contains("Plan mode is read-only"), "{denied}");
    let worktree = app.app.runtime.state(&ui).await.unwrap().info.directory;
    assert!(!std::path::Path::new(&worktree).join("src/ui.rs").exists());
    let step = app
        .events(&ui)
        .into_iter()
        .rev()
        .find(|e| e.kind == "session.step.started.1")
        .unwrap();
    assert_eq!(step.data["mode"], "plan", "{}", step.data);
    let (status, body) = app
        .send(
            "POST",
            &format!("/sessions/{ui}/mode"),
            Some(json!({"mode": "bypass"})),
        )
        .await;
    assert_eq!(status, 400, "{body}");
    assert!(
        body["message"]
            .as_str()
            .unwrap()
            .contains("Mode limited by the team lead (plan)"),
        "{body}"
    );
    let (status, body) = app
        .send(
            "POST",
            &format!("/sessions/{ui}/mode"),
            Some(json!({"mode": "plan"})),
        )
        .await;
    assert_eq!(status, 200, "a Mode at the ceiling is accepted: {body}");
    for id in ids.values() {
        app.app.runtime.wait_idle(id).await;
    }
}

/// `agent-teams` → Team definitions on the production App: starting a team from a
/// definition records it under the definition's name, admits the body to the lead as a
/// `team_instructions` system message its next request carries, and spawns the defined
/// teammates; a definition naming an unknown agent fails at start and records nothing; a
/// lead cannot start a second team, and `teams.max_teammates` bounds its later spawns.
#[tokio::test]
async fn a_definition_starts_a_team_with_lead_instructions_and_unknown_agents_fail_at_start() {
    let app = ModeApp::start(Setup {
        config: json!({"features": {"teams": true}, "teams": {"max_teammates": 2}}),
        ..Setup::default()
    })
    .await;
    let (_, body) = app.create("default").await;
    let lead = body["data"]["id"].as_str().unwrap().to_owned();
    let parse =
        |text: &str| cyber_core::teams::parse(text, std::path::Path::new("feature.md"), 6).unwrap();
    let broken = parse(
        "---\nteammates:\n  - name: api\n    agent: build\n  - name: ghost\n    agent: no-such-agent\n---\nPlan it.\n",
    );
    let error = app
        .app
        .runtime
        .create_team(&lead, &broken)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("no-such-agent"), "{error}");
    assert!(app.app.runtime.team_membership(&lead).unwrap().is_none());

    let feature = parse(
        "---\nteammates:\n  - name: api\n    agent: build\n    isolation: shared\n  - name: scout\n    agent: explore\n---\nSplit the work by layer.\n",
    );
    let (team, spawned) = app.app.runtime.create_team(&lead, &feature).await.unwrap();
    assert_eq!(team.name, "feature");
    let shown: Vec<_> = spawned
        .iter()
        .map(|m| (m.name.as_str(), m.agent.as_str(), m.isolation))
        .collect();
    use cyber_core::teams::Isolation::Shared;
    // Not a git checkout and one editor: the explorer shares the Location too.
    assert_eq!(
        shown,
        [("api", "build", Shared), ("scout", "explore", Shared)]
    );
    let scout = app.app.runtime.state(&spawned[1].session_id).await.unwrap();
    assert_eq!(scout.info.title, "feature/scout");
    app.run(&lead, "hello").await;
    let request = requests_of(&app, &lead).pop().unwrap().to_string();
    assert!(
        request.contains("<team_instructions>\\nSplit the work by layer.\\n</team_instructions>"),
        "{request}"
    );
    let again = app
        .app
        .runtime
        .create_team(&lead, &feature)
        .await
        .unwrap_err();
    assert!(
        again.to_string().contains("already a member of team"),
        "{again}"
    );
    // `teams.max_teammates` bounds later spawns of the same team.
    let full = app
        .app
        .runtime
        .team_spawn(
            &lead,
            vec![cyber_server::runtime::teams::TeamSpawn {
                name: "third".into(),
                ..Default::default()
            }],
        )
        .await
        .unwrap_err();
    assert!(
        full.to_string().contains("Team size limit reached (2)"),
        "{full}"
    );
}

/// A local HTTP endpoint that records every JSON hook request and answers `{}`.
struct HookRecorder {
    url: String,
    seen: std::sync::Arc<std::sync::Mutex<Vec<Value>>>,
    done: std::sync::Arc<std::sync::atomic::AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl HookRecorder {
    fn start() -> Self {
        use std::io::{Read, Write};
        use std::sync::atomic::Ordering;
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        listener.set_nonblocking(true).unwrap();
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let done = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (record, stop) = (seen.clone(), done.clone());
        let thread = std::thread::spawn(move || {
            while !stop.load(Ordering::SeqCst) {
                let Ok((mut stream, _)) = listener.accept() else {
                    std::thread::sleep(std::time::Duration::from_millis(5));
                    continue;
                };
                stream.set_nonblocking(false).unwrap();
                let mut bytes = Vec::new();
                let mut buffer = [0; 8192];
                let body = loop {
                    if let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                        let head = String::from_utf8_lossy(&bytes[..end]).to_ascii_lowercase();
                        let length = head
                            .lines()
                            .find_map(|line| line.strip_prefix("content-length:"))
                            .map_or(0, |value| value.trim().parse::<usize>().unwrap());
                        if bytes.len() >= end + 4 + length {
                            break serde_json::from_slice::<Value>(
                                &bytes[end + 4..end + 4 + length],
                            )
                            .ok();
                        }
                    }
                    match stream.read(&mut buffer) {
                        Ok(0) | Err(_) => break None,
                        Ok(n) => bytes.extend_from_slice(&buffer[..n]),
                    }
                };
                if let Some(body) = body {
                    record.lock().unwrap().push(body);
                }
                let _ = write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{{}}"
                );
            }
        });
        Self {
            url,
            seen,
            done,
            thread: Some(thread),
        }
    }

    /// The recorded hook events of one kind.
    fn events(&self, kind: &str) -> Vec<Value> {
        self.seen
            .lock()
            .unwrap()
            .iter()
            .filter(|event| event["event"] == kind)
            .cloned()
            .collect()
    }
}

impl Drop for HookRecorder {
    fn drop(&mut self) {
        self.done.store(true, std::sync::atomic::Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn task_call(tool: &str, input: Value) -> String {
    format!("TOOL<<{tool}>>{input}")
}

/// The task of a board tool's JSON output.
fn task_output(text: &str) -> Value {
    let output: Value = serde_json::from_str(text).unwrap_or_else(|_| panic!("{text}"));
    output["task"].clone()
}

/// Admit a prompt without waiting for its Drain.
async fn prompt(app: &ModeApp, id: &str, text: &str) {
    let (status, body) = app
        .send(
            "POST",
            &format!("/sessions/{id}/prompt"),
            Some(json!({"parts": [{"type": "text", "text": text}], "delivery": "queue"})),
        )
        .await;
    assert_eq!(status, 202, "{body}");
}

/// The `team.task.updated.1` records of one task on the lead's stream.
fn task_records(app: &ModeApp, lead: &str, task_id: &str) -> Vec<Value> {
    app.events(lead)
        .into_iter()
        .filter(|e| e.kind == "team.task.updated.1" && e.data["task"]["id"] == task_id)
        .map(|e| e.data)
        .collect()
}

/// `agent-teams` → Shared task list, Task claims and Team events on the production App:
/// the lead plans a board with a dependency; a teammate starting the dependent task is
/// refused with `Task depends on unfinished task_…` and nothing changes ("Dependency
/// blocks start"); two teammates whose gated Turns are released together claim one task
/// and exactly one owns it while the other gets `Task already claimed by <name>`
/// ("Concurrent claim"); a teammate cannot cancel; finishing the task runs the
/// `TaskCompleted` hook matched by `team_task` with `{ task, team_id, member }` ("Hook on
/// task completion"), and the dependent task can then start.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn teammates_share_a_board_where_dependencies_block_and_claims_are_exclusive() {
    let recorder = HookRecorder::start();
    let hook = json!([{"matcher": "team_task", "hooks": [
        {"type": "http", "url": format!("{}/hook", recorder.url), "id": "board"}]}]);
    let app = ModeApp::start(Setup {
        config: json!({"features": {"teams": true},
            "hooks": {"TaskCreated": hook, "TaskCompleted": hook}}),
        ..Setup::default()
    })
    .await;
    let (_, body) = app.create("default").await;
    let lead = body["data"]["id"].as_str().unwrap().to_owned();
    app.run(
        &lead,
        &spawn_call(json!([{"name": "api", "isolation": "shared"},
            {"name": "tests", "isolation": "shared"}])),
    )
    .await;
    let (team, _) = app.app.runtime.team_membership(&lead).unwrap().unwrap();
    let members = app.app.runtime.team_members(&team.id).unwrap();
    let id_of = |name: &str| {
        members
            .iter()
            .find(|m| m.name == name)
            .unwrap()
            .session_id
            .clone()
    };
    let (api, tests) = (id_of("api"), id_of("tests"));

    // The lead plans the board: `endpoint` waits for `schema`.
    app.run(
        &lead,
        &task_call("team_task_create", json!({"title": "schema"})),
    )
    .await;
    let schema = task_output(&last_output(&app, &lead).await);
    let schema_id = schema["id"].as_str().unwrap().to_owned();
    assert_eq!(
        (&schema["status"], &schema["version"], &schema["created_by"]),
        (&json!("todo"), &json!(0), &json!("lead"))
    );
    app.run(
        &lead,
        &task_call(
            "team_task_create",
            json!({"title": "endpoint", "depends_on": [schema_id]}),
        ),
    )
    .await;
    let endpoint_id = task_output(&last_output(&app, &lead).await)["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let offered = tool_names(&request_with(&app, "TOOL<<team_task_create>>"));
    for tool in [
        "team_task_create",
        "team_task_list",
        "team_task_get",
        "team_task_update",
    ] {
        assert!(offered.contains(&tool.to_owned()), "{offered:?}");
    }

    // Dependency blocks start: nothing changes.
    let start = json!({"id": endpoint_id, "status": "in_progress"});
    app.run(&api, &task_call("team_task_update", start.clone()))
        .await;
    let refused = last_output(&app, &api).await;
    assert!(
        refused.contains(&format!(
            "TaskDependencyError: Task depends on unfinished {schema_id}"
        )),
        "{refused}"
    );
    assert!(tool_names(&request_with(&app, &endpoint_id)).contains(&"team_task_update".into()));
    assert_eq!(task_records(&app, &lead, &endpoint_id).len(), 1);

    // Concurrent claim: both Turns wait at their gate, then claim together.
    let claim = task_call(
        "team_task_update",
        json!({"id": schema_id, "status": "in_progress"}),
    );
    prompt(&app, &api, &format!("GATE<<claim-api>>{claim}")).await;
    prompt(&app, &tests, &format!("GATE<<claim-tests>>{claim}")).await;
    tokio::time::timeout(std::time::Duration::from_secs(30), async {
        while app.model.held().len() < 2 {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("both claims wait at their gate");
    app.model.open_gate();
    app.model.open_gate();
    app.app.runtime.wait_idle(&api).await;
    app.app.runtime.wait_idle(&tests).await;
    let outcomes = [
        ("api", last_output(&app, &api).await),
        ("tests", last_output(&app, &tests).await),
    ];
    let won: Vec<&str> = outcomes
        .iter()
        .filter(|(_, text)| text.starts_with('{'))
        .map(|(name, _)| *name)
        .collect();
    assert_eq!(won.len(), 1, "{outcomes:?}");
    let winner = won[0];
    let (loser, lost) = outcomes.iter().find(|(name, _)| *name != winner).unwrap();
    assert!(
        lost.contains(&format!(
            "TaskClaimedError: Task already claimed by {winner}"
        )),
        "{lost}"
    );
    let claimed = task_output(&outcomes.iter().find(|(n, _)| *n == winner).unwrap().1);
    assert_eq!(
        (&claimed["status"], &claimed["owner"], &claimed["version"]),
        (&json!("in_progress"), &json!(winner), &json!(1))
    );
    let records = task_records(&app, &lead, &schema_id);
    assert_eq!(records.len(), 2, "one create and one claim: {records:?}");
    assert_eq!(records[1]["by"], winner);
    let (winner_id, loser_id) = (id_of(winner), id_of(loser));

    // Only the lead cancels.
    app.run(
        &loser_id,
        &task_call(
            "team_task_update",
            json!({"id": endpoint_id, "status": "cancelled"}),
        ),
    )
    .await;
    let refused = last_output(&app, &loser_id).await;
    assert!(refused.contains("Only the lead can cancel"), "{refused}");

    // Hook on task completion, then the dependent task can start.
    app.run(
        &winner_id,
        &task_call(
            "team_task_update",
            json!({"id": schema_id, "status": "done", "expected_version": 1}),
        ),
    )
    .await;
    let done = task_output(&last_output(&app, &winner_id).await);
    assert_eq!(
        (&done["status"], &done["version"]),
        (&json!("done"), &json!(2))
    );
    let completed = recorder.events("TaskCompleted");
    assert_eq!(completed.len(), 1, "{completed:?}");
    let event = &completed[0];
    assert_eq!(event["task"]["id"], schema_id.as_str(), "{event}");
    assert_eq!(event["task"]["status"], "done");
    assert_eq!(event["team_id"], team.id.as_str());
    assert_eq!(event["member"], winner);
    assert_eq!(event["session_id"], winner_id.as_str());
    assert!(event.get("job").is_none(), "{event}");
    let created: Vec<Value> = recorder
        .events("TaskCreated")
        .iter()
        .map(|e| e["task"]["title"].clone())
        .collect();
    assert_eq!(created, [json!("schema"), json!("endpoint")]);
    app.run(&api, &task_call("team_task_update", start)).await;
    let started = task_output(&last_output(&app, &api).await);
    assert_eq!(
        (&started["status"], &started["owner"]),
        (&json!("in_progress"), &json!("api"))
    );
    app.run(
        &tests,
        &task_call("team_task_list", json!({"status": "in_progress"})),
    )
    .await;
    let listed: Value = serde_json::from_str(&last_output(&app, &tests).await).unwrap();
    let ids: Vec<&Value> = listed["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| &t["id"])
        .collect();
    assert_eq!(ids, [&json!(endpoint_id)]);
    // A cycle is refused when written.
    app.run(
        &lead,
        &task_call(
            "team_task_update",
            json!({"id": schema_id, "depends_on": [endpoint_id]}),
        ),
    )
    .await;
    let refused = last_output(&app, &lead).await;
    assert!(
        refused.contains(&format!(
            "Task dependency cycle: {schema_id} → {endpoint_id} → {schema_id}"
        )),
        "{refused}"
    );
}

/// `agent-teams` → Task claims: two members claim one task from two threads at the same
/// moment, 50 times; each time exactly one owns it and the other is refused with `Task
/// already claimed by <winner>`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn concurrent_claims_of_one_task_have_exactly_one_winner() {
    let app = ModeApp::start(Setup {
        config: json!({"features": {"teams": true}}),
        ..Setup::default()
    })
    .await;
    let (_, body) = app.create("default").await;
    let lead = body["data"]["id"].as_str().unwrap().to_owned();
    let runtime = app.app.runtime.clone();
    let spawn = |name: &str| cyber_server::runtime::teams::TeamSpawn {
        name: name.into(),
        isolation: Some(cyber_core::teams::Isolation::Shared),
        ..Default::default()
    };
    let spawned = runtime
        .team_spawn(&lead, vec![spawn("api"), spawn("tests")])
        .await
        .unwrap();
    for round in 0..50 {
        let task = runtime
            .team_task_create(
                &lead,
                cyber_server::runtime::teams::TaskCreate {
                    title: format!("task {round}"),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        let barrier = std::sync::Arc::new(tokio::sync::Barrier::new(2));
        let claims: Vec<_> = spawned
            .iter()
            .map(|member| {
                let (runtime, barrier) = (runtime.clone(), barrier.clone());
                let (session, id) = (member.session_id.clone(), task.id.clone());
                tokio::spawn(async move {
                    barrier.wait().await;
                    runtime
                        .team_task_update(
                            &session,
                            cyber_server::runtime::teams::TaskUpdate {
                                id,
                                status: Some(cyber_server::runtime::teams::TaskStatus::InProgress),
                                ..Default::default()
                            },
                        )
                        .await
                })
            })
            .collect();
        let mut results = Vec::new();
        for claim in claims {
            results.push(claim.await.unwrap());
        }
        let owners: Vec<String> = results
            .iter()
            .filter_map(|r| r.as_ref().ok())
            .map(|t| t.owner.clone().unwrap())
            .collect();
        assert_eq!(owners.len(), 1, "round {round}: {results:?}");
        let refused = results
            .iter()
            .find_map(|r| r.as_ref().err())
            .unwrap()
            .to_string();
        assert_eq!(
            refused,
            format!("TaskClaimedError: Task already claimed by {}", owners[0]),
            "round {round}"
        );
        let board = runtime.board(&task_team(&runtime, &lead)).unwrap();
        let stored = board.iter().find(|t| t.id == task.id).unwrap();
        assert_eq!(stored.owner.as_deref(), Some(owners[0].as_str()));
        assert_eq!(stored.version, 1);
    }
}

fn task_team(runtime: &cyber_server::runtime::Runtime, lead: &str) -> String {
    runtime.team_membership(lead).unwrap().unwrap().0.id
}

/// Every text a model request carries, joined.
fn request_text(body: &Value) -> String {
    let mut texts = Vec::new();
    for message in body["messages"].as_array().into_iter().flatten() {
        match &message["content"] {
            Value::String(text) => texts.push(text.clone()),
            Value::Array(parts) => texts.extend(
                parts
                    .iter()
                    .filter_map(|part| part["text"].as_str().map(str::to_owned)),
            ),
            _ => {}
        }
    }
    texts.join("\n")
}

/// Wait for a model request of `session_id` whose text contains `needle`.
async fn eventually_request(app: &ModeApp, session_id: &str, needle: &str) -> String {
    let found = tokio::time::timeout(std::time::Duration::from_secs(30), async {
        loop {
            if let Some(text) = requests_of(app, session_id)
                .iter()
                .map(request_text)
                .find(|text| text.contains(needle))
            {
                return text;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await;
    found.unwrap_or_else(|_| panic!("no request of {session_id} contains {needle:?}"))
}

/// `agent-teams` → Teammate messaging and Idle notifications on the production App:
/// `ui` sends `which API shape should I use?` to `lead`; the lead's next request carries
/// it from `ui` and the lead replies to `ui` by name ("Teammate asks lead"). `tests`
/// claims its task and goes idle (`tests is idle with task_… in progress: …`), then
/// completes its last task and the lead receives `tests is idle: all assigned tasks
/// done` ("Teammate finishes"), recorded as `team.member.idle.1` with the notice
/// `msgx_team_idle_…`; a `TeammateIdle` hook gets `{ team_id, member, session_id,
/// tasks_in_progress, last_status }`; member status follows `working` and `idle` via
/// `team.member.updated.1`; and `all` reaches every non-stopped teammate once.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_teammate_asks_the_lead_and_reports_idle_when_done() {
    let recorder = HookRecorder::start();
    let hook = json!([{"matcher": "tests", "hooks": [
        {"type": "http", "url": format!("{}/hook", recorder.url), "id": "idle"}]}]);
    let app = ModeApp::start(Setup {
        config: json!({"features": {"teams": true}, "hooks": {"TeammateIdle": hook}}),
        ..Setup::default()
    })
    .await;
    let (_, body) = app.create("default").await;
    let lead = body["data"]["id"].as_str().unwrap().to_owned();
    app.run(
        &lead,
        &spawn_call(json!([{"name": "api", "isolation": "shared"},
            {"name": "ui", "isolation": "shared"},
            {"name": "tests", "isolation": "shared"}])),
    )
    .await;
    let (team, _) = app.app.runtime.team_membership(&lead).unwrap().unwrap();
    let members = app.app.runtime.team_members(&team.id).unwrap();
    let id_of = |name: &str| {
        members
            .iter()
            .find(|m| m.name == name)
            .unwrap()
            .session_id
            .clone()
    };
    let (api, ui, tests) = (id_of("api"), id_of("ui"), id_of("tests"));

    // Teammate asks lead.
    let question = "which API shape should I use?";
    app.run(
        &ui,
        &format!(
            "TOOL<<send_message>>{}",
            json!({"to": "lead", "text": question})
        ),
    )
    .await;
    let sent: Value = serde_json::from_str(&last_output(&app, &ui).await).unwrap();
    assert_eq!(sent["outcome"], "delivered", "{sent}");
    assert_eq!(
        (&sent["to"]["address"], &sent["to"]["session_id"]),
        (&json!("lead"), &json!(lead))
    );
    let asked = eventually_request(&app, &lead, question).await;
    assert!(asked.contains("from=\"ui\""), "{asked}");
    assert!(asked.contains("send_message to \"ui\""), "{asked}");
    app.app.runtime.wait_idle(&lead).await;
    let answer = "use REST with JSON bodies";
    app.run(
        &lead,
        &format!(
            "TOOL<<send_message>>{}",
            json!({"to": "ui", "text": answer})
        ),
    )
    .await;
    let replied: Value = serde_json::from_str(&last_output(&app, &lead).await).unwrap();
    assert_eq!(replied["outcome"], "delivered", "{replied}");
    assert_eq!(replied["to"]["session_id"], ui.as_str());
    let heard = eventually_request(&app, &ui, answer).await;
    assert!(heard.contains("from=\"lead\""), "{heard}");
    app.app.runtime.wait_idle(&ui).await;

    // Teammate finishes: the claim leaves a task in progress, completing it is all done.
    app.run(
        &lead,
        &task_call(
            "team_task_create",
            json!({"title": "write tests", "owner": "tests"}),
        ),
    )
    .await;
    let task_id = task_output(&last_output(&app, &lead).await)["id"]
        .as_str()
        .unwrap()
        .to_owned();
    app.run(
        &tests,
        &task_call(
            "team_task_update",
            json!({"id": task_id, "status": "in_progress"}),
        ),
    )
    .await;
    eventually_request(
        &app,
        &lead,
        &format!("tests is idle with {task_id} in progress: done"),
    )
    .await;
    app.run(
        &tests,
        &task_call("team_task_update", json!({"id": task_id, "status": "done"})),
    )
    .await;
    let notice = eventually_request(&app, &lead, "tests is idle: all assigned tasks done").await;
    assert!(notice.contains("<team_idle_notice"), "{notice}");
    app.app.runtime.wait_idle(&lead).await;
    let idles: Vec<Value> = app
        .events(&lead)
        .into_iter()
        .filter(|e| e.kind == "team.member.idle.1" && e.data["name"] == "tests")
        .map(|e| e.data)
        .collect();
    assert_eq!(idles.len(), 2, "{idles:?}");
    assert_eq!(idles[0]["tasks_in_progress"], json!([task_id]));
    assert_eq!(idles[1]["tasks_in_progress"], json!([]));
    assert_eq!(idles[1]["last_status"], "done");
    let prefix = format!("msgx_team_idle_{}_tests_", team.id);
    let notices: Vec<Value> = app
        .events(&lead)
        .into_iter()
        .filter(|e| e.kind == "session.prompt.admitted.1")
        .filter(|e| {
            e.data["message_id"]
                .as_str()
                .is_some_and(|id| id.starts_with(&prefix))
        })
        .map(|e| e.data)
        .collect();
    assert_eq!(notices.len(), 2, "{notices:?}");
    assert!(notices.iter().all(|n| n["source"] == "team"), "{notices:?}");

    // Member status: `working` while a Drain ran, `idle` after it with the status line.
    let statuses: Vec<Value> = app
        .events(&lead)
        .into_iter()
        .filter(|e| e.kind == "team.member.updated.1" && e.data["name"] == "tests")
        .map(|e| e.data["status"].clone())
        .collect();
    assert_eq!(
        statuses,
        [
            json!("working"),
            json!("idle"),
            json!("working"),
            json!("idle")
        ]
    );
    let row = app
        .app
        .runtime
        .team_members(&team.id)
        .unwrap()
        .into_iter()
        .find(|m| m.name == "tests")
        .unwrap();
    assert_eq!(
        (row.status, row.last_status.as_deref()),
        (
            cyber_server::runtime::teams::MemberStatus::Idle,
            Some("done")
        )
    );

    // TeammateIdle hook, matched by the member name, with its payload.
    let hooked = tokio::time::timeout(std::time::Duration::from_secs(30), async {
        loop {
            let events = recorder.events("TeammateIdle");
            if events.len() >= 2 {
                return events;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("TeammateIdle ran for both idles of tests");
    assert!(hooked.iter().all(|e| e["member"] == "tests"), "{hooked:?}");
    let last = &hooked[1];
    assert_eq!(last["team_id"], team.id.as_str(), "{last}");
    assert_eq!(last["session_id"], tests.as_str());
    assert_eq!(last["tasks_in_progress"], json!([]));
    assert_eq!(last["last_status"], "done");

    // `all` reaches every non-stopped teammate once, each with its own outcome.
    let broadcast = "standup in five";
    app.run(
        &lead,
        &format!(
            "TOOL<<send_message>>{}",
            json!({"to": "all", "text": broadcast})
        ),
    )
    .await;
    let output: Value = serde_json::from_str(&last_output(&app, &lead).await).unwrap();
    let results = output["results"].as_array().unwrap();
    let reached: Vec<(&str, &str)> = results
        .iter()
        .map(|r| {
            (
                r["to"]["address"].as_str().unwrap(),
                r["outcome"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        reached,
        [
            ("api", "delivered"),
            ("ui", "delivered"),
            ("tests", "delivered")
        ]
    );
    for id in [&api, &ui, &tests] {
        let received: Vec<_> = app
            .app
            .runtime
            .message_rows(id)
            .unwrap()
            .into_iter()
            .filter(|row| row.direction == cyber_server::runtime::messaging::Direction::In)
            .filter(|row| row.text.as_deref() == Some(broadcast))
            .collect();
        assert_eq!(received.len(), 1, "{id}: {received:?}");
    }
    for id in [&api, &ui, &tests] {
        app.app.runtime.wait_idle(id).await;
    }
    app.app.runtime.wait_idle(&lead).await;
}

/// Wait until a Turn waits at a scripted gate.
async fn held_at_gate(app: &ModeApp) {
    tokio::time::timeout(std::time::Duration::from_secs(30), async {
        while app.model.held().is_empty() {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the Turn waits at its gate");
}

/// `agent-teams` → Idle notifications across a restart on the production App: a
/// teammate whose Drain the shutdown cut off is still `working`; the restarted App
/// records it idle and notifies the lead (startup recovery). While the team is stopping
/// a cut-off teammate is recorded idle and no notice is sent, and recovery finishes the
/// stop (`team.stopped.1`, reason `user`): no teammate Drain survived the restart.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_teammate_cut_off_by_a_restart_goes_idle_and_none_is_sent_while_stopping() {
    let temp = tempfile::tempdir().unwrap();
    let setup = || Setup {
        config: json!({"features": {"teams": true}}),
        database: Some(temp.path().join("cyber.db")),
        ..Setup::default()
    };
    let app = ModeApp::start(setup()).await;
    let (_, body) = app.create("default").await;
    let lead = body["data"]["id"].as_str().unwrap().to_owned();
    app.run(
        &lead,
        &spawn_call(json!([{"name": "api", "isolation": "shared"},
            {"name": "docs", "isolation": "shared"}])),
    )
    .await;
    let (team, _) = app.app.runtime.team_membership(&lead).unwrap().unwrap();
    let member = |app: &ModeApp, name: &str| {
        app.app
            .runtime
            .team_members(&team.id)
            .unwrap()
            .into_iter()
            .find(|m| m.name == name)
            .unwrap()
    };
    let api = member(&app, "api").session_id;
    let docs = member(&app, "docs").session_id;
    let working = cyber_server::runtime::teams::MemberStatus::Working;
    let idle = cyber_server::runtime::teams::MemberStatus::Idle;

    // Startup recovery: `api` is cut off mid-Turn and notified after the restart.
    prompt(&app, &api, "GATE<<api-cut>>").await;
    held_at_gate(&app).await;
    assert_eq!(member(&app, "api").status, working);
    let app = app.restart(setup()).await;
    app.model.open_gate();
    let notice = eventually_request(&app, &lead, "api is idle: ").await;
    assert!(notice.contains("<team_idle_notice"), "{notice}");
    assert_eq!(member(&app, "api").status, idle);
    app.app.runtime.wait_idle(&lead).await;

    // While stopping: `docs` is cut off, the team begins to stop, and the restarted App
    // records `docs` idle without a notice.
    prompt(&app, &docs, "GATE<<docs-cut>>").await;
    held_at_gate(&app).await;
    assert_eq!(member(&app, "docs").status, working);
    app.app.runtime.shutdown().await;
    app.app
        .store
        .append(
            &lead,
            cyber_store::Expected::Any,
            vec![cyber_store::NewEvent::new(
                "team.stopped.1",
                json!({"team_id": team.id, "reason": "user", "stopping": true}),
            )],
        )
        .unwrap();
    let app = app.restart(setup()).await;
    app.model.open_gate();
    assert_eq!(
        app.app.runtime.team(&team.id).unwrap().unwrap().status,
        "stopped"
    );
    let stops: Vec<Value> = app
        .events(&lead)
        .into_iter()
        .filter(|e| e.kind == "team.stopped.1")
        .map(|e| e.data)
        .collect();
    assert_eq!(
        stops.last().unwrap(),
        &json!({"team_id": team.id, "reason": "user"})
    );
    let docs_row = member(&app, "docs");
    assert_eq!(
        docs_row.status,
        cyber_server::runtime::teams::MemberStatus::Stopped
    );
    // `docs` was recorded idle before the stop ended.
    let docs_idle = app.events(&lead).into_iter().any(|e| {
        e.kind == "team.member.updated.1" && e.data["name"] == "docs" && e.data["status"] == "idle"
    });
    assert!(docs_idle, "docs is recorded idle");
    let notices = app
        .events(&lead)
        .into_iter()
        .filter(|e| e.kind == "team.member.idle.1" && e.data["name"] == "docs")
        .count();
    assert_eq!(notices, 0, "no idle notice while the team is stopping");
    app.app.runtime.wait_idle(&lead).await;
    assert!(
        requests_of(&app, &lead)
            .iter()
            .all(|r| !request_text(r).contains("docs is idle")),
        "the lead hears nothing about docs"
    );
}

/// `agent-teams` → member status on the production App: a teammate whose command waits
/// for approval is `waiting_input` until the reply, then `working`, then `idle`, each a
/// `team.member.updated.1` on the lead's stream. The request is routed to the lead,
/// labelled `api`, and answered there (Labelled teammate requests).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_teammate_waiting_for_approval_is_waiting_input() {
    let app = ModeApp::start(Setup {
        interactive: true,
        config: json!({"features": {"teams": true}, "permissions": {"bash": "ask"}}),
        ..Setup::default()
    })
    .await;
    let (_, body) = app.create("default").await;
    let lead = body["data"]["id"].as_str().unwrap().to_owned();
    let spawned = app
        .app
        .runtime
        .team_spawn(
            &lead,
            vec![cyber_server::runtime::teams::TeamSpawn {
                name: "api".into(),
                isolation: Some(cyber_core::teams::Isolation::Shared),
                ..Default::default()
            }],
        )
        .await
        .unwrap();
    let api = spawned[0].session_id.clone();
    let team_id = app
        .app
        .runtime
        .team_membership(&lead)
        .unwrap()
        .unwrap()
        .0
        .id;
    let status = || {
        app.app
            .runtime
            .team_members(&team_id)
            .unwrap()
            .into_iter()
            .find(|m| m.name == "api")
            .unwrap()
            .status
    };
    prompt(&app, &api, "RUN<<echo approved>>").await;
    let request = tokio::time::timeout(std::time::Duration::from_secs(30), async {
        loop {
            if let Some(request) = app.app.runtime.pending_requests(Some(&api)).pop() {
                return request;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the command asks for approval");
    use cyber_server::runtime::teams::MemberStatus;
    // Recorded right after the request opens.
    tokio::time::timeout(std::time::Duration::from_secs(30), async {
        while status() != MemberStatus::WaitingInput {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the teammate is waiting_input");
    // Labelled teammate requests: the request is routed to the lead, labelled with the
    // teammate, and answered through the lead's own request route.
    let routes: Vec<&str> = request
        .routed_to
        .iter()
        .map(|route| route.session_id.as_str())
        .collect();
    assert_eq!(routes, [lead.as_str()]);
    let origin = request
        .origin
        .clone()
        .expect("a routed request names its origin");
    assert_eq!(origin.member.as_deref(), Some("api"));
    let listed = app
        .app
        .runtime
        .pending_requests(Some(&lead))
        .into_iter()
        .any(|r| r.id == request.id);
    assert!(listed, "the lead lists the teammate's request");
    let path = format!("/sessions/{lead}/permissions/{}/reply", request.id);
    let (code, body) = app
        .send("POST", &path, Some(json!({"reply": "once"})))
        .await;
    assert_eq!(code, 204, "{body}");
    app.app.runtime.wait_idle(&api).await;
    assert!(last_output(&app, &api).await.contains("approved"));
    // Idle is recorded as the Drain ends, with its notice to the lead.
    eventually_request(&app, &lead, "api is idle: ").await;
    let statuses: Vec<Value> = app
        .events(&lead)
        .into_iter()
        .filter(|e| e.kind == "team.member.updated.1" && e.data["name"] == "api")
        .map(|e| e.data["status"].clone())
        .collect();
    assert_eq!(
        statuses,
        [
            json!("working"),
            json!("waiting_input"),
            json!("working"),
            json!("idle")
        ]
    );
    assert_eq!(status(), MemberStatus::Idle);
    app.app.runtime.wait_idle(&lead).await;
}

fn write_call(path: &str, content: &str) -> String {
    format!("TOOL<<write>>{}", json!({"path": path, "content": content}))
}

fn merge_call(member: &str) -> String {
    format!("TOOL<<team_merge>>{}", json!({ "member": member }))
}

/// `agent-teams` → File conflict avoidance and Merging teammate work on the production
/// App. "Ownership enforced": in a team with shared teammates, `ui` (owns `web/**`)
/// writing `server/api.rs` owned by `api` is denied with `Path owned by teammate api`
/// before the `allow` rules, and so is a shell redirection into it; the lead is not
/// exempt; each owner edits its own paths. "Conflict reported": teammates `one` and `two`
/// change the same line in their worktrees; `team_merge` refuses a shared teammate, a
/// dirty lead checkout and a branch with nothing new; the lead merges `one` with a
/// `--no-ff` merge commit that runs no hooks; merging `two` lists `shared.txt` and leaves
/// the lead's HEAD, status and files unchanged.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ownership_denies_cross_owner_edits_and_merge_reports_conflicts_without_touching_the_lead()
{
    let app = ModeApp::start(Setup {
        config: json!({"features": {"teams": true},
            "permissions": {"bash": "allow", "edit": "allow"}}),
        ..Setup::default()
    })
    .await;
    init_repository(&app.repo);
    std::fs::write(app.repo.join("shared.txt"), "base\n").unwrap();
    git(&app.repo, &["add", "shared.txt"]);
    git(&app.repo, &["commit", "--quiet", "-m", "shared"]);
    let (_, body) = app.create("default").await;
    let lead = body["data"]["id"].as_str().unwrap().to_owned();
    app.run(
        &lead,
        &spawn_call(json!([
            {"name": "api", "isolation": "shared", "owns": ["server/**"]},
            {"name": "ui", "isolation": "shared", "owns": ["web/**"]},
            {"name": "one", "isolation": "worktree"},
            {"name": "two", "isolation": "worktree"}])),
    )
    .await;
    let text = last_output(&app, &lead).await;
    let output: Value = serde_json::from_str(&text).unwrap_or_else(|_| panic!("{text}"));
    let member = |name: &str| {
        output["spawned"]
            .as_array()
            .unwrap()
            .iter()
            .find(|m| m["name"] == name)
            .unwrap_or_else(|| panic!("{output}"))
            .clone()
    };
    let id = |name: &str| member(name)["session_id"].as_str().unwrap().to_owned();
    let (api, ui, one, two) = (id("api"), id("ui"), id("one"), id("two"));

    // Ownership enforced: denied before the `allow` rules, for edits and shell writes.
    app.run(&ui, &write_call("server/api.rs", "fn api() {}\n"))
        .await;
    assert_eq!(last_output(&app, &ui).await, "Path owned by teammate api");
    app.run(&ui, "RUN<<echo hijack > server/api.rs>>").await;
    assert_eq!(last_output(&app, &ui).await, "Path owned by teammate api");
    assert!(!app.repo.join("server/api.rs").exists());
    app.run(&lead, &write_call("web/index.ts", "lead\n")).await;
    assert_eq!(
        last_output(&app, &lead).await,
        "Path owned by teammate ui",
        "the lead is not exempt"
    );
    app.run(&ui, &write_call("web/app.ts", "ui\n")).await;
    app.run(&api, &write_call("server/api.rs", "fn api() {}\n"))
        .await;
    assert_eq!(
        std::fs::read_to_string(app.repo.join("web/app.ts")).unwrap(),
        "ui\n"
    );
    assert_eq!(
        std::fs::read_to_string(app.repo.join("server/api.rs")).unwrap(),
        "fn api() {}\n"
    );
    assert!(!app.repo.join("web/index.ts").exists());

    // Two teammates change the same line in their worktrees.
    for (session, name) in [(&one, "one"), (&two, "two")] {
        app.run(session, &format!("RUN<<printf '{name}\\n' > shared.txt>>"))
            .await;
        let directory = member(name)["directory"].as_str().unwrap().to_owned();
        let directory = std::path::Path::new(&directory);
        assert_eq!(
            std::fs::read_to_string(directory.join("shared.txt")).unwrap(),
            format!("{name}\n")
        );
        git(directory, &["commit", "--quiet", "-am", name]);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let hooks = app.repo.join(".git/hooks");
        for hook in ["pre-merge-commit", "post-merge"] {
            let path = hooks.join(hook);
            std::fs::write(
                &path,
                "#!/bin/sh\ntouch \"$(git rev-parse --show-toplevel)/hooked\"\nexit 1\n",
            )
            .unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
    }
    let offered = tool_names(&request_with(&app, "TOOL<<team_spawn>>"));
    assert!(offered.contains(&"team_merge".to_owned()), "{offered:?}");

    // Refusals: a shared teammate, a dirty checkout.
    app.run(&lead, &merge_call("api")).await;
    let refused = last_output(&app, &lead).await;
    assert!(refused.contains("no branch to merge"), "{refused}");
    std::fs::write(app.repo.join("README.md"), "dirty\n").unwrap();
    app.run(&lead, &merge_call("one")).await;
    let refused = last_output(&app, &lead).await;
    assert!(
        refused.contains("Commit or stash your changes before merging"),
        "{refused}"
    );
    git(&app.repo, &["checkout", "--quiet", "--", "README.md"]);

    // A clean merge: a merge commit with two parents, no hook ran.
    app.run(&lead, &merge_call("one")).await;
    let text = last_output(&app, &lead).await;
    let merged: Value = serde_json::from_str(&text).unwrap_or_else(|_| panic!("{text}"));
    assert_eq!(merged["merge"]["status"], "merged", "{merged}");
    let head = git(&app.repo, &["rev-parse", "HEAD"]).trim().to_owned();
    assert_eq!(merged["merge"]["commit"], head.as_str());
    let parents = git(&app.repo, &["rev-list", "--parents", "-n", "1", "HEAD"]);
    assert_eq!(parents.split_whitespace().count(), 3, "{parents}");
    assert_eq!(
        std::fs::read_to_string(app.repo.join("shared.txt")).unwrap(),
        "one\n"
    );
    assert!(
        !app.repo.join("hooked").exists(),
        "merge hooks are disabled"
    );

    // Conflict reported: the file is listed and the lead's checkout is unchanged.
    let status = git(&app.repo, &["status", "--porcelain"]);
    app.run(&lead, &merge_call("two")).await;
    let text = last_output(&app, &lead).await;
    let conflict: Value = serde_json::from_str(&text).unwrap_or_else(|_| panic!("{text}"));
    assert_eq!(conflict["merge"]["status"], "conflicts", "{conflict}");
    assert_eq!(conflict["merge"]["paths"], json!(["shared.txt"]));
    assert!(
        conflict["summary"].as_str().unwrap().contains("shared.txt"),
        "{conflict}"
    );
    assert_eq!(git(&app.repo, &["rev-parse", "HEAD"]).trim(), head);
    assert_eq!(git(&app.repo, &["status", "--porcelain"]), status);
    assert!(!app.repo.join(".git/MERGE_HEAD").exists());
    assert_eq!(
        std::fs::read_to_string(app.repo.join("shared.txt")).unwrap(),
        "one\n"
    );

    // Nothing new on a merged branch; a teammate is not offered the tool.
    app.run(&lead, &merge_call("one")).await;
    let refused = last_output(&app, &lead).await;
    assert!(refused.contains("Nothing to merge from one"), "{refused}");
    app.run(&one, &merge_call("two")).await;
    assert_eq!(last_output(&app, &one).await, "Unknown tool: team_merge");
    for session in [&api, &ui, &one, &two] {
        app.app.runtime.wait_idle(session).await;
    }
}

/// The `id` of a member of `team_id`.
fn member_session(app: &ModeApp, team_id: &str, name: &str) -> String {
    app.app
        .runtime
        .team_members(team_id)
        .unwrap()
        .into_iter()
        .find(|m| m.name == name)
        .unwrap_or_else(|| panic!("no member {name}"))
        .session_id
}

/// The `team.*` records of one kind on a lead's stream.
fn team_records(app: &ModeApp, lead: &str, kind: &str) -> Vec<Value> {
    app.events(lead)
        .into_iter()
        .filter(|e| e.kind == kind)
        .map(|e| e.data)
        .collect()
}

/// A team's rows as its projection holds them: the team, its members and its board.
fn team_rows(app: &ModeApp, team_id: &str) -> Value {
    let runtime = &app.app.runtime;
    json!({
        "team": runtime.team(team_id).unwrap(),
        "members": runtime.team_members(team_id).unwrap(),
        "board": runtime.board(team_id).unwrap(),
    })
}

/// Wait until a team reaches `status`.
async fn team_status(app: &ModeApp, team_id: &str, status: &str) {
    tokio::time::timeout(std::time::Duration::from_secs(30), async {
        while app.app.runtime.team(team_id).unwrap().unwrap().status != status {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("team {team_id} never became {status}"));
}

/// `agent-teams` → Team budgets and Shutdown and resume on the production App, with a
/// priced scripted model ($1.00 a step).
///
/// "Resume next day": a lead stops its team while `api` owns an `in_progress` task;
/// the App is rebuilt over the same data directory; the team is still stopped, a resume
/// records `team.resumed.1`, members go back to `idle`, and `api` is told to continue its
/// task: its next request carries its prior history, and the board is intact.
///
/// "Budget exceeded": with `budgets.team.max_cost_usd: 5`, the teammate Turn that would
/// start over the budget does not start (its input stays queued); `team.budget.exceeded.1`
/// is recorded once with the per-member spend; the team stops (reason `budget`); the
/// lead, whose Turns are never blocked, receives `Team budget exceeded (max_cost_usd
/// 5.00, spent 5.00): lead $3.00, api $1.00, ui $1.00`; a stopped teammate starts no
/// Turn. Deleting the lead stops the team with `lead_deleted` and keeps its records:
/// a resume needs `--lead`, a new lead takes over, a rebuild re-projects identical
/// rows, and the resumed team, still over budget, stops again at its next Turn.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_team_over_budget_stops_and_resumes_with_its_context() {
    let temp = tempfile::tempdir().unwrap();
    let setup = |config: Value| Setup {
        config,
        database: Some(temp.path().join("cyber.db")),
        models: Some(json!({"main": {"cost": {"input": 10000.0, "output": 0.0}}})),
        ..Setup::default()
    };
    let app = ModeApp::start(setup(json!({"features": {"teams": true}}))).await;

    // Resume next day: `api` claims a task and remembers a codeword, then the team stops.
    let (_, body) = app.create("default").await;
    let lead = body["data"]["id"].as_str().unwrap().to_owned();
    app.run(
        &lead,
        &spawn_call(json!([{"name": "api", "isolation": "shared"}])),
    )
    .await;
    let team = task_team(&app.app.runtime, &lead);
    let api = member_session(&app, &team, "api");
    app.run(
        &lead,
        &task_call(
            "team_task_create",
            json!({"title": "Wire the route", "owner": "api"}),
        ),
    )
    .await;
    let task_id = task_output(&last_output(&app, &lead).await)["id"]
        .as_str()
        .unwrap()
        .to_owned();
    app.run(
        &api,
        &task_call(
            "team_task_update",
            json!({"id": task_id, "status": "in_progress"}),
        ),
    )
    .await;
    app.run(&api, "The codeword is PELICAN.").await;
    eventually_request(&app, &lead, "api is idle with").await;
    app.app.runtime.wait_idle(&lead).await;
    let stopped = app.app.runtime.team_stop(&team).await.unwrap();
    assert_eq!(stopped.status, "stopped");
    let stops = team_records(&app, &lead, "team.stopped.1");
    assert_eq!(
        stops,
        [
            json!({"team_id": team, "reason": "user", "stopping": true}),
            json!({"team_id": team, "reason": "user"}),
        ]
    );
    let before = team_rows(&app, &team);
    assert!(
        before["members"]
            .as_array()
            .unwrap()
            .iter()
            .all(|m| m["status"] == "stopped"),
        "{before}"
    );

    // The next day: the App is rebuilt over the same data directory.
    let budget = json!({"features": {"teams": true}, "budgets": {"team": {"max_cost_usd": 5.0}}});
    let app = app.restart(setup(budget.clone())).await;
    assert_eq!(team_rows(&app, &team), before, "a restart changes nothing");
    let error = app
        .app
        .runtime
        .team_resume(&team, Some("ses_someone_else"))
        .await
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("only be named after the lead was deleted"),
        "{error}"
    );
    let api_requests = requests_of(&app, &api).len();
    let resumed = app.app.runtime.team_resume(&team, None).await.unwrap();
    assert_eq!(resumed.status, "active");
    assert_eq!(
        team_records(&app, &lead, "team.resumed.1"),
        [json!({"team_id": team, "lead_id": lead})]
    );
    let next = eventually_request(
        &app,
        &api,
        &format!("The team resumed. Continue {task_id}: Wire the route"),
    )
    .await;
    assert!(next.contains("The codeword is PELICAN."), "{next}");
    assert_eq!(requests_of(&app, &api).len(), api_requests + 1);
    app.app.runtime.wait_idle(&api).await;
    let board = app.app.runtime.board(&team).unwrap();
    assert_eq!(board.len(), 1);
    assert_eq!(
        (board[0].status, board[0].owner.as_deref(), board[0].version),
        (
            cyber_server::runtime::teams::TaskStatus::InProgress,
            Some("api"),
            1
        )
    );
    let error = app.app.runtime.team_resume(&team, None).await.unwrap_err();
    assert!(
        error.to_string().contains("only a stopped team resumes"),
        "{error}"
    );
    eventually_request(&app, &lead, "api is idle with").await;
    app.app.runtime.wait_idle(&lead).await;
    app.app.runtime.team_stop(&team).await.unwrap();

    // Budget exceeded: a new team takes `budgets.team` and spends $1.00 a step.
    let (_, body) = app.create("default").await;
    let lead = body["data"]["id"].as_str().unwrap().to_owned();
    app.run(
        &lead,
        &spawn_call(json!([{"name": "api", "isolation": "shared"},
            {"name": "ui", "isolation": "shared"}])),
    )
    .await;
    let team = task_team(&app.app.runtime, &lead);
    let (api, ui) = (
        member_session(&app, &team, "api"),
        member_session(&app, &team, "ui"),
    );
    for (id, name) in [(&api, "api"), (&ui, "ui")] {
        app.run(id, &format!("{name} step one")).await;
        eventually_request(&app, &lead, &format!("{name} is idle: ok")).await;
        app.app.runtime.wait_idle(&lead).await;
    }
    assert!(team_records(&app, &lead, "team.budget.exceeded.1").is_empty());
    let api_requests = requests_of(&app, &api).len();
    prompt(&app, &api, "api step two").await;
    let line =
        "Team budget exceeded (max_cost_usd 5.00, spent 5.00): lead $3.00, api $1.00, ui $1.00";
    eventually_request(&app, &lead, line).await;
    team_status(&app, &team, "stopped").await;
    app.app.runtime.wait_idle(&api).await;
    app.app.runtime.wait_idle(&lead).await;
    assert_eq!(
        requests_of(&app, &api).len(),
        api_requests,
        "no Turn over budget"
    );
    let state = app.app.runtime.state(&api).await.unwrap();
    let queued: Vec<_> = state
        .pending(cyber_server::runtime::Delivery::Queue)
        .collect();
    assert_eq!(queued.len(), 1, "the input stays queued");
    let exceeded = team_records(&app, &lead, "team.budget.exceeded.1");
    assert_eq!(exceeded.len(), 1, "{exceeded:?}");
    assert_eq!(
        exceeded[0]["budget"],
        json!({"max_cost_usd": 5.0, "enforcement": "soft"})
    );
    assert_eq!(exceeded[0]["spent"]["limit"], "max_cost_usd");
    assert_eq!(exceeded[0]["spent"]["cost_usd"], 5.0);
    let shares: Vec<(String, f64)> = exceeded[0]["members"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| {
            (
                m["name"].as_str().unwrap().to_owned(),
                m["cost_usd"].as_f64().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        shares,
        [
            ("lead".into(), 3.0),
            ("api".into(), 1.0),
            ("ui".into(), 1.0)
        ]
    );
    assert_eq!(
        team_records(&app, &lead, "team.stopped.1").last().unwrap(),
        &json!({"team_id": team, "reason": "budget"})
    );
    let ui_requests = requests_of(&app, &ui).len();
    app.run(&ui, "ui while stopped").await;
    assert_eq!(
        requests_of(&app, &ui).len(),
        ui_requests,
        "a stopped teammate starts no Turn"
    );

    // Deleting the lead stops the team (`lead_deleted`) and keeps its records.
    let (status, body) = app.send("DELETE", &format!("/sessions/{lead}"), None).await;
    assert!((200..300).contains(&status), "{status} {body}");
    assert!(
        app.events(&lead)
            .iter()
            .all(|e| e.kind.starts_with("team."))
    );
    assert_eq!(
        team_records(&app, &lead, "team.stopped.1").last().unwrap(),
        &json!({"team_id": team, "reason": "lead_deleted"})
    );
    let error = app.app.runtime.team_resume(&team, None).await.unwrap_err();
    assert!(error.to_string().contains("--lead <session>"), "{error}");
    let error = app
        .app
        .runtime
        .team_resume(&team, Some(&api))
        .await
        .unwrap_err();
    assert!(
        error.to_string().contains("Teammates cannot lead a team"),
        "{error}"
    );
    let (_, body) = app.create("default").await;
    let new_lead = body["data"]["id"].as_str().unwrap().to_owned();
    let resumed = app
        .app
        .runtime
        .team_resume(&team, Some(&new_lead))
        .await
        .unwrap();
    assert_eq!(resumed.lead_id, new_lead);
    assert_eq!(member_session(&app, &team, "lead"), new_lead);
    assert_eq!(
        team_records(&app, &new_lead, "team.resumed.1"),
        [json!({"team_id": team, "lead_id": new_lead})]
    );
    // Still over budget: the queued Turns restarted by the resume stop the team again,
    // and the new lead hears it.
    eventually_request(&app, &new_lead, "Team budget exceeded (max_cost_usd 5.00").await;
    team_status(&app, &team, "stopped").await;
    for id in [&api, &ui, &new_lead] {
        app.app.runtime.wait_idle(id).await;
    }
    assert_eq!(
        team_records(&app, &new_lead, "team.stopped.1")
            .last()
            .unwrap(),
        &json!({"team_id": team, "reason": "budget"})
    );
    let rows = team_rows(&app, &team);
    app.app.runtime.rebuild_teams().unwrap();
    assert_eq!(
        team_rows(&app, &team),
        rows,
        "a rebuild re-projects identical rows"
    );
}

/// `agent-teams` → Team budgets and Shutdown and resume on the production App: a
/// definition's `budget` wins over `budgets.team`; a stop lets a working teammate finish its current Turn (`stopping`),
/// starts no other (the next input stays queued), and the team is `stopped` once the
/// Turn ends; a resume restarts the Drain of a teammate with queued input.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_stop_lets_a_working_teammate_finish_its_turn() {
    let app = ModeApp::start(Setup {
        config: json!({"features": {"teams": true},
            "budgets": {"team": {"max_cost_usd": 50.0}}}),
        ..Setup::default()
    })
    .await;
    let parse =
        |text: &str| cyber_core::teams::parse(text, std::path::Path::new("feature.md"), 6).unwrap();
    let (_, body) = app.create("default").await;
    let lead = body["data"]["id"].as_str().unwrap().to_owned();
    let feature = parse(
        "---\nbudget:\n  max_turns: 40\nteammates:\n  - name: api\n    agent: build\n    isolation: shared\n---\n",
    );
    let (team, _) = app.app.runtime.create_team(&lead, &feature).await.unwrap();
    let created = team_records(&app, &lead, "team.created.1");
    assert_eq!(
        created[0]["budget"],
        json!({"max_turns": 40, "enforcement": "soft"})
    );
    let (_, body) = app.create("default").await;
    let other = body["data"]["id"].as_str().unwrap().to_owned();
    app.app
        .runtime
        .team_spawn(
            &other,
            vec![cyber_server::runtime::teams::TeamSpawn {
                name: "ui".into(),
                isolation: Some(cyber_core::teams::Isolation::Shared),
                ..Default::default()
            }],
        )
        .await
        .unwrap();
    assert_eq!(
        team_records(&app, &other, "team.created.1")[0]["budget"],
        json!({"max_cost_usd": 50.0, "enforcement": "soft"}),
        "budgets.team applies to a team whose definition sets none"
    );

    // A stop while `api` works: its Turn finishes, the next input waits.
    let api = member_session(&app, &team.id, "api");
    prompt(&app, &api, "GATE<<api-stop>>").await;
    held_at_gate(&app).await;
    let stopping = app.app.runtime.team_stop(&team.id).await.unwrap();
    assert_eq!(stopping.status, "stopping");
    prompt(&app, &api, "after the stop").await;
    app.model.open_gate();
    team_status(&app, &team.id, "stopped").await;
    app.app.runtime.wait_idle(&api).await;
    let texts: Vec<String> = requests_of(&app, &api).iter().map(request_text).collect();
    assert_eq!(texts.len(), 1, "{texts:?}");
    assert!(!texts[0].contains("after the stop"));
    assert_eq!(
        team_records(&app, &lead, "team.stopped.1"),
        [
            json!({"team_id": team.id, "reason": "user", "stopping": true}),
            json!({"team_id": team.id, "reason": "user"}),
        ]
    );
    // No idle notice while stopping, and a second stop changes nothing.
    assert!(team_records(&app, &lead, "team.member.idle.1").is_empty());
    app.app.runtime.team_stop(&team.id).await.unwrap();
    assert_eq!(team_records(&app, &lead, "team.stopped.1").len(), 2);

    // A resume restarts `api`'s Drain for its queued input.
    app.app.runtime.team_resume(&team.id, None).await.unwrap();
    eventually_request(&app, &api, "after the stop").await;
    app.app.runtime.wait_idle(&api).await;
    eventually_request(&app, &lead, "api is idle: ok").await;
    app.app.runtime.wait_idle(&lead).await;
}

/// Read SSE `event:` names from `response` until `wanted` arrives, in arrival order.
async fn events_until(response: &mut reqwest::Response, wanted: &str) -> Vec<String> {
    let mut text = String::new();
    let read = async {
        while !text.contains(&format!("event: {wanted}\n")) {
            let chunk = response
                .chunk()
                .await
                .unwrap()
                .expect("the stream stays open");
            text.push_str(&String::from_utf8_lossy(&chunk));
        }
    };
    tokio::time::timeout(std::time::Duration::from_secs(30), read)
        .await
        .unwrap_or_else(|_| panic!("no {wanted} event"));
    text.lines()
        .filter_map(|line| line.strip_prefix("event: "))
        .map(str::to_owned)
        .collect()
}

/// Task 7.1 on the production App over authenticated TCP: every team route answers
/// `404 FeatureDisabledError` while `teams` is off (also for a team that exists, so turning
/// the feature off closes them); `POST /teams` starts a team from a definition with a
/// budget over it, refuses a start with nothing to spawn, an unknown definition, a lead
/// already in a team (`ConflictError`) and a seventh teammate (`TeamLimitError`, nothing
/// created); unknown teams are `TeamNotFoundError`; bad filters are `InvalidRequestError`;
/// `GET /teams/:id/events` streams the team's `team.*` records of a stop and a resume.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn team_routes_start_show_stream_and_refuse_with_error_tags() {
    let app = ModeApp::start(Setup::default()).await;
    let (_, body) = app.create("default").await;
    let lead = body["data"]["id"].as_str().unwrap().to_owned();
    let start = json!({"lead_session_id": lead, "definition": "review",
        "budget": {"max_cost_usd": 3.0}});
    for (method, path, body) in [
        ("GET", "/teams", None),
        ("POST", "/teams", Some(start.clone())),
    ] {
        let (status, body) = app.send(method, path, body).await;
        assert_eq!(
            (status, body["_tag"].as_str()),
            (404, Some("FeatureDisabledError")),
            "{method} {path}: {body}"
        );
    }
    let config = app.repo.join("cyber.jsonc");
    write(config.clone(), r#"{"features": {"teams": true}}"#);
    write(
        app.root.join("config/teams/review.md"),
        "---\ndescription: Review\nteammates:\n  - name: security\n    agent: reviewer\n---\nReview it.\n",
    );
    refusals(&app, &lead).await;

    let (status, body) = app.send("POST", "/teams", Some(start)).await;
    assert_eq!(status, 201, "{body}");
    let view = &body["data"]["team"];
    let team = view["team"]["id"].as_str().unwrap().to_owned();
    assert_eq!(view["team"]["name"], "review");
    assert_eq!(view["budget"]["max_cost_usd"], 3.0, "{view}");
    assert_eq!(body["data"]["spawned"][0]["name"], "security");
    assert_eq!(body["data"]["spawned"][0]["isolation"], "shared");
    let (status, body) = app
        .send(
            "POST",
            "/teams",
            Some(json!({"lead_session_id": lead, "teammates": [{"name": "api"}]})),
        )
        .await;
    assert_eq!(
        (status, body["_tag"].as_str()),
        (409, Some("ConflictError")),
        "{body}"
    );
    let (_, body) = app
        .send("GET", &format!("/teams/{team}/tasks?status=todo"), None)
        .await;
    assert_eq!(body["data"], json!([]), "{body}");
    let (_, body) = app.send("GET", "/teams?status=active", None).await;
    assert_eq!(body["data"][0]["id"], team.as_str(), "{body}");
    assert_eq!(body["data"][0]["teammates"], 1);

    let mut stream = reqwest::Client::builder()
        .no_proxy()
        .build()
        .unwrap()
        .get(format!("{}/api/v1/teams/{team}/events", app.base))
        .basic_auth("cyber", Some(support::PASSWORD))
        .send()
        .await
        .unwrap();
    assert_eq!(stream.status(), 200);
    events_until(&mut stream, "server.connected").await;
    let (status, body) = app.send("POST", &format!("/teams/{team}/stop"), None).await;
    assert_eq!(
        (status, &body["data"]["status"]),
        (200, &json!("stopped")),
        "{body}"
    );
    assert_eq!(body["data"]["stop_reason"], "user");
    let seen = events_until(&mut stream, "team.stopped.1").await;
    assert!(
        seen.iter()
            .all(|kind| kind.starts_with("team.") || kind == "server.connected"),
        "{seen:?}"
    );
    let (status, body) = app
        .send("POST", &format!("/teams/{team}/resume"), Some(json!({})))
        .await;
    assert_eq!(
        (status, &body["data"]["status"]),
        (200, &json!("active")),
        "{body}"
    );
    events_until(&mut stream, "team.resumed.1").await;
    let (status, body) = app
        .send("POST", &format!("/teams/{team}/resume"), None)
        .await;
    assert_eq!(
        (status, body["_tag"].as_str()),
        (409, Some("ConflictError")),
        "{body}"
    );

    // Turning the feature off closes the routes of a team that exists (fail closed).
    write(config, r#"{"features": {"teams": false}}"#);
    for (method, path) in [
        ("GET", format!("/teams/{team}")),
        ("GET", format!("/teams/{team}/tasks")),
        ("POST", format!("/teams/{team}/stop")),
        ("GET", format!("/teams/{team}/events")),
    ] {
        let (status, body) = app.send(method, &path, None).await;
        assert_eq!(
            (status, body["_tag"].as_str()),
            (404, Some("FeatureDisabledError")),
            "{method} {path}: {body}"
        );
    }
    let state = app.app.runtime.team(&team).unwrap().unwrap();
    assert_eq!(state.status, "active", "a refused stop changes nothing");
}

/// Refusals while the feature is on; none of them creates a team.
async fn refusals(app: &ModeApp, lead: &str) {
    let seven: Vec<Value> = (1..=7).map(|n| json!({"name": format!("t{n}")})).collect();
    for (body, status, tag, message) in [
        (
            json!({"lead_session_id": lead}),
            400,
            "InvalidRequestError",
            "A team needs a definition or at least one teammate",
        ),
        (
            json!({"lead_session_id": lead, "definition": "nope"}),
            400,
            "InvalidRequestError",
            "No team definition nope",
        ),
        (
            json!({"lead_session_id": lead, "teammates": seven}),
            409,
            "TeamLimitError",
            "TeamLimitError: Team size limit reached (6)",
        ),
        (
            json!({"lead_session_id": "ses_nope", "teammates": [{"name": "api"}]}),
            404,
            "SessionNotFoundError",
            "",
        ),
    ] {
        let (got, body) = app.send("POST", "/teams", Some(body)).await;
        assert_eq!((got, body["_tag"].as_str()), (status, Some(tag)), "{body}");
        assert!(
            body["message"].as_str().unwrap().contains(message),
            "{body}"
        );
    }
    let (_, body) = app.send("GET", "/teams", None).await;
    assert_eq!(body["data"], json!([]), "nothing was created: {body}");
    for (path, status, tag) in [
        ("/teams?status=bogus", 400, "InvalidRequestError"),
        ("/teams/team_nope", 404, "TeamNotFoundError"),
        ("/teams/team_nope/tasks", 404, "TeamNotFoundError"),
    ] {
        let (got, body) = app.send("GET", path, None).await;
        assert_eq!(
            (got, body["_tag"].as_str()),
            (status, Some(tag)),
            "{path}: {body}"
        );
    }
}

/// The spawned teammates of a lead's `team_spawn` output, by name.
fn spawned(text: &str) -> std::collections::HashMap<String, Value> {
    let output: Value = serde_json::from_str(text).unwrap_or_else(|_| panic!("{text}"));
    output["spawned"]
        .as_array()
        .unwrap_or_else(|| panic!("{output}"))
        .iter()
        .map(|m| (m["name"].as_str().unwrap().to_owned(), m.clone()))
        .collect()
}

/// Regression: a deleted lead's stream keeps only its `team.*` records, and that is a
/// Session that no longer exists, not a corrupt one. `GET /sessions/<lead>` and a second
/// `DELETE` answer `404 SessionNotFoundError`; a teammate whose Turn was in flight when
/// its lead went is refused its next command (no lead bounds it any more: fail closed),
/// raises no request, and starts no further Turn.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_deleted_lead_is_not_found_and_its_teammates_fail_closed() {
    let app = ModeApp::start(Setup {
        interactive: true,
        config: json!({"features": {"teams": true}, "permissions": {"bash": "ask"}}),
        ..Setup::default()
    })
    .await;
    let (_, body) = app.create("default").await;
    let lead = body["data"]["id"].as_str().unwrap().to_owned();
    let api = app
        .app
        .runtime
        .team_spawn(
            &lead,
            vec![cyber_server::runtime::teams::TeamSpawn {
                name: "api".into(),
                isolation: Some(cyber_core::teams::Isolation::Shared),
                ..Default::default()
            }],
        )
        .await
        .unwrap()[0]
        .session_id
        .clone();
    prompt(&app, &api, "GATE<<api-mid>> RUN<<touch after-deletion>>").await;
    held_at_gate(&app).await;

    let (status, body) = app.send("DELETE", &format!("/sessions/{lead}"), None).await;
    assert!((200..300).contains(&status), "{status} {body}");
    assert!(!app.events(&lead).is_empty(), "the team records stay");
    for method in ["GET", "DELETE"] {
        let (status, body) = app.send(method, &format!("/sessions/{lead}"), None).await;
        assert_eq!(status, 404, "{method} {body}");
        assert_eq!(body["_tag"], "SessionNotFoundError", "{method} {body}");
    }

    let requests = requests_of(&app, &api).len();
    app.model.open_gate();
    app.app.runtime.wait_idle(&api).await;
    assert!(app.app.runtime.pending_requests(Some(&api)).is_empty());
    assert!(!app.repo.join("after-deletion").exists());
    let output = last_output(&app, &api).await;
    assert!(output.contains("was deleted"), "{output}");
    assert_eq!(requests_of(&app, &api).len(), requests, "no further Turn");
}

/// Regression: ownership is checked on the canonical path too, so a symlink inside the
/// shared Location (`s2 -> server`) does not hide a path another member owns from a
/// shared teammate, for edits and shell writes alike.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_symlink_in_the_shared_location_does_not_bypass_ownership() {
    let app = ModeApp::start(Setup {
        config: json!({"features": {"teams": true},
            "permissions": {"bash": "allow", "edit": "allow"}}),
        ..Setup::default()
    })
    .await;
    std::fs::create_dir_all(app.repo.join("server")).unwrap();
    std::fs::write(app.repo.join("server/api.rs"), "fn api() {}\n").unwrap();
    // However it got there (a checkout, a tool the guard does not see): `s2 -> server`.
    std::os::unix::fs::symlink("server", app.repo.join("s2")).unwrap();
    let (_, body) = app.create("default").await;
    let lead = body["data"]["id"].as_str().unwrap().to_owned();
    app.run(
        &lead,
        &spawn_call(json!([
            {"name": "api", "isolation": "shared", "owns": ["server/**"]},
            {"name": "ui", "isolation": "shared", "owns": ["web/**"]}])),
    )
    .await;
    let members = spawned(&last_output(&app, &lead).await);
    let ui = members["ui"]["session_id"].as_str().unwrap().to_owned();
    app.run(&ui, &write_call("s2/new.rs", "hijacked\n")).await;
    assert_eq!(last_output(&app, &ui).await, "Path owned by teammate api");
    app.run(&ui, "RUN<<echo hijacked > s2/api.rs>>").await;
    assert_eq!(last_output(&app, &ui).await, "Path owned by teammate api");
    assert!(!app.repo.join("server/new.rs").exists());
    assert_eq!(
        std::fs::read_to_string(app.repo.join("server/api.rs")).unwrap(),
        "fn api() {}\n"
    );
    for id in [
        &ui,
        &members["api"]["session_id"].as_str().unwrap().to_owned(),
    ] {
        app.app.runtime.wait_idle(id).await;
    }
}

/// Commit `files` (`None` removes the path) on the branch checked out at `directory`.
fn commit_in(directory: &std::path::Path, files: &[(&str, Option<&str>)], message: &str) {
    for (path, content) in files {
        match content {
            Some(content) => {
                write(directory.join(path), content);
                git(directory, &["add", "-f", "--", path]);
            }
            None => {
                git(directory, &["rm", "--quiet", "--cached", "--", path]);
                std::fs::remove_file(directory.join(path)).unwrap();
            }
        }
    }
    git(directory, &["commit", "--quiet", "-m", message]);
}

/// `team_merge` of `one` by `lead` is refused with `text` and changes nothing.
async fn merge_refused(app: &ModeApp, lead: &str, head: &str, text: &str) {
    app.run(lead, &merge_call("one")).await;
    let output = last_output(app, lead).await;
    assert!(output.contains(text), "{text}: {output}");
    assert!(!output.contains("\"merged\""), "{output}");
    assert_eq!(git(&app.repo, &["rev-parse", "HEAD"]), head, "{output}");
    assert!(!app.repo.join(".git/MERGE_HEAD").exists());
    assert_eq!(
        std::fs::read_to_string(app.repo.join(".env")).unwrap(),
        "SECRET=1\n"
    );
}

/// Regression: `team_merge` never writes over what the lead's checkout holds outside
/// git, nor around the lead's permissions. A branch adding `.env` that the lead keeps
/// ignored is refused (`.env exists untracked in your checkout`) and the lead's `.env`
/// survives; a branch adding `.cyber/hooks.jsonc` is refused as protected configuration;
/// a branch touching a path the lead's `edit` rules deny is refused; each refusal leaves
/// HEAD and the files unchanged, and a branch without such paths merges.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn team_merge_respects_untracked_files_protected_configuration_and_lead_rules() {
    let app = ModeApp::start(Setup {
        config: json!({"features": {"teams": true},
            "permissions": {"bash": "allow", "edit": {"*": "allow", "locked/**": "deny"}}}),
        ..Setup::default()
    })
    .await;
    init_repository(&app.repo);
    std::fs::write(app.repo.join(".gitignore"), ".env\n").unwrap();
    git(&app.repo, &["add", ".gitignore"]);
    git(&app.repo, &["commit", "--quiet", "-m", "ignore"]);
    std::fs::write(app.repo.join(".env"), "SECRET=1\n").unwrap();
    let (_, body) = app.create("default").await;
    let lead = body["data"]["id"].as_str().unwrap().to_owned();
    app.run(
        &lead,
        &spawn_call(json!([{"name": "one", "isolation": "worktree"}])),
    )
    .await;
    let members = spawned(&last_output(&app, &lead).await);
    let directory = std::path::PathBuf::from(members["one"]["directory"].as_str().unwrap());
    let head = git(&app.repo, &["rev-parse", "HEAD"]);
    let refused = |text: &'static str| merge_refused(&app, &lead, &head, text);

    commit_in(&directory, &[(".env", Some("EVIL=1\n"))], "env");
    refused(".env exists untracked in your checkout").await;
    commit_in(&directory, &[(".env", None)], "no env");

    commit_in(&directory, &[(".cyber/hooks.jsonc", Some("{}\n"))], "hooks");
    refused("team_merge does not change protected configuration (.cyber/hooks.jsonc)").await;
    assert!(!app.repo.join(".cyber/hooks.jsonc").exists());
    commit_in(&directory, &[(".cyber/hooks.jsonc", None)], "no hooks");

    commit_in(&directory, &[("locked/x.txt", Some("x\n"))], "locked");
    refused("").await;
    assert!(!app.repo.join("locked/x.txt").exists());
    commit_in(&directory, &[("locked/x.txt", None)], "unlocked");

    commit_in(&directory, &[("ok.txt", Some("ok\n"))], "ok");
    app.run(&lead, &merge_call("one")).await;
    let text = last_output(&app, &lead).await;
    let merged: Value = serde_json::from_str(&text).unwrap_or_else(|_| panic!("{text}"));
    assert_eq!(merged["merge"]["status"], "merged", "{merged}");
    assert_eq!(
        std::fs::read_to_string(app.repo.join("ok.txt")).unwrap(),
        "ok\n"
    );
    assert_eq!(
        std::fs::read_to_string(app.repo.join(".env")).unwrap(),
        "SECRET=1\n"
    );
    assert!(!app.repo.join("locked").exists());
}

/// Regression: the team gate and the stop bind a teammate's descendants. A teammate's
/// background subagent still running keeps the stopping team from being `stopped`; once
/// its Turn ends the team stops, and neither input prompted to the subagent nor its
/// handback to the teammate starts another Turn.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_stopped_team_starts_no_turn_of_a_teammates_subagent() {
    let app = ModeApp::start(Setup {
        config: json!({"features": {"teams": true}, "permissions": {"agent": "allow"}}),
        ..Setup::default()
    })
    .await;
    let (_, body) = app.create("default").await;
    let lead = body["data"]["id"].as_str().unwrap().to_owned();
    let (team, _) = app
        .app
        .runtime
        .create_team(
            &lead,
            &cyber_core::teams::parse(
                "---\nteammates:\n  - name: api\n    agent: build\n    isolation: shared\n---\n",
                std::path::Path::new("feature.md"),
                6,
            )
            .unwrap(),
        )
        .await
        .unwrap();
    let api = member_session(&app, &team.id, "api");
    // Escaped, so only the subagent's own prompt reads as a gate.
    let call = r#"TOOL<<agent>>{"prompt": "GATE\u003c\u003csub-1>>", "background": true}"#;
    app.run(&api, call).await;
    held_at_gate(&app).await;
    let filter = cyber_server::runtime::ListFilter {
        parent: Some(api.clone()),
        ..Default::default()
    };
    let row = app.app.runtime.list(&filter).unwrap().sessions.pop();
    let row = row.expect("the subagent's Session");
    let child = row.id;
    // A subagent's prompt cache key is `<root>:<agent>`.
    let child_key = format!("{api}:{}", row.agent);

    let stopping = app.app.runtime.team_stop(&team.id).await.unwrap();
    assert_eq!(stopping.status, "stopping", "the subagent still runs");
    let api_requests = requests_of(&app, &api).len();
    app.model.open_gate();
    team_status(&app, &team.id, "stopped").await;
    app.app.runtime.wait_idle(&child).await;
    app.app.runtime.wait_idle(&api).await;
    let texts: Vec<String> = requests_of(&app, &child_key)
        .iter()
        .map(request_text)
        .collect();
    assert_eq!(texts.len(), 1, "{texts:?}");
    prompt(&app, &child, "subagent while stopped").await;
    app.app.runtime.wait_idle(&child).await;
    assert_eq!(
        requests_of(&app, &child_key).len(),
        1,
        "no Turn while stopped"
    );
    assert_eq!(
        requests_of(&app, &api).len(),
        api_requests,
        "the handback starts no teammate Turn"
    );
}

/// `git` in `repo` that may fail: whether it succeeded.
fn git_status(repo: &std::path::Path, args: &[&str]) -> bool {
    std::process::Command::new("git")
        .current_dir(repo)
        .args(args)
        .output()
        .unwrap()
        .status
        .success()
}

/// `team_merge` of `one` by `lead` is refused with `text`; HEAD stays `head`.
async fn merge_blocked(app: &ModeApp, lead: &str, head: &str, text: &str) {
    app.run(lead, &merge_call("one")).await;
    let output = last_output(app, lead).await;
    assert!(output.contains(text), "{text}: {output}");
    assert_eq!(git(&app.repo, &["rev-parse", "HEAD"]), head, "{output}");
}

/// Regression: `team_merge` never commits into, nor aborts, an operation the user has in
/// progress in the lead's checkout. A `--no-commit` merge whose index matches HEAD
/// (status clean) is refused and its `MERGE_HEAD` survives; so is a rebase stopped with
/// a clean tree (detached, `rebase-merge/`), and a plain detached HEAD; once the
/// checkout is back on its branch the merge lands.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn team_merge_refuses_and_keeps_an_operation_in_progress() {
    let app = ModeApp::start(Setup {
        config: json!({"features": {"teams": true},
            "permissions": {"bash": "allow", "edit": "allow"}}),
        ..Setup::default()
    })
    .await;
    init_repository(&app.repo);
    let (_, body) = app.create("default").await;
    let lead = body["data"]["id"].as_str().unwrap().to_owned();
    app.run(
        &lead,
        &spawn_call(json!([{"name": "one", "isolation": "worktree"}])),
    )
    .await;
    let members = spawned(&last_output(&app, &lead).await);
    let directory = std::path::PathBuf::from(members["one"]["directory"].as_str().unwrap());
    commit_in(&directory, &[("ok.txt", Some("ok\n"))], "ok");
    git(&app.repo, &["checkout", "--quiet", "-b", "side"]);
    commit_in(&app.repo, &[("side.txt", Some("side\n"))], "side");
    git(&app.repo, &["checkout", "--quiet", "main"]);
    commit_in(&app.repo, &[("main.txt", Some("main\n"))], "main");
    let head = git(&app.repo, &["rev-parse", "HEAD"]);

    // The user's own merge, staged but not concluded: status is clean.
    git(
        &app.repo,
        &[
            "merge",
            "--quiet",
            "-s",
            "ours",
            "--no-commit",
            "--no-ff",
            "side",
        ],
    );
    let merge_head = app.repo.join(".git/MERGE_HEAD");
    let theirs = std::fs::read_to_string(&merge_head).unwrap();
    assert_eq!(
        git(
            &app.repo,
            &["status", "--porcelain", "--untracked-files=no"]
        ),
        ""
    );
    merge_blocked(&app, &lead, &head, "is in progress in your checkout").await;
    assert_eq!(
        std::fs::read_to_string(&merge_head).unwrap(),
        theirs,
        "the user's merge is kept"
    );
    git(&app.repo, &["merge", "--abort"]);

    // A rebase stopped by a failing `--exec`, with a clean tree.
    assert!(!git_status(
        &app.repo,
        &[
            "rebase",
            "--quiet",
            "--exec",
            "git rev-parse --verify --quiet refs/heads/none",
            "HEAD~1"
        ]
    ));
    assert!(app.repo.join(".git/rebase-merge").is_dir());
    let stopped = git(&app.repo, &["rev-parse", "HEAD"]);
    merge_blocked(&app, &lead, &stopped, "is in progress in your checkout").await;
    assert!(
        app.repo.join(".git/rebase-merge").is_dir(),
        "the rebase is kept"
    );
    git(&app.repo, &["rebase", "--abort"]);

    git(&app.repo, &["checkout", "--quiet", "--detach"]);
    merge_blocked(&app, &lead, &head, "detached HEAD").await;
    git(&app.repo, &["checkout", "--quiet", "main"]);

    app.run(&lead, &merge_call("one")).await;
    let text = last_output(&app, &lead).await;
    let merged: Value = serde_json::from_str(&text).unwrap_or_else(|_| panic!("{text}"));
    assert_eq!(merged["merge"]["status"], "merged", "{merged}");
    assert_eq!(
        std::fs::read_to_string(app.repo.join("ok.txt")).unwrap(),
        "ok\n"
    );
}

/// Regression: in a team with a shared teammate, the lead's ownership guard lists every
/// other member's `owns`, the worktree teammate's included; `team_merge` of that
/// teammate lands its own `server/**` work, while a branch touching `docs/**` (owned by
/// the shared `docs`) is still denied with `Path owned by teammate docs`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn team_merge_lands_the_merged_teammates_own_paths_but_no_one_elses() {
    let app = ModeApp::start(Setup {
        config: json!({"features": {"teams": true},
            "permissions": {"bash": "allow", "edit": "allow"}}),
        ..Setup::default()
    })
    .await;
    init_repository(&app.repo);
    let (_, body) = app.create("default").await;
    let lead = body["data"]["id"].as_str().unwrap().to_owned();
    app.run(
        &lead,
        &spawn_call(json!([
            {"name": "docs", "isolation": "shared", "owns": ["docs/**"]},
            {"name": "api", "isolation": "worktree", "owns": ["server/**"]}])),
    )
    .await;
    let members = spawned(&last_output(&app, &lead).await);
    let directory = std::path::PathBuf::from(members["api"]["directory"].as_str().unwrap());
    let head = git(&app.repo, &["rev-parse", "HEAD"]);

    commit_in(&directory, &[("docs/api.md", Some("api\n"))], "docs");
    app.run(&lead, &merge_call("api")).await;
    assert_eq!(
        last_output(&app, &lead).await,
        "Path owned by teammate docs"
    );
    assert_eq!(git(&app.repo, &["rev-parse", "HEAD"]), head);
    commit_in(&directory, &[("docs/api.md", None)], "no docs");

    commit_in(
        &directory,
        &[("server/x.rs", Some("fn x() {}\n"))],
        "server",
    );
    app.run(&lead, &merge_call("api")).await;
    let text = last_output(&app, &lead).await;
    let merged: Value = serde_json::from_str(&text).unwrap_or_else(|_| panic!("{text}"));
    assert_eq!(merged["merge"]["status"], "merged", "{merged}");
    assert_eq!(
        std::fs::read_to_string(app.repo.join("server/x.rs")).unwrap(),
        "fn x() {}\n"
    );
    app.app
        .runtime
        .wait_idle(members["docs"]["session_id"].as_str().unwrap())
        .await;
}

/// Regression: after `lead_deleted`, `POST /teams/:id/resume` refuses a new lead in
/// another Location (its checkout would receive merges and the team would vanish from
/// the lead's `GET /teams`) and records nothing; a Session in the team's Location takes
/// over and lists the team.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_new_lead_must_work_in_the_teams_location() {
    let app = ModeApp::start(Setup {
        config: json!({"features": {"teams": true}}),
        ..Setup::default()
    })
    .await;
    let (_, body) = app.create("default").await;
    let lead = body["data"]["id"].as_str().unwrap().to_owned();
    let team = app
        .app
        .runtime
        .team_spawn(
            &lead,
            vec![cyber_server::runtime::teams::TeamSpawn {
                name: "api".into(),
                isolation: Some(cyber_core::teams::Isolation::Shared),
                ..Default::default()
            }],
        )
        .await
        .unwrap();
    let team_id = app
        .app
        .runtime
        .team_membership(&team[0].session_id)
        .unwrap()
        .unwrap()
        .0
        .id;
    let (status, body) = app.send("DELETE", &format!("/sessions/{lead}"), None).await;
    assert!((200..300).contains(&status), "{status} {body}");

    let elsewhere = app.root.join("elsewhere");
    std::fs::create_dir_all(&elsewhere).unwrap();
    let elsewhere: String = elsewhere
        .display()
        .to_string()
        .bytes()
        .map(|byte| format!("%{byte:02X}"))
        .collect();
    let (status, body) = app
        .send(
            "POST",
            &format!("/sessions?location%5Bdirectory%5D={elsewhere}"),
            Some(json!({"mode": "default"})),
        )
        .await;
    assert_eq!(status, 201, "{body}");
    let outsider = body["data"]["id"].as_str().unwrap().to_owned();
    let (status, body) = app
        .send(
            "POST",
            &format!("/teams/{team_id}/resume"),
            Some(json!({ "lead": outsider })),
        )
        .await;
    assert_eq!(
        (status, body["_tag"].as_str()),
        (400, Some("InvalidRequestError")),
        "{body}"
    );
    assert!(
        body["message"]
            .as_str()
            .unwrap()
            .contains("The new lead must work in the team's Location"),
        "{body}"
    );
    assert!(team_records(&app, &outsider, "team.resumed.1").is_empty());
    assert_eq!(
        app.app.runtime.team(&team_id).unwrap().unwrap().status,
        "stopped"
    );

    let (_, body) = app.create("default").await;
    let new_lead = body["data"]["id"].as_str().unwrap().to_owned();
    let (status, body) = app
        .send(
            "POST",
            &format!("/teams/{team_id}/resume"),
            Some(json!({ "lead": new_lead })),
        )
        .await;
    assert_eq!(
        (status, &body["data"]["status"]),
        (200, &json!("active")),
        "{body}"
    );
    let (_, body) = app.send("GET", "/teams", None).await;
    assert!(body.to_string().contains(&team_id), "{body}");
    app.app.runtime.wait_idle(&team[0].session_id).await;
}

/// Regression: a teammate whose worktree setup fails is undone: the spawn fails, no
/// Session titled `adhoc/api` nor its checkout or branch remains, the team counts no such
/// member, and spawning `api` again (setup now passing) succeeds.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_teammate_whose_setup_fails_leaves_nothing_behind() {
    let app = ModeApp::start(Setup {
        // Exits 1 in any shell until the branch `setup-ok` exists.
        config: json!({"features": {"teams": true},
            "permissions": {"bash": "allow", "edit": "allow", "worktree": "allow"},
            "worktrees": {"setup": ["git rev-parse --verify --quiet refs/heads/setup-ok"]}}),
        ..Setup::default()
    })
    .await;
    init_repository(&app.repo);
    let (_, body) = app.create("default").await;
    let lead = body["data"]["id"].as_str().unwrap().to_owned();
    let spawn = spawn_call(json!([{"name": "api", "isolation": "worktree"}]));
    app.run(&lead, &spawn).await;
    let output = last_output(&app, &lead).await;
    assert!(output.contains("Teammate setup command"), "{output}");
    assert!(
        output.contains("the teammate and its checkout were removed"),
        "{output}"
    );
    let (_, sessions) = app.send("GET", "/sessions", None).await;
    assert!(!sessions.to_string().contains("adhoc/api"), "{sessions}");
    let worktrees = git(&app.repo, &["worktree", "list", "--porcelain"]);
    assert!(!worktrees.contains("team-"), "{worktrees}");
    assert_eq!(git(&app.repo, &["branch", "--list", "cyber/team-*"]), "");
    let team = app.app.runtime.team_membership(&lead).unwrap().unwrap().0;
    let names: Vec<String> = app
        .app
        .runtime
        .team_members(&team.id)
        .unwrap()
        .into_iter()
        .map(|m| m.name)
        .collect();
    assert_eq!(names, ["lead"]);

    git(&app.repo, &["branch", "setup-ok"]);
    app.run(&lead, &spawn).await;
    let members = spawned(&last_output(&app, &lead).await);
    assert_eq!(members["api"]["isolation"], "worktree", "{members:?}");
    assert_eq!(app.app.runtime.team_members(&team.id).unwrap().len(), 2);
}

/// Reserved enforcement is accepted at creation; provider bounds are checked at dispatch.
#[tokio::test]
async fn reserved_team_budget_is_captured_without_dispatching_a_provider() {
    let app = ModeApp::start(Setup {
        config: json!({"features":{"teams":true}}),
        ..Default::default()
    })
    .await;
    let (_, body) = app.create("default").await;
    let lead = body["data"]["id"].as_str().unwrap();
    let definition=cyber_core::teams::parse("---\nbudget:\n  max_tokens: 1000\n  enforcement: reserved\nteammates:\n  - name: api\n    agent: build\n---\n",std::path::Path::new("reserved-team.md"),6).unwrap();
    let (team, members) = app
        .app
        .runtime
        .create_team(lead, &definition)
        .await
        .unwrap();
    assert_eq!(members.len(), 1);
    let (stored, _) = app.app.runtime.team_membership(lead).unwrap().unwrap();
    assert_eq!(stored.id, team.id);
    let created = team_records(&app, lead, "team.created.1");
    assert_eq!(created[0]["budget"]["enforcement"], "reserved");
    assert_eq!(created[0]["budget"]["max_tokens"], 1000);
    app.app.runtime.wait_idle(lead).await;
    for member in members {
        app.app.runtime.wait_idle(&member.session_id).await;
    }
    assert!(app.model.requests().is_empty());
}
