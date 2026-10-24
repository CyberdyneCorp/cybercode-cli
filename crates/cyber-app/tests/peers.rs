//! Direct peers on the production App (`cross-session-messaging` → Direct peer transport,
//! `server-api` → Peer listener), served over authenticated TCP with the scripted
//! OpenAI-compatible provider. No test reaches another machine.

#[path = "support/scripted_model.rs"]
mod scripted_model;
#[path = "support/mode_app.rs"]
mod support;

use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use base64::Engine;
use cyber_app::{Registration, ServeOptions};
use cyber_core::paths::Paths;
use serde_json::{Value, json};
use support::{ModeApp, Setup};
use tokio::sync::Notify;

const FINGERPRINT: &str = "sha256:9f2c000000000000000000000000000000000000000000000000000000000001";

fn peers() -> Value {
    json!({
        "listen": {"address": "127.0.0.1:0", "tls": false},
        "machine": "desk",
        "remotes": [{"name": "laptop", "url": "https://laptop.local:4748",
            "fingerprint": FINGERPRINT}],
    })
}

/// `peers` in user config is accepted by the App; the same key in the checkout's project
/// or local config makes that Location's configuration invalid, naming the file and
/// `cyber peers add`, and nothing in the Location runs.
#[tokio::test]
async fn peers_config_is_accepted_only_from_user_config() {
    let app = ModeApp::start(Setup {
        config: json!({ "peers": peers() }),
        sandbox_policy: support::BASH_SANDBOX,
        ..Setup::default()
    })
    .await;
    let (status, body) = app.create("default").await;
    assert_eq!(status, 201, "{body}");

    for file in ["cyber.local.jsonc", "cyber.jsonc"] {
        let path = app.repo.join(".cyber").join(file);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, json!({ "peers": peers() }).to_string()).unwrap();
        let (status, body) = app.create("default").await;
        assert!(status >= 400, "{status} {body}");
        let message = body["message"].as_str().unwrap_or_default();
        assert!(
            message.contains("peers: accepted only in user config"),
            "{body}"
        );
        assert!(message.contains(&format!("{file}; remove")), "{body}");
        assert!(message.contains("cyber peers add"), "{body}");
        std::fs::remove_file(&path).unwrap();
    }
    let (status, body) = app.create("default").await;
    assert_eq!(status, 201, "{body}");
}

/// The served OpenAPI document declares the four peer error tags with their fields, and
/// the message send and watch routes answer them.
#[tokio::test]
async fn peer_errors_are_declared_by_the_served_api() {
    let app = ModeApp::start(Setup::default()).await;
    let (status, document) = app.send("GET", "/openapi.json", None).await;
    assert_eq!(status, 200, "{document}");
    let schemas = &document["components"]["schemas"];
    assert_eq!(
        schemas["PeerErrorTag"]["enum"],
        json!([
            "PeerUnreachableError",
            "PeerAuthError",
            "PeerNotConfiguredError",
            "PeerScopeError"
        ])
    );
    let body = &schemas["PeerErrorBody"]["properties"];
    for field in ["_tag", "message", "peer", "cause", "reason", "scope"] {
        assert!(body.get(field).is_some(), "{field}: {body}");
    }
    let reasons = schemas["PeerAuthReason"].to_string();
    for reason in ["fingerprint_mismatch", "unauthorized", "scope"] {
        assert!(reasons.contains(&format!("\"{reason}\"")), "{reasons}");
    }
    for route in ["/api/v1/messaging/send", "/api/v1/messaging/watch"] {
        let responses = &document["paths"][route]["post"]["responses"];
        let declared = responses["502"].to_string();
        assert!(
            declared.contains("PeerUnreachableError") && declared.contains("PeerErrorBody"),
            "{route}: {responses}"
        );
        assert!(
            responses["404"]
                .to_string()
                .contains("PeerNotConfiguredError"),
            "{route}: {responses}"
        );
    }
}

/// The production server (`run_server_until`, registered) on a temporary root with `peers`
/// in user config, stopped by `stop`.
struct Served {
    root: PathBuf,
    stop: Arc<Notify>,
    task: tokio::task::JoinHandle<Result<(), String>>,
    registration: Registration,
    /// The App's store, where `cyber peers invite` keeps grants.
    store: Arc<cyber_store::Store>,
    /// The App's runtime, for passes of its background tasks on a test clock.
    runtime: cyber_server::runtime::Runtime,
}

impl Served {
    async fn start(root: &Path, model: &scripted_model::ScriptedModel, peers: Value) -> Self {
        Self::start_with(root, model, json!({ "peers": peers })).await
    }

    /// [`Served::start`] with the whole user `config` (merged over the model's).
    async fn start_with(root: &Path, model: &scripted_model::ScriptedModel, config: Value) -> Self {
        let setup = Setup {
            config,
            sandbox_policy: support::BASH_SANDBOX,
            ..Setup::default()
        };
        Self::start_setup(root, model, &setup).await
    }

    /// [`Served::start_with`] with a database file under `root`, which a later start on
    /// the same `root` reopens (a restart).
    async fn start_on_disk(
        root: &Path,
        model: &scripted_model::ScriptedModel,
        config: Value,
    ) -> Self {
        let setup = Setup {
            config,
            database: Some(root.join("cyber.db")),
            sandbox_policy: support::BASH_SANDBOX,
            ..Setup::default()
        };
        Self::start_setup(root, model, &setup).await
    }

    async fn start_setup(
        root: &Path,
        model: &scripted_model::ScriptedModel,
        setup: &Setup,
    ) -> Self {
        Self::start_serving(root, model, setup, ServeOptions::default()).await
    }

    /// [`Served::start_setup`], with `mdns` (as `cyber serve --mdns` sets it) and the
    /// attach wait of `extra`.
    async fn start_serving(
        root: &Path,
        model: &scripted_model::ScriptedModel,
        setup: &Setup,
        extra: ServeOptions,
    ) -> Self {
        logs();
        let repo = root.join("repo");
        std::fs::create_dir_all(repo.join(".git/hooks")).unwrap();
        let app = ModeApp::build(root, &repo, model, setup).await;
        let paths = app.paths.clone();
        let store = Arc::clone(&app.store);
        let runtime = app.runtime.clone();
        let stop = Arc::new(Notify::new());
        let options = ServeOptions {
            port: Some(0),
            socket: cfg!(unix).then(|| root.join("s.sock")),
            register: true,
            ..extra
        };
        let task = tokio::spawn(cyber_app::run_server_until(
            app,
            options,
            |_| {},
            Arc::clone(&stop),
        ));
        let registration = tokio::time::timeout(Duration::from_secs(20), async {
            loop {
                if let Some(registration) = cyber_app::read_registration(&paths) {
                    break registration;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("the server registers");
        Self {
            root: root.to_path_buf(),
            stop,
            task,
            registration,
            store,
            runtime,
        }
    }

    fn paths(&self) -> Paths {
        Paths {
            data: self.root.join("data"),
            config: self.root.join("config"),
            state: self.root.join("state"),
            cache: self.root.join("cache"),
            tmp: self.root.join("tmp"),
        }
    }

    /// A main-API request in the checkout's Location, with the local password.
    async fn api(&self, method: &str, path: &str, body: Option<Value>) -> (u16, Value) {
        let method = reqwest::Method::from_bytes(method.as_bytes()).unwrap();
        let mut request = reqwest::Client::builder()
            .no_proxy()
            .build()
            .unwrap()
            .request(method, format!("{}/api/v1{path}", self.registration.url))
            .basic_auth("cyber", Some(support::PASSWORD))
            .header(
                "x-cyber-directory",
                self.root.join("repo").display().to_string(),
            );
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request.send().await.unwrap();
        let status = response.status().as_u16();
        (status, response.json().await.unwrap_or(Value::Null))
    }

    /// Create a root Session titled `title`; its ID.
    async fn session(&self, title: &str) -> String {
        let (status, body) = self
            .api(
                "POST",
                "/sessions",
                Some(json!({"mode": "default", "title": title})),
            )
            .await;
        assert_eq!(status, 201, "{body}");
        body["data"]["id"].as_str().unwrap().to_string()
    }

    /// The message records of a Session, oldest first.
    async fn history(&self, session: &str) -> Vec<Value> {
        let (status, body) = self
            .api(
                "GET",
                &format!("/sessions/{session}/messaging?order=asc"),
                None,
            )
            .await;
        assert_eq!(status, 200, "{body}");
        body["data"].as_array().cloned().unwrap_or_default()
    }

    /// The record of message `id` in a Session's history.
    async fn record(&self, session: &str, id: &str) -> Option<Value> {
        self.history(session)
            .await
            .into_iter()
            .find(|record| record["message_id"] == id)
    }

    /// Rewrite the user configuration file in place.
    fn edit_config(&self, edit: impl FnOnce(&mut Value)) {
        let path = self.paths().config.join("cyber.json");
        let mut config: Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        edit(&mut config);
        std::fs::write(&path, config.to_string()).unwrap();
    }

    async fn stop(self) {
        self.stop.notify_one();
        tokio::time::timeout(Duration::from_secs(20), self.task)
            .await
            .expect("the server stops")
            .unwrap()
            .unwrap();
    }
}

/// Structured logs of this test process, written under one temporary directory.
fn logs() -> &'static Path {
    static DIR: OnceLock<tempfile::TempDir> = OnceLock::new();
    let dir = DIR.get_or_init(|| tempfile::tempdir().unwrap()).path();
    cyber_core::log::init(dir, cyber_core::log::Level::Info, false);
    dir
}

fn logged(text: &str) -> bool {
    std::fs::read_dir(logs())
        .unwrap()
        .flatten()
        .any(|file| std::fs::read_to_string(file.path()).is_ok_and(|log| log.contains(text)))
}

/// A client that accepts the self-signed peer certificate (the pinning client is the peer
/// client's) and reports the certificate it saw.
fn peer_client() -> reqwest::Client {
    reqwest::Client::builder()
        .no_proxy()
        .tls_danger_accept_invalid_certs(true)
        .tls_info(true)
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap()
}

async fn peer_get(url: &str, headers: &[(&str, &str)]) -> (u16, Value, Option<Vec<u8>>) {
    let mut request = peer_client().get(url);
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    let response = request.send().await.unwrap();
    let certificate = response
        .extensions()
        .get::<reqwest::tls::TlsInfo>()
        .and_then(|info| info.peer_certificate().map(<[u8]>::to_vec));
    let status = response.status().as_u16();
    (
        status,
        response.json().await.unwrap_or(Value::Null),
        certificate,
    )
}

async fn main_health(registration: &Registration) -> u16 {
    reqwest::Client::builder()
        .no_proxy()
        .build()
        .unwrap()
        .get(format!("{}/api/v1/health", registration.url))
        .send()
        .await
        .unwrap()
        .status()
        .as_u16()
}

/// The options of `cyber serve --mdns`.
fn mdns() -> ServeOptions {
    ServeOptions {
        mdns: true,
        ..ServeOptions::default()
    }
}

/// `peers.listen` opens a second, TLS listener with the server: `server.json` names it and
/// the SPKI fingerprint of the certificate it presents, it serves only `/api/v1/peer/*`
/// (every other path is `404`, even with the local password), peer routes need a grant
/// (`401`), browser requests are `403`, and it closes with the server. The key in
/// `<state>/peer-tls/` survives a restart and a reissued certificate keeps the fingerprint.
#[tokio::test]
async fn the_peer_listener_serves_only_peer_routes() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let model = scripted_model::ScriptedModel::start(support::prompted_call);
    let listen = json!({"listen": {"address": "127.0.0.1:0"}, "machine": "desk"});
    let served = Served::start(&root, &model, listen.clone()).await;
    let registration = served.registration.clone();
    let peer = registration
        .peer_url
        .clone()
        .expect("server.json names the peer listener");
    assert!(peer.starts_with("https://127.0.0.1:"), "{peer}");
    assert_ne!(peer.replace("https", "http"), registration.url);
    let fingerprint = registration.peer_fingerprint.clone().unwrap();
    assert!(
        fingerprint.starts_with("sha256:") && fingerprint.len() == 71,
        "{fingerprint}"
    );
    assert_eq!(main_health(&registration).await, 200);

    // Only peer routes, and none without a grant; the main API's routes and password are
    // not served here.
    let token = "cyp_pgr_unknown_0000000000000000000000000000000000000000000";
    let bearer = format!("Bearer {token}");
    for path in ["/api/v1/sessions", "/api/v1/health", "/openapi.json", "/"] {
        let (status, body, _) =
            peer_get(&format!("{peer}{path}"), &[("authorization", &bearer)]).await;
        assert_eq!(status, 404, "{path}: {body}");
    }
    let basic = format!(
        "Basic {}",
        base64::engine::general_purpose::STANDARD.encode(format!("cyber:{}", support::PASSWORD))
    );
    let (status, _, _) = peer_get(
        &format!("{peer}/api/v1/sessions"),
        &[("authorization", &basic)],
    )
    .await;
    assert_eq!(status, 404);
    let hello = format!("{peer}/api/v1/peer/hello");
    for headers in [
        vec![],
        vec![("authorization", bearer.as_str())],
        vec![("authorization", basic.as_str())],
    ] {
        let (status, body, certificate) = peer_get(&hello, &headers).await;
        assert_eq!(status, 401, "{body}");
        assert_eq!(body["_tag"], "UnauthorizedError", "{body}");
        let presented =
            cyber_server::http::peer::tls::certificate_fingerprint(&certificate.unwrap()).unwrap();
        assert_eq!(presented, fingerprint);
    }

    // Browser requests are refused before routing and authentication.
    for path in ["/api/v1/peer/hello", "/api/v1/sessions"] {
        let (status, body, _) = peer_get(
            &format!("{peer}{path}"),
            &[
                ("origin", "http://192.168.1.20:8080"),
                ("authorization", &bearer),
            ],
        )
        .await;
        assert_eq!(status, 403, "{path}: {body}");
    }

    // TLS only: plain HTTP on the TLS port gets no answer.
    let plain = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap();
    let answer = plain
        .get(format!(
            "{}/api/v1/peer/hello",
            peer.replace("https", "http")
        ))
        .send()
        .await;
    assert!(answer.is_err(), "{answer:?}");

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let tls = served.paths().state.join("peer-tls");
        let mode = |path: &Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&tls), 0o700);
        for file in ["key.pem", "cert.pem"] {
            assert_eq!(mode(&tls.join(file)), 0o600, "{file}");
        }
    }

    // The listener closes with the server; the next start reissues the certificate from
    // the kept key.
    let paths = served.paths();
    served.stop().await;
    assert!(cyber_app::read_registration(&paths).is_none());
    let port = peer.rsplit(':').next().unwrap();
    assert!(
        tokio::net::TcpStream::connect(format!("127.0.0.1:{port}"))
            .await
            .is_err(),
        "the peer listener closed"
    );
    let cert = paths.state.join("peer-tls").join("cert.pem");
    std::fs::remove_file(&cert).unwrap();
    let served = Served::start(&root, &model, listen).await;
    assert_eq!(
        served.registration.peer_fingerprint.as_deref(),
        Some(fingerprint.as_str())
    );
    assert!(cert.exists());
    let (_, _, certificate) = peer_get(
        &format!(
            "{}/api/v1/peer/hello",
            served.registration.peer_url.clone().unwrap()
        ),
        &[],
    )
    .await;
    assert_eq!(
        cyber_server::http::peer::tls::certificate_fingerprint(&certificate.unwrap()).unwrap(),
        fingerprint
    );
    served.stop().await;
}

/// `tls: false` off loopback never opens a listener: the server logs why, registers no
/// peer listener and serves the main API. On loopback (an SSH-forwarded port) a plain
/// listener opens with the same rules.
#[tokio::test]
async fn a_plain_peer_listener_opens_only_on_loopback() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let model = scripted_model::ScriptedModel::start(support::prompted_call);
    let served = Served::start(
        &root,
        &model,
        json!({"listen": {"address": "0.0.0.0:0", "tls": false}}),
    )
    .await;
    assert_eq!(served.registration.peer_url, None);
    assert_eq!(served.registration.peer_fingerprint, None);
    assert_eq!(main_health(&served.registration).await, 200);
    assert!(logged("peer listener not started"));
    assert!(logged(
        "peers.listen.tls: false is allowed only on a loopback address"
    ));
    assert!(!served.paths().state.join("peer-tls").exists());
    served.stop().await;

    let served = Served::start(
        &root,
        &model,
        json!({"listen": {"address": "127.0.0.1:0", "tls": false}}),
    )
    .await;
    let peer = served.registration.peer_url.clone().unwrap();
    assert!(peer.starts_with("http://127.0.0.1:"), "{peer}");
    assert!(served.registration.peer_fingerprint.is_some());
    let (status, _, certificate) = peer_get(&format!("{peer}/api/v1/sessions"), &[]).await;
    assert_eq!((status, certificate), (404, None));
    let (status, _, _) = peer_get(&format!("{peer}/api/v1/peer/hello"), &[]).await;
    assert_eq!(status, 401);
    let (status, _, _) = peer_get(
        &format!("{peer}/api/v1/peer/hello"),
        &[("origin", "http://192.168.1.20:8080")],
    )
    .await;
    assert_eq!(status, 403);
    served.stop().await;
}

/// Without `peers.listen` nothing listens for peers.
#[tokio::test]
async fn no_peer_listener_without_listen() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let model = scripted_model::ScriptedModel::start(support::prompted_call);
    let served = Served::start(&root, &model, json!({"machine": "desk"})).await;
    assert_eq!(served.registration.peer_url, None);
    assert!(!served.paths().state.join("peer-tls").exists());
    served.stop().await;
}

/// Grants on the production server: `hello` over the TLS peer listener answers a grant's
/// token with this machine, the registered fingerprint and the grant's scopes; a message
/// grant cannot reach attach routes; the main API refuses the same token; a revoked grant
/// fails the next request.
#[tokio::test]
async fn grants_authenticate_on_the_production_peer_listener() {
    use cyber_server::runtime::peers::{NewGrant, grants};
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let model = scripted_model::ScriptedModel::start(support::prompted_call);
    let listen = json!({"listen": {"address": "127.0.0.1:0"}, "machine": "desk"});
    let served = Served::start(&root, &model, listen).await;
    let peer = served.registration.peer_url.clone().unwrap();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;
    let request = NewGrant {
        name: "laptop".into(),
        scopes: vec![],
        expires_ms: None,
    };
    let token = grants::create(&served.store, request, now).unwrap().token;
    let bearer = format!("Bearer {token}");

    let (status, body, _) = peer_get(
        &format!("{peer}/api/v1/peer/hello"),
        &[("authorization", &bearer)],
    )
    .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["machine"], "desk");
    assert_eq!(body["grant"], "laptop");
    assert_eq!(body["scopes"], json!(["message"]));
    assert_eq!(body["version"], cyber_app::version());
    assert_eq!(
        body["fingerprint"].as_str(),
        served.registration.peer_fingerprint.as_deref()
    );

    let (status, body, _) = peer_get(
        &format!("{peer}/api/v1/peer/attach/ses_x/events"),
        &[("authorization", &bearer)],
    )
    .await;
    assert_eq!(
        (status, &body["_tag"]),
        (403, &json!("PeerScopeError")),
        "{body}"
    );

    let main = reqwest::Client::builder()
        .no_proxy()
        .build()
        .unwrap()
        .get(format!("{}/api/v1/sessions", served.registration.url))
        .header("authorization", &bearer)
        .send()
        .await
        .unwrap();
    assert_eq!(main.status().as_u16(), 401);

    assert!(grants::revoke(&served.store, "laptop", now + 1).unwrap());
    let (status, _, _) = peer_get(
        &format!("{peer}/api/v1/peer/hello"),
        &[("authorization", &bearer)],
    )
    .await;
    assert_eq!(status, 401);
    served.stop().await;
}

/// A request to a peer listener with a grant's token.
async fn peer_call(method: &str, url: &str, token: &str, body: Option<Value>) -> (u16, Value) {
    let method = reqwest::Method::from_bytes(method.as_bytes()).unwrap();
    let mut request = peer_client().request(method, url).bearer_auth(token);
    if let Some(body) = body {
        request = request.json(&body);
    }
    let response = request.send().await.unwrap();
    let status = response.status().as_u16();
    (status, response.json().await.unwrap_or(Value::Null))
}

/// Wait until `probe` answers something.
async fn eventually<T, F: std::future::Future<Output = Option<T>>>(
    what: &str,
    mut probe: impl FnMut() -> F,
) -> T {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        if let Some(found) = probe().await {
            return found;
        }
        assert!(tokio::time::Instant::now() < deadline, "timed out: {what}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// Requirement "Direct peer transport" between two production servers on this host:
/// `laptop` pairs with `desk` (a grant on `desk`, a pinned remote on `laptop`), and
/// - "Two laptops on one network": the model's `send_message({ to: "migration@desk" })`
///   on `laptop` crosses the TLS peer listener and is held on `desk` until its user
///   releases it; both Sessions record it with `origin_kind: "peer"`, the sender with the
///   peer and the Session there, and `GET /peer/messages/:id` follows the release;
/// - "Sender cannot claim another machine": a body naming `build` from grant `laptop` is
///   `build@laptop`, the `*@workstation` refusal does not apply, the default `hold` does,
///   and a name claiming another machine is refused;
/// - "Unknown peer name": `api@nas` fails with `PeerNotConfiguredError` before any request;
/// - "Retry after a timeout": the same `msgx_` ID again returns the first outcome and
///   stores one message, whether the first answer was lost or the first send failed.
#[tokio::test]
async fn a_message_to_a_peer_is_held_there_and_named_by_grant() {
    use cyber_server::runtime::peers::{NewGrant, grants};
    let (desk_temp, laptop_temp) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let model = scripted_model::ScriptedModel::start(support::prompted_call);
    let desk = Served::start_with(
        &desk_temp.path().canonicalize().unwrap(),
        &model,
        json!({
            "peers": {"listen": {"address": "127.0.0.1:0"}, "machine": "desk"},
            "messaging": {"inbound": {"refuse": ["*@workstation"]}},
        }),
    )
    .await;
    let peer = desk.registration.peer_url.clone().unwrap();
    let fingerprint = desk.registration.peer_fingerprint.clone().unwrap();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;
    let grant = NewGrant {
        name: "laptop".into(),
        scopes: vec![],
        expires_ms: None,
    };
    let token = grants::create(&desk.store, grant, now).unwrap().token;
    let remote = json!({"name": "desk", "url": peer, "fingerprint": fingerprint, "token": token});
    let laptop = Served::start_with(
        &laptop_temp.path().canonicalize().unwrap(),
        &model,
        json!({
            "peers": {"machine": "laptop", "remotes": [remote]},
            "permissions": {"message.send": {"*@desk": "allow"}},
        }),
    )
    .await;
    let migration = desk.session("migration").await;
    let review = laptop.session("review").await;

    // Unknown peer name: nothing is sent or recorded.
    let (status, body) = laptop
        .api(
            "POST",
            "/messaging/send",
            Some(json!({"from": review, "to": "api@nas", "text": "hi"})),
        )
        .await;
    assert_eq!(
        (status, &body["_tag"]),
        (404, &json!("PeerNotConfiguredError")),
        "{body}"
    );
    assert_eq!(body["peer"], "nas", "{body}");
    assert!(laptop.history(&review).await.is_empty());

    // Two laptops on one network: the model's send is held on desk.
    let (status, body) = laptop
        .api(
            "POST",
            &format!("/sessions/{review}/prompt"),
            Some(json!({"parts": [{"type": "text", "text":
                "TOOL<<send_message>>{\"to\":\"migration@desk\",\"text\":\"schema ready?\"}"}],
                "delivery": "queue"})),
        )
        .await;
    assert_eq!(status, 202, "{body}");
    let sent = eventually("the sender's out record", || async {
        laptop.history(&review).await.into_iter().next()
    })
    .await;
    let id = sent["message_id"].as_str().unwrap().to_string();
    assert_eq!(sent["direction"], "out", "{sent}");
    assert_eq!(sent["to"], "migration@desk", "{sent}");
    assert_eq!(sent["origin_kind"], "peer", "{sent}");
    assert_eq!(sent["outcome"], "held", "{sent}");
    assert_eq!(sent["remote_peer"], "desk", "{sent}");
    assert_eq!(sent["remote_session_id"], migration.as_str(), "{sent}");
    assert_eq!(sent["hops"], 1, "{sent}");
    let held = desk.record(&migration, &id).await.expect("desk records it");
    assert_eq!(held["direction"], "in", "{held}");
    assert_eq!(held["from"], "review@laptop", "{held}");
    assert_eq!(held["origin_kind"], "peer", "{held}");
    assert_eq!(held["outcome"], "held", "{held}");
    assert_eq!(held["text"], "schema ready?", "{held}");
    assert_eq!(held["remote_peer"], "laptop", "{held}");
    assert_eq!(held["remote_session_id"], review.as_str(), "{held}");
    assert_eq!(held["hops"], 1, "{held}");
    assert!(held.get("peer_session_id").is_none(), "{held}");
    let outcome_url = format!("{peer}/api/v1/peer/messages/{id}");
    let (status, outcome) = peer_call("GET", &outcome_url, &token, None).await;
    assert_eq!(
        (status, &outcome["outcome"]),
        (200, &json!("held")),
        "{outcome}"
    );
    assert_eq!(outcome["target_session_id"], migration.as_str());
    // Held until desk's user releases it.
    let (status, body) = desk
        .api(
            "POST",
            &format!("/sessions/{migration}/inbox/{id}/release"),
            None,
        )
        .await;
    assert_eq!(status, 204, "{body}");
    let (_, outcome) = peer_call("GET", &outcome_url, &token, None).await;
    assert_eq!(outcome["outcome"], "released", "{outcome}");
    // Another grant's messages and unknown IDs are not this grant's to see.
    let (status, _) = peer_call(
        "GET",
        &format!("{peer}/api/v1/peer/messages/msgx_unknown"),
        &token,
        None,
    )
    .await;
    assert_eq!(status, 404);

    // Sender cannot claim another machine: the grant names the sender.
    let claim = json!({"id": "msgx_claim", "to": "migration", "text": "from build",
        "hops": 1, "from_session": "ses_01JBUILD", "from_name": "build"});
    let messages = format!("{peer}/api/v1/peer/messages");
    let (status, first) = peer_call("POST", &messages, &token, Some(claim.clone())).await;
    assert_eq!(
        (status, &first["outcome"]),
        (200, &json!("held")),
        "{first}"
    );
    assert_eq!(first["target_session_id"], migration.as_str());
    let named = desk.record(&migration, "msgx_claim").await.unwrap();
    assert_eq!(named["from"], "build@laptop", "{named}");
    assert_eq!(named["remote_peer"], "laptop", "{named}");
    for (field, value) in [
        ("from_name", json!("build@workstation")),
        ("from_session", json!("ses_x@workstation")),
        ("from_machine", json!("workstation")),
    ] {
        let mut spoof = claim.clone();
        spoof["id"] = json!("msgx_spoof");
        spoof[field] = value;
        let (status, body) = peer_call("POST", &messages, &token, Some(spoof)).await;
        assert_eq!(status, 400, "{field}: {body}");
    }
    assert!(desk.record(&migration, "msgx_spoof").await.is_none());

    // Retry after a timeout (the answer was lost): the first outcome, one message.
    let (status, again) = peer_call("POST", &messages, &token, Some(claim)).await;
    assert_eq!(
        (status, &again["outcome"]),
        (200, &json!("held")),
        "{again}"
    );
    assert_eq!(again["replayed"], true, "{again}");
    let lost = json!({"id": "msgx_lost", "to": "migration", "text": "lost answer",
        "hops": 1, "from_session": review, "from_name": "review"});
    let (status, _) = peer_call("POST", &messages, &token, Some(lost)).await;
    assert_eq!(status, 200);
    let send = |id: &str, text: &str| json!({"id": id, "from": review, "to": "migration@desk", "text": text});
    let (status, retried) = laptop
        .api(
            "POST",
            "/messaging/send",
            Some(send("msgx_lost", "lost answer")),
        )
        .await;
    assert_eq!(status, 200, "{retried}");
    assert_eq!(retried["data"]["outcome"], "held", "{retried}");
    assert_eq!(retried["data"]["replayed"], true, "{retried}");
    assert_eq!(
        retried["data"]["to"]["session_id"],
        migration.as_str(),
        "{retried}"
    );

    // Retry after a timeout (the first send failed): `failed`, then the outcome.
    let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let dead = format!("https://{}", closed.local_addr().unwrap());
    drop(closed);
    laptop.edit_config(|config| config["peers"]["remotes"][0]["url"] = json!(dead));
    let (status, body) = laptop
        .api(
            "POST",
            "/messaging/send",
            Some(send("msgx_retry", "try again")),
        )
        .await;
    assert_eq!(
        (status, &body["_tag"]),
        (502, &json!("PeerUnreachableError")),
        "{body}"
    );
    assert_eq!(body["peer"], "desk", "{body}");
    let failed = laptop.record(&review, "msgx_retry").await.unwrap();
    assert_eq!(
        (&failed["outcome"], &failed["reason"]),
        (&json!("failed"), &json!("unreachable"))
    );
    assert!(failed.get("text").is_none(), "{failed}");
    assert!(desk.record(&migration, "msgx_retry").await.is_none());
    laptop.edit_config(|config| config["peers"]["remotes"][0]["url"] = json!(peer));
    let (status, body) = laptop
        .api(
            "POST",
            "/messaging/send",
            Some(send("msgx_retry", "try again")),
        )
        .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["data"]["outcome"], "held", "{body}");
    let recorded = laptop.record(&review, "msgx_retry").await.unwrap();
    assert_eq!(recorded["outcome"], "held", "{recorded}");
    assert_eq!(
        recorded["remote_session_id"],
        migration.as_str(),
        "{recorded}"
    );
    // A send already answered replays locally, without the peer.
    laptop.edit_config(|config| config["peers"]["remotes"][0]["url"] = json!(dead));
    let (status, body) = laptop
        .api(
            "POST",
            "/messaging/send",
            Some(send("msgx_retry", "try again")),
        )
        .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["data"]["replayed"], true, "{body}");
    let ids: Vec<_> = desk
        .history(&migration)
        .await
        .into_iter()
        .map(|record| record["message_id"].as_str().unwrap().to_string())
        .collect();
    for id in ["msgx_claim", "msgx_lost", "msgx_retry"] {
        assert_eq!(ids.iter().filter(|i| *i == id).count(), 1, "{id}: {ids:?}");
    }
    laptop.stop().await;
    desk.stop().await;
}

/// The live and durable events of one Session's SSE stream (`GET /sessions/:id/events`,
/// from the start), as `(event, data)` pairs.
fn session_events(
    served: &Served,
    session: &str,
) -> tokio::sync::mpsc::UnboundedReceiver<(String, Value)> {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let request = reqwest::Client::builder()
        .no_proxy()
        .build()
        .unwrap()
        .get(format!(
            "{}/api/v1/sessions/{session}/events",
            served.registration.url
        ))
        .basic_auth("cyber", Some(support::PASSWORD));
    tokio::spawn(async move {
        let mut response = request.send().await.unwrap();
        let mut buffer = String::new();
        while let Ok(Some(chunk)) = response.chunk().await {
            buffer.push_str(&String::from_utf8_lossy(&chunk));
            while let Some(end) = buffer.find("\n\n") {
                let frame: String = buffer.drain(..end + 2).collect();
                let field = |name: &str| {
                    frame
                        .lines()
                        .find_map(|line| line.strip_prefix(name))
                        .map(|value| value.trim().to_string())
                };
                if let (Some(kind), Some(data)) = (field("event:"), field("data:")) {
                    let data = serde_json::from_str(&data).unwrap_or(Value::Null);
                    if tx.send((kind, data)).is_err() {
                        return;
                    }
                }
            }
        }
    });
    rx
}

/// The next event of `kind` about message `id`.
async fn next_event(
    events: &mut tokio::sync::mpsc::UnboundedReceiver<(String, Value)>,
    kind: &str,
    id: &str,
) -> Value {
    tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            let (seen, data) = events.recv().await.expect("the stream stays open");
            if seen == kind && data["data"]["message_id"] == id {
                return data["data"].clone();
            }
        }
    })
    .await
    .unwrap_or_else(|_| panic!("no {kind} for {id}"))
}

/// `created_ms` and `next_poll_ms` of the sender's record of message `id`.
fn poll_times(served: &Served, id: &str) -> (i64, Option<i64>) {
    let id = id.to_owned();
    served
        .store
        .read(move |db| {
            Ok(db.query_row(
                "SELECT created_ms, next_poll_ms FROM xsession_message
                 WHERE message_id = ?1 AND direction = 'out'",
                [id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?)
        })
        .unwrap()
}

/// Requirement "Peer outcome tracking", scenario "Release seen by the sender", between two
/// production servers on this host, with a test clock for the backoff: `laptop` sends
/// three messages that `desk` holds; the sender follows them from the poll times its store
/// keeps (5 s, then 15 s later, ...); `desk` releases one and drops another while
/// `laptop` is stopped; after `laptop` restarts, its next poll appends
/// `messaging.message.released.1` and `messaging.message.dropped.1` to the sender Session
/// and publishes the live `messaging.outcome` event on its stream, and the third message,
/// still held after 7 days, becomes `expired`. Changed messages are never polled again.
#[tokio::test]
async fn a_peer_release_reaches_the_sender_across_a_restart() {
    use cyber_server::runtime::messaging::tracker::TRACK_FOR_MS;
    use cyber_server::runtime::peers::{NewGrant, grants};
    let (desk_temp, laptop_temp) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let model = scripted_model::ScriptedModel::start(support::prompted_call);
    let desk = Served::start_with(
        &desk_temp.path().canonicalize().unwrap(),
        &model,
        json!({"peers": {"listen": {"address": "127.0.0.1:0"}, "machine": "desk"}}),
    )
    .await;
    let peer = desk.registration.peer_url.clone().unwrap();
    let fingerprint = desk.registration.peer_fingerprint.clone().unwrap();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;
    let grant = NewGrant {
        name: "laptop".into(),
        scopes: vec![],
        expires_ms: None,
    };
    let token = grants::create(&desk.store, grant, now).unwrap().token;
    let remote = json!({"name": "desk", "url": peer, "fingerprint": fingerprint, "token": token});
    let laptop_config = json!({
        "peers": {"machine": "laptop", "remotes": [remote]},
        "permissions": {"message.send": {"*@desk": "allow"}},
    });
    let laptop_root = laptop_temp.path().canonicalize().unwrap();
    let laptop = Served::start_on_disk(&laptop_root, &model, laptop_config.clone()).await;
    let migration = desk.session("migration").await;
    let review = laptop.session("review").await;

    let ids = ["msgx_release", "msgx_drop", "msgx_expire"];
    for id in ids {
        let (status, body) = laptop
            .api(
                "POST",
                "/messaging/send",
                Some(json!({"id": id, "from": review, "to": "migration@desk", "text": id})),
            )
            .await;
        assert_eq!(status, 200, "{body}");
        assert_eq!(body["data"]["outcome"], "held", "{body}");
    }
    // The first poll is due 5 s after the send.
    let sent: Vec<i64> = ids.iter().map(|id| poll_times(&laptop, id).0).collect();
    for (id, sent) in ids.iter().zip(&sent) {
        assert_eq!(poll_times(&laptop, id), (*sent, Some(sent + 5_000)), "{id}");
    }
    let last = *sent.iter().max().unwrap();
    let messaging = laptop.runtime.messaging();
    // Still held: the next poll is 15 s later, and nothing is due before it.
    let tracked = messaging.track_outcomes(last + 5_000).await.unwrap();
    assert_eq!((tracked.changed, tracked.expired), (0, 0), "{tracked:?}");
    for (id, sent) in ids.iter().zip(&sent) {
        assert_eq!(poll_times(&laptop, id).1, Some(sent + 20_000), "{id}");
    }
    let early = messaging.track_outcomes(last + 6_000).await.unwrap();
    assert_eq!(early, Default::default());

    // desk's user releases one and drops another while laptop is stopped.
    laptop.stop().await;
    for (id, action) in [("msgx_release", "release"), ("msgx_drop", "drop")] {
        let (status, body) = desk
            .api(
                "POST",
                &format!("/sessions/{migration}/inbox/{id}/{action}"),
                None,
            )
            .await;
        assert!(status == 204 || status == 200, "{action}: {status} {body}");
    }
    let laptop = Served::start_on_disk(&laptop_root, &model, laptop_config).await;
    let mut events = session_events(&laptop, &review);
    // Polling resumes from the stored times: the next poll sees both changes.
    let messaging = laptop.runtime.messaging();
    let tracked = messaging.track_outcomes(last + 20_000).await.unwrap();
    assert_eq!(tracked.expired, 0, "{tracked:?}");
    let released = next_event(&mut events, "messaging.message.released.1", "msgx_release").await;
    assert_eq!(released["direction"], "out", "{released}");
    let live = next_event(&mut events, "messaging.outcome", "msgx_release").await;
    assert_eq!(live["outcome"], "released", "{live}");
    assert_eq!(live["to"], "migration@desk", "{live}");
    assert_eq!(live["session_id"], review.as_str(), "{live}");
    let dropped = next_event(&mut events, "messaging.message.dropped.1", "msgx_drop").await;
    assert_eq!(dropped["direction"], "out", "{dropped}");
    let live = next_event(&mut events, "messaging.outcome", "msgx_drop").await;
    assert_eq!(live["outcome"], "dropped", "{live}");
    let outcome = |id: &'static str| {
        let laptop = &laptop;
        let review = review.clone();
        async move { laptop.record(&review, id).await.unwrap()["outcome"].clone() }
    };
    assert_eq!(outcome("msgx_release").await, "released");
    assert_eq!(outcome("msgx_drop").await, "dropped");
    assert_eq!(outcome("msgx_expire").await, "held");
    assert_eq!(poll_times(&laptop, "msgx_release").1, None);
    assert_eq!(poll_times(&laptop, "msgx_drop").1, None);
    assert_eq!(poll_times(&laptop, "msgx_expire").1, Some(sent[2] + 80_000));

    // Still held after 7 days: `expired`, and tracking ends.
    let end = sent[2] + TRACK_FOR_MS;
    let tracked = messaging.track_outcomes(end).await.unwrap();
    assert_eq!((tracked.changed, tracked.expired), (0, 1), "{tracked:?}");
    let expired = next_event(&mut events, "messaging.message.expired.1", "msgx_expire").await;
    assert_eq!(expired["direction"], "out", "{expired}");
    let live = next_event(&mut events, "messaging.outcome", "msgx_expire").await;
    assert_eq!(live["outcome"], "expired", "{live}");
    assert_eq!(outcome("msgx_expire").await, "expired");
    assert_eq!(poll_times(&laptop, "msgx_expire").1, None);
    let after = messaging.track_outcomes(end + TRACK_FOR_MS).await.unwrap();
    assert_eq!(after, Default::default());
    // The target keeps its own outcome; only the sender stopped following.
    let (_, remote) = peer_call(
        "GET",
        &format!("{peer}/api/v1/peer/messages/msgx_expire"),
        &token,
        None,
    )
    .await;
    assert_eq!(remote["outcome"], "held", "{remote}");

    // On the real clock, the server's own tracker sees a release within its next polls.
    let (status, body) = laptop
        .api(
            "POST",
            "/messaging/send",
            Some(
                json!({"id": "msgx_live", "from": review, "to": "migration@desk", "text": "live"}),
            ),
        )
        .await;
    assert_eq!(body["data"]["outcome"], "held", "{status} {body}");
    let (status, body) = desk
        .api(
            "POST",
            &format!("/sessions/{migration}/inbox/msgx_live/release"),
            None,
        )
        .await;
    assert_eq!(status, 204, "{body}");
    let live = next_event(&mut events, "messaging.outcome", "msgx_live").await;
    assert_eq!(live["outcome"], "released", "{live}");
    assert_eq!(outcome("msgx_live").await, "released");
    laptop.stop().await;
    desk.stop().await;
}

/// Requirement "Peer watches and listing" between two production servers on this host:
/// `laptop` lists `desk`'s Sessions next to its own and watches one of them.
/// - The listing (the model's `list_sessions` and `GET /messaging/sessions`) shows
///   `migration@desk` with `kind: "peer"` and desk's state; desk answers its peer route
///   with root Sessions only, located by workspace name, at most 50;
/// - "Refused grant hides Sessions": a desk Session that refuses `laptop`'s messages is
///   not listed, and its status is `404`, so it cannot be watched either;
/// - "Peer offline": a remote whose port is closed and one that never answers (cut at
///   5 s) are each one `*@<peer>` entry, `offline`, that cannot receive, and a send to
///   one fails with `PeerUnreachableError`;
/// - "Watch a remote Session": the model's `watch_session({ target: "migration@desk" })`
///   while it runs creates a watch on `ses_…@desk` that fires once it is idle, through the
///   server's own 15 s poller, and the notice reaches the watcher's model.
#[tokio::test]
async fn peer_sessions_are_listed_and_watched() {
    use cyber_server::runtime::peers::{NewGrant, grants};
    let (desk_temp, laptop_temp) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let model = scripted_model::ScriptedModel::start(support::prompted_call);
    let desk = Served::start_with(
        &desk_temp.path().canonicalize().unwrap(),
        &model,
        json!({"peers": {"listen": {"address": "127.0.0.1:0"}, "machine": "desk"}}),
    )
    .await;
    let peer = desk.registration.peer_url.clone().unwrap();
    let fingerprint = desk.registration.peer_fingerprint.clone().unwrap();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;
    let grant = NewGrant {
        name: "laptop".into(),
        scopes: vec![],
        expires_ms: None,
    };
    let token = grants::create(&desk.store, grant, now).unwrap().token;
    let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let dead = format!("https://{}", closed.local_addr().unwrap());
    drop(closed);
    // Accepts connections and never answers.
    let silent = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let slow = format!("https://{}", silent.local_addr().unwrap());
    let held = tokio::spawn(async move {
        let mut open = Vec::new();
        while let Ok((stream, _)) = silent.accept().await {
            open.push(stream);
        }
    });
    let remote = |name: &str, url: &str, token: &str| json!({"name": name, "url": url, "fingerprint": fingerprint, "token": token});
    let laptop = Served::start_with(
        &laptop_temp.path().canonicalize().unwrap(),
        &model,
        json!({
            "peers": {"machine": "laptop", "remotes": [
                remote("desk", &peer, &token),
                remote("nas", &dead, "cyp_pgrx_unused"),
                remote("slow", &slow, "cyp_pgrx_unused"),
            ]},
            "permissions": {"message.send": {"*@desk": "allow", "*@nas": "allow"}},
        }),
    )
    .await;
    let migration = desk.session("migration").await;
    let hidden = desk.session("hidden-from-laptop").await;
    let (status, body) = desk
        .api(
            "PATCH",
            &format!("/sessions/{hidden}"),
            Some(json!({"messaging": {"inbound": "refuse"}})),
        )
        .await;
    assert_eq!(status, 200, "{body}");
    let review = laptop.session("review").await;
    // migration runs until the gate opens.
    let (status, body) = desk
        .api(
            "POST",
            &format!("/sessions/{migration}/prompt"),
            Some(json!({"parts": [{"type": "text", "text": "GATE<<migrate>>"}]})),
        )
        .await;
    assert_eq!(status, 202, "{body}");
    eventually("migration runs", || async {
        desk.runtime.is_running(&migration).then_some(())
    })
    .await;

    // desk's peer route: what the grant may message, by workspace name only.
    let (status, listed) =
        peer_call("GET", &format!("{peer}/api/v1/peer/sessions"), &token, None).await;
    assert_eq!(status, 200, "{listed}");
    let entries = listed["sessions"].as_array().unwrap();
    assert_eq!(entries.len(), 1, "{listed}");
    assert_eq!(entries[0]["session_id"], migration.as_str(), "{listed}");
    assert_eq!(entries[0]["state"], "running", "{listed}");
    assert_eq!(entries[0]["location"]["directory"], "repo", "{listed}");
    let status_url = |id: &str| format!("{peer}/api/v1/peer/sessions/{id}/status");
    let (status, body) = peer_call("GET", &status_url(&hidden), &token, None).await;
    assert_eq!(status, 404, "{body}");
    let (status, body) = peer_call("GET", &status_url("migration"), &token, None).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(
        (&body["session_id"], &body["state"]),
        (&json!(migration), &json!("running"))
    );

    // The listing on laptop: its own Sessions, desk's, and one entry per silent peer.
    let started = std::time::Instant::now();
    let (status, body) = laptop
        .api("GET", &format!("/messaging/sessions?from={review}"), None)
        .await;
    let took = started.elapsed();
    assert_eq!(status, 200, "{body}");
    assert!(
        took >= Duration::from_secs(4) && took < Duration::from_secs(9),
        "the silent peer is cut at 5 s: {took:?}"
    );
    let sessions = body["data"].as_array().unwrap().clone();
    let entry = |address: &str| {
        sessions
            .iter()
            .find(|s| s["address"] == address)
            .cloned()
            .unwrap_or_else(|| panic!("{address}: {sessions:?}"))
    };
    let remote_migration = entry("migration@desk");
    assert_eq!(remote_migration["kind"], "peer");
    assert_eq!(remote_migration["state"], "running");
    assert_eq!(remote_migration["canReceive"], true);
    assert_eq!(
        remote_migration["session_id"],
        format!("{migration}@desk").as_str()
    );
    for name in ["nas", "slow"] {
        let down = entry(&format!("*@{name}"));
        assert_eq!(
            (&down["kind"], &down["state"], &down["canReceive"]),
            (&json!("peer"), &json!("offline"), &json!(false)),
            "{down}"
        );
    }
    assert!(
        !body.to_string().contains("hidden-from-laptop") && !body.to_string().contains(&hidden),
        "{body}"
    );
    // Cached for 15 s: asking again does not wait for the silent peer.
    let started = std::time::Instant::now();
    let (status, _) = laptop.api("GET", "/messaging/sessions", None).await;
    assert_eq!(status, 200);
    assert!(started.elapsed() < Duration::from_secs(2));
    // The model's list_sessions shows the same peer entries.
    let (status, body) = laptop
        .api(
            "POST",
            &format!("/sessions/{review}/prompt"),
            Some(json!({"parts": [{"type": "text", "text": "TOOL<<list_sessions>>{}"}]})),
        )
        .await;
    assert_eq!(status, 202, "{body}");
    let listing = eventually("the list_sessions result", || async {
        model.requests().into_iter().find_map(|request| {
            let text = request["messages"].to_string();
            (text.contains("migration@desk") && text.contains("*@nas")).then_some(text)
        })
    })
    .await;
    assert!(!listing.contains("hidden-from-laptop"), "{listing}");
    tokio::time::timeout(Duration::from_secs(60), laptop.runtime.wait_idle(&review))
        .await
        .unwrap();

    // Peer offline: a send fails locally.
    let (status, body) = laptop
        .api(
            "POST",
            "/messaging/send",
            Some(json!({"from": review, "to": "api@nas", "text": "hi"})),
        )
        .await;
    assert_eq!(
        (status, &body["_tag"]),
        (502, &json!("PeerUnreachableError")),
        "{body}"
    );

    // A Session desk hides cannot be watched; one desk does not have is not found.
    for target in ["hidden-from-laptop@desk", "nobody@desk"] {
        let (status, body) = laptop
            .api(
                "POST",
                "/messaging/watch",
                Some(json!({"from": review, "target": target})),
            )
            .await;
        assert_eq!(
            (status, &body["_tag"]),
            (404, &json!("AddressNotFoundError")),
            "{target}: {body}"
        );
    }
    assert!(laptop.runtime.watch_rows(&review).unwrap().is_empty());

    // Watch a remote Session: the model watches migration@desk while it runs.
    let (status, body) = laptop
        .api(
            "POST",
            &format!("/sessions/{review}/prompt"),
            Some(json!({"parts": [{"type": "text", "text":
                "TOOL<<watch_session>>{\"target\":\"migration@desk\"}"}]})),
        )
        .await;
    assert_eq!(status, 202, "{body}");
    let watch = eventually("the peer watch", || async {
        laptop
            .runtime
            .watch_rows(&review)
            .unwrap()
            .into_iter()
            .next()
    })
    .await;
    assert_eq!(watch.status, "active", "{watch:?}");
    assert_eq!(watch.target_peer.as_deref(), Some("desk"));
    assert_eq!(watch.target_id, format!("{migration}@desk"));
    assert_eq!(watch.target_address, "migration@desk");
    // The 12-hour expiry of local watches (taken just before the record is written).
    let ttl = watch.expires_ms - watch.created_ms;
    assert!(
        (12 * 3_600_000 - 5_000..=12 * 3_600_000).contains(&ttl),
        "{ttl}"
    );
    // Still running: a poll fires nothing.
    tokio::time::timeout(Duration::from_secs(60), laptop.runtime.wait_idle(&review))
        .await
        .unwrap();
    let fired = laptop
        .runtime
        .messaging()
        .poll_remote_watches()
        .await
        .unwrap();
    assert_eq!(fired, 0);

    // migration goes idle; the server's poller fires the watch within 15 s.
    model.open_gate();
    tokio::time::timeout(Duration::from_secs(60), desk.runtime.wait_idle(&migration))
        .await
        .unwrap();
    let idle_at = std::time::Instant::now();
    let fired = eventually("the watch fires", || async {
        laptop
            .runtime
            .watch_rows(&review)
            .unwrap()
            .into_iter()
            .find(|w| w.id == watch.id && w.status != "active")
    })
    .await;
    assert!(
        idle_at.elapsed() <= Duration::from_secs(16),
        "{:?}",
        idle_at.elapsed()
    );
    assert_eq!(fired.status, "fired", "{fired:?}");
    assert_eq!(
        fired.notice_id.as_deref(),
        Some(format!("msgx_watch_{}", watch.id).as_str())
    );
    let notice = eventually("the notice reaches the model", || async {
        model.requests().into_iter().find_map(|request| {
            let text = request["messages"].to_string();
            text.contains("session_idle_notice target=\\\"migration@desk\\\"")
                .then_some(text)
        })
    })
    .await;
    assert!(notice.contains("status=\\\"idle\\\""), "{notice}");
    // It fired once: later polls send nothing.
    let again = laptop
        .runtime
        .messaging()
        .poll_remote_watches()
        .await
        .unwrap();
    assert_eq!(again, 0);
    held.abort();
    laptop.stop().await;
    desk.stop().await;
}

/// Why mDNS cannot be exercised on this host, if it cannot: a probe one responder
/// publishes on the IPv4 interfaces (loopback off, as the server publishes) must be
/// resolved by another responder within 5 s. CI runners often have no multicast.
fn mdns_unavailable() -> Option<String> {
    use mdns_sd::{IfKind, ServiceDaemon, ServiceEvent, ServiceInfo};
    const PROBE: &str = "_cyberprobe._tcp.local.";
    let name = format!("probe-{}", ulid::Ulid::new().to_string().to_lowercase());
    let publisher = ServiceDaemon::new().map_err(|e| e.to_string()).ok()?;
    let _ = publisher.disable_interface(vec![IfKind::LoopbackV4, IfKind::LoopbackV6]);
    let mut info = ServiceInfo::new(PROBE, &name, "cyber-probe.local.", "", 9, &[("v", "1")][..])
        .unwrap()
        .enable_addr_auto();
    info.set_interfaces(vec![IfKind::IPv4]);
    if let Err(error) = publisher.register(info) {
        return Some(error.to_string());
    }
    let browser = ServiceDaemon::new().unwrap();
    let events = browser.browse(PROBE).unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    let mut found = false;
    while let Some(left) = deadline.checked_duration_since(std::time::Instant::now()) {
        match events.recv_timeout(left) {
            Ok(ServiceEvent::ServiceResolved(service)) if service.fullname.starts_with(&name) => {
                found = true;
                break;
            }
            Ok(_) => {}
            Err(_) => break,
        }
    }
    let _ = browser.shutdown();
    let _ = publisher.shutdown();
    (!found).then(|| "a published mDNS probe was not resolved within 5 s".to_string())
}

/// The peer listener named `machine` as a fresh browse finds it.
async fn discovered(
    machine: &str,
    timeout: Duration,
) -> Option<cyber_app::peer_discovery::Discovered> {
    let found =
        tokio::task::spawn_blocking(move || cyber_app::peer_discovery::browse(timeout).unwrap())
            .await
            .unwrap();
    found.into_iter().find(|peer| peer.machine == machine)
}

/// `peers.mdns` and `cyber serve --mdns` publish the peer listener only off loopback: a
/// loopback listener, or none, is skipped with a warning. Off loopback the record carries
/// the listener's port and fingerprint, follows a key rotation, and is withdrawn when the
/// server stops (scenario "Discover a desktop", the publishing side).
#[tokio::test(flavor = "multi_thread")]
async fn mdns_publishes_the_peer_listener_only_off_loopback() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let model = scripted_model::ScriptedModel::start(support::prompted_call);
    let served = Served::start(
        &root,
        &model,
        json!({"listen": {"address": "127.0.0.1:0"}, "mdns": true}),
    )
    .await;
    let peer_url = served.registration.peer_url.clone().unwrap();
    let address = peer_url.trim_start_matches("https://").to_string();
    served.stop().await;
    assert!(logged("mDNS not published"));
    assert!(logged(&format!(
        "the peer listener is on loopback {address}"
    )));

    let setup = Setup {
        config: json!({}),
        sandbox_policy: support::BASH_SANDBOX,
        ..Setup::default()
    };
    let served = Served::start_serving(&root, &model, &setup, mdns()).await;
    assert_eq!(served.registration.peer_url, None);
    served.stop().await;
    assert!(logged("no peer listener; set peers.listen in user config"));

    if let Some(reason) = mdns_unavailable() {
        eprintln!("skipping the network half of the mDNS test: {reason}");
        return;
    }
    let machine = format!("desk-{}", ulid::Ulid::new().to_string().to_lowercase());
    let setup = Setup {
        config: json!({"peers": {"listen": {"address": "0.0.0.0:0"}, "machine": machine}}),
        sandbox_policy: support::BASH_SANDBOX,
        ..Setup::default()
    };
    let served = Served::start_serving(&root, &model, &setup, mdns()).await;
    let port: u16 = served
        .registration
        .peer_url
        .as_deref()
        .and_then(|url| url.rsplit(':').next())
        .and_then(|port| port.parse().ok())
        .unwrap();
    let found = discovered(&machine, Duration::from_secs(5))
        .await
        .expect("the peer listener is discovered");
    assert_eq!(found.port, port);
    assert_eq!(
        Some(found.fingerprint.as_str()),
        served.registration.peer_fingerprint.as_deref()
    );
    assert!(!found.addresses.is_empty());
    assert!(
        found
            .addresses
            .iter()
            .all(|ip| ip.is_ipv4() && !ip.is_loopback()),
        "{found:?}"
    );

    let (status, body) = served
        .api(
            "POST",
            "/peers/grants",
            Some(json!({"name": "laptop", "rotate_key": true})),
        )
        .await;
    assert_eq!(status, 201, "{body}");
    let rotated = body["data"]["listener"]["fingerprint"]
        .as_str()
        .unwrap()
        .to_string();
    assert_ne!(rotated, found.fingerprint);
    let found = discovered(&machine, Duration::from_secs(5))
        .await
        .expect("the rotated listener is discovered");
    assert_eq!(found.fingerprint, rotated);
    // The code's default host is the LAN name the record advertises.
    let code = body["data"]["code"].as_str().unwrap();
    let lan = found
        .lan_host
        .clone()
        .expect("a wildcard listener advertises its LAN name");
    assert!(
        code.starts_with(&format!("cyberpeer://{lan}:{port}/")),
        "{code}"
    );
    assert!(found.serves(&format!("{lan}:{port}")));

    served.stop().await;
    assert!(logged("mDNS unpublished"));
    assert_eq!(discovered(&machine, Duration::from_secs(4)).await, None);
}

/// Pending permission requests for `action` of a Session on `served`.
async fn requests(served: &Served, session: &str, action: &str) -> Vec<Value> {
    let (status, body) = served
        .api(
            "GET",
            &format!("/permissions/requests?session_id={session}"),
            None,
        )
        .await;
    assert_eq!(status, 200, "{body}");
    body["data"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|request| request["action"] == action)
        .cloned()
        .collect()
}

/// Answer the one pending `action` request of `session` on `served` with `reply`.
async fn answer(served: &Served, session: &str, action: &str, reply: &str) -> Value {
    let request = eventually(action, || async {
        requests(served, session, action).await.into_iter().next()
    })
    .await;
    let id = request["id"].as_str().unwrap();
    let (status, body) = served
        .api(
            "POST",
            &format!("/sessions/{session}/permissions/{id}/reply"),
            Some(json!({ "reply": reply })),
        )
        .await;
    assert_eq!(status, 204, "{body}");
    request
}

/// The transcript of a Session on `served`, as text.
async fn transcript(served: &Served, session: &str) -> String {
    let (status, body) = served
        .api(
            "GET",
            &format!("/sessions/{session}/messages?order=asc&limit=200"),
            None,
        )
        .await;
    assert_eq!(status, 200, "{body}");
    body.to_string()
}

/// Wait until the Session's `echo <output>` has run and its Turn has ended. A timeout
/// shows the transcript, so a platform that records the output differently is diagnosable.
async fn command_finished(served: &Served, session: &str, output: &str) {
    let needle = format!(r#""output":"{output}"#);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let text = transcript(served, session).await;
        if text.contains(&needle) && !served.runtime.is_running(session) {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for `{output}` (running: {}): {text}",
            served.runtime.is_running(session)
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// Requirement "Peer attach" on the production server `desk`, through its TLS peer
/// listener and the pinned transport `cyber attach migration@desk` opens the TUI with
/// (`cyber_client::Client::attach_peer`):
/// - "Message grant cannot attach": a `message` grant gets `PeerScopeError` and nothing is
///   asked on desk;
/// - "Attach without approval": nobody answers `remote.attach` (resource `peer:laptop`)
///   within the wait (shortened from 2 minutes), the attach fails with
///   `PermissionDeniedError`, the request is withdrawn and no Session data was sent; a
///   refusal fails the same way and neither halts the Session nor declines its other
///   requests;
/// - "Attach approved" / "Attach to a peer Session": after desk approves, the client shows
///   the Session and its transcript, follows its events, prompts it, answers its
///   permission request and interrupts it, and reaches nothing else; an `allow` rule
///   skips the prompt, and a revoked grant ends the attach at its next request.
#[tokio::test]
async fn attach_through_a_peer_needs_scope_and_approval() {
    use cyber_client::Client;
    use cyber_server::runtime::messaging::{PeerScope, PeerTarget};
    use cyber_server::runtime::peers::{NewGrant, grants};
    use futures::StreamExt;
    let desk_temp = tempfile::tempdir().unwrap();
    let model = scripted_model::ScriptedModel::start(support::prompted_call);
    let setup = Setup {
        config: json!({"peers": {"listen": {"address": "127.0.0.1:0"}, "machine": "desk"}}),
        interactive: true,
        sandbox_policy: support::BASH_SANDBOX,
        ..Setup::default()
    };
    let wait = Duration::from_secs(3);
    let options = ServeOptions {
        peer_attach_wait: wait,
        ..ServeOptions::default()
    };
    let root = desk_temp.path().canonicalize().unwrap();
    let desk = Served::start_serving(&root, &model, &setup, options).await;
    let peer = desk.registration.peer_url.clone().unwrap();
    assert!(peer.starts_with("https://"), "{peer}");
    let fingerprint = desk.registration.peer_fingerprint.clone().unwrap();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;
    let grant = |name: &str, scopes: Vec<PeerScope>| {
        let grant = NewGrant {
            name: name.into(),
            scopes,
            expires_ms: None,
        };
        grants::create(&desk.store, grant, now).unwrap().token
    };
    let phone = grant("phone", vec![]);
    let laptop = grant("laptop", vec![PeerScope::Message, PeerScope::Attach]);
    let target = |token: &str| PeerTarget {
        name: "desk".into(),
        url: peer.clone(),
        fingerprint: fingerprint.clone(),
        token: token.into(),
    };
    let migration = desk.session("migration").await;
    let other = desk.session("other").await;
    let (status, body) = desk
        .api(
            "POST",
            &format!("/sessions/{migration}/prompt"),
            Some(json!({"parts": [{"type": "text", "text": "schema notes"}]})),
        )
        .await;
    assert_eq!(status, 202, "{body}");
    eventually("the first turn settles", || async {
        let text = transcript(&desk, &migration).await;
        (!desk.runtime.is_running(&migration) && text.contains("\"ok\"")).then_some(())
    })
    .await;

    // Message grant cannot attach; nothing is asked on desk.
    let error = Client::attach_peer(target(&phone), "migration")
        .await
        .err()
        .expect("a message grant cannot attach");
    assert_eq!(error.tag(), Some("PeerAuthError"), "{error}");
    let cyber_client::ClientError::Api { body, .. } = &error else {
        panic!("{error}")
    };
    assert_eq!(body["reason"], "scope", "{body}");
    let attach_url = format!("{peer}/api/v1/peer/attach/{migration}");
    let (status, body) = peer_call("POST", &attach_url, &phone, Some(json!({}))).await;
    assert_eq!((status, &body["_tag"]), (403, &json!("PeerScopeError")));
    assert!(
        requests(&desk, &migration, "remote.attach")
            .await
            .is_empty()
    );

    // Attach without approval: the request waits, then fails and is withdrawn.
    let started = std::time::Instant::now();
    let waiting = tokio::spawn(Client::attach_peer(target(&laptop), "migration"));
    let request = eventually("the attach asks", || async {
        requests(&desk, &migration, "remote.attach")
            .await
            .into_iter()
            .next()
    })
    .await;
    assert_eq!(request["resources"], json!(["peer:laptop"]), "{request}");
    assert_eq!(request["session_id"], migration.as_str(), "{request}");
    // No Session data without an approved attach, even with the attach grant.
    for route in ["session", "messages", "events", "permissions"] {
        let url = format!("{attach_url}/{route}");
        let (status, body) = peer_call("GET", &url, &laptop, None).await;
        assert_eq!(
            (status, &body["_tag"]),
            (403, &json!("PermissionDeniedError")),
            "{route}: {body}"
        );
        assert!(!body.to_string().contains("schema notes"), "{body}");
    }
    let error = waiting.await.unwrap().err().expect("unanswered");
    assert_eq!(error.tag(), Some("PermissionDeniedError"), "{error}");
    assert!(started.elapsed() >= wait, "{:?}", started.elapsed());
    eventually("the request is withdrawn", || async {
        let left = requests(&desk, &migration, "remote.attach").await;
        left.is_empty().then_some(())
    })
    .await;

    // A peer that hangs up withdraws its request before the wait ends.
    let gone = tokio::spawn(Client::attach_peer(target(&laptop), "migration"));
    eventually("the attach asks", || async {
        let asked = requests(&desk, &migration, "remote.attach").await;
        (!asked.is_empty()).then_some(())
    })
    .await;
    let hung_up = std::time::Instant::now();
    gone.abort();
    eventually("the abandoned request is withdrawn", || async {
        let left = requests(&desk, &migration, "remote.attach").await;
        left.is_empty().then_some(())
    })
    .await;
    assert!(hung_up.elapsed() < wait, "{:?}", hung_up.elapsed());

    // A refusal fails the attach and leaves the Session's own work and requests alone.
    let (status, body) = desk
        .api(
            "POST",
            &format!("/sessions/{migration}/prompt"),
            Some(json!({"parts": [{"type": "text", "text": "RUN<<echo still-running>>"}]})),
        )
        .await;
    assert_eq!(status, 202, "{body}");
    eventually("bash asks on desk", || async {
        requests(&desk, &migration, "bash").await.into_iter().next()
    })
    .await;
    let refused = tokio::spawn(Client::attach_peer(target(&laptop), "migration"));
    answer(&desk, &migration, "remote.attach", "reject").await;
    let error = refused.await.unwrap().err().expect("refused");
    assert_eq!(error.tag(), Some("PermissionDeniedError"), "{error}");
    answer(&desk, &migration, "bash", "once").await;
    command_finished(&desk, &migration, "still-running").await;

    // Attach approved: the Session, its transcript, its events, prompt and replies.
    let approved = tokio::spawn(Client::attach_peer(target(&laptop), "migration"));
    answer(&desk, &migration, "remote.attach", "once").await;
    let client = approved.await.unwrap().expect("approved");
    assert_eq!(client.attached_session(), Some(migration.as_str()));
    let shown = client.get(&format!("/sessions/{migration}")).await.unwrap();
    assert_eq!(shown["data"]["title"], "migration", "{shown}");
    let page = client
        .get(&format!(
            "/sessions/{migration}/messages?order=asc&limit=200"
        ))
        .await
        .unwrap();
    assert!(page.to_string().contains("schema notes"), "{page}");
    let mut replay = client.session_events(&migration, -1).boxed();
    let first = tokio::time::timeout(Duration::from_secs(10), replay.next())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        (first.kind.as_str(), first.seq),
        ("session.created.1", Some(0)),
        "{first:?}"
    );
    drop(replay);
    let mut live = client.events(true).await.unwrap().boxed();
    let prompted = client
        .post(
            &format!("/sessions/{migration}/prompt"),
            json!({"parts": [{"type": "text", "text": "RUN<<echo from-laptop>>"}]}),
        )
        .await
        .unwrap();
    assert!(prompted["data"].is_object(), "{prompted}");
    let asked = eventually("bash asks the attached client", || async {
        let pending = client
            .get(&format!("/permissions/requests?session_id={migration}"))
            .await
            .ok()?;
        pending["data"].as_array()?.first().cloned()
    })
    .await;
    assert_eq!(asked["action"], "bash", "{asked}");
    let reply = format!(
        "/sessions/{migration}/permissions/{}/reply",
        asked["id"].as_str().unwrap()
    );
    client.post(&reply, json!({"reply": "once"})).await.unwrap();
    eventually("the attached prompt runs", || async {
        let page = client
            .get(&format!(
                "/sessions/{migration}/messages?order=asc&limit=200"
            ))
            .await
            .ok()?;
        let ran = page.to_string().contains(r#""output":"from-laptop"#);
        (ran && !desk.runtime.is_running(&migration)).then_some(())
    })
    .await;
    // Regression: the attached TUI's stream carried only durable events, so its run never
    // ended there (no `session.idle`) and it showed "Thinking" for good.
    let (seen, kinds) = tokio::time::timeout(Duration::from_secs(10), async {
        let (mut durable, mut kinds) = (None, Vec::new());
        while let Some(event) = live.next().await {
            kinds.push(event.kind.clone());
            let ours = event.session_id.as_deref() == Some(migration.as_str());
            if ours && event.seq.is_some() {
                durable.get_or_insert(event.clone());
            }
            if ours && event.kind == "session.idle" {
                return (durable.expect("a durable event before idle"), kinds);
            }
        }
        panic!("the event stream ended: {kinds:?}")
    })
    .await
    .expect("the attached stream follows the Session to idle");
    assert!(seen.seq.unwrap() > 0, "{seen:?}");
    assert_eq!(kinds[0], "server.connected", "{kinds:?}");

    // Interrupt through the attach.
    client
        .post(
            &format!("/sessions/{migration}/prompt"),
            json!({"parts": [{"type": "text", "text": "GATE<<attached>>"}]}),
        )
        .await
        .unwrap();
    eventually("the gated prompt runs", || async {
        desk.runtime.is_running(&migration).then_some(())
    })
    .await;
    client
        .post(&format!("/sessions/{migration}/interrupt"), json!({}))
        .await
        .unwrap();
    eventually("the interrupt stops it", || async {
        (!desk.runtime.is_running(&migration)).then_some(())
    })
    .await;
    model.open_gate();

    // Only that Session's routes: the others never leave this machine.
    for (method, path) in [
        ("GET", format!("/sessions/{other}")),
        ("GET", "/sessions?limit=100".to_string()),
        ("POST", format!("/sessions/{migration}/shell")),
        ("POST", format!("/sessions/{migration}/mode")),
        ("GET", format!("/sessions/{migration}/inbox")),
        ("GET", "/config".to_string()),
    ] {
        let error = client
            .request(method, &path, Some(json!({})))
            .await
            .unwrap_err();
        assert!(error.route_missing(), "{method} {path}: {error}");
    }
    // Prompts are text and images from the user: no tool calls, no other source.
    for parts in [
        json!({"parts": [{"type": "tool_call", "id": "c", "name": "bash", "input": {}}]}),
        json!({"parts": [{"type": "text", "text": "x"}], "source": "workflow"}),
    ] {
        let path = format!("/sessions/{migration}/prompt");
        let error = client.post(&path, parts.clone()).await.unwrap_err();
        assert_eq!(error.tag(), Some("InvalidRequestError"), "{parts}: {error}");
    }

    // Closing ends the attach.
    client.detach().await.unwrap();
    let closed = client
        .get(&format!("/sessions/{migration}"))
        .await
        .unwrap_err();
    assert_eq!(closed.tag(), Some("PermissionDeniedError"), "{closed}");

    // An allow rule skips the prompt; an attach ID covers only its Session.
    desk.edit_config(|config| {
        config["permissions"] = json!({"remote.attach": {"peer:laptop": "allow"}});
    });
    let (status, opened) = peer_call("POST", &attach_url, &laptop, Some(json!({}))).await;
    assert_eq!(status, 200, "{opened}");
    assert!(
        requests(&desk, &migration, "remote.attach")
            .await
            .is_empty()
    );
    let attach_id = opened["attach_id"].as_str().unwrap();
    let with_id = |session: &str| {
        peer_client()
            .get(format!("{peer}/api/v1/peer/attach/{session}/session"))
            .bearer_auth(&laptop)
            .header("x-cyber-attach", attach_id)
            .send()
    };
    assert_eq!(with_id(&migration).await.unwrap().status().as_u16(), 200);
    assert_eq!(with_id(&other).await.unwrap().status().as_u16(), 403);
    let allowed = tokio::time::timeout(
        Duration::from_secs(10),
        Client::attach_peer(target(&laptop), &migration),
    )
    .await
    .expect("no prompt")
    .unwrap();
    allowed
        .get(&format!("/sessions/{migration}"))
        .await
        .unwrap();

    // A revoked grant ends the attach at its next request.
    assert!(grants::revoke(&desk.store, "laptop", now + 1).unwrap());
    let revoked = allowed
        .get(&format!("/sessions/{migration}"))
        .await
        .unwrap_err();
    assert_eq!(revoked.tag(), Some("PeerAuthError"), "{revoked}");
    desk.stop().await;
}

/// A production server `desk` with an interactive client, a TLS peer listener and an
/// attach wait of `wait`, with `config` merged into its user config.
async fn attach_desk(
    root: &Path,
    model: &scripted_model::ScriptedModel,
    config: Value,
    wait: Duration,
) -> Served {
    let mut merged = json!({"peers": {"listen": {"address": "127.0.0.1:0"}, "machine": "desk"}});
    for (key, value) in config.as_object().into_iter().flatten() {
        merged[key] = value.clone();
    }
    let setup = Setup {
        config: merged,
        interactive: true,
        sandbox_policy: support::BASH_SANDBOX,
        ..Setup::default()
    };
    let options = ServeOptions {
        peer_attach_wait: wait,
        ..ServeOptions::default()
    };
    Served::start_serving(root, model, &setup, options).await
}

/// A grant on `served` named `name` with `scopes`; its token.
fn grant_token(
    served: &Served,
    name: &str,
    scopes: Vec<cyber_server::runtime::messaging::PeerScope>,
) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;
    let grant = cyber_server::runtime::peers::NewGrant {
        name: name.into(),
        scopes,
        expires_ms: None,
    };
    cyber_server::runtime::peers::grants::create(&served.store, grant, now)
        .unwrap()
        .token
}

/// `POST /peer/attach/:session` with `token`, answered without a prompt; the attach ID.
async fn open_attach(peer: &str, session: &str, token: &str) -> String {
    let url = format!("{peer}/api/v1/peer/attach/{session}");
    let (status, body) = peer_call("POST", &url, token, Some(json!({}))).await;
    assert_eq!(status, 200, "{body}");
    body["attach_id"].as_str().unwrap().to_string()
}

/// An open `GET /peer/attach/:session/events` stream: its chunks, and `None` once it ends.
async fn attach_stream(
    peer: &str,
    session: &str,
    token: &str,
    attach: &str,
) -> tokio::sync::mpsc::UnboundedReceiver<Option<String>> {
    // No overall timeout: the stream stays open until the server ends it.
    let client = reqwest::Client::builder()
        .no_proxy()
        .tls_danger_accept_invalid_certs(true)
        .build()
        .unwrap();
    let mut response = client
        .get(format!(
            "{peer}/api/v1/peer/attach/{session}/events?after=-1"
        ))
        .bearer_auth(token)
        .header("x-cyber-attach", attach)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 200);
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    tokio::spawn(async move {
        while let Ok(Some(chunk)) = response.chunk().await {
            if tx
                .send(Some(String::from_utf8_lossy(&chunk).into_owned()))
                .is_err()
            {
                return;
            }
        }
        let _ = tx.send(None);
    });
    rx
}

/// Wait (bounded) until `stream` has sent `marker`; the text it sent so far.
async fn stream_until(
    stream: &mut tokio::sync::mpsc::UnboundedReceiver<Option<String>>,
    marker: &str,
) -> String {
    let mut seen = String::new();
    tokio::time::timeout(Duration::from_secs(10), async {
        while !seen.contains(marker) {
            match stream.recv().await {
                Some(Some(chunk)) => seen.push_str(&chunk),
                _ => break,
            }
        }
    })
    .await
    .unwrap_or_else(|_| panic!("the attach stream never sent {marker}: {seen}"));
    assert!(
        seen.contains(marker),
        "the attach stream ended before {marker}: {seen}"
    );
    seen
}

/// Wait (bounded) until `stream` ends; the text it sent before.
async fn stream_ends(stream: &mut tokio::sync::mpsc::UnboundedReceiver<Option<String>>) -> String {
    let mut seen = String::new();
    tokio::time::timeout(Duration::from_secs(10), async {
        while let Some(Some(chunk)) = stream.recv().await {
            seen.push_str(&chunk);
        }
    })
    .await
    .unwrap_or_else(|_| panic!("the attach stream stayed open: {seen}"));
    seen
}

/// Regression (requirement "Peer attach", revocation effective immediately): an open
/// `GET /peer/attach/:id/events` stream was admitted once and then followed the Session
/// for as long as the peer kept it open, past `DELETE /attach/:id`, past
/// `cyber peers revoke` (`DELETE /api/v1/peers/grants/:name`) and past any other
/// revocation. On the registered production server's TLS peer listener each of these now
/// ends the stream, and later Session events no longer reach the peer.
#[tokio::test]
async fn an_open_attach_stream_ends_with_its_attach_and_grant() {
    use cyber_server::runtime::messaging::PeerScope;
    let temp = tempfile::tempdir().unwrap();
    let model = scripted_model::ScriptedModel::start(support::prompted_call);
    let root = temp.path().canonicalize().unwrap();
    let allow = json!({"permissions": {"remote.attach": {"peer:laptop": "allow"}}});
    let desk = attach_desk(&root, &model, allow, Duration::from_secs(3)).await;
    let peer = desk.registration.peer_url.clone().unwrap();
    let migration = desk.session("migration").await;
    let scopes = || vec![PeerScope::Message, PeerScope::Attach];

    // Closing the attach ends its stream.
    let laptop = grant_token(&desk, "laptop", scopes());
    let attach = open_attach(&peer, &migration, &laptop).await;
    let mut stream = attach_stream(&peer, &migration, &laptop, &attach).await;
    // The stream has replayed the Session before it is closed, so the replay is not
    // racing the close on a slow runner.
    let mut seen = stream_until(&mut stream, "session.created").await;
    let close = peer_client()
        .delete(format!("{peer}/api/v1/peer/attach/{migration}"))
        .bearer_auth(&laptop)
        .header("x-cyber-attach", &attach)
        .send()
        .await
        .unwrap();
    assert_eq!(close.status().as_u16(), 204);
    seen.push_str(&stream_ends(&mut stream).await);
    assert!(seen.contains("session.created"), "{seen}");

    // `cyber peers revoke laptop` ends it at once; later events never reach the peer.
    let attach = open_attach(&peer, &migration, &laptop).await;
    let mut stream = attach_stream(&peer, &migration, &laptop, &attach).await;
    let (status, body) = desk.api("DELETE", "/peers/grants/laptop", None).await;
    assert_eq!(
        (status, &body["data"]["revoked"]),
        (200, &json!(true)),
        "{body}"
    );
    stream_ends(&mut stream).await;
    let (status, body) = desk
        .api(
            "PATCH",
            &format!("/sessions/{migration}"),
            Some(json!({"title": "after-revoke"})),
        )
        .await;
    assert_eq!(status, 200, "{body}");
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(stream.recv().await, None, "nothing after the end");

    // A revocation that bypasses the listener (here the store itself) ends it too.
    let laptop = grant_token(&desk, "laptop", scopes());
    let attach = open_attach(&peer, &migration, &laptop).await;
    let mut stream = attach_stream(&peer, &migration, &laptop, &attach).await;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;
    assert!(cyber_server::runtime::peers::grants::revoke(&desk.store, "laptop", now).unwrap());
    stream_ends(&mut stream).await;
    desk.stop().await;
}

/// Regression (requirement "Peer watches and listing"): the status and attach routes
/// searched only the 50 newest Sessions the grant is listed, so an older Session a send
/// still reached was "not found": `watch_session` failed, an existing watch fired a false
/// `exited`, and `cyber attach old@desk` failed. Through the production peer listener the
/// oldest of 52 Sessions now has a status and can be attached, while the listing keeps
/// its cap.
#[tokio::test]
async fn sessions_beyond_the_listing_have_a_status_and_attach() {
    use cyber_server::runtime::messaging::PeerScope;
    let temp = tempfile::tempdir().unwrap();
    let model = scripted_model::ScriptedModel::start(support::prompted_call);
    let root = temp.path().canonicalize().unwrap();
    let allow = json!({"permissions": {"remote.attach": {"peer:laptop": "allow"}}});
    let desk = attach_desk(&root, &model, allow, Duration::from_secs(3)).await;
    let peer = desk.registration.peer_url.clone().unwrap();
    let oldest = desk.session("oldest").await;
    for i in 0..51 {
        desk.session(&format!("newer-{i}")).await;
    }
    let laptop = grant_token(&desk, "laptop", vec![PeerScope::Message, PeerScope::Attach]);
    let (status, listed) = peer_call(
        "GET",
        &format!("{peer}/api/v1/peer/sessions?limit=100"),
        &laptop,
        None,
    )
    .await;
    assert_eq!(status, 200, "{listed}");
    let sessions = listed["sessions"].as_array().unwrap();
    assert_eq!(sessions.len(), 50);
    assert!(sessions.iter().all(|s| s["session_id"] != oldest.as_str()));

    for id in [oldest.as_str(), "oldest"] {
        let url = format!("{peer}/api/v1/peer/sessions/{id}/status");
        let (status, body) = peer_call("GET", &url, &laptop, None).await;
        assert_eq!(status, 200, "{id}: {body}");
        assert_eq!(
            (&body["session_id"], &body["state"]),
            (&json!(oldest), &json!("idle"))
        );
    }
    let attach = open_attach(&peer, "oldest", &laptop).await;
    let (status, body, _) = peer_get(
        &format!("{peer}/api/v1/peer/attach/{oldest}/session"),
        &[
            ("authorization", &format!("Bearer {laptop}")),
            ("x-cyber-attach", &attach),
        ],
    )
    .await;
    assert_eq!(status, 200, "{body}");

    // Still hidden: an unknown Session, and one whose inbound refuses the grant.
    let (status, body) = desk
        .api(
            "PATCH",
            &format!("/sessions/{oldest}"),
            Some(json!({"messaging": {"inbound": "refuse"}})),
        )
        .await;
    assert_eq!(status, 200, "{body}");
    for id in [oldest.as_str(), "ses_unknown"] {
        let url = format!("{peer}/api/v1/peer/sessions/{id}/status");
        let (status, body) = peer_call("GET", &url, &laptop, None).await;
        assert_eq!(status, 404, "{id}: {body}");
    }
    desk.stop().await;
}

/// Regression (requirement "Peer attach"): `remote.attach` counted any rule evaluation
/// that came out `allow`, so a blanket `"permissions": "allow"` or a repository rule
/// let an attach grant take over a Session with no prompt. On the production server both
/// still raise `remote.attach`, and with nobody answering the attach fails with
/// `PermissionDeniedError`.
#[tokio::test]
async fn only_an_explicit_user_rule_skips_the_attach_prompt() {
    use cyber_server::runtime::messaging::PeerScope;
    let temp = tempfile::tempdir().unwrap();
    let model = scripted_model::ScriptedModel::start(support::prompted_call);
    let root = temp.path().canonicalize().unwrap();
    let blanket = json!({"permissions": "allow"});
    let desk = attach_desk(&root, &model, blanket, Duration::from_secs(2)).await;
    let peer = desk.registration.peer_url.clone().unwrap();
    let migration = desk.session("migration").await;
    let laptop = grant_token(&desk, "laptop", vec![PeerScope::Attach]);
    let url = format!("{peer}/api/v1/peer/attach/{migration}");
    let refused = |what: &'static str| {
        let (url, laptop, desk, migration) = (&url, &laptop, &desk, &migration);
        async move {
            let attach = peer_call("POST", url, laptop, Some(json!({})));
            let asked = async {
                eventually(what, || async {
                    let asked = requests(desk, migration, "remote.attach").await;
                    (!asked.is_empty()).then_some(())
                })
                .await;
            };
            let ((status, body), ()) = tokio::join!(attach, asked);
            assert_eq!(
                (status, &body["_tag"]),
                (403, &json!("PermissionDeniedError")),
                "{what}: {body}"
            );
        }
    };
    refused("a blanket allow still asks").await;

    // A repository rule for `remote.attach` itself, in a trusted checkout, asks too.
    desk.edit_config(|config| {
        config.as_object_mut().unwrap().remove("permissions");
    });
    let repo = root.join("repo");
    let project = json!({"permissions": {"remote.attach": "allow"}});
    std::fs::write(repo.join("cyber.jsonc"), project.to_string()).unwrap();
    let loader = cyber_app::hook_config_loader(desk.paths(), root.join("home"));
    let resolved = loader(&repo).unwrap();
    cyber_core::trust::TrustStore::new(desk.paths().trust_file())
        .approve(
            &resolved.trust.checkout_root,
            resolved.trust.digest.as_deref().unwrap(),
        )
        .unwrap();
    let resolved = loader(&repo).unwrap();
    assert!(resolved.trust.trusted);
    assert_eq!(
        resolved.value["permissions"]["remote.attach"], "allow",
        "{:?}",
        resolved.sources
    );
    refused("a project allow still asks").await;
    desk.stop().await;
}

/// Regression: the server stopped its listeners with `Notify::notify_waiters`, which wakes
/// only tasks already waiting, so a stop that arrived as the server registered (before
/// its main, peer and socket listener tasks first ran) left them serving and the server
/// never returned. Stopped before it even starts, the production server now stops.
#[tokio::test]
async fn a_server_stopped_as_it_starts_still_stops() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let model = scripted_model::ScriptedModel::start(support::prompted_call);
    let repo = root.join("repo");
    std::fs::create_dir_all(repo.join(".git")).unwrap();
    let setup = Setup {
        config: json!({"peers": {"listen": {"address": "127.0.0.1:0"}, "machine": "desk"}}),
        sandbox_policy: support::BASH_SANDBOX,
        ..Setup::default()
    };
    let app = ModeApp::build(&root, &repo, &model, &setup).await;
    let stop = Arc::new(Notify::new());
    stop.notify_one();
    let options = ServeOptions {
        port: Some(0),
        socket: cfg!(unix).then(|| root.join("s.sock")),
        register: true,
        ..ServeOptions::default()
    };
    let served = cyber_app::run_server_until(app, options, |_| {}, stop);
    tokio::time::timeout(Duration::from_secs(20), served)
        .await
        .expect("the server stops")
        .unwrap();
}

/// `POST /peer/messages` to `desk` as grant `token`: from `ses_01JBUILD`, named `build`.
async fn post_from_build(peer: &str, token: &str, id: &str) -> (u16, Value) {
    let body = json!({"id": id, "to": "migration", "text": "from build", "hops": 1,
        "from_session": "ses_01JBUILD", "from_name": "build"});
    peer_call(
        "POST",
        &format!("{peer}/api/v1/peer/messages"),
        token,
        Some(body),
    )
    .await
}

/// Regression (requirement "Inbound controls", peer senders default to `hold`): `*` also
/// matches `@`, and a peer chooses its own Session labels, so `deliver` patterns written
/// for local Sessions (`*`, `ses_*`, `build*`) and a Session override `deliver` let a
/// peer's message start a Turn with no release. Through the production peer listener,
/// each of them still holds it; only a pattern naming the peer (`*@laptop`) delivers.
#[tokio::test]
async fn local_deliver_patterns_still_hold_peer_messages() {
    let temp = tempfile::tempdir().unwrap();
    let model = scripted_model::ScriptedModel::start(support::prompted_call);
    let desk = Served::start_with(
        &temp.path().canonicalize().unwrap(),
        &model,
        json!({
            "peers": {"listen": {"address": "127.0.0.1:0"}, "machine": "desk"},
            "messaging": {"inbound": {"deliver": ["*"]}},
        }),
    )
    .await;
    let peer = desk.registration.peer_url.clone().unwrap();
    let token = grant_token(&desk, "laptop", vec![]);
    let migration = desk.session("migration").await;
    for (i, pattern) in ["*", "ses_*", "build*"].into_iter().enumerate() {
        desk.edit_config(|config| config["messaging"]["inbound"]["deliver"] = json!([pattern]));
        let (status, body) = post_from_build(&peer, &token, &format!("msgx_local{i}")).await;
        assert_eq!(status, 200, "{pattern}: {body}");
        assert_eq!(body["outcome"], "held", "{pattern}: {body}");
    }
    desk.edit_config(|config| config["messaging"]["inbound"]["deliver"] = json!([]));
    let (status, body) = desk
        .api(
            "PATCH",
            &format!("/sessions/{migration}"),
            Some(json!({"messaging": {"inbound": "deliver"}})),
        )
        .await;
    assert_eq!(status, 200, "{body}");
    let (_, body) = post_from_build(&peer, &token, "msgx_override").await;
    assert_eq!(body["outcome"], "held", "{body}");
    for id in ["msgx_local0", "msgx_local1", "msgx_local2", "msgx_override"] {
        let record = desk.record(&migration, id).await.unwrap();
        assert_eq!(record["outcome"], "held", "{record}");
    }

    desk.edit_config(|config| {
        config["messaging"]["inbound"]["deliver"] = json!(["*@laptop"]);
    });
    let (_, body) = post_from_build(&peer, &token, "msgx_named").await;
    assert_eq!(body["outcome"], "delivered", "{body}");
    desk.stop().await;
}

/// Regression: the peer client turned every `5xx` into `PeerUnreachableError`, so a send
/// to a peer whose messaging is disabled (`503 ServiceUnavailableError`) said the machine
/// was offline and recorded outcome `failed`, reason `unreachable`. Between two
/// production servers it is now `ServiceUnavailableError` and the sender records nothing.
#[tokio::test]
async fn a_peer_with_messaging_disabled_says_so() {
    let (desk_temp, laptop_temp) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let model = scripted_model::ScriptedModel::start(support::prompted_call);
    let desk = Served::start_with(
        &desk_temp.path().canonicalize().unwrap(),
        &model,
        json!({
            "peers": {"listen": {"address": "127.0.0.1:0"}, "machine": "desk"},
            "messaging": {"enabled": false},
        }),
    )
    .await;
    let peer = desk.registration.peer_url.clone().unwrap();
    let fingerprint = desk.registration.peer_fingerprint.clone().unwrap();
    let token = grant_token(&desk, "laptop", vec![]);
    let remote = json!({"name": "desk", "url": peer, "fingerprint": fingerprint, "token": token});
    let laptop = Served::start_with(
        &laptop_temp.path().canonicalize().unwrap(),
        &model,
        json!({
            "peers": {"machine": "laptop", "remotes": [remote]},
            "permissions": {"message.send": {"*@desk": "allow"}},
        }),
    )
    .await;
    desk.session("migration").await;
    let review = laptop.session("review").await;
    let (status, body) = laptop
        .api(
            "POST",
            "/messaging/send",
            Some(
                json!({"id": "msgx_off", "from": review, "to": "migration@desk",
                "text": "hi"}),
            ),
        )
        .await;
    assert_eq!(status, 503, "{body}");
    assert_eq!(body["_tag"], "ServiceUnavailableError", "{body}");
    assert!(laptop.history(&review).await.is_empty());
    laptop.stop().await;
    desk.stop().await;
}

/// A request on an approved attach of `session`.
async fn attached(
    peer: &str,
    session: &str,
    token: &str,
    attach: &str,
    route: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let url = format!("{peer}/api/v1/peer/attach/{session}/{route}");
    let mut request = match &body {
        Some(_) => peer_client().post(url),
        None => peer_client().get(url),
    }
    .bearer_auth(token)
    .header("x-cyber-attach", attach);
    if let Some(body) = body {
        request = request.json(&body);
    }
    let response = request.send().await.unwrap();
    let status = response.status().as_u16();
    (status, response.json().await.unwrap_or(Value::Null))
}

/// Regression (requirement "Peer attach": the attached client acts with the Session's
/// own permissions and cannot change config): an attached peer's `always` reached the
/// local reply route, which saves a project-wide approval that outlives the attach and
/// its grant. On the production peer listener `always` is refused, the request stays
/// pending, and after a `once` the same command asks again.
#[tokio::test]
async fn an_attached_peer_cannot_approve_always() {
    use cyber_server::runtime::messaging::PeerScope;
    let temp = tempfile::tempdir().unwrap();
    let model = scripted_model::ScriptedModel::start(support::prompted_call);
    let root = temp.path().canonicalize().unwrap();
    let allow = json!({"permissions": {"remote.attach": {"peer:laptop": "allow"}}});
    let desk = attach_desk(&root, &model, allow, Duration::from_secs(3)).await;
    let peer = desk.registration.peer_url.clone().unwrap();
    let migration = desk.session("migration").await;
    let laptop = grant_token(&desk, "laptop", vec![PeerScope::Message, PeerScope::Attach]);
    let attach = open_attach(&peer, &migration, &laptop).await;
    let call = |route: &'static str, body: Option<Value>| {
        attached(&peer, &migration, &laptop, &attach, route, body)
    };
    let run = json!({"parts": [{"type": "text", "text": "RUN<<echo always-asked>>"}]});
    let (status, body) = call("prompt", Some(run.clone())).await;
    assert!((200..300).contains(&status), "{status} {body}");
    let asked = eventually("bash asks the attached peer", || async {
        let (_, pending) = call("permissions", None).await;
        pending["data"].as_array()?.first().cloned()
    })
    .await;
    let reply = |id: &str| format!("permissions/{id}/reply");
    let id = asked["id"].as_str().unwrap().to_string();
    let url = format!("{peer}/api/v1/peer/attach/{migration}/{}", reply(&id));
    let always = peer_client()
        .post(&url)
        .bearer_auth(&laptop)
        .header("x-cyber-attach", &attach)
        .json(&json!({"reply": "always"}))
        .send()
        .await
        .unwrap();
    assert_eq!(always.status().as_u16(), 400);
    let body: Value = always.json().await.unwrap();
    assert_eq!(body["_tag"], "InvalidRequestError", "{body}");
    assert_eq!(requests(&desk, &migration, "bash").await.len(), 1);
    let once = peer_client()
        .post(&url)
        .bearer_auth(&laptop)
        .header("x-cyber-attach", &attach)
        .json(&json!({"reply": "once"}))
        .send()
        .await
        .unwrap();
    assert_eq!(once.status().as_u16(), 204);
    command_finished(&desk, &migration, "always-asked").await;
    // Nothing was saved: the same command asks again.
    let (status, body) = call("prompt", Some(run)).await;
    assert!((200..300).contains(&status), "{status} {body}");
    eventually("bash asks again", || async {
        let asked = requests(&desk, &migration, "bash").await;
        (!asked.is_empty()).then_some(())
    })
    .await;
    answer(&desk, &migration, "bash", "reject").await;
    desk.stop().await;
}

/// Regression (requirement "Peer outcome tracking", scenario "Release seen by the
/// sender"): the tracker polled every due message one after another, a new client each,
/// so a peer that accepts connections and never answers held up a release on a
/// reachable peer for a timeout per message of the silent one. Between two production
/// servers, with three messages held through a remote that then goes silent and one
/// released through another, the release is recorded within a few seconds of the pass,
/// while the silent remote's messages are rescheduled after a single timeout.
#[tokio::test]
async fn one_silent_peer_does_not_delay_a_release_on_another() {
    let (desk_temp, laptop_temp) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let model = scripted_model::ScriptedModel::start(support::prompted_call);
    let desk = Served::start_with(
        &desk_temp.path().canonicalize().unwrap(),
        &model,
        json!({"peers": {"listen": {"address": "127.0.0.1:0"}, "machine": "desk"}}),
    )
    .await;
    let peer = desk.registration.peer_url.clone().unwrap();
    let fingerprint = desk.registration.peer_fingerprint.clone().unwrap();
    let remote = |name: &str, url: &str, grant: &str| {
        let token = grant_token(&desk, grant, vec![]);
        json!({"name": name, "url": url, "fingerprint": fingerprint, "token": token})
    };
    let remotes = json!([
        remote("nas", &peer, "laptop-b"),
        remote("desk", &peer, "laptop")
    ]);
    let laptop = Served::start_with(
        &laptop_temp.path().canonicalize().unwrap(),
        &model,
        json!({
            "peers": {"machine": "laptop", "remotes": remotes},
            "permissions": {"message.send": {"*@desk": "allow", "*@nas": "allow"}},
        }),
    )
    .await;
    let migration = desk.session("migration").await;
    let review = laptop.session("review").await;
    let send = |id: String, to: &'static str| {
        let laptop = &laptop;
        let review = review.clone();
        async move {
            let body = json!({"id": id, "from": review, "to": to, "text": "held"});
            let (status, body) = laptop.api("POST", "/messaging/send", Some(body)).await;
            assert_eq!(
                (status, &body["data"]["outcome"]),
                (200, &json!("held")),
                "{body}"
            );
        }
    };
    for i in 0..3 {
        send(format!("msgx_silent{i}"), "migration@nas").await;
    }
    send("msgx_seen".into(), "migration@desk").await;
    let last = poll_times(&laptop, "msgx_seen").0;

    // `nas` now accepts connections and never answers.
    let silent = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let silent_url = format!("https://{}", silent.local_addr().unwrap());
    let _held = tokio::spawn(async move {
        let mut open = Vec::new();
        while let Ok((stream, _)) = silent.accept().await {
            open.push(stream);
        }
    });
    laptop.edit_config(|config| config["peers"]["remotes"][0]["url"] = json!(silent_url));
    let (status, body) = desk
        .api(
            "POST",
            &format!("/sessions/{migration}/inbox/msgx_seen/release"),
            None,
        )
        .await;
    assert_eq!(status, 204, "{body}");

    let messaging = laptop.runtime.messaging();
    let started = std::time::Instant::now();
    let pass = tokio::spawn(async move { messaging.track_outcomes(last + 100_000).await });
    let released = eventually("the release is recorded", || async {
        let record = laptop.record(&review, "msgx_seen").await?;
        (record["outcome"] == "released").then(|| started.elapsed())
    })
    .await;
    assert!(released < Duration::from_secs(3), "{released:?}");
    pass.await.unwrap().unwrap();
    // One timeout for the silent peer, not one per message.
    let took = started.elapsed();
    assert!(
        took < cyber_server::runtime::messaging::peer::REQUEST_TIMEOUT * 2,
        "{took:?}"
    );
    for i in 0..3 {
        let id = format!("msgx_silent{i}");
        let (sent, next) = poll_times(&laptop, &id);
        assert!(
            next.is_some_and(|next| next > sent + 5_000),
            "{id}: {next:?}"
        );
        assert_eq!(
            laptop.record(&review, &id).await.unwrap()["outcome"],
            "held"
        );
    }
    laptop.stop().await;
    desk.stop().await;
}

/// Regression: for a listener on all interfaces the pairing code named
/// `<peers.machine>.local`, a name nothing answers for when `peers.machine` is not the
/// host name (the mDNS record is `cyber-<machine>.local`), so `cyber peers add` failed
/// with `PeerUnreachableError`. On the production server the default code now names this
/// host's LAN name, the one its mDNS record advertises.
#[tokio::test]
async fn a_wildcard_pairing_code_names_the_hosts_lan_name() {
    let temp = tempfile::tempdir().unwrap();
    let model = scripted_model::ScriptedModel::start(support::prompted_call);
    let machine = format!("desk-{}", ulid::Ulid::new().to_string().to_lowercase());
    let served = Served::start(
        &temp.path().canonicalize().unwrap(),
        &model,
        json!({"listen": {"address": "0.0.0.0:0"}, "machine": machine}),
    )
    .await;
    let port = served
        .registration
        .peer_url
        .as_deref()
        .and_then(|url| url.rsplit(':').next())
        .unwrap()
        .to_string();
    let (status, body) = served
        .api("POST", "/peers/grants", Some(json!({"name": "laptop"})))
        .await;
    assert_eq!(status, 201, "{body}");
    let code = body["data"]["code"].as_str().unwrap();
    let host = cyber_server::http::peer::lan_host(&machine);
    assert!(
        code.starts_with(&format!("cyberpeer://{host}:{port}/")),
        "{code}"
    );
    assert!(!code.contains(&format!("{machine}.local")), "{code}");
    served.stop().await;
}

/// `server-api` → Prompt attachment parts: a paired Device's path names nothing on this
/// machine, so an attached peer's `file` part is refused before anything is read, while
/// its inline `document` part passes the peer filter and reaches the normal admission
/// checks (here the text-only model's modality check).
#[tokio::test]
async fn peer_prompt_refuses_file_parts() {
    use cyber_server::runtime::messaging::PeerScope;
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let model = scripted_model::ScriptedModel::start(|_| json!({"content": "ok"}));
    let allow = json!({"permissions": {"remote.attach": {"peer:laptop": "allow"}}});
    let desk = attach_desk(&root, &model, allow, Duration::from_secs(3)).await;
    let peer = desk.registration.peer_url.clone().unwrap();
    let laptop = grant_token(&desk, "laptop", vec![PeerScope::Attach]);
    let session = desk.session("shots").await;
    let attach = open_attach(&peer, &session, &laptop).await;
    std::fs::write(root.join("repo/secret.png"), b"\x89PNG\r\n\x1a\nsecret").unwrap();
    let prompt = |parts: Value| {
        peer_client()
            .post(format!("{peer}/api/v1/peer/attach/{session}/prompt"))
            .bearer_auth(&laptop)
            .header("x-cyber-attach", attach.as_str())
            .json(&json!({ "parts": parts }))
            .send()
    };

    let refused = prompt(json!([
        {"type": "text", "text": "look"},
        {"type": "file", "path": root.join("repo/secret.png").display().to_string()},
    ]))
    .await
    .unwrap();
    assert_eq!(refused.status().as_u16(), 400);
    let body: Value = refused.json().await.unwrap();
    assert_eq!(body["_tag"], "InvalidRequestError", "{body}");
    assert!(
        body["message"]
            .as_str()
            .unwrap()
            .contains("attachment parts only"),
        "{body}"
    );
    assert!(requests(&desk, &session, "read").await.is_empty());
    assert!(!transcript(&desk, &session).await.contains("look"));
    assert!(!root.join("data/attachments").join(&session).exists());

    let pdf = base64::engine::general_purpose::STANDARD.encode(b"%PDF-1.4\n%%EOF\n");
    let document = prompt(json!([
        {"type": "document", "media_type": "application/pdf", "data": pdf, "name": "spec.pdf"},
    ]))
    .await
    .unwrap();
    assert_eq!(document.status().as_u16(), 422);
    let body: Value = document.json().await.unwrap();
    assert_eq!(body["_tag"], "UnsupportedModalityError", "{body}");
    assert_eq!(body["modality"], "pdf", "{body}");

    // A blob the Session stores (a queued input taken back in `cyber attach`) passes
    // the peer filter and reaches the Session's own checks: its type comes from its
    // bytes, and this model has no image input.
    let png = base64::engine::general_purpose::STANDARD
        .decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==")
        .unwrap();
    let store = desk.runtime.attachments().unwrap();
    let sha256 = store.write(&session, &png).unwrap().sha256;
    let stored = prompt(json!([
        {"type": "text", "text": "again"},
        {"type": "attachment", "sha256": sha256},
    ]))
    .await
    .unwrap();
    let status = stored.status().as_u16();
    let body: Value = stored.json().await.unwrap();
    assert_eq!(status, 422, "{body}");
    assert_eq!(body["_tag"], "UnsupportedModalityError", "{body}");
    assert_eq!(body["modality"], "image", "{body}");
    desk.stop().await;
}
