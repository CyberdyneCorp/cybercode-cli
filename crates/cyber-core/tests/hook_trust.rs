//! Handler approvals share checkout trust storage without inheriting workspace approval.
use std::process::{Command, Stdio};

use cyber_core::config::HookSettings;
use cyber_core::trust::{HookInvocationTrust, TrustStore};
use serde_json::{Value, json};

fn digest(handler: Value) -> String {
    HookSettings::from_config(&json!({"hooks":{"PreToolUse":[{"hooks":[handler]}]}}))
        .unwrap()
        .events["PreToolUse"][0]
        .hooks[0]
        .digest()
        .unwrap()
}

#[test]
fn handler_digests_bind_effective_options_and_ignore_object_key_order() {
    let original = json!({"type":"command","command":"echo reviewed"});
    let approved = digest(original.clone());
    assert_eq!(
        approved,
        digest(json!({"command":"echo reviewed","type":"command","timeout":60,"async":false}))
    );
    for (field, value) in [
        ("command", json!("echo changed")),
        ("timeout", json!(61)),
        ("async", json!(true)),
        ("once", json!(true)),
        ("fail_closed", json!(true)),
        ("if", json!({"field":"tool.name","matches":"edit"})),
        ("id", json!("different")),
        ("description", json!("different")),
        ("status_message", json!("running")),
        ("system_message", json!("done")),
    ] {
        let mut changed = original.clone();
        changed[field] = value;
        assert_ne!(approved, digest(changed), "changed {field}");
    }
    assert_eq!(
        digest(
            json!({"type":"mcp_tool","server":"s","tool":"t","arguments":{"a":1,"b":{"c":2,"d":3}}})
        ),
        digest(
            json!({"tool":"t","server":"s","type":"mcp_tool","arguments":{"b":{"d":3,"c":2},"a":1}})
        )
    );
}

#[test]
fn legacy_workspace_approval_is_separate_from_individual_hook_approval() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("trust.json");
    let root = std::fs::canonicalize(dir.path()).unwrap();
    let first = digest(json!({"type":"command","command":"first"}));
    let second = digest(json!({"type":"command","command":"second"}));
    std::fs::write(&path,json!({"version":1,"approvals":[{"checkout_root":root,"digest":"workspace","approved_at":1}]}).to_string()).unwrap();
    let store = TrustStore::new(&path);
    assert!(store.is_approved(&root, "workspace").unwrap());
    assert!(!store.is_hook_approved(&root, &first).unwrap());
    store.approve_hook(&root, &first).unwrap();
    store.approve_hook(&root, &second).unwrap();
    let reopened = TrustStore::new(&path);
    assert!(reopened.is_hook_approved(&root, &first).unwrap());
    assert!(reopened.is_hook_approved(&root, &second).unwrap());
    assert!(reopened.is_approved(&root, "workspace").unwrap());
    let other = tempfile::tempdir().unwrap();
    assert!(!reopened.is_hook_approved(other.path(), &first).unwrap());
    assert!(reopened.revoke_hook(&root, &first).unwrap());
    assert!(!reopened.revoke_hook(&root, &first).unwrap());
    assert!(reopened.is_hook_approved(&root, &second).unwrap());
    assert!(reopened.is_approved(&root, "workspace").unwrap());
    assert!(reopened.revoke(&root).unwrap());
    assert!(!reopened.is_hook_approved(&root, &second).unwrap());
    assert!(!reopened.is_approved(&root, "workspace").unwrap());
}

#[test]
fn invocation_trust_binds_only_inspected_digests_and_never_persists() {
    let root = tempfile::tempdir().unwrap();
    let other = tempfile::tempdir().unwrap();
    let store = TrustStore::new(root.path().join("trust.json"));
    let first = digest(json!({"type":"command","command":"first"}));
    let changed = digest(json!({"type":"command","command":"changed"}));
    let invocation = HookInvocationTrust::new(root.path(), std::slice::from_ref(&first)).unwrap();
    assert!(invocation.is_approved(root.path(), &first).unwrap());
    assert!(!invocation.is_approved(root.path(), &changed).unwrap());
    assert!(!invocation.is_approved(other.path(), &first).unwrap());
    assert!(!store.is_hook_approved(root.path(), &first).unwrap());
    drop(invocation);
    assert!(!root.path().join("trust.json").exists());
}

#[test]
fn malformed_storage_and_invalid_digests_refuse_approval() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("trust.json");
    let store = TrustStore::new(&path);
    assert!(store.approve_hook(dir.path(), "not-a-digest").is_err());
    assert!(HookInvocationTrust::new(dir.path(), &["sha256:bad".into()]).is_err());
    assert!(!path.exists());
    std::fs::write(&path, "invalid").unwrap();
    let hash = digest(json!({"type":"command","command":"first"}));
    assert!(store.is_hook_approved(dir.path(), &hash).is_err());
    assert!(store.approve_hook(dir.path(), &hash).is_err());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "invalid");
}

#[test]
fn concurrent_processes_preserve_workspace_and_handler_approvals() {
    let dir = tempfile::tempdir().unwrap();
    let mut children = Vec::new();
    for index in 0..4 {
        children.push(
            Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "trust_writer_child"])
                .env("CYBER_TEST_HOOK_TRUST_DIRECTORY", dir.path())
                .env("CYBER_TEST_HOOK_TRUST_WRITER", index.to_string())
                .stdout(Stdio::null())
                .spawn()
                .unwrap(),
        );
    }
    for mut child in children {
        assert!(child.wait().unwrap().success());
    }
    let store = TrustStore::new(dir.path().join("trust.json"));
    for index in 0..4 {
        let root = std::fs::canonicalize(dir.path().join(index.to_string())).unwrap();
        assert!(store.is_approved(&root, "workspace").unwrap());
        for item in 0..8 {
            let hash =
                digest(json!({"type":"command","command":format!("writer {index} item {item}")}));
            assert!(store.is_hook_approved(&root, &hash).unwrap());
            assert!(store.is_mcp_approved(&root, &hash).unwrap());
        }
    }
}

#[test]
fn trust_writer_child() {
    let Some(directory) = std::env::var_os("CYBER_TEST_HOOK_TRUST_DIRECTORY") else {
        return;
    };
    let index = std::env::var("CYBER_TEST_HOOK_TRUST_WRITER").unwrap();
    let directory = std::path::PathBuf::from(directory);
    let root = directory.join(&index);
    std::fs::create_dir(&root).unwrap();
    let root = std::fs::canonicalize(root).unwrap();
    let store = TrustStore::new(directory.join("trust.json"));
    store.approve(&root, "workspace").unwrap();
    for item in 0..8 {
        let hash =
            digest(json!({"type":"command","command":format!("writer {index} item {item}")}));
        store.approve_hook(&root, &hash).unwrap();
        store.approve_mcp(&root, &hash).unwrap();
    }
}

#[cfg(unix)]
#[test]
fn handler_trust_uses_canonical_checkout_identity_and_private_storage() {
    use std::os::unix::{fs::PermissionsExt, fs::symlink};
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("checkout");
    let alias = dir.path().join("alias");
    std::fs::create_dir(&root).unwrap();
    symlink(&root, &alias).unwrap();
    let path = dir.path().join("trust.json");
    let store = TrustStore::new(&path);
    let hash = digest(json!({"type":"command","command":"first"}));
    store.approve_hook(&alias, &hash).unwrap();
    assert!(store.is_hook_approved(&root, &hash).unwrap());
    assert_eq!(
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
        0o600
    );
}
