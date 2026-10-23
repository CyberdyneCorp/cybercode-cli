//! The production start route performs solve admission before recording a run. No solver
//! or verification implementation is offered by these tests' synthetic read-only module.
use super::*;

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
