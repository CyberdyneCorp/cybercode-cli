//! MCP OAuth 2.1 acquisition against in-process fakes on 127.0.0.1: protected-resource and
//! authorization-server discovery (fail closed), dynamic client registration, the loopback
//! callback listener, PKCE S256 with RFC 8707 `resource`, token exchange/refresh and the
//! private credential store. Nothing here reaches the internet.
#[path = "support/mcp_http.rs"]
mod mcp_http;
#[path = "support/oauth_as.rs"]
mod oauth_as;

use std::time::Duration;

use cyber_core::config::McpOAuth;
use cyber_tools::mcp::oauth::{
    self, AuthChallenge, CallbackListener, CredentialKey, FileCredentialStore, McpCredentialStore,
    MemoryCredentialStore, MemoryVault, OAuthClient, OAuthError, Pkce, StoredCredential,
    TokenRequest, TokenSet, VaultCredentialStore,
};
use cyber_tools::mcp::{
    HeaderSet, HttpConnection, HttpFailure, HttpTarget, McpError, RemoteTransport, StartupOptions,
};
use oauth_as::FakeAs;
use reqwest::Url;
use serde_json::json;

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap()
}

/// A fake whose own `/mcp` path is the protected resource, with AS metadata served.
async fn resource() -> (FakeAs, Url) {
    let fake = FakeAs::start().await;
    let url = Url::parse(&format!("{}/mcp", fake.origin)).unwrap();
    fake.serve_metadata();
    (fake, url)
}

fn challenge(resource_metadata: Option<String>) -> AuthChallenge {
    AuthChallenge {
        resource_metadata,
        scope: None,
        error: None,
    }
}

#[tokio::test]
async fn discovery_uses_resource_metadata_then_path_suffixed_then_root_well_known() {
    let (fake, url) = resource().await;
    fake.serve("/prm", fake.protected_resource(url.as_str()));
    let hinted = challenge(Some(format!("{}/prm", fake.origin)));
    let found = oauth::discover(&client(), &url, Some(&hinted))
        .await
        .unwrap();
    assert_eq!(found.resource, url);
    assert_eq!(found.server.issuer.as_str(), format!("{}/", fake.origin));
    assert_eq!(
        found.server.token_endpoint.as_str(),
        format!("{}/token", fake.origin)
    );
    assert!(found.server.registration_endpoint.is_some());
    assert_eq!(found.scopes_supported, vec!["mcp.read".to_string()]);
    assert_eq!(
        fake.paths(),
        ["/prm", "/.well-known/oauth-authorization-server"]
    );

    let (fake, url) = resource().await;
    fake.serve(
        "/.well-known/oauth-protected-resource/mcp",
        fake.protected_resource(url.as_str()),
    );
    oauth::discover(&client(), &url, None).await.unwrap();
    assert_eq!(fake.paths()[0], "/.well-known/oauth-protected-resource/mcp");

    let (fake, url) = resource().await;
    fake.serve(
        "/.well-known/oauth-protected-resource",
        fake.protected_resource(url.as_str()),
    );
    oauth::discover(&client(), &url, None).await.unwrap();
    assert_eq!(
        fake.paths()[..2],
        [
            "/.well-known/oauth-protected-resource/mcp",
            "/.well-known/oauth-protected-resource"
        ]
    );

    // RFC 8414 metadata absent: the OpenID Connect discovery document is used.
    let fake = FakeAs::start().await;
    let url = Url::parse(&format!("{}/mcp", fake.origin)).unwrap();
    fake.serve(
        "/.well-known/oauth-protected-resource",
        fake.protected_resource(url.as_str()),
    );
    fake.serve("/.well-known/openid-configuration", fake.metadata());
    oauth::discover(&client(), &url, None).await.unwrap();
    assert!(
        fake.paths()
            .contains(&"/.well-known/openid-configuration".to_string())
    );
}

/// A metadata field to replace (or remove, for `Null`) and the expected refusal.
type MetadataCase = (&'static str, serde_json::Value, fn(&OAuthError) -> bool);

#[tokio::test]
async fn discovery_fails_closed_on_resource_issuer_endpoint_or_pkce_mismatch() {
    // The protected resource names another resource: no AS request is made.
    let (fake, url) = resource().await;
    fake.serve(
        "/.well-known/oauth-protected-resource",
        fake.protected_resource(&format!("{}/other", fake.origin)),
    );
    let error = oauth::discover(&client(), &url, None).await.unwrap_err();
    assert!(matches!(error, OAuthError::ResourceMismatch), "{error:?}");
    assert!(
        !fake
            .paths()
            .iter()
            .any(|path| path.contains("authorization-server"))
    );

    let cases: [MetadataCase; 5] = [
        ("issuer", json!("http://127.0.0.1:1"), |e| {
            matches!(e, OAuthError::IssuerMismatch)
        }),
        ("token_endpoint", json!("http://example.test/token"), |e| {
            matches!(e, OAuthError::InsecureEndpoint)
        }),
        (
            "authorization_endpoint",
            json!("ftp://127.0.0.1/authorize"),
            |e| matches!(e, OAuthError::InsecureEndpoint),
        ),
        ("code_challenge_methods_supported", json!(["plain"]), |e| {
            matches!(e, OAuthError::NoPkce)
        }),
        (
            "code_challenge_methods_supported",
            serde_json::Value::Null,
            |e| matches!(e, OAuthError::NoPkce),
        ),
    ];
    for (field, value, expected) in cases {
        let (fake, url) = resource().await;
        fake.serve(
            "/.well-known/oauth-protected-resource",
            fake.protected_resource(url.as_str()),
        );
        let mut metadata = fake.metadata();
        if value.is_null() {
            metadata.as_object_mut().unwrap().remove(field);
        } else {
            metadata[field] = value;
        }
        fake.serve("/.well-known/oauth-authorization-server", metadata);
        let error = oauth::discover(&client(), &url, None).await.unwrap_err();
        assert!(expected(&error), "{field}: {error:?}");
        assert!(!error.to_string().contains("example.test"));
    }

    // A non-loopback plain-HTTP metadata hint is refused before any request.
    let (fake, url) = resource().await;
    let hinted = challenge(Some("http://example.test/prm".into()));
    let error = oauth::discover(&client(), &url, Some(&hinted))
        .await
        .unwrap_err();
    assert!(matches!(error, OAuthError::InsecureEndpoint), "{error:?}");
    assert!(fake.paths().is_empty());
}

#[tokio::test]
async fn registration_is_dynamic_unless_a_client_id_is_configured() {
    let (fake, url) = resource().await;
    fake.serve(
        "/.well-known/oauth-protected-resource",
        fake.protected_resource(url.as_str()),
    );
    let found = oauth::discover(&client(), &url, None).await.unwrap();
    let redirect = "http://127.0.0.1:4567/mcp/oauth/callback";
    let registered = oauth::client_for(&client(), &found.server, &McpOAuth::default(), redirect)
        .await
        .unwrap();
    assert_eq!(registered.client_id, oauth_as::REGISTERED_CLIENT);
    assert_eq!(registered.client_secret, None);
    let request = &fake.requests_to("/register")[0];
    assert_eq!(request.json["redirect_uris"], json!([redirect]));
    assert_eq!(request.json["token_endpoint_auth_method"], "none");
    assert_eq!(
        request.json["grant_types"],
        json!(["authorization_code", "refresh_token"])
    );

    let configured = McpOAuth {
        client_id: Some("configured".into()),
        client_secret: Some("configured-secret".into()),
        ..McpOAuth::default()
    };
    let fixed = oauth::client_for(&client(), &found.server, &configured, redirect)
        .await
        .unwrap();
    assert_eq!(fixed.client_id, "configured");
    assert_eq!(fixed.client_secret.as_deref(), Some("configured-secret"));
    assert!(!format!("{fixed:?}").contains("configured-secret"));
    assert_eq!(fake.requests_to("/register").len(), 1);

    let mut server = found.server.clone();
    server.registration_endpoint = None;
    let error = oauth::client_for(&client(), &server, &McpOAuth::default(), redirect)
        .await
        .unwrap_err();
    assert!(
        matches!(error, OAuthError::NeedsClientRegistration),
        "{error:?}"
    );
    assert_eq!(fake.requests_to("/register").len(), 1);
}

#[test]
fn pkce_verifier_is_random_unreserved_and_s256_bound() {
    use base64::Engine;
    use sha2::Digest;
    let first = Pkce::generate().unwrap();
    let second = Pkce::generate().unwrap();
    assert_ne!(first.verifier, second.verifier);
    assert!((43..=128).contains(&first.verifier.len()));
    assert!(
        first
            .verifier
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-._~".contains(&b))
    );
    assert_eq!(
        first.challenge,
        base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(sha2::Sha256::digest(first.verifier.as_bytes()))
    );
    let state = oauth::random_state().unwrap();
    assert!(state.len() >= 43);
    assert_ne!(state, oauth::random_state().unwrap());
}

/// Run `wait` while `drive` plays the browser against the redirect URI.
async fn callback<F, Fut>(
    port: Option<u16>,
    timeout: Duration,
    drive: F,
) -> (Result<String, OAuthError>, u16)
where
    F: FnOnce(String) -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    let listener = CallbackListener::bind(port).await.unwrap();
    assert_eq!(
        listener.local_addr().ip(),
        std::net::IpAddr::from([127, 0, 0, 1])
    );
    let redirect = listener.redirect_uri();
    let bound = listener.local_addr().port();
    assert_eq!(
        redirect,
        format!("http://127.0.0.1:{bound}/mcp/oauth/callback")
    );
    let (result, ()) = tokio::join!(listener.wait("expected-state", timeout), drive(redirect));
    (result, bound)
}

async fn closed(port: u16) -> bool {
    tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .is_err()
}

#[tokio::test]
async fn loopback_callback_accepts_only_its_path_and_state_once() {
    let (result, port) = callback(None, Duration::from_secs(10), |redirect| async move {
        let base = redirect.trim_end_matches("/mcp/oauth/callback").to_string();
        let (status, _, _) = oauth_as::get(&format!("{base}/favicon.ico")).await;
        assert_eq!(status, 404);
        let (status, _, _) =
            oauth_as::get(&format!("{base}/other?code=wrong&state=expected-state")).await;
        assert_eq!(status, 404);
        let (status, _, body) =
            oauth_as::get(&format!("{redirect}?code=the-code&state=expected-state")).await;
        assert_eq!(status, 200);
        assert!(!body.contains("the-code"));
    })
    .await;
    assert_eq!(result.unwrap(), "the-code");
    // The listener is joined: a second callback cannot connect.
    assert!(closed(port).await);

    let (result, port) = callback(None, Duration::from_secs(10), |redirect| async move {
        let (status, _, _) = oauth_as::get(&format!("{redirect}?code=the-code&state=forged")).await;
        assert_eq!(status, 400);
    })
    .await;
    assert!(
        matches!(result, Err(OAuthError::StateMismatch)),
        "{result:?}"
    );
    assert!(closed(port).await);

    let (result, _) = callback(None, Duration::from_secs(10), |redirect| async move {
        let (status, _, _) = oauth_as::get(&format!(
            "{redirect}?error=access_denied&error_description=%3Cscript%3E&state=expected-state"
        ))
        .await;
        assert_eq!(status, 400);
    })
    .await;
    match result {
        Err(OAuthError::Denied(code)) => assert_eq!(code, "access_denied"),
        other => panic!("{other:?}"),
    }

    let (result, port) = callback(None, Duration::from_millis(200), |_| async {}).await;
    assert!(
        matches!(result, Err(OAuthError::CallbackTimeout)),
        "{result:?}"
    );
    assert!(closed(port).await);

    // A configured port is used exactly.
    let free = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let (result, port) = callback(Some(free), Duration::from_secs(10), |redirect| async move {
        oauth_as::get(&format!("{redirect}?code=c&state=expected-state")).await;
    })
    .await;
    assert_eq!(port, free);
    assert_eq!(result.unwrap(), "c");
}

#[tokio::test]
async fn authorization_code_flow_binds_pkce_and_resource_and_refresh_rotates() {
    let (fake, url) = resource().await;
    fake.serve(
        "/.well-known/oauth-protected-resource",
        fake.protected_resource(url.as_str()),
    );
    fake.expect_resource(url.as_str());
    let found = oauth::discover(&client(), &url, None).await.unwrap();
    let listener = CallbackListener::bind(None).await.unwrap();
    let redirect = listener.redirect_uri();
    let registered = oauth::client_for(&client(), &found.server, &McpOAuth::default(), &redirect)
        .await
        .unwrap();
    let pkce = Pkce::generate().unwrap();
    let state = oauth::random_state().unwrap();
    let authorize = oauth::authorize_url(
        &found.server,
        &registered.client_id,
        &redirect,
        &pkce,
        &state,
        Some("mcp.read"),
        &found.resource,
    );
    let browser = async {
        let (status, location, body) = oauth_as::get(authorize.as_str()).await;
        assert_eq!(status, 302, "{body}");
        let (status, _, _) = oauth_as::get(&location.unwrap()).await;
        assert_eq!(status, 200);
    };
    let (code, ()) = tokio::join!(listener.wait(&state, Duration::from_secs(10)), browser);
    let code = code.unwrap();
    let authorize_seen = &fake.requests_to("/authorize")[0];
    assert_eq!(authorize_seen.query["resource"], url.as_str());
    assert_eq!(authorize_seen.query["code_challenge"], pkce.challenge);
    assert_eq!(authorize_seen.query["scope"], "mcp.read");

    let request = TokenRequest {
        client: &registered,
        endpoint: &found.server.token_endpoint,
        resource: &found.resource,
    };
    // A wrong verifier is refused and burns nothing usable.
    let wrong = oauth::exchange_code(
        &client(),
        &request,
        &code,
        "x".repeat(43).as_str(),
        &redirect,
    )
    .await
    .unwrap_err();
    assert!(
        matches!(&wrong, OAuthError::TokenRejected(code) if code == "invalid_grant"),
        "{wrong:?}"
    );
    assert!(!wrong.to_string().contains(&code));

    // The fake consumes a code once, so authorize again for the real exchange.
    let listener = CallbackListener::bind(None).await.unwrap();
    let redirect = listener.redirect_uri();
    let authorize = oauth::authorize_url(
        &found.server,
        &registered.client_id,
        &redirect,
        &pkce,
        &state,
        Some("mcp.read"),
        &found.resource,
    );
    let browser = async {
        let (_, location, _) = oauth_as::get(authorize.as_str()).await;
        oauth_as::get(&location.unwrap()).await;
    };
    let (code, ()) = tokio::join!(listener.wait(&state, Duration::from_secs(10)), browser);
    let tokens = oauth::exchange_code(
        &client(),
        &request,
        &code.unwrap(),
        &pkce.verifier,
        &redirect,
    )
    .await
    .unwrap();
    assert!(tokens.access_token.starts_with(oauth_as::ACCESS_PREFIX));
    assert_eq!(tokens.scopes, vec!["mcp.read".to_string()]);
    assert!(tokens.expires_at.is_some());
    assert!(!format!("{tokens:?}").contains(oauth_as::ACCESS_PREFIX));
    let token_seen = fake.requests_to("/token").pop().unwrap();
    assert_eq!(token_seen.form["code_verifier"], pkce.verifier);
    assert_eq!(token_seen.form["resource"], url.as_str());

    let old = tokens.refresh_token.clone().unwrap();
    let refreshed = oauth::refresh(&client(), &request, &old, None)
        .await
        .unwrap();
    assert_ne!(refreshed.access_token, tokens.access_token);
    assert_ne!(refreshed.refresh_token.as_deref(), Some(old.as_str()));
    assert_eq!(
        fake.requests_to("/token").pop().unwrap().form["resource"],
        url.as_str()
    );
    // The rotated-out refresh token is no longer accepted.
    let error = oauth::refresh(&client(), &request, &old, None)
        .await
        .unwrap_err();
    assert!(matches!(error, OAuthError::TokenRejected(_)), "{error:?}");
}

fn credential(token: &str) -> StoredCredential {
    StoredCredential {
        tokens: TokenSet {
            access_token: token.into(),
            refresh_token: Some(format!("{token}-refresh")),
            expires_at: Some(4_000_000_000),
            issued_at: None,
            scopes: vec!["mcp.read".into()],
        },
        client: OAuthClient {
            client_id: "dyn-client".into(),
            client_secret: None,
        },
        token_endpoint: "https://as.example.test/token".into(),
    }
}

fn key(url: &str) -> CredentialKey {
    CredentialKey {
        server: "remote".into(),
        url: url.into(),
        issuer: "https://as.example.test/".into(),
    }
}

fn exercise(store: &dyn McpCredentialStore) {
    let url = "https://mcp.example.test/mcp";
    assert!(store.get(&key(url)).unwrap().is_none());
    store.put(&key(url), &credential("first")).unwrap();
    store
        .put(&key("https://mcp.example.test/mcp/"), &credential("other"))
        .unwrap();
    assert_eq!(
        store.get(&key(url)).unwrap().unwrap().tokens.access_token,
        "first"
    );
    store.put(&key(url), &credential("second")).unwrap();
    let found = store.find("remote", url).unwrap().unwrap();
    assert_eq!(found.0, key(url));
    assert_eq!(found.1.tokens.access_token, "second");
    let mut other_issuer = key(url);
    other_issuer.issuer = "https://elsewhere.test/".into();
    assert!(store.get(&other_issuer).unwrap().is_none());
    assert!(store.remove("remote", url).unwrap());
    assert!(!store.remove("remote", url).unwrap());
    assert!(store.get(&key(url)).unwrap().is_none());
    assert_eq!(
        store
            .get(&key("https://mcp.example.test/mcp/"))
            .unwrap()
            .unwrap()
            .tokens
            .access_token,
        "other"
    );
}

#[test]
fn credential_stores_round_trip_by_exact_url_and_issuer() {
    exercise(&MemoryCredentialStore::default());
    {
        // The OS keyring layout, over the in-memory vault: nothing here touches a real
        // Keychain or Credential Manager.
        let dir = tempfile::tempdir().unwrap();
        let index = dir.path().join("data/mcp-auth-index.json");
        let vault = std::sync::Arc::new(MemoryVault::default());
        exercise(&VaultCredentialStore::new(index.clone(), vault.clone()));
        let text = std::fs::read_to_string(&index).unwrap();
        assert!(!text.contains("mcp.example.test"), "{text}");
        // Neither the remaining token nor its refresh token is in the index.
        assert!(!text.contains("other-refresh"), "{text}");
        assert!(!text.contains("\"other\""), "{text}");
        for (account, secret) in vault.entries() {
            assert!(!account.contains("example"), "{account}");
            assert!(secret.len() <= 2560);
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("data/mcp-auth.json");
        let store = FileCredentialStore::new(path.clone());
        exercise(&store);
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("https://mcp.example.test/mcp/"));
        // A credential file readable by others is refused rather than trusted.
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(matches!(
            store.get(&key("https://mcp.example.test/mcp/")),
            Err(OAuthError::Storage(_))
        ));
    }
    #[cfg(windows)]
    {
        let dir = tempfile::tempdir().unwrap();
        let store = FileCredentialStore::new(dir.path().join("mcp-auth.json"));
        assert!(matches!(
            store.put(&key("https://mcp.example.test/mcp"), &credential("x")),
            Err(OAuthError::Storage(_))
        ));
        assert!(!dir.path().join("mcp-auth.json").exists());
    }
}

#[tokio::test]
async fn unauthorized_carries_the_parsed_challenge_on_streamable_and_legacy_paths() {
    let challenge_header = r#"Bearer realm="mcp", resource_metadata="http://127.0.0.1:9/prm", scope="mcp.read mcp.write", error="invalid_token""#;
    for (behavior, transport) in [
        (
            mcp_http::Behavior::default(),
            RemoteTransport::StreamableHttp,
        ),
        (
            mcp_http::Behavior {
                legacy: true,
                ..Default::default()
            },
            RemoteTransport::Sse,
        ),
    ] {
        let http = mcp_http::McpHttp::start(behavior).await;
        http.require_authorization(Some("Bearer expected"));
        http.set_challenge(Some(challenge_header));
        let error = match HttpConnection::connect_with_options(
            HttpTarget {
                client: client(),
                url: Url::parse(&http.url).unwrap(),
                headers: HeaderSet::default(),
            },
            "remote",
            &Default::default(),
            Duration::from_secs(5),
            StartupOptions {
                roots: None,
                sampling: false,
                cancellation: None,
            },
            transport,
        )
        .await
        {
            Ok(_) => panic!("connected without authorization"),
            Err(error) => error,
        };
        assert!(error.needs_auth(), "{transport:?}: {}", error.error);
        let McpError::Http(HttpFailure::Unauthorized { status, challenge }) = &error.error else {
            panic!("{transport:?}: {}", error.error);
        };
        assert_eq!(*status, 401);
        let challenge = challenge.clone().unwrap();
        assert_eq!(
            challenge.resource_metadata.as_deref(),
            Some("http://127.0.0.1:9/prm")
        );
        assert_eq!(challenge.scope.as_deref(), Some("mcp.read mcp.write"));
        assert_eq!(challenge.error.as_deref(), Some("invalid_token"));
        let shown = error.error.to_string();
        assert!(
            !shown.contains("prm") && !shown.contains("realm"),
            "{shown}"
        );
    }
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

/// A credential minted by `issuer` (an origin), refreshed at its `/token`.
fn issued_by(issuer: &str, refresh: &str, expires_in: u64) -> (CredentialKey, StoredCredential) {
    let key = CredentialKey {
        server: "remote".into(),
        url: "http://127.0.0.1:9/mcp".into(),
        issuer: format!("{issuer}/"),
    };
    let credential = StoredCredential {
        tokens: TokenSet {
            access_token: format!("PRIVATE-ACCESS-{refresh}"),
            refresh_token: Some(refresh.into()),
            expires_at: Some(unix_now() + expires_in),
            issued_at: None,
            scopes: vec!["mcp.read".into()],
        },
        client: OAuthClient {
            client_id: oauth_as::REGISTERED_CLIENT.into(),
            client_secret: Some(format!("PRIVATE-SECRET-{refresh}")),
        },
        token_endpoint: format!("{issuer}/token"),
    };
    (key, credential)
}

fn resource_url() -> Url {
    Url::parse("http://127.0.0.1:9/mcp").unwrap()
}

async fn token_source(store: std::sync::Arc<dyn McpCredentialStore>) -> oauth::TokenSource {
    oauth::TokenSource::load(store, "remote", &resource_url())
        .await
        .unwrap()
        .expect("credential is stored")
}

#[tokio::test]
async fn refresh_follows_a_credential_moved_to_another_issuer() {
    let old = FakeAs::start().await;
    let new = FakeAs::start().await;
    new.issue_refresh("PRIVATE-NEW-REFRESH");
    let store = std::sync::Arc::new(MemoryCredentialStore::default());
    let (key, credential) = issued_by(&old.origin, "PRIVATE-OLD-REFRESH", 3600);
    store.put(&key, &credential).unwrap();
    let mut source = token_source(store.clone()).await;
    // `cyber mcp auth` re-ran against another issuer, whose token is already due.
    let (moved, credential) = issued_by(&new.origin, "PRIVATE-NEW-REFRESH", 60);
    store.put(&moved, &credential).unwrap();
    source.refresh(&client()).await.unwrap();
    // The new issuer's refresh token and secret never reach the old issuer.
    assert!(old.seen().is_empty(), "{:?}", old.paths());
    let posts = new.token_posts();
    assert_eq!(posts.len(), 1);
    assert_eq!(posts[0].form["refresh_token"], "PRIVATE-NEW-REFRESH");
    let (stored_key, stored) = store
        .find("remote", resource_url().as_str())
        .unwrap()
        .unwrap();
    assert_eq!(stored_key, moved);
    let issued = format!("{}1", oauth_as::ACCESS_PREFIX);
    assert_eq!(stored.tokens.access_token, issued);
    assert_eq!(
        source.authorization().to_str().unwrap(),
        format!("Bearer {issued}")
    );
}

#[tokio::test]
async fn an_adopted_credential_is_validated_before_any_refresh() {
    let fake = FakeAs::start().await;
    fake.issue_refresh("PRIVATE-REFRESH");
    let store = std::sync::Arc::new(MemoryCredentialStore::default());
    let (key, credential) = issued_by(&fake.origin, "PRIVATE-REFRESH", 3600);
    store.put(&key, &credential).unwrap();
    let mut source = token_source(store.clone()).await;
    // A remote issuer whose stored token endpoint is a local service.
    let (mut moved, credential) = issued_by(&fake.origin, "PRIVATE-REFRESH", 60);
    moved.issuer = "https://as.example.test/".into();
    store.put(&moved, &credential).unwrap();
    assert!(matches!(
        source.sync().await,
        Err(OAuthError::InsecureEndpoint)
    ));
    assert!(matches!(
        source.refresh(&client()).await,
        Err(OAuthError::InsecureEndpoint)
    ));
    assert!(fake.seen().is_empty(), "{:?}", fake.paths());
}

#[tokio::test]
async fn refresh_of_a_removed_credential_fails_closed_and_stores_nothing() {
    let fake = FakeAs::start().await;
    fake.issue_refresh("PRIVATE-REFRESH");
    let store = std::sync::Arc::new(MemoryCredentialStore::default());
    let (key, credential) = issued_by(&fake.origin, "PRIVATE-REFRESH", 60);
    store.put(&key, &credential).unwrap();
    let mut source = token_source(store.clone()).await;
    assert_eq!(store.remove_server("remote").unwrap(), 1);
    assert!(matches!(
        source.refresh(&client()).await,
        Err(OAuthError::CredentialRemoved)
    ));
    assert!(fake.seen().is_empty(), "{:?}", fake.paths());
    assert!(
        store
            .find("remote", resource_url().as_str())
            .unwrap()
            .is_none()
    );
}

/// A fresh handle on one shared credential store, as another process would open it.
type Opener = Box<dyn Fn() -> std::sync::Arc<dyn McpCredentialStore> + Send + Sync>;

/// Handles on the owner-only file at `<dir>/mcp-auth.json`.
#[cfg(unix)]
fn file_opener(dir: &tempfile::TempDir) -> Opener {
    let path = dir.path().join("mcp-auth.json");
    Box::new(move || std::sync::Arc::new(FileCredentialStore::new(path.clone())))
}

/// Handles on the OS keyring layout over one in-memory vault (never a real keyring).
fn vault_opener(dir: &tempfile::TempDir) -> Opener {
    let index = dir.path().join("mcp-auth-index.json");
    let vault = std::sync::Arc::new(MemoryVault::default());
    Box::new(move || std::sync::Arc::new(VaultCredentialStore::new(index.clone(), vault.clone())))
}

/// No file in `dir` holds a token: the vault keeps them, and the file store has
/// forgotten them after logout.
fn assert_no_token_in_files(dir: &tempfile::TempDir) {
    for entry in std::fs::read_dir(dir.path()).unwrap() {
        let path = entry.unwrap().path();
        let text = String::from_utf8_lossy(&std::fs::read(&path).unwrap_or_default()).into_owned();
        assert!(!text.contains("TOKEN-SECRET"), "{}: {text}", path.display());
        assert!(
            !text.contains("PRIVATE-REFRESH"),
            "{}: {text}",
            path.display()
        );
    }
}

async fn logout_during_a_refresh_is_not_undone(open: Opener, dir: &tempfile::TempDir) {
    let fake = FakeAs::start().await;
    fake.issue_refresh("PRIVATE-REFRESH");
    let store = open();
    let (key, credential) = issued_by(&fake.origin, "PRIVATE-REFRESH", 60);
    store.put(&key, &credential).unwrap();
    let mut source = token_source(store.clone()).await;
    fake.hold_tokens();
    let refresh = tokio::spawn(async move { source.refresh(&client()).await });
    wait_token_posts(&fake, 1).await;
    // `cyber mcp logout` while the token request is in flight.
    assert_eq!(open().remove_server("remote").unwrap(), 1);
    fake.release_tokens();
    assert!(matches!(
        refresh.await.unwrap(),
        Err(OAuthError::CredentialRemoved)
    ));
    assert!(
        store
            .find("remote", resource_url().as_str())
            .unwrap()
            .is_none()
    );
    assert_no_token_in_files(dir);
}

#[cfg(unix)]
#[tokio::test]
async fn file_logout_during_a_refresh_is_not_undone() {
    let dir = tempfile::tempdir().unwrap();
    logout_during_a_refresh_is_not_undone(file_opener(&dir), &dir).await;
}

#[tokio::test]
async fn vault_logout_during_a_refresh_is_not_undone() {
    let dir = tempfile::tempdir().unwrap();
    logout_during_a_refresh_is_not_undone(vault_opener(&dir), &dir).await;
}

async fn concurrent_refreshes_present_a_rotating_refresh_token_once(open: Opener) {
    let fake = FakeAs::start().await;
    fake.issue_refresh("PRIVATE-REFRESH");
    let (key, credential) = issued_by(&fake.origin, "PRIVATE-REFRESH", 60);
    open().put(&key, &credential).unwrap();
    // Two holders of one credential, each with its own store handle, as two processes.
    let mut first = token_source(open()).await;
    let mut second = token_source(open()).await;
    assert!(first.due() && second.due());
    fake.hold_tokens();
    let first = tokio::spawn(async move { first.refresh(&client()).await.map(|()| first) });
    wait_token_posts(&fake, 1).await;
    let second = tokio::spawn(async move { second.refresh(&client()).await.map(|()| second) });
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(fake.token_posts().len(), 1, "the second refresh waits");
    fake.release_tokens();
    let first = first.await.unwrap().unwrap();
    let second = second.await.unwrap().unwrap();
    // The second adopted the rotated credential instead of replaying the old token.
    assert_eq!(fake.token_posts().len(), 1, "{:?}", fake.paths());
    let issued = format!("Bearer {}1", oauth_as::ACCESS_PREFIX);
    assert_eq!(first.authorization().to_str().unwrap(), issued);
    assert_eq!(second.authorization().to_str().unwrap(), issued);
    assert!(!first.due() && !second.due());
}

#[cfg(unix)]
#[tokio::test]
async fn file_concurrent_refreshes_present_a_rotating_refresh_token_once() {
    let dir = tempfile::tempdir().unwrap();
    concurrent_refreshes_present_a_rotating_refresh_token_once(file_opener(&dir)).await;
}

#[tokio::test]
async fn vault_concurrent_refreshes_present_a_rotating_refresh_token_once() {
    let dir = tempfile::tempdir().unwrap();
    concurrent_refreshes_present_a_rotating_refresh_token_once(vault_opener(&dir)).await;
}

async fn wait_token_posts(fake: &FakeAs, count: usize) {
    tokio::time::timeout(Duration::from_secs(10), async {
        while fake.token_posts().len() < count {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("token request never arrived");
}

/// A memory store whose `replace` waits at `gate` twice (on entry and before writing), as
/// a keyring write waiting on the user does. Each refresh lock it hands out reports its
/// release on `released`.
struct GatedStore {
    inner: MemoryCredentialStore,
    gate: std::sync::Arc<std::sync::Barrier>,
    released: std::sync::mpsc::Sender<()>,
}

/// A refresh lock that reports its release.
struct Reported(
    #[allow(dead_code)] oauth::RefreshLock,
    std::sync::mpsc::Sender<()>,
);

impl Drop for Reported {
    fn drop(&mut self) {
        let _ = self.1.send(());
    }
}

impl McpCredentialStore for GatedStore {
    fn get(&self, key: &CredentialKey) -> Result<Option<StoredCredential>, OAuthError> {
        self.inner.get(key)
    }
    fn find(
        &self,
        server: &str,
        url: &str,
    ) -> Result<Option<(CredentialKey, StoredCredential)>, OAuthError> {
        self.inner.find(server, url)
    }
    fn put(&self, key: &CredentialKey, credential: &StoredCredential) -> Result<(), OAuthError> {
        self.inner.put(key, credential)
    }
    fn replace(
        &self,
        key: &CredentialKey,
        previous: &TokenSet,
        credential: &StoredCredential,
    ) -> Result<bool, OAuthError> {
        self.gate.wait();
        self.gate.wait();
        self.inner.replace(key, previous, credential)
    }
    fn try_lock_refresh(
        &self,
        server: &str,
        url: &str,
    ) -> Result<Option<oauth::RefreshLock>, OAuthError> {
        Ok(self
            .inner
            .try_lock_refresh(server, url)?
            .map(|lock| oauth::RefreshLock::new(Reported(lock, self.released.clone()))))
    }
    fn remove(&self, server: &str, url: &str) -> Result<bool, OAuthError> {
        self.inner.remove(server, url)
    }
    fn remove_server(&self, server: &str) -> Result<usize, OAuthError> {
        self.inner.remove_server(server)
    }
    fn marker(&self, server: &str, url: &str) -> Result<oauth::AuthMarker, OAuthError> {
        self.inner.marker(server, url)
    }
    fn mark(&self, server: &str, url: &str, marker: &oauth::AuthMarker) -> Result<(), OAuthError> {
        self.inner.mark(server, url, marker)
    }
    fn clear_marker(&self, server: &str, url: &str) -> Result<(), OAuthError> {
        self.inner.clear_marker(server, url)
    }
}

/// Regression: a refresh wrote the rotated credential synchronously on a runtime worker,
/// so a keyring write waiting on the user outlasted the caller's timeout; and a refresh
/// dropped mid-write must keep its lock until the write ends, so no other holder presents
/// the refresh token being rotated away.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_refresh_timeout_fires_during_a_blocked_write_which_keeps_the_lock() {
    let fake = FakeAs::start().await;
    fake.issue_refresh("PRIVATE-REFRESH");
    let gate = std::sync::Arc::new(std::sync::Barrier::new(2));
    let (released_tx, released) = std::sync::mpsc::channel();
    let store = std::sync::Arc::new(GatedStore {
        inner: MemoryCredentialStore::default(),
        gate: gate.clone(),
        released: released_tx,
    });
    let (key, credential) = issued_by(&fake.origin, "PRIVATE-REFRESH", 60);
    store.put(&key, &credential).unwrap();
    let mut source = token_source(store.clone()).await;
    let refresh = tokio::spawn(async move {
        tokio::time::timeout(Duration::from_secs(2), source.refresh(&client()))
            .await
            .is_err()
    });
    // The write of the rotated credential has begun and waits.
    let entered = gate.clone();
    tokio::task::spawn_blocking(move || entered.wait())
        .await
        .unwrap();
    let timed_out = tokio::time::timeout(Duration::from_secs(20), refresh)
        .await
        .expect("the caller's timeout never fired")
        .unwrap();
    assert!(timed_out);
    let url = resource_url();
    assert!(
        store
            .try_lock_refresh("remote", url.as_str())
            .unwrap()
            .is_none()
    );
    assert!(released.try_recv().is_err());
    let open = gate.clone();
    tokio::task::spawn_blocking(move || open.wait())
        .await
        .unwrap();
    tokio::task::spawn_blocking(move || released.recv_timeout(Duration::from_secs(10)))
        .await
        .unwrap()
        .expect("the refresh lock was never released");
    assert!(
        store
            .try_lock_refresh("remote", url.as_str())
            .unwrap()
            .is_some()
    );
    // The write finished after its caller gave up: the rotated credential is stored.
    let (_, stored) = store.find("remote", url.as_str()).unwrap().unwrap();
    assert_eq!(
        stored.tokens.refresh_token,
        Some(format!("{}1", oauth_as::REFRESH_PREFIX))
    );
}
