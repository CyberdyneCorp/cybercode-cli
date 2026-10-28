//! `provider-credentials` → GitHub Copilot subscription login and `provider-catalog` →
//! GitHub Copilot subscription provider: the device flow, the stored login, the Copilot
//! token mint and the model list, against a fake GitHub and Copilot API.
//! Storing a login is refused on Windows, so these run on unix only.
#![cfg(unix)]

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use cyber_llm::catalog::profile::ReasoningCarrier;
use cyber_llm::catalog::{BuildInputs, Catalog, CatalogError, CredentialSource, ModelRef};
use cyber_llm::copilot::{
    self, AuthFile, CopilotError, CopilotModel, CopilotToken, DeviceCode, Host, Login, ModelsFile,
    Overrides, Protocol, Session, Urls,
};
use cyber_llm::{ApiKind, Content, ErrorKind, Initiator, LlmError, LlmRequest, Message};
use serde_json::{Value, json};
use wiremock::matchers::{body_string_contains, header, method, path};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

const GITHUB_TOKEN: &str = "gho_test0123456789abcdef";

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

fn http() -> reqwest::Client {
    reqwest::Client::new()
}

fn urls(server: &MockServer) -> Urls {
    Overrides::parse(&server.uri(), &server.uri())
        .unwrap()
        .urls(&Host::default())
        .unwrap()
}

fn code(interval: u64, expires_in: u64) -> DeviceCode {
    DeviceCode {
        device_code: "device-1".into(),
        user_code: "ABCD-1234".into(),
        verification_uri: "https://github.com/login/device".into(),
        interval,
        expires_in,
    }
}

/// Answers each request with the next body of a script; the last one repeats.
struct Script(Mutex<Vec<Value>>);

impl Respond for Script {
    fn respond(&self, _: &Request) -> ResponseTemplate {
        let mut bodies = self.0.lock().unwrap();
        let body = if bodies.len() > 1 {
            bodies.remove(0)
        } else {
            bodies[0].clone()
        };
        ResponseTemplate::new(200).set_body_json(body)
    }
}

async fn mount_poll(server: &MockServer, answers: Vec<Value>) {
    Mock::given(method("POST"))
        .and(path("/login/oauth/access_token"))
        .and(body_string_contains("client_id=client-1"))
        .and(body_string_contains("device_code=device-1"))
        .and(body_string_contains(
            "grant_type=urn%3Aietf%3Aparams%3Aoauth%3Agrant-type%3Adevice_code",
        ))
        .respond_with(Script(Mutex::new(answers)))
        .mount(server)
        .await;
}

/// Polls `code`, recording each wait instead of sleeping.
async fn poll(
    server: &MockServer,
    code: &DeviceCode,
) -> (Result<String, CopilotError>, Vec<Duration>) {
    let waits = Arc::new(Mutex::new(Vec::new()));
    let record = waits.clone();
    let result = copilot::poll(&http(), &urls(server), "client-1", code, move |wait| {
        record.lock().unwrap().push(wait);
        std::future::ready(())
    })
    .await;
    let waits = waits.lock().unwrap().clone();
    (result, waits)
}

async fn polls(server: &MockServer) -> usize {
    server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .filter(|r| r.url.path() == "/login/oauth/access_token")
        .count()
}

#[tokio::test]
async fn the_device_code_request_names_the_client_and_scope() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/login/device/code"))
        .and(header("accept", "application/json"))
        .and(body_string_contains("client_id=client-1"))
        .and(body_string_contains("scope=read%3Auser"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "device_code": "device-1", "user_code": "ABCD-1234",
            "verification_uri": "https://github.com/login/device",
            "interval": 5, "expires_in": 899
        })))
        .expect(1)
        .mount(&server)
        .await;

    let code = copilot::request_code(&http(), &urls(&server), "client-1")
        .await
        .unwrap();

    assert_eq!(code, self::code(5, 899));
}

#[tokio::test]
async fn polling_waits_the_interval_until_approval() {
    let server = MockServer::start().await;
    let pending = json!({"error": "authorization_pending"});
    let granted = json!({"access_token": GITHUB_TOKEN, "token_type": "bearer"});
    mount_poll(&server, vec![pending.clone(), pending, granted]).await;

    let (token, waits) = poll(&server, &code(5, 900)).await;

    assert_eq!(token.unwrap(), GITHUB_TOKEN);
    assert_eq!(waits, [Duration::from_secs(5); 3]);
}

#[tokio::test]
async fn slow_down_adds_five_seconds_to_every_later_poll() {
    let server = MockServer::start().await;
    let answers = vec![
        json!({"error": "slow_down", "interval": 10}),
        json!({"error": "authorization_pending"}),
        json!({"access_token": GITHUB_TOKEN}),
    ];
    mount_poll(&server, answers).await;

    let (token, waits) = poll(&server, &code(5, 900)).await;

    assert!(token.is_ok());
    let seconds: Vec<u64> = waits.iter().map(Duration::as_secs).collect();
    assert_eq!(seconds, [5, 10, 10]);

    let server = MockServer::start().await;
    mount_poll(
        &server,
        vec![json!({"error": "slow_down"}), json!({"access_token": "t"})],
    )
    .await;
    let (_, waits) = poll(&server, &code(5, 900)).await;
    assert_eq!(waits, [Duration::from_secs(5), Duration::from_secs(10)]);
}

#[tokio::test]
async fn denial_and_an_expired_code_end_the_flow() {
    for (error, expected) in [
        ("access_denied", CopilotError::Denied),
        ("expired_token", CopilotError::Expired),
    ] {
        let server = MockServer::start().await;
        mount_poll(&server, vec![json!({"error": error})]).await;
        let (result, _) = poll(&server, &code(5, 900)).await;
        assert_eq!(result, Err(expected));
        assert_eq!(polls(&server).await, 1);
    }
}

#[tokio::test]
async fn polling_stops_at_the_codes_expiry() {
    let server = MockServer::start().await;
    mount_poll(&server, vec![json!({"error": "authorization_pending"})]).await;

    let (result, waits) = poll(&server, &code(5, 10)).await;

    assert_eq!(result, Err(CopilotError::Expired));
    assert_eq!(waits.len(), 3);
    assert_eq!(polls(&server).await, 2);
}

#[tokio::test]
async fn dropping_the_poll_cancels_it() {
    let server = MockServer::start().await;
    mount_poll(&server, vec![json!({"error": "authorization_pending"})]).await;
    let (cancel, cancelled) = tokio::sync::oneshot::channel::<()>();
    let mut cancel = Some(cancel);
    let mut waits = 0;
    let urls = urls(&server);
    let client = http();
    let code = code(5, 900);
    let polling = copilot::poll(&client, &urls, "client-1", &code, |_| {
        waits += 1;
        if waits == 2 {
            // The user presses Ctrl+C while the second wait runs.
            let _ = cancel.take().unwrap().send(());
            return futures::future::Either::Left(std::future::pending());
        }
        futures::future::Either::Right(std::future::ready(()))
    });

    tokio::select! {
        result = polling => panic!("polling ended: {result:?}"),
        _ = cancelled => {}
    }

    assert_eq!(polls(&server).await, 1);
}

#[tokio::test]
async fn a_failing_token_endpoint_fails_closed() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/login/oauth/access_token"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;
    let (result, _) = poll(&server, &code(1, 900)).await;
    assert_eq!(
        result,
        Err(CopilotError::Status {
            stage: "sign-in",
            status: 500
        })
    );
}

fn minted(server: &MockServer, expires_in: u64) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({
        "token": "tid=abc;exp=1;sku=copilot_pro;proxy-ep=x:8",
        "expires_at": now() + expires_in,
        "refresh_in": 1500,
        "endpoints": {"api": server.uri()}
    }))
}

fn mint_mock() -> wiremock::MockBuilder {
    Mock::given(method("GET"))
        .and(path("/copilot_internal/v2/token"))
        .and(header("authorization", format!("token {GITHUB_TOKEN}")))
        .and(header("editor-version", copilot::EDITOR_VERSION))
        .and(header("copilot-integration-id", "vscode-chat"))
}

fn login() -> Login {
    Login {
        host: "github.com".into(),
        github_token: GITHUB_TOKEN.into(),
        user: "octocat".into(),
    }
}

fn session(dir: &std::path::Path, server: &MockServer) -> Session {
    let file = AuthFile::new(dir.join("copilot-auth.json"));
    file.save(&login()).unwrap();
    Session::new(
        file,
        Overrides::parse(&server.uri(), &server.uri()).unwrap(),
    )
}

#[tokio::test]
async fn a_finished_device_flow_mints_a_token_and_names_the_user() {
    let server = MockServer::start().await;
    mint_mock()
        .respond_with(minted(&server, 1800))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/user"))
        .and(header("authorization", format!("token {GITHUB_TOKEN}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"login": "octocat"})))
        .expect(1)
        .mount(&server)
        .await;
    let host = Host::parse("octo.ghe.com").unwrap();

    let (login, token) = copilot::confirm(&http(), &urls(&server), &host, GITHUB_TOKEN.into())
        .await
        .unwrap();

    assert_eq!(login.host, "octo.ghe.com");
    assert_eq!(login.user, "octocat");
    assert_eq!(login.github_token, GITHUB_TOKEN);
    assert_eq!(token.api.as_str(), format!("{}/", server.uri()));
}

#[tokio::test]
async fn an_account_without_a_plan_is_refused() {
    let plan_missing = |server: &MockServer| {
        ResponseTemplate::new(200).set_body_json(json!({
            "token": "t", "expires_at": now() + 1800, "endpoints": {"other": server.uri()}
        }))
    };
    for status in [401, 403, 404, 200] {
        let server = MockServer::start().await;
        let response = if status == 200 {
            plan_missing(&server)
        } else {
            ResponseTemplate::new(status)
        };
        mint_mock().respond_with(response).mount(&server).await;
        let result = copilot::confirm(
            &http(),
            &urls(&server),
            &Host::default(),
            GITHUB_TOKEN.into(),
        )
        .await;
        let error = result.unwrap_err();
        assert_eq!(error, CopilotError::NoSubscription, "{status}");
        assert_eq!(
            error.to_string(),
            "This GitHub account has no active Copilot subscription"
        );
    }
}

#[tokio::test]
async fn a_minted_token_is_reused_and_concurrent_callers_share_one_mint() {
    let dir = tempfile::tempdir().unwrap();
    let server = MockServer::start().await;
    mint_mock()
        .respond_with(minted(&server, 1800).set_delay(Duration::from_millis(100)))
        .expect(1)
        .mount(&server)
        .await;
    let session = session(dir.path(), &server);
    let client = http();

    let tokens: Vec<CopilotToken> = futures::future::join_all((0..4).map(|_| {
        let session = session.clone();
        let client = client.clone();
        async move { session.token(&client).await.unwrap() }
    }))
    .await;
    let again = session.token(&client).await.unwrap();

    assert!(tokens.iter().all(|token| *token == again));
    assert!(!format!("{again:?}").contains("tid="));
}

#[tokio::test]
async fn a_token_inside_the_five_minute_window_is_minted_again() {
    let dir = tempfile::tempdir().unwrap();
    let server = MockServer::start().await;
    mint_mock()
        .respond_with(minted(&server, 4 * 60))
        .expect(2)
        .mount(&server)
        .await;
    let session = session(dir.path(), &server);

    session.token(&http()).await.unwrap();
    session.token(&http()).await.unwrap();
}

#[tokio::test]
async fn a_revoked_login_fails_with_the_hint_and_keeps_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let server = MockServer::start().await;
    mint_mock()
        .respond_with(ResponseTemplate::new(401))
        .mount(&server)
        .await;
    let session = session(dir.path(), &server);
    let before = std::fs::read(session.file.path()).unwrap();

    let error: LlmError = session.token(&http()).await.unwrap_err().into();

    assert_eq!(error.kind, ErrorKind::Authentication);
    assert_eq!(
        error.message,
        "GitHub Copilot login expired or was revoked; run cyber providers login copilot"
    );
    assert!(!error.message.contains(GITHUB_TOKEN));
    assert_eq!(std::fs::read(session.file.path()).unwrap(), before);
}

#[tokio::test]
async fn without_a_login_no_token_is_minted() {
    let dir = tempfile::tempdir().unwrap();
    let server = MockServer::start().await;
    let file = AuthFile::new(dir.path().join("copilot-auth.json"));
    let session = Session::new(
        file,
        Overrides::parse(&server.uri(), &server.uri()).unwrap(),
    );
    assert_eq!(session.token(&http()).await, Err(CopilotError::NotSignedIn));
    assert!(server.received_requests().await.unwrap().is_empty());
}

fn model_list() -> Value {
    json!({"object": "list", "data": [
        {"id": "claude-sonnet-4.5", "name": "Claude Sonnet 4.5", "model_picker_enabled": true,
         "policy": {"state": "enabled"},
         "capabilities": {"type": "chat",
            "limits": {"max_context_window_tokens": 200000, "max_output_tokens": 16000},
            "supports": {"vision": true, "tool_calls": true}},
         "supported_endpoints": ["/chat/completions"]},
        {"id": "gpt-5.6-sol", "name": "GPT-5.6 Sol", "model_picker_enabled": true,
         "capabilities": {"limits": {"max_context_window_tokens": 400000},
            "supports": {"tool_calls": true}},
         "supported_endpoints": ["/responses"]},
        {"id": "org-blocked", "model_picker_enabled": true, "policy": {"state": "disabled"}},
        {"id": "hidden", "model_picker_enabled": false}
    ]})
}

#[tokio::test]
async fn the_model_list_is_fetched_with_the_copilot_token_and_cached_daily() {
    let dir = tempfile::tempdir().unwrap();
    let server = MockServer::start().await;
    mint_mock()
        .respond_with(minted(&server, 1800))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/models"))
        .and(header(
            "authorization",
            "Bearer tid=abc;exp=1;sku=copilot_pro;proxy-ep=x:8",
        ))
        .and(header("copilot-integration-id", "vscode-chat"))
        .and(header("editor-version", copilot::EDITOR_VERSION))
        .respond_with(ResponseTemplate::new(200).set_body_json(model_list()))
        .expect(1)
        .mount(&server)
        .await;
    let session = session(dir.path(), &server);

    session.refresh_models(&http()).await;
    session.refresh_models(&http()).await;

    let models = session.models().load();
    let slugs: Vec<_> = models.iter().map(|m| m.slug.as_str()).collect();
    assert_eq!(slugs, ["claude-sonnet-4.5", "gpt-5.6-sol"]);
    assert_eq!(models[0].protocol, Protocol::Chat);
    assert_eq!(
        (models[0].context_window, models[0].max_output),
        (200_000, 16_000)
    );
    assert!(models[0].vision && models[0].tools);
    assert_eq!(models[1].protocol, Protocol::Responses);
    assert_eq!(
        session.models().path(),
        dir.path().join("copilot-models.json")
    );
}

#[tokio::test]
async fn a_failed_model_refresh_keeps_the_old_list_for_a_day() {
    let dir = tempfile::tempdir().unwrap();
    let server = MockServer::start().await;
    mint_mock()
        .respond_with(minted(&server, 1800))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/models"))
        .respond_with(ResponseTemplate::new(502))
        .expect(1)
        .mount(&server)
        .await;
    let session = session(dir.path(), &server);
    let old = format!(
        r#"{{"fetched_at":{},"models":[{{"slug":"old","name":"Old","context_window":1,
            "max_output":1,"vision":false,"tools":true,"protocol":"chat"}}]}}"#,
        now() - 2 * 24 * 60 * 60
    );
    std::fs::write(session.models().path(), old).unwrap();

    session.refresh_models(&http()).await;
    session.refresh_models(&http()).await;

    assert!(!session.models().stale());
    let slugs: Vec<_> = session
        .models()
        .load()
        .into_iter()
        .map(|m| m.slug)
        .collect();
    assert_eq!(slugs, ["old"]);
}

const COPILOT_TOKEN: &str = "tid=abc;exp=1;sku=copilot_pro;proxy-ep=x:8";

/// A stored login and a fresh cached model list naming a Chat Completions model and a
/// Responses-only model, at the data directory `env` names.
fn stored(dir: &std::path::Path, server: &MockServer) -> HashMap<String, String> {
    let env: HashMap<String, String> = [
        ("CYBER_HOME".to_string(), dir.display().to_string()),
        (copilot::GITHUB_URL_ENV.to_string(), server.uri()),
        (copilot::API_URL_ENV.to_string(), server.uri()),
    ]
    .into();
    let file = AuthFile::from_env(&env).unwrap();
    file.save(&login()).unwrap();
    let model = |slug: &str, protocol| CopilotModel {
        slug: slug.into(),
        name: slug.into(),
        context_window: 200_000,
        max_output: 16_000,
        vision: true,
        tools: true,
        protocol,
        api: format!("{}/", server.uri()),
    };
    ModelsFile::beside(file.path())
        .save(vec![
            model("claude-sonnet-4.5", Protocol::Chat),
            model("gpt-5.6-sol", Protocol::Responses),
        ])
        .unwrap();
    env
}

fn catalog(env: &HashMap<String, String>, config: Value) -> Catalog {
    Catalog::build(&BuildInputs {
        data: &json!({}),
        config: &config,
        env,
    })
}

fn chat_reply() -> ResponseTemplate {
    let body = [
        json!({"choices": [{"delta": {"content": "chat hi"}, "finish_reason": "stop"}]}),
        json!({"choices": [], "usage": {"prompt_tokens": 3, "completion_tokens": 1}}),
    ]
    .iter()
    .map(|data| format!("data: {data}\n\n"))
    .collect::<String>()
        + "data: [DONE]\n\n";
    ResponseTemplate::new(200)
        .insert_header("content-type", "text/event-stream")
        .set_body_string(body)
}

fn responses_reply() -> ResponseTemplate {
    let body: String = [
        (
            "response.output_text.delta",
            json!({"delta": "responses hi"}),
        ),
        (
            "response.completed",
            json!({"response": {"usage": {"input_tokens": 3, "output_tokens": 1}}}),
        ),
    ]
    .iter()
    .map(|(name, data)| format!("event: {name}\ndata: {data}\n\n"))
    .collect();
    ResponseTemplate::new(200)
        .insert_header("content-type", "text/event-stream")
        .set_body_string(body)
}

/// The Copilot headers every model request carries.
fn copilot_request(route: &str) -> wiremock::MockBuilder {
    Mock::given(method("POST"))
        .and(path(route))
        .and(header("authorization", format!("Bearer {COPILOT_TOKEN}")))
        .and(header("copilot-integration-id", "vscode-chat"))
        .and(header("editor-version", copilot::EDITOR_VERSION))
        .and(header("editor-plugin-version", copilot::EDITOR_VERSION))
        .and(header("openai-intent", "conversation-edits"))
}

async fn run(catalog: &Catalog, model: &str, request: LlmRequest) -> Result<String, LlmError> {
    let resolved = catalog
        .resolve(&ModelRef::parse(model).unwrap(), None)
        .unwrap();
    let request = LlmRequest {
        messages: request.messages,
        initiator: request.initiator,
        ..resolved.request.clone()
    };
    let stream = resolved.adapter().stream(request).await?;
    Ok(cyber_llm::collect(stream, |_| {}).await?.text)
}

fn prompt(initiator: Initiator) -> LlmRequest {
    LlmRequest {
        messages: vec![Message::user_text("hi")],
        initiator: Some(initiator),
        ..LlmRequest::default()
    }
}

/// The `X-Initiator` of each request to `route`.
async fn initiators(server: &MockServer, route: &str) -> Vec<String> {
    server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .filter(|request| request.url.path() == route)
        .map(|request| {
            let value = request.headers.get("x-initiator");
            value.map_or("", |v| v.to_str().unwrap()).to_owned()
        })
        .collect()
}

#[tokio::test]
async fn chat_models_use_chat_completions_with_the_copilot_headers_and_initiator() {
    let dir = tempfile::tempdir().unwrap();
    let server = MockServer::start().await;
    mint_mock()
        .respond_with(minted(&server, 1800))
        .expect(1)
        .mount(&server)
        .await;
    copilot_request("/chat/completions")
        .respond_with(chat_reply())
        .expect(2)
        .mount(&server)
        .await;
    let catalog = catalog(&stored(dir.path(), &server), json!({}));

    let first = run(
        &catalog,
        "copilot/claude-sonnet-4.5",
        prompt(Initiator::User),
    )
    .await;
    let next = run(
        &catalog,
        "copilot/claude-sonnet-4.5",
        prompt(Initiator::Agent),
    )
    .await;

    assert_eq!(first.unwrap(), "chat hi");
    assert_eq!(next.unwrap(), "chat hi");
    assert_eq!(
        initiators(&server, "/chat/completions").await,
        ["user", "agent"]
    );
    let requests = server.received_requests().await.unwrap();
    let body: Value = serde_json::from_slice(&requests.last().unwrap().body).unwrap();
    assert_eq!(body["model"], "claude-sonnet-4.5");
    assert!(
        requests
            .iter()
            .all(|r| !r.headers.contains_key("copilot-vision-request"))
    );
}

#[tokio::test]
async fn responses_models_use_responses_and_images_add_the_vision_header() {
    let dir = tempfile::tempdir().unwrap();
    let server = MockServer::start().await;
    mint_mock()
        .respond_with(minted(&server, 1800))
        .mount(&server)
        .await;
    copilot_request("/responses")
        .and(header("copilot-vision-request", "true"))
        .respond_with(responses_reply())
        .expect(1)
        .mount(&server)
        .await;
    let catalog = catalog(&stored(dir.path(), &server), json!({}));
    let mut request = prompt(Initiator::User);
    request.messages[0].content.push(Content::Image {
        media_type: "image/png".into(),
        data: "AA==".into(),
    });

    let text = run(&catalog, "copilot/gpt-5.6-sol", request).await.unwrap();

    assert_eq!(text, "responses hi");
    assert_eq!(initiators(&server, "/responses").await, ["user"]);
    let requests = server.received_requests().await.unwrap();
    let sent = requests
        .iter()
        .find(|r| r.url.path() == "/responses")
        .unwrap();
    let body: Value = serde_json::from_slice(&sent.body).unwrap();
    assert_eq!(body["store"], false);
    assert!(
        !requests.iter().any(|r| r.url.path() == "/chat/completions"),
        "a Responses-only model never reaches Chat Completions"
    );
}

#[tokio::test]
async fn a_revoked_login_fails_the_request_before_the_model_and_keeps_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let server = MockServer::start().await;
    mint_mock()
        .respond_with(ResponseTemplate::new(401))
        .mount(&server)
        .await;
    let env = stored(dir.path(), &server);
    let file = AuthFile::from_env(&env).unwrap();
    let before = std::fs::read(file.path()).unwrap();
    let catalog = catalog(&env, json!({}));

    let error = run(
        &catalog,
        "copilot/claude-sonnet-4.5",
        prompt(Initiator::User),
    )
    .await
    .unwrap_err();

    assert_eq!(error.kind, ErrorKind::Authentication);
    assert!(error.message.contains("cyber providers login copilot"));
    assert!(!error.message.contains(GITHUB_TOKEN));
    assert!(initiators(&server, "/chat/completions").await.is_empty());
    assert_eq!(std::fs::read(file.path()).unwrap(), before);
}

/// Regression: a hand-edited login whose host URLs read as an IPv4 address fails the
/// request closed instead of panicking while building the GitHub URLs.
#[tokio::test]
async fn a_stored_login_with_a_numeric_host_fails_the_request_without_panicking() {
    let dir = tempfile::tempdir().unwrap();
    let server = MockServer::start().await;
    let mut env = stored(dir.path(), &server);
    // Without overrides the URLs come from the stored host.
    env.remove(copilot::GITHUB_URL_ENV);
    env.remove(copilot::API_URL_ENV);
    let file = AuthFile::from_env(&env).unwrap();
    let mut document: Value = serde_json::from_slice(&std::fs::read(file.path()).unwrap()).unwrap();
    document["host"] = json!("foo.123");
    std::fs::write(file.path(), serde_json::to_vec(&document).unwrap()).unwrap();
    let catalog = catalog(&env, json!({}));

    let error = run(
        &catalog,
        "copilot/claude-sonnet-4.5",
        prompt(Initiator::User),
    )
    .await
    .unwrap_err();

    assert!(!error.message.contains(GITHUB_TOKEN));
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[test]
fn the_provider_exists_only_with_a_stored_login_and_lists_the_cached_models() {
    let dir = tempfile::tempdir().unwrap();
    let env: HashMap<String, String> =
        [("CYBER_HOME".to_string(), dir.path().display().to_string())].into();
    assert!(matches!(
        catalog(&env, json!({})).list(Some("copilot")),
        Err(CatalogError::ProviderNotFound(_))
    ));

    let file = AuthFile::from_env(&env).unwrap();
    file.save(&login()).unwrap();
    let catalog = catalog(
        &env,
        json!({"providers": {"copilot": {"name": "Work Copilot"}}}),
    );
    assert!(catalog.list(Some("copilot")).unwrap().is_empty());
    let provider = &catalog.providers["copilot"];
    assert_eq!(provider.kind, Some(ApiKind::CopilotChat));
    assert_eq!(provider.url.as_deref(), Some(copilot::API_URL));
    assert_eq!(
        catalog.credential("copilot").unwrap().source,
        CredentialSource::Login
    );
}

#[tokio::test]
async fn listed_models_are_unpriced_with_their_limits_and_the_tokens_endpoint() {
    let dir = tempfile::tempdir().unwrap();
    let server = MockServer::start().await;
    let catalog = catalog(&stored(dir.path(), &server), json!({}));
    let rows = catalog.list(Some("copilot")).unwrap();
    let refs: Vec<_> = rows.iter().map(|row| row.model_ref.as_str()).collect();
    assert_eq!(refs, ["copilot/claude-sonnet-4.5", "copilot/gpt-5.6-sol"]);
    let model = rows[0].model;
    assert!(catalog.is_available(model));
    assert_eq!(model.cost, None);
    assert_eq!(
        (model.limits.context, model.limits.output),
        (200_000, 16_000)
    );
    assert!(model.capabilities.vision && model.capabilities.tools);
    assert_eq!(
        catalog.providers["copilot"].url.as_deref(),
        Some(format!("{}/", server.uri()).as_str())
    );
    let resolved = catalog
        .resolve(&ModelRef::parse("copilot/gpt-5.6-sol").unwrap(), None)
        .unwrap();
    assert_eq!(resolved.kind, ApiKind::CopilotChat);
    assert!(resolved.endpoint.api_key.is_none());
    let route = resolved.endpoint.copilot.unwrap();
    assert_eq!(route.protocol, Protocol::Responses);
}

#[test]
fn configuration_cannot_select_the_copilot_protocol() {
    let env = HashMap::new();
    let catalog = catalog(
        &env,
        json!({"providers": {"evil": {"api": {"type": "copilot-chat", "url": "https://x.test"},
            "models": {"m": {}}}}}),
    );
    let (provider, _) = catalog.find(&ModelRef::parse("evil/m").unwrap()).unwrap();
    assert_eq!(provider.kind, None);
    assert!(catalog.credential("evil").is_none());
    assert_eq!(ApiKind::parse("copilot-chat"), None);
}

/// The events of one request through the catalog's resolution of `model`.
async fn events(
    catalog: &Catalog,
    model: &str,
    messages: Vec<Message>,
) -> Vec<cyber_llm::LlmEvent> {
    let resolved = catalog
        .resolve(&ModelRef::parse(model).unwrap(), None)
        .unwrap();
    let request = LlmRequest {
        messages,
        ..resolved.request.clone()
    };
    let stream = resolved.adapter().stream(request).await.unwrap();
    let mut seen = Vec::new();
    cyber_llm::collect(stream, |event| seen.push(event.clone()))
        .await
        .unwrap();
    seen
}

async fn last_body(server: &MockServer, route: &str) -> Value {
    let requests = server.received_requests().await.unwrap();
    let sent = requests.iter().rfind(|r| r.url.path() == route).unwrap();
    serde_json::from_slice(&sent.body).unwrap()
}

/// A tool loop whose answer reasoned with `opaque` before calling `clock`.
fn reasoned_loop(opaque: cyber_llm::Opaque) -> Vec<Message> {
    vec![
        Message::user_text("time?"),
        Message {
            role: cyber_llm::Role::Assistant,
            content: vec![
                Content::Reasoning {
                    text: "look".into(),
                    opaque: Some(opaque),
                },
                Content::ToolCall {
                    id: "c1".into(),
                    name: "clock".into(),
                    input: json!({}),
                    opaque: None,
                },
            ],
        },
        Message {
            role: cyber_llm::Role::User,
            content: vec![Content::ToolResult {
                call_id: "c1".into(),
                output: "12:00".into(),
                is_error: false,
            }],
        },
    ]
}

/// `session-runtime` → Reasoning items: Copilot's Chat Completions models stream
/// `reasoning_text` with a `reasoning_opaque` blob, which goes back on the assistant
/// message of the tool loop.
#[tokio::test]
async fn copilot_chat_round_trips_reasoning_opaque() {
    use cyber_llm::{LlmEvent, OpaqueKind};
    let dir = tempfile::tempdir().unwrap();
    let server = MockServer::start().await;
    mint_mock()
        .respond_with(minted(&server, 1800))
        .mount(&server)
        .await;
    let body = [
        json!({"choices": [{"delta": {"reasoning_text": "look"}}]}),
        json!({"choices": [{"delta": {"reasoning_opaque": "op4que"}}]}),
        json!({"choices": [{"delta": {"tool_calls": [{"index": 0, "id": "c1",
            "function": {"name": "clock", "arguments": "{}"}}]}, "finish_reason": "tool_calls"}]}),
    ]
    .iter()
    .map(|data| format!("data: {data}\n\n"))
    .collect::<String>()
        + "data: [DONE]\n\n";
    copilot_request("/chat/completions")
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(body),
        )
        .mount(&server)
        .await;
    let catalog = catalog(&stored(dir.path(), &server), json!({}));
    let model = "copilot/claude-sonnet-4.5";

    let seen = events(&catalog, model, vec![Message::user_text("time?")]).await;
    let order: Vec<String> = seen
        .iter()
        .filter_map(|event| match event {
            LlmEvent::ReasoningDelta { text } => Some(format!("text:{text}")),
            LlmEvent::ReasoningItem { opaque, .. } => {
                let opaque = opaque.as_ref().unwrap();
                assert_eq!(opaque.kind, OpaqueKind::EncryptedReasoning);
                Some(format!("opaque:{}", opaque.data))
            }
            LlmEvent::ToolCallDone(call) => Some(format!("call:{}", call.id)),
            _ => None,
        })
        .collect();
    assert_eq!(order, ["text:look", "opaque:op4que", "call:c1"]);

    let opaque = cyber_llm::Opaque::new(OpaqueKind::EncryptedReasoning, "op4que");
    events(&catalog, model, reasoned_loop(opaque)).await;
    let sent = last_body(&server, "/chat/completions").await;
    assert_eq!(
        sent["messages"][1],
        json!({"role": "assistant", "content": null,
            "tool_calls": [{"id": "c1", "type": "function",
                "function": {"name": "clock", "arguments": "{}"}}],
            "reasoning_text": "look", "reasoning_opaque": "op4que"})
    );
}

/// Copilot's Responses models keep their reasoning in Responses items, not in
/// `reasoning_opaque`.
#[tokio::test]
async fn copilot_responses_models_replay_encrypted_reasoning_items() {
    use cyber_llm::OpaqueKind;
    let dir = tempfile::tempdir().unwrap();
    let server = MockServer::start().await;
    mint_mock()
        .respond_with(minted(&server, 1800))
        .mount(&server)
        .await;
    copilot_request("/responses")
        .respond_with(responses_reply())
        .mount(&server)
        .await;
    let catalog = catalog(&stored(dir.path(), &server), json!({}));
    // The profile layer names the carrier per protocol, so `cyber models --json` shows
    // what goes on the wire; the adapter rewrites nothing.
    let carrier = |model: &str| {
        let resolved = catalog
            .resolve(&ModelRef::parse(model).unwrap(), None)
            .unwrap();
        resolved.request.reasoning_carrier
    };
    assert_eq!(carrier("copilot/gpt-5.6-sol"), ReasoningCarrier::Responses);
    assert_eq!(
        carrier("copilot/claude-sonnet-4.5"),
        ReasoningCarrier::CopilotOpaque
    );
    let mut opaque = cyber_llm::Opaque::new(OpaqueKind::EncryptedReasoning, "gAAA");
    opaque.id = Some("rs_7".into());

    events(&catalog, "copilot/gpt-5.6-sol", reasoned_loop(opaque)).await;
    let sent = last_body(&server, "/responses").await;
    assert_eq!(sent["include"], json!(["reasoning.encrypted_content"]));
    assert_eq!(
        sent["input"],
        json!([
            {"role": "user", "content": [{"type": "input_text", "text": "time?"}]},
            {"type": "reasoning", "id": "rs_7", "encrypted_content": "gAAA",
                "summary": [{"type": "summary_text", "text": "look"}]},
            {"type": "function_call", "call_id": "c1", "name": "clock", "arguments": "{}"},
            {"type": "function_call_output", "call_id": "c1", "output": "12:00"},
        ])
    );
    assert!(sent["input"].to_string().find("reasoning_opaque").is_none());
}
