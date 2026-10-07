//! Permission and question requests (`permissions-modes` → Replies and approvals,
//! Non-interactive behavior, Doom-loop detection).

mod support;

use std::time::Duration;

use cyber_server::runtime::{
    Admission, CallStatus, Delivery, PendingKind, PermissionReply, QuestionReply,
};
use support::*;

fn admit(text: &str) -> Admission {
    Admission::text(text, Delivery::Steer)
}

async fn wait_pending(
    h: &Harness,
    id: &str,
    n: usize,
) -> Vec<cyber_server::runtime::PendingRequest> {
    for _ in 0..200 {
        let pending = h.runtime.pending_requests(Some(id));
        if pending.len() >= n {
            return pending;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("no pending request appeared");
}

#[tokio::test]
async fn once_lets_the_tool_run_and_is_recorded() {
    let h = Harness::new(Setup {
        scripts: vec![(
            "test/main",
            vec![tools(&[("c1", "shell", "{}")]), text("ok")],
        )],
        ..Setup::default()
    });
    h.tools.set("shell", Behavior::Ask("npm test".into()));
    let id = h.session().await;
    h.runtime.admit(&id, admit("test it")).await.unwrap();
    let pending = wait_pending(&h, &id, 1).await;
    assert!(matches!(&pending[0].kind, PendingKind::Permission(a) if a.resources == ["npm test"]));
    assert!(pending[0].id.starts_with("per_"));
    h.runtime
        .reply_permission(&pending[0].id, PermissionReply::Once)
        .await
        .unwrap();
    h.settle(&id).await;
    assert_eq!(
        h.state(&id).await.calls["c1"].output.as_deref(),
        Some("ran npm test")
    );
    let kinds: Vec<String> = h
        .store
        .read_events(&id, -1, 500)
        .unwrap()
        .events
        .into_iter()
        .map(|e| e.kind)
        .collect();
    assert!(
        kinds.contains(&"permission.asked.1".to_string())
            && kinds.contains(&"permission.replied.1".to_string())
    );
}

#[tokio::test]
async fn reject_with_feedback_continues_and_without_feedback_halts() {
    let h = Harness::new(Setup {
        scripts: vec![(
            "test/main",
            vec![
                tools(&[("c1", "shell", "{}")]),
                text("adjusted"),
                tools(&[("c2", "shell", "{}")]),
                text("never"),
            ],
        )],
        ..Setup::default()
    });
    h.tools.set("shell", Behavior::Ask("rm -rf build".into()));
    let id = h.session().await;
    h.runtime.admit(&id, admit("clean")).await.unwrap();
    let p = wait_pending(&h, &id, 1).await;
    h.runtime
        .reply_permission(
            &p[0].id,
            PermissionReply::Reject {
                message: Some("use the existing helper".into()),
            },
        )
        .await
        .unwrap();
    h.settle(&id).await;
    assert_eq!(
        h.state(&id).await.calls["c1"].output.as_deref(),
        Some("Rejected by user: use the existing helper")
    );
    assert_eq!(
        h.models.requests("test/main").len(),
        2,
        "the Drain continued with the feedback"
    );

    h.runtime.admit(&id, admit("again")).await.unwrap();
    let p = wait_pending(&h, &id, 1).await;
    h.runtime
        .reply_permission(&p[0].id, PermissionReply::Reject { message: None })
        .await
        .unwrap();
    h.settle(&id).await;
    assert_eq!(
        h.models.requests("test/main").len(),
        3,
        "a bare reject halts the Drain"
    );
}

#[tokio::test]
async fn reject_declines_other_pending_requests_and_always_approves_covered_ones() {
    let h = Harness::new(Setup {
        scripts: vec![(
            "test/main",
            vec![
                tools(&[("c1", "ask", "{}"), ("c2", "ask", "{}")]),
                text("ok"),
            ],
        )],
        ..Setup::default()
    });
    // `ask` is read-only and not concurrency-safe in the harness, so make it parallel to get two pending at once.
    h.tools
        .defs
        .lock()
        .unwrap()
        .iter_mut()
        .find(|d| d.spec.name == "ask")
        .unwrap()
        .concurrency_safe = true;
    h.tools.set("ask", Behavior::Ask("npm run build".into()));
    let id = h.session().await;
    h.runtime.admit(&id, admit("build")).await.unwrap();
    let pending = wait_pending(&h, &id, 2).await;
    h.runtime
        .reply_permission(&pending[0].id, PermissionReply::Always)
        .await
        .unwrap();
    h.settle(&id).await;
    let state = h.state(&id).await;
    assert_eq!(state.calls["c1"].status, CallStatus::Ok);
    assert_eq!(
        state.calls["c2"].status,
        CallStatus::Ok,
        "`npm *` covers the second request"
    );
    assert!(h.runtime.pending_requests(Some(&id)).is_empty());
}

#[tokio::test]
async fn unattended_sessions_never_block() {
    let h = Harness::new(Setup {
        scripts: vec![(
            "test/main",
            vec![tools(&[("c1", "shell", "{}")]), text("done")],
        )],
        interactive: false,
        ..Setup::default()
    });
    h.tools
        .set("shell", Behavior::Ask("curl https://example.com".into()));
    let id = h.session().await;
    h.runtime.admit(&id, admit("fetch")).await.unwrap();
    h.settle(&id).await;
    assert_eq!(
        h.state(&id).await.calls["c1"].output.as_deref(),
        Some("Denied: no approver")
    );
    assert!(h.runtime.pending_requests(None).is_empty());
}

#[tokio::test]
async fn questions_are_answered_and_dismissal_halts() {
    let h = Harness::new(Setup {
        scripts: vec![(
            "test/main",
            vec![
                tools(&[("c1", "ask", "{}")]),
                text("chosen"),
                tools(&[("c2", "ask", "{}")]),
                text("never"),
            ],
        )],
        ..Setup::default()
    });
    h.tools.set("ask", Behavior::AskQuestion);
    let id = h.session().await;
    h.runtime.admit(&id, admit("pick")).await.unwrap();
    let q = wait_pending(&h, &id, 1).await;
    assert!(q[0].id.starts_with("que_"));
    h.runtime
        .answer_question(
            &q[0].id,
            QuestionReply::Answers {
                answers: vec![vec!["sqlite".into()]],
            },
        )
        .await
        .unwrap();
    h.settle(&id).await;
    assert_eq!(
        h.state(&id).await.calls["c1"].output.as_deref(),
        Some("sqlite")
    );

    h.runtime.admit(&id, admit("pick again")).await.unwrap();
    let q = wait_pending(&h, &id, 1).await;
    h.runtime
        .answer_question(&q[0].id, QuestionReply::Dismissed)
        .await
        .unwrap();
    h.settle(&id).await;
    assert_eq!(
        h.models.requests("test/main").len(),
        3,
        "dismissal halts the Drain"
    );
}

#[tokio::test]
async fn identical_calls_raise_a_doom_loop_request() {
    let h = Harness::new(Setup {
        scripts: vec![(
            "test/main",
            vec![
                tools(&[("c1", "read", "{\"path\":\"a\"}")]),
                tools(&[("c2", "read", "{\"path\":\"a\"}")]),
                tools(&[("c3", "read", "{\"path\":\"a\"}")]),
                text("stopped"),
            ],
        )],
        ..Setup::default()
    });
    let id = h.session().await;
    h.runtime.admit(&id, admit("read")).await.unwrap();
    let p = wait_pending(&h, &id, 1).await;
    assert!(
        matches!(&p[0].kind, PendingKind::Permission(a) if a.action == "doom_loop" && a.resources == ["read"])
    );
    h.runtime
        .reply_permission(
            &p[0].id,
            PermissionReply::Reject {
                message: Some("stop repeating".into()),
            },
        )
        .await
        .unwrap();
    h.settle(&id).await;
    assert_eq!(
        h.state(&id).await.calls["c3"].output.as_deref(),
        Some("Rejected by user: stop repeating")
    );
    assert_eq!(
        h.tools.executed_names().len(),
        2,
        "the third identical call never ran"
    );
}

#[tokio::test]
async fn doom_loop_in_dont_ask_mode_halts() {
    let h = Harness::new(Setup {
        scripts: vec![(
            "test/main",
            vec![
                tools(&[("c1", "read", "{}")]),
                tools(&[("c2", "read", "{}")]),
                tools(&[("c3", "read", "{}")]),
                text("never"),
            ],
        )],
        ..Setup::default()
    });
    let id = h.session().await;
    h.runtime.switch_mode(&id, "dont-ask").await.unwrap();
    h.runtime.admit(&id, admit("read")).await.unwrap();
    h.settle(&id).await;
    assert_eq!(
        h.state(&id).await.calls["c3"].output.as_deref(),
        Some("Repeated identical tool calls detected")
    );
    assert_eq!(h.models.requests("test/main").len(), 3);
}

#[tokio::test]
async fn always_approval_does_not_cascade_into_confirmation_only_requests() {
    let h = Harness::new(Setup {
        scripts: vec![(
            "test/main",
            vec![
                tools(&[("c1", "ask", "{}"), ("c2", "ask", "{}")]),
                text("done"),
            ],
        )],
        ..Setup::default()
    });
    h.tools
        .defs
        .lock()
        .unwrap()
        .iter_mut()
        .find(|d| d.spec.name == "ask")
        .unwrap()
        .concurrency_safe = true;
    h.tools
        .set("ask", Behavior::AskConfirm("rm -rf /repo".into()));
    let id = h.session().await;
    h.runtime
        .admit(&id, admit("review removals"))
        .await
        .unwrap();
    let requests = wait_pending(&h, &id, 2).await;
    h.runtime
        .reply_permission(&requests[0].id, PermissionReply::Always)
        .await
        .unwrap();
    let remaining = h.runtime.pending_requests(Some(&id));
    assert_eq!(
        remaining.len(),
        1,
        "the second critical action still needs its own approval"
    );
    h.runtime
        .reply_permission(
            &remaining[0].id,
            PermissionReply::Reject {
                message: Some("Keep the root".into()),
            },
        )
        .await
        .unwrap();
    h.settle(&id).await;
    assert_eq!(
        h.state(&id)
            .await
            .calls
            .values()
            .filter(|c| c.status == CallStatus::Ok)
            .count(),
        1
    );
}

async fn child_session(h: &Harness, parent: &str) -> String {
    h.runtime
        .create_session(cyber_server::runtime::CreateSession {
            directory: h.repo.display().to_string(),
            model: "test/main".into(),
            parent_id: Some(parent.into()),
            title: Some("Review changes (@general)".into()),
            ..Default::default()
        })
        .await
        .unwrap()
        .id
}

#[tokio::test]
async fn ancestors_see_child_requests_but_unrelated_sessions_do_not() {
    let h = Harness::new(Setup {
        scripts: vec![(
            "test/main",
            vec![tools(&[("c1", "shell", "{}")]), text("done")],
        )],
        ..Setup::default()
    });
    h.tools.set("shell", Behavior::Ask("npm test".into()));
    let parent = h.session().await;
    let unrelated = h.session().await;
    let child = child_session(&h, &parent).await;
    let grandchild = child_session(&h, &child).await;
    h.runtime.admit(&grandchild, admit("test")).await.unwrap();
    let pending = wait_pending(&h, &grandchild, 1).await;
    assert_eq!(h.runtime.pending_requests(Some(&parent)), pending);
    assert_eq!(h.runtime.pending_requests(Some(&child)), pending);
    assert!(h.runtime.pending_requests(Some(&unrelated)).is_empty());
    assert_eq!(pending[0].session_id, grandchild);
    h.runtime
        .reply_permission(&pending[0].id, PermissionReply::Once)
        .await
        .unwrap();
    h.settle(&grandchild).await;
    assert!(h.runtime.pending_requests(Some(&parent)).is_empty());
    for id in [&parent, &child] {
        assert!(
            !h.store
                .read_events(id, -1, 500)
                .unwrap()
                .events
                .iter()
                .any(|e| e.kind.starts_with("permission."))
        );
    }
    assert!(
        h.store
            .read_events(&grandchild, -1, 500)
            .unwrap()
            .events
            .iter()
            .any(|e| e.kind == "permission.replied.1")
    );
}

#[tokio::test]
async fn parent_rejection_does_not_decline_a_child_request() {
    let h = Harness::new(Setup {
        scripts: vec![(
            "test/main",
            vec![
                tools(&[("p1", "shell", "{}")]),
                tools(&[("c1", "shell", "{}")]),
                text("child done"),
            ],
        )],
        ..Setup::default()
    });
    h.tools.set("shell", Behavior::Ask("npm test".into()));
    let parent = h.session().await;
    let child = child_session(&h, &parent).await;
    h.runtime
        .admit(&parent, admit("test parent"))
        .await
        .unwrap();
    let parent_request = wait_pending(&h, &parent, 1).await.remove(0);
    h.runtime.admit(&child, admit("test child")).await.unwrap();
    let child_request = wait_pending(&h, &child, 1).await.remove(0);
    assert_eq!(h.runtime.pending_requests(Some(&parent)).len(), 2);
    h.runtime
        .reply_permission(
            &parent_request.id,
            PermissionReply::Reject { message: None },
        )
        .await
        .unwrap();
    h.settle(&parent).await;
    assert_eq!(
        h.runtime.pending_requests(Some(&child)),
        vec![child_request.clone()]
    );
    h.runtime
        .reply_permission(&child_request.id, PermissionReply::Once)
        .await
        .unwrap();
    h.settle(&child).await;
    assert_eq!(h.state(&child).await.calls["c1"].status, CallStatus::Ok);
}

#[tokio::test]
async fn parent_always_does_not_preapprove_a_child_request() {
    let h = Harness::new(Setup {
        scripts: vec![(
            "test/main",
            vec![
                tools(&[("p1", "shell", "{}")]),
                tools(&[("c1", "shell", "{}")]),
                text("parent done"),
                text("child done"),
            ],
        )],
        ..Setup::default()
    });
    h.tools.set("shell", Behavior::Ask("npm test".into()));
    let parent = h.session().await;
    let child = child_session(&h, &parent).await;
    h.runtime
        .admit(&parent, admit("test parent"))
        .await
        .unwrap();
    let parent_request = wait_pending(&h, &parent, 1).await.remove(0);
    h.runtime.admit(&child, admit("test child")).await.unwrap();
    let child_request = wait_pending(&h, &child, 1).await.remove(0);
    assert_eq!(h.runtime.pending_requests(Some(&parent)).len(), 2);
    h.runtime
        .reply_permission(&parent_request.id, PermissionReply::Always)
        .await
        .unwrap();
    h.settle(&parent).await;
    assert_eq!(
        h.runtime.pending_requests(Some(&child)),
        vec![child_request.clone()]
    );
    h.runtime
        .reply_permission(&child_request.id, PermissionReply::Reject { message: None })
        .await
        .unwrap();
    h.settle(&child).await;
}

#[tokio::test]
async fn child_rejection_preserves_parent_and_sibling_requests() {
    let h = Harness::new(Setup {
        scripts: vec![(
            "test/main",
            vec![
                tools(&[("p1", "shell", "{}")]),
                tools(&[("c1", "shell", "{}")]),
                tools(&[("s1", "shell", "{}")]),
                text("parent done"),
                text("sibling done"),
            ],
        )],
        ..Setup::default()
    });
    h.tools.set("shell", Behavior::Ask("npm test".into()));
    let parent = h.session().await;
    let child = child_session(&h, &parent).await;
    let sibling = child_session(&h, &parent).await;
    h.runtime
        .admit(&parent, admit("test parent"))
        .await
        .unwrap();
    let parent_request = wait_pending(&h, &parent, 1).await.remove(0);
    h.runtime.admit(&child, admit("test child")).await.unwrap();
    let child_request = wait_pending(&h, &child, 1).await.remove(0);
    h.runtime
        .admit(&sibling, admit("test sibling"))
        .await
        .unwrap();
    let sibling_request = wait_pending(&h, &sibling, 1).await.remove(0);
    h.runtime
        .reply_permission(&child_request.id, PermissionReply::Reject { message: None })
        .await
        .unwrap();
    h.settle(&child).await;
    let remaining = h.runtime.pending_requests(Some(&parent));
    assert_eq!(remaining.len(), 2);
    assert!(remaining.contains(&parent_request));
    assert!(remaining.contains(&sibling_request));
    h.runtime
        .reply_permission(&parent_request.id, PermissionReply::Once)
        .await
        .unwrap();
    h.settle(&parent).await;
    h.runtime
        .reply_permission(&sibling_request.id, PermissionReply::Once)
        .await
        .unwrap();
    h.settle(&sibling).await;
    assert_eq!(h.state(&parent).await.calls["p1"].status, CallStatus::Ok);
    assert_eq!(h.state(&sibling).await.calls["s1"].status, CallStatus::Ok);
}

#[tokio::test]
async fn ancestors_can_list_and_answer_child_questions() {
    let h = Harness::new(Setup {
        scripts: vec![(
            "test/main",
            vec![tools(&[("q1", "ask", "{}")]), text("done")],
        )],
        ..Setup::default()
    });
    h.tools.set("ask", Behavior::AskQuestion);
    let parent = h.session().await;
    let child = child_session(&h, &parent).await;
    h.runtime.admit(&child, admit("choose")).await.unwrap();
    let pending = wait_pending(&h, &child, 1).await;
    assert_eq!(h.runtime.pending_requests(Some(&parent)), pending);
    h.runtime
        .answer_question(
            &pending[0].id,
            QuestionReply::Answers {
                answers: vec![vec!["sqlite".into()]],
            },
        )
        .await
        .unwrap();
    h.settle(&child).await;
    assert_eq!(
        h.state(&child).await.calls["q1"].output.as_deref(),
        Some("sqlite")
    );
    assert!(h.runtime.pending_requests(Some(&parent)).is_empty());
}
