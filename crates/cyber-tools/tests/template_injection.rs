//! `skills-commands` → Shell output injection: permission before execution, sandboxed
//! concurrent runs, the per-command timeout, the stdout cap and credential redaction.
mod support;

use std::time::{Duration, Instant};

use cyber_llm::Content;
use cyber_server::runtime::{Asker, PromptTemplate, TemplateInjection, ToolHost, TurnContext};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

fn turn(f: &support::Fixture, mode: &str) -> TurnContext {
    TurnContext {
        session_id: "ses_test".into(),
        directory: f.repo.display().to_string(),
        agent: "build".into(),
        mode: mode.into(),
        prefers_apply_patch: false,
        rules: Value::Null,
    }
}

/// Plan `template` as a global command would be planned.
fn planned(template: &str) -> (PromptTemplate, Vec<Content>) {
    let expanded =
        cyber_core::skills::expand_template(template, "", cyber_core::skills::Append::Raw);
    let plan = PromptTemplate {
        kind: "command".into(),
        name: "t".into(),
        project: false,
        digest: cyber_core::commands::template_digest("command", "t", template),
        injections: expanded
            .injections
            .into_iter()
            .map(|i| TemplateInjection {
                start: i.start,
                end: i.end,
                command: i.command,
            })
            .collect(),
    };
    (
        plan,
        vec![Content::Text {
            text: expanded.text,
        }],
    )
}

async fn expand(f: &support::Fixture, mode: &str, template: &str) -> String {
    let (plan, parts) = planned(template);
    let parts = f
        .host
        .expand_prompt_template(
            &turn(f, mode),
            &plan,
            &parts,
            Asker::detached(),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    match &parts[0] {
        Content::Text { text } => text.clone(),
        other => panic!("unexpected part {other:?}"),
    }
}

/// Windows has no sandbox route, so these run under explicit full access with an
/// explicitly selected Git Bash: a bare `bash` resolves to the WSL launcher in System32.
fn fixture(permissions: Value) -> support::Fixture {
    #[cfg(windows)]
    let f = {
        let bash = std::path::PathBuf::from(std::env::var_os("ProgramFiles").unwrap())
            .join("Git/bin/bash.exe");
        assert!(bash.is_file(), "native test requires installed Git Bash");
        support::Fixture::with_policy(
            &bash.display().to_string(),
            cyber_sandbox::find_helper(),
            Some("full-access".into()),
        )
    };
    #[cfg(not(windows))]
    let f = support::Fixture::native_git();
    f.set_config(json!({ "permissions": permissions }));
    f
}

#[tokio::test]
async fn denied_injection_is_never_run_and_is_substituted_visibly() {
    let f = fixture(json!({"bash":{"*":"allow","cat *":"deny","touch *":"deny"}}));
    let text = expand(
        &f,
        "default",
        "Creds: !`cat ~/.aws/credentials`\nMark: !`touch ran`\nOk: !`echo hi`",
    )
    .await;
    assert_eq!(
        text,
        "Creds: [denied: cat ~/.aws/credentials]\nMark: [denied: touch ran]\nOk: hi"
    );
    assert!(!f.repo.join("ran").exists());
}

#[tokio::test]
async fn plan_mode_and_unanswered_asks_deny_injections() {
    let f = fixture(json!({"bash":{"*":"allow"}}));
    assert_eq!(expand(&f, "plan", "!`echo hi`").await, "[denied: echo hi]");
    let f = fixture(json!({"bash":{"*":"ask"}}));
    assert_eq!(
        expand(&f, "dont-ask", "!`echo hi`").await,
        "[denied: echo hi]"
    );
    // No client is attached, so the ask resolves unattended and nothing runs.
    assert_eq!(
        expand(&f, "default", "!`touch asked`").await,
        "[denied: touch asked]"
    );
    assert!(!f.repo.join("asked").exists());
}

#[tokio::test]
async fn output_is_capped_redacted_and_bounded_by_the_timeout() {
    let f = fixture(json!({"bash":{"*":"allow"}}));
    let capped = expand(&f, "default", "!`head -c 30000 /dev/zero | tr '\\0' x`").await;
    assert!(capped.ends_with("[output truncated at 20 KiB]"));
    assert!(capped.len() <= 20 * 1024 + 40);
    let redacted = expand(&f, "default", "!`echo aws_secret_access_key=abc123`").await;
    assert!(!redacted.contains("abc123"), "{redacted}");
    f.host.set_injection_timeout(Duration::from_millis(300));
    let started = Instant::now();
    let timed_out = expand(&f, "default", "!`sleep 5`").await;
    assert!(started.elapsed() < Duration::from_secs(4));
    assert!(timed_out.contains("timed out after 300 ms"), "{timed_out}");
}

#[tokio::test]
async fn allowed_injections_run_concurrently() {
    let f = fixture(json!({"bash":{"*":"allow"}}));
    let started = Instant::now();
    // Two 2 s commands finish in about one interval, not two.
    let text = expand(
        &f,
        "default",
        "A !`sleep 2 && echo a` B !`sleep 2 && echo b`",
    )
    .await;
    assert_eq!(text, "A a B b");
    assert!(
        started.elapsed() < Duration::from_millis(3500),
        "{:?}",
        started.elapsed()
    );
}

#[tokio::test]
async fn untrusted_project_templates_run_nothing() {
    let f = fixture(json!({"bash":{"*":"allow"}}));
    let (mut plan, parts) = planned("!`touch project-ran`");
    plan.project = true;
    let parts = f
        .host
        .expand_prompt_template(
            &turn(&f, "default"),
            &plan,
            &parts,
            Asker::detached(),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(
        parts,
        [Content::Text {
            text: "[untrusted: touch project-ran]".into()
        }]
    );
    assert!(!f.repo.join("project-ran").exists());
}
