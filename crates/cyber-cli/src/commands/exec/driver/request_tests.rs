//! Two driver controllers, one actual App/Session and real model/tool requests.
#[path = "../../../../../cyber-app/tests/support/scripted_model.rs"]
mod scripted_model;
use super::*;
use crate::cli::Format;
use cyber_app::{App, AppOptions};
use cyber_core::paths::{DatabaseLocation, Paths};
use cyber_server::runtime::{PendingKind, PendingRequest};

struct Fixture {
    temp: tempfile::TempDir,
    app: App,
    client: Client,
    session: String,
    _model: scripted_model::ScriptedModel,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if std::thread::panicking() {
            self.temp.disable_cleanup(true);
            eprintln!(
                "Preserved shared-driver fixture at {}",
                self.temp.path().display()
            );
        }
    }
}
impl Fixture {
    async fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let paths = Paths {
            data: root.join("data"),
            config: root.join("config"),
            state: root.join("state"),
            cache: root.join("cache"),
            tmp: root.join("tmp"),
        };
        paths.ensure().unwrap();
        let project = root.join("project");
        std::fs::create_dir(&project).unwrap();
        let model = scripted_model::ScriptedModel::start(response);
        let mut config = model.config();
        config["permissions"] = json!({"question":"allow","bash":"ask"});
        std::fs::write(paths.config.join("cyber.jsonc"), config.to_string()).unwrap();
        let app = App::build(AppOptions {
            paths: paths.clone(),
            home: root.join("home"),
            database: DatabaseLocation::File(paths.data.join("test.db")),
            default_directory: project.clone(),
            sandbox_policy: None,
            snapshots: false,
            interactive: false,
            password: None,
            mcp_credentials: None,
            remote_skill_fetcher: None,
            workflow_cores: None,
            isolation: None,
        })
        .await
        .unwrap();
        let client = Client::embedded(app.embedded()).at(project.to_str().unwrap());
        let session = client
            .post(
                "/sessions",
                json!({"model":"test/main","mode":"default","unattended":true}),
            )
            .await
            .unwrap()["data"]["id"]
            .as_str()
            .unwrap()
            .to_owned();
        Self {
            temp,
            app,
            client,
            session,
            _model: model,
        }
    }
    async fn request(&self) -> PendingRequest {
        tokio::time::timeout(Duration::from_secs(30), async {
            loop {
                if let Some(request) = self
                    .app
                    .runtime
                    .pending_requests(Some(&self.session))
                    .into_iter()
                    .next()
                {
                    return request;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("model/tool request did not appear")
    }
}
fn response(body: &Value) -> Value {
    if !body["tools"].is_array()
        || body["messages"]
            .as_array()
            .and_then(|messages| messages.last())
            .is_some_and(|last| last["role"] == "tool")
    {
        return json!({"content":"done"});
    }
    if body["messages"]
        .as_array()
        .and_then(|messages| {
            messages
                .iter()
                .rev()
                .find(|message| message["role"] == "user")
        })
        .is_some_and(|message| message.to_string().contains("TEXT_CASE"))
    {
        return json!({"content":"owned answer"});
    }
    let question = body.to_string().contains("QUESTION_CASE");
    let (name, arguments) = if question {
        (
            "question",
            json!({"questions":[{"question":"Choose","header":"Choice","options":[{"label":"yes"},{"label":"no"}]}]}),
        )
    } else {
        ("bash", json!({"command":"echo must-not-run"}))
    };
    let id = format!("call_{}", body["messages"].as_array().unwrap().len());
    json!({"tool_calls":[{"index":0,"id":id,"type":"function","function":{"name":name,"arguments":arguments.to_string()}}]})
}
fn event(request: &PendingRequest) -> (&'static str, Value) {
    let base = if matches!(request.kind, PendingKind::Question { .. }) {
        "question.asked"
    } else {
        "permission.asked"
    };
    (base, serde_json::to_value(request).unwrap())
}
fn reply(question: bool) -> Value {
    if question {
        json!({"answers":[["yes"]]})
    } else {
        json!({"reply":"reject","message":"owner declines"})
    }
}

#[tokio::test]
async fn shared_session_requests_replies_and_close_keep_exact_driver_ownership() {
    for text in ["PERMISSION_CASE", "QUESTION_CASE"] {
        exercise(text).await;
    }
}
async fn exercise(text: &str) {
    let f = Fixture::new().await;
    let args = ExecArgs::default();
    let session = json!({"id":f.session});
    let mut first_out = Out::new(Format::Text, &args, &session);
    let mut second_out = Out::new(Format::Text, &args, &session);
    let mut first = Driver::new(
        &f.client,
        &f.session,
        &args,
        &mut first_out,
        Run::new(&args, Instant::now()),
    );
    let mut second = Driver::new(
        &f.client,
        &f.session,
        &args,
        &mut second_out,
        Run::new(&args, Instant::now()),
    );
    first.attach().await;
    second.attach().await;
    first
        .admit(
            Some("same-correlation-label".into()),
            vec![json!({"type":"text","text":text})],
            "queue",
        )
        .await;
    let request = f.request().await;
    let question = matches!(request.kind, PendingKind::Question { .. });
    assert_eq!(question, text == "QUESTION_CASE");
    second
        .admit(
            Some("same-correlation-label".into()),
            vec![json!({"type":"text","text":text})],
            "steer",
        )
        .await;
    protect_first(&f, &mut first, &mut second, &request).await;
    finish_second(&f, &mut first, &mut second).await;
    first.close().await;
    f.app.runtime.shutdown().await;
}
async fn protect_first(
    f: &Fixture,
    first: &mut Driver<'_>,
    second: &mut Driver<'_>,
    request: &PendingRequest,
) {
    let question = matches!(request.kind, PendingKind::Question { .. });
    let (base, data) = event(request);
    second.asked(base, &data).await;
    assert!(second.asks.is_empty());
    second.reply(&request.id, question, reply(question)).await;
    assert_eq!(
        f.app.runtime.pending_requests(Some(&f.session))[0].id,
        request.id
    );
    second.close().await;
    assert_eq!(
        f.app.runtime.pending_requests(Some(&f.session))[0].id,
        request.id
    );
    first.asked(base, &data).await;
    assert_eq!(first.asks.len(), 1);
    first.reply(&request.id, question, reply(question)).await;
}
async fn finish_second(f: &Fixture, first: &mut Driver<'_>, second: &mut Driver<'_>) {
    let request = f.request().await;
    let (base, data) = event(&request);
    first.asked(base, &data).await;
    assert!(first.asks.is_empty());
    second.asked(base, &data).await;
    // The closed driver declines only its own late request.
    assert!(second.asks.is_empty());
    tokio::time::timeout(Duration::from_secs(30), f.app.runtime.wait_idle(&f.session))
        .await
        .unwrap();
    assert!(f.app.runtime.pending_requests(Some(&f.session)).is_empty());
}

#[tokio::test]
async fn completed_driver_retires_without_waiting_for_another_drivers_active_question() {
    let f = Fixture::new().await;
    let args = ExecArgs::default();
    let session = json!({"id":f.session});
    let mut first_out = Out::new(Format::Text, &args, &session);
    let mut second_out = Out::new(Format::Text, &args, &session);
    let mut first = Driver::new(
        &f.client,
        &f.session,
        &args,
        &mut first_out,
        Run::new(&args, Instant::now()),
    );
    let mut second = Driver::new(
        &f.client,
        &f.session,
        &args,
        &mut second_out,
        Run::new(&args, Instant::now()),
    );
    let cursor = f
        .client
        .get(&format!("/sessions/{}", f.session))
        .await
        .unwrap()["data"]["seq"]
        .as_i64()
        .unwrap();
    first.seen_seq = Some(cursor);
    first.attach().await;
    second.attach().await;
    first
        .admit(
            Some("first".into()),
            vec![json!({"type":"text","text":"TEXT_CASE"})],
            "queue",
        )
        .await;
    tokio::time::timeout(Duration::from_secs(30), f.app.runtime.wait_idle(&f.session))
        .await
        .unwrap();
    second
        .admit(
            Some("second".into()),
            vec![json!({"type":"text","text":"QUESTION_CASE"})],
            "queue",
        )
        .await;
    let request = f.request().await;
    let first_input = first.inputs.identities()[0].clone();
    first.inputs.get_mut(&first_input).unwrap().answer_ready = true;
    assert!(
        !first.poll_background().await.unwrap(),
        "retired ahead of unread durable answer events"
    );
    assert!(first.inputs.contains(&first_input));
    let history = f
        .client
        .get(&format!(
            "/sessions/{}/history?after={cursor}&limit=500",
            f.session
        ))
        .await
        .unwrap();
    assert_eq!(history["hasMore"], false);
    for stored in history["data"].as_array().unwrap() {
        first
            .event(Event {
                kind: stored["type"].as_str().unwrap().into(),
                data: stored["data"].clone(),
                seq: stored["durable"]["seq"].as_i64(),
                session_id: Some(f.session.clone()),
            })
            .await;
    }
    assert!(!first.idle, "the other driver is still executing");
    assert!(first.poll_background().await.unwrap());
    assert!(first.inputs.is_empty());
    first.close().await;
    assert!(
        first.done().await,
        "completed driver waited for unrelated work"
    );
    assert_eq!(
        f.app.runtime.pending_requests(Some(&f.session))[0].id,
        request.id
    );
    let (base, data) = event(&request);
    second.asked(base, &data).await;
    second.reply(&request.id, true, reply(true)).await;
    tokio::time::timeout(Duration::from_secs(30), f.app.runtime.wait_idle(&f.session))
        .await
        .unwrap();
    second.close().await;
    f.app.runtime.shutdown().await;
}

async fn replay(f: &Fixture, driver: &mut Driver<'_>) {
    let cursor = driver.seen_seq.unwrap_or(-1);
    let history = f
        .client
        .get(&format!(
            "/sessions/{}/history?after={cursor}&limit=500",
            f.session
        ))
        .await
        .unwrap();
    assert_eq!(history["hasMore"], false);
    for stored in history["data"].as_array().unwrap() {
        driver
            .event(Event {
                kind: stored["type"].as_str().unwrap().into(),
                data: stored["data"].clone(),
                seq: stored["durable"]["seq"].as_i64(),
                session_id: Some(f.session.clone()),
            })
            .await;
    }
}
async fn settle_interruption(f: &Fixture, driver: &mut Driver<'_>) {
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            replay(f, driver).await;
            driver.poll_interruption().await;
            if driver.interruptions.is_empty() || driver.background_failed {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("driver interruption did not settle");
    assert!(!driver.background_failed, "{:?}", driver.run.error);
}
#[tokio::test]
async fn queued_driver_interrupt_retires_only_its_input_while_foreign_question_stays_active() {
    let f = Fixture::new().await;
    let args = ExecArgs::default();
    let session = json!({"id":f.session});
    let mut first_out = Out::new(Format::Text, &args, &session);
    let mut second_out = Out::new(Format::Text, &args, &session);
    let mut first = Driver::new(
        &f.client,
        &f.session,
        &args,
        &mut first_out,
        Run::new(&args, Instant::now()),
    );
    let mut second = Driver::new(
        &f.client,
        &f.session,
        &args,
        &mut second_out,
        Run::new(&args, Instant::now()),
    );
    first.attach().await;
    second.attach().await;
    first
        .admit(
            Some("first".into()),
            vec![json!({"type":"text","text":"QUESTION_CASE"})],
            "queue",
        )
        .await;
    let active = f.request().await;
    second
        .admit(
            Some("second".into()),
            vec![json!({"type":"text","text":"TEXT_CASE"})],
            "queue",
        )
        .await;
    second.interrupt().await;
    assert_eq!(second.interruptions.len(), 1);
    // A repeated interrupt with no new input reuses the retained operation.
    second.interrupt().await;
    assert_eq!(second.interruptions.len(), 1);
    second.close().await;
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let history = f
                .client
                .get(&format!(
                    "/sessions/{}/history?after=-1&limit=500",
                    f.session
                ))
                .await
                .unwrap();
            if history["data"].as_array().unwrap().iter().any(|event| {
                event["type"] == "session.input.interruption.1"
                    && event["data"]["status"] == "acknowledged"
            }) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    second.poll_interruption().await;
    assert!(
        !second.inputs.is_empty(),
        "retired before consuming the acknowledged receipt revision"
    );
    assert_eq!(second.interruptions.len(), 1);
    settle_interruption(&f, &mut second).await;
    assert!(second.inputs.is_empty());
    assert_eq!(second.inputs.failure_code(), Some(1));
    assert!(second.done().await);
    assert_eq!(
        f.app.runtime.pending_requests(Some(&f.session))[0].id,
        active.id
    );
    let (base, data) = event(&active);
    first.asked(base, &data).await;
    first.reply(&active.id, true, reply(true)).await;
    f.app.runtime.wait_idle(&f.session).await;
    first.close().await;
    f.app.runtime.shutdown().await;
}
#[tokio::test]
async fn active_driver_interrupt_finishes_without_waiting_for_another_drivers_replacement_question()
{
    let f = Fixture::new().await;
    let args = ExecArgs::default();
    let session = json!({"id":f.session});
    let mut first_out = Out::new(Format::Text, &args, &session);
    let mut second_out = Out::new(Format::Text, &args, &session);
    let mut first = Driver::new(
        &f.client,
        &f.session,
        &args,
        &mut first_out,
        Run::new(&args, Instant::now()),
    );
    let mut second = Driver::new(
        &f.client,
        &f.session,
        &args,
        &mut second_out,
        Run::new(&args, Instant::now()),
    );
    first.attach().await;
    second.attach().await;
    first
        .admit(
            Some("first".into()),
            vec![json!({"type":"text","text":"QUESTION_CASE"})],
            "queue",
        )
        .await;
    let original = f.request().await;
    second
        .admit(
            Some("second".into()),
            vec![json!({"type":"text","text":"QUESTION_CASE"})],
            "queue",
        )
        .await;
    first.interrupt().await;
    first.close().await;
    settle_interruption(&f, &mut first).await;
    assert!(first.done().await);
    let replacement = f.request().await;
    assert_ne!(replacement.id, original.id);
    assert!(
        second
            .inputs
            .owns_request(&f.session, &serde_json::to_value(&replacement).unwrap())
    );
    let (base, data) = event(&replacement);
    second.asked(base, &data).await;
    assert_eq!(second.asks.len(), 1);
    second.reply(&replacement.id, true, reply(true)).await;
    f.app.runtime.wait_idle(&f.session).await;
    second.close().await;
    f.app.runtime.shutdown().await;
}
#[tokio::test]
async fn interruption_retirement_preserves_a_newer_input_and_its_reply_authority() {
    let f = Fixture::new().await;
    let args = ExecArgs::default();
    let session = json!({"id":f.session});
    let mut out = Out::new(Format::Text, &args, &session);
    let mut driver = Driver::new(
        &f.client,
        &f.session,
        &args,
        &mut out,
        Run::new(&args, Instant::now()),
    );
    driver.attach().await;
    driver
        .admit(
            Some("old".into()),
            vec![json!({"type":"text","text":"QUESTION_CASE"})],
            "queue",
        )
        .await;
    f.request().await;
    let old = driver.inputs.identities()[0].clone();
    driver.interrupt().await;
    driver
        .admit(
            Some("new".into()),
            vec![json!({"type":"text","text":"QUESTION_CASE"})],
            "queue",
        )
        .await;
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            replay(&f, &mut driver).await;
            if driver.asks.values().any(|ask| !ask.inputs.contains(&old)) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    settle_interruption(&f, &mut driver).await;
    assert!(!driver.inputs.contains(&old));
    assert_eq!(driver.inputs.identities().len(), 1);
    assert_eq!(
        driver.asks.len(),
        1,
        "old interruption erased newer reply authority"
    );
    let request = f.request().await;
    driver.reply(&request.id, true, reply(true)).await;
    f.app.runtime.wait_idle(&f.session).await;
    driver.close().await;
    f.app.runtime.shutdown().await;
}
#[tokio::test]
async fn overlapping_interrupts_keep_each_original_scope_and_release_all_pending_slots() {
    let f = Fixture::new().await;
    let args = ExecArgs::default();
    let session = json!({"id":f.session});
    let mut out = Out::new(Format::Text, &args, &session);
    let mut driver = Driver::new(
        &f.client,
        &f.session,
        &args,
        &mut out,
        Run::new(&args, Instant::now()),
    );
    driver.attach().await;
    driver
        .admit(
            Some("old".into()),
            vec![json!({"type":"text","text":"QUESTION_CASE"})],
            "queue",
        )
        .await;
    f.request().await;
    driver.interrupt().await;
    driver
        .admit(
            Some("new".into()),
            vec![json!({"type":"text","text":"QUESTION_CASE"})],
            "queue",
        )
        .await;
    driver.interrupt().await;
    assert_eq!(driver.interruptions.len(), 2);
    settle_interruption(&f, &mut driver).await;
    assert!(driver.inputs.is_empty());
    assert_eq!(driver.inputs.total(), 2);
    assert_eq!(driver.inputs.failure_code(), Some(1));
    driver.close().await;
    assert!(driver.done().await);
    f.app.runtime.shutdown().await;
}
