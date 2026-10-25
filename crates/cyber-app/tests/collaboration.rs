//! Production owner API defaults: opening a checkout never enrolls its owner/agents.
#[path = "support/mode_app.rs"]
mod mode_app;
#[path = "support/scripted_model.rs"]
mod scripted_model;
use serde_json::json;

#[tokio::test]
async fn disabled_collaboration_is_local_only_and_rejects_owner_mutations() {
    let app = mode_app::ModeApp::start(Default::default()).await;
    let (status, body) = app
        .send(
            "GET",
            "/collaboration/status?session_id=ses_nonexistent",
            None,
        )
        .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["data"]["enabled"], false);
    assert!(body["data"]["account"].is_null());
    assert!(body["data"]["published"].is_null());
    let (status,body)=app.send("POST","/collaboration/owner/bind",Some(json!({"operation_id":"cop_disabled","hub_url":"https://127.0.0.1:9","project":"cprj_disabled","confirm":true}))).await;
    assert_eq!(status, 503, "{body}");
    assert_eq!(body["_tag"], "ServiceUnavailableError");
    assert_eq!(body["service"], "collaboration");
    let publications = app
        .app
        .store
        .read(|db| {
            Ok(
                db.query_row("SELECT COUNT(*) FROM collaboration_publication", [], |r| {
                    r.get::<_, i64>(0)
                })?,
            )
        })
        .unwrap();
    let bindings = app
        .app
        .store
        .read(|db| {
            Ok(
                db.query_row("SELECT COUNT(*) FROM collaboration_binding", [], |r| {
                    r.get::<_, i64>(0)
                })?,
            )
        })
        .unwrap();
    assert_eq!((publications, bindings), (0, 0));
}

#[path = "../../cyber-account/tests/support/oidc.rs"]
mod oidc;

#[tokio::test]
async fn real_hub_publication_delivers_once_held_and_logout_blocks_release() {
    use cyber_account::{AccountSettings, AccountStore, HostSecrets};
    use cyber_app::{account::Accounts, collaboration::Collaboration};
    use cyber_collab::{
        api::*,
        client::HubClient,
        model::*,
        server::{Hub, router},
    };
    use cyber_server::{
        http::{account::AccountService, collaboration::CollaborationService},
        runtime::{Delivery as InboxDelivery, InputStatus},
    };
    use std::{sync::Arc, time::Duration};
    let fixture = oidc::Fixture::start().await;
    let app = mode_app::ModeApp::start(Default::default()).await;
    let remote = "https://github.com/acme/collaboration-fixture.git";
    assert!(
        std::process::Command::new("git")
            .args(["init", "--quiet"])
            .arg(&app.repo)
            .status()
            .unwrap()
            .success()
    );
    assert!(
        std::process::Command::new("git")
            .arg("-C")
            .arg(&app.repo)
            .args(["remote", "add", "origin", remote])
            .status()
            .unwrap()
            .success()
    );
    let mut oidc = fixture.client().await;
    let tokens = fixture.bundle(&mut oidc).await;
    let vault = Arc::new(oidc::MemoryVault::default());
    let account_store = AccountStore::with_vault(app.app.paths.data.clone(), Some(vault.clone()));
    account_store.save(tokens).await.unwrap();
    let settings=AccountSettings::from_global(&json!({"account":{"issuer":fixture.state.issuer,"client_id":"cyber-cli","audience":"hub","allow_loopback_http":true}}),None).unwrap();
    let accounts = Arc::new(Accounts::new(
        account_store,
        app.app.paths.cache.clone(),
        Arc::new(move || Ok(settings.clone())),
    ));
    let mut config = cyber_collab::ServeConfig::new(
        &fixture.state.issuer,
        "hub",
        "alice",
        app.root.join("hub.db"),
    );
    config.enabled = true;
    config.allow_loopback_http = true;
    config.introspection = Some(cyber_collab::BackendCredentials::Token(fixture.service()));
    let hub = Hub::with_vault(&config, Some(vault.clone())).await.unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let serving_hub = hub.clone();
    let historical_ack_path = Arc::new(std::sync::Mutex::new(None::<String>));
    let historical_ack_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let observed_path = historical_ack_path.clone();
    let observed_count = historical_ack_count.clone();
    let monitored = router(serving_hub).layer(axum::middleware::from_fn(
        move |request: axum::extract::Request, next: axum::middleware::Next| {
            let path = observed_path.clone();
            let count = observed_count.clone();
            async move {
                let observed = path
                    .lock()
                    .unwrap()
                    .as_ref()
                    .is_some_and(|path| request.uri().path().ends_with(path));
                if observed {
                    count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                }
                next.run(request).await
            }
        },
    ));
    let server = tokio::spawn(async move {
        axum::serve(listener, monitored).await.unwrap();
    });
    let human = HubClient::new(&url, fixture.access()).unwrap();
    let created: MutationResult<Project> = human
        .mutate(
            reqwest::Method::POST,
            "/projects",
            &Mutation::new(CreateProject {
                name: "fixture".into(),
                repository: remote.into(),
                mode: ProjectMode::Organization {
                    organization_id: "org-uuid".into(),
                },
            }),
        )
        .await
        .unwrap();
    let project_revision = created.data.revision;
    let project = created.data.id;
    let secrets = HostSecrets::new(
        app.app.paths.data.clone(),
        "fixture-collaboration.json",
        "fixture",
        Some(vault),
    )
    .unwrap();
    let connector = Collaboration::with_secrets(
        app.app.store.clone(),
        accounts.clone(),
        app.app.runtime.clone(),
        app.repo.clone(),
        Arc::new(|| Ok(json!({"features":{"collaboration":true}}))),
        secrets,
    )
    .unwrap();
    let listed = connector
        .call("project.list".into(), json!({"hub_url":url}))
        .await
        .unwrap();
    assert_eq!(listed.as_array().unwrap().len(), 1);
    assert_eq!(listed[0]["id"], project.to_string());
    let shown = connector
        .call(
            "project.show".into(),
            json!({"hub_url":url,"project":project}),
        )
        .await
        .unwrap();
    assert_eq!(shown["id"], project.to_string());
    assert!(
        connector.local.bindings().unwrap().is_empty(),
        "project discovery must not implicitly bind a checkout"
    );
    assert!(
        connector.local.publications().unwrap().is_empty(),
        "read-only discovery must not publish agents"
    );
    // A mismatched public issuer is rejected before a user token or project read
    // is sent to that host, even when an explicit URL was supplied for discovery.
    let wrong_token = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let wrong_projects = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let observed_token = wrong_token.clone();
    let observed_projects = wrong_projects.clone();
    let wrong_hub_id = cyber_collab::ids::HubId::new();
    let wrong_router=axum::Router::new().route("/api/v1/hub/hello",axum::routing::get(move|headers:axum::http::HeaderMap|{
        let token=observed_token.clone();let hub=wrong_hub_id.clone();
        async move {if headers.get("authorization").is_some_and(|value|value.to_str().unwrap_or_default().trim()!="Bearer"){token.store(true,std::sync::atomic::Ordering::SeqCst);}axum::Json(json!({"data":{"hub_id":hub,"issuer":"https://unrelated.example","audience":"hub"}}))}
    })).route("/api/v1/hub/projects",axum::routing::get(move||{let count=observed_projects.clone();async move{count.fetch_add(1,std::sync::atomic::Ordering::SeqCst);axum::Json(json!({"data":[]}))}}));
    let wrong_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let wrong_url = format!("http://{}", wrong_listener.local_addr().unwrap());
    let wrong_server = tokio::spawn(async move {
        axum::serve(wrong_listener, wrong_router).await.unwrap();
    });
    let rejected = connector
        .call("project.list".into(), json!({"hub_url":wrong_url}))
        .await
        .unwrap_err();
    assert_eq!(rejected.body.tag, "IssuerMismatchError");
    assert!(!wrong_token.load(std::sync::atomic::Ordering::SeqCst));
    assert_eq!(wrong_projects.load(std::sync::atomic::Ordering::SeqCst), 0);
    wrong_server.abort();
    connector.call("bind".into(),json!({"operation_id":"cop_fixture_bind","directory":app.repo,"hub_url":url,"project":project,"confirm":true})).await.unwrap();
    let mismatch = connector.call("messages.send".into(),json!({"project":project,"operation_id":"cop_wrong_actor","expected_actor":{"issuer":fixture.state.issuer,"subject":"another-user"},"target":{"kind":"project"},"text":"must not dispatch"})).await.unwrap_err();
    assert_eq!(mismatch.status, reqwest::StatusCode::CONFLICT);
    assert_eq!(mismatch.body.tag, "AccountChangedError");
    let (_, created) = app.create("default").await;
    let session = created["data"]["id"].as_str().unwrap().to_owned();
    let publication=connector.call("publish".into(),json!({"operation_id":"cop_fixture_publish","session_id":session,"project":project,"name":"reviewer"})).await.unwrap();
    assert!(publication.get("capability").is_none());
    let agent: cyber_collab::ids::AgentId =
        publication["agent_id"].as_str().unwrap().parse().unwrap();
    // An independently authenticated sender joins through the normal bound-invite
    // flow, then loses write membership. Its old held deliveries must not starve
    // a later authorized sender, even when that backlog spans multiple pages.
    let bob = HubClient::new(&url, fixture.user("bob", json!([{"id":"org-uuid"}]))).unwrap();
    let invite: MutationResult<IssuedInvitation> = human
        .mutate(
            reqwest::Method::POST,
            &format!("/projects/{project}/invitations"),
            &Mutation::new(Invite {
                role: Role::Member,
                subject: Some("bob".into()),
                guest: false,
            }),
        )
        .await
        .unwrap();
    let joined: MutationResult<JoinOutcome> = bob
        .mutate(
            reqwest::Method::POST,
            "/join",
            &Mutation::new(Join {
                invitation_token: invite.data.token,
                project: None,
                name: Some("Bob".into()),
            }),
        )
        .await
        .unwrap();
    let application = joined.data.application.unwrap();
    let joined: MutationResult<JoinOutcome> = bob
        .mutate(
            reqwest::Method::POST,
            &format!("/applications/{}/redeem", application.id),
            &Mutation::new(Redeem {
                repository: cyber_collab::repository::Repository::parse(remote).unwrap(),
                code: None,
            }),
        )
        .await
        .unwrap();
    let bob_member = joined.data.member.unwrap();
    let stale: MutationResult<Message> = bob
        .mutate(
            reqwest::Method::POST,
            &format!("/projects/{project}/messages"),
            &Mutation::new(SendMessage {
                target: MessageTarget::Direct(Participant::Agent(agent.clone())),
                text: "withdrawn source content must stay out of the inbox".into(),
                reply_to: None,
                depth: 0,
                hops: 1,
            }),
        )
        .await
        .unwrap();
    *historical_ack_path.lock().unwrap() =
        Some(format!("/deliveries/{}/{agent}/ack", stale.data.id));
    // Receive and explicitly release one valid Bob message before withdrawal. Its
    // later ACK reports an authorized historical observation, not fresh execution.
    let expired: MutationResult<Message> = bob
        .mutate(
            reqwest::Method::POST,
            &format!("/projects/{project}/messages"),
            &Mutation::new(SendMessage {
                target: MessageTarget::Direct(Participant::Agent(agent.clone())),
                text: "authorized before the remote delivery expired".into(),
                reply_to: None,
                depth: 0,
                hops: 1,
            }),
        )
        .await
        .unwrap();
    let mut expired_inbox = None;
    for message in [&stale.data, &expired.data] {
        let hash =
            cyber_collab::storage::hash(cyber_collab::storage::encode(message).unwrap().as_bytes());
        let inbox = connector
            .local
            .delivery(
                publication["hub_id"].as_str().unwrap(),
                &project.to_string(),
                message.id.as_str(),
                agent.as_str(),
                &hash,
            )
            .unwrap();
        app.app
            .runtime
            .messaging()
            .receive_collaboration(
                &session,
                &inbox,
                cyber_server::runtime::messaging::CollaborationOrigin {
                    hub_id: publication["hub_id"].as_str().unwrap().into(),
                    project_id: project.to_string(),
                    member_id: bob_member.id.to_string(),
                    agent_id: None,
                    member_generation: bob_member.generation,
                    membership_kind: "internal".into(),
                    remote_message_id: message.id.to_string(),
                    target_agent_id: agent.to_string(),
                    depth: 0,
                    hops: 1,
                },
                message.text.clone(),
            )
            .await
            .unwrap();
        app.app
            .runtime
            .release(&session, &inbox, InboxDelivery::Queue)
            .await
            .unwrap();
        if message.id == expired.data.id {
            expired_inbox = Some(inbox);
        }
    }
    let expired_inbox = expired_inbox.unwrap();
    let expired_id = format!("{}:{agent}", expired.data.id);
    let expired_project = project.to_string();
    hub.db
        .write(move |db| {
            let mut delivery: Delivery = cyber_collab::storage::get(db, "delivery", &expired_id)?;
            delivery.state = DeliveryState::Expired;
            cyber_collab::storage::put(db, "delivery", &expired_id, &expired_project, &delivery)
        })
        .unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        while app.app.runtime.is_running(&session) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let authorized_entries = app.app.runtime.state(&session).await.unwrap().entries.len();
    let _: MutationResult<Member> = human
        .mutate(
            reqwest::Method::PATCH,
            &format!("/projects/{project}/members/{}", bob_member.id),
            &Mutation::new(SetRole { role: Role::Viewer }),
        )
        .await
        .unwrap();
    let stale_message = stale.data;
    let target = agent.clone();
    let project_id = project.to_string();
    hub.db
        .write(move |db| {
            use cyber_collab::storage::{encode, hash, put};
            let timestamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis() as u64
                - 2000;
            for index in 0..105 {
                let mut message = stale_message.clone();
                message.id = format!("cmsg_{}", ulid::Ulid::from_parts(timestamp + index, 0))
                    .parse()
                    .unwrap();
                let delivery = Delivery {
                    content_hash: hash(encode(&message)?.as_bytes()),
                    message: message.clone(),
                    target: target.clone(),
                    state: DeliveryState::Held,
                };
                put(db, "message", message.id.as_str(), &project_id, &message)?;
                put(
                    db,
                    "delivery",
                    &format!("{}:{target}", message.id),
                    &project_id,
                    &delivery,
                )?;
            }
            Ok(())
        })
        .unwrap();
    let mutation = Mutation::new(SendMessage {
        target: MessageTarget::Direct(Participant::Agent(agent)),
        text: "Remote untrusted fixture content".into(),
        reply_to: None,
        depth: 0,
        hops: 1,
    });
    let path = format!("/projects/{project}/messages");
    let first: MutationResult<Message> = human
        .mutate(reqwest::Method::POST, &path, &mutation)
        .await
        .unwrap();
    let replay: MutationResult<Message> = human
        .mutate(reqwest::Method::POST, &path, &mutation)
        .await
        .unwrap();
    assert_eq!(first.data.id, replay.data.id);
    assert!(replay.replayed);
    let _: MutationResult<Message> = human
        .mutate(
            reqwest::Method::POST,
            &path,
            &Mutation::new(SendMessage {
                target: mutation.payload.target.clone(),
                text: "Second held fixture message".into(),
                reply_to: None,
                depth: 0,
                hops: 1,
            }),
        )
        .await
        .unwrap();
    // Force an expired persisted cursor. The authorized resync snapshot contains a
    // project conversation entry which must never become executable agent input.
    let _: MutationResult<Message> = human
        .mutate(
            reqwest::Method::POST,
            &path,
            &Mutation::new(SendMessage {
                target: MessageTarget::Project,
                text: "Snapshot only: RUN<<this must never execute>>".into(),
                reply_to: None,
                depth: 0,
                hops: 1,
            }),
        )
        .await
        .unwrap();
    let members: Vec<Member> = human
        .get(&format!("/projects/{project}/members"))
        .await
        .unwrap();
    let member = members
        .iter()
        .find(|member| member.subject == "alice")
        .unwrap();
    let scope = cyber_app::collaboration::local::CursorScope {
        checkout: std::fs::canonicalize(&app.repo)
            .unwrap()
            .display()
            .to_string(),
        identity: serde_json::to_string(&(&fixture.state.issuer, "alice")).unwrap(),
        hub: publication["hub_id"].as_str().unwrap().into(),
        project: project.to_string(),
        member: member.id.to_string(),
        generation: member.generation,
        project_revision,
    };
    connector.local.checkpoint(scope.clone(), 1).unwrap();
    let project_id = project.to_string();
    let snapshot_cursor = hub
        .db
        .write(move |db| {
            let cursor: i64 = db.query_row(
                "SELECT MAX(cursor) FROM ch_events WHERE project=?1",
                [&project_id],
                |row| row.get(0),
            )?;
            db.execute(
                "DELETE FROM ch_events WHERE project=?1 AND cursor<?2",
                (&project_id, cursor),
            )?;
            Ok(cursor as u64)
        })
        .unwrap();
    assert!(snapshot_cursor > 2);
    let inbox = tokio::time::timeout(Duration::from_secs(55), async {
        loop {
            let state = app.app.runtime.state(&session).await.unwrap();
            if let Some(row) = state
                .inbox
                .iter()
                .find(|row| row.source == "session" && row.status == InputStatus::Held)
                .filter(|_| {
                    state
                        .inbox
                        .iter()
                        .filter(|row| row.status == InputStatus::Held)
                        .count()
                        == 2
                })
            {
                break row.clone();
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("targeted delivery must reach the normal held inbox");
    assert_eq!(inbox.status, InputStatus::Held);
    assert!(
        connector.local.cursor(scope).unwrap() >= snapshot_cursor,
        "expired cursor must checkpoint the authorized snapshot without replaying its content"
    );
    let state = app.app.runtime.state(&session).await.unwrap();
    assert_eq!(
        state
            .inbox
            .iter()
            .filter(|row| row.message_id == inbox.message_id)
            .count(),
        1
    );
    assert_eq!(
        state.inbox.len(),
        4,
        "withdrawn source backlog must never enter the local inbox"
    );
    assert_eq!(
        state.entries.len(),
        authorized_entries,
        "only the explicitly approved Bob messages may start model inference"
    );
    assert!(
        connector
            .local
            .delivery_outcomes(
                publication["hub_id"].as_str().unwrap().into(),
                project.to_string(),
                publication["agent_id"].as_str().unwrap().into()
            )
            .unwrap()
            .is_empty(),
        "historical release ACK must persist locally and stop retransmission after source withdrawal"
    );
    assert_eq!(
        historical_ack_count.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "successful historical terminal ACK must not be posted again on the next heartbeat"
    );
    let expired_receipt=app.app.store.read(move|db|Ok(db.query_row("SELECT receipt FROM collaboration_delivery_ack WHERE inbox=?1 AND outcome='released'",[expired_inbox],|row|row.get::<_,String>(0))?)).unwrap();
    let expired_receipt: serde_json::Value = serde_json::from_str(&expired_receipt).unwrap();
    assert_eq!(expired_receipt["status"], "reconciled");
    assert_eq!(expired_receipt["reason"], "DeliveryTerminalConflictError");
    let second = state
        .inbox
        .iter()
        .find(|row| row.message_id != inbox.message_id && row.status == InputStatus::Held)
        .unwrap()
        .message_id
        .clone();
    app.app
        .runtime
        .release(&session, &inbox.message_id, InboxDelivery::Queue)
        .await
        .expect("current live publication/member proof must permit explicit receiver release");
    // An unattempted offline intent from the old generation must not be sent under
    // a same-subject rejoin that happened entirely while this client was disconnected.
    let prior: ContributionAuthorization = human
        .get(&format!("/projects/{project}/contributions/authorization"))
        .await
        .unwrap();
    let owner_member = prior.member.clone();
    hub.db
        .write(move |db| {
            let mut member: Member =
                cyber_collab::storage::get(db, "member", owner_member.as_str())?;
            member.generation += 2;
            cyber_collab::storage::put(
                db,
                "member",
                member.id.as_str(),
                member.project.as_str(),
                &member,
            )
        })
        .unwrap();
    connector.local.enqueue("cop_old_generation".into(),serde_json::to_string(&(&fixture.state.issuer,"alice")).unwrap(),publication["hub_id"].as_str().unwrap().into(),project.to_string(),json!({"method":"POST","path":format!("/projects/{project}/messages"),"mutation":{"operation_id":"cop_old_generation","payload":{"target":{"kind":"project"},"text":"STALE_GENERATION_MUST_NOT_SEND","reply_to":null,"depth":0,"hops":1}},"authorization":prior})).unwrap();
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let receipts = connector
                .local
                .receipts(
                    publication["hub_id"].as_str().unwrap().into(),
                    project.to_string(),
                )
                .unwrap();
            if receipts.iter().any(|receipt| {
                receipt["operation_id"] == "cop_old_generation" && receipt["state"] == "rejected"
            }) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("old generation intent must receive a local rejection receipt");
    let visible: Vec<Message> = human
        .get(&format!("/projects/{project}/messages"))
        .await
        .unwrap();
    assert!(
        !visible
            .iter()
            .any(|message| message.text == "STALE_GENERATION_MUST_NOT_SEND")
    );
    accounts.logout(false).await.unwrap();
    let released = app
        .app
        .runtime
        .release(&session, &second, InboxDelivery::Steer)
        .await;
    assert!(
        released.is_err(),
        "logout must immediately revoke release authority"
    );
    assert_eq!(
        app.app
            .runtime
            .state(&session)
            .await
            .unwrap()
            .inbox
            .iter()
            .find(|row| row.message_id == second)
            .unwrap()
            .status,
        InputStatus::Held
    );
    server.abort();
}
