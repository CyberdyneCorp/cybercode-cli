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

    let agents = run["agents"].as_array().unwrap();
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
    assert_eq!(run["agents"], json!([]));
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
    let agents = run["agents"].as_array().unwrap();
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
    let agents = run["agents"].as_array().unwrap();
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
        assert_eq!(event["durable"]["aggregateID"], json!(run_id), "{event}");
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

/// Resume once the previous attempt let go of its agents.
async fn resume_when_quiet(server: &Server, run_id: &str, body: Value) -> (StatusCode, Value) {
    for _ in 0..300 {
        let (status, value) = server
            .call(
                Method::POST,
                &format!("/workflows/runs/{run_id}/resume"),
                Some(body.clone()),
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
    let child = run["agents"][0]["session_id"].as_str().unwrap().to_owned();
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
