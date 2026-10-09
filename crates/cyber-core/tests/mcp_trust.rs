use cyber_core::config::McpSettings;
use cyber_core::trust::TrustStore;
use serde_json::json;

fn digest(server: serde_json::Value, name: &str) -> String {
    McpSettings::from_config(&json!({"mcp":{name:server}}))
        .unwrap()
        .servers[name]
        .digest(name)
        .unwrap()
}

#[test]
fn effective_digest_binds_name_defaults_credentials_and_execution_fields() {
    let original = json!({"type":"local","command":"server"});
    let first = digest(original.clone(), "audit");
    assert_eq!(
        first,
        digest(
            json!({"type":"local","command":"server","args":[],"env":{},"enabled":true,"tools":{}}),
            "audit"
        )
    );
    assert_ne!(first, digest(original.clone(), "other"));
    let mut explicit_default = original.clone();
    explicit_default["timeout"] = json!(30);
    assert_ne!(first, digest(explicit_default, "audit"));
    for (key, value) in [
        ("command", json!("changed")),
        ("args", json!(["--changed"])),
        ("env", json!({"TOKEN":"changed"})),
        ("cwd", json!("/other")),
        ("timeout", json!(31)),
        ("enabled", json!(false)),
        ("tools", json!({"deny":["*"]})),
    ] {
        let mut changed = original.clone();
        changed[key] = value;
        assert_ne!(first, digest(changed, "audit"));
    }
    let remote =
        json!({"type":"remote","url":"https://example.test","headers":{"authorization":"secret"}});
    let first = digest(remote.clone(), "remote");
    let mut changed = remote;
    changed["headers"]["authorization"] = json!("new secret");
    assert_ne!(first, digest(changed, "remote"));
}

#[test]
fn legacy_workspace_and_hook_approvals_do_not_grant_server_authority() {
    let root = tempfile::tempdir().unwrap();
    let canonical = std::fs::canonicalize(root.path()).unwrap();
    let path = root.path().join("trust.json");
    std::fs::write(&path,json!({"version":1,"approvals":[{"checkout_root":canonical,"digest":"workspace","approved_at":1}]}).to_string()).unwrap();
    let store = TrustStore::new(&path);
    let hash = digest(json!({"type":"local","command":"server"}), "audit");
    store.approve_hook(root.path(), &hash).unwrap();
    assert!(!store.is_mcp_approved(root.path(), &hash).unwrap());
    store.approve_mcp(root.path(), &hash).unwrap();
    assert!(
        TrustStore::new(&path)
            .is_mcp_approved(root.path(), &hash)
            .unwrap()
    );
    let other = tempfile::tempdir().unwrap();
    assert!(!store.is_mcp_approved(other.path(), &hash).unwrap());
    assert!(store.revoke_mcp(root.path(), &hash).unwrap());
    assert!(store.is_hook_approved(root.path(), &hash).unwrap());
    assert!(store.is_approved(&canonical, "workspace").unwrap());
    store.approve_mcp(root.path(), &hash).unwrap();
    assert!(store.revoke(&canonical).unwrap());
    assert!(!store.is_mcp_approved(root.path(), &hash).unwrap());
    assert!(!store.is_hook_approved(root.path(), &hash).unwrap());
}

#[test]
fn malformed_storage_and_server_digests_fail_closed() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("trust.json");
    let store = TrustStore::new(&path);
    assert!(store.approve_mcp(root.path(), "bad").is_err());
    assert!(!path.exists());
    std::fs::write(&path, "invalid").unwrap();
    let hash = digest(json!({"type":"local","command":"server"}), "audit");
    assert!(store.is_mcp_approved(root.path(), &hash).is_err());
    assert!(store.approve_mcp(root.path(), &hash).is_err());
    assert_eq!(std::fs::read_to_string(path).unwrap(), "invalid");
}
