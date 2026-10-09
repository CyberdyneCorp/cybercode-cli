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

#[test]
fn required_flags_are_typed_and_default_definition_digests_are_preserved() {
    let settings = McpSettings::from_config(&json!({"mcp":{
        "local":{"type":"local","command":"x"},
        "remote":{"type":"remote","url":"https://example.test","required":true}
    }}))
    .unwrap();
    assert!(!settings.servers["local"].required());
    assert!(settings.servers["remote"].required());
    let legacy = json!({"kind":"mcp_server","name":"local","server":{
        "type":"local","command":"x","args":[],"env":{},"cwd":null,
        "enabled":true,"timeout":30,"tools":{"allow":null,"deny":[]}
    }});
    use sha2::{Digest, Sha256};
    assert_eq!(
        settings.servers["local"].digest("local").unwrap(),
        format!(
            "sha256:{:x}",
            Sha256::digest(config::canonical_json(&legacy))
        )
    );
    let required = McpSettings::from_config(
        &json!({"mcp":{"local":{"type":"local","command":"x","required":true}}}),
    )
    .unwrap();
    assert_ne!(
        settings.servers["local"].digest("local").unwrap(),
        required.servers["local"].digest("local").unwrap()
    );
    for invalid in [json!("private secret"), json!(1), json!(null)] {
        let error = McpSettings::from_config(
            &json!({"mcp":{"local":{"type":"local","command":"x","required":invalid}}}),
        )
        .unwrap_err();
        assert!(!error.contains("private secret"));
    }
}

#[test]
fn per_server_output_limits_are_positive_typed_and_digest_bound() {
    let default =
        McpSettings::from_config(&json!({"mcp":{"db":{"type":"local","command":"x"}}})).unwrap();
    let capped = McpSettings::from_config(
        &json!({"mcp":{"db":{"type":"local","command":"x","output_token_limit":25}}}),
    )
    .unwrap();
    assert_eq!(default.servers["db"].output_token_limit(), None);
    assert_eq!(capped.servers["db"].output_token_limit(), Some(25));
    assert_ne!(
        default.servers["db"].digest("db").unwrap(),
        capped.servers["db"].digest("db").unwrap()
    );
    for invalid in [json!(0), json!(-1), json!(1.5), json!("private-secret")] {
        let error = McpSettings::from_config(&json!({"mcp":{"db":{"type":"remote","url":"https://example.test","output_token_limit":invalid}}})).unwrap_err();
        assert!(!error.contains("private-secret"));
    }
}
