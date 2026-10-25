//! Real owner HTTP authorization with a fake native backend; never capture or inject input.
mod support;
use cyber_computer::{
    Error,
    backend::{Backend, Capture, Health, NativeTarget, Selector},
    control::InputPermit,
};
use cyber_server::{
    http::{self, AgentInfo, AppState, CommandInfo, HttpOptions, ModelInfo, Services},
    runtime::{Runtime, ToolDef, TurnContext, computer::ComputerPolicy, presence::Watching},
};
use futures::future::BoxFuture;
use serde_json::{Value, json};
use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};
use support::{Harness, Setup};
#[derive(Default)]
struct NoServices {
    publication_calls: Arc<AtomicUsize>,
}
impl Services for NoServices {
    fn integration_operation<'a>(
        &'a self,
        _: &'a TurnContext,
        _: &'a str,
        _: &'a str,
        _: Value,
    ) -> BoxFuture<'a, Result<Value, http::ApiError>> {
        self.publication_calls.fetch_add(1, Ordering::SeqCst);
        Box::pin(async {
            Err(http::ApiError::service_unavailable(
                "fixture",
                "publication must not dispatch",
            ))
        })
    }
    fn models(&self, _: &Path) -> BoxFuture<'_, Result<Vec<ModelInfo>, String>> {
        Box::pin(async { Ok(vec![]) })
    }
    fn default_model(&self, _: &Path) -> Option<String> {
        Some("test/main".into())
    }
    fn agents(&self, _: &Path) -> Vec<AgentInfo> {
        vec![]
    }
    fn tools(&self, _: &TurnContext) -> Vec<ToolDef> {
        vec![]
    }
    fn commands(&self, _: &Path) -> Vec<CommandInfo> {
        vec![]
    }
    fn find_files(&self, _: &Path, _: &str, _: usize) -> Vec<String> {
        vec![]
    }
    fn expand_command(&self, _: &Path, _: &str, _: &str) -> Option<String> {
        None
    }
}
fn state(h: &Harness, runtime: Runtime) -> AppState {
    AppState {
        service: None,
        telemetry: None,
        runtime,
        remote_tools: Arc::default(),
        store: h.store.clone(),
        services: Arc::new(NoServices::default()),
        options: Arc::new(HttpOptions {
            version: "fixture".into(),
            password: Some("owner-fixture-password".into()),
            cors_origins: vec![],
            default_directory: h.repo.clone(),
            features: vec!["session".into()],
        }),
    }
}

#[derive(Default)]
struct Fake {
    prompts: AtomicUsize,
    capture_allowed: bool,
}
impl Backend for Fake {
    fn health(&self, prompt: bool) -> Result<Health, Error> {
        if prompt {
            self.prompts.fetch_add(1, Ordering::SeqCst);
        }
        Ok(Health {
            supported: true,
            platform: "fixture".into(),
            minimum_version: "fixture".into(),
            screen_capture: true,
            accessibility: true,
            binary: "fixture".into(),
        })
    }
    fn resolve(&self, _: &Selector) -> Result<NativeTarget, Error> {
        Ok(NativeTarget {
            identity: "app:test.fixture:42:1:7".into(),
            app: Some("test.fixture".into()),
            pid: 42,
            window: 7,
            display: 1,
            launch_ms: 1,
            x: 0.0,
            y: 0.0,
            width: 100.0,
            height: 100.0,
            image_width: 100,
            image_height: 100,
            rotation: 0,
        })
    }
    fn capture(&self, target: &NativeTarget) -> Result<Capture, Error> {
        assert!(
            self.capture_allowed,
            "owner control route must never capture"
        );
        let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
        png.extend(100u32.to_be_bytes());
        png.extend(100u32.to_be_bytes());
        Ok(Capture {
            target: target.clone(),
            png,
        })
    }
    fn input(&self, _: &NativeTarget, _: &InputPermit) -> Result<(), Error> {
        panic!("owner control route must never inject input")
    }
}
#[tokio::test]
async fn owner_routes_bind_auth_location_exact_review_and_global_stop() {
    let h = Harness::new(Setup::default());
    let id = h.session().await;
    let backend = Arc::new(Fake::default());
    h.runtime
        .configure_computer_backend(
            h.dir.path().join("computer"),
            Arc::new(|| ComputerPolicy {
                enabled: true,
                retention_days: 1,
            }),
            backend.clone(),
        )
        .unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/api/v1", listener.local_addr().unwrap());
    let services = Arc::new(NoServices::default());
    let mut configured = state(&h, h.runtime.clone());
    configured.services = services.clone();
    let task = tokio::spawn(http::serve_tcp(
        http::router(configured),
        listener,
        std::future::pending(),
    ));
    let client = reqwest::Client::new();
    assert_eq!(
        client
            .get(format!("{url}/tools/computer/health"))
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    let owner = || {
        client
            .post(format!("{url}/sessions/{id}/computer/preview"))
            .basic_auth("cyber", Some("owner-fixture-password"))
            .json(&json!({"app":"test.fixture"}))
    };
    assert_eq!(
        owner()
            .header("origin", "https://untrusted.fixture")
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    assert_eq!(owner().send().await.unwrap().status(), 403); // no actual human Session stream
    let _presence = h.runtime.attach_client(Watching::Session(id.clone()));
    assert_eq!(
        owner()
            .header("x-cyber-directory", h.dir.path().display().to_string())
            .send()
            .await
            .unwrap()
            .status(),
        404
    );
    let preview = owner().send().await.unwrap();
    assert_eq!(preview.status(), 200);
    assert_eq!(preview.headers()["cache-control"], "no-store");
    let preview = preview.json::<Value>().await.unwrap();
    let path = format!("{url}/sessions/{id}/computer/allow");
    assert_eq!(
        client
            .post(&path)
            .basic_auth("cyber", Some("owner-fixture-password"))
            .json(&json!({"preview_id":preview["data"]["id"],"confirm":false}))
            .send()
            .await
            .unwrap()
            .status(),
        400
    );
    let armed = client
        .post(&path)
        .basic_auth("cyber", Some("owner-fixture-password"))
        .json(&json!({"preview_id":preview["data"]["id"],"confirm":true}))
        .send()
        .await
        .unwrap();
    assert_eq!(armed.status(), 200);
    assert!(
        h.runtime.switch_mode(&id, "bypass").await.is_err(),
        "native input cannot change its own Mode while armed"
    );
    let publication = client.post(format!("{url}/workflows/integrations/intg_fixture/publish"))
        .basic_auth("cyber",Some("owner-fixture-password"))
        .json(&json!({"expected_revision":1,"operation_id":"native-self-publish","remote":"origin","repository":"fixture/repository","title":"fixture","body":"fixture"}))
        .send().await.unwrap();
    assert_eq!(publication.status(), 403);
    let denied: Value = publication.json().await.unwrap();
    assert_eq!(denied["_tag"], "ComputerConsentError");
    assert_eq!(
        services.publication_calls.load(Ordering::SeqCst),
        0,
        "armed native input cannot dispatch Git/GitHub publication"
    );
    let original = h.runtime.session_info(&id).await.unwrap();
    assert!(h.runtime.switch_agent(&id, "reviewer").await.is_err());
    assert!(h.runtime.switch_model(&id, "other/model").await.is_err());
    let unchanged = h.runtime.session_info(&id).await.unwrap();
    assert_eq!(unchanged.agent, original.agent);
    assert_eq!(unchanged.model, original.model);
    let asker = h.runtime.operation_asker(&id).await.unwrap();
    let asking = tokio::spawn(async move {
        asker
            .permission(cyber_server::runtime::PermissionAsk {
                action: "bash".into(),
                resources: vec!["echo fixture".into()],
                always_patterns: vec![],
                metadata: Value::Null,
            })
            .await
    });
    let request = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            if let Some(request) = h.runtime.pending_requests(Some(&id)).into_iter().next() {
                break request;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(
        h.runtime
            .reply_permission(&request.id, cyber_server::runtime::PermissionReply::Always)
            .await
            .is_err()
    );
    assert!(
        h.runtime
            .pending_requests(Some(&id))
            .iter()
            .any(|r| r.id == request.id),
        "blocked self-approval leaves the request pending"
    );
    h.runtime
        .reply_permission(
            &request.id,
            cyber_server::runtime::PermissionReply::Reject {
                message: Some("fixture denial".into()),
            },
        )
        .await
        .unwrap();
    assert!(matches!(
        asking.await.unwrap(),
        cyber_server::runtime::PermissionReply::Reject { .. }
    ));
    let health = client
        .get(format!("{url}/tools/computer/health"))
        .basic_auth("cyber", Some("owner-fixture-password"))
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap();
    assert_eq!(health["data"]["grant"]["session_id"], id);
    assert_eq!(
        client
            .post(format!("{url}/tools/computer/stop"))
            .basic_auth("cyber", Some("owner-fixture-password"))
            .send()
            .await
            .unwrap()
            .status(),
        204
    );
    assert!(
        h.runtime
            .computer()
            .unwrap()
            .status(&id)
            .unwrap()
            .grant
            .is_none()
    );
    assert_eq!(
        client
            .post(format!("{url}/tools/computer/setup"))
            .basic_auth("cyber", Some("owner-fixture-password"))
            .json(&json!({"confirm":false}))
            .send()
            .await
            .unwrap()
            .status(),
        400
    );
    assert_eq!(backend.prompts.load(Ordering::SeqCst), 0);
    task.abort();
}

#[tokio::test]
async fn native_tool_arguments_and_doom_loop_metadata_never_persist_typed_content() {
    use cyber_server::runtime::{Admission, Delivery, PermissionReply, RetrySafety};
    let secret = "native-private-sentinel";
    let args = r#"{"action":"type","frame_id":"cuf_00000000000000000000000000","text":"native-private-sentinel"}"#;
    let h = Harness::new(Setup {
        scripts: vec![(
            "test/main",
            vec![
                support::tools(&[
                    ("one", "computer", args),
                    ("two", "computer", args),
                    ("three", "computer", args),
                ]),
                support::text("done"),
            ],
        )],
        ..Setup::default()
    });
    h.tools
        .defs
        .lock()
        .unwrap()
        .push(support::def("computer", RetrySafety::Never, false));
    h.tools
        .set("computer", support::Behavior::Return("posted".into()));
    let id = h.session().await;
    h.runtime
        .admit(&id, Admission::text("native fixture", Delivery::Queue))
        .await
        .unwrap();
    let request = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            if let Some(request) = h
                .runtime
                .pending_requests(Some(&id))
                .into_iter()
                .find(|r| matches!(&r.kind,cyber_server::runtime::PendingKind::Permission(ask) if ask.action=="doom_loop"))
            {
                break request;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let metadata = match &request.kind {
        cyber_server::runtime::PendingKind::Permission(ask) => &ask.metadata,
        _ => unreachable!(),
    };
    assert!(!metadata.to_string().contains(secret));
    assert!(metadata.to_string().contains("text_sha256"));
    h.runtime
        .reply_permission(&request.id, PermissionReply::Reject { message: None })
        .await
        .unwrap();
    h.settle(&id).await;
    for event in h.store.read_events(&id, -1, 500).unwrap().events {
        assert!(
            !event.data.to_string().contains(secret),
            "raw native input in {}",
            event.kind
        );
    }
    assert!(
        h.tools
            .executed
            .lock()
            .unwrap()
            .iter()
            .any(|inv| inv.input.to_string().contains(secret)),
        "actual native invocation remains ephemeral until dispatch"
    );
}

#[tokio::test]
async fn deleting_session_removes_private_captures_with_retention_disabled() {
    use sha2::{Digest, Sha256};
    let h = Harness::new(Setup::default());
    let id = h.session().await;
    let artifacts = h.dir.path().join("computer-delete");
    h.runtime
        .configure_computer_backend(
            artifacts.clone(),
            Arc::new(|| ComputerPolicy {
                enabled: true,
                retention_days: 0,
            }),
            Arc::new(Fake {
                capture_allowed: true,
                ..Fake::default()
            }),
        )
        .unwrap();
    let _presence = h.runtime.attach_client(Watching::Session(id.clone()));
    let computer = h.runtime.computer().unwrap();
    let preview = computer
        .preview(
            &id,
            Selector {
                app: Some("test.fixture".into()),
                display: None,
            },
        )
        .unwrap();
    computer.allow(&id, &preview.id, None, false).unwrap();
    let frame = computer
        .screenshot(&id, "delete-private-frame")
        .unwrap()
        .frame
        .unwrap();
    let path = artifacts
        .join(format!("{:x}", Sha256::digest(id.as_bytes())))
        .join(format!("{}.png", frame.id));
    assert!(path.is_file());
    let owner = id.clone();
    h.store.transaction(move|tx|{tx.execute("INSERT INTO computer_operation(operation_key,session_id,action,fingerprint,audit,status,started_ms) VALUES('unknown-on-delete',?1,'type','hash','{}','unknown',0)",[owner])?;Ok(())}).unwrap();
    h.runtime.delete(&id).await.unwrap();
    assert!(!path.exists());
    assert!(computer.frame_bytes(&id, &frame.id).is_err());
    let owner = id.clone();
    let (tombstone,unknown)=h.store.read(move|db|Ok((db.query_row("SELECT COUNT(*) FROM computer_deleted_session WHERE session_id=?1",[owner],|row|row.get::<_,i64>(0))?,db.query_row("SELECT COUNT(*) FROM computer_operation WHERE operation_key='unknown-on-delete'",[],|row|row.get::<_,i64>(0))?))).unwrap();
    assert_eq!((tombstone, unknown), (1, 1));
    assert!(computer.screenshot(&id, "late-after-delete").is_err());
    let unrelated = h.session().await;
    assert!(h.runtime.session_info(&unrelated).await.is_ok());
}

#[cfg(unix)]
#[tokio::test]
async fn same_uid_unix_clients_need_owner_auth_before_presence_or_computer_grants() {
    let h = Harness::new(Setup::default());
    let id = h.session().await;
    let backend = Arc::new(Fake::default());
    h.runtime
        .configure_computer_backend(
            h.dir.path().join("unix-computer"),
            Arc::new(|| ComputerPolicy {
                enabled: true,
                retention_days: 1,
            }),
            backend.clone(),
        )
        .unwrap();
    // A custom socket outside the protected state directory must not restore
    // same-UID owner authority for an otherwise sandboxed process.
    let socket = h.dir.path().join("custom-owner.sock");
    let serving = socket.clone();
    let router = http::router(state(&h, h.runtime.clone()));
    let task = tokio::spawn(async move {
        http::serve_unix(router, &serving, std::future::pending())
            .await
            .unwrap()
    });
    for _ in 0..100 {
        if socket.exists() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    let client = reqwest::Client::builder()
        .unix_socket(socket.clone())
        .build()
        .unwrap();
    assert_eq!(
        client
            .get("http://localhost/api/v1/health")
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    for password in [None, Some("wrong-fixture-password")] {
        for (suffix, body) in [
            ("interactive-clients", json!({})),
            ("computer/preview", json!({"app":"test.fixture"})),
            (
                "computer/allow",
                json!({"preview_id":"not-owner-reviewed","confirm":true}),
            ),
        ] {
            let request = client
                .post(format!("http://localhost/api/v1/sessions/{id}/{suffix}"))
                .json(&body);
            let request = match password {
                Some(password) => request.basic_auth("cyber", Some(password)),
                None => request,
            };
            let denied = request.send().await.unwrap();
            assert_eq!(denied.status(), 401);
            assert_eq!(
                denied.headers()["www-authenticate"],
                "Basic realm=\"cyber\""
            );
        }
    }
    assert!(
        h.runtime
            .computer()
            .unwrap()
            .status(&id)
            .unwrap()
            .grant
            .is_none()
    );
    let presence = client
        .post(format!(
            "http://localhost/api/v1/sessions/{id}/interactive-clients"
        ))
        .basic_auth("cyber", Some("owner-fixture-password"))
        .send()
        .await
        .unwrap();
    assert_eq!(presence.status(), 201);
    let preview = client
        .post(format!(
            "http://localhost/api/v1/sessions/{id}/computer/preview"
        ))
        .basic_auth("cyber", Some("owner-fixture-password"))
        .json(&json!({"app":"test.fixture"}))
        .send()
        .await
        .unwrap();
    assert_eq!(preview.status(), 200);
    let preview = preview.json::<Value>().await.unwrap();
    let allowed = client
        .post(format!(
            "http://localhost/api/v1/sessions/{id}/computer/allow"
        ))
        .basic_auth("cyber", Some("owner-fixture-password"))
        .json(&json!({"preview_id":preview["data"]["id"],"confirm":true}))
        .send()
        .await
        .unwrap();
    assert_eq!(allowed.status(), 200);
    assert_eq!(
        backend.prompts.load(Ordering::SeqCst),
        0,
        "fixture never prompts for native OS consent"
    );
    h.runtime.computer_stop(&id);
    task.abort();
}

#[cfg(unix)]
#[tokio::test]
async fn unix_owner_routes_fail_closed_without_a_configured_password() {
    let h = Harness::new(Setup::default());
    let id = h.session().await;
    let mut configured = state(&h, h.runtime.clone());
    Arc::get_mut(&mut configured.options).unwrap().password = None;
    let socket = h.dir.path().join("no-password.sock");
    let serving = socket.clone();
    let router = http::router(configured);
    let task = tokio::spawn(async move {
        http::serve_unix(router, &serving, std::future::pending())
            .await
            .unwrap()
    });
    for _ in 0..100 {
        if socket.exists() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    let client = reqwest::Client::builder()
        .unix_socket(socket.clone())
        .build()
        .unwrap();
    for password in [None, Some("arbitrary-fixture-password")] {
        let request = client.get(format!("http://localhost/api/v1/sessions/{id}"));
        let request = match password {
            Some(password) => request.basic_auth("cyber", Some(password)),
            None => request,
        };
        assert_eq!(request.send().await.unwrap().status(), 401);
    }
    assert_eq!(
        client
            .get("http://localhost/api/v1/health")
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    task.abort();
}
