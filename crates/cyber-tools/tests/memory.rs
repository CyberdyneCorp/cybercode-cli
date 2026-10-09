//! Memory through actual host dispatch, permissions and managed storage.
mod support;
use cyber_server::runtime::{ToolHost, ToolOutcome, TurnContext};
use serde_json::{Value, json};
use support::{Fixture, failed, ok};
use tokio_util::sync::CancellationToken;

fn note(body: &str) -> String {
    format!("---\nname: coding-policy\ndescription: Coding policy\ntype: reference\n---\n{body}\n")
}
fn definitions(f: &Fixture, mode: &str) -> Vec<cyber_server::runtime::ToolDef> {
    f.host.definitions(&TurnContext {
        session_id: "ses_test".into(),
        directory: f.repo.display().to_string(),
        agent: "build".into(),
        mode: mode.into(),
        prefers_apply_patch: false,
        rules: Value::Null,
    })
}

#[tokio::test]
async fn empty_listing_is_a_golden_and_creates_no_storage() {
    let f = Fixture::new();
    let result = ok(f
        .call("default", "memory", json!({"operation":"list"}))
        .await);
    support::golden(&f, "memory", &result);
    assert!(!f.dir.path().join("memory").exists());
}

#[tokio::test]
async fn disabled_and_invalid_settings_hide_definition_and_refuse_direct_dispatch() {
    let f = Fixture::new();
    for config in [
        json!({"memory":{"enabled":false}}),
        json!({"memory":{"enabled":"yes"}}),
    ] {
        f.set_config(config);
        assert!(!f.tool_names("default", false).contains(&"memory".into()));
        failed(
            f.call(
                "bypass",
                "memory",
                json!({"operation":"write","content":note("fact")}),
            )
            .await,
        );
        assert!(!f.dir.path().join("memory").exists());
    }
    f.set_config(json!({}));
    f.env.set("CYBER_DISABLE_MEMORY", "1");
    assert!(!f.tool_names("default", false).contains(&"memory".into()));
    assert_eq!(
        failed(
            f.call("default", "memory", json!({"operation":"list"}))
                .await
        ),
        "Memory is disabled"
    );
}

#[tokio::test]
async fn readonly_and_plan_offer_reads_and_refuse_stale_mutation_calls() {
    let f = Fixture::new();
    for (mode, config) in [
        ("default", json!({"memory":{"generate":false}})),
        ("plan", json!({})),
    ] {
        f.set_config(config);
        let defs = definitions(&f, mode);
        let def = defs.iter().find(|d| d.spec.name == "memory").unwrap();
        assert_eq!(
            def.spec.input_schema["properties"]["operation"]["enum"],
            json!(["list", "read"])
        );
        assert_eq!(
            def.retry_safety,
            cyber_server::runtime::RetrySafety::ReadOnly
        );
        assert_eq!(
            failed(
                f.call(
                    mode,
                    "memory",
                    json!({"operation":"write","content":note("fact")})
                )
                .await
            ),
            "Memory is read-only"
        );
        assert!(!f.dir.path().join("memory").exists());
    }
}

#[tokio::test]
async fn permission_denial_invalid_input_and_secrets_have_no_storage_effects() {
    let f = Fixture::new();
    f.set_config(json!({"permissions":{"memory":"deny"}}));
    assert!(!f.tool_names("default", false).contains(&"memory".into()));
    failed(
        f.call(
            "bypass",
            "memory",
            json!({"operation":"write","content":note("fact")}),
        )
        .await,
    );
    assert!(!f.dir.path().join("memory").exists());
    f.set_config(json!({}));
    for input in [
        json!({"operation":"read","name":"../escape"}),
        json!({"operation":"write","scope":"../escape","content":note("fact")}),
        json!({"operation":"write","content":note("password = hunter-secret")}),
        json!({"operation":"write","name":"other","content":note("fact")}),
    ] {
        failed(f.call("default", "memory", input).await);
        assert!(!f.dir.path().join("memory").exists());
    }
}

#[tokio::test]
async fn cancelled_admission_creates_no_storage() {
    let f = Fixture::new();
    let cancel = CancellationToken::new();
    cancel.cancel();
    let result = f
        .host
        .execute(
            f.invocation(
                "default",
                "memory",
                json!({"operation":"write","content":note("fact")}),
            ),
            cancel,
        )
        .await;
    assert!(matches!(result, ToolOutcome::Aborted), "{result:?}");
    assert!(!f.dir.path().join("memory").exists());
}

#[cfg(unix)]
#[tokio::test]
async fn writes_replace_same_name_update_requires_existing_and_delete_keeps_index() {
    let f = Fixture::new();
    failed(
        f.call(
            "default",
            "memory",
            json!({"operation":"update","name":"coding-policy","content":note("first")}),
        )
        .await,
    );
    assert!(!f.dir.path().join("memory").exists());
    for operation in ["write", "write", "update"] {
        let value = ok(f
            .call(
                "default",
                "memory",
                json!({"operation":operation,"name":"coding-policy","content":note(operation)}),
            )
            .await);
        let receipt: Value = serde_json::from_str(&value).unwrap();
        assert_eq!(receipt["deleted"], false);
    }
    let read: Value = serde_json::from_str(&ok(f
        .call(
            "default",
            "memory",
            json!({"operation":"read","name":"coding-policy"}),
        )
        .await))
    .unwrap();
    assert_eq!(read["body"], "update");
    let index = std::fs::read_to_string(f.dir.path().join("memory/global/MEMORY.md")).unwrap();
    assert_eq!(index.matches("coding-policy.md").count(), 1);
    let receipt: Value = serde_json::from_str(&ok(f
        .call(
            "default",
            "memory",
            json!({"operation":"delete","name":"coding-policy"}),
        )
        .await))
    .unwrap();
    assert_eq!(receipt["deleted"], true);
    assert!(!f.dir.path().join("memory/global/coding-policy.md").exists());
    assert!(
        !std::fs::read_to_string(f.dir.path().join("memory/global/MEMORY.md"))
            .unwrap()
            .contains("coding-policy")
    );
}

#[cfg(unix)]
#[tokio::test]
async fn pending_storage_is_not_automatically_recovered_and_scope_contention_refuses() {
    let f = Fixture::new();
    let store = cyber_core::memory::MemoryStore::open(f.dir.path(), "global").unwrap();
    let mut owner = store.claim().unwrap();
    assert_eq!(
        failed(
            f.call("default", "memory", json!({"operation":"list"}))
                .await
        ),
        "Memory storage is busy"
    );
    drop(owner.prepare_write(&note("retained")).unwrap());
    drop(owner);
    assert_eq!(
        failed(
            f.call("default", "memory", json!({"operation":"list"}))
                .await
        ),
        "Memory recovery required"
    );
    assert!(
        f.dir
            .path()
            .join("memory/global/.memory-transaction/intent.json")
            .exists()
    );
    assert!(!f.dir.path().join("memory/global/coding-policy.md").exists());
}

#[cfg(unix)]
#[tokio::test]
async fn project_and_global_scopes_do_not_alias_in_a_repository() {
    let f = Fixture::new();
    assert!(
        std::process::Command::new("git")
            .args(["init", "--quiet"])
            .current_dir(&f.repo)
            .status()
            .unwrap()
            .success()
    );
    let project = cyber_core::project::identify(&f.repo).id;
    assert_ne!(project, "global");
    for (scope, body) in [("project", "project fact"), ("global", "global fact")] {
        ok(f.call(
            "default",
            "memory",
            json!({"operation":"write","scope":scope,"content":note(body)}),
        )
        .await);
        let read: Value = serde_json::from_str(&ok(f
            .call(
                "default",
                "memory",
                json!({"operation":"read","scope":scope,"name":"coding-policy"}),
            )
            .await))
        .unwrap();
        assert_eq!(read["body"], body);
    }
    assert!(
        f.dir
            .path()
            .join(format!("memory/{project}/coding-policy.md"))
            .exists()
    );
    assert!(f.dir.path().join("memory/global/coding-policy.md").exists());
}

#[cfg(not(unix))]
#[tokio::test]
async fn unsupported_mutation_refuses_before_storage_admission() {
    let f = Fixture::new();
    failed(
        f.call(
            "default",
            "memory",
            json!({"operation":"write","content":note("fact")}),
        )
        .await,
    );
    assert!(!f.dir.path().join("memory").exists());
}

#[tokio::test]
async fn configuration_revocation_while_permission_waits_refuses_effects() {
    use cyber_server::runtime::{PendingKind, PermissionReply};
    use support::flow::{Flow, call, text};
    for settings in [json!({"enabled":false}), json!({"generate":false})] {
        let flow = Flow::new(
            vec![
                call(
                    "remember",
                    "memory",
                    json!({"operation":"write","scope":"global","content":note("fact")}),
                ),
                text("done"),
            ],
            true,
        );
        flow.f.set_config(json!({"permissions":{"memory":"ask"}}));
        let session = flow.session("default").await;
        flow.prompt(&session, "remember").await;
        let pending = flow.pending(&session).await;
        let PendingKind::Permission(ask) = &pending.kind else {
            panic!("expected memory permission");
        };
        assert_eq!(ask.action, "memory");
        assert_eq!(ask.resources, vec!["global"]);
        assert!(!flow.f.dir.path().join("memory").exists());
        flow.f
            .set_config(json!({"permissions":{"memory":"ask"},"memory":settings}));
        flow.runtime
            .reply_permission(&pending.id, PermissionReply::Once)
            .await
            .unwrap();
        flow.settle(&session).await;
        assert!(
            flow.output(&session, "remember")
                .await
                .starts_with("Memory is ")
        );
        assert!(!flow.f.dir.path().join("memory").exists());
    }
}

#[cfg(unix)]
#[tokio::test]
async fn memory_read_uses_managed_output_budget() {
    let f = Fixture::new();
    f.set_config(json!({"tool_output":{"max_bytes":256}}));
    let content = note(&"durable fact. ".repeat(100));
    ok(f.call(
        "default",
        "memory",
        json!({"operation":"write","scope":"global","content":content}),
    )
    .await);
    let out = ok(f
        .call(
            "default",
            "memory",
            json!({"operation":"read","scope":"global","name":"coding-policy"}),
        )
        .await);
    assert!(out.contains("[output truncated:"));
    let files = std::fs::read_dir(f.dir.path().join("tool-output"))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(files.len(), 1);
    let complete: Value =
        serde_json::from_str(&std::fs::read_to_string(files[0].path()).unwrap()).unwrap();
    assert_eq!(complete["body"], "durable fact. ".repeat(100).trim());
}

#[cfg(unix)]
#[tokio::test]
async fn tool_refuses_symlinked_scope_without_outside_effects() {
    let f = Fixture::new();
    let outside = f.dir.path().join("outside");
    std::fs::create_dir(&outside).unwrap();
    std::fs::create_dir(f.dir.path().join("memory")).unwrap();
    std::os::unix::fs::symlink(&outside, f.dir.path().join("memory/global")).unwrap();
    failed(
        f.call(
            "default",
            "memory",
            json!({"operation":"write","scope":"global","content":note("fact")}),
        )
        .await,
    );
    assert_eq!(std::fs::read_dir(&outside).unwrap().count(), 0);
}
