//! Real HTTP and signed-identity integration tests; no provisioned credentials.
use crate::fixture;
use crate::{
    api::*,
    auth::BackendCredentials,
    client::HubClient,
    ids::*,
    model::*,
    serve::ServeConfig,
    server::{self, Hub},
    storage::*,
};
use cyber_account::Secret;
use reqwest::Method;
use serde_json::json;
use std::sync::Arc;
#[derive(Default)]
struct Vault(std::sync::Mutex<std::collections::HashMap<String, Secret>>);
impl cyber_account::CredentialVault for Vault {
    fn get(&self, key: &str) -> Result<Option<Secret>, cyber_account::Error> {
        Ok(self.0.lock().unwrap().get(key).cloned())
    }
    fn put(&self, key: &str, value: &str) -> Result<(), cyber_account::Error> {
        self.0
            .lock()
            .unwrap()
            .insert(key.into(), Secret::new(value.into()));
        Ok(())
    }
    fn delete(&self, key: &str) -> Result<(), cyber_account::Error> {
        self.0.lock().unwrap().remove(key);
        Ok(())
    }
}
struct TestHub {
    hub: Arc<Hub>,
    url: String,
    _dir: tempfile::TempDir,
    task: tokio::task::JoinHandle<()>,
    fixture: fixture::Fixture,
    alice: HubClient,
    bob: HubClient,
    mallory: HubClient,
}
impl Drop for TestHub {
    fn drop(&mut self) {
        self.task.abort();
    }
}

#[tokio::test]
async fn hub_restart_preserves_identity_and_backup_but_missing_mac_fails_closed() {
    let f = fixture::Fixture::start().await;
    let dir = tempfile::tempdir().unwrap();
    let mut config = ServeConfig::new(&f.state.issuer, "hub", "alice", dir.path().join("hub.db"));
    config.enabled = true;
    config.allow_loopback_http = true;
    config.introspection = Some(BackendCredentials::Token(f.service()));
    let vault = Arc::new(Vault::default());
    let hub = Hub::with_vault(&config, Some(vault.clone())).await.unwrap();
    let id = hub.db.hub_id.clone();
    hub.db
        .write(|db| put(db, "test", "persistent", "scope", &json!({"value":42})))
        .unwrap();
    let backup = dir.path().join("backup.db");
    cyber_store::backup::backup(&config.database, &backup).unwrap();
    assert_eq!(cyber_store::backup::integrity_check(&backup).unwrap(), "ok");
    let db = rusqlite::Connection::open(&backup).unwrap();
    assert_eq!(
        get::<serde_json::Value>(&db, "test", "persistent").unwrap()["value"],
        42
    );
    drop(hub);
    let restarted = Hub::with_vault(&config, Some(vault.clone())).await.unwrap();
    assert_eq!(id, restarted.db.hub_id);
    assert_eq!(
        restarted
            .db
            .read(|db| get::<serde_json::Value>(db, "test", "persistent"))
            .unwrap()["value"],
        42
    );
    drop(restarted);
    vault.0.lock().unwrap().clear();
    assert!(Hub::with_vault(&config, Some(vault)).await.is_err());
}

#[test]
fn failed_storage_rolls_back_and_stops_new_mutations() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("hub.db");
    let db = Database::open(&path).unwrap();
    let failure = db.write(|db| {
        put(db, "test", "partial", "scope", &json!({"not":"committed"}))?;
        Err::<(), _>(crate::error::Error::storage())
    });
    assert!(failure.is_err());
    assert!(
        db.read(|db| get::<serde_json::Value>(db, "test", "partial"))
            .is_err()
    );
    assert!(
        db.write(|db| put(db, "test", "later", "scope", &json!({})))
            .is_err()
    );
    drop(db);
    let restarted = Database::open(&path).unwrap();
    restarted
        .write(|db| put(db, "test", "after-restart", "scope", &json!({"ok":true})))
        .unwrap();
}
impl TestHub {
    async fn new() -> Self {
        let fixture = fixture::Fixture::start().await;
        let dir = tempfile::tempdir().unwrap();
        let mut config = ServeConfig::new(
            &fixture.state.issuer,
            "hub",
            "alice",
            dir.path().join("hub.db"),
        );
        config.enabled = true;
        config.allow_loopback_http = true;
        config.introspection = Some(BackendCredentials::Token(fixture.service()));
        config.require_mfa = true;
        let hub = Hub::with_vault(&config, Some(Arc::new(Vault::default())))
            .await
            .unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let app = server::router(hub.clone());
        let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let alice = HubClient::new(&url, fixture.user("alice", json!([{"id":"org"}]))).unwrap();
        let bob = HubClient::new(&url, fixture.user("bob", json!([{"id":"org"}]))).unwrap();
        let mallory = HubClient::new(&url, fixture.user("mallory", json!([]))).unwrap();
        Self {
            hub,
            url,
            _dir: dir,
            task,
            fixture,
            alice,
            bob,
            mallory,
        }
    }
    async fn project(&self) -> Project {
        self.alice
            .mutate(
                Method::POST,
                "/projects",
                &Mutation::new(CreateProject {
                    name: "project".into(),
                    repository: "git@github.com:Team/Repo.git".into(),
                    mode: ProjectMode::Organization {
                        organization_id: "org".into(),
                    },
                }),
            )
            .await
            .unwrap()
            .data
    }
    fn add(&self, project: &Project, sub: &str) -> Member {
        let member = Member {
            id: MemberId::new(),
            project: project.id.clone(),
            issuer: self.fixture.state.issuer.clone(),
            subject: sub.into(),
            name: sub.into(),
            role: Role::Member,
            kind: MembershipKind::Internal,
            generation: 1,
            active: true,
        };
        let copy = member.clone();
        self.hub
            .db
            .write(move |db| put(db, "member", copy.id.as_str(), copy.project.as_str(), &copy))
            .unwrap();
        member
    }
    async fn agent(&self, client: &HubClient, p: &Project, name: &str) -> Published {
        let reg: RuntimeRegistration = client
            .mutate(
                Method::POST,
                &format!("/projects/{}/runtimes", p.id),
                &Mutation::new(RegisterRuntime::default()),
            )
            .await
            .unwrap()
            .data;
        client
            .mutate(
                Method::POST,
                &format!("/projects/{}/agents", p.id),
                &Mutation::new(Publish {
                    runtime: reg.runtime,
                    publication_key: name.into(),
                    name: name.into(),
                    repository: p.repository.clone(),
                    subscribe_project_messages: false,
                }),
            )
            .await
            .unwrap()
            .data
    }
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn project_scope_private_conversations_and_durable_fenced_tasks() {
    let h = TestHub::new().await;
    let p = h.project().await;
    let bob = h.add(&p, "bob");
    assert!(
        h.mallory
            .get::<Project>(&format!("/projects/{}", p.id))
            .await
            .is_err()
    );
    assert!(
        h.mallory
            .mutate::<_, Project>(
                Method::POST,
                "/projects",
                &Mutation::new(CreateProject {
                    name: "hijack".into(),
                    repository: "https://github.com/team/repo".into(),
                    mode: ProjectMode::Friends
                })
            )
            .await
            .is_err()
    );
    let a = h.agent(&h.alice, &p, "a").await;
    let b = h.agent(&h.bob, &p, "b").await;
    let ac = HubClient::new(&h.url, Secret::new(a.capability.clone().unwrap())).unwrap();
    let bc = HubClient::new(&h.url, Secret::new(b.capability.clone().unwrap())).unwrap();
    let input = Mutation::new(SendMessage {
        target: MessageTarget::Direct(Participant::Agent(b.agent.id.clone())),
        text: "review this patch".into(),
        reply_to: None,
        depth: 0,
        hops: 1,
    });
    let message: Message = ac
        .mutate(
            Method::POST,
            &format!("/projects/{}/messages", p.id),
            &input,
        )
        .await
        .unwrap()
        .data;
    let replay: MutationResult<Message> = ac
        .mutate(
            Method::POST,
            &format!("/projects/{}/messages", p.id),
            &input,
        )
        .await
        .unwrap();
    assert!(replay.replayed);
    assert_eq!(replay.data.id, message.id);
    let mut conflict = input.clone();
    conflict.payload.text = "different".into();
    assert_eq!(
        ac.mutate::<_, Message>(
            Method::POST,
            &format!("/projects/{}/messages", p.id),
            &conflict
        )
        .await
        .unwrap_err()
        .tag,
        "OperationConflictError"
    );
    let deliveries: Vec<Delivery> = bc
        .get(&format!(
            "/projects/{}/deliveries?agent={}",
            p.id, b.agent.id
        ))
        .await
        .unwrap();
    assert_eq!(deliveries.len(), 1);
    let authorization: DeliveryAuthorization = bc
        .get(&format!(
            "/projects/{}/deliveries/{}/{}/authorization",
            p.id, message.id, b.agent.id
        ))
        .await
        .unwrap();
    assert!(authorization.allowed);
    let task: Task = ac
        .mutate(
            Method::POST,
            &format!("/projects/{}/tasks", p.id),
            &Mutation::new(CreateTask {
                title: "fix".into(),
                description: "bug".into(),
                dependencies: vec![],
            }),
        )
        .await
        .unwrap()
        .data;
    let route = format!("/projects/{}/tasks/{}/claim", p.id, task.id);
    let ia = Mutation::new(ClaimTask {
        expected_revision: 1,
    });
    let ib = Mutation::new(ClaimTask {
        expected_revision: 1,
    });
    let (ra, rb) = tokio::join!(
        ac.mutate::<_, Task>(Method::POST, &route, &ia),
        bc.mutate::<_, Task>(Method::POST, &route, &ib)
    );
    assert_ne!(ra.is_ok(), rb.is_ok());
    let claimed = ra.as_ref().or(rb.as_ref()).unwrap();
    assert_eq!(claimed.data.claim_generation, 1);
    let loser = if ra.is_ok() { &bc } else { &ac };
    assert!(
        loser
            .mutate::<_, Task>(
                Method::PATCH,
                &format!("/projects/{}/tasks/{}", p.id, task.id),
                &Mutation::new(UpdateTask {
                    expected_revision: 2,
                    claim_generation: 1,
                    status: Some(TaskStatus::Done),
                    ..Default::default()
                })
            )
            .await
            .is_err()
    );
    let id = bob.id.clone();
    h.hub
        .db
        .write(move |db| {
            let mut m: Member = get(db, "member", id.as_str())?;
            m.active = false;
            m.generation += 1;
            put(db, "member", id.as_str(), m.project.as_str(), &m)
        })
        .unwrap();
    assert!(
        bc.get::<Vec<Task>>(&format!("/projects/{}/tasks", p.id))
            .await
            .is_err()
    );
    let surviving: Vec<Task> = h
        .alice
        .get(&format!("/projects/{}/tasks", p.id))
        .await
        .unwrap();
    assert_eq!(surviving[0].assignee, claimed.data.assignee);
    assert!(
        reqwest::get(format!("{}/api/v1/session", h.url))
            .await
            .unwrap()
            .status()
            .is_client_error()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn human_senders_need_no_runtime_and_direct_history_hides_from_unrelated_owner() {
    let h = TestHub::new().await;
    let p = h.project().await;
    let bob = h.add(&p, "bob");
    let target = h.agent(&h.bob, &p, "target").await;
    let bc = HubClient::new(&h.url, Secret::new(target.capability.unwrap())).unwrap();
    let message: Message = h
        .alice
        .mutate(
            Method::POST,
            &format!("/projects/{}/messages", p.id),
            &Mutation::new(SendMessage {
                target: MessageTarget::Direct(Participant::Agent(target.agent.id.clone())),
                text: "human without runtime".into(),
                reply_to: None,
                depth: 0,
                hops: 1,
            }),
        )
        .await
        .unwrap()
        .data;
    let allowed: DeliveryAuthorization = bc
        .get(&format!(
            "/projects/{}/deliveries/{}/{}/authorization",
            p.id, message.id, target.agent.id
        ))
        .await
        .unwrap();
    assert!(allowed.allowed);
    let private: Message = h
        .bob
        .mutate(
            Method::POST,
            &format!("/projects/{}/messages", p.id),
            &Mutation::new(SendMessage {
                target: MessageTarget::Direct(Participant::Agent(target.agent.id.clone())),
                text: "private human to own agent".into(),
                reply_to: None,
                depth: 0,
                hops: 1,
            }),
        )
        .await
        .unwrap()
        .data;
    let messages: Vec<Message> = h
        .alice
        .get(&format!("/projects/{}/messages", p.id))
        .await
        .unwrap();
    assert!(!messages.iter().any(|m| m.id == private.id));
    let snapshot: Snapshot = h
        .alice
        .get(&format!("/projects/{}/snapshot", p.id))
        .await
        .unwrap();
    assert!(!snapshot.messages.iter().any(|m| m.id == private.id));
    let events: EventPage = h
        .alice
        .get(&format!("/projects/{}/events", p.id))
        .await
        .unwrap();
    assert!(
        !events
            .events
            .iter()
            .any(|e| e.data["id"] == private.id.as_str())
    );
    h.fixture
        .state
        .users
        .lock()
        .unwrap()
        .get_mut("alice")
        .unwrap()["orgs"] = json!([]);
    // Sensitive operations always introspect, invalidating the old cached source proof.
    assert!(
        h.alice
            .mutate::<_, Message>(
                Method::POST,
                &format!("/projects/{}/messages", p.id),
                &Mutation::new(SendMessage {
                    target: MessageTarget::Project,
                    text: "revoked org".into(),
                    reply_to: None,
                    depth: 0,
                    hops: 1
                })
            )
            .await
            .is_err()
    );
    let allowed: DeliveryAuthorization = bc
        .get(&format!(
            "/projects/{}/deliveries/{}/{}/authorization",
            p.id, message.id, target.agent.id
        ))
        .await
        .unwrap();
    assert!(!allowed.allowed);
    let _ = bob;
}

#[tokio::test]
async fn issuance_replay_never_persists_capabilities_and_stale_runtime_cannot_publish() {
    let h = TestHub::new().await;
    let p = h.project().await;
    let reg: RuntimeRegistration = h
        .alice
        .mutate(
            Method::POST,
            &format!("/projects/{}/runtimes", p.id),
            &Mutation::new(RegisterRuntime::default()),
        )
        .await
        .unwrap()
        .data;
    let input = Mutation::new(Publish {
        runtime: reg.runtime.clone(),
        publication_key: "stable".into(),
        name: "agent".into(),
        repository: p.repository.clone(),
        subscribe_project_messages: false,
    });
    let first: MutationResult<Published> = h
        .alice
        .mutate(Method::POST, &format!("/projects/{}/agents", p.id), &input)
        .await
        .unwrap();
    let secret = first.data.capability.unwrap();
    let replay: MutationResult<Published> = h
        .alice
        .mutate(Method::POST, &format!("/projects/{}/agents", p.id), &input)
        .await
        .unwrap();
    assert!(replay.replayed);
    assert!(replay.data.capability.is_none());
    assert_eq!(replay.data.agent.id, first.data.agent.id);
    let public = h
        .hub
        .db
        .read(move |db| {
            let mut stmt = db.prepare("SELECT result FROM ch_operations")?;
            let rows = stmt
                .query_map([], |r| r.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows.join(""))
        })
        .unwrap();
    assert!(!public.contains(&secret));
    h.hub.leases_clear_for_test();
    assert!(
        h.alice
            .mutate::<_, Published>(
                Method::POST,
                &format!("/projects/{}/agents", p.id),
                &Mutation::new(input.payload)
            )
            .await
            .is_err()
    );
    assert!(
        HubClient::new(&h.url, Secret::new(secret))
            .unwrap()
            .get::<Vec<Task>>(&format!("/projects/{}/tasks", p.id))
            .await
            .is_err()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn friends_enrollment_over_http_keeps_code_attempt_retries_idempotent() {
    let h = TestHub::new().await;
    let p: Project = h
        .alice
        .mutate(
            Method::POST,
            "/projects",
            &Mutation::new(CreateProject {
                name: "friends".into(),
                repository: "https://github.com/team/friends".into(),
                mode: ProjectMode::Friends,
            }),
        )
        .await
        .unwrap()
        .data;
    let invite: IssuedInvitation = h
        .alice
        .mutate(
            Method::POST,
            &format!("/projects/{}/invitations", p.id),
            &Mutation::new(Invite {
                role: Role::Member,
                subject: Some("bob".into()),
                guest: false,
            }),
        )
        .await
        .unwrap()
        .data;
    let joined: JoinOutcome = h
        .bob
        .mutate(
            Method::POST,
            "/join",
            &Mutation::new(Join {
                invitation_token: invite.token,
                project: None,
                name: None,
            }),
        )
        .await
        .unwrap()
        .data;
    assert_eq!(joined.status, ApplicationStatus::PendingApproval);
    assert!(joined.project.is_none());
    let application = joined.application.unwrap();
    let issued: IssuedChallenge = h
        .alice
        .mutate(
            Method::POST,
            &format!("/projects/{}/applications/{}/approve", p.id, application.id),
            &Mutation::new(Empty {}),
        )
        .await
        .unwrap()
        .data;
    let code = issued.code.unwrap();
    let wrong = if code == "00000000" {
        "00000001"
    } else {
        "00000000"
    };
    let route = format!("/applications/{}/redeem", application.id);
    let attempt = Mutation::new(Redeem {
        repository: p.repository.clone(),
        code: Some(wrong.into()),
    });
    for _ in 0..2 {
        assert_eq!(
            h.bob
                .mutate::<_, JoinOutcome>(Method::POST, &route, &attempt)
                .await
                .unwrap_err()
                .tag,
            "InvalidJoinCodeError"
        );
    }
    let project = p.id.to_string();
    let challenges: Vec<serde_json::Value> = h
        .hub
        .db
        .read(move |db| list(db, "challenge", &project))
        .unwrap();
    assert_eq!(challenges.len(), 1);
    assert_eq!(challenges[0]["attempts"], 1);
    let redeem = Mutation::new(Redeem {
        repository: p.repository.clone(),
        code: Some(code.clone()),
    });
    let member: JoinOutcome = h
        .bob
        .mutate(Method::POST, &route, &redeem)
        .await
        .unwrap()
        .data;
    assert_eq!(member.status, ApplicationStatus::Joined);
    let replay: MutationResult<JoinOutcome> =
        h.bob.mutate(Method::POST, &route, &redeem).await.unwrap();
    assert!(replay.replayed);
    let id = member.member.unwrap().id;
    let _: MutationResult<serde_json::Value> = h
        .alice
        .mutate(
            Method::DELETE,
            &format!("/projects/{}/members/{id}", p.id),
            &Mutation::new(Empty {}),
        )
        .await
        .unwrap();
    assert!(
        h.bob
            .mutate::<_, JoinOutcome>(Method::POST, &route, &redeem)
            .await
            .is_err()
    );
    let cache = h
        .hub
        .db
        .read(|db| {
            let mut stmt = db.prepare("SELECT result FROM ch_operations")?;
            Ok(stmt
                .query_map([], |r| r.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()?
                .join(""))
        })
        .unwrap();
    assert!(!cache.contains(&code));
}
