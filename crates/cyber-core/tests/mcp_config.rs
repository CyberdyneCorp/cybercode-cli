//! MCP definitions validate after trust filtering and substitution, without execution.
use cyber_core::config::{self, LoadRequest, McpServer, McpSettings};
use cyber_core::paths::Paths;
use cyber_core::trust::TrustStore;
use serde_json::{Value, json};
use std::collections::HashMap;

#[test]
fn tool_timeout_object_is_a_valid_server_name() {
    let settings = McpSettings::from_config(&json!({"mcp": {
        "tool_timeout": {"type": "local", "command": "audit-server"}
    }}))
    .unwrap();
    assert_eq!(settings.tool_timeout, 300);
    assert!(settings.servers.contains_key("tool_timeout"));
    let scalar = McpSettings::from_config(&json!({"mcp": {"tool_timeout": 120}})).unwrap();
    assert_eq!(scalar.tool_timeout, 120);
    assert!(scalar.servers.is_empty());
}

#[test]
fn local_remote_defaults_filters_and_timeout_are_typed() {
    let settings = McpSettings::from_config(&json!({"mcp":{"tool_timeout":120,"audit":{"type":"local","command":"audit-server","args":["--stdio"],"tools":{"allow":["read*"],"deny":["read_secret"]}},"remote":{"type":"remote","url":"https://example.test/mcp","enabled":false,"oauth":false}}})).unwrap();
    assert_eq!(settings.tool_timeout, 120);
    let local = &settings.servers["audit"];
    assert!(local.enabled());
    assert_eq!(local.timeout(), 30);
    assert!(local.tools().permits("read_file").unwrap());
    assert!(!local.tools().permits("read_secret").unwrap());
    assert!(!local.tools().permits("write_file").unwrap());
    assert!(!settings.servers["remote"].enabled());
}

#[test]
fn invalid_definitions_fail_without_echoing_header_or_environment_values() {
    let cases = [
        json!({"bad name":{"type":"local","command":"x"}}),
        json!({"audit":{"type":"local","command":""}}),
        json!({"audit":{"type":"local","command":"x","env":{"BAD=KEY":"private secret"}}}),
        json!({"audit":{"type":"local","command":"x","args":["nul\u{0}"]}}),
        json!({"audit":{"type":"local","command":"x","timeout":0}}),
        json!({"audit":{"type":"local","command":"x","tools":{"allow":["["]}}}),
        json!({"audit":{"type":"remote","url":"file:///private secret"}}),
        json!({"audit":{"type":"remote","url":"https://user:private secret@example.test"}}),
        json!({"audit":{"type":"remote","url":"https://example.test","headers":{"authorization":"private secret\r\ninjection"}}}),
        json!({"audit":{"type":"remote","url":"https://example.test","oauth":"private secret"}}),
        json!({"audit":{"type":"remote","url":"https://example.test","unknown":"private secret"}}),
        json!({"tool_timeout":0}),
    ];
    for servers in cases {
        let error = McpSettings::from_config(&json!({"mcp":servers})).unwrap_err();
        assert!(!error.contains("private secret"));
        assert!(error.starts_with("mcp."));
    }
}

#[test]
fn actual_loading_withholds_untrusted_servers_before_substitution_and_validation() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    std::fs::create_dir_all(repo.join(".git")).unwrap();
    let env = HashMap::from([
        (
            "CYBER_HOME".into(),
            dir.path().join("home").display().to_string(),
        ),
        ("MCP_TOKEN".into(), "private credential".into()),
    ]);
    let paths = Paths::resolve(&env, dir.path());
    paths.ensure().unwrap();
    std::fs::write(repo.join("cyber.jsonc"), json!({"mcp":{"audit":{"type":"remote","url":"https://example.test/mcp","headers":{"authorization":"{env:MCP_TOKEN}"}}}}).to_string()).unwrap();
    let request = LoadRequest {
        location: &repo,
        paths: &paths,
        env: &env,
        home: dir.path(),
        profile: None,
        overrides: &[],
        flags: json!({}),
    };
    let untrusted = config::load(&request).unwrap();
    assert!(
        McpSettings::from_config(&untrusted.value)
            .unwrap()
            .servers
            .is_empty()
    );
    let report = config::trust_report(&request).unwrap();
    TrustStore::new(paths.trust_file())
        .approve(&report.checkout_root, report.digest.as_deref().unwrap())
        .unwrap();
    let trusted = config::load(&request).unwrap();
    let settings = McpSettings::from_config(&trusted.value).unwrap();
    let McpServer::Remote { headers, .. } = &settings.servers["audit"] else {
        panic!("remote");
    };
    assert_eq!(headers["authorization"], "private credential");
    std::fs::write(repo.join("cyber.jsonc"), json!({"mcp":{"audit":{"type":"remote","url":"file:///invalid","headers":{"authorization":"{env:UNAVAILABLE}"}}}}).to_string()).unwrap();
    assert!(config::load(&request).unwrap().value.get("mcp").is_none());
    let report = config::trust_report(&request).unwrap();
    TrustStore::new(paths.trust_file())
        .approve(&report.checkout_root, report.digest.as_deref().unwrap())
        .unwrap();
    assert!(config::load(&request).is_err());
}

#[test]
fn global_invalid_server_is_rejected_through_actual_config_loading() {
    let dir = tempfile::tempdir().unwrap();
    let env = HashMap::from([(
        "CYBER_HOME".into(),
        dir.path().join("home").display().to_string(),
    )]);
    let paths = Paths::resolve(&env, dir.path());
    paths.ensure().unwrap();
    std::fs::write(
        paths.config.join("cyber.jsonc"),
        json!({"mcp":{"audit":{"type":"local","command":"x","timeout":0}}}).to_string(),
    )
    .unwrap();
    let request = LoadRequest {
        location: dir.path(),
        paths: &paths,
        env: &env,
        home: dir.path(),
        profile: None,
        overrides: &[],
        flags: Value::Object(Default::default()),
    };
    assert!(
        config::load(&request)
            .unwrap_err()
            .to_string()
            .contains("mcp.audit")
    );
}
