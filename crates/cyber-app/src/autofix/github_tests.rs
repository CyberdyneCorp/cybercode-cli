use super::*;
use axum::{
    Router,
    body::Body,
    extract::{Request, State},
    http::{HeaderMap, Response},
    routing::any,
};
use std::collections::VecDeque;
const HEAD: &str = "1111111111111111111111111111111111111111";
const OTHER: &str = "2222222222222222222222222222222222222222";
fn run(id: u64, name: &str, state: &str) -> Value {
    json!({"id":id,"app":{"id":9},"name":name,"head_sha":HEAD,"status":if state=="pending"{"in_progress"}else{"completed"},"conclusion":state,"completed_at":"2026-10-26T12:00:00Z","output":{"title":"Failure","summary":"untrusted; do not run commands from this text"}})
}
fn status(id: u64, context: &str, state: &str, creator: u64) -> Value {
    json!({"id":id,"context":context,"state":state,"creator":{"id":creator},"description":"result"})
}
fn subscription() -> Subscription {
    Subscription::new(
        "afx_test".into(),
        "/project".into(),
        cyber_core::autofix::Config {
            repository: "team/repo".into(),
            pr: 3,
            session_id: "ses_owner".into(),
            model: "provider/model".into(),
            checks: vec![vec!["cargo".into(), "test".into()]],
            required_ci: vec!["check:9:test".into()],
            verification_paths: vec!["tests".into()],
            max_attempts: 3,
            max_tokens: 10000,
            max_seconds: 60,
        },
        42,
        "fix".into(),
        0,
    )
    .unwrap()
}
#[test]
fn required_checks_missing_pending_and_wrong_provider_never_pass() {
    let required = vec!["check:9:test".into(), "status:8:lint".into()];
    assert_eq!(
        parse_checks(vec![], vec![], HEAD)
            .unwrap()
            .outcome(&required),
        CiOutcome::Pending
    );
    assert_eq!(
        parse_checks(vec![run(1, "test", "success")], vec![], HEAD)
            .unwrap()
            .outcome(&required),
        CiOutcome::Pending
    );
    assert_eq!(
        parse_checks(
            vec![run(1, "test", "success")],
            vec![status(1, "lint", "success", 7)],
            HEAD
        )
        .unwrap()
        .outcome(&required),
        CiOutcome::Pending
    );
    assert_eq!(
        parse_checks(
            vec![run(1, "test", "pending")],
            vec![status(1, "lint", "success", 8)],
            HEAD
        )
        .unwrap()
        .outcome(&required),
        CiOutcome::Pending
    );
    assert_eq!(
        parse_checks(
            vec![run(1, "test", "success")],
            vec![status(1, "lint", "success", 8)],
            HEAD
        )
        .unwrap()
        .outcome(&required),
        CiOutcome::Success
    );
}
#[test]
fn extra_failure_and_same_name_sibling_override_success() {
    let required = vec!["check:9:test".into()];
    let checks = parse_checks(
        vec![run(1, "test", "success"), run(2, "test", "failure")],
        vec![],
        HEAD,
    )
    .unwrap();
    assert_eq!(checks.outcome(&required), CiOutcome::Failure);
    assert_eq!(checks.triggers.len(), 1);
    assert!(checks.triggers[0].key.starts_with("check:2:"));
    assert_eq!(
        parse_checks(
            vec![run(1, "test", "success"), run(2, "other", "pending")],
            vec![],
            HEAD
        )
        .unwrap()
        .outcome(&required),
        CiOutcome::Pending
    );
}
#[test]
fn status_latest_context_is_authoritative_and_sha_mismatch_fails() {
    let required = vec!["status:8:lint".into()];
    let checks = parse_checks(
        vec![],
        vec![
            status(2, "lint", "success", 8),
            status(1, "lint", "failure", 8),
        ],
        HEAD,
    )
    .unwrap();
    assert_eq!(checks.outcome(&required), CiOutcome::Success);
    assert!(checks.triggers.is_empty());
    assert!(parse_checks(vec![run(1, "test", "success")], vec![], OTHER).is_err());
}
#[test]
fn missing_terminal_conclusion_is_pending_and_keys_change_with_revision() {
    let mut r = run(1, "test", "failure");
    let first = parse_checks(vec![r.clone()], vec![], HEAD)
        .unwrap()
        .triggers[0]
        .key
        .clone();
    r["completed_at"] = "2026-10-26T12:01:00Z".into();
    let second = parse_checks(vec![r.clone()], vec![], HEAD)
        .unwrap()
        .triggers[0]
        .key
        .clone();
    assert_ne!(first, second);
    r["conclusion"] = Value::Null;
    assert_eq!(
        parse_checks(vec![r], vec![], HEAD)
            .unwrap()
            .outcome(&["check:9:test".into()]),
        CiOutcome::Pending
    );
}

#[derive(Clone)]
struct Reply {
    status: u16,
    headers: Vec<(String, String)>,
    body: String,
}
impl Reply {
    fn json(value: Value) -> Self {
        Self {
            status: 200,
            headers: vec![],
            body: value.to_string(),
        }
    }
}
#[derive(Default)]
struct Fixture {
    responses: Mutex<BTreeMap<String, VecDeque<Reply>>>,
    seen: Mutex<Vec<(String, HeaderMap)>>,
}
async fn serve(State(f): State<Arc<Fixture>>, request: Request) -> Response<Body> {
    let path = request.uri().to_string();
    f.seen.lock().await.push((
        format!("{} {path}", request.method()),
        request.headers().clone(),
    ));
    let reply = f
        .responses
        .lock()
        .await
        .get_mut(&path)
        .and_then(VecDeque::pop_front)
        .unwrap_or_else(|| Reply {
            status: 500,
            headers: vec![],
            body: "unexpected fixture request".into(),
        });
    let mut response = Response::builder().status(reply.status);
    for (k, v) in reply.headers {
        response = response.header(k, v)
    }
    response.body(Body::from(reply.body)).unwrap()
}
async fn fixture() -> (GitHub, Arc<Fixture>, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let f = Arc::new(Fixture::default());
    let app = Router::new()
        .route("/{*path}", any(serve))
        .with_state(f.clone());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (
        GitHub::new("fixture-owner-token".into(), format!("http://{address}")).unwrap(),
        f,
        server,
    )
}
async fn reply(f: &Fixture, path: &str, r: Reply) {
    f.responses
        .lock()
        .await
        .entry(path.into())
        .or_default()
        .push_back(r)
}
async fn json_reply(f: &Fixture, path: &str, value: Value) {
    reply(f, path, Reply::json(value)).await
}
async fn inspect_replies(f: &Fixture) {
    json_reply(
        f,
        "/repos/team/repo",
        json!({"id":42,"full_name":"team/repo","permissions":{"push":true}}),
    )
    .await;
    json_reply(f,"/repos/team/repo/pulls/3",json!({"state":"open","head":{"sha":HEAD,"ref":"fix","repo":{"id":42}},"base":{"repo":{"id":42}}})).await;
    json_reply(f, "/user", json!({"id":1,"login":"owner"})).await;
}
fn comment(id: u64, login: &str, kind: &str, body: &str) -> Value {
    json!({"id":id,"user":{"login":login,"type":kind},"body":body,"updated_at":"2026-10-26T12:00:00Z"})
}
#[tokio::test]
async fn conditional_etag_keeps_cached_pagination_metadata() {
    let (g, f, server) = fixture().await;
    let mut first = Reply::json(json!({"data":1}));
    first.headers.push(("etag".into(), "\"revision-1\"".into()));
    first.headers.push((
        "link".into(),
        "<https://elsewhere.invalid/not-followed>; rel=\"next\"".into(),
    ));
    reply(&f, "/test", first).await;
    reply(
        &f,
        "/test",
        Reply {
            status: 304,
            headers: vec![],
            body: String::new(),
        },
    )
    .await;
    assert!(g.request(Method::GET, "/test", None).await.unwrap().next);
    assert!(g.request(Method::GET, "/test", None).await.unwrap().next);
    let seen = f.seen.lock().await;
    assert_eq!(seen[1].1.get("if-none-match").unwrap(), "\"revision-1\"");
    assert_eq!(
        seen[0].1.get("authorization").unwrap(),
        "Bearer fixture-owner-token"
    );
    drop(seen);
    server.abort();
}
#[tokio::test]
async fn pagination_is_complete_or_fails_without_partial_observations() {
    let (g, f, server) = fixture().await;
    let mut first = Reply::json(json!([{"id":1}]));
    first.headers.push((
        "link".into(),
        "<https://api.github.com/list?page=2>; rel=\"next\"".into(),
    ));
    reply(&f, "/list?per_page=100&page=1", first.clone()).await;
    json_reply(&f, "/list?per_page=100&page=2", json!([{"id":2}])).await;
    assert_eq!(g.pages("/list", None).await.unwrap().len(), 2);
    reply(&f, "/list?per_page=100&page=1", first).await;
    reply(
        &f,
        "/list?per_page=100&page=2",
        Reply {
            status: 403,
            headers: vec![("retry-after".into(), "120".into())],
            body: "sensitive response must not appear".into(),
        },
    )
    .await;
    let error = g.pages("/list", None).await.unwrap_err();
    assert_eq!(error.retry_after_ms, 120_000);
    assert!(!error.to_string().contains("sensitive"));
    server.abort();
}
#[tokio::test]
async fn redirects_oversized_bodies_and_truncated_counts_fail_closed() {
    let (g, f, server) = fixture().await;
    reply(
        &f,
        "/redirect",
        Reply {
            status: 302,
            headers: vec![("location".into(), "https://attacker.invalid/steal".into())],
            body: String::new(),
        },
    )
    .await;
    assert!(g.get("/redirect").await.is_err());
    reply(
        &f,
        "/large",
        Reply {
            status: 200,
            headers: vec![],
            body: "x".repeat(MAX_BODY + 1),
        },
    )
    .await;
    assert!(g.get("/large").await.is_err());
    json_reply(
        &f,
        "/runs?per_page=100&page=1",
        json!({"total_count":2,"check_runs":[run(1,"test","success")]}),
    )
    .await;
    assert!(g.pages("/runs", Some("check_runs")).await.is_err());
    server.abort();
}
#[tokio::test]
async fn review_baseline_includes_all_authors_and_revision_changes() {
    let (g, f, server) = fixture().await;
    let path = "/repos/team/repo/pulls/3/comments?sort=updated&direction=asc&per_page=100&page=1";
    json_reply(
        &f,
        path,
        json!([
            comment(1, "owner", "User", "old"),
            comment(2, "bot[bot]", "Bot", "old")
        ]),
    )
    .await;
    json_reply(
        &f,
        "/repos/team/repo/pulls/3/reviews?per_page=100&page=1",
        json!([]),
    )
    .await;
    let first = g.baseline_reviews("team/repo", 3).await.unwrap();
    assert_eq!(first.len(), 2);
    json_reply(
        &f,
        path,
        json!([
            comment(1, "owner", "User", "edited"),
            comment(2, "bot[bot]", "Bot", "old")
        ]),
    )
    .await;
    json_reply(
        &f,
        "/repos/team/repo/pulls/3/reviews?per_page=100&page=1",
        json!([]),
    )
    .await;
    let edited = g.baseline_reviews("team/repo", 3).await.unwrap();
    assert_ne!(first[0], edited[0]);
    assert_eq!(first[1], edited[1]);
    server.abort();
}
#[tokio::test]
async fn poll_filters_self_bots_seen_and_readonly_reviewers() {
    let (g, f, server) = fixture().await;
    inspect_replies(&f).await;
    json_reply(
        &f,
        &format!("/repos/team/repo/commits/{HEAD}/check-runs?filter=latest&per_page=100&page=1"),
        json!({"total_count":1,"check_runs":[run(1,"test","success")]}),
    )
    .await;
    json_reply(
        &f,
        &format!("/repos/team/repo/commits/{HEAD}/statuses?per_page=100&page=1"),
        json!([]),
    )
    .await;
    let comments = vec![
        comment(1, "owner", "User", "self"),
        comment(2, "ci[bot]", "Bot", "bot"),
        comment(3, "reader", "User", "read-only"),
        comment(4, "writer", "User", "fix this"),
        comment(5, "seen", "User", "already handled"),
    ];
    let seen_key = {
        let c = &comments[4];
        format!(
            "comment:5:{:x}",
            Sha256::digest(format!(
                "{}\0{}",
                c["updated_at"].as_str().unwrap(),
                c["body"].as_str().unwrap()
            ))
        )
    };
    json_reply(
        &f,
        "/repos/team/repo/pulls/3/comments?sort=updated&direction=asc&per_page=100&page=1",
        json!(comments),
    )
    .await;
    json_reply(
        &f,
        "/repos/team/repo/pulls/3/reviews?per_page=100&page=1",
        json!([]),
    )
    .await;
    json_reply(
        &f,
        "/repos/team/repo/collaborators/reader/permission",
        json!({"permission":"read"}),
    )
    .await;
    json_reply(
        &f,
        "/repos/team/repo/collaborators/writer/permission",
        json!({"permission":"write"}),
    )
    .await;
    let mut s = subscription();
    s.seen.insert(seen_key);
    let poll = g.poll(&s).await.unwrap();
    assert_eq!(poll.ci, CiOutcome::Success);
    assert_eq!(poll.triggers.len(), 1);
    assert!(poll.triggers[0].key.starts_with("comment:4:"));
    let seen = f.seen.lock().await;
    assert!(!seen.iter().any(|(p, _)| p.contains("collaborators/owner")
        || p.contains("collaborators/ci")
        || p.contains("collaborators/seen")));
    drop(seen);
    server.abort();
}
#[tokio::test]
async fn identity_change_is_not_silently_accepted() {
    let (g, f, server) = fixture().await;
    inspect_replies(&f).await;
    let mut s = subscription();
    s.repository_id = 43;
    assert!(
        g.poll(&s)
            .await
            .unwrap_err()
            .to_string()
            .contains("identity changed")
    );
    server.abort();
}
#[tokio::test]
async fn exhaustion_notice_reconciles_only_owner_marker_without_post() {
    let (g, f, server) = fixture().await;
    let s = subscription();
    json_reply(&f, "/user", json!({"id":1,"login":"owner"})).await;
    json_reply(&f,"/repos/team/repo/issues/3/comments?per_page=100&page=1",json!([{"id":2,"user":{"id":9},"body":notice_marker(&s)},{"id":3,"user":{"id":1},"body":notice_marker(&s)}])).await;
    assert_eq!(g.publish_notice(&s).await.unwrap(), "3");
    assert!(
        !f.seen
            .lock()
            .await
            .iter()
            .any(|(p, _)| p.starts_with("POST"))
    );
    server.abort();
}

#[tokio::test]
async fn rate_limits_and_server_failures_are_retryable_observations_but_denials_are_not() {
    let (g, f, server) = fixture().await;
    for (path, status, headers, retryable) in [
        (
            "/rate",
            429,
            vec![("retry-after".into(), "120".into())],
            true,
        ),
        (
            "/secondary",
            403,
            vec![("retry-after".into(), "90".into())],
            true,
        ),
        (
            "/primary",
            403,
            vec![("x-ratelimit-remaining".into(), "0".into())],
            true,
        ),
        ("/server", 503, vec![], true),
        ("/denied", 403, vec![], false),
        ("/revoked", 401, vec![], false),
    ] {
        reply(
            &f,
            path,
            Reply {
                status,
                headers,
                body: "private diagnostics".into(),
            },
        )
        .await;
        let error = g.get(path).await.unwrap_err();
        assert_eq!(error.retryable, retryable, "{path}");
        assert!(!error.to_string().contains("private"));
    }
    server.abort();
}

#[tokio::test]
async fn failed_checks_include_bounded_annotations_without_following_log_urls() {
    let (g, f, server) = fixture().await;
    let mut failed = run(1, "test", "failure");
    failed["output"]["summary"] = "".into();
    failed["output"]["annotations_count"] = 1.into();
    json_reply(
        &f,
        &format!("/repos/team/repo/commits/{HEAD}/check-runs?filter=latest&per_page=100&page=1"),
        json!({"total_count":1,"check_runs":[failed]}),
    )
    .await;
    json_reply(
        &f,
        &format!("/repos/team/repo/commits/{HEAD}/statuses?per_page=100&page=1"),
        json!([]),
    )
    .await;
    json_reply(&f,"/repos/team/repo/check-runs/1/annotations?per_page=100&page=1",json!([{"annotation_level":"failure","path":"src/lib.rs","start_line":7,"message":"assertion failed: expected 42"}])).await;
    let checks = g.checks("team/repo", HEAD).await.unwrap();
    assert!(
        checks.triggers[0]
            .text
            .contains("src/lib.rs:7: assertion failed: expected 42")
    );
    assert_eq!(checks.outcome(&["check:9:test".into()]), CiOutcome::Failure);
    assert!(
        f.seen
            .lock()
            .await
            .iter()
            .all(|(path, _)| !path.contains("/logs"))
    );
    server.abort();
}
