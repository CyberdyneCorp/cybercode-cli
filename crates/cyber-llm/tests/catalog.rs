//! `provider-catalog` and `provider-credentials` behavior.

use std::collections::HashMap;
use std::time::Duration;

use cyber_llm::Reasoning;
use cyber_llm::adapters::ApiKind;
use cyber_llm::catalog::{
    Availability, BuildInputs, Catalog, CatalogError, CredentialSource, ModelRef, ModelRole,
    Origin, SourceOptions, default_model, load_source, recent_models, record_recent, role_ref,
};
use serde_json::{Value, json};
use wiremock::matchers::path;
use wiremock::{Mock, MockServer, ResponseTemplate};

fn env(pairs: &[(&str, &str)]) -> HashMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

fn data() -> Value {
    json!({
        "openai": { "id": "openai", "name": "OpenAI", "npm": "@ai-sdk/openai", "env": ["OPENAI_API_KEY"], "models": {
            "gpt-6": { "id": "gpt-6", "name": "GPT-6", "tool_call": true, "reasoning": true, "release_date": "2026-09-01",
                "reasoning_options": [{"type": "effort", "values": ["none", "low", "high"]}],
                "limit": {"context": 400000, "output": 128000}, "cost": {"input": 2, "output": 10, "cache_read": 0.2},
                "modalities": {"input": ["text", "image"], "output": ["text"]} },
            "gpt-6-mini": { "id": "gpt-6-mini", "tool_call": true, "release_date": "2026-09-15", "limit": {"context": 128000, "output": 16000}, "cost": {"input": 0.1, "output": 0.4} },
            "gpt-old": { "id": "gpt-old", "tool_call": true, "status": "deprecated", "release_date": "2020-01-01" }
        }},
        "anthropic": { "id": "anthropic", "name": "Anthropic", "npm": "@ai-sdk/anthropic", "env": ["ANTHROPIC_API_KEY"], "models": {
            "claude-sonnet": { "id": "claude-sonnet", "tool_call": true, "reasoning": true, "release_date": "2026-09-28",
                "reasoning_options": [{"type": "budget_tokens", "max": 20000}], "limit": {"context": 1000000, "output": 128000} }
        }},
        "google": { "id": "google", "npm": "@ai-sdk/google", "env": ["GEMINI_API_KEY"], "models": { "gemini": { "id": "gemini", "tool_call": true } } }
    })
}

fn catalog(config: Value, e: &HashMap<String, String>) -> Catalog {
    Catalog::build(&BuildInputs {
        data: &data(),
        config: &config,
        env: e,
    })
}

fn model_ref(s: &str) -> ModelRef {
    ModelRef::parse(s).unwrap()
}

#[test]
fn bundled_snapshot_has_p0_providers() {
    let dir = tempfile::tempdir().unwrap();
    let opts = SourceOptions {
        url: "https://models.dev".into(),
        cache_dir: dir.path().to_path_buf(),
        path: None,
        allow_fetch: false,
        refresh: false,
        fresh_for: Duration::from_secs(300),
        timeout: Duration::from_secs(1),
    };
    let loaded = tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(load_source(&opts))
        .unwrap();
    assert_eq!(loaded.origin, Origin::Snapshot);
    let c = Catalog::build(&BuildInputs {
        data: &loaded.data,
        config: &json!({}),
        env: &env(&[]),
    });
    assert_eq!(c.providers["openai"].kind, Some(ApiKind::OpenaiResponses));
    assert_eq!(c.providers["anthropic"].kind, Some(ApiKind::Anthropic));
    assert!(
        c.providers
            .values()
            .any(|p| p.kind == Some(ApiKind::OpenaiCompatible))
    );
    assert!(c.providers["openai"].models.len() > 10);
}

#[test]
fn availability_follows_credentials_status_and_adapters() {
    let c = catalog(json!({}), &env(&[("OPENAI_API_KEY", "sk-x")]));
    let get = |r: &str| {
        let (_, m) = c.find(&model_ref(r)).unwrap();
        c.availability(m)
    };
    assert_eq!(get("openai/gpt-6"), Availability::Available);
    assert_eq!(
        get("anthropic/claude-sonnet"),
        Availability::Unavailable("missing_credentials")
    );
    assert_eq!(
        get("openai/gpt-old"),
        Availability::Unavailable("deprecated")
    );
    assert_eq!(
        get("google/gemini"),
        Availability::Unavailable("adapter_unsupported")
    );
    let rows = c.list(None).unwrap();
    let first_unavailable = rows
        .iter()
        .position(|r| r.availability != Availability::Available)
        .unwrap();
    assert!(
        rows[first_unavailable..]
            .iter()
            .all(|r| r.availability != Availability::Available),
        "available first"
    );
    assert!(matches!(
        c.list(Some("nope")),
        Err(CatalogError::ProviderNotFound(_))
    ));
}

#[test]
fn credential_precedence() {
    let auth = r#"{"openai": {"kind": "api_key", "key": "sk-injected"}}"#;
    let c = catalog(
        json!({}),
        &env(&[("OPENAI_API_KEY", "sk-env"), ("CYBER_AUTH_CONTENT", auth)]),
    );
    assert_eq!(
        c.credential("openai").unwrap().source,
        CredentialSource::AuthContent
    );
    let configured =
        json!({"providers": {"openai": {"api": {"settings": {"api_key": "sk-config"}}}}});
    let c = catalog(
        configured,
        &env(&[("OPENAI_API_KEY", "sk-env"), ("CYBER_AUTH_CONTENT", auth)]),
    );
    assert_eq!(
        c.credential("openai").unwrap().api_key.as_deref(),
        Some("sk-config")
    );
    let c = catalog(json!({}), &env(&[("OPENAI_API_KEY", "sk-env")]));
    assert_eq!(
        c.credential("openai").unwrap().source,
        CredentialSource::Env("OPENAI_API_KEY".into())
    );
    assert!(
        !format!("{:?}", c.credential("openai").unwrap()).contains("sk-env"),
        "debug output is redacted"
    );
}

#[test]
fn local_custom_provider_needs_no_key() {
    let config = json!({"providers": {"ollama": {
        "api": {"type": "openai-compatible", "url": "http://127.0.0.1:11434/v1"},
        "models": {"qwen3-coder": {"limits": {"context": 32768, "output": 4096}}}
    }}});
    let c = catalog(config, &env(&[]));
    let (_, m) = c.find(&model_ref("ollama/qwen3-coder")).unwrap();
    assert_eq!(c.availability(m), Availability::Available);
    assert!(m.capabilities.tools);
    assert_eq!(m.cost, None, "unknown pricing is unpriced, not zero");
    let resolved = c.resolve(&model_ref("ollama/qwen3-coder"), None).unwrap();
    assert_eq!(resolved.kind, ApiKind::OpenaiCompatible);
    assert_eq!(resolved.endpoint.api_key, None);
    assert_eq!(resolved.request.max_output_tokens, Some(4096));
}

#[test]
fn remote_custom_provider_without_key_is_unavailable() {
    let config = json!({"providers": {"corp": {"api": {"type": "openai-compatible", "url": "https://llm.corp/v1"}, "models": {"coder": {}}}}});
    let c = catalog(config, &env(&[]));
    let (_, m) = c.find(&model_ref("corp/coder")).unwrap();
    assert_eq!(
        c.availability(m),
        Availability::Unavailable("missing_credentials")
    );
}

#[test]
fn variants_come_from_reasoning_options_and_config() {
    let config = json!({"providers": {"openai": {"models": {"gpt-6": {"variants": {
        "low": {"disabled": true},
        "deep": {"effort": "xhigh", "request": {"body": {"text": {"verbosity": "low"}}}}
    }}}}}});
    let c = catalog(
        config,
        &env(&[("OPENAI_API_KEY", "k"), ("ANTHROPIC_API_KEY", "k")]),
    );
    let (_, gpt) = c.find(&model_ref("openai/gpt-6")).unwrap();
    assert_eq!(
        gpt.variants.keys().collect::<Vec<_>>(),
        vec!["deep", "high", "none"]
    );
    assert_eq!(gpt.variants["none"].reasoning, Some(Reasoning::Off));
    let deep = c.resolve(&model_ref("openai/gpt-6#deep"), None).unwrap();
    assert_eq!(
        deep.request.reasoning,
        Some(Reasoning::Effort("xhigh".into()))
    );
    assert_eq!(deep.request.body["text"]["verbosity"], "low");

    let claude = c
        .resolve(&model_ref("anthropic/claude-sonnet#max"), None)
        .unwrap();
    assert_eq!(
        claude.request.reasoning,
        Some(Reasoning::BudgetTokens(20000)),
        "budget capped at the catalog max"
    );

    match c.resolve(&model_ref("openai/gpt-6#ultra"), None) {
        Err(CatalogError::VariantUnavailable { available, .. }) => {
            assert_eq!(available, "deep, high, none")
        }
        other => panic!("expected VariantUnavailable, got {:?}", other.err()),
    }
}

#[test]
fn request_options_layer_and_never_carry_keys() {
    let config = json!({"providers": {"openai": {
        "request": {"headers": {"X-A": "provider", "X-B": "provider"}, "body": {"temperature": 0.7, "api_key": "leak"}},
        "models": {"gpt-6": {"request": {"headers": {"X-B": "model"}, "body": {"top_p": 0.9}}}}
    }}});
    let c = catalog(config, &env(&[("OPENAI_API_KEY", "k")]));
    let agent = cyber_llm::catalog::RequestOverlay::from_config(Some(
        &json!({"body": {"temperature": 0.2}}),
    ));
    let r = c.resolve(&model_ref("openai/gpt-6"), Some(&agent)).unwrap();
    assert_eq!(r.request.body, json!({"temperature": 0.2, "top_p": 0.9}));
    assert!(
        r.request
            .headers
            .contains(&("X-A".into(), "provider".into()))
    );
    assert!(r.request.headers.contains(&("X-B".into(), "model".into())));
}

#[test]
fn unavailable_model_is_not_resolved() {
    let c = catalog(json!({}), &env(&[]));
    assert!(matches!(
        c.resolve(&model_ref("openai/gpt-6"), None),
        Err(CatalogError::Unavailable { .. })
    ));
    assert!(matches!(
        c.resolve(&model_ref("openai/nope"), None),
        Err(CatalogError::ModelNotFound(_))
    ));
}

#[test]
fn role_fallbacks() {
    let config = json!({"model": "anthropic/claude-sonnet", "small_model": "openai/gpt-6-mini"});
    assert_eq!(
        role_ref(&config, ModelRole::Title).as_deref(),
        Some("openai/gpt-6-mini")
    );
    assert_eq!(
        role_ref(&config, ModelRole::Evaluator).as_deref(),
        Some("openai/gpt-6-mini")
    );
    assert_eq!(role_ref(&config, ModelRole::Advisor), None);
    let only_default = json!({"model": "openai/gpt-6"});
    assert_eq!(
        role_ref(&only_default, ModelRole::Compaction).as_deref(),
        Some("openai/gpt-6")
    );
    let explicit = json!({"model": "a/b", "model_roles": {"advisor": "anthropic/claude-sonnet", "title": "x/y"}});
    assert_eq!(
        role_ref(&explicit, ModelRole::Advisor).as_deref(),
        Some("anthropic/claude-sonnet")
    );
    assert_eq!(
        role_ref(&explicit, ModelRole::Title).as_deref(),
        Some("x/y")
    );
}

#[test]
fn default_model_resolution_order() {
    let c = catalog(json!({}), &env(&[("OPENAI_API_KEY", "k")]));
    let configured = json!({"model": "anthropic/claude-sonnet"});
    let (r, warnings) = default_model(&c, &configured, &["openai/gpt-6".into()]).unwrap();
    assert_eq!(r.to_string(), "openai/gpt-6");
    assert!(warnings[0].contains("configured model anthropic/claude-sonnet is unavailable"));
    let (newest, _) = default_model(&c, &json!({}), &[]).unwrap();
    assert_eq!(
        newest.to_string(),
        "openai/gpt-6-mini",
        "newest available by release date"
    );
    let none = catalog(json!({}), &env(&[]));
    assert!(matches!(
        default_model(&none, &json!({}), &[]),
        Err(CatalogError::ModelNotSelected)
    ));
}

#[test]
fn recent_models_are_capped_and_deduplicated() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("model.json");
    for i in 0..12 {
        record_recent(&file, &format!("p/m{i}")).unwrap();
    }
    record_recent(&file, "p/m5").unwrap();
    let recent = recent_models(&file);
    assert_eq!(recent.len(), 10);
    assert_eq!(recent[0], "p/m5");
    assert_eq!(recent.iter().filter(|r| *r == "p/m5").count(), 1);
}

fn opts(url: String, dir: &std::path::Path) -> SourceOptions {
    SourceOptions {
        url,
        cache_dir: dir.to_path_buf(),
        path: None,
        allow_fetch: true,
        refresh: false,
        fresh_for: Duration::from_secs(300),
        timeout: Duration::from_secs(2),
    }
}

#[tokio::test]
async fn fetch_writes_cache_and_fresh_cache_avoids_network() {
    let server = MockServer::start().await;
    Mock::given(path("/api.json"))
        .respond_with(ResponseTemplate::new(200).set_body_json(data()))
        .expect(1)
        .mount(&server)
        .await;
    let dir = tempfile::tempdir().unwrap();
    let o = opts(server.uri(), dir.path());
    assert_eq!(load_source(&o).await.unwrap().origin, Origin::Fetched);
    assert!(
        o.cache_file()
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .starts_with("models-"),
        "non-default URL gets a hashed cache name"
    );
    assert_eq!(load_source(&o).await.unwrap().origin, Origin::FreshCache);
}

#[tokio::test]
async fn concurrent_refreshes_fetch_once() {
    let server = MockServer::start().await;
    Mock::given(path("/api.json"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(data())
                .set_delay(Duration::from_millis(200)),
        )
        .expect(1)
        .mount(&server)
        .await;
    let dir = tempfile::tempdir().unwrap();
    let a = opts(server.uri(), dir.path());
    let b = opts(server.uri(), dir.path());
    let (ra, rb) = tokio::join!(load_source(&a), load_source(&b));
    let mut origins = vec![ra.unwrap().origin, rb.unwrap().origin];
    origins.sort_by_key(|o| format!("{o:?}"));
    assert_eq!(origins, vec![Origin::Fetched, Origin::FreshCache]);
}

#[tokio::test]
async fn failed_refresh_falls_back_to_stale_cache_then_snapshot() {
    let server = MockServer::start().await;
    Mock::given(path("/api.json"))
        .respond_with(ResponseTemplate::new(503))
        .mount(&server)
        .await;
    let dir = tempfile::tempdir().unwrap();
    let mut o = opts(server.uri(), dir.path());
    o.timeout = Duration::from_millis(500);
    let none = load_source(&o).await.unwrap();
    assert_eq!(none.origin, Origin::Snapshot);
    assert!(none.warning.unwrap().contains("refresh failed"));

    std::fs::write(o.cache_file(), data().to_string()).unwrap();
    o.fresh_for = Duration::ZERO;
    assert_eq!(load_source(&o).await.unwrap().origin, Origin::StaleCache);
}

#[tokio::test]
async fn pinned_path_is_used_without_network() {
    let dir = tempfile::tempdir().unwrap();
    let pinned = dir.path().join("pinned.json");
    std::fs::write(&pinned, data().to_string()).unwrap();
    let mut o = opts("http://127.0.0.1:9".into(), dir.path());
    o.path = Some(pinned);
    let loaded = load_source(&o).await.unwrap();
    assert_eq!(loaded.origin, Origin::Path);
    assert!(loaded.data.get("openai").is_some());
}
