//! Real TUI actions through a production App owner API and a separate authenticated
//! hub with signed OIDC. Linux uses temporary protected files, never native vaults.
#[path = "../../cyber-account/tests/support/oidc.rs"]
mod oidc;

use crate::{
    app::{Action, App},
    screens::{Opts, Server, render},
    screens_workflows::press,
};
use crossterm::event::KeyCode;
use cyber_collab::{
    BackendCredentials, ServeConfig,
    api::{CreateProject, Mutation},
    client::HubClient,
    model::{Project, ProjectMode},
    server::{Hub, router},
};
use serde_json::json;
use std::collections::HashMap;

async fn drive(server: &Server, app: &mut App, mut actions: Vec<Action>) {
    while !actions.is_empty() {
        let action = actions.remove(0);
        let msg = crate::perform::perform_owned(&server.client, &app.session, action, None).await;
        actions.extend(crate::runner::apply(app, msg, &mut Default::default()));
    }
}
async fn git(directory: &std::path::Path, args: &[&str]) {
    let output = tokio::process::Command::new("git")
        .arg("-C")
        .arg(directory)
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .output()
        .await
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Sign the App in through the fixture's authorization code flow.
async fn sign_in(server: &Server, fixture: &oidc::Fixture, access: &cyber_account::Secret) {
    let attempt = server
        .client
        .post("/account/login", json!({}))
        .await
        .unwrap();
    let attempt = &attempt["data"];
    let authorization =
        reqwest::Url::parse(attempt["authorization_url"].as_str().unwrap()).unwrap();
    let params: HashMap<_, _> = authorization
        .query_pairs()
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    *fixture.state.exchange.lock().unwrap() = json!({"access_token":access.expose(),"id_token":fixture.signing.sign(&fixture.id_claims(&params["nonce"])),"token_type":"Bearer"});
    server.client.post(&format!("/account/login/{}/code",attempt["attempt_id"].as_str().unwrap()),json!({"redirect_url":format!("{}?state={}&code=auth-code",params["redirect_uri"],params["state"])})).await.unwrap();
    let mut complete = false;
    for _ in 0..100 {
        let result = server
            .client
            .get(&format!(
                "/account/login/{}",
                attempt["attempt_id"].as_str().unwrap()
            ))
            .await
            .unwrap();
        if result["data"]["status"] == "complete" {
            complete = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(complete, "verified fixture login completed");
}

/// Stops the hub's listener when the fixture goes.
struct Stop(tokio::task::JoinHandle<std::io::Result<()>>);
impl Drop for Stop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// A production App signed in through a signed OIDC fixture, with a Session bound to a
/// project on a separate authenticated hub.
pub(crate) struct Bound {
    pub server: Server,
    pub session: String,
    pub project: String,
    pub client: HubClient,
    _fixture: oidc::Fixture,
    _stop: Stop,
    _directory: tempfile::TempDir,
}

impl Bound {
    pub(crate) async fn start() -> Self {
        let fixture = oidc::Fixture::start().await;
        let access = fixture.user("alice", json!([{"id":"org-fixture"}]));
        let directory = tempfile::tempdir().unwrap();
        let mut config = ServeConfig::new(
            &fixture.state.issuer,
            "hub",
            "alice",
            directory.path().join("hub.db"),
        );
        config.enabled = true;
        config.allow_loopback_http = true;
        config.introspection = Some(BackendCredentials::Token(fixture.service()));
        let hub = Hub::with_vault(&config, None).await.unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(cyber_server::http::serve_tcp(
            router(hub.clone()),
            listener,
            std::future::pending::<()>(),
        ));
        let stop = Stop(task);
        let client = HubClient::new(&url, access.clone()).unwrap();
        let project: Project = client
            .mutate(
                reqwest::Method::POST,
                "/projects",
                &Mutation::new(CreateProject {
                    name: "TUI project".into(),
                    repository: "https://github.com/fixture/project.git".into(),
                    mode: ProjectMode::Organization {
                        organization_id: "org-fixture".into(),
                    },
                }),
            )
            .await
            .unwrap()
            .data;
        let server=Server::start_config(json!({}),json!({"features":{"collaboration":true},"account":{"issuer":fixture.state.issuer,"client_id":"cyber-cli","audience":"hub","allow_loopback_http":true}}),false).await;
        git(&server.root(), &["init", "-q"]).await;
        git(
            &server.root(),
            &[
                "remote",
                "add",
                "origin",
                "https://github.com/fixture/project.git",
            ],
        )
        .await;
        sign_in(&server, &fixture, &access).await;
        let session = server.session().await;
        server.client.post("/collaboration/owner/bind",json!({"operation_id":"tui_bind_fixture","session_id":session,"project":project.id,"hub_url":url,"confirm":true})).await.unwrap();
        Self {
            server,
            session,
            project: project.id.to_string(),
            client,
            _fixture: fixture,
            _stop: stop,
            _directory: directory,
        }
    }
}

#[tokio::test]
#[cfg_attr(
    not(target_os = "linux"),
    ignore = "uses Linux temporary protected files; avoids native vault writes"
)]
async fn collab_panel_reads_and_mutates_real_hub_state_then_clears_after_logout() {
    let bound = Bound::start().await;
    let (server, session, client) = (&bound.server, bound.session.clone(), &bound.client);
    let project_id = bound.project.clone();
    let mut app = server.load(&session).await;
    app.composer.set_text("/collab");
    app.completion = None;
    let actions = press(&mut app, KeyCode::Enter);
    drive(server, &mut app, actions).await;
    let screen = render(&mut app, 140, 32, &Opts::default());
    assert!(screen.contains("[human, internal]"), "{screen}");
    assert!(screen.contains("owner"));
    for _ in 0..2 {
        press(&mut app, KeyCode::Tab);
    }
    press(&mut app, KeyCode::Char('n'));
    app.on_paste("Run shared checks");
    let actions = press(&mut app, KeyCode::Enter);
    drive(server, &mut app, actions).await;
    assert_eq!(
        app.collaboration.snapshot.as_ref().unwrap().tasks[0]["title"],
        "Run shared checks"
    );
    let actions = press(&mut app, KeyCode::Char('c'));
    drive(server, &mut app, actions).await;
    let task = &app.collaboration.snapshot.as_ref().unwrap().tasks[0];
    assert_eq!(task["claim_generation"], 1);
    assert_eq!(task["status"], "in_progress");
    let actions = press(&mut app, KeyCode::Char('d'));
    drive(server, &mut app, actions).await;
    let tasks: Vec<cyber_collab::model::Task> = client
        .get(&format!("/projects/{project_id}/tasks"))
        .await
        .unwrap();
    assert_eq!(tasks[0].status, cyber_collab::model::TaskStatus::Done);
    // Tasks -> People -> Conversation, then a human project message. No agent prompt.
    for _ in 0..2 {
        press(&mut app, KeyCode::Tab);
    }
    press(&mut app, KeyCode::Char('n'));
    app.on_paste("TUI human conversation");
    let actions = press(&mut app, KeyCode::Enter);
    drive(server, &mut app, actions).await;
    let screen = render(&mut app, 140, 32, &Opts::default());
    assert!(screen.contains("TUI human conversation"), "{screen}");
    assert!(!server.host.runtime.is_running(&session));
    server.client.delete("/account?all=true").await.unwrap();
    let actions = press(&mut app, KeyCode::Char('r'));
    drive(server, &mut app, actions).await;
    assert!(app.collaboration.snapshot.is_none());
    let screen = render(&mut app, 140, 32, &Opts::default());
    assert!(screen.contains("Unavailable"));
    assert!(!screen.contains("TUI human conversation"));
}
