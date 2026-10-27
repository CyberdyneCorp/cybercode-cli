//! Marked auto-fix Sessions use only native file tools, with immutable host ceilings.
mod support;
use cyber_core::autofix::{Config, Subscription};
use cyber_server::runtime::{ClientHook, ClientHooks, ToolOutcome};
use futures::future::BoxFuture;
use serde_json::{Value, json};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use support::Fixture;
use tokio_util::sync::CancellationToken;
fn mark(f: &Fixture) {
    let subscription = Subscription::new(
        "afx_guard".into(),
        f.repo.display().to_string(),
        Config {
            repository: "team/repo".into(),
            pr: 3,
            session_id: "ses_owner".into(),
            model: "test/main".into(),
            checks: vec![vec!["cargo".into(), "test".into()]],
            required_ci: vec!["check:9:test".into()],
            verification_paths: vec!["tests".into()],
            max_attempts: 3,
            max_tokens: 1000,
            max_seconds: 60,
        },
        42,
        "fix".into(),
        0,
    )
    .unwrap();
    cyber_server::autofix::records::Repository::new(f.store.clone())
        .create(subscription)
        .unwrap();
    let directory = f.repo.display().to_string();
    f.store.transaction(move|tx|{tx.execute("INSERT INTO autofix_session(session_id,subscription_id,attempt_id,directory) VALUES('ses_test','afx_guard','attempt_guard',?1)",[directory])?;Ok(())}).unwrap();
}
fn refused(outcome: ToolOutcome) {
    assert!(
        matches!(outcome, ToolOutcome::Failed(_)),
        "unexpected outcome: {outcome:?}"
    )
}
#[tokio::test]
async fn ordinary_files_work_but_relative_absolute_policy_and_verification_writes_are_refused() {
    let f = Fixture::new();
    mark(&f);
    assert!(matches!(
        f.call(
            "bypass",
            "write",
            json!({"path":"src/fix.rs","content":"fixed"})
        )
        .await,
        ToolOutcome::Ok(_)
    ));
    assert_eq!(f.read("src/fix.rs"), "fixed");
    for path in [
        ".git/evil",
        ".github/workflows/new.yml",
        ".cyber/hooks/new",
        "cyber.json",
        "nested/AGENTS.md",
        "tests/new.rs",
    ] {
        refused(
            f.call("bypass", "write", json!({"path":path,"content":"unsafe"}))
                .await,
        );
        assert!(!f.repo.join(path).exists(), "wrote protected {path}");
    }
    let absolute = f.repo.join(".git/absolute");
    refused(
        f.call(
            "bypass",
            "write",
            json!({"path":absolute,"content":"unsafe"}),
        )
        .await,
    );
    refused(f.call("bypass","apply_patch",json!({"patch":"*** Begin Patch\n*** Add File: .github/workflows/evil.yml\n+unsafe\n*** End Patch"})).await);
}
#[tokio::test]
async fn external_paths_and_rebound_location_are_refused() {
    use cyber_server::runtime::ToolHost;
    let f = Fixture::new();
    mark(&f);
    let outside = f.dir.path().join("outside.txt");
    std::fs::write(&outside, "owner-secret").unwrap();
    refused(f.call("bypass", "read", json!({"path":outside})).await);
    let target = f.dir.path().join("created.txt");
    refused(
        f.call("bypass", "write", json!({"path":target,"content":"unsafe"}))
            .await,
    );
    assert!(!target.exists());
    let mut invocation = f.invocation(
        "bypass",
        "write",
        json!({"path":"rebound.txt","content":"unsafe"}),
    );
    invocation.directory = f.dir.path().display().to_string();
    refused(f.host.execute(invocation, CancellationToken::new()).await);
    assert!(!f.dir.path().join("rebound.txt").exists());
}
#[cfg(unix)]
#[tokio::test]
async fn symlink_aliases_cannot_expose_git_or_escape_or_change_verification_sources() {
    let f = Fixture::new();
    mark(&f);
    f.write(".git/config", "secret");
    f.write("tests/existing.rs", "verify");
    std::os::unix::fs::symlink(f.repo.join(".git"), f.repo.join("metadata")).unwrap();
    std::os::unix::fs::symlink(f.repo.join("tests"), f.repo.join("verification")).unwrap();
    std::os::unix::fs::symlink(f.dir.path(), f.repo.join("outside")).unwrap();
    refused(
        f.call("bypass", "read", json!({"path":"metadata/config"}))
            .await,
    );
    refused(
        f.call(
            "bypass",
            "write",
            json!({"path":"metadata/new","content":"unsafe"}),
        )
        .await,
    );
    refused(
        f.call(
            "bypass",
            "write",
            json!({"path":"verification/new.rs","content":"unsafe"}),
        )
        .await,
    );
    refused(
        f.call(
            "bypass",
            "write",
            json!({"path":"outside/new","content":"unsafe"}),
        )
        .await,
    );
    assert_eq!(f.read(".git/config"), "secret");
    assert_eq!(f.read("tests/existing.rs"), "verify");
    assert!(!f.dir.path().join("new").exists());
}
struct Hooks(AtomicUsize);
impl ClientHooks for Hooks {
    fn client_hooks(&self, _: &[String]) -> Vec<ClientHook> {
        self.0.fetch_add(1, Ordering::SeqCst);
        vec![]
    }
    fn execute_client_hook<'a>(
        &'a self,
        _: &'a str,
        _: Value,
        _: std::time::Duration,
        _: CancellationToken,
    ) -> BoxFuture<'a, Result<Value, String>> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Box::pin(async { Ok(json!({})) })
    }
}
#[tokio::test]
async fn pre_post_denial_and_lifecycle_callbacks_never_dispatch_for_marked_sessions() {
    use cyber_server::runtime::ToolHost;
    let f = Fixture::new();
    mark(&f);
    let hooks = Arc::new(Hooks(AtomicUsize::new(0)));
    f.host.attach_client_hooks(hooks.clone()).unwrap();
    f.global_hooks(json!({}));
    assert!(matches!(
        f.call(
            "bypass",
            "write",
            json!({"path":"safe.txt","content":"safe"})
        )
        .await,
        ToolOutcome::Ok(_)
    ));
    f.set_config(json!({"permissions":{"edit":"deny"}}));
    refused(
        f.call(
            "bypass",
            "write",
            json!({"path":"denied.txt","content":"unsafe"}),
        )
        .await,
    );
    refused(
        f.call("bypass", "bash", json!({"command":"touch forbidden"}))
            .await,
    );
    refused(
        f.call("bypass", "webfetch", json!({"url":"https://example.com"}))
            .await,
    );
    let _ = f
        .host
        .lifecycle_hooks(
            cyber_server::runtime::LifecycleHook {
                session_id: "ses_test".into(),
                directory: f.repo.display().to_string(),
                agent: "build".into(),
                mode: "bypass".into(),
                kind: "SessionStart".into(),
                subject: "startup".into(),
                fields: Default::default(),
                workflow_run_id: None,
            },
            CancellationToken::new(),
        )
        .await;
    assert_eq!(hooks.0.load(Ordering::SeqCst), 0);
    assert!(!f.repo.join("forbidden").exists());
}

#[cfg(unix)]
#[tokio::test]
async fn automatic_formatters_never_run_after_autofix_file_edits() {
    let f = Fixture::new();
    mark(&f);
    let marker = f.dir.path().join("formatter-ran");
    f.set_config(json!({"formatters":{"guard":{"command":["sh","-c",format!("touch '{}'",marker.display())],"extensions":[".custom"]}}}));
    assert!(matches!(
        f.call(
            "bypass",
            "write",
            json!({"path":"safe.custom","content":"safe"})
        )
        .await,
        ToolOutcome::Ok(_)
    ));
    assert!(!marker.exists());
    assert_eq!(f.read("safe.custom"), "safe");
    assert!(
        f.tool_names("bypass", false)
            .iter()
            .all(|name| cyber_tools::autofix_allowed_tool(name))
    );
}
