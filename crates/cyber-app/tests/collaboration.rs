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
    let server = tokio::spawn(async move {
        axum::serve(listener, router(serving_hub)).await.unwrap();
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
    connector.call("bind".into(),json!({"operation_id":"cop_fixture_bind","directory":app.repo,"hub_url":url,"project":project,"confirm":true})).await.unwrap();
    let (_, created) = app.create("default").await;
    let session = created["data"]["id"].as_str().unwrap().to_owned();
    let publication=connector.call("publish".into(),json!({"operation_id":"cop_fixture_publish","session_id":session,"project":project,"name":"reviewer"})).await.unwrap();
    assert!(publication.get("capability").is_none());
    let agent = publication["agent_id"].as_str().unwrap().parse().unwrap();
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
    let inbox = tokio::time::timeout(Duration::from_secs(35), async {
        loop {
            let state = app.app.runtime.state(&session).await.unwrap();
            if let Some(row) = state
                .inbox
                .iter()
                .find(|row| row.source == "session")
                .filter(|_| state.inbox.len() == 2)
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
    assert!(
        state.entries.is_empty(),
        "held content must not start model inference"
    );
    let second = state
        .inbox
        .iter()
        .find(|row| row.message_id != inbox.message_id)
        .unwrap()
        .message_id
        .clone();
    app.app
        .runtime
        .release(&session, &inbox.message_id, InboxDelivery::Queue)
        .await
        .expect("current live publication/member proof must permit explicit receiver release");
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
