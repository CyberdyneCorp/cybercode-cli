//! `/model <provider/model> <prompt>` (`tui` → Model picker) against a production App and
//! server: the prompt is answered by the named model and the Session keeps its own.

use std::time::Duration;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use serde_json::json;

use crate::app::{Action, App};
use crate::screens::Server;

fn typed(app: &mut App, text: &str) -> Vec<Action> {
    for c in text.chars() {
        app.on_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
    }
    app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
}

/// The provider model of every Turn request (one with tools, unlike title generation)
/// that carried `text`.
fn models_answering(server: &Server, text: &str) -> Vec<String> {
    server
        .model
        .requests()
        .iter()
        .filter(|request| {
            request["tools"]
                .as_array()
                .is_some_and(|tools| !tools.is_empty())
        })
        .filter(|request| request["messages"].to_string().contains(text))
        .map(|request| request["model"].as_str().unwrap_or_default().to_owned())
        .collect()
}

async fn wait_for(server: &Server, id: &str, text: &str) -> Vec<String> {
    for _ in 0..1000 {
        let models = models_answering(server, text);
        if !models.is_empty() && !server.host.runtime.is_running(id) {
            return models;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("no request for {text:?}: {:?}", server.model.requests());
}

#[test]
fn model_command_keeps_the_picker_and_parses_a_one_shot_prompt() {
    let mut app = App::new(crate::model::Session::default(), Vec::new(), "cyber");
    assert_eq!(typed(&mut app, "/model"), [Action::LoadModels]);
    assert_eq!(typed(&mut app, "/model test/other"), [Action::LoadModels]);
    assert!(typed(&mut app, "/model other explain").is_empty());
    assert_eq!(
        app.toast.as_ref().unwrap().0,
        "Usage: /model <provider/model> <prompt>"
    );
    assert_eq!(
        typed(&mut app, "/model test/other  explain fib "),
        [Action::ModelPrompt {
            model: "test/other".into(),
            text: "explain fib".into(),
            delivery: "steer",
        }]
    );
    app.session.running = true;
    assert!(matches!(
        typed(&mut app, "/model test/other again").as_slice(),
        [Action::ModelPrompt {
            delivery: "queue",
            ..
        }]
    ));
}

#[tokio::test]
async fn one_shot_model_prompt_then_the_session_model_answers_again() {
    let server = Server::start_models(json!({"main": {}, "other": {}}), json!({}), false).await;
    let id = server.session().await;
    let mut app = server.load(&id).await;
    let actions = typed(&mut app, "/model test/other explain fib");
    for action in actions {
        let msg = crate::perform::perform(&server.client, &app.session, action).await;
        crate::runner::apply(&mut app, msg, &mut Default::default());
    }
    assert_eq!(wait_for(&server, &id, "explain fib").await, ["other"]);
    let session = server.client.get(&format!("/sessions/{id}")).await.unwrap();
    assert_eq!(session["data"]["model"], "test/main");

    server.prompt(&id, "and now").await;
    assert_eq!(wait_for(&server, &id, "and now").await, ["main"]);
}
