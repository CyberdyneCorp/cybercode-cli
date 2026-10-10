//! Imported provider references pass through the actual native loader and catalog.
use cyber_core::{
    config::{self, LoadRequest},
    import::codex_provider_config,
    paths::Paths,
};
use cyber_llm::{
    adapters::ApiKind,
    catalog::{Availability, BuildInputs, Catalog, ModelRef},
};
use serde_json::json;
use std::collections::HashMap;

#[test]
fn migrated_provider_credentials_headers_and_models_resolve_through_native_configuration() {
    for (wire, kind) in [
        (None, ApiKind::OpenaiResponses),
        (Some("responses"), ApiKind::OpenaiResponses),
        (Some("chat"), ApiKind::OpenaiCompatible),
    ] {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let mut source = json!({"model":"coder","model_provider":"corp","model_providers":{"corp":{"base_url":"https://example.com/v1","api_key":"source-private-key","http_headers":{"X-Account":"source-private-header"}}}});
        if let Some(wire) = wire {
            source["model_providers"]["corp"]["wire_api"] = json!(wire);
        }
        let imported = codex_provider_config(&source).unwrap();
        let mut env = HashMap::from([(
            "CYBER_HOME".into(),
            root.join("cyber-home").display().to_string(),
        )]);
        for required in &imported.required_environment {
            env.insert(required.variable.clone(), "runtime-private-value".into());
        }
        let paths = Paths::resolve(&env, &root);
        std::fs::create_dir_all(&paths.config).unwrap();
        let document = serde_json::to_string(&imported.config).unwrap();
        assert!(!document.contains("source-private"));
        std::fs::write(paths.config.join("cyber.jsonc"), document).unwrap();
        let loaded = config::load(&LoadRequest {
            location: &root,
            paths: &paths,
            env: &env,
            home: &root,
            profile: None,
            overrides: &[],
            flags: json!({}),
        })
        .unwrap();
        let catalog = Catalog::build(&BuildInputs {
            data: &json!({}),
            config: &loaded.value,
            env: &env,
        });
        let reference = ModelRef::parse(loaded.value["model"].as_str().unwrap()).unwrap();
        let (_, model) = catalog.find(&reference).unwrap();
        assert_eq!(
            (catalog.availability(model), model.cost.as_ref()),
            (Availability::Available, None)
        );
        let resolved = catalog.resolve(&reference, None).unwrap();
        assert_eq!(
            (
                resolved.kind,
                resolved.endpoint.base_url.as_str(),
                resolved.endpoint.api_key.as_deref()
            ),
            (
                kind,
                "https://example.com/v1",
                Some("runtime-private-value")
            )
        );
        assert!(
            resolved
                .request
                .headers
                .contains(&("X-Account".into(), "runtime-private-value".into()))
        );
    }
}

#[test]
fn opencode_versions_resolve_aliases_limits_headers_and_ordered_environment_fallback() {
    for (root_key, package_key, settings_key, package, kind, alias) in [
        (
            "provider",
            "npm",
            "options",
            "@ai-sdk/openai-compatible",
            ApiKind::OpenaiCompatible,
            "id",
        ),
        (
            "providers",
            "package",
            "settings",
            "@opencode/ai/providers/openai/responses",
            ApiKind::OpenaiResponses,
            "modelID",
        ),
    ] {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let source = json!({root_key:{"corp":{package_key:package,settings_key:{"baseURL":"https://example.com/v1"},"env":["ABSENT_KEY","FALLBACK_KEY"],"headers":{"X-Account":"private-header"},"models":{"coder/v1":{alias:"wire-model","name":"Coder","limit":{"context":8192,"input":4096,"output":1024}}}}}});
        let mut imported = cyber_core::import::opencode_provider_config(&source).unwrap();
        imported.config["model"] = json!("corp/coder/v1");
        let mut env = HashMap::from([
            (
                "CYBER_HOME".into(),
                root.join("cyber-home").display().to_string(),
            ),
            ("FALLBACK_KEY".into(), "runtime-key".into()),
        ]);
        for required in &imported.required_environment {
            env.insert(required.variable.clone(), "runtime-header".into());
        }
        let paths = Paths::resolve(&env, &root);
        std::fs::create_dir_all(&paths.config).unwrap();
        std::fs::write(
            paths.config.join("cyber.jsonc"),
            serde_json::to_string(&imported.config).unwrap(),
        )
        .unwrap();
        let loaded = config::load(&LoadRequest {
            location: &root,
            paths: &paths,
            env: &env,
            home: &root,
            profile: None,
            overrides: &[],
            flags: json!({}),
        })
        .unwrap();
        let catalog = Catalog::build(&BuildInputs {
            data: &json!({}),
            config: &loaded.value,
            env: &env,
        });
        let reference = ModelRef::parse("corp/coder/v1").unwrap();
        let (_, model) = catalog.find(&reference).unwrap();
        assert_eq!(catalog.availability(model), Availability::Available);
        assert_eq!(
            (
                model.limits.context,
                model.limits.input,
                model.limits.output
            ),
            (8192, Some(4096), 1024)
        );
        let resolved = catalog.resolve(&reference, None).unwrap();
        assert_eq!(resolved.kind, kind);
        assert_eq!(resolved.endpoint.api_key.as_deref(), Some("runtime-key"));
        assert_eq!(resolved.request.model, "wire-model");
        assert!(
            resolved
                .request
                .headers
                .contains(&("X-Account".into(), "runtime-header".into()))
        );
    }
}

#[test]
fn opencode_request_layers_merge_provider_model_and_selected_variant_in_native_catalog() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let source = json!({"providers":{"corp":{"package":"@opencode/ai/providers/openai/chat","settings":{"baseURL":"https://example.com/v1","apiKey":"{env:KEY}"},"headers":{"X-Team":"provider-secret"},"body":{"nested":{"provider":true,"override":"provider"},"arr":[1,2]},"models":{"coder":{"headers":{"x-team":"model-secret"},"body":{"nested":{"model":true,"override":"model"},"arr":[3]},"variants":[{"id":"deep","headers":{"X-Team":"variant-secret"},"body":{"nested":{"variant":true,"override":"variant"},"arr":[4,5]}}]}}}}});
    let mut imported = cyber_core::import::opencode_provider_config(&source).unwrap();
    imported.config["model"] = json!("corp/coder");
    let mut env = HashMap::from([
        (
            "CYBER_HOME".into(),
            root.join("cyber-home").display().to_string(),
        ),
        ("KEY".into(), "runtime-key".into()),
    ]);
    for required in &imported.required_environment {
        if required.variable == "KEY" {
            continue;
        }
        let value = if required.field.contains("/variants/") {
            "variant-value"
        } else if required.field.contains("/models/") {
            "model-value"
        } else {
            "provider-value"
        };
        env.insert(required.variable.clone(), value.into());
    }
    let paths = Paths::resolve(&env, &root);
    std::fs::create_dir_all(&paths.config).unwrap();
    std::fs::write(
        paths.config.join("cyber.jsonc"),
        serde_json::to_string(&imported.config).unwrap(),
    )
    .unwrap();
    let loaded = config::load(&LoadRequest {
        location: &root,
        paths: &paths,
        env: &env,
        home: &root,
        profile: None,
        overrides: &[],
        flags: json!({}),
    })
    .unwrap();
    let catalog = Catalog::build(&BuildInputs {
        data: &json!({}),
        config: &loaded.value,
        env: &env,
    });
    let reference = ModelRef::parse("corp/coder").unwrap();
    let normal = catalog.resolve(&reference, None).unwrap();
    assert_eq!(
        normal.request.body,
        json!({"nested":{"provider":true,"model":true,"override":"model"},"arr":[3]})
    );
    assert_eq!(
        normal.request.headers,
        vec![("x-team".into(), "model-value".into())]
    );
    let deep = catalog
        .resolve(&ModelRef::parse("corp/coder#deep").unwrap(), None)
        .unwrap();
    assert_eq!(
        deep.request.body,
        json!({"nested":{"provider":true,"model":true,"variant":true,"override":"variant"},"arr":[4,5]})
    );
    assert_eq!(
        deep.request.headers,
        vec![("X-Team".into(), "variant-value".into())]
    );
    assert_eq!(deep.endpoint.api_key.as_deref(), Some("runtime-key"));
    assert!(
        catalog
            .resolve(&ModelRef::parse("corp/coder#missing").unwrap(), None)
            .is_err()
    );
}
