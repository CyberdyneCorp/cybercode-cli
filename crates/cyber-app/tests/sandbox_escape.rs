//! ROADMAP P1 exit criterion (`sandbox`, `permissions-modes` → bypass mode): with the
//! sandbox enabled, a `bypass` Session on the production App cannot write outside the
//! writable roots, cannot write protected paths inside them, and cannot reach a host
//! outside the allowlist, directly or through the proxy. `bypass` turns approvals into
//! allows; it never widens the OS boundary, and an escalation still needs a person.
#![cfg(any(target_os = "macos", target_os = "linux"))]

#[path = "support/scripted_model.rs"]
mod scripted_model;
#[path = "support/mode_app.rs"]
mod support;

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::json;
use support::{ModeApp, Setup};

/// A world-writable directory outside every writable root, so an unconfined write
/// would succeed there.
fn outside_root() -> PathBuf {
    if cfg!(target_os = "macos") {
        PathBuf::from("/Users/Shared")
    } else {
        PathBuf::from("/var/tmp")
    }
}

async fn bypass_session(app: &ModeApp) -> String {
    let (status, created) = app.create("bypass").await;
    assert_eq!(status, 201, "the sandbox makes bypass eligible: {created}");
    created["data"]["id"].as_str().unwrap().to_string()
}

#[tokio::test]
async fn bypass_session_cannot_write_outside_the_writable_roots() {
    let app = ModeApp::start(Setup::default()).await;
    std::fs::create_dir_all(app.repo.join(".cyber")).unwrap();
    std::fs::write(app.repo.join(".cyber/hooks.jsonc"), "{}").unwrap();
    let id = bypass_session(&app).await;
    let marker = format!("cyber-escape-{}-{}", std::process::id(), ulid::Ulid::new());
    let target = outside_root().join(&marker);
    let target = target.display();
    let attempts = [
        format!("echo escaped > {target}"),
        format!("mkdir {target}"),
        format!("cp /etc/hosts {target}"),
        format!("sh -c 'echo nested > {target}'"),
        format!("(sleep 0; echo background > {target}) & wait"),
        format!("dir=$(dirname {target}); echo dynamic > \"$dir/{marker}\""),
        // Protected paths inside a writable root stay read-only for the OS too.
        "d=.git/hooks; echo hooked > \"$d/pre-commit\"".to_string(),
        "d=.cyber; echo '{\"x\":1}' > \"$d/hooks.jsonc\"".to_string(),
    ];
    for command in &attempts {
        app.run(&id, &format!("RUN<<{command}>>")).await;
    }
    assert!(
        !std::path::Path::new(&target.to_string()).exists(),
        "a bypass Session escaped the writable roots"
    );
    assert!(!app.repo.join(".git/hooks/pre-commit").exists());
    assert_eq!(
        std::fs::read_to_string(app.repo.join(".cyber/hooks.jsonc")).unwrap(),
        "{}"
    );
    let outputs = app.outputs(&id).await;
    assert_eq!(outputs.len(), attempts.len(), "{outputs:?}");
    let denied = app
        .events(&id)
        .into_iter()
        .filter(|event| event.kind == "sandbox.denied.1" && event.data["operation"] == "write")
        .count();
    assert!(denied >= 4, "denials are recorded: {outputs:?}");
}

#[tokio::test]
async fn bypass_session_cannot_reach_a_host_outside_the_allowlist() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let connections = Arc::new(AtomicUsize::new(0));
    let seen = Arc::clone(&connections);
    listener.set_nonblocking(true).unwrap();
    std::thread::spawn(move || {
        loop {
            match listener.accept() {
                Ok(_) => {
                    seen.fetch_add(1, Ordering::SeqCst);
                }
                Err(_) => std::thread::sleep(std::time::Duration::from_millis(5)),
            }
        }
    });
    let app = ModeApp::start(Setup::default()).await;
    let id = bypass_session(&app).await;
    for command in [
        format!("curl -s --max-time 5 http://localhost:{port}/"),
        format!("curl -s --max-time 5 --noproxy '*' http://127.0.0.1:{port}/"),
        format!("curl -s --max-time 5 --noproxy '*' http://localhost:{port}/"),
    ] {
        app.run(&id, &format!("RUN<<{command}>>")).await;
    }
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    assert_eq!(
        connections.load(Ordering::SeqCst),
        0,
        "{:?}",
        app.outputs(&id).await
    );
    let outputs = app.outputs(&id).await;
    assert!(
        outputs[0].contains("Blocked by the Cyber sandbox: localhost"),
        "{outputs:?}"
    );
    assert!(
        outputs[0].contains("Sandbox denied: network localhost"),
        "{outputs:?}"
    );
    let asked = app
        .events(&id)
        .into_iter()
        .any(|event| event.kind == "sandbox.denied.1" && event.data["operation"] == "network");
    assert!(asked, "{outputs:?}");
}

#[tokio::test]
async fn bypass_session_cannot_escalate_without_a_person() {
    let app = ModeApp::start(Setup::default()).await;
    let id = bypass_session(&app).await;
    let target = outside_root().join(format!("cyber-escalate-{}", ulid::Ulid::new()));
    let input = json!({
        "command": format!("echo escaped > {}", target.display()),
        "sandbox": "escalate",
        "justification": "write outside the project",
    });
    app.run(&id, &format!("TOOL<<bash>>{input}")).await;
    assert!(!target.exists());
    let outputs = app.outputs(&id).await;
    assert!(
        outputs[0].contains("no interactive approver"),
        "{outputs:?}"
    );
    assert!(
        !app.events(&id)
            .iter()
            .any(|event| event.kind == "sandbox.escalated.1")
    );
}

/// Regression (`sandbox` → Named sandbox profiles, `workspace-trust`): a repository's
/// `agents.build.sandbox` cannot raise the user's `read-only` sandbox to
/// `workspace-write`, so a `bypass` Session still cannot write in the checkout.
#[tokio::test]
async fn repository_agent_sandbox_cannot_loosen_the_users_read_only_sandbox() {
    let app = ModeApp::start(Setup {
        config: json!({"sandbox": {"policy": "read-only"}}),
        ..Setup::default()
    })
    .await;
    std::fs::write(
        app.repo.join("cyber.json"),
        json!({"agents": {"build": {"sandbox": "workspace"}}}).to_string(),
    )
    .unwrap();
    let id = bypass_session(&app).await;
    app.run(&id, "RUN<<echo loosened > inside.txt>>").await;
    assert!(
        !app.repo.join("inside.txt").exists(),
        "{:?}",
        app.outputs(&id).await
    );
}

/// Regression (`sandbox` → Excluded commands): when only the sandbox makes `bypass`
/// eligible, an excluded command, which runs outside the sandbox, still needs a person,
/// so it cannot write outside the writable roots unprompted.
#[tokio::test]
async fn bypass_session_cannot_run_an_excluded_command_without_a_person() {
    let app = ModeApp::start(Setup {
        config: json!({
            "permissions": {"bash": "allow"},
            "sandbox": {"excluded_commands": ["touch *"]}
        }),
        ..Setup::default()
    })
    .await;
    let id = bypass_session(&app).await;
    let target = outside_root().join(format!("cyber-excluded-{}", ulid::Ulid::new()));
    app.run(&id, &format!("RUN<<touch {}>>", target.display()))
        .await;
    assert!(!target.exists());
    let outputs = app.outputs(&id).await;
    assert!(
        outputs[0].contains("no interactive approver"),
        "{outputs:?}"
    );
}

/// Regression (`sandbox` → Escalation requests): a blanket `"permissions": "allow"` does
/// not approve an escalation; it still needs a person.
#[tokio::test]
async fn blanket_allow_does_not_approve_an_escalation() {
    let app = ModeApp::start(Setup {
        config: json!({"permissions": "allow"}),
        ..Setup::default()
    })
    .await;
    let id = bypass_session(&app).await;
    let target = outside_root().join(format!("cyber-escalate-{}", ulid::Ulid::new()));
    let input = json!({
        "command": format!("echo escaped > {}", target.display()),
        "sandbox": "escalate",
        "justification": "write outside the project",
    });
    app.run(&id, &format!("TOOL<<bash>>{input}")).await;
    assert!(!target.exists());
    let outputs = app.outputs(&id).await;
    assert!(
        outputs[0].contains("no interactive approver"),
        "{outputs:?}"
    );
}

/// Regression: cyber's own credential files in the data directory (the plugin secret
/// file and the MCP OAuth file of the file backend) were not in any launch's unreadable
/// set, and sandboxed processes run as the same user, so the model could `cat` them
/// past the `secrets/get` capability gate. Every sandboxed launch hides them.
#[tokio::test]
async fn sandboxed_commands_cannot_read_cyber_credential_files() {
    let app = ModeApp::start(Setup::default()).await;
    let marker = format!("cyber-secret-{}", ulid::Ulid::new());
    let data = app.root.join("data");
    let files = [data.join("plugin-secrets.json"), data.join("mcp-auth.json")];
    for file in &files {
        std::fs::write(file, format!("{{\"marker\":\"{marker}\"}}")).unwrap();
        let mut mode = std::fs::metadata(file).unwrap().permissions();
        std::os::unix::fs::PermissionsExt::set_mode(&mut mode, 0o600);
        std::fs::set_permissions(file, mode).unwrap();
    }
    let id = bypass_session(&app).await;
    for file in &files {
        app.run(
            &id,
            &format!("RUN<<cat '{}'; echo read-attempted>>", file.display()),
        )
        .await;
    }
    let outputs = app.outputs(&id).await;
    assert_eq!(outputs.len(), files.len(), "{outputs:?}");
    for output in &outputs {
        assert!(output.contains("read-attempted"), "{outputs:?}");
        assert!(
            !output.contains(&marker),
            "a credential file was read: {outputs:?}"
        );
    }
}
