//! Workflow Runs through the production App (`workflows` → Background execution on the
//! server, agent() call, Parallel and pipeline helpers; `server-api` → Route groups): the
//! real AppHost and AppServices on a TCP listener with the local password, and a local
//! OpenAI-compatible scripted model. Runs are started over HTTP and execute in the server.

#[path = "support/scripted_model.rs"]
mod scripted_model;

use std::time::Duration;

use cyber_app::{App, AppOptions};
use cyber_core::paths::{DatabaseLocation, Paths};
use reqwest::{Client, Method, StatusCode};
use scripted_model::ScriptedModel;
use serde_json::{Value, json};

const PASSWORD: &str = "workflow-test";

struct Server {
    temp: tempfile::TempDir,
    app: App,
    url: String,
    client: Client,
    stop: tokio::sync::oneshot::Sender<()>,
    server: tokio::task::JoinHandle<std::io::Result<()>>,
}

fn paths(root: &std::path::Path) -> Paths {
    Paths {
        data: root.join("data"),
        config: root.join("config"),
        state: root.join("state"),
        cache: root.join("cache"),
        tmp: root.join("tmp"),
    }
}

impl Server {
    async fn start(global: Value) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let paths = paths(&temp.path().canonicalize().unwrap());
        paths.ensure().unwrap();
        std::fs::write(paths.config.join("cyber.json"), global.to_string()).unwrap();
        Self::open(temp).await
    }

    /// The production application over the database file of `temp`.
    async fn open(temp: tempfile::TempDir) -> Self {
        let directory = temp.path().canonicalize().unwrap();
        let app = App::build(AppOptions {
            paths: paths(&directory),
            home: directory.join("home"),
            database: DatabaseLocation::File(directory.join("data").join("cyber.db")),
            default_directory: directory.clone(),
            sandbox_policy: None,
            snapshots: false,
            interactive: true,
            password: Some(PASSWORD.into()),
            mcp_credentials: None,
            remote_skill_fetcher: None,
        })
        .await
        .unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/api/v1", listener.local_addr().unwrap());
        let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
        let server = tokio::spawn(cyber_server::http::serve_tcp(
            app.router(),
            listener,
            async {
                let _ = stopped.await;
            },
        ));
        Self {
            temp,
            app,
            url,
            client: Client::builder().no_proxy().build().unwrap(),
            stop,
            server,
        }
    }

    /// Stop the server and build a new App over the same files, as a restart does.
    async fn restart(self) -> Self {
        let _ = self.stop.send(());
        let _ = self.server.await;
        self.app.runtime.shutdown().await;
        drop(self.app);
        Self::open(self.temp).await
    }

    /// Build a new App over the same files while this one still executes, as after a
    /// crash: the new App recovers before the old one lets go of its runs, whose work then
    /// ends without a shutdown pause (the runs are already paused).
    async fn crash(self) -> Self {
        let Self {
            temp,
            app,
            stop,
            server,
            ..
        } = self;
        let _ = stop.send(());
        let _ = server.await;
        let restarted = Self::open(temp).await;
        app.runtime.shutdown().await;
        drop(app);
        restarted
    }

    async fn call(&self, method: Method, path: &str, body: Option<Value>) -> (StatusCode, Value) {
        let mut request = self
            .client
            .request(method, format!("{}{path}", self.url))
            .basic_auth("cyber", Some(PASSWORD));
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request.send().await.unwrap();
        let status = response.status();
        let text = response.text().await.unwrap();
        (status, serde_json::from_str(&text).unwrap_or(Value::Null))
    }

    async fn ok(&self, method: Method, path: &str, body: Option<Value>) -> Value {
        let (status, value) = self.call(method, path, body).await;
        assert!(status.is_success(), "{path}: {status} {value}");
        value
    }

    async fn session(&self) -> String {
        self.ok(Method::POST, "/sessions", Some(json!({"title": "Owner"})))
            .await["data"]["id"]
            .as_str()
            .unwrap()
            .to_owned()
    }

    async fn start_run(&self, body: Value) -> String {
        let started = self.ok(Method::POST, "/workflows/runs", Some(body)).await;
        started["data"]["id"].as_str().unwrap().to_owned()
    }

    /// Poll a run until it leaves `running`.
    async fn settled_run(&self, id: &str) -> Value {
        for _ in 0..600 {
            let run = self
                .ok(Method::GET, &format!("/workflows/runs/{id}"), None)
                .await;
            if run["data"]["status"] != "running" {
                return run["data"].clone();
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("run {id} did not settle");
    }

    async fn close(self) {
        let _ = self.stop.send(());
        let _ = self.server.await;
        self.app.runtime.shutdown().await;
    }
}

const SCRIPT: &str = r#"export const meta = { name: 'pair', description: 'Two agents in parallel' };
export default async function () {
  phase('Fan out');
  const [plain, structured] = await parallel([
    () => agent('alpha task', { label: 'plain' }),
    () => agent('beta task', { schema: { type: 'object', required: ['findings'], additionalProperties: false,
      properties: { findings: { type: 'array', items: { type: 'string' } } } } }),
  ]);
  log('done', plain.text);
  return { plain, structured, target: args.target };
}
"#;

/// `alpha` answers in text; a schema-governed child returns its result through the tool.
fn reply(body: &Value) -> Value {
    let text = body.to_string();
    if text.contains("return_result") {
        let arguments = json!({"findings": ["beta finding"]}).to_string();
        return json!({"tool_calls":[{"index":0,"id":"call_result","type":"function",
            "function":{"name":"return_result","arguments":arguments}}]});
    }
    json!({"content": "alpha answer"})
}

#[tokio::test]
async fn an_inline_run_started_over_http_spawns_real_child_sessions() {
    let model = ScriptedModel::start(reply);
    let server = Server::start(model.config()).await;
    let owner = server
        .ok(Method::POST, "/sessions", Some(json!({"title": "Owner"})))
        .await["data"]["id"]
        .as_str()
        .unwrap()
        .to_owned();

    let started = server
        .ok(
            Method::POST,
            "/workflows/runs",
            Some(json!({"session_id": owner, "script": SCRIPT, "args": {"target": "src"}})),
        )
        .await;
    let run_id = started["data"]["id"].as_str().unwrap().to_owned();
    assert!(run_id.starts_with("run_"), "{started}");
    assert_eq!(started["data"]["name"], "pair");
    assert_eq!(started["data"]["session_id"], json!(owner));

    let run = server.settled_run(&run_id).await;
    assert_eq!(run["status"], "completed", "{run}");
    assert_eq!(run["phases"], json!(["Fan out"]));
    let result = &run["result"];
    assert_eq!(result["target"], "src");
    assert_eq!(result["plain"]["text"], "alpha answer", "{result}");
    assert!(
        result["plain"]["tokens"].as_u64().unwrap() >= 110,
        "{result}"
    );
    assert_eq!(result["structured"], json!({"findings": ["beta finding"]}));

    let agents = run["agents_detail"].as_array().unwrap();
    assert_eq!(agents.len(), 2, "{run}");
    for agent in agents {
        assert!(agent["id"].as_str().unwrap().starts_with("agt_"), "{agent}");
        assert_eq!(agent["status"], "completed");
        assert_eq!(agent["phase"], "Fan out");
        let child = agent["session_id"].as_str().unwrap();
        let session = server
            .ok(Method::GET, &format!("/sessions/{child}"), None)
            .await;
        let info = &session["data"];
        assert_eq!(info["workflow_run_id"], json!(run_id), "{session}");
        assert_eq!(info["parent_id"], json!(owner));
        assert_eq!(info["subagent_name"], agent["id"]);
    }
    assert_eq!(
        result["plain"]["session_id"],
        agents.iter().find(|a| a["label"] == "plain").unwrap()["session_id"]
    );

    // Exactly one request per child: a plain answer and a structured result.
    let requests = model.requests();
    assert_eq!(requests.len(), 2, "{requests:?}");
    let texts: Vec<String> = requests.iter().map(Value::to_string).collect();
    assert!(
        texts
            .iter()
            .any(|t| t.contains("alpha task") && !t.contains("return_result"))
    );
    assert!(
        texts
            .iter()
            .any(|t| t.contains("beta task") && t.contains("return_result"))
    );

    let listed = server.ok(Method::GET, "/workflows/runs", None).await;
    assert_eq!(listed["data"][0]["id"], json!(run_id));
    let owned = server
        .ok(
            Method::GET,
            &format!("/workflows/runs?session_id={owner}"),
            None,
        )
        .await;
    assert_eq!(owned["data"].as_array().unwrap().len(), 1);
    server.close().await;
}

#[tokio::test]
async fn script_failures_and_invalid_starts_are_reported() {
    let model = ScriptedModel::start(reply);
    let server = Server::start(model.config()).await;
    let owner = server.ok(Method::POST, "/sessions", Some(json!({}))).await["data"]["id"]
        .as_str()
        .unwrap()
        .to_owned();

    // A script error fails the run with its diagnostic and location.
    let failing = "export const meta = { name: 'bad', description: 'Fails' };\nexport default async () => {\n  await agent('x', { goal: 'done' });\n};\n";
    let started = server
        .ok(
            Method::POST,
            "/workflows/runs",
            Some(json!({"session_id": owner, "script": failing})),
        )
        .await;
    let run = server
        .settled_run(started["data"]["id"].as_str().unwrap())
        .await;
    assert_eq!(run["status"], "failed", "{run}");
    assert_eq!(run["error"]["kind"], "ExecutionFailure");
    assert!(
        run["error"]["message"]
            .as_str()
            .unwrap()
            .contains("option goal is not available"),
        "{run}"
    );
    assert_eq!(run["agents_detail"], json!([]));
    assert!(model.requests().is_empty());

    // An unknown tier rejects the call with ModelUnavailable, which the script can catch.
    let tiers = "export const meta = { name: 'tiers', description: 'Unknown tier' };\nexport default async () => {\n  try { await agent('x', { model: 'galaxy' }); } catch (e) { return e.kind; }\n};\n";
    let started = server
        .ok(
            Method::POST,
            "/workflows/runs",
            Some(json!({"session_id": owner, "script": tiers})),
        )
        .await;
    let run = server
        .settled_run(started["data"]["id"].as_str().unwrap())
        .await;
    assert_eq!(run["result"], "ModelUnavailable", "{run}");

    let (status, body) = server
        .call(
            Method::POST,
            "/workflows/runs",
            Some(json!({"session_id": owner, "script": "export default () => 1;"})),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["diagnostic"]["kind"], "ParseError", "{body}");
    let (status, body) = server
        .call(
            Method::POST,
            "/workflows/runs",
            Some(json!({"session_id": owner, "name": "missing"})),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    let (status, _) = server
        .call(
            Method::POST,
            "/workflows/runs",
            Some(json!({"session_id": "ses_missing", "script": SCRIPT})),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = server
        .call(Method::GET, "/workflows/runs/run_missing", None)
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    server.close().await;
}

#[tokio::test]
async fn workflow_routes_require_authentication_and_name_the_group_when_unavailable() {
    let model = ScriptedModel::start(reply);
    let server = Server::start(model.config()).await;
    let anonymous = server
        .client
        .get(format!("{}/workflows/runs", server.url))
        .send()
        .await
        .unwrap();
    assert_eq!(anonymous.status(), StatusCode::UNAUTHORIZED);
    let anonymous = server
        .client
        .post(format!("{}/workflows/runs", server.url))
        .json(&json!({}))
        .send()
        .await
        .unwrap();
    assert_eq!(anonymous.status(), StatusCode::UNAUTHORIZED);

    for (method, path) in [
        (Method::GET, "/workflows"),
        (Method::GET, "/workflows/saved"),
    ] {
        let (status, body) = server.call(method.clone(), path, None).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{method} {path}");
        assert_eq!(
            body["_tag"], "ServiceUnavailableError",
            "{method} {path}: {body}"
        );
        assert_eq!(body["service"], "workflow", "{method} {path}: {body}");
    }
    for (method, path) in [
        (Method::POST, "/workflows/runs/run_1/pause"),
        (Method::POST, "/workflows/runs/run_1/resume"),
        (Method::POST, "/workflows/runs/run_1/stop"),
        (Method::GET, "/workflows/runs/run_1/logs"),
    ] {
        let (status, body) = server.call(method.clone(), path, None).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{method} {path}: {body}");
        assert_eq!(body["_tag"], "WorkflowRunNotFoundError");
    }
    server.close().await;
}

const TRIO: &str = r#"export const meta = { name: 'trio', description: 'Three agents in sequence' };
export default async function () {
  const one = await agent('agent one task');
  const two = await agent('agent two task');
  log('two done', two.text);
  const three = await agent('agent three task');
  return [one.text, two.text, three.text];
}
"#;

/// Agent three's first request is held open; its continuation after a resume answers.
fn trio_reply(body: &Value) -> Value {
    let text = body.to_string();
    if text.contains("was interrupted") {
        return json!({"content": "three answer"});
    }
    for (task, answer) in [
        ("agent one task", "one answer"),
        ("agent two task", "two answer"),
    ] {
        if text.contains(task) {
            return json!({"content": answer});
        }
    }
    json!({"hold": true})
}

fn requests_with(model: &ScriptedModel, needle: &str) -> usize {
    model
        .requests()
        .iter()
        .filter(|r| r.to_string().contains(needle))
        .count()
}

/// Child Sessions of `owner`, oldest first.
fn children(server: &Server, owner: &str) -> Vec<(String, Option<String>)> {
    let owner = owner.to_owned();
    server
        .app
        .store
        .read(move |db| {
            let mut statement = db.prepare(
                "SELECT id, subagent_name FROM session WHERE parent_id = ?1 ORDER BY created_at, id",
            )?;
            let rows = statement
                .query_map([owner], |row| Ok((row.get(0)?, row.get(1)?)))?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
        .unwrap()
}

async fn wait_for(what: &str, mut done: impl FnMut() -> bool) {
    for _ in 0..600 {
        if done() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("{what} did not happen");
}

#[tokio::test]
async fn a_restart_pauses_the_run_and_resume_replays_settled_agents_and_continues_the_child() {
    let model = ScriptedModel::start(trio_reply);
    let server = Server::start(model.config()).await;
    let owner = server.session().await;
    let run_id = server
        .start_run(json!({"session_id": owner, "script": TRIO}))
        .await;
    wait_for("agent three's request", || {
        requests_with(&model, "agent three task") == 1
    })
    .await;
    let before = children(&server, &owner);
    assert_eq!(before.len(), 3, "{before:?}");
    let third = before[2].clone();

    let server = server.restart().await;
    let run = server
        .ok(Method::GET, &format!("/workflows/runs/{run_id}"), None)
        .await["data"]
        .clone();
    assert_eq!(run["status"], "paused", "{run}");
    let agents = run["agents_detail"].as_array().unwrap();
    assert_eq!(agents.len(), 3, "{run}");
    assert_eq!(agents[0]["status"], "completed");
    assert_eq!(agents[1]["status"], "completed");
    assert_eq!(agents[2]["status"], "interrupted", "{run}");
    assert_eq!(agents[2]["id"].as_str(), third.1.as_deref());

    let resumed = server
        .ok(
            Method::POST,
            &format!("/workflows/runs/{run_id}/resume"),
            None,
        )
        .await;
    assert_eq!(resumed["data"]["status"], "running", "{resumed}");
    let run = server.settled_run(&run_id).await;
    assert_eq!(run["status"], "completed", "{run}");
    assert_eq!(
        run["result"],
        json!(["one answer", "two answer", "three answer"])
    );
    // Agents one and two were served from the store, not the model.
    assert_eq!(requests_with(&model, "agent one task"), 1);
    assert_eq!(requests_with(&model, "agent two task"), 1);
    assert_eq!(requests_with(&model, "was interrupted"), 1);
    // Agent three continued its existing child Session instead of creating another.
    let after = children(&server, &owner);
    assert_eq!(after, before);
    let agents = run["agents_detail"].as_array().unwrap();
    assert_eq!(agents.len(), 3);
    assert_eq!(agents[2]["session_id"], json!(third.0));
    assert_eq!(agents[2]["status"], "completed");
    let logs = server
        .ok(Method::GET, &format!("/workflows/runs/{run_id}/logs"), None)
        .await;
    assert_eq!(logs["data"].as_array().unwrap().len(), 1, "{logs}");
    assert_eq!(logs["data"][0]["values"], json!(["two done", "two answer"]));
    model.release();
    server.close().await;
}

#[tokio::test]
async fn stop_over_http_cancels_the_child_and_resume_reopens_and_continues_it() {
    let model = ScriptedModel::start(trio_reply);
    let server = Server::start(model.config()).await;
    let owner = server.session().await;
    let script = "export const meta = { name: 'solo', description: 'One agent' };\nexport default async () => (await agent('agent three task')).text;\n";
    let run_id = server
        .start_run(json!({"session_id": owner, "script": script}))
        .await;
    wait_for("the held request", || {
        requests_with(&model, "agent three task") == 1
    })
    .await;
    let stopped = server
        .ok(
            Method::POST,
            &format!("/workflows/runs/{run_id}/stop"),
            None,
        )
        .await;
    assert_eq!(stopped["data"]["status"], "stopped", "{stopped}");
    let (status, body) = server
        .call(
            Method::POST,
            &format!("/workflows/runs/{run_id}/pause"),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");

    let mut resumed = None;
    for _ in 0..300 {
        let (status, body) = server
            .call(
                Method::POST,
                &format!("/workflows/runs/{run_id}/resume"),
                None,
            )
            .await;
        if status.is_success() {
            resumed = Some(body);
            break;
        }
        assert_eq!(status, StatusCode::CONFLICT, "{body}");
        assert_eq!(
            body["message"],
            cyber_server::runtime::AGENTS_EXITING,
            "{body}"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(resumed.is_some(), "resume stayed refused");
    let run = server.settled_run(&run_id).await;
    assert_eq!(run["status"], "completed", "{run}");
    assert_eq!(run["result"], "three answer");
    let children = children(&server, &owner);
    assert_eq!(children.len(), 1);
    // The stop closed the child's admission; the resume reopened it with the receipt.
    let kinds: Vec<String> = server
        .app
        .store
        .read_events(&children[0].0, -1, cyber_store::MAX_PAGE_LIMIT)
        .unwrap()
        .events
        .into_iter()
        .map(|event| event.kind)
        .collect();
    let stopped = kinds.iter().position(|k| k == "session.subtree.stopped.1");
    let reopened = kinds
        .iter()
        .position(|k| k == "session.admission.reopened.1");
    assert!(stopped.is_some() && reopened > stopped, "{kinds:?}");
    model.release();
    server.close().await;
}

#[tokio::test]
async fn max_agents_over_http_rejects_the_next_agent_and_ends_budget_exceeded() {
    let model = ScriptedModel::start(reply);
    let server = Server::start(model.config()).await;
    let owner = server.session().await;
    let script = "export const meta = { name: 'capped', description: 'Budget', budget: { max_agents: 5 } };\nexport default async () => {\n  const first = await agent('alpha task');\n  try { await agent('alpha task again'); } catch (e) { return { first: first.text, kind: e.kind, budget: ctx.budget }; }\n};\n";
    let run_id = server
        .start_run(json!({"session_id": owner, "script": script, "budget": {"max_agents": 1}}))
        .await;
    let run = server.settled_run(&run_id).await;
    assert_eq!(run["status"], "budget_exceeded", "{run}");
    assert_eq!(run["budget"]["max_agents"], 1);
    assert_eq!(run["budget"]["max_wall_seconds"], 14400.0);
    assert_eq!(run["result"]["first"], "alpha answer");
    assert_eq!(run["result"]["kind"], "BudgetExceeded");
    assert_eq!(run["result"]["budget"]["agents_started"], 1);
    assert_eq!(model.requests().len(), 1);

    let (status, body) = server
        .call(
            Method::POST,
            "/workflows/runs",
            Some(json!({"session_id": owner, "script": script, "budget": {"enforcement": "reserved"}})),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    server.close().await;
}

/// Each tier's child answers in text; usage reports cost per model and per tier.
#[tokio::test]
async fn run_usage_reports_cost_per_model_and_tier_for_a_mixed_run() {
    let model = ScriptedModel::start(reply);
    let mut config = model.config();
    config["providers"]["test"]["models"] = json!({
        "main": {},
        "small": {"cost": {"input": 1.0, "output": 2.0}},
        "big": {"cost": {"input": 10.0, "output": 20.0}},
    });
    config["model_roles"] = json!({"small": "test/small", "advisor": "test/big"});
    let server = Server::start(config).await;
    let owner = server.session().await;
    let script = "export const meta = { name: 'mixed', description: 'Fast then verify' };\nexport default async () => {\n  await parallel([() => agent('alpha one', { model: 'fast' }), () => agent('alpha two', { model: 'fast' })]);\n  return (await agent('alpha check', { model: 'verify' })).text;\n};\n";
    let run_id = server
        .start_run(json!({"session_id": owner, "script": script}))
        .await;
    let run = server.settled_run(&run_id).await;
    assert_eq!(run["status"], "completed", "{run}");

    let usage = server
        .ok(Method::GET, &format!("/usage?scope=run&id={run_id}"), None)
        .await["data"]
        .clone();
    assert_eq!(usage["scope"], "run");
    assert_eq!(usage["agents"].as_array().unwrap().len(), 3, "{usage}");
    let models = usage["models"].as_array().unwrap();
    assert_eq!(models.len(), 2, "{usage}");
    let cost_of = |key: &str| {
        models.iter().find(|m| m["key"] == key).unwrap()["cost"]
            .as_f64()
            .unwrap()
    };
    let (small, big) = (cost_of("test/small"), cost_of("test/big"));
    assert!(small > 0.0 && big > 0.0, "{usage}");
    // Two cheap agents still cost less than one priced ten times higher.
    assert!((big - 5.0 * small).abs() < 1e-9, "{usage}");
    let tiers = usage["tiers"].as_array().unwrap();
    assert_eq!(tiers[0]["key"], "fast");
    assert_eq!(tiers[0]["agents"], 2);
    assert_eq!(tiers[1]["key"], "verify");
    assert!((usage["cost"].as_f64().unwrap() - (small + big)).abs() < 1e-9);
    server.close().await;
}

const TOOLED: &str = "export const meta = { name: 'tooled', description: 'Started by the model', phases: [{ title: 'Greet' }] };\nexport default async () => {\n  phase('Greet');\n  const child = await agent('child task');\n  return { text: child.text, who: args.who };\n};\n";

const BLOCKED: &str = "export const meta = { name: 'blocked', description: 'Denied by a rule' };\nexport default async () => (await agent('child task')).text;\n";

/// Its second agent exceeds `max_agents: 1`; the partial result is over 1 MiB.
const CAPPED: &str = "export const meta = { name: 'capped', description: 'Partial, then complete' };\nexport default async () => {\n  await agent('child task');\n  try { await agent('second child task'); } catch (e) { return 'x'.repeat(1100000); }\n  return 'small';\n};\n";

const ASKING: &str = "export const meta = { name: 'asking', description: 'An agent that needs approval' };\nexport default async () => (await agent('run a command')).text;\n";

fn last_message(body: &Value) -> Value {
    body["messages"]
        .as_array()
        .and_then(|messages| messages.last())
        .cloned()
        .unwrap_or_default()
}

fn workflow_call(script: &str, args: Value) -> Value {
    workflow_call_with(script, args, json!({"max_agents": 3}))
}

fn workflow_call_with(script: &str, args: Value, budget: Value) -> Value {
    workflow_call_as("call_workflow", script, args, budget)
}

/// A `workflow` tool call with its own call ID, for a second call in one Session.
fn workflow_call_as(id: &str, script: &str, args: Value, budget: Value) -> Value {
    let arguments = json!({"script": script, "args": args, "budget": budget}).to_string();
    json!({"tool_calls":[{"index":0,"id":id,"type":"function",
        "function":{"name":"workflow","arguments":arguments}}]})
}

/// The owner starts a workflow through the tool and answers its handback; children answer
/// their task, and a child asked to run a command calls `bash`.
fn session_reply(body: &Value) -> Value {
    let last = last_message(body);
    let text = last.to_string();
    if last["role"] == "tool" {
        return json!({"content": format!("tool said: {}", last["content"])});
    }
    if text.contains("Workflow run handback") {
        return json!({"content": "handback received"});
    }
    if text.contains("start the blocked workflow") {
        return workflow_call(BLOCKED, json!({}));
    }
    if text.contains("start the widened workflow") {
        return workflow_call_as(
            "call_widened",
            TOOLED,
            json!({}),
            json!({"max_cost_usd": 50.0, "max_agents": 1_000_000, "max_wall_seconds": 1e9}),
        );
    }
    if text.contains("start the unlimited workflow") {
        return workflow_call_with(TOOLED, json!({}), json!({"max_cost_usd": null}));
    }
    if text.contains("start the shared workflow") {
        let arguments = json!({"script": TOOLED, "args": {}, "max_concurrent": 1}).to_string();
        return json!({"tool_calls":[{"index":0,"id":"call_shared","type":"function",
            "function":{"name":"workflow","arguments":arguments}}]});
    }
    if text.contains("start the capped workflow") {
        return workflow_call_with(CAPPED, json!({}), json!({"max_agents": 1}));
    }
    if text.contains("start the workflow") {
        return workflow_call(TOOLED, json!({"who": "model"}));
    }
    if text.contains("run a command") {
        let arguments =
            json!({"command": "echo approved", "description": "Echo a word"}).to_string();
        return json!({"tool_calls":[{"index":0,"id":"call_bash","type":"function",
            "function":{"name":"bash","arguments":arguments}}]});
    }
    json!({"content": "child answer"})
}

impl Server {
    async fn prompt(&self, session: &str, text: &str) {
        self.ok(
            Method::POST,
            &format!("/sessions/{session}/prompt"),
            Some(json!({"parts": [{"type": "text", "text": text}]})),
        )
        .await;
    }

    async fn idle(&self, session: &str) {
        for _ in 0..600 {
            if !self.app.runtime.is_running(session) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!("{session} did not go idle");
    }
}

#[tokio::test]
async fn the_workflow_tool_asks_with_name_phases_and_budget_and_hands_back_once() {
    let model = ScriptedModel::start(session_reply);
    let server = Server::start(model.config()).await;
    let owner = server.session().await;
    server.prompt(&owner, "please start the workflow").await;

    // Default Mode asks before any agent starts, showing name, phases and budgets.
    let request = loop {
        let pending = server
            .ok(
                Method::GET,
                &format!("/permissions/requests?session_id={owner}"),
                None,
            )
            .await;
        if let Some(request) = pending["data"].as_array().and_then(|r| r.first()).cloned() {
            break request;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    assert_eq!(request["action"], "workflow.run", "{request}");
    // An inline script's resource names its origin and digest, not just its name.
    let resource = request["resources"][0].as_str().unwrap();
    assert!(resource.starts_with("inline:tooled@sha256:"), "{request}");
    let shown = &request["metadata"];
    assert_eq!(shown["name"], "tooled");
    assert_eq!(shown["phases"], json!(["Greet"]));
    assert_eq!(shown["budget"]["max_agents"], 3, "{shown}");
    assert_eq!(shown["origin"], "inline");
    let runs = server
        .ok(
            Method::GET,
            &format!("/workflows/runs?session_id={owner}"),
            None,
        )
        .await;
    assert_eq!(runs["data"], json!([]), "nothing starts before approval");
    server
        .call(
            Method::POST,
            &format!(
                "/sessions/{owner}/permissions/{}/reply",
                request["id"].as_str().unwrap()
            ),
            Some(json!({"reply": "once"})),
        )
        .await;

    let run_id = loop {
        let runs = server
            .ok(
                Method::GET,
                &format!("/workflows/runs?session_id={owner}"),
                None,
            )
            .await;
        if let Some(id) = runs["data"][0]["id"].as_str() {
            break id.to_owned();
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    let run = server.settled_run(&run_id).await;
    assert_eq!(run["status"], "completed", "{run}");
    assert_eq!(
        run["result"],
        json!({"text": "child answer", "who": "model"})
    );
    assert_eq!(run["continue_owner"], true);
    assert_eq!(run["interactive"], true);
    assert_eq!(run["budget"]["max_agents"], 3);
    // The inline script is kept under the state directory.
    let script = std::path::PathBuf::from(run["script_file"].as_str().unwrap());
    let state = server.temp.path().canonicalize().unwrap().join("state");
    assert_eq!(script, state.join("workflows").join(format!("{run_id}.js")));
    assert_eq!(std::fs::read_to_string(&script).unwrap(), TOOLED);

    // The owner receives one queued summary and continues on it.
    wait_for("the owner's answer to the handback", || {
        model.requests().iter().any(|body| {
            last_message(body)
                .to_string()
                .contains("Workflow run handback")
        })
    })
    .await;
    server.idle(&owner).await;
    let message = cyber_server::runtime::handback_message_id(&run_id);
    let state = server.app.runtime.state(&owner).await.unwrap();
    let delivered: Vec<_> = state
        .entries
        .iter()
        .filter(|entry| entry.id() == message)
        .collect();
    assert_eq!(delivered.len(), 1, "{:?}", state.entries);
    let transcript = format!("{:?}", state.entries);
    assert!(transcript.contains(&run_id), "{transcript}");
    assert!(transcript.contains("handback received"), "{transcript}");
    assert!(
        server
            .ok(Method::GET, &format!("/workflows/runs/{run_id}"), None)
            .await["data"]["notified"]
            == true
    );
    server.close().await;
}

#[tokio::test]
async fn a_denied_workflow_run_starts_nothing_even_in_bypass() {
    let model = ScriptedModel::start(session_reply);
    let mut config = model.config();
    config["permissions"] = json!({"workflow.run": {"blocked": "deny"}});
    let server = Server::start(config).await;
    let owner = server
        .ok(Method::POST, "/sessions", Some(json!({"mode": "bypass"})))
        .await["data"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    server
        .prompt(&owner, "please start the blocked workflow")
        .await;
    wait_for("the denied tool result", || {
        model
            .requests()
            .iter()
            .any(|body| last_message(body)["role"] == "tool")
    })
    .await;
    server.idle(&owner).await;
    let denied = model
        .requests()
        .iter()
        .map(last_message)
        .find(|message| message["role"] == "tool")
        .unwrap();
    assert!(
        denied["content"].to_string().contains("Permission denied"),
        "{denied}"
    );
    let runs = server.ok(Method::GET, "/workflows/runs", None).await;
    assert_eq!(runs["data"], json!([]));
    assert!(!server.temp.path().join("state/workflows").exists());
    server.close().await;
}

/// SSE `data:` payloads received so far.
fn sse_events(buffer: &str) -> Vec<Value> {
    buffer
        .lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .filter_map(|data| serde_json::from_str(data).ok())
        .collect()
}

#[tokio::test]
async fn api_runs_reject_agent_asks_and_stream_run_events_for_their_location() {
    use futures::StreamExt;
    let model = ScriptedModel::start(session_reply);
    let server = Server::start(model.config()).await;
    let owner = server.session().await;
    let response = server
        .client
        .get(format!("{}/event", server.url))
        .basic_auth("cyber", Some(PASSWORD))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let mut stream = response.bytes_stream();
    let run_id = server
        .start_run(json!({"session_id": owner, "script": ASKING}))
        .await;
    let run = server.settled_run(&run_id).await;
    // A run started over the API has no approver: the agent's ask is rejected at once.
    assert_eq!(run["status"], "completed", "{run}");
    assert_eq!(run["interactive"], false);
    let answer = run["result"].as_str().unwrap();
    assert!(
        answer.contains("No interactive approver for workflow run"),
        "{answer}"
    );
    assert!(
        server.ok(Method::GET, "/permissions/requests", None).await["data"]
            .as_array()
            .unwrap()
            .is_empty()
    );

    let mut buffer = String::new();
    let directory = std::path::PathBuf::from(run["directory"].as_str().unwrap());
    let ended = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let events = sse_events(&buffer);
            if events
                .iter()
                .any(|event| event["type"] == "workflow.run.completed.1")
            {
                return events;
            }
            let chunk = stream.next().await.unwrap().unwrap();
            buffer.push_str(&String::from_utf8_lossy(&chunk));
        }
    })
    .await
    .expect("run events on the instance stream");
    let run_events: Vec<&Value> = ended
        .iter()
        .filter(|event| {
            event["type"]
                .as_str()
                .unwrap_or_default()
                .starts_with("workflow.")
        })
        .collect();
    let kinds: Vec<&str> = run_events
        .iter()
        .map(|event| event["type"].as_str().unwrap())
        .collect();
    for kind in [
        "workflow.run.started.1",
        "workflow.agent.started.1",
        "workflow.agent.settled.1",
        "workflow.run.completed.1",
    ] {
        assert!(kinds.contains(&kind), "{kind}: {kinds:?}");
    }
    for event in run_events {
        // The live progress event carries the run in its data, not a durable reference.
        if event["type"] == "workflow.run.progress" {
            assert_eq!(event["data"]["run_id"], json!(run_id), "{event}");
            assert!(event["durable"].is_null(), "{event}");
        } else {
            assert_eq!(event["durable"]["aggregateID"], json!(run_id), "{event}");
        }
        let location = std::path::PathBuf::from(event["location"].as_str().unwrap());
        assert_eq!(
            location.canonicalize().unwrap(),
            directory.canonicalize().unwrap()
        );
    }
    server.close().await;
}

// The recording hook is a POSIX shell command.
#[cfg(unix)]
#[tokio::test]
async fn workflow_run_hooks_fire_with_the_run_in_the_envelope() {
    let model = ScriptedModel::start(session_reply);
    let mut config = model.config();
    let record = "cat >> workflow-hooks.jsonl; echo >> workflow-hooks.jsonl";
    let hook = json!([{"hooks": [{"type": "command", "command": record}]}]);
    let tool = json!([{"matcher": "bash", "hooks": [{"type": "command", "command": record}]}]);
    config["hooks"] = json!({"WorkflowRunStart": hook, "WorkflowRunEnd": hook, "PreToolUse": tool, "SubagentStart": hook});
    let server = Server::start(config).await;
    let owner = server.session().await;
    let run_id = server
        .start_run(json!({"session_id": owner, "script": TOOLED, "args": {"who": "api"}}))
        .await;
    let run = server.settled_run(&run_id).await;
    assert_eq!(run["status"], "completed", "{run}");
    let file = server
        .temp
        .path()
        .canonicalize()
        .unwrap()
        .join("workflow-hooks.jsonl");
    let read = || -> Vec<Value> {
        std::fs::read_to_string(&file)
            .unwrap_or_default()
            .lines()
            .filter(|line| !line.trim().is_empty())
            .filter_map(|line| serde_json::from_str(line).ok())
            .collect()
    };
    wait_for("both run hooks", || read().len() == 3).await;
    let (events, starts): (Vec<Value>, Vec<Value>) = read()
        .into_iter()
        .partition(|event| event["event"] != "SubagentStart");
    // The agent's SubagentStart on the owner carries the run in its envelope.
    assert_eq!(starts.len(), 1);
    assert_eq!(starts[0]["workflow_run_id"], json!(run_id), "{}", starts[0]);
    assert_eq!(events[0]["event"], "WorkflowRunStart");
    assert_eq!(events[1]["event"], "WorkflowRunEnd");
    for event in &events {
        assert_eq!(event["workflow_run_id"], json!(run_id), "{event}");
        assert_eq!(event["session_id"], json!(owner));
        assert_eq!(event["workflow"]["name"], "tooled");
    }
    assert_eq!(events[0]["workflow"]["status"], "running");
    assert_eq!(events[1]["workflow"]["status"], "completed");

    // A workflow agent's own tool hooks carry the run in their envelope.
    let asking = server
        .start_run(json!({"session_id": owner, "script": ASKING}))
        .await;
    server.settled_run(&asking).await;
    wait_for("the agent's tool hook and the second run's hooks", || {
        read().len() == 7
    })
    .await;
    let tool = read()
        .into_iter()
        .find(|event| event["event"] == "PreToolUse")
        .unwrap();
    assert_eq!(tool["workflow_run_id"], json!(asking), "{tool}");
    assert_eq!(tool["parent_session_id"], json!(owner));
    assert_eq!(tool["tool_name"], "bash");
    server.close().await;
}

/// Resume once the previous attempt let go of its agents; `None` sends no body.
async fn resume_when_quiet(
    server: &Server,
    run_id: &str,
    body: impl Into<Option<Value>>,
) -> (StatusCode, Value) {
    let body = body.into();
    for _ in 0..300 {
        let (status, value) = server
            .call(
                Method::POST,
                &format!("/workflows/runs/{run_id}/resume"),
                body.clone(),
            )
            .await;
        if value["message"] != cyber_server::runtime::AGENTS_EXITING {
            return (status, value);
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("resume stayed refused");
}

#[tokio::test]
async fn a_tool_started_run_that_is_resumed_hands_back_each_end_and_drops_a_stale_result_file() {
    let model = ScriptedModel::start(session_reply);
    let server = Server::start(model.config()).await;
    let owner = server
        .ok(Method::POST, "/sessions", Some(json!({"mode": "bypass"})))
        .await["data"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    server
        .prompt(&owner, "please start the capped workflow")
        .await;
    let run_id = loop {
        let runs = server
            .ok(
                Method::GET,
                &format!("/workflows/runs?session_id={owner}"),
                None,
            )
            .await;
        if let Some(id) = runs["data"][0]["id"].as_str() {
            break id.to_owned();
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    let run = server.settled_run(&run_id).await;
    assert_eq!(run["status"], "budget_exceeded", "{run}");
    assert!(run["result_file"].is_string(), "{run}");
    let handbacks = || {
        model
            .requests()
            .iter()
            .filter(|body| {
                last_message(body)
                    .to_string()
                    .contains("Workflow run handback")
            })
            .count()
    };
    wait_for("the first handback", || handbacks() == 1).await;
    server.idle(&owner).await;

    let (status, resumed) =
        resume_when_quiet(&server, &run_id, json!({"budget": {"max_agents": 3}})).await;
    assert!(status.is_success(), "{resumed}");
    let run = server.settled_run(&run_id).await;
    assert_eq!(run["status"], "completed", "{run}");
    assert_eq!(run["result"], "small");
    assert!(run.get("result_file").is_none(), "{run}");
    assert!(run.get("result_preview").is_none(), "{run}");
    // The owner is woken by the second summary too.
    wait_for("the second handback", || handbacks() == 2).await;
    server.idle(&owner).await;
    let second = cyber_server::runtime::attempt_handback_message_id(&run_id, 1);
    let state = server.app.runtime.state(&owner).await.unwrap();
    assert!(
        state.entries.iter().any(|entry| entry.id() == second),
        "{:?}",
        state.entries
    );
    assert!(
        server
            .ok(Method::GET, &format!("/workflows/runs/{run_id}"), None)
            .await["data"]["notified"]
            == true
    );
    server.close().await;
}

#[tokio::test]
async fn http_starts_and_resumes_honour_workflow_run_deny_rules() {
    let model = ScriptedModel::start(trio_reply);
    let mut config = model.config();
    config["permissions"] = json!({"workflow.run": {"blocked": "deny"}});
    let server = Server::start(config.clone()).await;
    let owner = server.session().await;
    let (status, body) = server
        .call(
            Method::POST,
            "/workflows/runs",
            Some(json!({"session_id": owner, "script": BLOCKED})),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(body["reason"], "PermissionDenied", "{body}");
    let runs = server.ok(Method::GET, "/workflows/runs", None).await;
    assert_eq!(runs["data"], json!([]));
    assert!(model.requests().is_empty());

    // A rule added after the start refuses the resume.
    let script = "export const meta = { name: 'solo', description: 'One agent' };\nexport default async () => (await agent('agent three task')).text;\n";
    let run_id = server
        .start_run(json!({"session_id": owner, "script": script}))
        .await;
    wait_for("the held request", || {
        requests_with(&model, "agent three task") == 1
    })
    .await;
    server
        .ok(
            Method::POST,
            &format!("/workflows/runs/{run_id}/stop"),
            None,
        )
        .await;
    config["permissions"] = json!({"workflow.run": {"solo": "deny"}});
    let global = server
        .temp
        .path()
        .canonicalize()
        .unwrap()
        .join("config/cyber.json");
    std::fs::write(global, config.to_string()).unwrap();
    let (status, body) = resume_when_quiet(&server, &run_id, json!({})).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    let run = server
        .ok(Method::GET, &format!("/workflows/runs/{run_id}"), None)
        .await;
    assert_eq!(run["data"]["status"], "stopped");
    model.release();
    server.close().await;
}

#[tokio::test]
async fn a_resumed_run_keeps_its_starting_mode_as_the_agents_ceiling() {
    let model = ScriptedModel::start(trio_reply);
    let server = Server::start(model.config()).await;
    let owner = server
        .ok(Method::POST, "/sessions", Some(json!({"mode": "plan"})))
        .await["data"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let script = "export const meta = { name: 'ceiling', description: 'Mode' };\nexport default async () => {\n  const held = await agent('agent three task');\n  const next = await agent('agent one task');\n  return [held.text, next.text];\n};\n";
    let run_id = server
        .start_run(json!({"session_id": owner, "script": script}))
        .await;
    wait_for("the held request", || {
        requests_with(&model, "agent three task") == 1
    })
    .await;
    server
        .ok(
            Method::POST,
            &format!("/workflows/runs/{run_id}/stop"),
            None,
        )
        .await;
    // The owner widens its Mode for unrelated work; the run keeps `plan`.
    server
        .ok(
            Method::POST,
            &format!("/sessions/{owner}/mode"),
            Some(json!({"mode": "bypass"})),
        )
        .await;
    let (status, body) = resume_when_quiet(&server, &run_id, json!({})).await;
    assert!(status.is_success(), "{body}");
    let run = server.settled_run(&run_id).await;
    assert_eq!(run["status"], "completed", "{run}");
    assert_eq!(run["result"], json!(["three answer", "one answer"]));
    let children = children(&server, &owner);
    assert_eq!(children.len(), 2, "{children:?}");
    for (child, _) in children {
        let session = server
            .ok(Method::GET, &format!("/sessions/{child}"), None)
            .await;
        assert_eq!(session["data"]["mode"], "plan", "{session}");
    }
    model.release();
    server.close().await;
}

#[tokio::test]
async fn the_workflow_tool_cannot_lift_configured_run_budgets() {
    let model = ScriptedModel::start(session_reply);
    let mut config = model.config();
    config["budgets"] = json!({"run": {"max_cost_usd": 1.0, "max_agents": 2}});
    let server = Server::start(config).await;
    let owner = server
        .ok(Method::POST, "/sessions", Some(json!({"mode": "bypass"})))
        .await["data"]["id"]
        .as_str()
        .unwrap()
        .to_owned();

    // `null` would remove the configured cap: the tool refuses and nothing starts.
    server
        .prompt(&owner, "please start the unlimited workflow")
        .await;
    wait_for("the refused tool result", || {
        model
            .requests()
            .iter()
            .any(|body| last_message(body)["role"] == "tool")
    })
    .await;
    server.idle(&owner).await;
    let refused = model
        .requests()
        .iter()
        .map(last_message)
        .find(|message| message["role"] == "tool")
        .unwrap();
    assert!(
        refused["content"].to_string().contains("cannot be removed"),
        "{refused}"
    );
    assert_eq!(
        server.ok(Method::GET, "/workflows/runs", None).await["data"],
        json!([])
    );

    // Larger values keep the configured caps and the default wall-clock ceiling.
    server
        .prompt(&owner, "please start the widened workflow")
        .await;
    let mut run_id = None;
    for _ in 0..600 {
        let runs = server.ok(Method::GET, "/workflows/runs", None).await;
        if let Some(id) = runs["data"][0]["id"].as_str() {
            run_id = Some(id.to_owned());
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let run = server
        .settled_run(&run_id.expect("the widened workflow started"))
        .await;
    assert_eq!(run["status"], "completed", "{run}");
    assert_eq!(run["budget"]["max_cost_usd"], 1.0, "{run}");
    assert_eq!(run["budget"]["max_agents"], 2, "{run}");
    assert_eq!(run["budget"]["max_wall_seconds"], 14400.0, "{run}");
    server.idle(&owner).await;

    // The authenticated client may still raise a limit explicitly.
    let raised = server
        .ok(
            Method::POST,
            "/workflows/runs",
            Some(json!({"session_id": owner, "script": TOOLED, "budget": {"max_cost_usd": 3.0}})),
        )
        .await;
    assert_eq!(raised["data"]["budget"]["max_cost_usd"], 3.0, "{raised}");
    let raised = server
        .settled_run(raised["data"]["id"].as_str().unwrap())
        .await;
    assert_eq!(raised["status"], "completed", "{raised}");
    server.close().await;
}

#[tokio::test]
async fn a_workflow_agent_session_cannot_own_a_run_over_http() {
    let model = ScriptedModel::start(reply);
    let server = Server::start(model.config()).await;
    let owner = server.session().await;
    let script = "export const meta = { name: 'solo', description: 'One agent' };\nexport default async () => (await agent('alpha task')).text;\n";
    let run_id = server
        .start_run(json!({"session_id": owner, "script": script}))
        .await;
    let run = server.settled_run(&run_id).await;
    assert_eq!(run["status"], "completed", "{run}");
    let child = run["agents_detail"][0]["session_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let grandchild = server
        .ok(
            Method::POST,
            "/sessions",
            Some(json!({"parent_id": child, "title": "Below the agent"})),
        )
        .await["data"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    for session in [&child, &grandchild] {
        let (status, body) = server
            .call(
                Method::POST,
                "/workflows/runs",
                Some(json!({"session_id": session, "script": script})),
            )
            .await;
        assert_eq!(status, StatusCode::CONFLICT, "{session}: {body}");
        assert_eq!(
            body["message"],
            cyber_server::runtime::WORKFLOW_AGENT_STARTS_RUN,
            "{body}"
        );
    }
    let runs = server.ok(Method::GET, "/workflows/runs", None).await;
    assert_eq!(runs["data"].as_array().unwrap().len(), 1, "{runs}");
    server.close().await;
}

#[tokio::test]
async fn an_edited_script_file_relaunches_as_a_new_revision() {
    let model = ScriptedModel::start(trio_reply);
    let server = Server::start(model.config()).await;
    let owner = server.session().await;
    let script = "export const meta = { name: 'solo', description: 'One agent' };\nexport default async () => (await agent('agent three task')).text;\n";
    let started = server
        .ok(
            Method::POST,
            "/workflows/runs",
            Some(json!({"session_id": owner, "script": script})),
        )
        .await["data"]
        .clone();
    let run_id = started["id"].as_str().unwrap().to_owned();
    wait_for("the held request", || {
        requests_with(&model, "agent three task") == 1
    })
    .await;
    server
        .ok(
            Method::POST,
            &format!("/workflows/runs/{run_id}/stop"),
            None,
        )
        .await;

    // The user edits the kept copy of the inline script.
    let file = std::path::PathBuf::from(started["script_file"].as_str().unwrap());
    let edited = "export const meta = { name: 'solo', description: 'One agent' };\nexport default async () => ({ edited: true, run: ctx.run_id });\n";
    std::fs::write(&file, edited).unwrap();
    let (status, body) = resume_when_quiet(&server, &run_id, json!({})).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["reason"], "WorkflowReplayMismatch", "{body}");

    let (status, body) = resume_when_quiet(&server, &run_id, json!({"revision": true})).await;
    assert!(status.is_success(), "{body}");
    assert_eq!(body["data"]["revision"], 2, "{body}");
    assert_ne!(body["data"]["digest"], started["digest"], "{body}");
    let run = server.settled_run(&run_id).await;
    assert_eq!(run["status"], "completed", "{run}");
    assert_eq!(run["result"], json!({"edited": true, "run": run_id}));
    assert_eq!(requests_with(&model, "agent three task"), 1);
    model.release();
    server.close().await;
}

/// A fan-out of gated agents labelled `<prefix><n>`, `count` of them.
fn gated_fan_out(name: &str, prefix: &str, count: usize) -> String {
    let calls: Vec<String> = (1..=count).map(|n| format!("'GATE-{prefix}{n}'")).collect();
    format!(
        "export const meta = {{ name: '{name}', description: 'Gated fan-out' }};\nexport default () => parallel([{}].map((p) => () => agent(p)));\n",
        calls.join(", ")
    )
}

/// Each child's request waits at the provider gate named by its task.
fn gated_reply(body: &Value) -> Value {
    let text = last_message(body).to_string();
    let Some(at) = text.find("GATE-") else {
        return json!({"content": "child answer"});
    };
    let label: String = text[at + 5..]
        .chars()
        .take_while(char::is_ascii_alphanumeric)
        .collect();
    json!({ "gate": label })
}

/// Labels of `prefix` waiting at the gate.
fn held_of(model: &ScriptedModel, prefix: char) -> usize {
    model
        .held()
        .iter()
        .filter(|l| l.starts_with(prefix))
        .count()
}

/// Fair agent scheduling on the production App: a run started over HTTP with
/// `max_concurrent: 1` never runs two agents, a second run uses the remaining slot, and
/// freed slots alternate between the runs; the provider's start log shows the order.
#[tokio::test]
async fn runs_share_the_agent_limit_fairly_and_a_share_caps_its_run() {
    let model = ScriptedModel::start(gated_reply);
    let mut config = model.config();
    config["workflows"] = json!({"max_concurrent": 2});
    let server = Server::start(config).await;
    let owner = server.session().await;
    let a = server
        .start_run(json!({"session_id": owner, "script": gated_fan_out("ra", "a", 3), "max_concurrent": 1}))
        .await;
    wait_for("a1 at the gate", || model.held() == ["a1"]).await;
    let shown = server
        .ok(Method::GET, &format!("/workflows/runs/{a}"), None)
        .await;
    assert_eq!(shown["data"]["share"], 1, "{shown}");
    // The only other run takes the free slot.
    let b = server
        .start_run(json!({"session_id": owner, "script": gated_fan_out("rb", "b", 3)}))
        .await;
    wait_for("b1 at the gate", || model.held() == ["a1", "b1"]).await;
    let shown = server
        .ok(Method::GET, &format!("/workflows/runs/{b}"), None)
        .await;
    assert_eq!(shown["data"]["share"], 2, "{shown}");
    for expected in [
        vec!["b1", "a2"],
        vec!["a2", "b2"],
        vec!["b2", "a3"],
        vec!["a3", "b3"],
    ] {
        model.open_gate();
        wait_for("the next agent at the gate", || model.held() == expected).await;
        assert!(held_of(&model, 'a') <= 1, "{:?}", model.held());
    }
    model.open_gate();
    model.open_gate();
    for run in [&a, &b] {
        assert_eq!(server.settled_run(run).await["status"], "completed");
    }
    let order: Vec<String> = model.starts().into_iter().map(|(label, _)| label).collect();
    assert_eq!(order, ["a1", "b1", "a2", "b2", "a3", "b3"]);
    server.close().await;
}

/// A share must be a positive integer: `400` from the API, `ExecutionFailure` from `meta`.
/// A resume may change it without a new revision.
#[tokio::test]
async fn a_bad_share_is_refused_and_a_resume_changes_the_share_without_a_revision() {
    let model = ScriptedModel::start(gated_reply);
    let server = Server::start(model.config()).await;
    let owner = server.session().await;
    let script = gated_fan_out("shared", "c", 1);
    for bad in [json!(0), json!(-2), json!("4")] {
        let (status, body) = server
            .call(
                Method::POST,
                "/workflows/runs",
                Some(json!({"session_id": owner, "script": script, "max_concurrent": bad})),
            )
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{bad}: {body}");
    }
    let declared = script.replace(
        "description: 'Gated fan-out'",
        "description: 'x', max_concurrent: 0",
    );
    let (status, body) = server
        .call(
            Method::POST,
            "/workflows/runs",
            Some(json!({"session_id": owner, "script": declared})),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["reason"], "ExecutionFailure", "{body}");
    let runs = server.ok(Method::GET, "/workflows/runs", None).await;
    assert_eq!(runs["data"], json!([]), "no run starts");

    let meta = "export const meta = { name: 'shared', description: 'x', max_concurrent: 3 };\nexport default async () => { await agent('GATE-c1'); return (await agent('GATE-c2')).text; };\n";
    let run = server
        .start_run(json!({"session_id": owner, "script": meta}))
        .await;
    wait_for("c1 at the gate", || model.held() == ["c1"]).await;
    let shown = server
        .ok(Method::GET, &format!("/workflows/runs/{run}"), None)
        .await;
    let digest = shown["data"]["config_digest"].clone();
    server
        .ok(Method::POST, &format!("/workflows/runs/{run}/pause"), None)
        .await;
    model.open_gate();
    let (status, body) = resume_when_quiet(&server, &run, json!({"max_concurrent": 1})).await;
    assert!(status.is_success(), "{body}");
    assert_eq!(body["data"]["share"], 1, "{body}");
    assert_eq!(body["data"]["revision"], 1, "{body}");
    assert_eq!(body["data"]["config_digest"], digest, "{body}");
    wait_for("c2 at the gate", || model.held() == ["c2"]).await;
    model.open_gate();
    let ended = server.settled_run(&run).await;
    assert_eq!(ended["status"], "completed", "{ended}");
    assert_eq!(ended["share"], 1, "{ended}");
    server.close().await;
}

/// A resume that gives no share keeps the one the run was started with, as the CLI, the
/// TUI and the SDK resume (regression: it fell back to the whole limit); a share given on
/// a resume is kept by later ones, including resuming the runs a restart interrupted.
#[tokio::test]
async fn a_resume_without_a_share_keeps_the_runs_share() {
    let model = ScriptedModel::start(gated_reply);
    let mut config = model.config();
    config["workflows"] = json!({"max_concurrent": 2});
    let server = Server::start(config).await;
    let owner = server.session().await;
    let run = server
        .start_run(json!({"session_id": owner, "script": gated_fan_out("capped", "s", 6), "max_concurrent": 1}))
        .await;
    wait_for("s1 at the gate", || model.held() == ["s1"]).await;
    let pause = format!("/workflows/runs/{run}/pause");
    server.ok(Method::POST, &pause, None).await;
    model.open_gate();
    for body in [None, Some(json!({})), Some(json!({"max_concurrent": null}))] {
        let (status, resumed) = resume_when_quiet(&server, &run, body.clone()).await;
        assert!(status.is_success(), "{body:?}: {resumed}");
        assert_eq!(resumed["data"]["share"], 1, "{body:?}: {resumed}");
        let next = model.starts().len();
        wait_for("the next agent at the gate", || model.starts().len() > next).await;
        // The scheduler grants the run one slot only, though the limit has two.
        tokio::time::sleep(Duration::from_millis(150)).await;
        assert_eq!(model.held().len(), 1, "{:?}", model.held());
        server.ok(Method::POST, &pause, None).await;
        model.open_gate();
    }
    // A share given on a resume is the one later resumes keep: the restart interrupts the
    // run, and resuming the interrupted runs relaunches it with that share.
    let (status, resumed) = resume_when_quiet(&server, &run, json!({"max_concurrent": 2})).await;
    assert!(status.is_success(), "{resumed}");
    wait_for("s5 and s6 at the gate", || model.held() == ["s5", "s6"]).await;
    let server = server.restart().await;
    let report = server
        .ok(Method::POST, "/workflows/runs/resume_interrupted", None)
        .await;
    assert_eq!(report["data"]["resumed"], json!([run]), "{report}");
    let shown = server
        .ok(Method::GET, &format!("/workflows/runs/{run}"), None)
        .await;
    assert_eq!(shown["data"]["share"], 2, "{shown}");
    server.ok(Method::POST, &pause, None).await;
    let (status, resumed) = resume_when_quiet(&server, &run, None).await;
    assert!(status.is_success(), "{resumed}");
    assert_eq!(resumed["data"]["share"], 2, "{resumed}");
    // The requests the restart abandoned are still first at the gate.
    for _ in 0..4 {
        model.open_gate();
    }
    let ended = server.settled_run(&run).await;
    assert_eq!(ended["status"], "completed", "{ended}");
    assert_eq!(ended["share"], 2, "{ended}");
    server.close().await;
}

/// The model's `workflow` tool accepts `max_concurrent` and records it as the run's share.
#[tokio::test]
async fn the_workflow_tool_records_its_max_concurrent_as_the_share() {
    let model = ScriptedModel::start(session_reply);
    let server = Server::start(model.config()).await;
    let owner = server
        .ok(Method::POST, "/sessions", Some(json!({"mode": "bypass"})))
        .await["data"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    server
        .prompt(&owner, "please start the shared workflow")
        .await;
    let run_id = loop {
        let runs = server
            .ok(
                Method::GET,
                &format!("/workflows/runs?session_id={owner}"),
                None,
            )
            .await;
        if let Some(id) = runs["data"][0]["id"].as_str() {
            break id.to_owned();
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    let run = server.settled_run(&run_id).await;
    assert_eq!(run["status"], "completed", "{run}");
    assert_eq!(run["share"], 1, "{run}");
    let handed = model.requests().iter().any(|body| {
        let last = last_message(body);
        last["role"] == "tool" && last["content"].to_string().contains(r#"\"share\":1"#)
    });
    assert!(handed, "the tool result shows the share");
    server.idle(&owner).await;
    server.close().await;
}

const REVIEW: &str = "export const meta = { name: 'review', description: 'Gated review' };\nexport default async () => (await agent('GATE-r1')).text;\n";

const PLAN: &str = "export const meta = { name: 'plan', description: 'Gated plan' };\nexport default async () => {\n  await agent('GATE-p1');\n  return (await agent('GATE-p2')).text;\n};\n";

/// The owner starts `review` and `plan` through its `workflow` tool and answers anything
/// else; each agent waits at the provider gate named by its task.
fn two_runs_reply(body: &Value) -> Value {
    let last = last_message(body);
    let text = last.to_string();
    if last["role"] == "tool" {
        return json!({"content": "started"});
    }
    if text.contains("Workflow run handback") {
        return json!({"content": "handback received"});
    }
    if text.contains("start the review workflow") {
        return workflow_call_as("call_review", REVIEW, json!({}), json!({}));
    }
    if text.contains("start the plan workflow") {
        return workflow_call_as("call_plan", PLAN, json!({}), json!({}));
    }
    if text.contains("GATE-") {
        return gated_reply(body);
    }
    json!({"content": "owner answer"})
}

impl Server {
    /// The ID of the owner's run named `name`, once it is listed.
    async fn run_named(&self, owner: &str, name: &str) -> String {
        for _ in 0..600 {
            let runs = self
                .ok(
                    Method::GET,
                    &format!("/workflows/runs?session_id={owner}"),
                    None,
                )
                .await;
            let found = runs["data"].as_array().and_then(|runs| {
                runs.iter()
                    .find(|run| run["name"] == name)
                    .and_then(|run| run["id"].as_str().map(str::to_owned))
            });
            if let Some(id) = found {
                return id;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!("{name} was not started");
    }

    async fn status(&self, run: &str) -> Value {
        self.ok(Method::GET, &format!("/workflows/runs/{run}"), None)
            .await["data"]["status"]
            .clone()
    }
}

/// `workflows` → Concurrent runs on the production App: one Session starts two runs
/// through its `workflow` tool and stays usable; stopping one, then pausing and resuming
/// the other over HTTP, acts on that run only, and each attempt that ends hands its
/// summary back once.
#[tokio::test]
async fn two_runs_of_one_session_are_controlled_independently_over_http() {
    let model = ScriptedModel::start(two_runs_reply);
    let server = Server::start(model.config()).await;
    let owner = server
        .ok(Method::POST, "/sessions", Some(json!({"mode": "bypass"})))
        .await["data"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    server
        .prompt(&owner, "please start the review workflow")
        .await;
    let review = server.run_named(&owner, "review").await;
    wait_for("r1 at the gate", || model.held() == ["r1"]).await;
    server.idle(&owner).await;
    server
        .prompt(&owner, "please start the plan workflow")
        .await;
    let plan = server.run_named(&owner, "plan").await;
    wait_for("p1 at the gate", || model.held() == ["r1", "p1"]).await;
    server.idle(&owner).await;

    // The owner answers while both runs execute.
    server.prompt(&owner, "how are the runs going?").await;
    server.idle(&owner).await;
    let answered = model.requests().iter().any(|body| {
        last_message(body)
            .to_string()
            .contains("how are the runs going?")
    });
    assert!(answered);
    for run in [&review, &plan] {
        assert_eq!(server.status(run).await, "running");
    }

    let stopped = server
        .ok(
            Method::POST,
            &format!("/workflows/runs/{review}/stop"),
            None,
        )
        .await;
    assert_eq!(stopped["data"]["status"], "stopped", "{stopped}");
    assert_eq!(server.status(&plan).await, "running");
    // The cancelled r1 request still waits at the gate; let it go first.
    model.open_gate();
    wait_for("only p1 at the gate", || model.held() == ["p1"]).await;

    let paused = server
        .ok(Method::POST, &format!("/workflows/runs/{plan}/pause"), None)
        .await;
    assert_eq!(paused["data"]["status"], "paused", "{paused}");
    assert_eq!(server.status(&review).await, "stopped");
    model.open_gate();
    let (status, body) = resume_when_quiet(&server, &plan, json!({})).await;
    assert!(status.is_success(), "{body}");
    wait_for("p2 at the gate", || model.held() == ["p2"]).await;
    model.open_gate();
    let ended = server.settled_run(&plan).await;
    assert_eq!(ended["status"], "completed", "{ended}");
    assert_eq!(ended["result"], "p2 done", "{ended}");
    assert_eq!(server.status(&review).await, "stopped");

    // Review's stop and plan's resumed attempt each hand back exactly once.
    let ids = [
        cyber_server::runtime::handback_message_id(&review),
        cyber_server::runtime::attempt_handback_message_id(&plan, 1),
    ];
    let mut counts = Vec::new();
    for _ in 0..300 {
        server.idle(&owner).await;
        let state = server.app.runtime.state(&owner).await.unwrap();
        counts = ids
            .iter()
            .map(|id| state.entries.iter().filter(|e| e.id() == id).count())
            .collect();
        if counts == [1, 1] {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(counts, [1, 1]);
    assert!(
        requests_with(&model, "Workflow run handback") >= 2,
        "the owner continued on both summaries"
    );
    server.close().await;
}

/// `workflows` → Write isolation default across runs on the production App: two runs
/// started over HTTP in one git checkout each await one edit-capable agent; the agents
/// overlap at the provider gate, and the one that starts second gets its own worktree.
#[cfg(unix)]
#[tokio::test]
async fn writers_of_two_http_runs_in_one_checkout_are_isolated() {
    let model = ScriptedModel::start(gated_reply);
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    for args in [
        vec!["init", "--quiet", "-b", "main"],
        vec!["config", "user.name", "Workflow"],
        vec!["config", "user.email", "test@example.invalid"],
    ] {
        let status = std::process::Command::new("git")
            .current_dir(&root)
            .args(args)
            .status()
            .unwrap();
        assert!(status.success());
    }
    std::fs::write(root.join("tracked.txt"), "baseline\n").unwrap();
    for args in [
        vec!["add", "tracked.txt"],
        vec!["commit", "--quiet", "-m", "initial"],
    ] {
        let status = std::process::Command::new("git")
            .current_dir(&root)
            .args(args)
            .status()
            .unwrap();
        assert!(status.success());
    }
    let paths = paths(&root);
    paths.ensure().unwrap();
    std::fs::write(paths.config.join("cyber.json"), model.config().to_string()).unwrap();
    let server = Server::open(temp).await;
    let owner = server.session().await;
    let script = |label: &str| {
        format!(
            "export const meta = {{ name: 'writer-{label}', description: 'One writer' }};\nexport default () => agent('GATE-{label}', {{ label: '{label}' }});\n"
        )
    };
    let a = server
        .start_run(json!({"session_id": owner, "script": script("a")}))
        .await;
    wait_for("a at the gate", || model.held() == ["a"]).await;
    let b = server
        .start_run(json!({"session_id": owner, "script": script("b")}))
        .await;
    wait_for("b at the gate", || model.held() == ["a", "b"]).await;
    model.open_gate();
    model.open_gate();
    let (a, b) = (server.settled_run(&a).await, server.settled_run(&b).await);
    assert_eq!(a["status"], "completed", "{a}");
    assert_eq!(b["status"], "completed", "{b}");
    assert!(a["result"].get("branch").is_none(), "{a}");
    let id = b["id"]
        .as_str()
        .unwrap()
        .strip_prefix("run_")
        .unwrap()
        .to_ascii_lowercase();
    assert_eq!(b["result"]["branch"], format!("cyber/run-{id}/1"), "{b}");
    let child = b["result"]["session_id"].as_str().unwrap();
    let directory = server
        .app
        .runtime
        .state(child)
        .await
        .unwrap()
        .info
        .directory;
    assert_ne!(std::path::Path::new(&directory), root.as_path());
    assert!(std::path::Path::new(&directory).is_dir(), "{directory}");
    server.close().await;
}

impl Server {
    /// Poll a run until it ends.
    async fn ended_run(&self, id: &str) -> Value {
        for _ in 0..600 {
            let run = self
                .ok(Method::GET, &format!("/workflows/runs/{id}"), None)
                .await;
            if ["completed", "failed", "budget_exceeded", "stopped"]
                .contains(&run["data"]["status"].as_str().unwrap_or_default())
            {
                return run["data"].clone();
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("run {id} did not end");
    }

    /// `(status, queue_position)` of a run.
    async fn place(&self, run: &str) -> (Value, Value) {
        let shown = self
            .ok(Method::GET, &format!("/workflows/runs/{run}"), None)
            .await;
        (
            shown["data"]["status"].clone(),
            shown["data"]["queue_position"].clone(),
        )
    }
}

fn gated_script(name: &str, label: &str) -> String {
    format!(
        "export const meta = {{ name: '{name}', description: 'Gated' }};\nexport default async () => (await agent('GATE-{label}')).text;\n"
    )
}

/// `workflows` → Run queue on the production App: with `workflows.max_running_runs: 2`
/// in the global configuration, a third and fourth run started over HTTP are recorded
/// `queued` at positions 1 and 2 and start no agent. A queued run paused over HTTP
/// leaves the queue, its resume queues it last, a stopped queued run ends without an
/// agent, and freed slots dispatch the rest in queue order.
#[tokio::test]
async fn runs_beyond_the_run_limit_queue_over_http_and_dispatch_in_order() {
    let model = ScriptedModel::start(gated_reply);
    let mut config = model.config();
    config["workflows"] = json!({"max_running_runs": 2});
    let server = Server::start(config).await;
    let owner = server.session().await;
    let a = server
        .start_run(json!({"session_id": owner, "script": gated_script("ra", "a1")}))
        .await;
    // a1 reaches the gate first, so the first opening ends A.
    wait_for("a1 at the gate", || model.held() == ["a1"]).await;
    let b = server
        .start_run(json!({"session_id": owner, "script": gated_script("rb", "b1")}))
        .await;
    wait_for("a1 and b1 at the gate", || model.held() == ["a1", "b1"]).await;
    let mut queued = Vec::new();
    for (name, label) in [("rc", "c1"), ("rd", "d1"), ("re", "e1")] {
        let started = server
            .ok(
                Method::POST,
                "/workflows/runs",
                Some(json!({"session_id": owner, "script": gated_script(name, label)})),
            )
            .await;
        assert_eq!(started["data"]["status"], "queued", "{started}");
        queued.push((
            started["data"]["id"].as_str().unwrap().to_owned(),
            started["data"]["queue_position"].clone(),
        ));
    }
    let positions: Vec<Value> = queued.iter().map(|(_, at)| at.clone()).collect();
    assert_eq!(positions, [json!(1), json!(2), json!(3)]);
    let [(c, _), (d, _), (e, _)] = <[_; 3]>::try_from(queued).unwrap();
    let listed = server
        .ok(
            Method::GET,
            &format!("/workflows/runs?session_id={owner}"),
            None,
        )
        .await;
    let queued_listed = listed["data"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|run| run["status"] == "queued")
        .count();
    assert_eq!(queued_listed, 3, "{listed}");
    let detail = server
        .ok(Method::GET, &format!("/workflows/runs/{c}"), None)
        .await;
    assert_eq!(detail["data"]["agents_detail"], json!([]), "{detail}");

    // Pause C (leaves the queue), stop E (ends without an agent), resume C (queued last).
    let paused = server
        .ok(Method::POST, &format!("/workflows/runs/{c}/pause"), None)
        .await;
    assert_eq!(paused["data"]["status"], "paused", "{paused}");
    assert_eq!(server.place(&d).await, (json!("queued"), json!(1)));
    let stopped = server
        .ok(Method::POST, &format!("/workflows/runs/{e}/stop"), None)
        .await;
    assert_eq!(stopped["data"]["status"], "stopped", "{stopped}");
    let resumed = server
        .ok(
            Method::POST,
            &format!("/workflows/runs/{c}/resume"),
            Some(json!({})),
        )
        .await;
    assert_eq!(resumed["data"]["status"], "queued", "{resumed}");
    assert_eq!(resumed["data"]["queue_position"], 2, "{resumed}");

    model.open_gate();
    assert_eq!(server.ended_run(&a).await["status"], "completed");
    wait_for("d1 at the gate", || model.held() == ["b1", "d1"]).await;
    assert_eq!(server.place(&c).await, (json!("queued"), json!(1)));
    model.open_gate();
    wait_for("c1 at the gate", || model.held() == ["d1", "c1"]).await;
    model.open_gate();
    model.open_gate();
    for run in [&b, &c, &d] {
        assert_eq!(server.ended_run(run).await["status"], "completed");
    }
    let order: Vec<String> = model.starts().into_iter().map(|(label, _)| label).collect();
    assert_eq!(order, ["a1", "b1", "d1", "c1"]);
    let e = server.ended_run(&e).await;
    assert_eq!(e["status"], "stopped", "{e}");
    let shown = server
        .ok(
            Method::GET,
            &format!("/workflows/runs/{}", e["id"].as_str().unwrap()),
            None,
        )
        .await;
    assert_eq!(shown["data"]["agents_detail"], json!([]), "{shown}");
    assert_eq!(requests_with(&model, "GATE-e1"), 0);
    server.close().await;
}

/// A restart of the production App with every run slot taken: the running run comes back
/// paused and interrupted, and the queued one, never dispatched before the restart, keeps
/// its place and runs its agent once, after it.
#[tokio::test]
async fn a_restart_keeps_a_queued_run_queued_and_dispatches_it_after() {
    let model = ScriptedModel::start(gated_reply);
    let mut config = model.config();
    config["workflows"] = json!({"max_running_runs": 1});
    let server = Server::start(config).await;
    let owner = server.session().await;
    let a = server
        .start_run(json!({"session_id": owner, "script": gated_script("ra", "a1")}))
        .await;
    wait_for("a1 at the gate", || model.held() == ["a1"]).await;
    let b = server
        .start_run(json!({"session_id": owner, "script": gated_script("rb", "b1")}))
        .await;
    assert_eq!(server.place(&b).await, (json!("queued"), json!(1)));
    let server = server.restart().await;
    let shown = server
        .ok(Method::GET, &format!("/workflows/runs/{a}"), None)
        .await;
    assert_eq!(shown["data"]["status"], "paused", "{shown}");
    assert!(
        shown["data"]["reason"]
            .as_str()
            .is_some_and(|reason| reason.contains("server stopped")),
        "{shown}"
    );
    // The abandoned a1 request is first at the gate, then B's agent.
    wait_for("b1 at the gate", || model.held() == ["a1", "b1"]).await;
    model.open_gate();
    model.open_gate();
    let ended = server.ended_run(&b).await;
    assert_eq!(ended["status"], "completed", "{ended}");
    let labels: Vec<String> = model.starts().into_iter().map(|(label, _)| label).collect();
    assert_eq!(
        labels,
        ["a1", "b1"],
        "B ran its agent once, after the restart"
    );
    server.close().await;
}

/// A pause and a `revision: true` resume sent while a queued run's `WorkflowRunStart`
/// hook still runs wait for its start to finish queueing it, so the run is queued once and
/// dispatched with the revised script (regression: the start queued its original module
/// after the resume, ahead of it, and that one ran). The slow hook is a POSIX shell
/// command.
#[cfg(unix)]
#[tokio::test]
async fn a_resume_during_the_start_hook_runs_the_revision_it_validated() {
    let model = ScriptedModel::start(gated_reply);
    let mut config = model.config();
    config["workflows"] = json!({"max_running_runs": 1});
    let slow = r#"input=$(cat); case "$input" in *slowstart*) sleep 2;; esac"#;
    config["hooks"] =
        json!({"WorkflowRunStart": [{"hooks": [{"type": "command", "command": slow}]}]});
    let server = Server::start(config).await;
    let owner = server.session().await;
    let a = server
        .start_run(json!({"session_id": owner, "script": gated_script("ra", "a1")}))
        .await;
    wait_for("a1 at the gate", || model.held() == ["a1"]).await;
    let start = server.call(
        Method::POST,
        "/workflows/runs",
        Some(json!({"session_id": owner, "script": gated_script("slowstart", "b1")})),
    );
    let control = async {
        // The run is listed once recorded, while its start hook still runs.
        let run = loop {
            let runs = server
                .ok(
                    Method::GET,
                    &format!("/workflows/runs?session_id={owner}"),
                    None,
                )
                .await;
            let found = runs["data"]
                .as_array()
                .unwrap()
                .iter()
                .find(|run| run["name"] == "slowstart")
                .cloned();
            if let Some(run) = found {
                break run;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        };
        let id = run["id"].as_str().unwrap().to_owned();
        server
            .ok(Method::POST, &format!("/workflows/runs/{id}/pause"), None)
            .await;
        let file = std::path::PathBuf::from(run["script_file"].as_str().unwrap());
        std::fs::write(&file, gated_script("slowstart", "b2")).unwrap();
        let (status, resumed) = resume_when_quiet(&server, &id, json!({"revision": true})).await;
        assert!(status.is_success(), "{resumed}");
        assert_eq!(resumed["data"]["status"], "queued", "{resumed}");
        assert_eq!(resumed["data"]["revision"], 2, "{resumed}");
        id
    };
    let ((status, started), b) = tokio::join!(start, control);
    assert!(status.is_success(), "{started}");
    assert_eq!(server.place(&b).await, (json!("queued"), json!(1)));
    model.open_gate();
    assert_eq!(server.ended_run(&a).await["status"], "completed");
    wait_for("b2 at the gate", || model.held() == ["b2"]).await;
    model.open_gate();
    let ended = server.ended_run(&b).await;
    assert_eq!(ended["status"], "completed", "{ended}");
    assert_eq!(ended["revision"], 2, "{ended}");
    let labels: Vec<String> = model.starts().into_iter().map(|(label, _)| label).collect();
    assert_eq!(labels, ["a1", "b2"], "only the revised script ran");
    server.close().await;
}

/// The owner starts the review workflow through its tool while the only run slot is
/// taken; anything else it answers.
fn queue_reply(body: &Value) -> Value {
    let last = last_message(body);
    if last["role"] == "tool" {
        return json!({"content": "noted"});
    }
    let text = last.to_string();
    if text.contains("Workflow run handback") {
        return json!({"content": "handback received"});
    }
    if text.contains("start the review workflow") {
        return workflow_call_as("call_review", REVIEW, json!({}), json!({}));
    }
    if text.contains("GATE-") {
        return gated_reply(body);
    }
    json!({"content": "owner answer"})
}

/// The `workflow` tool reports a run started while every run slot is taken as `queued`
/// with its `queue_position`, and the run is dispatched when the slot frees. A 0.3 s wall
/// budget survives a one-second wait in the queue.
#[tokio::test]
async fn the_workflow_tool_reports_a_queued_run_and_its_wall_budget_waits() {
    /// The queued run's `max_wall_seconds`.
    const WALL: f64 = 3.0;
    let model = ScriptedModel::start(queue_reply);
    let mut config = model.config();
    config["workflows"] = json!({"max_running_runs": 1});
    let server = Server::start(config).await;
    let api_owner = server.session().await;
    let a = server
        .start_run(json!({"session_id": api_owner, "script": gated_script("ra", "a1")}))
        .await;
    wait_for("a1 at the gate", || model.held() == ["a1"]).await;
    let owner = server
        .ok(Method::POST, "/sessions", Some(json!({"mode": "bypass"})))
        .await["data"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    server
        .prompt(&owner, "please start the review workflow")
        .await;
    let review = server.run_named(&owner, "review").await;
    server.idle(&owner).await;
    let reported = model.requests().iter().any(|body| {
        let last = last_message(body);
        let content = last["content"].to_string();
        last["role"] == "tool"
            && content.contains(r#"\"status\":\"queued\""#)
            && content.contains(r#"\"queue_position\":1"#)
    });
    assert!(reported, "the tool result reports the queue position");
    assert_eq!(server.place(&review).await, (json!("queued"), json!(1)));

    let timed = "export const meta = { name: 'timed', description: 'Wall budget' };\nexport default async () => { await agent('quick'); checkpoint('after', 1); return ctx.budget.remaining.max_wall_seconds; };\n";
    let timed = server
        .start_run(
            json!({"session_id": api_owner, "script": timed, "budget": {"max_wall_seconds": WALL}}),
        )
        .await;
    assert_eq!(server.place(&timed).await, (json!("queued"), json!(2)));
    // The run waits queued for longer than its whole wall budget; only the time after its
    // dispatch counts. The budget is wide enough for one agent on a loaded CI runner.
    tokio::time::sleep(Duration::from_secs_f64(WALL + 0.5)).await;
    model.open_gate();
    assert_eq!(server.ended_run(&a).await["status"], "completed");
    wait_for("r1 at the gate", || model.held() == ["r1"]).await;
    model.open_gate();
    assert_eq!(server.ended_run(&review).await["status"], "completed");
    let timed = server.ended_run(&timed).await;
    assert_eq!(timed["status"], "completed", "{timed}");
    let remaining = timed["result"].as_f64().unwrap();
    assert!(remaining > 0.0 && remaining <= WALL, "{timed}");
    server.idle(&owner).await;
    server.close().await;
}

/// First attempts of `HOLD-` agents never answer; a resumed child's continuation does.
fn recovery_reply(body: &Value) -> Value {
    let text = body.to_string();
    if text.contains("was interrupted") {
        return json!({"content": "continued"});
    }
    if text.contains("HOLD-") {
        return json!({"hold": true});
    }
    json!({"content": "plain answer"})
}

fn held_script(name: &str) -> String {
    format!(
        "export const meta = {{ name: '{name}', description: 'Held' }};\nexport default async () => (await agent('HOLD-{name}')).text;\n"
    )
}

/// Requests continuing an interrupted child whose task mentions `needle`.
fn continuations(model: &ScriptedModel, needle: &str) -> usize {
    model
        .requests()
        .iter()
        .map(Value::to_string)
        .filter(|text| text.contains("was interrupted") && text.contains(needle))
        .count()
}

/// `workflows` → Recovery of several runs on the production App, "Crash with two runs
/// mid-flight": a new App opens the store while the old one still executes runs A and B
/// (agents in flight) with run C queued behind them. A and B come back paused with their
/// agents interrupted, C keeps its place and is dispatched, and
/// `POST /workflows/runs/resume_interrupted` resumes A and B, each continuing its own
/// child Session once.
#[tokio::test]
async fn a_crash_with_two_runs_mid_flight_recovers_and_resume_interrupted_continues_each() {
    let model = ScriptedModel::start(recovery_reply);
    let mut config = model.config();
    config["workflows"] = json!({"max_running_runs": 2});
    let server = Server::start(config).await;
    let (owner_a, owner_b) = (server.session().await, server.session().await);
    let a = server
        .start_run(json!({"session_id": owner_a, "script": held_script("ra")}))
        .await;
    let b = server
        .start_run(json!({"session_id": owner_b, "script": held_script("rb")}))
        .await;
    wait_for("both agents at the model", || {
        requests_with(&model, "HOLD-") == 2
    })
    .await;
    let c = server
        .ok(
            Method::POST,
            "/workflows/runs",
            Some(json!({"session_id": owner_a, "script": "export const meta = { name: 'rc', description: 'Plain' };\nexport default async () => (await agent('plain c')).text;\n"})),
        )
        .await;
    assert_eq!(c["data"]["status"], "queued", "{c}");
    let c = c["data"]["id"].as_str().unwrap().to_owned();
    let children_b = children(&server, &owner_b);
    assert_eq!(children_b.len(), 1, "{children_b:?}");

    let server = server.crash().await;
    for id in [&a, &b] {
        let run = server
            .ok(Method::GET, &format!("/workflows/runs/{id}"), None)
            .await["data"]
            .clone();
        assert_eq!(run["status"], "paused", "{run}");
        assert!(
            run["reason"].as_str().unwrap().contains("restarted"),
            "{run}"
        );
        assert_eq!(run["agents_detail"][0]["status"], "interrupted", "{run}");
    }
    let c = server.ended_run(&c).await;
    assert_eq!(c["status"], "completed", "{c}");
    assert_eq!(c["result"], "plain answer");

    let report = server
        .ok(Method::POST, "/workflows/runs/resume_interrupted", None)
        .await;
    assert_eq!(report["data"]["resumed"], json!([a, b]), "{report}");
    assert_eq!(report["data"]["refused"], json!([]), "{report}");
    for id in [&a, &b] {
        let run = server.ended_run(id).await;
        assert_eq!(run["status"], "completed", "{run}");
        assert_eq!(run["result"], "continued");
    }
    assert_eq!(continuations(&model, "HOLD-ra"), 1);
    assert_eq!(continuations(&model, "HOLD-rb"), 1);
    // B's agent continued its own child Session instead of creating another.
    assert_eq!(children(&server, &owner_b), children_b);
    let again = server
        .ok(
            Method::POST,
            "/workflows/runs/resume_interrupted?directory=all",
            None,
        )
        .await;
    assert_eq!(again["data"], json!({"resumed": [], "refused": []}));
    model.release();
    server.close().await;
}

/// "Automatic resume after restart" on the production App: with
/// `workflows.resume_after_restart: true`, a restart resumes the interrupted runs of
/// saved Workflows; the one whose saved module was edited stays paused with
/// `WorkflowReplayMismatch`, which `resume_interrupted` reports again.
#[tokio::test]
async fn resume_after_restart_resumes_saved_runs_and_leaves_an_edited_one_paused() {
    let model = ScriptedModel::start(recovery_reply);
    let mut config = model.config();
    config["workflows"] = json!({"resume_after_restart": true});
    let server = Server::start(config).await;
    let saved = server.temp.path().join("config/workflows");
    std::fs::create_dir_all(&saved).unwrap();
    for name in ["keep", "edit"] {
        std::fs::write(saved.join(format!("{name}.js")), held_script(name)).unwrap();
    }
    let owner = server.session().await;
    let keep = server
        .start_run(json!({"session_id": owner, "name": "keep"}))
        .await;
    let edit = server
        .start_run(json!({"session_id": owner, "name": "edit"}))
        .await;
    wait_for("both agents at the model", || {
        requests_with(&model, "HOLD-") == 2
    })
    .await;
    std::fs::write(
        saved.join("edit.js"),
        format!("{}// edited\n", held_script("edit")),
    )
    .unwrap();

    let server = server.restart().await;
    let kept = server.ended_run(&keep).await;
    assert_eq!(kept["status"], "completed", "{kept}");
    assert_eq!(kept["result"], "continued");
    let edited = server
        .ok(Method::GET, &format!("/workflows/runs/{edit}"), None)
        .await["data"]
        .clone();
    assert_eq!(edited["status"], "paused", "{edited}");
    let reason = edited["reason"].as_str().unwrap();
    assert!(reason.starts_with("WorkflowReplayMismatch"), "{edited}");
    assert_eq!(continuations(&model, "HOLD-keep"), 1);
    assert_eq!(continuations(&model, "HOLD-edit"), 0);
    let report = server
        .ok(Method::POST, "/workflows/runs/resume_interrupted", None)
        .await;
    assert_eq!(report["data"]["resumed"], json!([]), "{report}");
    assert_eq!(
        report["data"]["refused"][0]["run_id"],
        json!(edit),
        "{report}"
    );
    let refusal = report["data"]["refused"][0]["reason"].as_str().unwrap();
    assert!(refusal.starts_with("WorkflowReplayMismatch"), "{report}");
    model.release();
    server.close().await;
}

/// The owner starts `review` (gate r1) and `audit` (gate a1) through its `workflow` tool;
/// a prompt naming a gate keeps its own Turn waiting at that gate.
fn handback_reply(body: &Value) -> Value {
    let last = last_message(body);
    let text = last.to_string();
    if last["role"] == "tool" {
        return json!({"content": "started"});
    }
    if text.contains("Workflow run handback") {
        return json!({"content": "handback received"});
    }
    if text.contains("start the review workflow") {
        return workflow_call_as("call_review", REVIEW, json!({}), json!({}));
    }
    if text.contains("start the audit workflow") {
        let audit = gated_script("audit", "a1");
        return workflow_call_as("call_audit", &audit, json!({}), json!({}));
    }
    if text.contains("GATE-") {
        return gated_reply(body);
    }
    json!({"content": "owner answer"})
}

/// The runs whose summaries a request carries, in the order the request holds them.
fn handbacks_in(body: &Value, runs: &[&str]) -> Vec<String> {
    body.to_string()
        .split("Workflow run handback")
        .skip(1)
        .filter_map(|summary| {
            runs.iter()
                .filter_map(|run| summary.find(run).map(|at| (at, *run)))
                .min()
                .map(|(_, run)| run.to_owned())
        })
        .collect()
}

/// Indices of the owner's requests whose last message is a run summary.
fn handback_turns(model: &ScriptedModel) -> Vec<usize> {
    model
        .requests()
        .iter()
        .enumerate()
        .filter(|(_, body)| {
            last_message(body)
                .to_string()
                .contains("Workflow run handback")
        })
        .map(|(at, _)| at)
        .collect()
}

impl Server {
    /// A bypass owner with runs `review` and `audit` started by its tool, each holding
    /// one agent at the gate, and the owner busy in a Turn held at gate o1.
    async fn owner_busy_with_two_runs(&self, model: &ScriptedModel) -> (String, String, String) {
        let owner = self
            .ok(Method::POST, "/sessions", Some(json!({"mode": "bypass"})))
            .await["data"]["id"]
            .as_str()
            .unwrap()
            .to_owned();
        self.prompt(&owner, "please start the review workflow")
            .await;
        let review = self.run_named(&owner, "review").await;
        wait_for("r1 at the gate", || model.held() == ["r1"]).await;
        self.idle(&owner).await;
        self.prompt(&owner, "please start the audit workflow").await;
        let audit = self.run_named(&owner, "audit").await;
        wait_for("a1 at the gate", || model.held() == ["r1", "a1"]).await;
        self.idle(&owner).await;
        self.prompt(&owner, "GATE-o1 keep working").await;
        wait_for("o1 at the gate", || model.held() == ["r1", "a1", "o1"]).await;
        (owner, review, audit)
    }

    /// Let the oldest gated agent answer, then wait until its run's summary is admitted.
    async fn end_and_hand_back(&self, model: &ScriptedModel, run: &str) {
        model.open_gate();
        assert_eq!(self.ended_run(run).await["status"], "completed");
        for _ in 0..600 {
            let shown = self
                .ok(Method::GET, &format!("/workflows/runs/{run}"), None)
                .await;
            if shown["data"]["notified"] == true {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!("the summary of {run} was not admitted");
    }

    /// How often each message is in the owner's model-visible history.
    async fn delivered(&self, owner: &str, ids: &[String]) -> Vec<usize> {
        let state = self.app.runtime.state(owner).await.unwrap();
        ids.iter()
            .map(|id| state.entries.iter().filter(|e| e.id() == id).count())
            .collect()
    }
}

/// `workflows` → Result handback, "Two runs end together", on the production App: two
/// runs started by the owner's `workflow` tool end while the owner is busy, and when its
/// Turn ends the owner runs one Turn whose input holds both summaries in admission order.
#[tokio::test]
async fn two_runs_that_end_together_hand_back_in_one_turn() {
    let model = ScriptedModel::start(handback_reply);
    let server = Server::start(model.config()).await;
    let (owner, review, audit) = server.owner_busy_with_two_runs(&model).await;
    server.end_and_hand_back(&model, &review).await;
    server.end_and_hand_back(&model, &audit).await;
    assert_eq!(model.held(), ["o1"]);
    assert!(handback_turns(&model).is_empty(), "the owner is still busy");

    model.open_gate();
    wait_for("the handback Turn", || !handback_turns(&model).is_empty()).await;
    server.idle(&owner).await;
    let turns = handback_turns(&model);
    assert_eq!(turns.len(), 1, "one Turn for both summaries");
    let body = &model.requests()[turns[0]];
    assert_eq!(
        handbacks_in(body, &[&review, &audit]),
        [review.clone(), audit.clone()]
    );
    let ids = [
        cyber_server::runtime::handback_message_id(&review),
        cyber_server::runtime::handback_message_id(&audit),
    ];
    assert_eq!(server.delivered(&owner, &ids).await, [1, 1]);
    server.close().await;
}

/// A user message queued between two summaries splits them: the first summary, the
/// user's message and the second summary each get their own Turn, in admission order.
#[tokio::test]
async fn a_user_message_between_two_summaries_splits_their_turns() {
    let model = ScriptedModel::start(handback_reply);
    let server = Server::start(model.config()).await;
    let (owner, review, audit) = server.owner_busy_with_two_runs(&model).await;
    server.end_and_hand_back(&model, &review).await;
    server
        .ok(
            Method::POST,
            &format!("/sessions/{owner}/prompt"),
            Some(json!({"delivery": "queue",
                "parts": [{"type": "text", "text": "what comes next?"}]})),
        )
        .await;
    server.end_and_hand_back(&model, &audit).await;

    model.open_gate();
    wait_for("both handback Turns", || handback_turns(&model).len() == 2).await;
    server.idle(&owner).await;
    let requests = model.requests();
    let turns = handback_turns(&model);
    assert_eq!(turns.len(), 2, "{turns:?}");
    let runs = [review.as_str(), audit.as_str()];
    assert_eq!(handbacks_in(&requests[turns[0]], &runs), [review.as_str()]);
    assert_eq!(
        handbacks_in(&requests[turns[1]], &runs),
        [review.clone(), audit.clone()],
        "the second Turn adds the audit summary after the review one"
    );
    let asked = requests
        .iter()
        .position(|body| last_message(body).to_string().contains("what comes next?"))
        .expect("the user's Turn");
    assert!(turns[0] < asked && asked < turns[1], "{turns:?} {asked}");
    server.close().await;
}

/// After a restart with both summaries admitted but neither recorded as admitted, the
/// recovery admits nothing twice, and the owner's next Turn still takes both summaries
/// together, each delivered exactly once.
#[tokio::test]
async fn coalesced_summaries_are_admitted_once_across_a_restart() {
    let model = ScriptedModel::start(handback_reply);
    let server = Server::start(model.config()).await;
    let (owner, review, audit) = server.owner_busy_with_two_runs(&model).await;
    server.end_and_hand_back(&model, &review).await;
    server.end_and_hand_back(&model, &audit).await;
    server
        .app
        .store
        .transaction(|db| {
            db.execute(
                "UPDATE workflow_run SET data = json_set(data, '$.notified', json('false'))",
                [],
            )?;
            Ok(())
        })
        .unwrap();

    let server = server.restart().await;
    // The Turn the restart interrupted still waits at the gate; let it go.
    model.open_gate();
    for run in [&review, &audit] {
        let shown = server
            .ok(Method::GET, &format!("/workflows/runs/{run}"), None)
            .await;
        assert_eq!(shown["data"]["notified"], true, "{shown}");
    }
    let ids = [
        cyber_server::runtime::handback_message_id(&review),
        cyber_server::runtime::handback_message_id(&audit),
    ];
    let state = server.app.runtime.state(&owner).await.unwrap();
    let queued: Vec<&str> = state
        .inbox
        .iter()
        .filter(|row| row.source == "workflow")
        .map(|row| row.message_id.as_str())
        .collect();
    assert_eq!(queued, ids, "each summary is admitted once");

    server.prompt(&owner, "carry on").await;
    wait_for("the handback Turn", || !handback_turns(&model).is_empty()).await;
    server.idle(&owner).await;
    let turns = handback_turns(&model);
    assert_eq!(turns.len(), 1, "{turns:?}");
    let body = &model.requests()[turns[0]];
    assert_eq!(
        handbacks_in(body, &[&review, &audit]),
        [review.clone(), audit.clone()]
    );
    assert_eq!(server.delivered(&owner, &ids).await, [1, 1]);
    server.close().await;
}

/// Events of one instance stream, as they arrive, with their arrival time.
struct Events(std::sync::Arc<std::sync::Mutex<Vec<(std::time::Instant, Value)>>>);

impl Events {
    /// Follow `/event` with `query` (`?location[directory]=…`, or empty).
    async fn follow(server: &Server, query: &str) -> Self {
        use futures::StreamExt;
        let response = server
            .client
            .get(format!("{}/event{query}", server.url))
            .basic_auth("cyber", Some(PASSWORD))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let sink = std::sync::Arc::clone(&seen);
        tokio::spawn(async move {
            let (mut stream, mut buffer) = (response.bytes_stream(), String::new());
            while let Some(Ok(chunk)) = stream.next().await {
                buffer.push_str(&String::from_utf8_lossy(&chunk));
                // Keep a partial last line for the next chunk.
                let complete = buffer.rfind('\n').map_or(0, |at| at + 1);
                let lines: String = buffer.drain(..complete).collect();
                let now = std::time::Instant::now();
                sink.lock()
                    .unwrap()
                    .extend(sse_events(&lines).into_iter().map(|event| (now, event)));
            }
        });
        let events = Self(seen);
        wait_for("the stream to connect", || {
            events.of("server.connected").next().is_some()
        })
        .await;
        events
    }

    fn of(&self, kind: &str) -> std::vec::IntoIter<Value> {
        self.0
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, event)| event["type"] == kind)
            .map(|(_, event)| event.clone())
            .collect::<Vec<_>>()
            .into_iter()
    }

    /// Progress events of `run`, with their arrival times.
    fn progress(&self, run: &str) -> Vec<(std::time::Instant, Value)> {
        self.0
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, event)| {
                event["type"] == "workflow.run.progress" && event["data"]["run_id"] == run
            })
            .cloned()
            .collect()
    }
}

/// `(running, queued, completed)` of an `agents` object.
fn moving(agents: &Value) -> (u64, u64, u64) {
    let count = |key: &str| agents[key].as_u64().unwrap_or(u64::MAX);
    (count("running"), count("queued"), count("completed"))
}

impl Server {
    /// The listed summary of `run` (every Location).
    async fn listed(&self, run: &str) -> Value {
        let page = self
            .ok(Method::GET, "/workflows/runs?directory=all&limit=200", None)
            .await;
        page["data"]
            .as_array()
            .unwrap()
            .iter()
            .find(|summary| summary["id"] == run)
            .cloned()
            .unwrap_or_else(|| panic!("{run} is not listed: {page}"))
    }

    /// A Session of the Location `directory`.
    async fn session_in(&self, directory: &std::path::Path) -> String {
        let created: Value = self
            .client
            .post(format!("{}/sessions", self.url))
            .basic_auth("cyber", Some(PASSWORD))
            .header("x-cyber-directory", directory.display().to_string())
            .json(&json!({"title": "Elsewhere"}))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        created["data"]["id"].as_str().unwrap().to_owned()
    }
}

/// `workflows` → Run monitor, "Queued agents counted", on the production App: a run of
/// 100 gated agents lists every slot of the limit running and the rest queued, a
/// `workflow.run.progress` event with the same counts follows each change on the run's
/// Location only, at most every 250 ms, and a client that resyncs from the list reads the
/// counts of the last event.
#[tokio::test]
async fn queued_agents_are_counted_and_progress_streams_on_the_runs_location() {
    let model = ScriptedModel::start(gated_reply);
    let mut config = model.config();
    config["workflows"] = json!({"max_concurrent": 16});
    let server = Server::start(config).await;
    let elsewhere = server.temp.path().canonicalize().unwrap().join("elsewhere");
    std::fs::create_dir_all(&elsewhere).unwrap();
    let here = Events::follow(&server, "").await;
    let there = Events::follow(
        &server,
        &format!("?location[directory]={}", elsewhere.display()),
    )
    .await;
    let limit = 16.min(std::thread::available_parallelism().map_or(1, usize::from)) as u64;
    let owner = server.session().await;
    let run = server
        .start_run(json!({"session_id": owner, "script": gated_fan_out("counted", "q", 100)}))
        .await;
    wait_for("every slot at the gate", || {
        model.held().len() as u64 == limit
    })
    .await;
    let queued = 100 - limit;
    let mut listed = Value::Null;
    for _ in 0..100 {
        listed = server.listed(&run).await;
        if moving(&listed["agents"]) == (limit, queued, 0) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(moving(&listed["agents"]), (limit, queued, 0), "{listed}");
    assert_eq!(listed["agents"]["total"], 100, "{listed}");
    assert_eq!(listed["status"], "running");
    wait_for("progress with the listed counts", || {
        here.progress(&run)
            .iter()
            .any(|(_, event)| event["data"]["agents"] == listed["agents"])
    })
    .await;

    // One agent ends: the next call starts, and an event with the new counts follows.
    model.open_gate();
    wait_for("progress after the change", || {
        here.progress(&run)
            .iter()
            .any(|(_, event)| moving(&event["data"]["agents"]) == (limit, queued - 1, 1))
    })
    .await;
    // Resync: the list reads what the last event carried once the run is still.
    wait_for("the stream and the list to agree", || {
        model.held().len() as u64 == limit
    })
    .await;
    tokio::time::sleep(Duration::from_millis(600)).await;
    let last = here.progress(&run).last().unwrap().1.clone();
    let listed = server.listed(&run).await;
    for key in ["status", "agents", "tokens", "cost_usd"] {
        assert_eq!(last["data"][key], listed[key], "{key}: {last} {listed}");
    }
    let progress = here.progress(&run);
    let (first, latest) = (progress[0].0, progress.last().unwrap().0);
    let bound = latest.duration_since(first).as_millis() / 250 + 2;
    assert!(
        progress.len() as u128 <= bound,
        "{} events in {:?}",
        progress.len(),
        latest.duration_since(first)
    );
    let directory = std::path::PathBuf::from(listed["directory"].as_str().unwrap());
    for (_, event) in &progress {
        assert!(event["durable"].is_null(), "{event}");
        let location = std::path::PathBuf::from(event["location"].as_str().unwrap());
        assert_eq!(
            location.canonicalize().unwrap(),
            directory.canonicalize().unwrap()
        );
        let keys: Vec<&str> = event["data"]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        for key in &keys {
            assert!(
                ["run_id", "status", "phase", "agents", "tokens", "cost_usd"].contains(key),
                "{key}"
            );
        }
    }
    assert_eq!(there.of("workflow.run.progress").count(), 0);

    server
        .ok(Method::POST, &format!("/workflows/runs/{run}/stop"), None)
        .await;
    wait_for("the stopped run's progress", || {
        here.progress(&run)
            .iter()
            .any(|(_, event)| event["data"]["status"] == "stopped")
    })
    .await;
    server.close().await;
}

/// `workflows` → Run monitor, "Filter active runs", on the production App: running and
/// queued runs of two Locations are listed with `status=running,queued&directory=all`,
/// active first and then the newest, one page at a time; without `directory` the list is
/// the request Location's.
#[tokio::test]
async fn active_runs_of_two_locations_are_filtered_and_paginated_over_http() {
    let model = ScriptedModel::start(gated_reply);
    let mut config = model.config();
    config["workflows"] = json!({"max_running_runs": 2});
    let server = Server::start(config).await;
    let elsewhere = server.temp.path().canonicalize().unwrap().join("elsewhere");
    std::fs::create_dir_all(&elsewhere).unwrap();
    let (owner, other) = (server.session().await, server.session_in(&elsewhere).await);
    let done = server
        .start_run(json!({"session_id": owner, "script": "export const meta = { name: 'done', description: 'No agents' };\nexport default () => 1;\n"}))
        .await;
    assert_eq!(server.ended_run(&done).await["status"], "completed");
    let a = server
        .start_run(json!({"session_id": owner, "script": gated_script("ra", "a1")}))
        .await;
    wait_for("a1 at the gate", || model.held() == ["a1"]).await;
    let b = server
        .start_run(json!({"session_id": other, "script": gated_script("rb", "b1")}))
        .await;
    wait_for("b1 at the gate", || model.held() == ["a1", "b1"]).await;
    let c = server
        .start_run(json!({"session_id": other, "script": gated_script("rc", "c1")}))
        .await;
    let ids = |page: &Value| -> Vec<String> {
        page["data"]
            .as_array()
            .unwrap()
            .iter()
            .map(|run| run["id"].as_str().unwrap().to_owned())
            .collect()
    };
    let active = server
        .ok(
            Method::GET,
            "/workflows/runs?status=running,queued&directory=all",
            None,
        )
        .await;
    assert_eq!(ids(&active), [c.clone(), b.clone(), a.clone()], "{active}");
    let summaries = active["data"].as_array().unwrap();
    assert_eq!(summaries[0]["status"], "queued");
    assert_eq!(summaries[0]["queue_position"], 1);
    assert_eq!(summaries[0]["agents"]["total"], 0);
    assert_eq!(moving(&summaries[1]["agents"]), (1, 0, 0), "{active}");
    for summary in summaries {
        for field in ["tokens", "cost_usd", "elapsed_ms", "share"] {
            assert!(summary[field].is_number(), "{field}: {summary}");
        }
    }
    let here = server.ok(Method::GET, "/workflows/runs", None).await;
    assert_eq!(
        ids(&here),
        [a.clone(), done.clone()],
        "the request Location"
    );
    let there = server
        .ok(
            Method::GET,
            &format!("/workflows/runs?directory={}", elsewhere.display()),
            None,
        )
        .await;
    assert_eq!(ids(&there), [c.clone(), b.clone()]);
    // One run per page, following the cursor: active runs first, then the newest.
    let (mut paged, mut cursor) = (Vec::new(), String::new());
    loop {
        let page = server
            .ok(
                Method::GET,
                &format!("/workflows/runs?directory=all&limit=1{cursor}"),
                None,
            )
            .await;
        paged.extend(ids(&page));
        match page["cursor"]["next"].as_str() {
            Some(next) => cursor = format!("&cursor={next}"),
            None => break,
        }
    }
    assert_eq!(paged, [c.clone(), b.clone(), a.clone(), done.clone()]);
    for _ in 0..3 {
        model.open_gate();
    }
    for run in [&a, &b, &c] {
        assert_eq!(server.ended_run(run).await["status"], "completed");
    }
    server.close().await;
}

/// `workflows` → Run queue, FIFO: a run refused a slot holds its place while its slow
/// `WorkflowRunStart` hook runs, so a slot freed during the hook goes to it and not to a
/// run started after it (regression: the later start took the slot while the earlier run
/// was still `queued` at position 1). The slow hook is a POSIX shell command.
#[cfg(unix)]
#[tokio::test]
async fn a_slot_freed_during_a_queued_runs_start_hook_goes_to_that_run() {
    let model = ScriptedModel::start(gated_reply);
    let mut config = model.config();
    config["workflows"] = json!({"max_running_runs": 1});
    let slow = r#"input=$(cat); case "$input" in *slowstart*) sleep 2;; esac"#;
    config["hooks"] =
        json!({"WorkflowRunStart": [{"hooks": [{"type": "command", "command": slow}]}]});
    let server = Server::start(config).await;
    let owner = server.session().await;
    let a = server
        .start_run(json!({"session_id": owner, "script": gated_script("ra", "a1")}))
        .await;
    wait_for("a1 at the gate", || model.held() == ["a1"]).await;
    let start = server.call(
        Method::POST,
        "/workflows/runs",
        Some(json!({"session_id": owner, "script": gated_script("slowstart", "q1")})),
    );
    let later = async {
        // Q is listed once recorded, while its start hook still runs.
        loop {
            let runs = server
                .ok(
                    Method::GET,
                    &format!("/workflows/runs?session_id={owner}"),
                    None,
                )
                .await;
            if runs["data"]
                .as_array()
                .unwrap()
                .iter()
                .any(|run| run["name"] == "slowstart")
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        model.open_gate();
        assert_eq!(server.ended_run(&a).await["status"], "completed");
        let started = server
            .ok(
                Method::POST,
                "/workflows/runs",
                Some(json!({"session_id": owner, "script": gated_script("rr", "r1")})),
            )
            .await;
        assert_eq!(started["data"]["status"], "queued", "{started}");
        assert_eq!(started["data"]["queue_position"], 2, "{started}");
        started["data"]["id"].as_str().unwrap().to_owned()
    };
    let ((status, started), r) = tokio::join!(start, later);
    assert!(status.is_success(), "{started}");
    let q = started["data"]["id"].as_str().unwrap().to_owned();
    wait_for("q1 at the gate", || model.held() == ["q1"]).await;
    assert_eq!(server.place(&r).await, (json!("queued"), json!(1)));
    model.open_gate();
    assert_eq!(server.ended_run(&q).await["status"], "completed");
    wait_for("r1 at the gate", || model.held() == ["r1"]).await;
    model.open_gate();
    assert_eq!(server.ended_run(&r).await["status"], "completed");
    let labels: Vec<String> = model.starts().into_iter().map(|(label, _)| label).collect();
    assert_eq!(labels, ["a1", "q1", "r1"]);
    server.close().await;
}

/// `workflows` → Recovery of several runs: queued runs come back in their durable queue
/// order, not their start order. X started queued before Y but was paused and resumed to
/// the end of the queue, so after a restart Y is dispatched first (regression: recovery
/// queued and dispatched X before it had re-queued Y). Several worker threads let a
/// dispatch spawned mid-recovery run at once, as it does in the server.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_restart_dispatches_queued_runs_in_queue_order_not_start_order() {
    let model = ScriptedModel::start(gated_reply);
    let mut config = model.config();
    config["workflows"] = json!({"max_running_runs": 1});
    let server = Server::start(config).await;
    let owner = server.session().await;
    server
        .start_run(json!({"session_id": owner, "script": gated_script("ra", "a1")}))
        .await;
    wait_for("a1 at the gate", || model.held() == ["a1"]).await;
    let x = server
        .start_run(json!({"session_id": owner, "script": gated_script("rx", "x1")}))
        .await;
    tokio::time::sleep(Duration::from_millis(5)).await;
    let y = server
        .start_run(json!({"session_id": owner, "script": gated_script("ry", "y1")}))
        .await;
    server
        .ok(Method::POST, &format!("/workflows/runs/{x}/pause"), None)
        .await;
    let resumed = server
        .ok(
            Method::POST,
            &format!("/workflows/runs/{x}/resume"),
            Some(json!({})),
        )
        .await;
    assert_eq!(resumed["data"]["queue_position"], 2, "{resumed}");
    let server = server.restart().await;
    // A comes back paused, freeing the only slot for the head of the queue; the abandoned
    // a1 request is still first at the gate.
    wait_for("a queued run dispatched", || model.held().len() == 2).await;
    assert_eq!(model.held(), ["a1", "y1"], "Y is first in the queue");
    assert_eq!(server.place(&x).await, (json!("queued"), json!(1)));
    model.open_gate();
    model.open_gate();
    assert_eq!(server.ended_run(&y).await["status"], "completed");
    wait_for("x1 at the gate", || model.held() == ["x1"]).await;
    model.open_gate();
    assert_eq!(server.ended_run(&x).await["status"], "completed");
    let labels: Vec<String> = model.starts().into_iter().map(|(label, _)| label).collect();
    assert_eq!(labels, ["a1", "y1", "x1"]);
    server.close().await;
}

/// Runs are listed by Location canonically: a run owned by a Session whose directory is a
/// symlink is listed from the symlink and from its target, with the default Location and
/// with `directory` (regression: an exact match against the canonical request Location
/// left it out of every list).
#[cfg(unix)]
#[tokio::test]
async fn a_run_of_a_session_in_a_symlinked_directory_is_listed_from_either_spelling() {
    let model = ScriptedModel::start(gated_reply);
    let server = Server::start(model.config()).await;
    let root = server.temp.path().canonicalize().unwrap();
    let target = root.join("target");
    let link = root.join("link");
    std::fs::create_dir_all(&target).unwrap();
    std::os::unix::fs::symlink(&target, &link).unwrap();
    let owner = server
        .app
        .runtime
        .create_session(cyber_server::runtime::CreateSession {
            directory: link.display().to_string(),
            model: "test/main".into(),
            ..Default::default()
        })
        .await
        .unwrap()
        .id;
    let run = server
        .start_run(json!({"session_id": owner, "script": gated_script("linked", "l1")}))
        .await;
    wait_for("l1 at the gate", || model.held() == ["l1"]).await;
    for directory in [&link, &target] {
        let listed: Value = server
            .client
            .get(format!("{}/workflows/runs", server.url))
            .basic_auth("cyber", Some(PASSWORD))
            .header("x-cyber-directory", directory.display().to_string())
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let ids: Vec<&str> = listed["data"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|run| run["id"].as_str())
            .collect();
        assert_eq!(ids, [run.as_str()], "{}: {listed}", directory.display());
        let filtered = server
            .ok(
                Method::GET,
                &format!("/workflows/runs?directory={}", directory.display()),
                None,
            )
            .await;
        assert_eq!(filtered["data"].as_array().unwrap().len(), 1, "{filtered}");
    }
    model.open_gate();
    assert_eq!(server.ended_run(&run).await["status"], "completed");
    server.close().await;
}
