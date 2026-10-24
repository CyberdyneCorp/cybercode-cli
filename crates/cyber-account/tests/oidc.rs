use axum::{
    Router,
    body::Body,
    extract::State,
    http::{Response, StatusCode},
    response::IntoResponse,
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use cyber_account::{AuthorizationAttempt, DiscoveryClient, Error, Provider, Transport};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet},
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};
use tokio::task::JoinHandle;
use url::Url;

#[derive(Clone)]
struct Reply {
    body: String,
    status: StatusCode,
    redirect: Option<String>,
    chunked: bool,
}

#[derive(Clone)]
struct ServerState {
    reply: Arc<Mutex<Reply>>,
    requests: Arc<AtomicUsize>,
}

struct Fixture {
    issuer: String,
    metadata: Value,
    state: ServerState,
    task: JoinHandle<()>,
}

impl Fixture {
    async fn start() -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let issuer = format!("http://{}", listener.local_addr().unwrap());
        let metadata = json!({
            "issuer": issuer,
            "authorization_endpoint": format!("{issuer}/sign-in"),
            "token_endpoint": format!("{issuer}/exchange"),
            "jwks_uri": format!("{issuer}/keys"),
            "userinfo_endpoint": format!("{issuer}/person"),
            "end_session_endpoint": format!("{issuer}/end"),
            "introspection_endpoint": format!("{issuer}/check"),
            "scopes_supported": ["openid", "profile", "email", "offline_access", "cyber:collaborate"],
            "code_challenge_methods_supported": ["S256"],
            "grant_types_supported": ["authorization_code", "refresh_token"]
        });
        let state = ServerState {
            reply: Arc::new(Mutex::new(Reply {
                body: metadata.to_string(),
                status: StatusCode::OK,
                redirect: None,
                chunked: false,
            })),
            requests: Arc::new(AtomicUsize::new(0)),
        };
        let app = Router::new()
            .route(
                "/.well-known/openid-configuration",
                axum::routing::get(serve),
            )
            .route("/sink", axum::routing::get(serve))
            .with_state(state.clone());
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        Self {
            issuer,
            metadata,
            state,
            task,
        }
    }

    fn update(&mut self, edit: impl FnOnce(&mut Value)) {
        edit(&mut self.metadata);
        self.state.reply.lock().unwrap().body = self.metadata.to_string();
    }

    async fn discover(&self) -> Result<Provider, Error> {
        DiscoveryClient::new(Transport::AllowLoopbackHttp)
            .unwrap()
            .discover(&self.issuer)
            .await
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn serve(State(state): State<ServerState>) -> impl IntoResponse {
    state.requests.fetch_add(1, Ordering::SeqCst);
    let reply = state.reply.lock().unwrap().clone();
    let mut response = Response::builder().status(reply.status);
    if let Some(location) = reply.redirect {
        response = response.header("location", location);
    }
    let body = if reply.chunked {
        Body::from_stream(futures::stream::iter(vec![Ok::<_, std::io::Error>(
            reply.body,
        )]))
    } else {
        Body::from(reply.body)
    };
    response.body(body).unwrap()
}

fn redirect() -> Url {
    Url::parse("http://127.0.0.1:53682/callback").unwrap()
}

fn callback(attempt: &AuthorizationAttempt, extra: &str) -> String {
    let state = attempt
        .authorization_url()
        .query_pairs()
        .find(|(k, _)| k == "state")
        .unwrap()
        .1;
    format!("http://127.0.0.1:53682/callback?state={state}&{extra}")
}

#[tokio::test]
async fn discovery_uses_advertised_paths_and_filters_unavailable_scopes() {
    let f = Fixture::start().await;
    let provider = f.discover().await.unwrap();
    assert_eq!(provider.issuer(), f.issuer);
    assert_eq!(provider.token_endpoint().path(), "/exchange");
    assert_eq!(provider.jwks_uri().path(), "/keys");
    assert_eq!(provider.userinfo_endpoint().unwrap().path(), "/person");
    assert_eq!(provider.end_session_endpoint().unwrap().path(), "/end");
    assert_eq!(provider.introspection_endpoint().unwrap().path(), "/check");
    let scopes = provider.login_scopes().unwrap();
    assert_eq!(
        scopes.requested,
        [
            "openid",
            "profile",
            "email",
            "offline_access",
            "cyber:collaborate"
        ]
    );
    assert_eq!(
        scopes.omitted,
        [
            "cyber:relay",
            "cyber:runner",
            "cyber:share",
            "cyber:messaging"
        ]
    );
    assert_eq!(f.state.requests.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn issuer_mismatch_is_rejected_without_other_requests() {
    let mut f = Fixture::start().await;
    f.update(|doc| doc["issuer"] = json!("https://unrelated.example"));
    assert_eq!(f.discover().await.unwrap_err(), Error::IssuerMismatch);
    assert_eq!(f.state.requests.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn trailing_slash_is_normalized() {
    let mut f = Fixture::start().await;
    f.update(|doc| doc["issuer"] = json!(format!("{}/", doc["issuer"].as_str().unwrap())));
    assert!(f.discover().await.is_ok());
}

#[tokio::test]
async fn insecure_issuers_are_rejected_before_network_io() {
    let client = DiscoveryClient::new(Transport::Https).unwrap();
    for issuer in [
        "http://127.0.0.1",
        "file:///tmp/auth",
        "https://user:secret@example.com",
        "https://auth.example?q=secret",
        "https://auth.example#fragment",
        "not a url",
    ] {
        assert_eq!(
            client.discover(issuer).await.unwrap_err(),
            Error::InvalidIssuer
        );
    }
    let development = DiscoveryClient::new(Transport::AllowLoopbackHttp).unwrap();
    for issuer in [
        "http://localhost",
        "http://192.168.1.1",
        "http://auth.example",
    ] {
        assert_eq!(
            development.discover(issuer).await.unwrap_err(),
            Error::InvalidIssuer
        );
    }
}

#[tokio::test]
async fn unsafe_discovered_endpoints_are_rejected() {
    let mut f = Fixture::start().await;
    for endpoint in [
        "authorization_endpoint",
        "token_endpoint",
        "jwks_uri",
        "userinfo_endpoint",
        "end_session_endpoint",
        "introspection_endpoint",
        "device_authorization_endpoint",
    ] {
        for url in [
            "http://remote.example/token",
            "file:///secret",
            "https://user:password@auth.example/token",
            "https://auth.example/token#fragment",
            "/relative",
        ] {
            f.update(|doc| doc[endpoint] = json!(url));
            assert_eq!(
                f.discover().await.unwrap_err(),
                Error::InvalidDiscovery,
                "{endpoint}: {url}"
            );
        }
        f.update(|doc| {
            doc[endpoint] = json!("https://auth.example/endpoint");
        });
    }
}

#[tokio::test]
async fn discovery_redirects_are_not_followed() {
    let f = Fixture::start().await;
    {
        let mut reply = f.state.reply.lock().unwrap();
        reply.status = StatusCode::TEMPORARY_REDIRECT;
        reply.redirect = Some(format!("{}/sink", f.issuer));
    }
    assert_eq!(f.discover().await.unwrap_err(), Error::DiscoveryUnavailable);
    assert_eq!(f.state.requests.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn metadata_is_bounded_with_and_without_content_length() {
    let f = Fixture::start().await;
    for chunked in [false, true] {
        {
            let mut reply = f.state.reply.lock().unwrap();
            reply.body = "x".repeat(256 * 1024 + 1);
            reply.chunked = chunked;
        }
        assert_eq!(f.discover().await.unwrap_err(), Error::DiscoveryTooLarge);
    }
}

#[tokio::test]
async fn malformed_or_incomplete_metadata_returns_a_safe_error() {
    let f = Fixture::start().await;
    for body in ["not-json", "{}", r#"{"issuer":"secret from issuer"}"#] {
        f.state.reply.lock().unwrap().body = body.to_owned();
        let error = f.discover().await.unwrap_err();
        assert_eq!(error, Error::InvalidDiscovery);
        assert!(!format!("{error:?} {error}").contains("secret"));
    }
    f.state.reply.lock().unwrap().status = StatusCode::UNAUTHORIZED;
    assert_eq!(f.discover().await.unwrap_err(), Error::DiscoveryUnavailable);
}

#[tokio::test]
async fn pkce_requires_s256_and_authorization_code() {
    let mut f = Fixture::start().await;
    f.update(|doc| doc["code_challenge_methods_supported"] = json!(["plain"]));
    assert_eq!(f.discover().await.unwrap_err(), Error::PkceUnsupported);
    f.update(|doc| {
        doc["code_challenge_methods_supported"] = json!(["S256"]);
        doc["grant_types_supported"] = json!(["client_credentials"]);
    });
    assert_eq!(f.discover().await.unwrap_err(), Error::PkceUnsupported);
    f.update(|doc| doc["grant_types_supported"] = json!([]));
    assert_eq!(f.discover().await.unwrap_err(), Error::PkceUnsupported);
}

#[tokio::test]
async fn optional_metadata_is_supported_but_explicitly_missing_openid_is_rejected() {
    let mut f = Fixture::start().await;
    f.update(|doc| {
        for field in [
            "userinfo_endpoint",
            "end_session_endpoint",
            "introspection_endpoint",
            "grant_types_supported",
            "scopes_supported",
        ] {
            doc.as_object_mut().unwrap().remove(field);
        }
    });
    let provider = f.discover().await.unwrap();
    assert!(provider.userinfo_endpoint().is_none());
    assert!(provider.end_session_endpoint().is_none());
    assert!(provider.introspection_endpoint().is_none());
    assert!(provider.login_scopes().unwrap().omitted.is_empty());
    f.update(|doc| doc["scopes_supported"] = json!(["profile"]));
    assert_eq!(
        f.discover().await.unwrap().login_scopes().unwrap_err(),
        Error::OpenIdUnsupported
    );
}

#[tokio::test]
async fn device_login_requires_both_endpoint_and_grant() {
    let mut f = Fixture::start().await;
    f.update(|doc| doc["device_authorization_endpoint"] = json!("https://auth.example/device"));
    assert!(
        f.discover()
            .await
            .unwrap()
            .device_authorization_endpoint()
            .is_none()
    );
    f.update(|doc| {
        doc["grant_types_supported"] = json!([
            "authorization_code",
            "urn:ietf:params:oauth:grant-type:device_code"
        ])
    });
    assert_eq!(
        f.discover()
            .await
            .unwrap()
            .device_authorization_endpoint()
            .unwrap()
            .path(),
        "/device"
    );
    f.update(|doc| {
        doc.as_object_mut()
            .unwrap()
            .remove("device_authorization_endpoint");
    });
    assert!(
        f.discover()
            .await
            .unwrap()
            .device_authorization_endpoint()
            .is_none()
    );
}

#[tokio::test]
async fn pkce_roundtrip_preserves_nonce_and_encodes_public_client_exchange() {
    let f = Fixture::start().await;
    let provider = f.discover().await.unwrap();
    let attempt = AuthorizationAttempt::new(&provider, "cyber-cli", redirect()).unwrap();
    let params: HashMap<_, _> = attempt
        .authorization_url()
        .query_pairs()
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    assert_eq!(params["response_type"], "code");
    assert_eq!(params["code_challenge_method"], "S256");
    assert_eq!(params["redirect_uri"], redirect().as_str());
    assert!(params["scope"].contains("cyber:collaborate"));
    assert!(!params.contains_key("code_verifier"));
    assert_eq!(URL_SAFE_NO_PAD.decode(&params["state"]).unwrap().len(), 32);
    assert_eq!(URL_SAFE_NO_PAD.decode(&params["nonce"]).unwrap().len(), 32);
    let pasted = callback(&attempt, "code=a%2Bb%26c");
    let grant = attempt.finish(&pasted).unwrap();
    assert_eq!(grant.issuer(), provider.issuer());
    assert_eq!(grant.token_endpoint(), provider.token_endpoint());
    assert_eq!(grant.expected_nonce(), params["nonce"]);
    let form: HashMap<_, _> = grant.token_form().unwrap().into_iter().collect();
    assert_eq!(form["code"], "a+b&c");
    assert_eq!(form["client_id"], "cyber-cli");
    assert_eq!(form["grant_type"], "authorization_code");
    assert!(!form.contains_key("client_secret"));
    assert_eq!(
        URL_SAFE_NO_PAD.decode(form["code_verifier"]).unwrap().len(),
        64
    );
    assert_eq!(
        URL_SAFE_NO_PAD.encode(Sha256::digest(form["code_verifier"].as_bytes())),
        params["code_challenge"]
    );
}

#[tokio::test]
async fn attempts_use_independent_secrets() {
    let f = Fixture::start().await;
    let provider = f.discover().await.unwrap();
    let mut seen = HashSet::new();
    for _ in 0..16 {
        let attempt = AuthorizationAttempt::new(&provider, "cyber-cli", redirect()).unwrap();
        for (key, value) in attempt.authorization_url().query_pairs() {
            if ["state", "nonce", "code_challenge"].contains(&key.as_ref()) {
                assert!(seen.insert(value.into_owned()));
            }
        }
    }
}

#[tokio::test]
async fn callback_requires_matching_state_and_exact_redirect() {
    let f = Fixture::start().await;
    let provider = f.discover().await.unwrap();
    for response in [
        "http://127.0.0.1:53682/callback?code=secret",
        "http://127.0.0.1:53682/callback?state=wrong&code=secret",
    ] {
        let attempt = AuthorizationAttempt::new(&provider, "cyber-cli", redirect()).unwrap();
        assert!(matches!(
            attempt.finish(response),
            Err(Error::StateMismatch)
        ));
    }
    for replacement in [
        "http://127.0.0.1:53683/callback",
        "http://localhost:53682/callback",
        "http://127.0.0.1:53682/other",
        "https://127.0.0.1:53682/callback",
        "http://attacker.example/callback",
    ] {
        let attempt = AuthorizationAttempt::new(&provider, "cyber-cli", redirect()).unwrap();
        let pasted = callback(&attempt, "code=secret")
            .replace("http://127.0.0.1:53682/callback", replacement);
        assert!(matches!(
            attempt.finish(&pasted),
            Err(Error::InvalidCallback)
        ));
    }
}

#[tokio::test]
async fn duplicate_and_ambiguous_callback_parameters_are_rejected() {
    let f = Fixture::start().await;
    let provider = f.discover().await.unwrap();
    for extra in [
        "code=one&code=two",
        "code=one&state=duplicate",
        "code=one&error=denied",
        "error=denied&error=other",
        "code=",
        "",
        "code=one#fragment",
    ] {
        let attempt = AuthorizationAttempt::new(&provider, "cyber-cli", redirect()).unwrap();
        let pasted = callback(&attempt, extra);
        assert!(
            matches!(attempt.finish(&pasted), Err(Error::InvalidCallback)),
            "{extra}"
        );
    }
}

#[tokio::test]
async fn denial_is_safe_and_validated_for_state_first() {
    let f = Fixture::start().await;
    let provider = f.discover().await.unwrap();
    let attempt = AuthorizationAttempt::new(&provider, "cyber-cli", redirect()).unwrap();
    let pasted = callback(&attempt, "error=secret-error&error_description=secret");
    let error = attempt.finish(&pasted).err().unwrap();
    assert_eq!(error, Error::AuthorizationDenied);
    assert!(!format!("{error:?} {error}").contains("secret"));
}

#[tokio::test]
async fn invalid_clients_and_redirects_cannot_start_login() {
    let f = Fixture::start().await;
    let provider = f.discover().await.unwrap();
    assert!(matches!(
        AuthorizationAttempt::new(&provider, "  ", redirect()),
        Err(Error::InvalidClient)
    ));
    for url in [
        "https://127.0.0.1:1234/callback",
        "http://localhost:1234/callback",
        "http://192.168.1.1:1234/callback",
        "http://127.0.0.1:1234/other",
        "http://127.0.0.1:1234/callback?state=evil",
        "http://user:secret@127.0.0.1:1234/callback",
        "http://127.0.0.1:1234/callback#fragment",
    ] {
        assert!(
            matches!(
                AuthorizationAttempt::new(&provider, "cyber-cli", Url::parse(url).unwrap()),
                Err(Error::InvalidCallback)
            ),
            "{url}"
        );
    }
    assert!(
        AuthorizationAttempt::new(
            &provider,
            "cyber-cli",
            Url::parse("http://[::1]:1234/callback").unwrap()
        )
        .is_ok()
    );
}

#[tokio::test]
async fn provider_query_cannot_override_oauth_security_parameters() {
    let mut f = Fixture::start().await;
    f.update(|doc| {
        doc["authorization_endpoint"] = json!("https://auth.example/authorize?state=attacker")
    });
    let provider = f.discover().await.unwrap();
    assert!(matches!(
        AuthorizationAttempt::new(&provider, "cyber-cli", redirect()),
        Err(Error::InvalidDiscovery)
    ));
    f.update(|doc| {
        doc["authorization_endpoint"] = json!("https://auth.example/authorize?tenant=acme")
    });
    let attempt =
        AuthorizationAttempt::new(&f.discover().await.unwrap(), "cyber-cli", redirect()).unwrap();
    assert!(
        attempt
            .authorization_url()
            .query_pairs()
            .any(|(k, v)| k == "tenant" && v == "acme")
    );
}

#[tokio::test]
async fn loopback_listener_accepts_a_real_callback_without_reflecting_secrets() {
    let f = Fixture::start().await;
    let provider = f.discover().await.unwrap();
    let flow = cyber_account::LoopbackLogin::start(&provider, "cyber-cli")
        .await
        .unwrap();
    let params: HashMap<_, _> = flow
        .authorization_url()
        .query_pairs()
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    let callback = format!(
        "{}?state={}&code=private-code",
        params["redirect_uri"], params["state"]
    );
    let client = reqwest::Client::new();
    let (response, grant) = tokio::join!(client.get(callback).send(), flow.finish());
    let body = response.unwrap().text().await.unwrap();
    assert!(!body.contains("private-code"));
    assert!(!body.contains(&params["state"]));
    assert_eq!(
        grant
            .unwrap()
            .token_form()
            .unwrap()
            .into_iter()
            .find(|(k, _)| *k == "code")
            .unwrap()
            .1,
        "private-code"
    );
}

#[tokio::test]
async fn headless_paste_and_browser_callback_share_single_use_state_validation() {
    let f = Fixture::start().await;
    let provider = f.discover().await.unwrap();
    let flow = cyber_account::LoopbackLogin::start(&provider, "cyber-cli")
        .await
        .unwrap();
    let params: HashMap<_, _> = flow
        .authorization_url()
        .query_pairs()
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    let callback = format!(
        "{}?state={}&code=pasted",
        params["redirect_uri"], params["state"]
    );
    let handle = flow.callback_handle();
    handle.submit(&callback).unwrap();
    assert_eq!(handle.submit(&callback), Err(Error::LoginExpired));
    assert!(flow.finish().await.is_ok());
    let failed = cyber_account::LoopbackLogin::start(&provider, "cyber-cli")
        .await
        .unwrap();
    assert!(matches!(
        failed
            .callback_handle()
            .submit("http://127.0.0.1/callback?code=injected"),
        Err(Error::InvalidCallback)
    ));
    assert!(matches!(failed.finish().await, Err(Error::InvalidCallback)));
}

#[tokio::test]
async fn discovery_cache_reuses_current_metadata_and_refetches_expired_or_wrong_issuer() {
    let f = Fixture::start().await;
    let cache = tempfile::tempdir().unwrap();
    let client = DiscoveryClient::new(Transport::AllowLoopbackHttp).unwrap();
    client
        .discover_cached(&f.issuer, cache.path())
        .await
        .unwrap();
    client
        .discover_cached(&f.issuer, cache.path())
        .await
        .unwrap();
    assert_eq!(f.state.requests.load(Ordering::SeqCst), 1);
    let path = std::fs::read_dir(cache.path())
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let mut record: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    record["checked_at"] = json!(0);
    std::fs::write(&path, record.to_string()).unwrap();
    client
        .discover_cached(&f.issuer, cache.path())
        .await
        .unwrap();
    assert_eq!(f.state.requests.load(Ordering::SeqCst), 2);
    record = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    record["document"]["issuer"] = json!("https://attacker.example");
    std::fs::write(&path, record.to_string()).unwrap();
    client
        .discover_cached(&f.issuer, cache.path())
        .await
        .unwrap();
    assert_eq!(f.state.requests.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn corrupt_discovery_cache_never_substitutes_for_failed_network_validation() {
    let f = Fixture::start().await;
    let cache = tempfile::tempdir().unwrap();
    let client = DiscoveryClient::new(Transport::AllowLoopbackHttp).unwrap();
    client
        .discover_cached(&f.issuer, cache.path())
        .await
        .unwrap();
    let path = std::fs::read_dir(cache.path())
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    std::fs::write(path, "broken").unwrap();
    f.state.reply.lock().unwrap().status = StatusCode::SERVICE_UNAVAILABLE;
    assert_eq!(
        client
            .discover_cached(&f.issuer, cache.path())
            .await
            .unwrap_err(),
        Error::DiscoveryUnavailable
    );
}
