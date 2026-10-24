#![allow(dead_code)]
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
    collections::{HashMap, VecDeque},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    },
    time::{SystemTime, UNIX_EPOCH},
};
use tokio::task::JoinHandle;

pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}
pub struct Signing {
    key: EncodingKey,
    pub jwk: Value,
    kid: String,
}
impl Signing {
    pub fn new(kid: &str) -> Self {
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
    pub fn sign(&self, claims: &Value) -> String {
        let mut header = Header::new(Algorithm::ES256);
        header.kid = Some(self.kid.clone());
        encode(&header, claims, &self.key).unwrap()
    }
}
#[derive(Clone)]
pub struct ServerState {
    pub issuer: String,
    pub keys: Arc<Mutex<Value>>,
    pub live: Arc<Mutex<Value>>,
    pub exchange: Arc<Mutex<Value>>,
    pub denial: Arc<Mutex<Option<String>>>,
    pub checks: Arc<AtomicUsize>,
    pub key_requests: Arc<AtomicUsize>,
    pub forms: Arc<Mutex<Vec<HashMap<String, String>>>>,
    pub delay: Arc<Mutex<Option<Arc<tokio::sync::Notify>>>>,
    pub device_enabled: Arc<AtomicBool>,
    pub device_errors: Arc<Mutex<VecDeque<String>>>,
    pub device_expiry: Arc<AtomicU64>,
    pub device_polls: Arc<AtomicUsize>,
    pub token_subjects: Arc<Mutex<HashMap<String, String>>>,
    pub users: Arc<Mutex<HashMap<String, Value>>>,
    pub service_tokens: Arc<AtomicUsize>,
}
pub struct Fixture {
    pub state: ServerState,
    pub signing: Signing,
    pub task: JoinHandle<()>,
}
impl Fixture {
    pub async fn start() -> Self {
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
            device_enabled: Arc::new(AtomicBool::new(false)),
            device_errors: Arc::new(Mutex::new(VecDeque::new())),
            device_expiry: Arc::new(AtomicU64::new(60)),
            device_polls: Arc::new(AtomicUsize::new(0)),
            token_subjects: Arc::new(Mutex::new(HashMap::new())),
            users: Arc::new(Mutex::new(HashMap::new())),
            service_tokens: Arc::new(AtomicUsize::new(0)),
        };
        let app = Router::new()
            .route("/.well-known/openid-configuration", get(metadata))
            .route("/jwks", get(keys))
            .route("/introspect", post(introspect))
            .route("/token", post(token))
            .route("/device", post(device))
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
    pub async fn client(&self) -> OidcClient {
        OidcClient::discover(
            &self.state.issuer,
            "cyber-cli",
            Transport::AllowLoopbackHttp,
        )
        .await
        .unwrap()
    }
    pub fn claims(&self) -> Value {
        json!({"iss":self.state.issuer,"sub":"alice","aud":"hub","iat":now(),"exp":now()+3600,"scope":"openid cyber:collaborate","orgs":[{"id":"stale-jwt-org"}],"auth_time":now()-3600,"roles":["other:owner","cyber-cli:reader"]})
    }
    /// Register an independently authenticated principal with mutable current live
    /// authorization. Signed JWT organization snapshots deliberately stay unchanged.
    pub fn user(&self, subject: &str, orgs: Value) -> Secret {
        let mut claims = self.claims();
        claims["sub"] = json!(subject);
        claims["orgs"] = orgs.clone();
        let token = self.signing.sign(&claims);
        self.state
            .token_subjects
            .lock()
            .unwrap()
            .insert(token.clone(), subject.to_owned());
        self.state.users.lock().unwrap().insert(subject.to_owned(),json!({"active":true,"token_type":"user","sub":subject,"aud":"hub","exp":now()+3600,"scope":"openid cyber:collaborate","orgs":orgs,"auth_time":now(),"amr":["pwd","otp"]}));
        Secret::new(token)
    }

    pub fn access(&self) -> Secret {
        Secret::new(self.signing.sign(&self.claims()))
    }
    pub fn id_claims(&self, nonce: &str) -> Value {
        let mut claims = self.claims();
        claims["aud"] = json!("cyber-cli");
        claims["nonce"] = json!(nonce);
        claims["email"] = json!("alice@example.test");
        claims["test_padding"] = json!("padding".repeat(600));
        claims
    }
    pub fn service(&self) -> Secret {
        Secret::new("backend-only".into())
    }
    pub async fn bundle(&self, client: &mut OidcClient) -> cyber_account::TokenSet {
        self.bundle_until(client, now() + 3600).await
    }
    pub async fn bundle_until(
        &self,
        client: &mut OidcClient,
        expires: u64,
    ) -> cyber_account::TokenSet {
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
    let mut doc = json!({"issuer":s.issuer,"authorization_endpoint":format!("{}/authorize",s.issuer),"token_endpoint":format!("{}/token",s.issuer),"jwks_uri":format!("{}/jwks",s.issuer),"introspection_endpoint":format!("{}/introspect",s.issuer),"scopes_supported":["openid","cyber:collaborate"],"code_challenge_methods_supported":["S256"],"grant_types_supported":["authorization_code","refresh_token","client_credentials"],"id_token_signing_alg_values_supported":["ES256"]});
    if s.device_enabled.load(Ordering::SeqCst) {
        doc["device_authorization_endpoint"] = json!(format!("{}/device", s.issuer));
        doc["grant_types_supported"]
            .as_array_mut()
            .unwrap()
            .push(json!("urn:ietf:params:oauth:grant-type:device_code"));
    }
    Json(doc)
}
async fn device(
    State(s): State<ServerState>,
    Form(form): Form<HashMap<String, String>>,
) -> Json<Value> {
    assert_eq!(form["client_id"], "cyber-cli");
    assert!(form["scope"].contains("openid"));
    Json(
        json!({"device_code":"private-device-code","user_code":"ABCD-EFGH","verification_uri":format!("{}/device-ui",s.issuer),"expires_in":s.device_expiry.load(Ordering::SeqCst),"interval":1}),
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
    let subject = s
        .token_subjects
        .lock()
        .unwrap()
        .get(&form["token"])
        .cloned();
    let live = subject
        .and_then(|sub| s.users.lock().unwrap().get(&sub).cloned())
        .unwrap_or_else(|| s.live.lock().unwrap().clone());
    (StatusCode::OK, Json(live))
}
async fn token(
    State(s): State<ServerState>,
    headers: HeaderMap,
    Form(form): Form<HashMap<String, String>>,
) -> (StatusCode, Json<Value>) {
    if form
        .get("grant_type")
        .is_some_and(|g| g == "client_credentials")
    {
        assert_eq!(
            headers.get("authorization").unwrap(),
            "Basic YmFja2VuZDpzZXJ2aWNlLXNlY3JldA=="
        );
        assert!(!form.contains_key("client_secret"));
        s.service_tokens.fetch_add(1, Ordering::SeqCst);
        return (
            StatusCode::OK,
            Json(json!({"access_token":"backend-only","token_type":"Bearer","expires_in":600})),
        );
    }
    assert!(!headers.contains_key("authorization"));
    assert!(!form.contains_key("client_secret"));
    assert_eq!(form["client_id"], "cyber-cli");
    if form
        .get("grant_type")
        .is_some_and(|g| g == "urn:ietf:params:oauth:grant-type:device_code")
    {
        assert_eq!(form["device_code"], "private-device-code");
        s.device_polls.fetch_add(1, Ordering::SeqCst);
        if let Some(error) = s.device_errors.lock().unwrap().pop_front() {
            return (StatusCode::BAD_REQUEST, Json(json!({"error":error})));
        }
    }
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

#[derive(Default)]
pub struct MemoryVault {
    pub values: Mutex<HashMap<String, String>>,
    pub fail_writes: std::sync::atomic::AtomicBool,
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
