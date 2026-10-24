use aws_lc_rs::{
    rand::SystemRandom,
    signature::{ECDSA_P256_SHA256_FIXED_SIGNING, EcdsaKeyPair},
};
use axum::{
    Form, Json, Router,
    extract::State,
    http::{HeaderMap, StatusCode},
    routing::{get, post},
};
use cyber_account::{AuthorizationAttempt, Error, OidcClient, Secret, Transport};
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode, jwk::Jwk};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tokio::task::JoinHandle;

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}
struct Signing {
    key: EncodingKey,
    jwk: Value,
    kid: String,
}
impl Signing {
    fn new(kid: &str) -> Self {
        let der =
            EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &SystemRandom::new())
                .unwrap();
        let key = EncodingKey::from_ec_der(der.as_ref());
        let mut jwk =
            serde_json::to_value(Jwk::from_encoding_key(&key, Algorithm::ES256).unwrap()).unwrap();
        jwk["kid"] = json!(kid);
        jwk["use"] = json!("sig");
        Self {
            key,
            jwk,
            kid: kid.to_owned(),
        }
    }
    fn sign(&self, claims: &Value) -> String {
        let mut header = Header::new(Algorithm::ES256);
        header.kid = Some(self.kid.clone());
        encode(&header, claims, &self.key).unwrap()
    }
}
#[derive(Clone)]
struct ServerState {
    issuer: String,
    keys: Arc<Mutex<Value>>,
    live: Arc<Mutex<Value>>,
    exchange: Arc<Mutex<Value>>,
    denial: Arc<Mutex<Option<String>>>,
    checks: Arc<AtomicUsize>,
    key_requests: Arc<AtomicUsize>,
    forms: Arc<Mutex<Vec<HashMap<String, String>>>>,
    delay: Arc<Mutex<Option<Arc<tokio::sync::Notify>>>>,
}
struct Fixture {
    state: ServerState,
    signing: Signing,
    task: JoinHandle<()>,
}
impl Fixture {
    async fn start() -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let issuer = format!("http://{}", listener.local_addr().unwrap());
        let signing = Signing::new("first");
        let state = ServerState {
            issuer,
            keys: Arc::new(Mutex::new(json!({"keys":[signing.jwk]}))),
            live: Arc::new(Mutex::new(
                json!({"active":true,"token_type":"user","sub":"alice", "aud":"hub", "exp":now()+3600,"scope":"openid cyber:collaborate", "orgs":[{"id":"org-uuid","short_name":"team","github_login":"acme"}], "auth_time":now(), "amr":["pwd","otp"]}),
            )),
            exchange: Arc::new(Mutex::new(json!({}))),
            denial: Arc::new(Mutex::new(None)),
            checks: Arc::new(AtomicUsize::new(0)),
            key_requests: Arc::new(AtomicUsize::new(0)),
            forms: Arc::new(Mutex::new(Vec::new())),
            delay: Arc::new(Mutex::new(None)),
        };
        let app = Router::new()
            .route("/.well-known/openid-configuration", get(metadata))
            .route("/jwks", get(keys))
            .route("/introspect", post(introspect))
            .route("/token", post(token))
            .with_state(state.clone());
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        Self {
            state,
            signing,
            task,
        }
    }
    async fn client(&self) -> OidcClient {
        OidcClient::discover(
            &self.state.issuer,
            "cyber-cli",
            Transport::AllowLoopbackHttp,
        )
        .await
        .unwrap()
    }
    fn claims(&self) -> Value {
        json!({"iss":self.state.issuer,"sub":"alice","aud":"hub","iat":now(),"exp":now()+3600,"scope":"openid cyber:collaborate","orgs":[{"id":"stale-jwt-org"}],"auth_time":now()-3600,"roles":["other:owner","cyber-cli:reader"]})
    }
    fn access(&self) -> Secret {
        Secret::new(self.signing.sign(&self.claims()))
    }
    fn id_claims(&self, nonce: &str) -> Value {
        let mut claims = self.claims();
        claims["aud"] = json!("cyber-cli");
        claims["nonce"] = json!(nonce);
        claims["email"] = json!("alice@example.test");
        claims["test_padding"] = json!("padding".repeat(600));
        claims
    }
    fn service(&self) -> Secret {
        Secret::new("backend-only".into())
    }
    async fn bundle(&self, client: &mut OidcClient) -> cyber_account::TokenSet {
        self.bundle_until(client, now() + 3600).await
    }
    async fn bundle_until(&self, client: &mut OidcClient, expires: u64) -> cyber_account::TokenSet {
        let attempt = AuthorizationAttempt::new(
            client.provider(),
            "cyber-cli",
            "http://127.0.0.1:12345/callback".parse().unwrap(),
        )
        .unwrap();
        let params: HashMap<_, _> = attempt
            .authorization_url()
            .query_pairs()
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect();
        let mut access_claims = self.claims();
        access_claims["exp"] = json!(expires);
        *self.state.exchange.lock().unwrap() = json!({"access_token":self.signing.sign(&access_claims),"id_token":self.signing.sign(&self.id_claims(&params["nonce"])),"refresh_token":"refresh-1","token_type":"Bearer"});
        let grant = attempt
            .finish(&format!(
                "http://127.0.0.1:12345/callback?state={}&code=auth-code",
                params["state"]
            ))
            .unwrap();
        client.exchange(grant, "hub").await.unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.task.abort();
    }
}
async fn metadata(State(s): State<ServerState>) -> Json<Value> {
    Json(
        json!({"issuer":s.issuer,"authorization_endpoint":format!("{}/authorize",s.issuer),"token_endpoint":format!("{}/token",s.issuer),"jwks_uri":format!("{}/jwks",s.issuer),"introspection_endpoint":format!("{}/introspect",s.issuer),"scopes_supported":["openid","cyber:collaborate"],"code_challenge_methods_supported":["S256"],"grant_types_supported":["authorization_code","refresh_token"],"id_token_signing_alg_values_supported":["ES256"]}),
    )
}
async fn keys(State(s): State<ServerState>) -> Json<Value> {
    s.key_requests.fetch_add(1, Ordering::SeqCst);
    Json(s.keys.lock().unwrap().clone())
}
async fn introspect(
    State(s): State<ServerState>,
    headers: HeaderMap,
    Form(form): Form<HashMap<String, String>>,
) -> (StatusCode, Json<Value>) {
    s.checks.fetch_add(1, Ordering::SeqCst);
    assert_eq!(headers.get("authorization").unwrap(), "Bearer backend-only");
    assert_eq!(form["token_type_hint"], "access_token");
    assert!(!form["token"].is_empty());
    (StatusCode::OK, Json(s.live.lock().unwrap().clone()))
}
async fn token(
    State(s): State<ServerState>,
    headers: HeaderMap,
    Form(form): Form<HashMap<String, String>>,
) -> (StatusCode, Json<Value>) {
    assert!(!headers.contains_key("authorization"));
    assert!(!form.contains_key("client_secret"));
    assert_eq!(form["client_id"], "cyber-cli");
    s.forms.lock().unwrap().push(form);
    let delay = s.delay.lock().unwrap().clone();
    if let Some(delay) = delay {
        delay.notified().await;
    }
    match s.denial.lock().unwrap().clone() {
        Some(error) => (StatusCode::BAD_REQUEST, Json(json!({"error":error}))),
        None => (StatusCode::OK, Json(s.exchange.lock().unwrap().clone())),
    }
}

#[tokio::test]
async fn verifies_id_signature_nonce_audience_and_authorized_party() {
    let f = Fixture::start().await;
    let mut client = f.client().await;
    let account = client
        .verify_id_token(&f.signing.sign(&f.id_claims("nonce")), "nonce")
        .await
        .unwrap();
    assert_eq!(account.sub, "alice");
    assert_eq!(account.email.as_deref(), Some("alice@example.test"));
    let edits = vec![
        ("nonce", json!("wrong")),
        ("aud", json!("other-client")),
        ("azp", json!("other-client")),
        ("iss", json!("https://attacker.example")),
        ("sub", json!("")),
        ("exp", json!(now() - 120)),
        ("iat", json!(now() + 120)),
        ("nbf", json!(now() + 120)),
        ("auth_time", json!(now() + 120)),
    ];
    for (field, value) in edits {
        let mut c = f.id_claims("nonce");
        c[field] = value;
        assert!(
            matches!(
                client.verify_id_token(&f.signing.sign(&c), "nonce").await,
                Err(Error::InvalidToken)
            ),
            "{field}"
        );
    }
    let mut c = f.id_claims("nonce");
    c["aud"] = json!(["cyber-cli", "second"]);
    assert!(matches!(
        client.verify_id_token(&f.signing.sign(&c), "nonce").await,
        Err(Error::InvalidToken)
    ));
    c["azp"] = json!("cyber-cli");
    assert!(
        client
            .verify_id_token(&f.signing.sign(&c), "nonce")
            .await
            .is_ok()
    );
    for missing in ["nonce", "iat", "exp", "aud", "iss", "sub"] {
        let mut c = f.id_claims("nonce");
        c.as_object_mut().unwrap().remove(missing);
        assert!(
            matches!(
                client.verify_id_token(&f.signing.sign(&c), "nonce").await,
                Err(Error::InvalidToken)
            ),
            "{missing}"
        );
    }
}

#[tokio::test]
async fn unknown_kid_refetches_discovery_and_keys_once_and_can_rotate() {
    let f = Fixture::start().await;
    let mut client = f.client().await;
    let replacement = Signing::new("second");
    *f.state.keys.lock().unwrap() = json!({"keys":[replacement.jwk]});
    client
        .verify_id_token(&replacement.sign(&f.id_claims("n")), "n")
        .await
        .unwrap();
    assert_eq!(f.state.key_requests.load(Ordering::SeqCst), 2);
    assert!(matches!(
        client
            .verify_id_token(&f.signing.sign(&f.id_claims("n")), "n")
            .await,
        Err(Error::UnknownSigningKey)
    ));
    assert_eq!(f.state.key_requests.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn wrong_signatures_hmac_keys_and_duplicate_key_ids_are_rejected() {
    let f = Fixture::start().await;
    let mut client = f.client().await;
    let attacker = Signing::new("first");
    assert!(matches!(
        client
            .verify_id_token(&attacker.sign(&f.id_claims("n")), "n")
            .await,
        Err(Error::InvalidToken)
    ));
    let mut header = Header::new(Algorithm::HS256);
    header.kid = Some("first".into());
    let token = encode(
        &header,
        &f.id_claims("n"),
        &EncodingKey::from_secret(b"not-a-public-key"),
    )
    .unwrap();
    assert!(matches!(
        client.verify_id_token(&token, "n").await,
        Err(Error::InvalidToken)
    ));
    *f.state.keys.lock().unwrap() = json!({"keys":[f.signing.jwk,f.signing.jwk]});
    let mut other = f.client().await;
    assert!(matches!(
        other
            .verify_id_token(&f.signing.sign(&f.id_claims("n")), "n")
            .await,
        Err(Error::InvalidToken)
    ));
}

#[tokio::test]
async fn key_use_operations_and_algorithm_are_enforced() {
    let f = Fixture::start().await;
    for (field, value) in [
        ("use", json!("enc")),
        ("key_ops", json!(["sign"])),
        ("alg", json!("RS256")),
    ] {
        let mut key = f.signing.jwk.clone();
        key[field] = value;
        *f.state.keys.lock().unwrap() = json!({"keys":[key]});
        let mut client = f.client().await;
        assert!(matches!(
            client
                .verify_id_token(&f.signing.sign(&f.id_claims("n")), "n")
                .await,
            Err(Error::InvalidToken)
        ));
    }
}

#[tokio::test]
async fn live_introspection_replaces_stale_orgs_and_bounds_authority() {
    let f = Fixture::start().await;
    let mut client = f.client().await;
    let before = Instant::now();
    let lease = client
        .authorize_human(&f.access(), &f.service(), "hub")
        .await
        .unwrap();
    assert!(lease.is_current());
    assert!(lease.deadline() >= before + Duration::from_secs(59));
    assert!(lease.deadline() <= Instant::now() + Duration::from_secs(60));
    assert_eq!(lease.account().orgs[0].id, "org-uuid");
    assert!(lease.require_organization("org-uuid").is_ok());
    assert_eq!(
        lease.require_organization("stale-jwt-org"),
        Err(Error::InactiveIdentity)
    );
    assert!(lease.require_step_up(true).is_ok());
    assert_eq!(lease.account().roles, ["cyber-cli:reader"]);
    assert_eq!(f.state.checks.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn inactive_service_missing_scope_and_mismatched_live_principals_fail_closed() {
    let f = Fixture::start().await;
    let mut client = f.client().await;
    let original = f.state.live.lock().unwrap().clone();
    for (field, value) in [
        ("active", json!(false)),
        ("token_type", json!("service")),
        ("sub", json!("mallory")),
        ("aud", json!("other-hub")),
        ("exp", json!(now())),
        ("scope", json!("openid")),
    ] {
        let mut live = original.clone();
        live[field] = value;
        *f.state.live.lock().unwrap() = live;
        assert!(
            matches!(
                client
                    .authorize_human(&f.access(), &f.service(), "hub")
                    .await,
                Err(Error::InactiveIdentity)
            ),
            "{field}"
        );
    }
    *f.state.live.lock().unwrap() = json!({"active":false});
    assert!(matches!(
        client
            .authorize_human(&f.access(), &f.service(), "hub")
            .await,
        Err(Error::InactiveIdentity)
    ));
}

#[tokio::test]
async fn sensitive_revalidation_observes_org_removal_and_disabled_users_immediately() {
    let f = Fixture::start().await;
    let mut client = f.client().await;
    client
        .authorize_human(&f.access(), &f.service(), "hub")
        .await
        .unwrap();
    f.state
        .live
        .lock()
        .unwrap()
        .as_object_mut()
        .unwrap()
        .remove("orgs");
    let lease = client
        .authorize_human(&f.access(), &f.service(), "hub")
        .await
        .unwrap();
    assert_eq!(
        lease.require_organization("org-uuid"),
        Err(Error::InactiveIdentity)
    );
    f.state.live.lock().unwrap()["active"] = json!(false);
    assert!(matches!(
        client
            .authorize_human(&f.access(), &f.service(), "hub")
            .await,
        Err(Error::InactiveIdentity)
    ));
    assert_eq!(f.state.checks.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn signed_scope_and_audience_are_mandatory_before_introspection() {
    let f = Fixture::start().await;
    let mut client = f.client().await;
    let mut claims = f.claims();
    claims["scope"] = json!("openid");
    assert!(matches!(
        client
            .authorize_human(&Secret::new(f.signing.sign(&claims)), &f.service(), "hub")
            .await,
        Err(Error::MissingCollaborationScope)
    ));
    claims = f.claims();
    claims["aud"] = json!("cyber-cli");
    assert!(matches!(
        client
            .authorize_human(&Secret::new(f.signing.sign(&claims)), &f.service(), "hub")
            .await,
        Err(Error::InvalidToken)
    ));
    assert_eq!(f.state.checks.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn live_failure_never_falls_back_to_signed_jwt_or_old_lease() {
    let f = Fixture::start().await;
    let mut client = f.client().await;
    let lease = client
        .authorize_human(&f.access(), &f.service(), "hub")
        .await
        .unwrap();
    f.task.abort();
    assert!(matches!(
        client
            .authorize_human(&f.access(), &f.service(), "hub")
            .await,
        Err(Error::LiveValidationUnavailable)
    ));
    assert!(lease.deadline() <= Instant::now() + Duration::from_secs(60));
}

#[tokio::test]
async fn step_up_uses_live_authentication_time_and_managed_mfa() {
    let f = Fixture::start().await;
    let mut client = f.client().await;
    for (time, amr) in [
        (json!(now() - 901), json!(["pwd", "otp"])),
        (Value::Null, json!(["pwd", "otp"])),
        (json!(now()), json!(["pwd"])),
        (json!(now()), json!(["hwk"])),
    ] {
        f.state.live.lock().unwrap()["auth_time"] = time;
        f.state.live.lock().unwrap()["amr"] = amr;
        let lease = client
            .authorize_human(&f.access(), &f.service(), "hub")
            .await
            .unwrap();
        assert_eq!(lease.require_step_up(true), Err(Error::StepUpRequired));
    }
}

#[tokio::test]
async fn exchange_binds_access_identity_to_verified_id_and_redacts_credentials() {
    let f = Fixture::start().await;
    let mut client = f.client().await;
    let bundle = f.bundle(&mut client).await;
    assert_eq!(bundle.account().sub, "alice");
    assert_eq!(
        bundle.account().email.as_deref(),
        Some("alice@example.test")
    );
    let debug = format!("{bundle:?}");
    assert!(!debug.contains(bundle.access_token().expose()));
    assert!(!debug.contains("refresh-1"));
    let forms = f.state.forms.lock().unwrap();
    assert_eq!(forms[0]["code"], "auth-code");
    assert!(forms[0].contains_key("code_verifier"));
}

#[tokio::test]
async fn refresh_rotates_credentials_and_cannot_renew_step_up_time() {
    let f = Fixture::start().await;
    let mut client = f.client().await;
    let old = f.bundle(&mut client).await;
    let mut claims = f.claims();
    claims["auth_time"] = json!(now());
    claims["exp"] = json!(now() + 7200);
    *f.state.exchange.lock().unwrap() = json!({"access_token":f.signing.sign(&claims),"refresh_token":"refresh-2","token_type":"Bearer"});
    let new = client.refresh(&old, "hub").await.unwrap();
    assert_eq!(new.account().auth_time, old.account().auth_time);
    assert!(new.account().expires_at > old.account().expires_at);
    assert_eq!(
        f.state.forms.lock().unwrap()[1]["refresh_token"],
        "refresh-1"
    );
    *f.state.denial.lock().unwrap() = Some("invalid_grant".into());
    assert!(matches!(
        client.refresh(&new, "hub").await,
        Err(Error::SessionExpired)
    ));
}

#[tokio::test]
async fn refresh_rejects_forged_replacement_id_and_subject_switches() {
    let f = Fixture::start().await;
    let mut client = f.client().await;
    let old = f.bundle(&mut client).await;
    let attacker = Signing::new("first");
    *f.state.exchange.lock().unwrap() = json!({"access_token":f.access().expose(),"id_token":attacker.sign(&f.id_claims("any")),"token_type":"Bearer"});
    assert!(matches!(
        client.refresh(&old, "hub").await,
        Err(Error::InvalidToken)
    ));
    let mut claims = f.claims();
    claims["sub"] = json!("bob");
    *f.state.exchange.lock().unwrap() =
        json!({"access_token":f.signing.sign(&claims),"token_type":"Bearer"});
    assert!(matches!(
        client.refresh(&old, "hub").await,
        Err(Error::InvalidToken)
    ));
}

#[derive(Default)]
struct MemoryVault {
    values: Mutex<HashMap<String, String>>,
    fail_writes: std::sync::atomic::AtomicBool,
}
impl cyber_account::CredentialVault for MemoryVault {
    fn get(&self, account: &str) -> Result<Option<Secret>, Error> {
        Ok(self
            .values
            .lock()
            .unwrap()
            .get(account)
            .cloned()
            .map(Secret::new))
    }
    fn put(&self, account: &str, value: &str) -> Result<(), Error> {
        if self.fail_writes.load(Ordering::SeqCst) {
            return Err(Error::Storage("test write failure"));
        }
        assert!(value.len() <= 2500);
        self.values
            .lock()
            .unwrap()
            .insert(account.into(), value.into());
        Ok(())
    }
    fn delete(&self, account: &str) -> Result<(), Error> {
        self.values.lock().unwrap().remove(account);
        Ok(())
    }
}

#[tokio::test]
async fn vault_storage_chunks_credentials_binds_identity_and_keeps_tokens_out_of_index() {
    let f = Fixture::start().await;
    let mut client = f.client().await;
    let bundle = f.bundle(&mut client).await;
    let directory = tempfile::tempdir().unwrap();
    let vault = Arc::new(MemoryVault::default());
    let store =
        cyber_account::AccountStore::with_vault(directory.path().into(), Some(vault.clone()));
    assert_eq!(
        store.save(bundle.clone()).await.unwrap(),
        cyber_account::StorageKind::Keyring
    );
    assert!(!directory.path().join("account.json").exists());
    let index = std::fs::read_to_string(directory.path().join("account-index.json")).unwrap();
    assert!(!index.contains(bundle.access_token().expose()));
    assert!(!index.contains("refresh-1"));
    assert_eq!(
        store.load().await.unwrap().unwrap().access_token().expose(),
        bundle.access_token().expose()
    );
    assert!(vault.values.lock().unwrap().len() > 2);
    let again =
        cyber_account::AccountStore::with_vault(directory.path().into(), Some(vault.clone()));
    assert_eq!(again.load().await.unwrap().unwrap().account().sub, "alice");
    store.logout(true).await.unwrap();
    assert!(store.load().await.unwrap().is_none());
    assert!(vault.values.lock().unwrap().is_empty());
}

#[tokio::test]
async fn vault_chunk_tampering_and_failed_writes_are_not_silently_downgraded() {
    let f = Fixture::start().await;
    let mut client = f.client().await;
    let bundle = f.bundle(&mut client).await;
    let directory = tempfile::tempdir().unwrap();
    let vault = Arc::new(MemoryVault::default());
    let store =
        cyber_account::AccountStore::with_vault(directory.path().into(), Some(vault.clone()));
    store.save(bundle.clone()).await.unwrap();
    let chunk = vault
        .values
        .lock()
        .unwrap()
        .keys()
        .find(|k| k.ends_with(".0"))
        .unwrap()
        .clone();
    vault.values.lock().unwrap().insert(chunk, "corrupt".into());
    assert!(matches!(store.load().await, Err(Error::Storage(_))));
    vault.fail_writes.store(true, Ordering::SeqCst);
    assert!(matches!(store.save(bundle).await, Err(Error::Storage(_))));
    assert!(!directory.path().join("account.json").exists());
}

#[tokio::test]
async fn concurrent_refreshes_reread_rotated_credentials_under_one_lock() {
    let f = Fixture::start().await;
    let mut client = f.client().await;
    let bundle = f.bundle_until(&mut client, now() + 30).await;
    let directory = tempfile::tempdir().unwrap();
    let vault = Arc::new(MemoryVault::default());
    let store = cyber_account::AccountStore::with_vault(directory.path().into(), Some(vault));
    store.save(bundle).await.unwrap();
    *f.state.exchange.lock().unwrap() =
        json!({"access_token":f.access().expose(),"refresh_token":"rotated","token_type":"Bearer"});
    let (a, b) = (f.client().await, f.client().await);
    let (a, b) = tokio::join!(
        store.refresh(a, "hub".into()),
        store.refresh(b, "hub".into())
    );
    assert_eq!(a.unwrap().account().sub, "alice");
    assert_eq!(b.unwrap().account().sub, "alice");
    let requests = f.state.forms.lock().unwrap();
    assert_eq!(
        requests
            .iter()
            .filter(|f| f["grant_type"] == "refresh_token")
            .count(),
        1
    );
}

#[tokio::test]
async fn invalid_grant_clears_credentials_and_a_durable_pending_marker_blocks_replay() {
    let f = Fixture::start().await;
    let mut client = f.client().await;
    let bundle = f.bundle_until(&mut client, now() + 30).await;
    let directory = tempfile::tempdir().unwrap();
    let vault = Arc::new(MemoryVault::default());
    let store =
        cyber_account::AccountStore::with_vault(directory.path().into(), Some(vault.clone()));
    store.save(bundle.clone()).await.unwrap();
    *f.state.denial.lock().unwrap() = Some("invalid_grant".into());
    assert!(matches!(
        store.refresh(f.client().await, "hub".into()).await,
        Err(Error::SessionExpired)
    ));
    assert!(store.load().await.unwrap().is_none());
    store.save(bundle).await.unwrap();
    let anchor = vault
        .values
        .lock()
        .unwrap()
        .keys()
        .find(|k| {
            !k.ends_with(".0")
                && !k
                    .rsplit('.')
                    .next()
                    .unwrap()
                    .bytes()
                    .all(|b| b.is_ascii_digit())
        })
        .unwrap()
        .clone();
    {
        let mut values = vault.values.lock().unwrap();
        let mut manifest: Value = serde_json::from_str(&values[&anchor]).unwrap();
        manifest["pending"] = json!(true);
        values.insert(anchor, manifest.to_string());
    }
    let before = f.state.forms.lock().unwrap().len();
    assert!(matches!(
        store.refresh(f.client().await, "hub".into()).await,
        Err(Error::SessionExpired)
    ));
    assert_eq!(f.state.forms.lock().unwrap().len(), before);
}

#[cfg(unix)]
#[tokio::test]
async fn private_file_fallback_refuses_public_files_and_symlinks() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let f = Fixture::start().await;
    let mut client = f.client().await;
    let bundle = f.bundle(&mut client).await;
    let directory = tempfile::tempdir().unwrap();
    let store = cyber_account::AccountStore::with_vault(directory.path().into(), None);
    assert_eq!(
        store.save(bundle.clone()).await.unwrap(),
        cyber_account::StorageKind::File
    );
    let path = directory.path().join("account.json");
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(store.load().await.unwrap().unwrap().account().sub, "alice");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert!(matches!(store.load().await, Err(Error::Storage(_))));
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    let target = directory.path().join("original.json");
    std::fs::rename(&path, &target).unwrap();
    symlink(&target, &path).unwrap();
    assert!(matches!(store.load().await, Err(Error::Storage(_))));
}

#[tokio::test]
async fn cancellation_does_not_release_the_refresh_lock_before_persistence() {
    let f = Fixture::start().await;
    let mut client = f.client().await;
    let bundle = f.bundle_until(&mut client, now() + 30).await;
    let directory = tempfile::tempdir().unwrap();
    let vault = Arc::new(MemoryVault::default());
    let store = cyber_account::AccountStore::with_vault(directory.path().into(), Some(vault));
    store.save(bundle).await.unwrap();
    *f.state.exchange.lock().unwrap() =
        json!({"access_token":f.access().expose(),"refresh_token":"rotated","token_type":"Bearer"});
    let gate = Arc::new(tokio::sync::Notify::new());
    *f.state.delay.lock().unwrap() = Some(gate.clone());
    let refreshed = store.clone();
    let first = f.client().await;
    let task = tokio::spawn(async move { refreshed.refresh(first, "hub".into()).await });
    tokio::time::timeout(Duration::from_secs(5), async {
        while f.state.forms.lock().unwrap().len() < 2 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    task.abort();
    gate.notify_one();
    let result = store.refresh(f.client().await, "hub".into()).await.unwrap();
    assert_eq!(result.account().sub, "alice");
    assert_eq!(
        f.state
            .forms
            .lock()
            .unwrap()
            .iter()
            .filter(|f| f["grant_type"] == "refresh_token")
            .count(),
        1
    );
}
