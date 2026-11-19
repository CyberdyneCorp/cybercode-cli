//! The planner in the TUI (`provider-catalog` → Planner and executor models): the footer
//! names the planner while it is the active model.

use serde_json::json;

use crate::screens::{Opts, Server, render};

#[tokio::test]
async fn the_footer_names_the_planner_while_it_is_active() {
    let server = Server::start().await;
    let id = server.answered_session().await;
    let mut app = server.app(&id).await;

    app.session.model = "test/planner".into();
    app.session.model_role = Some("plan".into());
    let screen = render(&mut app, 120, 40, &Opts::default());
    assert!(screen.contains("planner: test/planner"), "{screen}");

    app.session.model = "test/main".into();
    app.session.model_role = None;
    let screen = render(&mut app, 120, 40, &Opts::default());
    assert!(
        !screen.contains("planner:"),
        "the executor is not marked: {screen}"
    );
}

#[test]
fn the_session_view_carries_the_model_role() {
    let session = crate::model::Session::parse(&json!({"id": "ses_1", "model_role": "plan"}));
    assert_eq!(session.model_role.as_deref(), Some("plan"));
    assert_eq!(
        crate::model::Session::parse(&json!({"id": "ses_1"})).model_role,
        None
    );
}
