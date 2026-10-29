//! Production solve admission, isolated execution, verification authority and recovery.
//! Admission-only cases use a synthetic read-only preparation module.
use super::*;
use std::io::Write;

const PREPARE: &str = r#"
export const meta = { name: "solve-admission-test", description: "Inspect admission only", prepare: "solve" };
export default async () => args;
"#;

fn args() -> Value {
    json!({"task":"fix the parser", "checks":[{"argv":["sh","-c","touch must-not-run"]}]})
}

fn repository() -> tempfile::TempDir {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    git(&root, &["init", "--quiet", "-b", "main"]);
    git(&root, &["config", "user.name", "Solve"]);
    git(&root, &["config", "user.email", "test@example.invalid"]);
    std::fs::write(
        root.join(".gitignore"),
        "/data/\n/config/\n/state/\n/cache/\n/tmp/\n/home/\n",
    )
    .unwrap();
    std::fs::write(root.join("source.txt"), "base\n").unwrap();
    git(&root, &["add", ".gitignore", "source.txt"]);
    git(&root, &["commit", "--quiet", "-m", "base"]);
    temp
}

async fn open(temp: tempfile::TempDir, config: Value) -> Server {
    let p = paths(&temp.path().canonicalize().unwrap());
    p.ensure().unwrap();
    std::fs::write(p.config.join("cyber.json"), config.to_string()).unwrap();
    Server::open(temp).await
}

async fn start(server: &Server, owner: &str, args: Value) -> (StatusCode, Value) {
    server
        .call(
            Method::POST,
            "/workflows/runs",
            Some(json!({"session_id":owner,"script":PREPARE,"args":args})),
        )
        .await
}

async fn no_runs(server: &Server, model: &ScriptedModel) {
    let runs = server.ok(Method::GET, "/workflows/runs", None).await;
    assert_eq!(runs["data"], json!([]));
    assert!(
        model.requests().is_empty(),
        "admission dispatched inference"
    );
}

#[tokio::test]
async fn defaults_models_and_clean_base_are_recorded_without_executing_checks() {
    let model = ScriptedModel::start(reply);
    let temp = repository();
    let root = temp.path().canonicalize().unwrap();
    let mut config = model.config();
    config["providers"]["test"]["models"]["second"] = json!({});
    config["small_model"] = "test/second".into();
    let server = open(temp, config).await;
    let owner = server.session().await;
    let (status, body) = start(&server, &owner, args()).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let prepared = &body["data"]["args"];
    assert_eq!(prepared["checks"][0]["argv"], args()["checks"][0]["argv"]);
    assert_eq!(prepared["checks"][0]["timeout_seconds"], 300);
    let admission = &prepared["solve_admission"];
    assert_eq!(admission["candidates"][0]["requested_model"], "fast");
    assert_eq!(admission["candidates"][0]["model"], "test/second");
    assert_eq!(admission["candidates"][1]["model"], "test/main");
    assert_eq!(admission["distinct_solver_models"], 2);
    assert_eq!(admission["judge_model"], "test/main");
    let expected = std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(&root)
        .output()
        .unwrap();
    assert_eq!(
        admission["base"]["commit"],
        String::from_utf8(expected.stdout).unwrap().trim()
    );
    assert_eq!(
        admission["base"]["directory"],
        root.to_string_lossy().as_ref()
    );
    let run = server
        .settled_run(body["data"]["id"].as_str().unwrap())
        .await;
    assert_eq!(run["status"], "completed", "{run}");
    assert!(!root.join("must-not-run").exists());
    assert_eq!(
        std::fs::read_to_string(root.join("source.txt")).unwrap(),
        "base\n"
    );
    assert!(model.requests().is_empty());
    server.close().await;
}

#[tokio::test]
async fn dirty_and_nonignored_untracked_files_refuse_admission_without_stashing() {
    let model = ScriptedModel::start(reply);
    let temp = repository();
    let root = temp.path().canonicalize().unwrap();
    let server = open(temp, model.config()).await;
    let owner = server.session().await;
    std::fs::write(root.join("source.txt"), "user edit\n").unwrap();
    let (status, body) = start(&server, &owner, args()).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(body.to_string().contains("clean working tree"), "{body}");
    assert_eq!(
        std::fs::read_to_string(root.join("source.txt")).unwrap(),
        "user edit\n"
    );
    std::fs::write(root.join("source.txt"), "base\n").unwrap();
    std::fs::write(root.join("untracked.txt"), "user addition").unwrap();
    let (status, body) = start(&server, &owner, args()).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(body.to_string().contains("clean working tree"), "{body}");
    assert_eq!(
        std::fs::read_to_string(root.join("untracked.txt")).unwrap(),
        "user addition"
    );
    no_runs(&server, &model).await;
    server.close().await;
}

#[tokio::test]
async fn invalid_inputs_and_unavailable_models_fail_before_git_or_run_admission() {
    let model = ScriptedModel::start(reply);
    let server = Server::start(model.config()).await; // deliberately not a Git repository
    let owner = server.session().await;
    for (value, message) in [
        (json!({"task":"x","checks":[]}), "args.checks"),
        (
            json!({"task":"x","checks":[{"argv":["true"]}],"solve_admission":{"base":"forged"}}),
            "unknown field",
        ),
        (
            json!({"task":"x","checks":[{"argv":["true"]}],"candidates":[{"label":"a","model":"test/main"},{"label":"b","model":"test/missing"}]}),
            "ModelUnavailable",
        ),
        (
            json!({"task":"x","checks":[{"argv":["true"]}],"judge_model":"missing-tier"}),
            "ModelUnavailable",
        ),
    ] {
        let (status, body) = start(&server, &owner, value).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
        assert!(body.to_string().contains(message), "{body}");
    }
    let (status, body) = start(&server, &owner, args()).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(body.to_string().contains("Git checkout"), "{body}");
    no_runs(&server, &model).await;
    server.close().await;
}

#[tokio::test]
async fn explicit_read_denial_refuses_native_inspection() {
    let model = ScriptedModel::start(reply);
    let mut config = model.config();
    config["permissions"] = json!({"read":"deny"});
    let server = Server::start(config).await; // a read denial must win over the non-Git error
    let owner = server.session().await;
    let (status, body) = start(&server, &owner, args()).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(body["reason"], "PermissionDenied");
    no_runs(&server, &model).await;
    server.close().await;
}

#[tokio::test]
async fn inspecting_dirty_sources_never_executes_repository_clean_filters() {
    let model = ScriptedModel::start(reply);
    let temp = repository();
    let root = temp.path().canonicalize().unwrap();
    std::fs::write(root.join(".gitattributes"), "*.txt filter=trap\n").unwrap();
    git(&root, &["add", ".gitattributes"]);
    git(&root, &["commit", "--quiet", "-m", "attributes"]);
    git(
        &root,
        &["config", "filter.trap.clean", "touch filter-ran; cat"],
    );
    let server = open(temp, model.config()).await;
    let owner = server.session().await;
    std::fs::write(root.join("source.txt"), "user\n").unwrap();
    let (status, body) = start(&server, &owner, args()).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(
        !root.join("filter-ran").exists(),
        "read-only admission ran a repository clean filter"
    );
    assert_eq!(
        std::fs::read_to_string(root.join("source.txt")).unwrap(),
        "user\n"
    );
    no_runs(&server, &model).await;
    server.close().await;
}

fn solve_reply(body: &Value) -> Value {
    let text = body["messages"].to_string();
    if text.contains("Task: judge solve") {
        let arguments = json!({"recommended_candidate":"fast", "rationale":"Focused passing patch", "alternatives":["smart"]});
        return json!({"tool_calls":[{"index":0,"id":"solve_judge_result","type":"function",
            "function":{"name":"return_result","arguments":arguments.to_string()}}]});
    }
    json!({"content":"The patch is ready. All tests passed."})
}

async fn solve_run(server: &Server, checks: Value) -> Value {
    let owner = server.session().await;
    let started = server.ok(Method::POST, "/workflows/runs", Some(json!({"session_id":owner,"name":"solve",
        "args":{"task":"inspect issue", "candidates":[{"label":"fast","model":"test/main"},{"label":"smart","model":"test/main"}],"checks":checks}}))).await;
    server
        .ended_run(started["data"]["id"].as_str().unwrap())
        .await
}

#[tokio::test]
async fn solve_uses_host_checks_on_independent_checkouts_and_discloses_same_model() {
    let model = ScriptedModel::start(solve_reply);
    let temp = repository();
    let root = temp.path().canonicalize().unwrap();
    let mut config = model.config();
    config["permissions"] = json!({"bash":"allow", "edit":"allow"});
    let server = open(temp, config).await;
    let run = solve_run(&server, json!([{"argv":["true"]}])).await;
    assert_eq!(run["status"], "completed", "{run}");
    let result = &run["result"];
    assert_eq!(result["verification_status"], "recommended", "{run}");
    assert_eq!(result["recommended_candidate"], "fast");
    assert_eq!(result["distinct_solver_models"], 1);
    let rows = result["candidates"].as_array().unwrap();
    assert_ne!(rows[0]["branch"], rows[1]["branch"]);
    for row in rows {
        assert_eq!(row["checks"][0]["outcome"], "passed", "{row}");
        assert_eq!(row["checks"][0]["settled"], true);
        assert_eq!(row["checks"][0]["source_unchanged"], true);
        assert_ne!(
            std::path::Path::new(row["checks"][0]["cwd"].as_str().unwrap()),
            root
        );
        assert_eq!(row["base"], result["base"]["commit"]);
    }
    assert_eq!(
        std::fs::read_to_string(root.join("source.txt")).unwrap(),
        "base\n"
    );
    server.close().await;
}

#[tokio::test]
async fn solve_does_not_trust_solver_test_claims_or_source_mutating_checks() {
    for argv in [
        json!(["sh", "-c", "exit 7"]),
        json!(["sh", "-c", "printf changed > source.txt"]),
    ] {
        let model = ScriptedModel::start(solve_reply);
        let temp = repository();
        let root = temp.path().canonicalize().unwrap();
        let mut config = model.config();
        config["permissions"] = json!({"bash":"allow", "edit":"allow"});
        let server = open(temp, config).await;
        let run = solve_run(&server, json!([{"argv":argv}])).await;
        assert_eq!(run["status"], "completed", "{run}");
        assert_eq!(
            run["result"]["verification_status"], "no_candidate_passed",
            "{run}"
        );
        assert_eq!(run["result"]["recommended_candidate"], Value::Null);
        for row in run["result"]["candidates"].as_array().unwrap() {
            assert_eq!(row["checks"][0]["outcome"], "failed", "{row}");
        }
        assert_eq!(
            std::fs::read_to_string(root.join("source.txt")).unwrap(),
            "base\n"
        );
        assert!(!model.requests().iter().any(|request| {
            request["messages"]
                .to_string()
                .contains("Task: judge solve")
        }));
        server.close().await;
    }
}

#[tokio::test]
async fn solve_denied_checks_never_launch() {
    let model = ScriptedModel::start(solve_reply);
    let temp = repository();
    let mut config = model.config();
    config["permissions"] = json!({"bash":"deny"});
    let server = open(temp, config).await;
    let run = solve_run(&server, json!([{"argv":["sh","-c","touch must-not-run"]}])).await;
    assert_eq!(run["status"], "completed", "{run}");
    for row in run["result"]["candidates"].as_array().unwrap() {
        assert_eq!(row["checks"][0]["outcome"], "blocked", "{row}");
        let cwd = row["checks"][0]["cwd"].as_str().unwrap();
        assert!(!std::path::Path::new(cwd).join("must-not-run").exists());
    }
    server.close().await;
}

fn patch_reply(body: &Value) -> Value {
    let text = body["messages"].to_string();
    if text.contains("Task: judge solve") {
        return solve_reply(body);
    }
    let last = body["messages"].as_array().unwrap().last().unwrap();
    if last["role"] == "tool" {
        return json!({"content":"All tests passed."});
    }
    let content = if text.contains("Task: solve smart") {
        "bad\n"
    } else {
        "fixed\n"
    };
    json!({"tool_calls":[{"index":0,"id":"write_patch","type":"function",
        "function":{"name":"bash","arguments":json!({"command":format!("printf '{content}' > source.txt")}).to_string()}}]})
}

#[tokio::test]
async fn solve_tests_real_competing_patches_and_fences_further_solver_writes() {
    let model = ScriptedModel::start(patch_reply);
    let temp = repository();
    let root = temp.path().canonicalize().unwrap();
    let mut config = model.config();
    config["permissions"] = json!({"bash":"allow", "edit":"allow"});
    let server = open(temp, config).await;
    let run = solve_run(
        &server,
        json!([{"argv":["sh","-c","test \"$(cat source.txt)\" = fixed"]}]),
    )
    .await;
    assert_eq!(run["result"]["verification_status"], "recommended", "{run}");
    let rows = run["result"]["candidates"].as_array().unwrap();
    assert_eq!(rows[0]["eligible"], true);
    assert_eq!(rows[1]["eligible"], false);
    for (index, row) in rows.iter().enumerate() {
        let cwd = std::path::Path::new(row["checks"][0]["cwd"].as_str().unwrap());
        assert_eq!(
            std::fs::read_to_string(cwd.join("source.txt")).unwrap(),
            if index == 0 { "fixed\n" } else { "bad\n" }
        );
        let session = row["session_id"].as_str().unwrap();
        let error = server
            .app
            .runtime
            .shell(session, "printf overwritten > source.txt")
            .await
            .unwrap_err();
        assert!(error.to_string().contains("admission"), "{error}");
    }
    assert_eq!(
        std::fs::read_to_string(root.join("source.txt")).unwrap(),
        "base\n"
    );
    server.close().await;
}

#[tokio::test]
async fn an_override_cannot_call_the_host_verification_protocol() {
    let model = ScriptedModel::start(solve_reply);
    let server = open(repository(), model.config()).await;
    let owner = server.session().await;
    let script = r#"export const meta = { name: "solve", description: "untrusted override" };
        export default async () => {
            try { await verifyCandidate({label:"fast"}); return "unexpected"; }
            catch (e) { return { kind:e.kind, recommended_candidate:"fake", verification_status:"recommended" }; }
        };"#;
    let run_id = server
        .start_run(json!({"session_id":owner,"script":script}))
        .await;
    let run = server.ended_run(&run_id).await;
    assert_eq!(run["result"]["kind"], "PermissionDenied", "{run}");
    assert!(run.get("solve_evidence").is_none());
    assert!(model.requests().is_empty());
    server.close().await;
}

fn held_judge_reply(body: &Value) -> Value {
    if body["messages"].to_string().contains("Task: judge solve") {
        json!({"hold":true})
    } else {
        patch_reply(body)
    }
}

#[tokio::test]
async fn solve_restart_during_judging_reuses_settled_checks_without_native_redispatch() {
    let model = ScriptedModel::start(held_judge_reply);
    let temp = repository();
    let root = temp.path().canonicalize().unwrap();
    std::fs::OpenOptions::new()
        .append(true)
        .open(root.join(".gitignore"))
        .unwrap()
        .write_all(b"/build/\n")
        .unwrap();
    git(&root, &["add", ".gitignore"]);
    git(&root, &["commit", "--quiet", "-m", "ignore build products"]);
    let mut config = model.config();
    config["permissions"] = json!({"bash":"allow", "edit":"allow"});
    let server = open(temp, config).await;
    let owner = server.session().await;
    let run_id = server.start_run(json!({"session_id":owner,"name":"solve","args":{
        "task":"fix source", "candidates":[{"label":"fast","model":"test/main"},{"label":"smart","model":"test/main"}],
        "checks":[{"argv":["sh","-c","mkdir -p build; echo checked >> build/count"]}]}})).await;
    wait_for("judge starts after durable checks", || {
        model
            .requests()
            .iter()
            .any(|r| r["messages"].to_string().contains("Task: judge solve"))
    })
    .await;
    let before = server.app.runtime.solve_evidence(&run_id).unwrap().unwrap();
    assert!(before.candidates.iter().all(|row| row.host_verified));
    let server = server.restart().await;
    model.release();
    let (status, body) = resume_when_quiet(&server, &run_id, json!({})).await;
    assert!(status.is_success(), "{body}");
    let run = server.ended_run(&run_id).await;
    assert_eq!(run["status"], "completed", "{run}");
    let after = server.app.runtime.solve_evidence(&run_id).unwrap().unwrap();
    assert_eq!(after.candidates.len(), 2);
    for row in &after.candidates {
        let cwd = row.checks[0]["cwd"].as_str().unwrap();
        assert_eq!(
            std::fs::read_to_string(std::path::Path::new(cwd).join("build/count")).unwrap(),
            "checked\n"
        );
        assert!(row.host_verified);
    }
    assert_eq!(run["agents_detail"].as_array().unwrap().len(), 3);
    server.close().await;
}

#[tokio::test]
async fn solve_timeout_is_incomplete_and_large_logs_are_explicitly_bounded() {
    for (argv, timeout, expected) in [
        (json!(["sh", "-c", "sleep 10"]), 1, "timed_out"),
        (json!(["sh", "-c", "yes x | head -c 1100000"]), 30, "passed"),
    ] {
        let model = ScriptedModel::start(solve_reply);
        let mut config = model.config();
        config["permissions"] = json!({"bash":"allow", "edit":"allow"});
        let server = open(repository(), config).await;
        let run = solve_run(&server, json!([{"argv":argv,"timeout_seconds":timeout}])).await;
        let evidence = server
            .app
            .runtime
            .solve_evidence(run["id"].as_str().unwrap())
            .unwrap()
            .unwrap();
        for row in &evidence.candidates {
            assert_eq!(row.checks[0]["outcome"], expected, "{row:?}");
            assert!(row.checks[0]["log"].as_str().unwrap_or_default().len() <= 1024 * 1024);
            if expected == "passed" {
                assert_eq!(row.checks[0]["log_truncated"], true);
            } else {
                assert!(!row.host_verified);
            }
        }
        if expected == "timed_out" {
            assert_eq!(run["result"]["verification_status"], "incomplete");
        }
        server.close().await;
    }
}

#[tokio::test]
async fn solve_restart_during_checking_preserves_cancelled_effect_without_redispatch() {
    let model = ScriptedModel::start(solve_reply);
    let temp = repository();
    let root = temp.path().canonicalize().unwrap();
    std::fs::OpenOptions::new()
        .append(true)
        .open(root.join(".gitignore"))
        .unwrap()
        .write_all(b"/build/\n")
        .unwrap();
    git(&root, &["add", ".gitignore"]);
    git(&root, &["commit", "--quiet", "-m", "ignore build"]);
    let mut config = model.config();
    config["permissions"] = json!({"bash":"allow", "edit":"allow"});
    let server = open(temp, config).await;
    let owner = server.session().await;
    let run_id = server.start_run(json!({"session_id":owner,"name":"solve","args":{
        "task":"fix source", "candidates":[{"label":"fast","model":"test/main"},{"label":"smart","model":"test/main"}],
        "checks":[{"argv":["sh","-c","mkdir -p build; echo checked >> build/count; sleep 5"],"timeout_seconds":60}]}})).await;
    let mut checkout = None;
    for _ in 0..300 {
        if let Some(evidence) = server.app.runtime.solve_evidence(&run_id).unwrap() {
            checkout = evidence
                .candidates
                .iter()
                .flat_map(|row| &row.checks)
                .filter_map(|check| check["cwd"].as_str())
                .map(std::path::PathBuf::from)
                .find(|path| path.join("build/count").exists());
            if checkout.is_some() {
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let checkout = checkout.expect("native check started");
    let server = server.restart().await;
    let (status, body) = resume_when_quiet(&server, &run_id, json!({})).await;
    assert!(status.is_success(), "{body}");
    let run = server.ended_run(&run_id).await;
    assert_eq!(run["result"]["recommended_candidate"], Value::Null, "{run}");
    assert_eq!(
        std::fs::read_to_string(checkout.join("build/count")).unwrap(),
        "checked\n"
    );
    let evidence = server.app.runtime.solve_evidence(&run_id).unwrap().unwrap();
    assert!(evidence.candidates[0].checks[0]["outcome"] != "passed");
    assert!(!evidence.candidates[0].host_verified);
    server.close().await;
}

fn escape_reply(body: &Value) -> Value {
    let text = body["messages"].to_string();
    if text.contains("Task: judge solve") {
        return solve_reply(body);
    }
    if body["messages"].as_array().unwrap().last().unwrap()["role"] == "tool" {
        return json!({"content":"done"});
    }
    let at = text.find("OUTSIDE=").unwrap() + 8;
    let target: String = text[at..]
        .chars()
        .take_while(|c| *c != '\\' && *c != '"' && !c.is_whitespace())
        .collect();
    json!({"tool_calls":[{"index":0,"id":"escape_write","type":"function",
        "function":{"name":"bash","arguments":json!({"command":format!("printf compromised > '{target}'")}).to_string()}}]})
}

#[tokio::test]
async fn solve_commands_cannot_write_owner_checkout_even_with_full_access_and_extra_roots() {
    let model = ScriptedModel::start(escape_reply);
    let temp = repository();
    let root = temp.path().canonicalize().unwrap();
    let mut config = model.config();
    config["permissions"] = json!("allow");
    config["sandbox"] = json!({"policy":"full-access","writable_roots":[root]});
    let server = open(temp, config).await;
    let owner = server.session().await;
    let run_id = server
        .start_run(json!({"session_id":owner,"name":"solve","args":{
        "task":format!("Try OUTSIDE={}",root.join("source.txt").display()),
        "candidates":[{"label":"fast","model":"test/main"},{"label":"smart","model":"test/main"}],
        "checks":[{"argv":["true"]}]}}))
        .await;
    let run = server.ended_run(&run_id).await;
    assert_eq!(run["status"], "completed", "{run}");
    assert_eq!(
        std::fs::read_to_string(root.join("source.txt")).unwrap(),
        "base\n"
    );
    if let Some(judge) = run["agents_detail"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["label"] == "solve judge")
    {
        let child = judge["session_id"].as_str().unwrap();
        assert_eq!(
            server.app.runtime.state(child).await.unwrap().info.mode,
            "plan"
        );
    }
    server.close().await;
}

fn write_reply(body: &Value) -> Value {
    let last = body["messages"].as_array().unwrap().last().unwrap();
    let (name, id, arguments) = if last["role"] != "tool" {
        ("read", "read_source", json!({"path":"source.txt"}))
    } else if last["tool_call_id"] == "read_source" {
        (
            "write",
            "write_source",
            json!({"path":"source.txt","content":"fixed\n"}),
        )
    } else {
        return json!({"content":"All tests passed."});
    };
    json!({"tool_calls":[{"index":0,"id":id,"type":"function",
        "function":{"name":name,"arguments":arguments.to_string()}}]})
}

#[tokio::test]
async fn solve_setup_failures_and_denied_edits_do_not_produce_verified_patches() {
    for setup_fails in [true, false] {
        let model = ScriptedModel::start(write_reply);
        let temp = repository();
        let root = temp.path().canonicalize().unwrap();
        let mut config = model.config();
        config["permissions"] = if setup_fails {
            json!("allow")
        } else {
            json!({"bash":"allow","edit":"deny"})
        };
        if setup_fails {
            config["worktrees"] = json!({"setup":["exit 7"]});
        }
        let server = open(temp, config).await;
        let run = solve_run(
            &server,
            json!([{"argv":["grep","-q","fixed","source.txt"]}]),
        )
        .await;
        assert_eq!(run["result"]["recommended_candidate"], Value::Null, "{run}");
        assert!(
            run["result"]["candidates"]
                .as_array()
                .unwrap()
                .iter()
                .all(|row| row["eligible"] == false)
        );
        assert_eq!(
            std::fs::read_to_string(root.join("source.txt")).unwrap(),
            "base\n"
        );
        if setup_fails {
            assert!(model.requests().is_empty());
        }
        server.close().await;
    }
}

#[tokio::test]
async fn solve_restart_during_solving_keeps_the_owned_solver_session() {
    let model = ScriptedModel::start(|_| json!({"hold":true}));
    let mut config = model.config();
    config["permissions"] = json!("allow");
    let server = open(repository(), config).await;
    let owner = server.session().await;
    let run_id = server.start_run(json!({"session_id":owner,"name":"solve","args":{
        "task":"fix source", "candidates":[{"label":"fast","model":"test/main"},{"label":"smart","model":"test/main"}],
        "checks":[{"argv":["true"]}]}})).await;
    wait_for("first solver starts", || !model.requests().is_empty()).await;
    let agents = server.app.runtime.workflow_agents(&run_id).unwrap();
    let first = server
        .app
        .runtime
        .resolve_subagent(&owner, &agents[0].id)
        .await
        .unwrap()
        .id;
    let server = server.restart().await;
    model.release();
    let (status, body) = resume_when_quiet(&server, &run_id, json!({})).await;
    assert!(status.is_success(), "{body}");
    let run = server.ended_run(&run_id).await;
    let solvers: Vec<_> = run["agents_detail"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|row| row["label"] != "solve judge")
        .collect();
    assert_eq!(solvers.len(), 2, "{run}");
    assert!(
        solvers.iter().any(|row| row["session_id"] == first),
        "{run}"
    );
    for row in solvers {
        let state = server
            .app
            .runtime
            .state(row["session_id"].as_str().unwrap())
            .await
            .unwrap();
        assert_eq!(state.info.max_steps, Some(20));
    }
    server.close().await;
}

#[tokio::test]
async fn interactive_solve_checks_route_approval_after_solver_submission() {
    use cyber_server::runtime::{
        RunStarter, StartWorkflowRun, TurnContext, TurnWorktree, workflow_start,
    };
    let model = ScriptedModel::start(solve_reply);
    let mut config = model.config();
    config["permissions"] = json!({"bash":"ask","edit":"allow"});
    let server = open(repository(), config.clone()).await;
    let owner = server.session().await;
    let info = server.app.runtime.state(&owner).await.unwrap().info;
    let context = TurnContext {
        session_id: owner.clone(),
        directory: info.directory,
        agent: info.agent,
        mode: info.mode,
        rules: info.rules,
        prefers_apply_patch: false,
        worktree: TurnWorktree::Unmanaged,
    };
    let module = cyber_workflow::bundled::module("solve").unwrap().clone();
    let args = server.app.host.prepare_workflow_args(&context,&module,json!({"task":"inspect", "candidates":[
        {"label":"fast","model":"test/main"},{"label":"smart","model":"test/main"}],"checks":[{"argv":["true"]}]}),true).await.unwrap();
    let settings = workflow_start::location(&config).unwrap();
    let budget = workflow_start::client_budget(&settings, &module, None).unwrap();
    let share = workflow_start::share(&settings, &module, None).unwrap();
    let run = server
        .app
        .runtime
        .start_workflow_run(StartWorkflowRun {
            module,
            args,
            session_id: owner.clone(),
            limits: settings.limits,
            tiers: settings.tiers,
            budget,
            share,
            starter: RunStarter::Session { interactive: true },
            tools: vec![],
        })
        .await
        .unwrap();
    let mut answered = 0;
    for _ in 0..600 {
        let requests = server
            .ok(
                Method::GET,
                &format!("/permissions/requests?session_id={owner}"),
                None,
            )
            .await;
        for request in requests["data"].as_array().unwrap() {
            assert_eq!(request["action"], "bash", "{request}");
            let session = request["session_id"].as_str().unwrap();
            let id = request["id"].as_str().unwrap();
            server
                .ok(
                    Method::POST,
                    &format!("/sessions/{session}/permissions/{id}/reply"),
                    Some(json!({"reply":"once"})),
                )
                .await;
            answered += 1;
        }
        if server
            .app
            .runtime
            .workflow_run(&run.id)
            .unwrap()
            .unwrap()
            .status
            .is_terminal()
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let result = server.ended_run(&run.id).await;
    assert_eq!(answered, 2, "{result}");
    assert_eq!(
        result["result"]["verification_status"], "recommended",
        "{result}"
    );
    server.close().await;
}

/// Release only after settlement assertions, including on assertion failure.
struct DetachedCheckRelease(Vec<std::path::PathBuf>);
impl Drop for DetachedCheckRelease {
    fn drop(&mut self) {
        for directory in &self.0 {
            let _ = std::fs::write(directory.join("release-detached-fixture"), []);
        }
        // The native child runs independently of this test's Tokio executor.
        // Give it the release before the test removes its temporary checkout.
        for _ in 0..100 {
            if self
                .0
                .iter()
                .all(|path| path.join("detached-fixture-finished").exists())
            {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

#[tokio::test]
async fn solve_unknown_native_settlement_fences_later_checks_and_the_checkout_lease() {
    let model = ScriptedModel::start(solve_reply);
    let mut config = model.config();
    config["permissions"] = json!("allow");
    let temp = repository();
    let root = temp.path().canonicalize().unwrap();
    std::fs::write(
        root.join("detach.c"),
        include_str!("fixtures/solve_detach.c"),
    )
    .unwrap();
    let compiled = std::process::Command::new("cc")
        .args(["detach.c", "-o", "detach"])
        .current_dir(&root)
        .output()
        .unwrap();
    assert!(compiled.status.success(), "{compiled:?}");
    git(&root, &["add", "detach.c", "detach"]);
    git(&root, &["commit", "--quiet", "-m", "add process fixture"]);
    let server = open(temp, config).await;
    // A pipe receipt proves setsid completed before the leader can exit. The
    // descendant then retains stdio until this test releases it, so neither the
    // process-group cleanup race nor a four-second sleep can fake settlement.
    let run = solve_run(
        &server,
        json!([{"argv":["./detach"]},
        {"argv":["sh","-c","touch must-not-run"]}]),
    )
    .await;
    let evidence = server
        .app
        .runtime
        .solve_evidence(run["id"].as_str().unwrap())
        .unwrap()
        .unwrap();
    let releases = (!cfg!(target_os = "linux")).then(|| {
        DetachedCheckRelease(
            evidence
                .candidates
                .iter()
                .filter_map(|row| {
                    row.checks.first()?["cwd"]
                        .as_str()
                        .map(std::path::PathBuf::from)
                })
                .collect(),
        )
    });
    if cfg!(target_os = "linux") {
        // Bubblewrap settles the entire namespace, including setsid descendants.
        assert_eq!(run["result"]["verification_status"], "no_candidate_passed");
        for row in &evidence.candidates {
            assert_eq!(row.checks[0]["outcome"], "passed");
            assert_eq!(row.checks[0]["settled"], true);
            assert_eq!(row.checks[1]["outcome"], "failed");
            assert!(!row.host_verified);
        }
        server.close().await;
        return;
    }
    assert_eq!(run["result"]["verification_status"], "incomplete", "{run}");
    for row in &evidence.candidates {
        assert_eq!(row.checks[0]["outcome"], "unknown", "{row:?}");
        assert_eq!(row.checks[0]["settled"], false, "{row:?}");
        assert_eq!(row.checks[0]["source_unchanged"], true, "{row:?}");
        assert!(!row.host_verified);
        assert_eq!(row.checks[1]["outcome"], "not_run");
        let cwd = std::path::Path::new(row.checks[0]["cwd"].as_str().unwrap());
        assert!(!cwd.join("must-not-run").exists());
        assert!(
            server
                .app
                .runtime
                .shell(row.session_id.as_deref().unwrap(), "true")
                .await
                .is_err()
        );
    }
    drop(releases);
    server.close().await;
}

#[tokio::test]
async fn queued_solve_refuses_a_replaced_repository_before_creating_candidate_branches() {
    let model = ScriptedModel::start(|body| {
        if body["messages"].to_string().contains("occupy slot") {
            json!({"hold":true})
        } else {
            solve_reply(body)
        }
    });
    let temp = repository();
    let root = temp.path().canonicalize().unwrap();
    let replacement = tempfile::tempdir().unwrap();
    let clone = replacement.path().join("clone");
    git(
        &root,
        &[
            "clone",
            "--quiet",
            "--no-hardlinks",
            ".",
            clone.to_str().unwrap(),
        ],
    );
    let mut config = model.config();
    config["permissions"] = json!("allow");
    config["workflows"] = json!({"max_running_runs":1,"prompt_cache_warmup":false});
    let server = open(temp, config).await;
    let owner = server.session().await;
    let occupy = "export const meta = { name:'occupy', description:'Hold one run slot' }; export default async()=>agent('occupy slot',{agent:'reviewer'});";
    server
        .start_run(json!({"session_id":owner,"script":occupy}))
        .await;
    wait_for("run slot is occupied", || !model.requests().is_empty()).await;
    let run_id = server.start_run(json!({"session_id":owner,"name":"solve","args":{
        "task":"fix source", "candidates":[{"label":"fast","model":"test/main"},{"label":"smart","model":"test/main"}],
        "checks":[{"argv":["true"]}]}})).await;
    assert_eq!(
        server
            .app
            .runtime
            .workflow_run(&run_id)
            .unwrap()
            .unwrap()
            .status,
        cyber_server::runtime::WorkflowRunStatus::Queued
    );
    std::fs::rename(root.join(".git"), root.join("state/original.git")).unwrap();
    std::fs::write(
        root.join(".git"),
        format!("gitdir: {}\n", clone.join(".git").display()),
    )
    .unwrap();
    model.release();
    let run = server.ended_run(&run_id).await;
    assert_eq!(run["result"]["recommended_candidate"], Value::Null, "{run}");
    assert!(
        run["result"]["limitations"]
            .to_string()
            .contains("repository identity changed"),
        "{run}"
    );
    let branches = std::process::Command::new("git")
        .args(["branch", "--list", "cyber/run-*"])
        .current_dir(&clone)
        .output()
        .unwrap();
    assert!(branches.status.success(), "{branches:?}");
    assert!(branches.stdout.is_empty(), "{branches:?}");
    assert_eq!(
        std::fs::read_to_string(root.join("source.txt")).unwrap(),
        "base\n"
    );
    server.close().await;
}

fn escape_file_reply(body: &Value) -> Value {
    if body["messages"].to_string().contains("Task: judge solve") {
        return solve_reply(body);
    }
    let text = body["messages"].to_string();
    let at = text.find("OUTSIDE=").unwrap() + 8;
    let target: String = text[at..]
        .chars()
        .take_while(|c| *c != '\\' && *c != '"' && !c.is_whitespace())
        .collect();
    let last = body["messages"].as_array().unwrap().last().unwrap();
    let (name, id, input) = if last["role"] != "tool" {
        ("read", "outside_read", json!({"path":target}))
    } else if last["tool_call_id"] == "outside_read" {
        (
            "write",
            "outside_write",
            json!({"path":target,"content":"compromised"}),
        )
    } else {
        return json!({"content":"done"});
    };
    json!({"tool_calls":[{"index":0,"id":id,"type":"function","function":{"name":name,"arguments":input.to_string()}}]})
}

#[tokio::test]
async fn solve_file_tools_cannot_write_outside_the_candidate_even_after_reading_the_target() {
    let model = ScriptedModel::start(escape_file_reply);
    let temp = repository();
    let root = temp.path().canonicalize().unwrap();
    let mut config = model.config();
    config["permissions"] = json!("allow");
    config["sandbox"] = json!({"policy":"full-access"});
    let server = open(temp, config).await;
    let owner = server.session().await;
    let run_id = server
        .start_run(json!({"session_id":owner,"name":"solve","args":{
        "task":format!("Try OUTSIDE={}",root.join("source.txt").display()),
        "candidates":[{"label":"fast","model":"test/main"},{"label":"smart","model":"test/main"}],
        "checks":[{"argv":["true"]}]}}))
        .await;
    let run = server.ended_run(&run_id).await;
    assert_eq!(run["status"], "completed", "{run}");
    assert_eq!(
        std::fs::read_to_string(root.join("source.txt")).unwrap(),
        "base\n"
    );
    assert!(model.requests().iter().any(|request| {
        request["messages"]
            .to_string()
            .contains("Solve candidates cannot write outside their checkout")
    }));
    server.close().await;
}
