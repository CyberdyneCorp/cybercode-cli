//! Production account service and owner HTTP routes with an injected credential vault.
#[path = "support/mode_app.rs"]
mod mode_app;
#[path = "../../cyber-account/tests/support/oidc.rs"]
mod oidc;
#[path = "support/scripted_model.rs"]
mod scripted_model;
use cyber_account::{AccountSettings, AccountStore};
use cyber_app::account::Accounts;
use cyber_server::http::{self, Services, account::AccountService};
use futures::future::BoxFuture;
use oidc::{Fixture, MemoryVault};
use serde_json::{Value, json};
use std::{collections::HashMap, path::Path, sync::Arc, time::Duration};

struct WithAccount {
    base: Arc<dyn Services>,
    account: Arc<Accounts>,
}
impl Services for WithAccount {
    fn account(&self) -> Option<Arc<dyn AccountService>> {
        Some(self.account.clone())
    }
    fn models(&self, p: &Path) -> BoxFuture<'_, Result<Vec<http::ModelInfo>, String>> {
        self.base.models(p)
    }
    fn default_model(&self, p: &Path) -> Option<String> {
        self.base.default_model(p)
    }
    fn agents(&self, p: &Path) -> Vec<http::AgentInfo> {
        self.base.agents(p)
    }
    fn tools(&self, t: &cyber_server::runtime::TurnContext) -> Vec<cyber_server::runtime::ToolDef> {
        self.base.tools(t)
    }
    fn commands(&self, p: &Path) -> Vec<http::CommandInfo> {
        self.base.commands(p)
    }
    fn find_files(&self, p: &Path, q: &str, l: usize) -> Vec<String> {
        self.base.find_files(p, q, l)
    }
    fn expand_command(&self, p: &Path, n: &str, a: &str) -> Option<String> {
        self.base.expand_command(p, n, a)
    }
}
struct TestApp {
    app: mode_app::ModeApp,
    base: String,
    task: tokio::task::JoinHandle<()>,
    vault: Arc<MemoryVault>,
    http: reqwest::Client,
}
impl TestApp {
    async fn start(issuer: &str) -> Self {
        let app = mode_app::ModeApp::start(Default::default()).await;
        let vault = Arc::new(MemoryVault::default());
        let store = AccountStore::with_vault(app.app.paths.data.clone(), Some(vault.clone()));
        let settings=AccountSettings::from_global(&json!({"account":{"issuer":issuer,"client_id":"cyber-cli","audience":"hub","allow_loopback_http":true}}),None).unwrap();
        let accounts = Arc::new(Accounts::new(
            store,
            app.app.paths.cache.clone(),
            Arc::new(move || Ok(settings.clone())),
        ));
        let mut state = app.app.state.clone();
        state.services = Arc::new(WithAccount {
            base: state.services.clone(),
            account: accounts,
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}/api/v1", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            http::serve_tcp(http::router(state), listener, std::future::pending())
                .await
                .unwrap();
        });
        Self {
            app,
            base,
            task,
            vault,
            http: reqwest::Client::new(),
        }
    }
    fn get(&self, path: &str) -> reqwest::RequestBuilder {
        self.http
            .get(format!("{}{path}", self.base))
            .basic_auth("cyber", Some(mode_app::PASSWORD))
    }
    fn post(&self, path: &str) -> reqwest::RequestBuilder {
        self.http
            .post(format!("{}{path}", self.base))
            .basic_auth("cyber", Some(mode_app::PASSWORD))
    }
    async fn logout(&self) {
        let response = self
            .http
            .delete(format!("{}/account?all=true", self.base))
            .basic_auth("cyber", Some(mode_app::PASSWORD))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 204);
    }
    async fn begin(&self) -> Value {
        let response = self.post("/account/login").send().await.unwrap();
        assert_eq!(response.status(), 201);
        assert_eq!(response.headers()["cache-control"], "no-store");
        response.json::<Value>().await.unwrap()["data"].clone()
    }
    async fn terminal(&self, id: &str) -> Value {
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let data = self
                    .get(&format!("/account/login/{id}"))
                    .send()
                    .await
                    .unwrap()
                    .json::<Value>()
                    .await
                    .unwrap()["data"]
                    .clone();
                if data["status"] != "pending" {
                    break data;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap()
    }
}
impl Drop for TestApp {
    fn drop(&mut self) {
        self.task.abort();
    }
}
fn response_for(f: &Fixture, attempt: &Value) -> String {
    let url = url::Url::parse(attempt["authorization_url"].as_str().unwrap()).unwrap();
    let params: HashMap<_, _> = url
        .query_pairs()
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    *f.state.exchange.lock().unwrap() = json!({"access_token":f.access().expose(),"id_token":f.signing.sign(&f.id_claims(&params["nonce"])),"refresh_token":"api-refresh-sentinel","token_type":"Bearer"});
    format!(
        "{}?state={}&code=auth-code",
        params["redirect_uri"], params["state"]
    )
}

#[tokio::test]
async fn account_api_keeps_owner_auth_and_never_returns_credentials() {
    let f = Fixture::start().await;
    let app = TestApp::start(&f.state.issuer).await;
    let denied = app
        .http
        .get(format!("{}/account", app.base))
        .bearer_auth(f.access().expose())
        .send()
        .await
        .unwrap();
    assert_eq!(denied.status(), 401);
    let empty = app
        .get("/account")
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap();
    assert_eq!(empty["data"]["logged_in"], false);
    let (status, local) = app.app.create("default").await;
    assert_eq!(status, 201);
    let session = local["data"]["id"].as_str().unwrap();
    let attempt = app.begin().await;
    let redirect = response_for(&f, &attempt);
    let submitted = app
        .post(&format!(
            "/account/login/{}/code",
            attempt["attempt_id"].as_str().unwrap()
        ))
        .json(&json!({"redirect_url":redirect}))
        .send()
        .await
        .unwrap();
    assert_eq!(submitted.status(), 200);
    let terminal = app.terminal(attempt["attempt_id"].as_str().unwrap()).await;
    assert_eq!(terminal["status"], "complete");
    assert!(terminal["authorization_url"].is_null());
    let response = app.get("/account").send().await.unwrap();
    assert_eq!(response.headers()["cache-control"], "no-store");
    let body = response.text().await.unwrap();
    let view: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(view["data"]["sub"], "alice");
    assert!(!body.contains("api-refresh-sentinel"));
    assert!(!body.contains("access_token"));
    app.logout().await;
    assert!(app.vault.values.lock().unwrap().is_empty());
    let empty = app
        .get("/account")
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap();
    assert_eq!(empty["data"]["logged_in"], false);
    let (status, _) = app
        .app
        .send("GET", &format!("/sessions/{session}"), None)
        .await;
    assert_eq!(status, 200);
}

#[tokio::test]
async fn logout_cancels_pending_api_login_even_if_browser_finishes_later() {
    let f = Fixture::start().await;
    let app = TestApp::start(&f.state.issuer).await;
    let attempt = app.begin().await;
    let redirect = response_for(&f, &attempt);
    app.logout().await;
    // An already-open browser may still deliver its callback; the durable epoch
    // refuses the credential commit rather than reactivating the account.
    let _ = app.http.get(redirect).send().await;
    let terminal = app.terminal(attempt["attempt_id"].as_str().unwrap()).await;
    assert_eq!(terminal["status"], "failed");
    let empty = app
        .get("/account")
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap();
    assert_eq!(empty["data"]["logged_in"], false);
    assert!(app.vault.values.lock().unwrap().is_empty());
}

#[tokio::test]
async fn reauthentication_uses_pkce_even_when_headless_device_login_is_advertised() {
    use std::sync::atomic::Ordering;
    let f = Fixture::start().await;
    f.state.device_enabled.store(true, Ordering::SeqCst);
    let app = TestApp::start(&f.state.issuer).await;
    let response = app
        .post("/account/login")
        .json(&json!({"no_browser":true,"reauthenticate":true}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 201);
    let attempt = response.json::<Value>().await.unwrap()["data"].clone();
    assert!(attempt["user_code"].is_null());
    let url = url::Url::parse(attempt["authorization_url"].as_str().unwrap()).unwrap();
    let params: HashMap<_, _> = url.query_pairs().collect();
    assert_eq!(params["prompt"], "login");
    assert_eq!(params["max_age"], "900");
    let redirect = response_for(&f, &attempt);
    let mut id = f.id_claims(&params["nonce"]);
    id["auth_time"] = json!(oidc::now());
    f.state.exchange.lock().unwrap()["id_token"] = json!(f.signing.sign(&id));
    assert_eq!(app.http.get(redirect).send().await.unwrap().status(), 200);
    assert_eq!(
        app.terminal(attempt["attempt_id"].as_str().unwrap()).await["status"],
        "complete"
    );
    assert_eq!(f.state.device_polls.load(Ordering::SeqCst), 0);
    app.logout().await;
}

#[tokio::test]
async fn logout_hook_runs_after_local_credentials_are_removed() {
    use std::sync::atomic::{AtomicBool, Ordering};
    let f = Fixture::start().await;
    let mut client = f.client().await;
    let dir = tempfile::tempdir().unwrap();
    let store = AccountStore::with_vault(dir.path().into(), Some(Arc::new(MemoryVault::default())));
    store.save(f.bundle(&mut client).await).await.unwrap();
    let settings=AccountSettings::from_global(&json!({"account":{"issuer":f.state.issuer,"client_id":"cyber-cli","audience":"hub","allow_loopback_http":true}}),None).unwrap();
    let accounts = Accounts::new(
        store.clone(),
        dir.path().join("cache"),
        Arc::new(move || Ok(settings.clone())),
    );
    let called = Arc::new(AtomicBool::new(false));
    let observed = called.clone();
    accounts.on_logout(Arc::new(move |token, all| {
        let store = store.clone();
        let observed = observed.clone();
        Box::pin(async move {
            assert!(all);
            assert!(token.is_some());
            assert!(store.load().await.unwrap().is_none());
            observed.store(true, Ordering::SeqCst);
        })
    }));
    accounts.logout(true).await.unwrap();
    assert!(called.load(Ordering::SeqCst));
}
