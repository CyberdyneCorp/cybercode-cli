//! `provider-credentials` → ChatGPT subscription login / token refresh and
//! `provider-catalog` → ChatGPT subscription provider.

use std::collections::HashMap;

use base64::Engine;
use cyber_llm::adapters::{ApiKind, adapter};
use cyber_llm::catalog::profile::{ReasoningCarrier, VerbosityLevel};
use cyber_llm::catalog::{
    BuildInputs, Catalog, CatalogError, CredentialSource, ModelRef, ProfilePatch,
};
use cyber_llm::chatgpt::{AuthFile, Issuer, Login, ModelsFile, PlanModel, Session};
use cyber_llm::{Endpoint, ErrorKind, LlmRequest, Message, collect};
use serde_json::{Value, json};
use wiremock::matchers::{body_partial_json, header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn jwt(payload: Value) -> String {
    let encode =
        |value: Value| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(value.to_string());
    format!("{}.{}.sig", encode(json!({"alg": "none"})), encode(payload))
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

fn login(expires_in: u64) -> Login {
    Login {
        access_token: jwt(json!({"exp": now() + expires_in})),
        refresh_token: "refresh-1".into(),
        id_token: None,
        account_id: "acct-1".into(),
        email: Some("dev@example.com".into()),
        plan: Some("plus".into()),
        expires_at: Some(now() + expires_in),
    }
}

fn completed() -> ResponseTemplate {
    let body = [
        ("response.output_text.delta", json!({"delta": "hi"})),
        (
            "response.completed",
            json!({"response": {"usage": {"input_tokens": 3, "output_tokens": 1}}}),
        ),
    ]
    .iter()
    .map(|(name, data)| format!("event: {name}\ndata: {data}\n\n"))
    .collect::<String>();
    ResponseTemplate::new(200).set_body_raw(body, "text/event-stream")
}

fn session(dir: &std::path::Path, issuer: &MockServer) -> Session {
    Session {
        file: AuthFile::new(dir.join("chatgpt-auth.json")),
        issuer: Issuer::parse(&issuer.uri()).unwrap(),
        backend: "unused".into(),
    }
}

fn endpoint(backend: &MockServer, session: Session) -> Endpoint {
    let mut endpoint = Endpoint::new(backend.uri(), None);
    endpoint.login = Some(session);
    endpoint
}

fn request() -> LlmRequest {
    LlmRequest {
        model: "gpt-5.1".into(),
        messages: vec![Message::user_text("hello")],
        max_output_tokens: Some(32_000),
        ..LlmRequest::default()
    }
}

#[tokio::test]
async fn subscription_requests_carry_the_account_and_no_output_cap() {
    let dir = tempfile::tempdir().unwrap();
    let (backend, issuer) = (MockServer::start().await, MockServer::start().await);
    let session = session(dir.path(), &issuer);
    let stored = login(3600);
    session.file.save(&stored).unwrap();
    Mock::given(method("POST"))
        .and(path("/responses"))
        .and(header(
            "authorization",
            format!("Bearer {}", stored.access_token),
        ))
        .and(header("chatgpt-account-id", "acct-1"))
        .and(header("originator", "cyber"))
        .and(header("OpenAI-Beta", "responses=experimental"))
        .respond_with(completed())
        .expect(1)
        .mount(&backend)
        .await;

    let stream = adapter(ApiKind::ChatgptCodex, endpoint(&backend, session))
        .stream(request())
        .await
        .unwrap();
    let out = collect(stream, |_| {}).await.unwrap();

    assert_eq!(out.text, "hi");
    let received = backend.received_requests().await.unwrap();
    let body = received
        .iter()
        .find(|r| r.url.path() == "/responses")
        .unwrap()
        .body_json::<Value>()
        .unwrap();
    assert!(body.get("max_output_tokens").is_none());
    assert_eq!(body["store"], false);
    assert_eq!(body["instructions"], "");
    assert!(issuer.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn concurrent_requests_refresh_a_due_token_once() {
    let dir = tempfile::tempdir().unwrap();
    let (backend, issuer) = (MockServer::start().await, MockServer::start().await);
    let session = session(dir.path(), &issuer);
    session.file.save(&login(60)).unwrap();
    let renewed = jwt(json!({"exp": now() + 3600}));
    Mock::given(method("POST"))
        .and(path("/oauth/token"))
        .and(body_partial_json(json!({
            "grant_type": "refresh_token",
            "refresh_token": "refresh-1",
            "client_id": cyber_llm::chatgpt::CLIENT_ID,
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "access_token": renewed, "refresh_token": "refresh-2"
        })))
        .expect(1)
        .mount(&issuer)
        .await;
    Mock::given(path("/responses"))
        .and(header("authorization", format!("Bearer {renewed}")))
        .respond_with(completed())
        .expect(2)
        .mount(&backend)
        .await;

    let first = adapter(ApiKind::ChatgptCodex, endpoint(&backend, session.clone()));
    let second = adapter(ApiKind::ChatgptCodex, endpoint(&backend, session.clone()));
    let (a, b) = tokio::join!(first.stream(request()), second.stream(request()));
    collect(a.unwrap(), |_| {}).await.unwrap();
    collect(b.unwrap(), |_| {}).await.unwrap();

    let stored = session.file.load().unwrap().unwrap();
    assert_eq!(stored.access_token, renewed);
    assert_eq!(stored.refresh_token, "refresh-2");
    assert_eq!(stored.account_id, "acct-1");
}

#[tokio::test]
async fn a_revoked_refresh_token_fails_with_the_login_hint_and_keeps_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let (backend, issuer) = (MockServer::start().await, MockServer::start().await);
    let session = session(dir.path(), &issuer);
    let stored = login(10);
    session.file.save(&stored).unwrap();
    let before = std::fs::read(session.file.path()).unwrap();
    Mock::given(path("/oauth/token"))
        .respond_with(ResponseTemplate::new(400).set_body_json(json!({"error": "invalid_grant"})))
        .mount(&issuer)
        .await;

    let error = match adapter(ApiKind::ChatgptCodex, endpoint(&backend, session.clone()))
        .stream(request())
        .await
    {
        Err(error) => error,
        Ok(_) => panic!("a revoked login must not reach the backend"),
    };

    assert_eq!(error.kind, ErrorKind::Authentication);
    assert!(error.message.contains("cyber providers login chatgpt"));
    assert!(!error.message.contains("refresh-1"));
    assert_eq!(std::fs::read(session.file.path()).unwrap(), before);
    assert!(backend.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn a_missing_model_list_is_fetched_once_and_a_failure_waits_a_day() {
    let dir = tempfile::tempdir().unwrap();
    let (backend, issuer) = (MockServer::start().await, MockServer::start().await);
    let session = session(dir.path(), &issuer);
    session.file.save(&login(3600)).unwrap();
    Mock::given(method("GET"))
        .and(path("/models"))
        .and(query_param(
            "client_version",
            cyber_llm::chatgpt::CLIENT_VERSION,
        ))
        .and(header("chatgpt-account-id", "acct-1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"models": [
            {"slug": "gpt-6-sol", "display_name": "GPT-6-Sol", "visibility": "list",
             "context_window": 272000, "supported_reasoning_levels": [{"effort": "high"}]},
            {"slug": "gpt-reserve", "visibility": "hide"}
        ]})))
        .expect(1)
        .mount(&backend)
        .await;
    Mock::given(path("/responses"))
        .respond_with(completed())
        .expect(2)
        .mount(&backend)
        .await;

    for _ in 0..2 {
        let a = adapter(ApiKind::ChatgptCodex, endpoint(&backend, session.clone()));
        collect(a.stream(request()).await.unwrap(), |_| {})
            .await
            .unwrap();
    }
    let slugs: Vec<_> = session
        .models()
        .load()
        .into_iter()
        .map(|m| m.slug)
        .collect();
    assert_eq!(slugs, ["gpt-6-sol"]);

    // A backend that cannot list keeps the old list and is not asked again for a day.
    let other = tempfile::tempdir().unwrap();
    let session = self::session(other.path(), &issuer);
    session.file.save(&login(3600)).unwrap();
    let failing = MockServer::start().await;
    Mock::given(path("/responses"))
        .respond_with(completed())
        .mount(&failing)
        .await;
    for _ in 0..2 {
        let a = adapter(ApiKind::ChatgptCodex, endpoint(&failing, session.clone()));
        collect(a.stream(request()).await.unwrap(), |_| {})
            .await
            .unwrap();
    }
    let listed = failing
        .received_requests()
        .await
        .unwrap()
        .iter()
        .filter(|r| r.url.path() == "/models")
        .count();
    assert_eq!(listed, 1);
    assert!(session.models().load().is_empty());
}

fn catalog(env: &HashMap<String, String>, config: Value) -> Catalog {
    let data = json!({ "openai": { "id": "openai", "npm": "@ai-sdk/openai", "models": {
        "gpt-5.1": { "id": "gpt-5.1", "tool_call": true, "cost": {"input": 1.25, "output": 10} }
    }}});
    Catalog::build(&BuildInputs {
        data: &data,
        config: &config,
        env,
    })
}

/// The listed `/models` profile fields reach the catalog profile (layer 3).
fn assert_listed_profile(profile: &cyber_llm::catalog::ModelProfile) {
    assert_eq!(profile.reasoning_carrier, ReasoningCarrier::Responses);
    assert_eq!(profile.verbosity.default, Some(VerbosityLevel::Low));
    assert_eq!(profile.efforts, ["low", "max"]);
}

#[test]
fn the_provider_lists_the_plans_models_after_login() {
    let dir = tempfile::tempdir().unwrap();
    let env: HashMap<String, String> =
        [("CYBER_HOME".to_string(), dir.path().display().to_string())].into();
    let missing = catalog(&env, json!({}));
    assert!(matches!(
        missing.list(Some("chatgpt")),
        Err(CatalogError::ProviderNotFound(_))
    ));

    let file = AuthFile::from_env(&env).unwrap();
    file.save(&login(3600)).unwrap();
    ModelsFile::beside(file.path())
        .save(vec![PlanModel {
            slug: "gpt-6-sol".into(),
            name: "GPT-6-Sol".into(),
            context_window: 272_000,
            vision: true,
            efforts: vec!["low".into(), "max".into()],
            profile: ProfilePatch {
                verbosity_supported: Some(true),
                verbosity_default: Some(VerbosityLevel::Low),
                ..ProfilePatch::default()
            },
        }])
        .unwrap();
    let catalog = catalog(&env, json!({}));
    let rows = catalog.list(Some("chatgpt")).unwrap();
    let refs: Vec<_> = rows.iter().map(|row| row.model_ref.as_str()).collect();
    assert_eq!(refs, ["chatgpt/gpt-6-sol"]);
    assert!(catalog.is_available(rows[0].model));
    assert_eq!(rows[0].model.cost, None);
    assert_eq!(rows[0].model.limits.context, 272_000);
    assert_eq!(
        rows[0].model.variants.keys().collect::<Vec<_>>(),
        ["low", "max"]
    );
    assert_listed_profile(&rows[0].model.profile);
    assert_eq!(
        catalog.credential("chatgpt").unwrap().source,
        CredentialSource::Login
    );

    let resolved = catalog
        .resolve(&ModelRef::parse("chatgpt/gpt-6-sol#max").unwrap(), None)
        .unwrap();
    assert_eq!(resolved.kind, ApiKind::ChatgptCodex);
    assert_eq!(
        resolved.endpoint.base_url,
        "https://chatgpt.com/backend-api/codex"
    );
    assert!(resolved.endpoint.api_key.is_none());
    assert!(resolved.endpoint.login.is_some());
}

#[test]
fn configuration_cannot_select_the_subscription_protocol() {
    let env = HashMap::new();
    let catalog = catalog(
        &env,
        json!({"providers": {"evil": {"api": {"kind": "chatgpt-codex", "url": "https://x.test"},
            "models": {"m": {}}}}}),
    );
    let (provider, _) = catalog.find(&ModelRef::parse("evil/m").unwrap()).unwrap();
    assert_eq!(provider.kind, None);
    assert!(catalog.credential("evil").is_none());
}
