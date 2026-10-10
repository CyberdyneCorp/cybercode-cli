use cyber_core::import::opencode_provider_config;
use serde_json::json;

#[test]
fn versioned_packages_map_protocols_models_limits_and_exact_source_fields() {
    for (root, package_key, settings, package, kind, alias) in [
        (
            "provider",
            "npm",
            "options",
            "@ai-sdk/openai-compatible",
            "openai-compatible",
            "id",
        ),
        (
            "provider",
            "npm",
            "options",
            "@ai-sdk/openai",
            "openai-responses",
            "id",
        ),
        (
            "providers",
            "package",
            "settings",
            "@opencode/ai/providers/anthropic",
            "anthropic",
            "modelID",
        ),
        (
            "providers",
            "package",
            "settings",
            "@opencode/ai/providers/openai/responses",
            "openai-responses",
            "modelID",
        ),
    ] {
        let source = json!({root:{"llama.cpp":{package_key:package,settings:{"baseURL":"http://localhost:8000/v1","apiKey":"{env:LOCAL_KEY}"},"models":{"coder/v1":{alias:"wire-model","name":"Coder","limit":{"context":8192,"input":4096,"output":1024}}}}}});
        let result = opencode_provider_config(&source).unwrap();
        assert_eq!(result.config["providers"]["llama.cpp"]["api"]["type"], kind);
        assert_eq!(
            result.config["providers"]["llama.cpp"]["models"]["coder/v1"]["id"],
            "wire-model"
        );
        assert_eq!(
            result.config["providers"]["llama.cpp"]["models"]["coder/v1"]["limits"]["context"],
            8192
        );
        assert_eq!(
            result.field_sources["/providers/llama.cpp/models/coder~1v1/id"],
            vec![format!("/{root}/llama.cpp/models/coder~1v1/{alias}")]
        );
        assert_eq!(
            result.required_environment[0].field,
            format!("/{root}/llama.cpp/{settings}/apiKey")
        );
        assert!(!result.required_environment[0].from_literal);
        assert!(result.not_imported.is_empty());
    }
}

#[test]
fn literal_credentials_headers_and_echoed_endpoints_never_enter_proposal() {
    let result = opencode_provider_config(&json!({"provider":{"corp":{"npm":"@ai-sdk/openai-compatible","options":{"apiKey":"private-api-secret","baseURL":"https://example.com/private-api-secret/v1","headers":{"Authorization":"private-header-secret","X-Account":"{env:ACCOUNT}"}}}}})).unwrap();
    assert_eq!(result.required_environment.len(), 4);
    let serialized = serde_json::to_string(&result).unwrap();
    assert!(!serialized.contains("private-"));
    assert!(!format!("{result:?}").contains("private-"));
    assert_eq!(
        result.config["providers"]["corp"]["request"]["headers"]["X-Account"],
        "{env:ACCOUNT}"
    );
}

#[test]
fn unsupported_packages_and_fields_are_reported_without_loading_or_values() {
    let result = opencode_provider_config(&json!({"providers":{
        "custom":{"package":"file:private-code","settings":{"apiKey":"private-key"}},
        "corp":{"package":"@opencode/ai/providers/openai/chat","settings":{"timeout":7},"body":{"store":false},"models":{"coder":{"variants":[{"id":"deep","settings":{"private-option":"private-value"}}],"limit":{"extra":5}}}}
    }})).unwrap();
    assert!(result.config["providers"].get("custom").is_none());
    assert_eq!(result.not_imported.len(), 4);
    assert!(!serde_json::to_string(&result).unwrap().contains("private-"));
}

#[test]
fn ambiguous_unsafe_and_unbounded_inputs_refuse_with_value_free_errors() {
    for provider in [
        json!({"options":{"baseURL":"https://user:private-secret@example.com"}}),
        json!({"options":{"baseURL":"https://example.com?key=private-secret"}}),
        json!({"options":{"apiKey":"{file:private-secret}"}}),
        json!({"env":["invalid-name"]}),
        json!({"headers":{"X-Key":"private-secret","x-key":"other"}}),
        json!({"headers":{"X-Key":"private-secret"},"options":{"headers":{}}}),
        json!({"models":{"coder":{"limit":{"context":0}}}}),
    ] {
        let mut provider = provider;
        provider["npm"] = json!("@ai-sdk/openai-compatible");
        let error = opencode_provider_config(&json!({"provider":{"corp":provider}})).unwrap_err();
        assert!(!error.to_string().contains("private-secret"));
    }
    assert!(opencode_provider_config(&json!({"provider":{},"providers":{}})).is_err());
    let providers: serde_json::Map<_, _> = (0..65).map(|i| (format!("p{i}"), json!({}))).collect();
    assert!(opencode_provider_config(&json!({"provider":providers})).is_err());
}

#[test]
fn generated_bindings_cannot_alias_source_references_or_credential_metadata() {
    let source = json!({"provider":{"corp":{"npm":"@ai-sdk/openai-compatible","options":{"apiKey":"private-secret","headers":{"X-Key":"{env:CYBER_IMPORT_OPENCODE_636F7270_API_KEY}"}}}}});
    assert!(opencode_provider_config(&source).is_err());
    let source = json!({"provider":{"corp":{"npm":"@ai-sdk/openai-compatible","options":{"apiKey":"private-secret"},"models":{"private-secret":{}}}}});
    assert!(opencode_provider_config(&source).is_err());
}

#[test]
fn fallback_names_and_report_pointers_cannot_reuse_literal_credentials() {
    for extra in [
        json!({"env":["CYBER_IMPORT_OPENCODE_636F7270_API_KEY"]}),
        json!({"private-secret":"ignored"}),
        json!({"models":{"coder":{"private-secret":"ignored"}}}),
        json!({"models":{"coder":{"disabled":"invalid"}}}),
    ] {
        let mut provider = extra;
        provider["npm"] = json!("@ai-sdk/openai-compatible");
        provider["options"] = json!({"apiKey":"private-secret"});
        let error = opencode_provider_config(&json!({"provider":{"corp":provider}})).unwrap_err();
        assert!(!error.to_string().contains("private-secret"));
    }
}

#[test]
fn v2_request_overlays_and_variant_arrays_preserve_source_indices_and_binding_namespaces() {
    let result = opencode_provider_config(&json!({"providers":{"corp":{"package":"@opencode/ai/providers/openai/chat","headers":{"X-Team":"private-provider-header"},"body":{"nested":{"provider":true}},"models":{"coder/v1":{"headers":{"x-team":"private-model-header"},"body":{"nested":{"model":true}},"variants":[{"id":"deep","headers":{"X-Team":"private-variant-header"},"body":{"nested":{"variant":true},"arr":[1,2]},"settings":{"reasoningEffort":"high"}}]}}}}})).unwrap();
    assert_eq!(result.required_environment.len(), 3);
    let names: std::collections::BTreeSet<_> = result
        .required_environment
        .iter()
        .map(|r| &r.variable)
        .collect();
    assert_eq!(names.len(), 3);
    assert_eq!(
        result.config["providers"]["corp"]["models"]["coder/v1"]["variants"]["deep"]["request"]["body"]
            ["arr"],
        json!([1, 2])
    );
    assert_eq!(
        result.field_sources["/providers/corp/models/coder~1v1/variants/deep/request/body/arr/1"],
        vec!["/providers/corp/models/coder~1v1/variants/0/body/arr/1"]
    );
    assert_eq!(result.not_imported.len(), 1);
    assert_eq!(
        result.not_imported[0].field,
        "/providers/corp/models/coder~1v1/variants/0/settings"
    );
    assert!(!serde_json::to_string(&result).unwrap().contains("private-"));
}

#[test]
fn malformed_or_unsafe_overlay_fields_refuse_without_credentials() {
    for model in [
        json!({"body":{"apiKey":"private-secret"}}),
        json!({"body":{"nested":[{"api_key":false}]}}),
        json!({"body":{"x":"{env:PRIVATE_SECRET}"}}),
        json!({"body":{"x":"{file:private-secret}"}}),
        json!({"body":{"x":null}}),
        json!({"body":[]}),
        json!({"variants":[{"id":"deep"},{"id":"deep"}]}),
        json!({"variants":[{"id":"invalid/name"}]}),
        json!({"variants":{"deep":{}}}),
        json!({"headers":{"X-Team":7}}),
    ] {
        let source = json!({"providers":{"corp":{"package":"@opencode/ai/providers/openai/chat","models":{"coder":model}}}});
        let error = opencode_provider_config(&source).unwrap_err();
        assert!(!error.to_string().contains("private-secret"));
    }
}

#[test]
fn body_recursion_nodes_and_credential_echoes_are_bounded_before_proposal() {
    let mut deep = json!(true);
    for _ in 0..18 {
        deep = json!({"next":deep});
    }
    for body in [
        deep,
        json!({"items":vec![true;4096]}),
        json!({"echo":"private-header-secret"}),
    ] {
        let source = json!({"providers":{"corp":{"package":"@opencode/ai/providers/openai/chat","headers":{"X-Team":"private-header-secret"},"body":body}}});
        let error = opencode_provider_config(&source).unwrap_err();
        assert!(!error.to_string().contains("private-header-secret"));
    }
}

#[test]
fn v1_package_specific_model_overlays_remain_pending() {
    let result=opencode_provider_config(&json!({"provider":{"corp":{"npm":"@ai-sdk/openai","body":{"store":false},"models":{"coder":{"headers":{"X-Team":"private-header"},"body":{"store":false},"variants":{"high":{"reasoningEffort":"high"}}}}}}})).unwrap();
    assert_eq!(result.not_imported.len(), 4);
    assert!(result.required_environment.is_empty());
    assert!(
        !serde_json::to_string(&result)
            .unwrap()
            .contains("private-header")
    );
}
