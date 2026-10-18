//! `notify` and `send_file` through real dispatch and the runtime (`background-tasks` →
//! Notify tool, Send file to user): limits, truncation, permissions and the durable
//! records clients act on.
mod support;

use cyber_server::runtime::{ToolHost, ToolOutcome};
use serde_json::{Value, json};
use support::flow::Flow;
use tokio_util::sync::CancellationToken;

fn flow(config: Value) -> Flow {
    let flow = Flow::new(Vec::new(), false);
    flow.f.set_config(config);
    flow
}

async fn call(flow: &Flow, session: &str, mode: &str, name: &str, input: Value) -> ToolOutcome {
    let mut inv = flow.f.invocation(mode, name, input);
    inv.session_id = session.into();
    inv.call_id = cyber_core::ids::new_id("call");
    inv.message_id = "msg_sender".into();
    flow.f.host.execute(inv, CancellationToken::new()).await
}

fn events(flow: &Flow, session: &str, kind: &str) -> Vec<Value> {
    flow.f
        .store
        .read_events(session, -1, 500)
        .unwrap()
        .events
        .into_iter()
        .filter(|event| event.kind == kind)
        .map(|event| event.data)
        .collect()
}

#[tokio::test]
async fn notify_rate_limits_to_10_per_hour_and_truncates_fields() {
    let flow = flow(json!({}));
    let session = flow.session("default").await;
    let long =
        json!({"title": format!("Migration done {}", "x".repeat(100)), "body": "y".repeat(400)});
    let first: Value = serde_json::from_str(&support::ok(
        call(&flow, &session, "default", "notify", long).await,
    ))
    .unwrap();
    let title = first["title"].as_str().unwrap();
    assert_eq!(title.chars().count(), 80);
    assert!(title.starts_with("Migration done ") && title.ends_with('…'));
    assert_eq!(first["body"].as_str().unwrap().chars().count(), 300);
    for n in 2..=10 {
        let input = json!({"title": format!("Step {n}"), "body": "line one\nline\u{1b}[2J two"});
        let output = support::ok(call(&flow, &session, "default", "notify", input).await);
        if n == 10 {
            support::golden(&flow.f, "notify", &output);
        }
    }
    let refused = support::failed(
        call(
            &flow,
            &session,
            "default",
            "notify",
            json!({"title": "one more"}),
        )
        .await,
    );
    assert_eq!(refused, "Notification limit reached (10 per hour)");

    let sent = events(&flow, &session, "session.notification.sent.1");
    assert_eq!(sent.len(), 10);
    assert_eq!(sent[0]["title"], title);
    assert_eq!(sent[9]["title"], "Step 10");
    assert_eq!(sent[9]["body"], "line one line[2J two");
    assert!(
        sent.iter()
            .all(|event| event["call_id"].as_str().unwrap().starts_with("call_"))
    );

    // Another Session has its own allowance.
    let other = flow.session("default").await;
    support::ok(call(&flow, &other, "default", "notify", json!({"title": "hi"})).await);
    let untitled = json!({"title": " \u{7}", "body": "no title"});
    let refused = support::failed(call(&flow, &other, "default", "notify", untitled).await);
    assert_eq!(refused, "A notification needs a title");
}

#[tokio::test]
async fn notify_is_offered_in_plan_mode_unless_configured() {
    let open = flow(json!({}));
    assert!(
        open.f
            .tool_names("plan", false)
            .contains(&"notify".to_string())
    );
    let session = open.session("plan").await;
    support::ok(
        call(
            &open,
            &session,
            "plan",
            "notify",
            json!({"title": "Plan ready"}),
        )
        .await,
    );

    let closed = flow(json!({"background": {"deny_notify_in_plan": true}}));
    let session = closed.session("plan").await;
    let refused = support::failed(
        call(
            &closed,
            &session,
            "plan",
            "notify",
            json!({"title": "Plan ready"}),
        )
        .await,
    );
    assert!(refused.contains("deny_notify_in_plan"), "{refused}");
    assert!(events(&closed, &session, "session.notification.sent.1").is_empty());
    support::ok(
        call(
            &closed,
            &session,
            "default",
            "notify",
            json!({"title": "Built"}),
        )
        .await,
    );

    let denied = flow(json!({"permissions": {"notify": "deny"}}));
    assert!(
        !denied
            .f
            .tool_names("default", false)
            .contains(&"notify".to_string())
    );
}

#[tokio::test]
async fn send_file_caps_25mb_and_10_files_and_records_part() {
    let flow = flow(json!({}));
    let session = flow.session("default").await;
    flow.f.write("report.pdf", "%PDF-1.7 report");
    flow.f.write("notes/summary.md", "# Summary");
    flow.f.write(".env", "TOKEN=secret");
    let big = flow.f.repo.join("big.bin");
    std::fs::File::create(&big)
        .unwrap()
        .set_len(25 * 1024 * 1024 + 1)
        .unwrap();
    std::fs::write(flow.f.repo.join("edge.bin"), vec![0_u8; 25 * 1024 * 1024]).unwrap();

    let eleven: Vec<&str> = vec!["report.pdf"; 11];
    let refused = support::failed(
        call(
            &flow,
            &session,
            "default",
            "send_file",
            json!({"paths": eleven}),
        )
        .await,
    );
    assert_eq!(refused, "Send 1 to 10 files per call, got 11");
    let refused = support::failed(
        call(
            &flow,
            &session,
            "default",
            "send_file",
            json!({"paths": ["report.pdf", "big.bin"]}),
        )
        .await,
    );
    assert!(refused.contains("files are limited to 25 MB"), "{refused}");
    // Secrets need the read permission, which asks; with nobody to answer, nothing is sent.
    let refused = support::failed(
        call(
            &flow,
            &session,
            "default",
            "send_file",
            json!({"paths": [".env"]}),
        )
        .await,
    );
    assert!(refused.contains("Denied"), "{refused}");
    assert!(events(&flow, &session, "session.files.sent.1").is_empty());

    let input = json!({"paths": ["report.pdf", "notes/summary.md", "edge.bin"], "caption": "Weekly\nreport"});
    let output = support::ok(call(&flow, &session, "default", "send_file", input).await);
    let result: Value = serde_json::from_str(&output).unwrap();
    let mut stable = result.clone();
    for file in stable["files"].as_array_mut().unwrap() {
        file["id"] = json!("<file>");
    }
    support::golden(&flow.f, "send_file", &stable.to_string());
    assert_eq!(result["caption"], "Weekly report");
    let files = result["files"].as_array().unwrap();
    assert_eq!(files.len(), 3);
    assert_eq!(files[0]["name"], "report.pdf");
    assert_eq!(files[0]["media_type"], "application/pdf");
    assert_eq!(files[2]["size"], 25 * 1024 * 1024);

    let sent = events(&flow, &session, "session.files.sent.1");
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0]["message_id"], "msg_sender");
    assert_eq!(sent[0]["caption"], "Weekly report");
    let id = sent[0]["files"][1]["id"].as_str().unwrap();
    assert_eq!(id, files[1]["id"]);
    // The delivery is a copy: later edits do not change what the user received.
    flow.f.write("notes/summary.md", "# Changed");
    let stored = flow.runtime.sent_file(&session, id).await.unwrap().unwrap();
    assert_eq!(
        std::fs::read_to_string(&stored.stored_path).unwrap(),
        "# Summary"
    );
    assert_eq!(stored.sha256.len(), 64);
    assert!(
        flow.runtime
            .sent_file(&session, "file_missing")
            .await
            .unwrap()
            .is_none()
    );
}
