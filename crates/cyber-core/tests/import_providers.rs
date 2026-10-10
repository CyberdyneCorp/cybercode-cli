use cyber_core::import::{codex_provider_config, codex_provider_source};
use serde_json::json;

#[test]
fn codex_toml_provider_and_model_are_native_catalog_shapes() {
    let result = codex_provider_source("model='coder'\nmodel_provider='local'\n[model_providers.local]\nbase_url='http://localhost:8000/v1'\nenv_key='LOCAL_KEY'\nwire_api='chat'\n").unwrap();
    assert_eq!(
        result.config,
        json!({"model":"local/coder","providers":{"local":{"api":{"type":"openai-compatible","url":"http://localhost:8000/v1","settings":{"api_key":"{env:LOCAL_KEY}"}},"models":{"coder":{}}}}})
    );
    assert_eq!(result.required_environment[0].variable, "LOCAL_KEY");
    assert!(!result.required_environment[0].from_literal);
    assert!(result.not_imported.is_empty());
    let builtin = codex_provider_config(&json!({"model":"gpt-6-luna"})).unwrap();
    assert_eq!(builtin.config["model"], "openai/gpt-6-luna");
}
#[test]
fn provider_credentials_and_literal_headers_are_references_without_secret_values() {
    let result = codex_provider_config(&json!({"model_providers":{
        "corp": {"base_url":"https://example.com/v1","experimental_bearer_token":"private-api-secret","http_headers":{"Authorization":"private-header-secret"},"env_http_headers":{"X-Account":"ACCOUNT_HEADER"}},
        "corp-": {"api_key":"private-other-secret"},
        "corp_": {"api_key":"private-other-secret"}
    },"notify":["private-command-secret"]})).unwrap();
    let variables: std::collections::BTreeSet<_> = result
        .required_environment
        .iter()
        .map(|v| &v.variable)
        .collect();
    assert_eq!(variables.len(), 5);
    assert_eq!(
        result.config["providers"]["corp"]["api"]["type"],
        "openai-responses"
    );
    assert_eq!(
        result.config["providers"]["corp"]["request"]["headers"]["X-Account"],
        "{env:ACCOUNT_HEADER}"
    );
    assert_eq!(result.not_imported.len(), 1);
    assert!(!serde_json::to_string(&result).unwrap().contains("private-"));
    assert!(!format!("{result:?}").contains("private-"));
}
#[test]
fn unsafe_or_ambiguous_provider_settings_refuse_without_echoing_values() {
    for provider in [
        json!({"env_key":"KEY","api_key":"private-secret"}),
        json!({"base_url":"https://u:private-secret@example.com/v1"}),
        json!({"base_url":"https://example.com/v1?token=private-secret"}),
        json!({"base_url":"file:///private-secret"}),
        json!({"base_url":"https://example.com/{file:private-secret}"}),
        json!({"env_key":"{file:private-secret}"}),
        json!({"requires_openai_auth":true}),
        json!({"wire_api":"private-secret"}),
        json!({"http_headers":{"X-Key":"private-secret"},"env_http_headers":{"x-key":"OTHER"}}),
        json!({"http_headers":{"Bad\r\nName":"private-secret"}}),
        json!({"http_headers":{"X-Key":"private-secret\u{0}"}}),
    ] {
        let error =
            codex_provider_config(&json!({"model_providers":{"corp":provider}})).unwrap_err();
        assert!(!error.to_string().contains("private-secret"));
    }
}
#[test]
fn limits_and_pending_fields_are_explicit_and_value_free() {
    let providers: serde_json::Map<_, _> = (0..65).map(|i| (format!("p{i}"), json!({}))).collect();
    assert!(codex_provider_config(&json!({"model_providers":providers})).is_err());
    assert!(
        codex_provider_source(&"x".repeat(1024 * 1024 + 1))
            .unwrap_err()
            .reason
            .contains("one MiB")
    );
    assert!(
        !codex_provider_source("api_key='private-secret'\ninvalid=[oops")
            .unwrap_err()
            .to_string()
            .contains("private-secret")
    );
    let result = codex_provider_config(&json!({"model_provider":"local","profiles":{"private-name":{"api_key":"private-secret"}},"model_providers":{"local":{"query_params":{"token":"private-secret"}}}})).unwrap();
    assert_eq!(result.not_imported.len(), 3);
    assert!(!serde_json::to_string(&result).unwrap().contains("private-"));
    assert!(codex_provider_config(&json!({"model":"{file:private-secret}"})).is_err());
}

#[test]
fn generated_credential_variables_cannot_alias_existing_source_environment_bindings() {
    let source = json!({"model_providers": {
        "corp": {"api_key":"private-secret"},
        "other": {"env_key":"CYBER_IMPORT_CODEX_636F7270_API_KEY"}
    }});
    let error = codex_provider_config(&source).unwrap_err();
    assert!(error.reason.contains("conflicts"));
    assert!(!error.to_string().contains("private-secret"));
}

#[test]
fn endpoint_paths_containing_declared_credentials_become_environment_references() {
    let source = json!({"model_providers":{"corp":{
        "base_url":"https://example.com/private-secret/v1",
        "api_key":"private-secret"
    }}});
    let result = codex_provider_config(&source).unwrap();
    assert_eq!(result.required_environment.len(), 2);
    assert_eq!(
        result.config["providers"]["corp"]["api"]["url"],
        "{env:CYBER_IMPORT_CODEX_636F7270_BASE_URL}"
    );
    assert!(
        !serde_json::to_string(&result)
            .unwrap()
            .contains("private-secret")
    );
}

#[test]
fn credentials_reused_as_metadata_or_other_endpoints_cannot_escape_the_plan() {
    for extra in [
        json!({"model":"private-secret"}),
        json!({"model_providers":{"private-secret":{},"corp":{"api_key":"private-secret"}}}),
        json!({"model_providers":{"corp":{"api_key":"private-secret","http_headers":{"private-secret":"other"}}}}),
        json!({"model_providers":{"corp":{"api_key":"private-secret"},"other":{"base_url":"https://example.com/private-secret"}}}),
    ] {
        let mut source = json!({"model_providers":{"corp":{"api_key":"private-secret"}}});
        for (key, value) in extra.as_object().unwrap() {
            source[key] = value.clone();
        }
        let error = codex_provider_config(&source).unwrap_err();
        assert!(!error.to_string().contains("private-secret"));
    }
}
