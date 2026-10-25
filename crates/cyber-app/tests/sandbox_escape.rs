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
/// file, the MCP OAuth file, the peer token file of the file backend and the ChatGPT and
/// GitHub Copilot logins) and the peer
/// listener's private key in `<state>/peer-tls/` were not in any launch's unreadable set,
/// and sandboxed processes run as the same user, so the model could `cat` them past the
/// `secrets/get` capability gate. Every sandboxed launch hides them.
#[tokio::test]
async fn sandboxed_commands_cannot_read_cyber_credential_files() {
    let app = ModeApp::start(Setup::default()).await;
    let marker = format!("cyber-secret-{}", ulid::Ulid::new());
    let data = app.root.join("data");
    let tls = app.root.join("state").join("peer-tls");
    std::fs::create_dir_all(&tls).unwrap();
    let files = [
        data.join("plugin-secrets.json"),
        data.join("mcp-auth.json"),
        data.join("peers-secrets.json"),
        // Regression: the subscription logins (the GitHub OAuth token behind Copilot
        // does not rotate) were readable by sandboxed commands.
        data.join("chatgpt-auth.json"),
        data.join("copilot-auth.json"),
        tls.join("key.pem"),
    ];
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

#[tokio::test]
async fn sandboxed_commands_cannot_read_another_sessions_private_computer_artifacts() {
    use sha2::{Digest, Sha256};
    for sandbox_policy in [None, Some("read-only")] {
        let app = ModeApp::start(Setup {
            sandbox_policy,
            ..Setup::default()
        })
        .await;
        let private = app.root.join("data/computer");
        assert!(
            private.is_dir(),
            "bootstrap must prepare the hidden root before subprocesses launch"
        );
        let victim = bypass_session(&app).await;
        let attacker = bypass_session(&app).await;
        assert_ne!(victim, attacker);
        let folder = private.join(format!("{:x}", Sha256::digest(victim.as_bytes())));
        std::fs::create_dir_all(&folder).unwrap();
        let capture = folder.join(format!("{}.png", cyber_core::ids::new_id("cuf")));
        let marker = format!("computer-private-{}", ulid::Ulid::new());
        std::fs::write(&capture, &marker).unwrap();
        let public = app.repo.join("ordinary-file.txt");
        std::fs::write(&public, "public-canary-readable").unwrap();
        // An explicit readable_paths grant still cannot expose a registered
        // private root, including files created after the App was bootstrapped.
        let config = app.root.join("config/cyber.json");
        let mut value: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&config).unwrap()).unwrap();
        value["sandbox"]["readable_paths"] = json!([private]);
        std::fs::write(config, value.to_string()).unwrap();
        app.run(
            &attacker,
            &format!(
                "RUN<<cat '{}'; echo private-read-attempted; cat '{}'>>",
                capture.display(),
                public.display()
            ),
        )
        .await;
        let outputs = app.outputs(&attacker).await;
        assert_eq!(outputs.len(), 1, "{outputs:?}");
        assert!(
            outputs[0].contains("private-read-attempted"),
            "the real subprocess did not run: {outputs:?}"
        );
        assert!(
            outputs[0].contains("public-canary-readable"),
            "ordinary reads should remain available: {outputs:?}"
        );
        assert!(
            !outputs[0].contains(&marker),
            "cross-Session private capture escaped the OS sandbox ({sandbox_policy:?}): {outputs:?}"
        );
        assert_eq!(
            std::fs::read_to_string(&capture).unwrap(),
            marker,
            "read denial must not destroy the owner's capture"
        );
    }
}

#[tokio::test]
async fn sandboxed_commands_cannot_read_owner_password_after_rotation() {
    for sandbox_policy in [None, Some("read-only")] {
        let app = ModeApp::start(Setup {
            sandbox_policy,
            ..Setup::default()
        })
        .await;
        let state = app.root.join("state");
        let password = state.join("password");
        let marker = format!("owner-password-{}", ulid::Ulid::new());
        let replacement = state.join("password-replacement");
        std::fs::write(&replacement, &marker).unwrap();
        std::fs::rename(&replacement, &password).unwrap();
        let session = bypass_session(&app).await;
        let public = app.repo.join("public-owner-control.txt");
        std::fs::write(&public, "OWNER_PUBLIC_CONTROL").unwrap();
        app.run(
            &session,
            &format!(
                "RUN<<cat '{}'; echo owner-read-attempted; cat '{}'>>",
                password.display(),
                public.display()
            ),
        )
        .await;
        let outputs = app.outputs(&session).await;
        assert_eq!(outputs.len(), 1);
        assert!(outputs[0].contains("owner-read-attempted"));
        assert!(outputs[0].contains("OWNER_PUBLIC_CONTROL"));
        assert!(
            !outputs[0].contains(&marker),
            "private owner credential escaped sandbox"
        );
        assert_eq!(std::fs::read_to_string(password).unwrap(), marker);
    }
}

#[cfg(target_os = "macos")]
#[tokio::test]
async fn unreadable_owner_state_cannot_be_overwritten_or_moved_from_a_writable_root() {
    // This fixture deliberately grants a root containing its synthetic home.
    // Override only bypass eligibility; the command still runs in the explicit
    // workspace-write OS sandbox whose private-path enforcement is under test.
    let app = ModeApp::start(Setup {
        sandbox_policy: Some("workspace-write"),
        isolation: cyber_tools::IsolationOptions {
            dangerously_bypass: true,
            container: false,
        },
        ..Setup::default()
    })
    .await;
    let state = app.root.join("state");
    let data = app.root.join("data");
    assert!(
        data.join("computer").is_dir(),
        "the data ancestor must contain a registered hidden capture directory"
    );
    let canary = state.join("password");
    let marker = format!("private-state-canary-{}", ulid::Ulid::new());
    std::fs::write(&canary, &marker).unwrap();
    let config = app.root.join("config/cyber.json");
    let mut value: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&config).unwrap()).unwrap();
    value["sandbox"]["writable_roots"] = json!([app.root]);
    std::fs::write(config, value.to_string()).unwrap();
    let session = bypass_session(&app).await;
    let control = app.root.join("ordinary-writable-control.txt");
    let attempts = [
        format!("printf replaced > '{}'", canary.display()),
        format!("rm '{}'", canary.display()),
        format!(
            "mv '{}' '{}'",
            canary.display(),
            app.repo.join("moved-password").display()
        ),
        format!(
            "ln '{}' '{}'",
            canary.display(),
            app.repo.join("linked-password").display()
        ),
        format!(
            "mv '{}' '{}'",
            state.display(),
            app.root.join("moved-state").display()
        ),
        format!(
            "mv '{}' '{}'",
            data.display(),
            app.root.join("moved-data").display()
        ),
    ];
    for (index, command) in attempts.iter().enumerate() {
        app.run(
            &session,
            &format!(
                "RUN<<{command}; printf public-control > '{}'; echo write-attempted-{index}>>",
                control.display()
            ),
        )
        .await;
        assert_eq!(
            std::fs::read_to_string(&canary).unwrap(),
            marker,
            "private owner canary changed after {command}"
        );
        assert!(
            state.is_dir() && data.is_dir(),
            "a hidden root or its ancestor moved"
        );
        assert_eq!(
            std::fs::read_to_string(&control).unwrap(),
            "public-control",
            "ordinary writes must remain available"
        );
    }
    assert!(!app.repo.join("moved-password").exists());
    assert!(!app.repo.join("linked-password").exists());
    assert!(!app.root.join("moved-state").exists());
    assert!(!app.root.join("moved-data").exists());
    let outputs = app.outputs(&session).await;
    assert_eq!(outputs.len(), attempts.len());
    for output in outputs {
        assert!(
            output.contains("write-attempted-"),
            "real sandboxed subprocess did not run: {output}"
        );
        assert!(!output.contains(&marker));
    }
}
