//! Agent permission rules participate in actual built-in admission.
mod support;

use cyber_server::runtime::ToolHost;
use serde_json::json;
use support::{Fixture, failed, ok};
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn profile_deny_blocks_bypass_without_writing() {
    let f = Fixture::new();
    f.write("a.txt", "original");
    ok(f.call("bypass", "read", json!({"path":"a.txt"})).await);
    f.set_config(json!({"agents":{"build":{"permissions":[{"action":"edit","resource":"*","effect":"deny"}]}}}));
    let error = failed(
        f.call(
            "bypass",
            "write",
            json!({"path":"a.txt","content":"changed"}),
        )
        .await,
    );
    assert!(error.contains("denied"), "{error}");
    assert_eq!(f.read("a.txt"), "original");
}

#[tokio::test]
async fn profile_allow_applies_in_dont_ask() {
    let f = Fixture::new();
    f.write("a.txt", "original");
    ok(f.call("bypass", "read", json!({"path":"a.txt"})).await);
    f.set_config(json!({"agents":{"build":{"permissions":{"edit":"allow"}}}}));
    ok(f.call(
        "dont-ask",
        "write",
        json!({"path":"a.txt","content":"changed"}),
    )
    .await);
    assert_eq!(f.read("a.txt"), "changed");
}

#[test]
fn whole_action_profile_denial_removes_definitions() {
    let f = Fixture::new();
    assert!(f.tool_names("default", false).contains(&"bash".into()));
    f.set_config(json!({"agents":{"build":{"permissions":{"bash":"deny"}}}}));
    assert!(!f.tool_names("default", false).contains(&"bash".into()));
}

#[tokio::test]
async fn session_rules_follow_profile_rules_without_widening_config_ceilings() {
    let f = Fixture::new();
    f.write("a.txt", "original");
    ok(f.call("bypass", "read", json!({"path":"a.txt"})).await);
    f.set_config(json!({"agents":{"build":{"permissions":{"edit":"deny"}}}}));
    let mut inv = f.invocation(
        "dont-ask",
        "write",
        json!({"path":"a.txt","content":"changed"}),
    );
    inv.rules = json!([{ "action":"edit", "resource":"*", "effect":"allow" }]);
    ok(f.host.execute(inv.clone(), CancellationToken::new()).await);
    assert_eq!(f.read("a.txt"), "changed");
    ok(f.call("bypass", "read", json!({"path":"a.txt"})).await);
    f.set_config(
        json!({"permissions":{"edit":"deny"},"agents":{"build":{"permissions":{"edit":"allow"}}}}),
    );
    inv.input = json!({"path":"a.txt","content":"forbidden"});
    let error = failed(f.host.execute(inv, CancellationToken::new()).await);
    assert!(error.contains("denied"), "{error}");
    assert_eq!(f.read("a.txt"), "changed");
}

#[tokio::test]
async fn ordered_profile_rules_preserve_resource_exceptions() {
    let f = Fixture::new();
    f.write("public.txt", "public");
    f.write("secret.txt", "secret");
    f.set_config(json!({"agents":{"build":{"permissions":[
        {"action":"read","resource":"*","effect":"deny"},
        {"action":"read","resource":"public.txt","effect":"allow"}
    ]}}}));
    ok(f.call("bypass", "read", json!({"path":"public.txt"})).await);
    let error = failed(f.call("bypass", "read", json!({"path":"secret.txt"})).await);
    assert!(error.contains("denied"), "{error}");
}

#[tokio::test]
async fn subagent_only_profiles_receive_noninteractive_defaults() {
    let f = Fixture::new();
    f.set_config(json!({"agents":{"reviewer":{"mode":"subagent"}}}));
    let mut inv = f.invocation("bypass", "question", json!({"questions":[]}));
    inv.agent = "reviewer".into();
    let error = failed(f.host.execute(inv, CancellationToken::new()).await);
    assert!(error.contains("denied"), "{error}");
}

#[tokio::test]
async fn a_profile_named_user_does_not_get_the_explicit_shell_exemption() {
    let f = Fixture::new();
    f.write("a.txt", "original");
    f.set_config(json!({"agents":{"user":{"permissions":{"read":"deny"}}}}));
    let mut inv = f.invocation("bypass", "read", json!({"path":"a.txt"}));
    inv.agent = "user".into();
    let error = failed(f.host.execute(inv, CancellationToken::new()).await);
    assert!(error.contains("denied"), "{error}");
}

#[tokio::test]
async fn explicit_user_shell_is_independent_of_model_agent_permissions() {
    #[cfg(windows)]
    let f = {
        let bash = std::path::PathBuf::from(std::env::var_os("ProgramFiles").unwrap())
            .join("Git/bin/bash.exe");
        assert!(bash.is_file(), "native test requires installed Git Bash");
        Fixture::with_shell(&bash.display().to_string(), cyber_sandbox::find_helper())
    };
    #[cfg(not(windows))]
    let f = Fixture::new();
    let mut config = json!({"agents":{
        "build":{"permissions":{"bash":"deny"}},
        "user":{"permissions":{"bash":"deny"}}
    }});
    f.set_config(config.clone());
    if cfg!(windows) {
        let error = f
            .host
            .shell(&f.repo.display().to_string(), "ses_test", "echo user-shell")
            .await
            .unwrap_err();
        assert!(error.contains("SandboxUnavailableError"), "{error}");
        config["sandbox"] = json!({"policy":"full-access"});
        f.set_config(config);
    }
    let output = f
        .host
        .shell(&f.repo.display().to_string(), "ses_test", "echo user-shell")
        .await
        .unwrap();
    assert!(output.contains("user-shell"), "{output}");
}

fn host_with_loader(
    f: &Fixture,
    config: std::sync::Arc<cyber_tools::ConfigFn>,
) -> std::sync::Arc<cyber_tools::BuiltinHost> {
    let root = f.repo.parent().unwrap();
    cyber_tools::BuiltinHost::new(cyber_tools::HostOptions {
        store: f.store.clone(),
        tool_output_dir: root.join("tool-output"),
        allowed_dirs: vec![],
        home: root.join("home"),
        shell: "bash".into(),
        config,
        global_config_dir: root.join("home/.config/cyber"),
        env: f.env.clone(),
        models: None,
        temp_dir: root.join("tmp"),
        sandbox_policy: None,
        sandbox_helper: cyber_sandbox::find_helper(),
    })
}

#[tokio::test]
async fn policy_does_not_lose_config_denies_when_later_reads_fail() {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    };
    let f = Fixture::new();
    f.write("a.txt", "original");
    let failures = Arc::new(AtomicBool::new(false));
    let loads = Arc::new(AtomicUsize::new(0));
    let failure_state = failures.clone();
    let load_state = loads.clone();
    let host = host_with_loader(
        &f,
        Arc::new(move |_| {
            let count = load_state.fetch_add(1, Ordering::SeqCst);
            if failure_state.load(Ordering::SeqCst) && count >= 2 {
                return Err("config unavailable".into());
            }
            Ok((json!({"permissions":{"edit":"deny"}}), Default::default()))
        }),
    );
    ok(host
        .execute(
            f.invocation("bypass", "read", json!({"path":"a.txt"})),
            CancellationToken::new(),
        )
        .await);
    loads.store(0, Ordering::SeqCst);
    failures.store(true, Ordering::SeqCst);
    let error = failed(
        host.execute(
            f.invocation(
                "bypass",
                "write",
                json!({"path":"a.txt","content":"changed"}),
            ),
            CancellationToken::new(),
        )
        .await,
    );
    assert!(error.contains("denied"), "{error}");
    assert_eq!(f.read("a.txt"), "original");
    assert_eq!(loads.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn failed_policy_config_read_refuses_dispatch_before_a_new_file_is_created() {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    let f = Fixture::new();
    let loads = Arc::new(AtomicUsize::new(0));
    let host = host_with_loader(
        &f,
        Arc::new(move |_| {
            if loads.fetch_add(1, Ordering::SeqCst) == 1 {
                return Err("config unavailable".into());
            }
            Ok((json!({}), Default::default()))
        }),
    );
    let error = failed(
        host.execute(
            f.invocation(
                "bypass",
                "write",
                json!({"path":"new.txt","content":"forbidden"}),
            ),
            CancellationToken::new(),
        )
        .await,
    );
    assert!(error.contains("config unavailable"), "{error}");
    assert!(!f.repo.join("new.txt").exists());
}
