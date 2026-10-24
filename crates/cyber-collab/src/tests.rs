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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn recent_message_pages_ignore_private_traffic_and_restore_cursors_require_resync() {
    let h = TestHub::new().await;
    let p = h.project().await;
    let bob = h.add(&p, "bob");
    let project = p.id.clone();
    let issuer = h.fixture.state.issuer.clone();
    h.hub
        .db
        .write(move |db| {
            let alice = list::<Member>(db, "member", project.as_str())?
                .into_iter()
                .find(|m| m.issuer == issuer && m.subject == "alice")
                .unwrap();
            for index in 1u128..=1010 {
                let message = Message {
                    id: format!("cmsg_{}", ulid::Ulid::from(index)).parse().unwrap(),
                    project: project.clone(),
                    author: Participant::Human(if index <= 505 {
                        alice.id.clone()
                    } else {
                        bob.id.clone()
                    }),
                    owner: if index <= 505 {
                        alice.id.clone()
                    } else {
                        bob.id.clone()
                    },
                    membership_kind: MembershipKind::Internal,
                    member_generation: 1,
                    depth: 0,
                    hops: 1,
                    target: if index <= 505 {
                        MessageTarget::Project
                    } else {
                        MessageTarget::Direct(Participant::Human(bob.id.clone()))
                    },
                    reply_to: None,
                    text: index.to_string(),
                    created_at: now(),
                    expires_at: now() + 604800,
                };
                put(
                    db,
                    "message",
                    message.id.as_str(),
                    project.as_str(),
                    &message,
                )?;
            }
            for _ in 0..6 {
                event(db, project.as_str(), "test.updated", &json!({}))?;
            }
            db.execute("DELETE FROM ch_events WHERE cursor<5", [])?;
            Ok(())
        })
        .unwrap();
    let recent: Vec<Message> = h
        .alice
        .get(&format!("/projects/{}/messages", p.id))
        .await
        .unwrap();
    assert_eq!(recent.len(), 500);
    assert_eq!(recent[0].text, "6");
    assert_eq!(recent.last().unwrap().text, "505");
    let cursor = format!("cmsg_{}", ulid::Ulid::from(250u128));
    let next: Vec<Message> = h
        .alice
        .get(&format!("/projects/{}/messages?after={cursor}", p.id))
        .await
        .unwrap();
    assert_eq!(next.len(), 255);
    assert_eq!(next[0].text, "251");
    for cursor in [1, 100000] {
        assert_eq!(
            h.alice
                .get::<EventPage>(&format!("/projects/{}/events?after={cursor}", p.id))
                .await
                .unwrap_err()
                .tag,
            "ResyncRequiredError"
        );
    }
    let snapshot: Snapshot = h
        .alice
        .get(&format!("/projects/{}/snapshot", p.id))
        .await
        .unwrap();
    assert_eq!(snapshot.cursor, 7);
    assert!(
        snapshot
            .messages
            .iter()
            .all(|m| m.target == MessageTarget::Project)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn expired_delivery_and_changed_sender_generation_cannot_release() {
    let h = TestHub::new().await;
    let p = h.project().await;
    h.add(&p, "bob");
    let target = h.agent(&h.bob, &p, "target").await;
    let client = HubClient::new(&h.url, Secret::new(target.capability.unwrap())).unwrap();
    let message: Message = h
        .alice
        .mutate(
            Method::POST,
            &format!("/projects/{}/messages", p.id),
            &Mutation::new(SendMessage {
                target: MessageTarget::Direct(Participant::Agent(target.agent.id.clone())),
                text: "expires offline".into(),
                reply_to: None,
                depth: 0,
                hops: 1,
            }),
        )
        .await
        .unwrap()
        .data;
    let project = p.id.clone();
    let id = message.id.clone();
    let agent = target.agent.id.clone();
    h.hub
        .db
        .write(move |db| {
            let mut msg: Message = get(db, "message", id.as_str())?;
            msg.expires_at = now() - 1;
            put(db, "message", id.as_str(), project.as_str(), &msg)?;
            let key = format!("{id}:{agent}");
            let mut delivery: Delivery = get(db, "delivery", &key)?;
            delivery.message.expires_at = msg.expires_at;
            put(db, "delivery", &key, project.as_str(), &delivery)
        })
        .unwrap();
    let pending: Vec<Delivery> = client
        .get(&format!(
            "/projects/{}/deliveries?agent={}",
            p.id, target.agent.id
        ))
        .await
        .unwrap();
    assert!(pending.is_empty());
    let path = format!(
        "/projects/{}/deliveries/{}/{}/authorization",
        p.id, message.id, target.agent.id
    );
    let auth: DeliveryAuthorization = client.get(&path).await.unwrap();
    assert!(!auth.allowed);
    let id = message.id;
    let target_id = target.agent.id.clone();
    h.hub
        .db
        .write(move |db| {
            let mut msg: Message = get(db, "message", id.as_str())?;
            msg.expires_at = now() + 604800;
            put(db, "message", id.as_str(), msg.project.as_str(), &msg)?;
            let delivery_id = format!("{id}:{target_id}");
            let mut delivery: Delivery = get(db, "delivery", &delivery_id)?;
            assert_eq!(delivery.state, DeliveryState::Expired);
            delivery.state = DeliveryState::Held;
            delivery.message = msg.clone();
            put(
                db,
                "delivery",
                &delivery_id,
                msg.project.as_str(),
                &delivery,
            )?;
            let mut member: Member = get(db, "member", msg.owner.as_str())?;
            member.generation += 1;
            put(
                db,
                "member",
                member.id.as_str(),
                member.project.as_str(),
                &member,
            )
        })
        .unwrap();
    let auth: DeliveryAuthorization = client.get(&path).await.unwrap();
    assert!(!auth.allowed);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn task_dependencies_block_claims_and_reject_cycles_or_another_project() {
    let h = TestHub::new().await;
    let p = h.project().await;
    h.add(&p, "bob");
    let base = format!("/projects/{}/tasks", p.id);
    let parent: Task = h
        .alice
        .mutate(
            Method::POST,
            &base,
            &Mutation::new(CreateTask {
                title: "parent".into(),
                description: String::new(),
                dependencies: vec![],
            }),
        )
        .await
        .unwrap()
        .data;
    let child: Task = h
        .alice
        .mutate(
            Method::POST,
            &base,
            &Mutation::new(CreateTask {
                title: "child".into(),
                description: String::new(),
                dependencies: vec![parent.id.clone()],
            }),
        )
        .await
        .unwrap()
        .data;
    assert!(
        h.bob
            .mutate::<_, Task>(
                Method::POST,
                &format!("{base}/{}/claim", child.id),
                &Mutation::new(ClaimTask {
                    expected_revision: 1
                })
            )
            .await
            .is_err()
    );
    let owned: Task = h
        .alice
        .mutate(
            Method::POST,
            &format!("{base}/{}/claim", parent.id),
            &Mutation::new(ClaimTask {
                expected_revision: 1,
            }),
        )
        .await
        .unwrap()
        .data;
    let _: Task = h
        .alice
        .mutate(
            Method::PATCH,
            &format!("{base}/{}", parent.id),
            &Mutation::new(UpdateTask {
                expected_revision: owned.revision,
                claim_generation: owned.claim_generation,
                status: Some(TaskStatus::Done),
                ..Default::default()
            }),
        )
        .await
        .unwrap()
        .data;
    let owned: Task = h
        .bob
        .mutate(
            Method::POST,
            &format!("{base}/{}/claim", child.id),
            &Mutation::new(ClaimTask {
                expected_revision: 1,
            }),
        )
        .await
        .unwrap()
        .data;
    assert!(
        h.bob
            .mutate::<_, Task>(
                Method::PATCH,
                &format!("{base}/{}", child.id),
                &Mutation::new(UpdateTask {
                    expected_revision: owned.revision,
                    claim_generation: owned.claim_generation,
                    dependencies: Some(vec![child.id]),
                    ..Default::default()
                })
            )
            .await
            .is_err()
    );
    let other = h.project().await;
    assert!(
        h.alice
            .mutate::<_, Task>(
                Method::POST,
                &format!("/projects/{}/tasks", other.id),
                &Mutation::new(CreateTask {
                    title: "wrong project".into(),
                    description: String::new(),
                    dependencies: vec![parent.id]
                })
            )
            .await
            .is_err()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn republishing_cannot_revive_a_revoked_agents_pending_prompts() {
    let h = TestHub::new().await;
    let p = h.project().await;
    h.add(&p, "bob");
    let source = h.agent(&h.alice, &p, "source").await;
    let target = h.agent(&h.bob, &p, "target").await;
    let sender = HubClient::new(&h.url, Secret::new(source.capability.unwrap())).unwrap();
    let receiver = HubClient::new(&h.url, Secret::new(target.capability.unwrap())).unwrap();
    let message: Message = sender
        .mutate(
            Method::POST,
            &format!("/projects/{}/messages", p.id),
            &Mutation::new(SendMessage {
                target: MessageTarget::Direct(Participant::Agent(target.agent.id.clone())),
                text: "old publication".into(),
                reply_to: None,
                depth: 0,
                hops: 1,
            }),
        )
        .await
        .unwrap()
        .data;
    let path = format!(
        "/projects/{}/deliveries/{}/{}/authorization",
        p.id, message.id, target.agent.id
    );
    assert!(
        receiver
            .get::<DeliveryAuthorization>(&path)
            .await
            .unwrap()
            .allowed
    );
    let _: MutationResult<serde_json::Value> = h
        .alice
        .mutate(
            Method::DELETE,
            &format!("/projects/{}/agents/{}", p.id, source.agent.id),
            &Mutation::new(Empty {}),
        )
        .await
        .unwrap();
    let replacement = h.agent(&h.alice, &p, "source").await;
    assert_ne!(replacement.agent.id, source.agent.id);
    assert!(
        !receiver
            .get::<DeliveryAuthorization>(&path)
            .await
            .unwrap()
            .allowed
    );
    let _: MutationResult<serde_json::Value> = h
        .bob
        .mutate(
            Method::DELETE,
            &format!("/projects/{}/agents/{}", p.id, target.agent.id),
            &Mutation::new(Empty {}),
        )
        .await
        .unwrap();
    let replacement = h.agent(&h.bob, &p, "target").await;
    assert_ne!(replacement.agent.id, target.agent.id);
    let replacement = HubClient::new(&h.url, Secret::new(replacement.capability.unwrap())).unwrap();
    assert!(
        replacement
            .get::<DeliveryAuthorization>(&path)
            .await
            .is_err()
    );
    assert!(
        replacement
            .get::<Vec<Delivery>>(&format!(
                "/projects/{}/deliveries?agent={}",
                p.id, target.agent.id
            ))
            .await
            .is_err()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn delivery_pages_filter_historical_outcomes_before_the_page_limit() {
    let h = TestHub::new().await;
    let p = h.project().await;
    h.add(&p, "bob");
    let target = h.agent(&h.bob, &p, "receiver").await;
    let client = HubClient::new(&h.url, Secret::new(target.capability.unwrap())).unwrap();
    let queued: Message = h
        .alice
        .mutate(
            Method::POST,
            &format!("/projects/{}/messages", p.id),
            &Mutation::new(SendMessage {
                target: MessageTarget::Direct(Participant::Agent(target.agent.id.clone())),
                text: "new work after long history".into(),
                reply_to: None,
                depth: 0,
                hops: 1,
            }),
        )
        .await
        .unwrap()
        .data;
    let template = queued.clone();
    let agent = target.agent.id.clone();
    h.hub
        .db
        .write(move |db| {
            for index in 1u128..=10102 {
                let mut message = template.clone();
                message.id = format!("cmsg_{}", ulid::Ulid::from(index)).parse().unwrap();
                put(
                    db,
                    "message",
                    message.id.as_str(),
                    message.project.as_str(),
                    &message,
                )?;
                let delivery = Delivery {
                    content_hash: hash(encode(&message)?.as_bytes()),
                    state: if index <= 10001 {
                        DeliveryState::Released
                    } else {
                        DeliveryState::Held
                    },
                    message,
                    target: agent.clone(),
                };
                put(
                    db,
                    "delivery",
                    &format!("{}:{agent}", delivery.message.id),
                    delivery.message.project.as_str(),
                    &delivery,
                )?;
            }
            Ok(())
        })
        .unwrap();
    let route = format!("/projects/{}/deliveries?agent={}", p.id, target.agent.id);
    let first: Vec<Delivery> = client.get(&route).await.unwrap();
    assert_eq!(first.len(), 100);
    assert!(first.iter().all(|d| d.state == DeliveryState::Held));
    let next: Vec<Delivery> = client
        .get(&format!(
            "{route}&after={}",
            first
                .last()
                .unwrap()
                .message
                .id
                .to_string()
                .to_ascii_lowercase()
        ))
        .await
        .unwrap();
    assert_eq!(next.len(), 2);
    assert_eq!(next.last().unwrap().message.id, queued.id);
    assert_eq!(next.last().unwrap().state, DeliveryState::Queued);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn historical_receipts_cannot_authorize_content_or_block_after_source_withdrawal() {
    let h = TestHub::new().await;
    let p = h.project().await;
    h.add(&p, "bob");
    let target = h.agent(&h.bob, &p, "receiver").await;
    let client = HubClient::new(&h.url, Secret::new(target.capability.unwrap())).unwrap();
    let message: Message = h
        .alice
        .mutate(
            Method::POST,
            &format!("/projects/{}/messages", p.id),
            &Mutation::new(SendMessage {
                target: MessageTarget::Direct(Participant::Agent(target.agent.id.clone())),
                text: "local outcome awaiting acknowledgement".into(),
                reply_to: None,
                depth: 0,
                hops: 1,
            }),
        )
        .await
        .unwrap()
        .data;
    let deliveries: Vec<Delivery> = client
        .get(&format!(
            "/projects/{}/deliveries?agent={}",
            p.id, target.agent.id
        ))
        .await
        .unwrap();
    h.fixture
        .state
        .users
        .lock()
        .unwrap()
        .get_mut("alice")
        .unwrap()["active"] = json!(false);
    let base = format!(
        "/projects/{}/deliveries/{}/{}",
        p.id, message.id, target.agent.id
    );
    let denied: DeliveryAuthorization = client.get(&format!("{base}/authorization")).await.unwrap();
    assert!(!denied.allowed);
    assert_eq!(denied.reason.as_deref(), Some("source_not_current"));
    let receipt: Delivery = client
        .mutate(
            Method::POST,
            &format!("{base}/ack"),
            &Mutation::new(Acknowledge {
                state: DeliveryState::Released,
                content_hash: deliveries[0].content_hash.clone(),
            }),
        )
        .await
        .unwrap()
        .data;
    assert_eq!(receipt.state, DeliveryState::Released);
    let denied: DeliveryAuthorization = client.get(&format!("{base}/authorization")).await.unwrap();
    assert!(!denied.allowed);
    assert_eq!(
        client
            .mutate::<_, Delivery>(
                Method::POST,
                &format!("{base}/ack"),
                &Mutation::new(Acknowledge {
                    state: DeliveryState::Held,
                    content_hash: deliveries[0].content_hash.clone()
                })
            )
            .await
            .unwrap_err()
            .tag,
        "DeliveryTerminalConflictError"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn human_replies_derive_chain_counters_but_agent_replies_cannot_reset_them() {
    let h = TestHub::new().await;
    let p = h.project().await;
    let bob = h.add(&p, "bob");
    let members: Vec<Member> = h
        .alice
        .get(&format!("/projects/{}/members", p.id))
        .await
        .unwrap();
    let alice = members.into_iter().find(|m| m.subject == "alice").unwrap();
    let route = format!("/projects/{}/messages", p.id);
    let parent: Message = h
        .alice
        .mutate(
            Method::POST,
            &route,
            &Mutation::new(SendMessage {
                target: MessageTarget::Direct(Participant::Human(bob.id.clone())),
                text: "thread".into(),
                reply_to: None,
                depth: 0,
                hops: 31,
            }),
        )
        .await
        .unwrap()
        .data;
    let reply: Message = h
        .bob
        .mutate(
            Method::POST,
            &route,
            &Mutation::new(SendMessage {
                target: MessageTarget::Direct(Participant::Human(alice.id)),
                text: "reply without counter configuration".into(),
                reply_to: Some(parent.id),
                depth: 0,
                hops: 1,
            }),
        )
        .await
        .unwrap()
        .data;
    assert_eq!(reply.hops, 32);
    assert!(
        h.alice
            .mutate::<_, Message>(
                Method::POST,
                &route,
                &Mutation::new(SendMessage {
                    target: MessageTarget::Direct(Participant::Human(bob.id.clone())),
                    text: "bound".into(),
                    reply_to: Some(reply.id),
                    depth: 0,
                    hops: 1
                })
            )
            .await
            .is_err()
    );
    let publication = h.agent(&h.alice, &p, "agent").await;
    let agent = HubClient::new(&h.url, Secret::new(publication.capability.unwrap())).unwrap();
    let parent: Message = agent
        .mutate(
            Method::POST,
            &route,
            &Mutation::new(SendMessage {
                target: MessageTarget::Direct(Participant::Human(bob.id.clone())),
                text: "agent thread".into(),
                reply_to: None,
                depth: 0,
                hops: 5,
            }),
        )
        .await
        .unwrap()
        .data;
    let reply: Message = h
        .bob
        .mutate(
            Method::POST,
            &route,
            &Mutation::new(SendMessage {
                target: MessageTarget::Direct(Participant::Agent(publication.agent.id)),
                text: "human response".into(),
                reply_to: Some(parent.id),
                depth: 0,
                hops: 1,
            }),
        )
        .await
        .unwrap()
        .data;
    assert_eq!(reply.hops, 6);
    assert!(
        agent
            .mutate::<_, Message>(
                Method::POST,
                &route,
                &Mutation::new(SendMessage {
                    target: MessageTarget::Direct(Participant::Human(bob.id)),
                    text: "cannot reset chain".into(),
                    reply_to: Some(reply.id),
                    depth: 0,
                    hops: 1
                })
            )
            .await
            .is_err()
    );
}
