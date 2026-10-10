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
