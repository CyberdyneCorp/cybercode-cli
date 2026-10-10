use cyber_core::{
    config::McpSettings,
    import::{SourceTool, mcp_config},
};
use serde_json::json;
#[test]
fn three_source_shapes_are_native_definitions_without_copying_credential_values() {
    for (tool, document) in [
        (
            SourceTool::Claude,
            json!({"mcpServers":{"audit":{"command":"audit-server","args":["--stdio",""],"env":{"API_KEY":"private-key"}}}}),
        ),
        (
            SourceTool::Codex,
            json!({"mcp_servers":{"audit":{"command":"audit-server","args":["--stdio",""],"env":{"API_KEY":"private-key"}}}}),
        ),
        (
            SourceTool::OpenCode,
            json!({"mcp":{"servers":{"audit":{"type":"local","command":["audit-server","--stdio",""],"environment":{"API_KEY":"private-key"},"enabled":false}}}}),
        ),
    ] {
        let mapped = mcp_config(tool, &document).unwrap();
        assert_eq!(mapped.required_environment.len(), 1);
        assert!(
            !serde_json::to_string(&mapped)
                .unwrap()
                .contains("private-key")
        );
        let native = McpSettings::from_config(&mapped.config).unwrap();
        assert_eq!(
            native.servers["audit"].enabled(),
            tool != SourceTool::OpenCode
        );
        assert_eq!(
            mapped.config["mcp"]["audit"]["args"],
            json!(["--stdio", ""])
        );
    }
}
#[test]
fn remote_header_values_are_environment_references_and_existing_references_remain() {
    let document = json!({"mcpServers":{"remote":{"type":"http","url":"https://example.test/mcp","headers":{"Authorization":"Bearer private-token","X-Organization":"{env:ORGANIZATION}"}}}});
    let mapped = mcp_config(SourceTool::Claude, &document).unwrap();
    assert_eq!(mapped.required_environment.len(), 2);
    assert_eq!(
        mapped.config["mcp"]["remote"]["headers"]["X-Organization"],
        "{env:ORGANIZATION}"
    );
    assert!(
        !serde_json::to_string(&mapped)
            .unwrap()
            .contains("private-token")
    );
    assert!(McpSettings::from_config(&mapped.config).unwrap().servers["remote"].enabled());
}
#[test]
fn unsafe_ambiguous_or_unsupported_sources_refuse_the_entire_server_batch() {
    for server in [
        json!({"command":"server","url":"https://example.test/mcp"}),
        json!({"command":"server","env":{"API_KEY":"private-key"},"args":["private-key"]}),
        json!({"type":"sse","url":"https://example.test/sse"}),
        json!({"url":"https://example.test/mcp?token=private-key"}),
        json!({"command":"server","enabled":false,"disabled":false}),
        json!({"url":"https://example.test/mcp","headers":{"Authorization":"one","authorization":"two"}}),
    ] {
        let error = mcp_config(
            SourceTool::Claude,
            &json!({"mcpServers":{"first":{"command":"server"},"second":server}}),
        )
        .unwrap_err();
        assert!(!error.to_string().contains("private-key"));
    }
}
#[test]
fn unsupported_options_are_indexed_and_source_limits_are_enforced() {
    let mapped=mcp_config(SourceTool::Codex,&json!({"mcp_servers":{"audit":{"command":"server","startup_timeout_sec":10,"custom":"private-source-value"}}})).unwrap();
    assert_eq!(mapped.not_imported.len(), 2);
    assert!(
        !serde_json::to_string(&mapped)
            .unwrap()
            .contains("private-source-value")
    );
    let names: serde_json::Map<_, _> = (0..129)
        .map(|i| (format!("server_{i}"), json!({"command":"server"})))
        .collect();
    assert!(mcp_config(SourceTool::Codex, &json!({"mcp_servers":names})).is_err());
}

#[test]
fn malformed_documents_and_declared_credentials_in_metadata_are_refused() {
    for document in [
        json!(null),
        json!([]),
        json!({"mcpServers":{"private-key":{"command":"server","env":{"API_KEY":"private-key"}}}}),
        json!({"mcpServers":{"audit":{"command":"server","env":{"API_KEY":"private-key","private-key":"value"}}}}),
    ] {
        let error = mcp_config(SourceTool::Claude, &document).unwrap_err();
        assert!(!error.to_string().contains("private-key"));
    }
}

#[test]
fn codex_http_header_sources_and_required_state_are_native_and_secret_safe() {
    let source = json!({"mcp_servers":{"audit":{"url":"https://example.test/mcp","http_headers":{"Authorization":"Bearer private-source-token"},"env_http_headers":{"X-Organization":"ORGANIZATION"},"required":true}}});
    let mapped = mcp_config(SourceTool::Codex, &source).unwrap();
    assert_eq!(
        mapped.config["mcp"]["audit"]["headers"]["X-Organization"],
        "{env:ORGANIZATION}"
    );
    assert_eq!(mapped.required_environment.len(), 2);
    assert!(McpSettings::from_config(&mapped.config).unwrap().servers["audit"].required());
    assert!(mapped.not_imported.is_empty());
    assert!(
        !serde_json::to_string(&mapped)
            .unwrap()
            .contains("private-source-token")
    );
}

#[test]
fn codex_header_aliases_collisions_and_invalid_environment_names_refuse_whole_batch() {
    for server in [
        json!({"url":"https://example.test/mcp","headers":{"X-Key":"value"},"http_headers":{"X-Key":"other"}}),
        json!({"url":"https://example.test/mcp","http_headers":{"Authorization":"private-token"},"env_http_headers":{"authorization":"TOKEN"}}),
        json!({"url":"https://example.test/mcp","env_http_headers":{"X-Key":"INVALID-NAME"}}),
        json!({"url":"https://example.test/mcp","env_http_headers":{"Bad\r\nName":"TOKEN"}}),
        json!({"url":"https://example.test/mcp","required":"private-value"}),
    ] {
        let error =
            mcp_config(SourceTool::Codex, &json!({"mcp_servers":{"audit":server}})).unwrap_err();
        assert!(!error.to_string().contains("private-"));
    }
}

#[test]
fn environment_header_metadata_cannot_echo_declared_credentials() {
    let source = json!({"mcp_servers":{"audit":{"url":"https://example.test/mcp","http_headers":{"X-Secret":"PRIVATE_TOKEN"},"env_http_headers":{"X-Other":"PRIVATE_TOKEN"}}}});
    let error = mcp_config(SourceTool::Codex, &source).unwrap_err();
    assert!(!error.to_string().contains("PRIVATE_TOKEN"));
}
