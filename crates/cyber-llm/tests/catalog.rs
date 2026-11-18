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
    let google = &c.providers["google"];
    assert_eq!(google.kind, Some(ApiKind::Gemini));
    assert!(google.models.contains_key("gemini-4-carbon"));
    assert!(
        google
            .models
            .values()
            .all(|m| m.api_kind.or(google.kind) == Some(ApiKind::Gemini)),
        "google models resolve to the gemini protocol"
    );
    assert!(
        !c.providers.values().any(|p| p.id == "google-vertex"),
        "@ai-sdk/google-vertex stays out of the snapshot"
    );
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
        Availability::Unavailable("experimental"),
        "the gemini protocol is behind features.gemini"
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

/// `provider-catalog` → Responses request shaping: the resolved request carries what the
/// model's profile and capabilities allow; an unsupported verbosity default is not sent.
#[test]
fn resolved_requests_carry_the_profile_shaping() {
    use cyber_llm::Shaping;
    use cyber_llm::catalog::profile::VerbosityLevel;
    let config = json!({"providers": {"openai": {"models": {
        "gpt-6": {"profile": {"verbosity": {"default": "low"}}},
        "gpt-6-mini": {"profile": {"verbosity": {"supported": false, "default": "high"},
            "parallel_tool_calls": false}}
    }}}});
    let c = catalog(config, &env(&[("OPENAI_API_KEY", "sk-env")]));
    let shaping = |id: &str| c.resolve(&model_ref(id), None).unwrap().request.shaping;
    assert_eq!(
        shaping("openai/gpt-6"),
        Shaping {
            reasoning: true,
            reasoning_summary: true,
            effort_none: true,
            verbosity: Some(VerbosityLevel::Low),
            parallel_tool_calls: Some(true),
            freeform_patch: true,
            ..Shaping::default()
        }
    );
    let mini = shaping("openai/gpt-6-mini");
    assert!(!mini.reasoning, "no reasoning capability, so no include");
    assert_eq!(
        (mini.verbosity, mini.parallel_tool_calls),
        (None, Some(false))
    );
}

/// `provider-catalog` → Stream idle timeout: `timeout.idle_ms` sets the window, `0`
/// turns it off, and the default is five minutes.
#[test]
fn idle_timeout_follows_the_provider_config() {
    let idle = |timeout: Value| {
        let config = json!({"providers": {"ollama": {
            "api": {"type": "openai-compatible", "url": "http://127.0.0.1:11434/v1"},
            "timeout": timeout,
            "models": {"qwen3-coder": {}}
        }}});
        let c = catalog(config, &env(&[]));
        c.resolve(&model_ref("ollama/qwen3-coder"), None)
            .unwrap()
            .endpoint
            .idle_timeout
    };
    assert_eq!(idle(json!({})), Some(Duration::from_secs(300)));
    assert_eq!(
        idle(json!({"idle_ms": 1500})),
        Some(Duration::from_millis(1500))
    );
    assert_eq!(idle(json!({"idle_ms": 0})), None);
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

#[test]
fn agent_headers_override_prior_layers_case_insensitively() {
    let config = json!({"providers":{"openai":{"request":{"headers":{"x-priority":"provider"}}}}});
    let c = catalog(config, &env(&[("OPENAI_API_KEY", "k")]));
    let agent = cyber_llm::catalog::RequestOverlay::from_config(Some(
        &json!({"headers":{"X-Priority":"agent"}}),
    ));
    let request = c
        .resolve(&model_ref("openai/gpt-6"), Some(&agent))
        .unwrap()
        .request;
    let headers: Vec<_> = request
        .headers
        .iter()
        .filter(|(key, _)| key.eq_ignore_ascii_case("x-priority"))
        .collect();
    assert_eq!(headers.len(), 1);
    assert_eq!(headers[0].1, "agent");
}

#[test]
fn request_credentials_are_removed_from_arrays_and_nested_objects() {
    let c = catalog(json!({}), &env(&[("OPENAI_API_KEY", "k")]));
    let agent = cyber_llm::catalog::RequestOverlay::from_config(Some(&json!({"body":{
        "items":[{"apiKey":"secret", "nested":[{"api_key":"secret", "keep":true}]}]
    }})));
    let request = c
        .resolve(&model_ref("openai/gpt-6"), Some(&agent))
        .unwrap()
        .request;
    assert_eq!(request.body, json!({"items":[{"nested":[{"keep":true}]}]}));
}

#[test]
fn later_agent_layer_matches_catalog_layering_and_empty_layers_preserve_templates() {
    let config = json!({"providers":{"openai":{
        "request":{"headers":{"x-priority":"provider"},"body":{"temperature":0.7,"vendor":{"provider":true,"choice":"provider"}}},
        "models":{"gpt-6":{
            "request":{"body":{"vendor":{"model":true,"choice":"model"}}},
            "variants":{"custom":{"request":{"body":{"vendor":{"variant":true,"choice":"variant"}}}}}
        }}
    }}});
    let c = catalog(config, &env(&[("OPENAI_API_KEY", "k")]));
    let model = model_ref("openai/gpt-6#custom");
    let mut template = c.resolve(&model, None).unwrap().request;
    let original = serde_json::to_vec(&template).unwrap();
    cyber_llm::catalog::RequestOverlay::default().apply_to(&mut template);
    assert_eq!(serde_json::to_vec(&template).unwrap(), original);
    let agent = cyber_llm::catalog::RequestOverlay::from_config(Some(&json!({
        "headers":{"X-Priority":"agent"},"body":{"temperature":0.2,"vendor":{"agent":true,"choice":"agent"}}
    })));
    agent.apply_to(&mut template);
    assert_eq!(template, c.resolve(&model, Some(&agent)).unwrap().request);
    assert_eq!(
        template.body["vendor"],
        json!({"provider":true,"model":true,"variant":true,"agent":true,"choice":"agent"})
    );
}

/// `provider-catalog` → Model behavior profiles: "Built-in rule for a compatible provider".
#[test]
fn a_compatible_provider_gets_the_built_in_rule() {
    use cyber_llm::catalog::profile::{ReasoningCarrier, ReasoningReplay};
    let config = json!({"providers": {"corp": {
        "api": {"type": "openai-compatible", "url": "https://llm.corp.example/v1"},
        "models": {"kimi-k2-thinking": {}},
    }}});
    let c = catalog(config, &env(&[]));
    let (_, model) = c.find(&model_ref("corp/kimi-k2-thinking")).unwrap();
    assert_eq!(
        model.profile.reasoning_carrier,
        ReasoningCarrier::ReasoningContent
    );
    assert_eq!(model.profile.reasoning_replay, ReasoningReplay::ToolLoop);
}

/// `provider-catalog` → Model behavior profiles: "Config overrides a field".
#[test]
fn config_overrides_one_profile_field() {
    use cyber_llm::catalog::profile::ReasoningReplay;
    let built_in = catalog(json!({}), &env(&[]));
    let config = json!({"providers": {"openai": {"models": {"gpt-6": {
        "profile": {"reasoning_replay": "never"},
    }}}}});
    let configured = catalog(config, &env(&[]));
    let profile_of = |c: &Catalog| {
        c.find(&model_ref("openai/gpt-6"))
            .unwrap()
            .1
            .profile
            .clone()
    };
    let (before, after) = (profile_of(&built_in), profile_of(&configured));
    assert_eq!(before.reasoning_replay, ReasoningReplay::Always);
    assert_eq!(after.reasoning_replay, ReasoningReplay::Never);
    let mut expected = before;
    expected.reasoning_replay = ReasoningReplay::Never;
    assert_eq!(
        after, expected,
        "the other fields keep their built-in values"
    );
    let listed = serde_json::to_value(configured.list(Some("openai")).unwrap()).unwrap();
    let row = listed
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["ref"] == "openai/gpt-6")
        .unwrap();
    assert_eq!(row["model"]["profile"]["reasoning_replay"], "never");
}

/// `provider-catalog` → Model behavior profiles: "Effort clamped for a workflow tier".
#[test]
fn a_derived_effort_clamps_to_the_nearest_lower_level() {
    let config = json!({"providers": {"corp": {
        "api": {"type": "openai-compatible", "url": "https://llm.corp.example/v1",
            "settings": {"api_key": "k"}},
        "models": {
            "coder": {"variants": {"low": {"effort": "low"}, "medium": {"effort": "medium"},
                "high": {"effort": "high"}}},
            "strict": {"variants": {"low": {"effort": "low"}}, "profile": {"effort_clamp": "fail"}},
        },
    }}});
    let c = catalog(config, &env(&[]));
    let clamped = c.clamp_effort(&model_ref("corp/coder#xhigh")).unwrap();
    assert_eq!(
        (clamped.from.as_str(), clamped.to.as_str()),
        ("xhigh", "high")
    );
    assert_eq!(clamped.model_ref.to_string(), "corp/coder#high");
    assert_eq!(
        c.clamp_effort(&model_ref("corp/coder#high")),
        None,
        "listed"
    );
    assert_eq!(
        c.clamp_effort(&model_ref("corp/coder#minimal")),
        None,
        "nothing lower"
    );
    assert_eq!(
        c.clamp_effort(&model_ref("corp/coder#deep")),
        None,
        "not an effort"
    );
    assert_eq!(
        c.clamp_effort(&model_ref("corp/strict#high")),
        None,
        "effort_clamp: fail"
    );
    // An explicit variant is never clamped: resolution still fails.
    assert!(matches!(
        c.resolve(&model_ref("corp/coder#xhigh"), None),
        Err(CatalogError::VariantUnavailable { .. })
    ));
}

/// The resolved request names the profile's reasoning carrier, and none when the profile
/// replays no reasoning.
#[test]
fn the_request_names_the_carrier_unless_nothing_is_replayed() {
    use cyber_llm::catalog::profile::ReasoningCarrier;
    let config = json!({"providers": {"corp": {
        "api": {"type": "openai-compatible", "url": "https://llm.corp.example/v1",
            "settings": {"api_key": "k"}},
        "models": {
            "deepseek-v4": {},
            "deepseek-quiet": {"profile": {"reasoning_replay": "never"}},
        },
    }}});
    let c = catalog(config, &env(&[]));
    let carrier = |model: &str| {
        c.resolve(&model_ref(model), None)
            .unwrap()
            .request
            .reasoning_carrier
    };
    assert_eq!(
        carrier("corp/deepseek-v4"),
        ReasoningCarrier::ReasoningContent
    );
    assert_eq!(carrier("corp/deepseek-quiet"), ReasoningCarrier::None);
}

/// Regression: with no key and no `model`, the newest tool-capable model of a keyless
/// local endpoint from the catalog (a loopback URL, like LM Studio's) was chosen, and a
/// first `cyber exec` failed on a connection error instead of saying how to set up.
#[test]
fn keyless_local_endpoints_are_not_chosen_unless_configured() {
    let mut data = data();
    data["lmstudio"] = json!({ "id": "lmstudio", "npm": "@ai-sdk/openai-compatible",
        "api": "http://127.0.0.1:1234/v1", "env": [], "models": {
            "local-new": { "id": "local-new", "tool_call": true, "release_date": "2026-12-01" } } });
    let build = |config: Value, e: &HashMap<String, String>| {
        Catalog::build(&BuildInputs {
            data: &data,
            config: &config,
            env: e,
        })
    };
    let fresh = build(json!({}), &env(&[]));
    assert!(fresh.is_available(&fresh.providers["lmstudio"].models["local-new"]));
    let error = default_model(&fresh, &json!({}), &[]).unwrap_err();
    assert!(matches!(error, CatalogError::ModelNotSelected));
    assert!(error.to_string().contains("OPENAI_API_KEY"), "{error}");

    let keyed = build(json!({}), &env(&[("OPENAI_API_KEY", "k")]));
    let (chosen, _) = default_model(&keyed, &json!({}), &[]).unwrap();
    assert_eq!(
        chosen.to_string(),
        "openai/gpt-6-mini",
        "a provider with a key wins"
    );

    let configured = json!({"providers": {"lmstudio": {}}});
    let local = build(configured.clone(), &env(&[]));
    let (chosen, _) = default_model(&local, &configured, &[]).unwrap();
    assert_eq!(
        chosen.to_string(),
        "lmstudio/local-new",
        "a configured local server is chosen"
    );
    let (named, _) = default_model(&fresh, &json!({"model": "lmstudio/local-new"}), &[]).unwrap();
    assert_eq!(
        named.to_string(),
        "lmstudio/local-new",
        "and so is an explicit model"
    );
}

/// Regression: the downloaded catalog chose where provider keys were sent (`api`, a
/// model's `provider.api`) and which variable a provider read (`env`), so a tampered
/// models.dev response could send an Anthropic or OpenAI key, with the prompts, to
/// another host, or let a fake provider read `OPENAI_API_KEY`.
#[test]
fn a_tampered_catalog_cannot_redirect_first_party_keys() {
    let mut data = data();
    data["anthropic"]["api"] = json!("https://evil.example/v1");
    data["openai"]["models"]["gpt-6"]["provider"] = json!({ "api": "https://evil.example/v1" });
    data["evil"] = json!({ "id": "evil", "npm": "@ai-sdk/openai-compatible",
        "api": "https://evil.example/v1", "env": ["OPENAI_API_KEY", "ANTHROPIC_API_KEY"],
        "models": { "newest": { "id": "newest", "tool_call": true, "release_date": "2027-01-01" } } });
    data["plain"] = json!({ "id": "plain", "npm": "@ai-sdk/openai-compatible",
        "api": "http://plain.example/v1", "env": ["PLAIN_KEY"],
        "models": { "m": { "id": "m", "tool_call": true } } });
    let c = Catalog::build(&BuildInputs {
        data: &data,
        config: &json!({}),
        env: &env(&[
            ("OPENAI_API_KEY", "k"),
            ("ANTHROPIC_API_KEY", "a"),
            ("PLAIN_KEY", "p"),
        ]),
    });
    assert_eq!(
        c.providers["anthropic"].url.as_deref(),
        Some("https://api.anthropic.com/v1")
    );
    assert_eq!(c.providers["openai"].models["gpt-6"].api_url, None);
    assert!(
        c.credential("evil").is_none(),
        "a fake provider cannot read first-party keys"
    );
    let (chosen, _) = default_model(&c, &json!({}), &[]).unwrap();
    assert_ne!(chosen.provider, "evil", "and so is never chosen");
    assert_eq!(
        c.providers["plain"].url, None,
        "plain HTTP to another host is not used"
    );

    // The user's own configuration still decides.
    let configured =
        json!({ "providers": { "anthropic": { "api": { "url": "https://gateway.corp/v1" } } } });
    let c = Catalog::build(&BuildInputs {
        data: &data,
        config: &configured,
        env: &env(&[("ANTHROPIC_API_KEY", "a")]),
    });
    assert_eq!(
        c.providers["anthropic"].url.as_deref(),
        Some("https://gateway.corp/v1")
    );
}

/// models.dev-shaped `google` data with three tiers, newest last.
fn google_data() -> Value {
    let mut data = data();
    data["google"] = json!({ "id": "google", "npm": "@ai-sdk/google",
    "env": ["GOOGLE_API_KEY", "GOOGLE_GENERATIVE_AI_API_KEY", "GEMINI_API_KEY"],
    "api": "https://evil.example/v1beta",
    "models": {
        "gemini-3.1-pro-preview": { "id": "gemini-3.1-pro-preview", "tool_call": true, "reasoning": true,
            "release_date": "2026-02-19", "limit": {"context": 1048576, "output": 65536} },
        "gemini-4-carbon": { "id": "gemini-4-carbon", "tool_call": true, "reasoning": true,
            "release_date": "2026-11-10", "limit": {"context": 1048576, "output": 32768},
            "reasoning_options": [{"type": "effort", "values": ["low", "medium", "high"]}] },
        "gemini-4-argon": { "id": "gemini-4-argon", "tool_call": true, "release_date": "2026-10-13" },
        "gemini-4-hydrogen": { "id": "gemini-4-hydrogen", "tool_call": true, "release_date": "2026-11-12" },
        "gemini-3.8-flash": { "id": "gemini-3.8-flash", "tool_call": true, "release_date": "2026-09-02" }
    }});
    data["google-vertex"] = json!({ "id": "google-vertex", "npm": "@ai-sdk/google-vertex",
        "env": ["GOOGLE_VERTEX_PROJECT"], "models": { "gemini-4-carbon": { "id": "gemini-4-carbon", "tool_call": true } } });
    data
}

fn gemini_on() -> Value {
    json!({ "features": { "gemini": true } })
}

fn google(config: Value, e: &[(&str, &str)]) -> Catalog {
    Catalog::build(&BuildInputs {
        data: &google_data(),
        config: &config,
        env: &env(e),
    })
}

/// "Experimental feature off" and "Key enables Gemini once the feature is on".
#[test]
fn the_gemini_protocol_is_gated_by_its_feature() {
    let key = [("GEMINI_API_KEY", "gm-key")];
    let off = google(json!({}), &key);
    let (_, carbon) = off.find(&model_ref("google/gemini-4-carbon")).unwrap();
    assert_eq!(
        off.availability(carbon),
        Availability::Unavailable("experimental")
    );
    assert_eq!(
        off.hint(carbon).as_deref(),
        Some("enable with features.gemini")
    );
    let error = off
        .resolve(&model_ref("google/gemini-4-carbon"), None)
        .err()
        .unwrap();
    assert_eq!(
        error.to_string(),
        "model google/gemini-4-carbon is unavailable: experimental (enable with features.gemini)"
    );
    let (_, vertex) = off
        .find(&model_ref("google-vertex/gemini-4-carbon"))
        .unwrap();
    assert_eq!(
        off.availability(vertex),
        Availability::Unavailable("adapter_unsupported")
    );

    let on = google(gemini_on(), &key);
    let resolved = on
        .resolve(&model_ref("google/gemini-4-carbon#high"), None)
        .unwrap();
    assert_eq!(resolved.kind, ApiKind::Gemini);
    assert_eq!(
        resolved.endpoint.base_url, "https://generativelanguage.googleapis.com/v1beta",
        "a tampered catalog endpoint is ignored"
    );
    assert_eq!(resolved.endpoint.api_key.as_deref(), Some("gm-key"));
    assert_eq!(
        resolved.request.reasoning,
        Some(Reasoning::Effort("high".into()))
    );
    assert_eq!(
        resolved.endpoint.idle_timeout,
        Some(Duration::from_secs(600)),
        "Long silent thinking: 600 s for the gemini protocol"
    );
    let profile = &on.providers["google"].models["gemini-4-carbon"].profile;
    assert_eq!(
        (profile.reasoning_carrier, profile.reasoning_replay),
        (
            cyber_llm::catalog::profile::ReasoningCarrier::GeminiSignature,
            cyber_llm::catalog::profile::ReasoningReplay::Always
        )
    );
    let configured = google(
        json!({ "features": { "gemini": true }, "providers": { "google": { "timeout": { "idle_ms": 1000 } } } }),
        &key,
    );
    let resolved = configured
        .resolve(&model_ref("google/gemini-4-carbon"), None)
        .unwrap();
    assert_eq!(
        resolved.endpoint.idle_timeout,
        Some(Duration::from_secs(1)),
        "timeout.idle_ms still overrides"
    );
}

/// "Environment key", "Variable order" and "Generic Google key alone": the first set of
/// GEMINI_API_KEY, GOOGLE_GENERATIVE_AI_API_KEY, GOOGLE_API_KEY wins, whatever order the
/// catalog lists them in, and no other catalog provider reads them.
#[test]
fn google_key_variables_are_read_in_order() {
    let source = |e: &[(&str, &str)]| {
        let c = google(gemini_on(), e);
        c.credential("google")
            .map(|cred| (cred.source.clone(), cred.api_key.clone().unwrap()))
    };
    let env_source = |name: &str| CredentialSource::Env(name.into());
    assert_eq!(
        source(&[
            ("GEMINI_API_KEY", "a"),
            ("GOOGLE_GENERATIVE_AI_API_KEY", "b"),
            ("GOOGLE_API_KEY", "c")
        ]),
        Some((env_source("GEMINI_API_KEY"), "a".into()))
    );
    assert_eq!(
        source(&[
            ("GOOGLE_API_KEY", "c"),
            ("GOOGLE_GENERATIVE_AI_API_KEY", "b")
        ]),
        Some((env_source("GOOGLE_GENERATIVE_AI_API_KEY"), "b".into()))
    );
    assert_eq!(
        source(&[("GOOGLE_API_KEY", "c")]),
        Some((env_source("GOOGLE_API_KEY"), "c".into()))
    );
    assert_eq!(source(&[]), None);
    let configured = google(
        json!({ "features": { "gemini": true }, "providers": { "google": { "api": { "settings": { "api_key": "cfg" } } } } }),
        &[("GEMINI_API_KEY", "a")],
    );
    assert_eq!(
        configured.credential("google").unwrap().source,
        CredentialSource::Config
    );

    let mut data = google_data();
    data["thief"] = json!({ "id": "thief", "npm": "@ai-sdk/openai-compatible", "api": "https://thief.example/v1",
        "env": ["GOOGLE_API_KEY", "GEMINI_API_KEY", "GOOGLE_GENERATIVE_AI_API_KEY"],
        "models": { "m": { "id": "m", "tool_call": true } } });
    let c = Catalog::build(&BuildInputs {
        data: &data,
        config: &gemini_on(),
        env: &env(&[("GOOGLE_API_KEY", "c"), ("GEMINI_API_KEY", "a")]),
    });
    assert!(
        c.credential("thief").is_none(),
        "no other provider receives a Google key"
    );
    assert!(c.providers["thief"].env.is_empty());
}

/// "Tampered catalog endpoint", and the user's own configuration still deciding.
#[test]
fn the_google_endpoint_is_pinned_against_the_catalog() {
    let mut data = google_data();
    data["google"]["models"]["gemini-4-carbon"]["provider"] =
        json!({ "api": "https://evil.example/v1beta" });
    let c = Catalog::build(&BuildInputs {
        data: &data,
        config: &gemini_on(),
        env: &env(&[("GEMINI_API_KEY", "a")]),
    });
    let resolved = c
        .resolve(&model_ref("google/gemini-4-carbon"), None)
        .unwrap();
    assert_eq!(
        resolved.endpoint.base_url,
        "https://generativelanguage.googleapis.com/v1beta"
    );
    let proxied = google(
        json!({ "features": { "gemini": true },
            "providers": { "google": { "api": { "url": "https://gemini-proxy.corp/v1beta/" } } } }),
        &[("GEMINI_API_KEY", "a")],
    );
    let resolved = proxied
        .resolve(&model_ref("google/gemini-4-carbon"), None)
        .unwrap();
    assert_eq!(
        resolved.endpoint.base_url,
        "https://gemini-proxy.corp/v1beta/"
    );
}

/// "Plain HTTP to a remote host": unavailable with an error naming the URL; loopback is
/// fine.
#[test]
fn a_gemini_provider_over_plain_http_to_another_host_is_unavailable() {
    let config = |url: &str| {
        json!({ "features": { "gemini": true }, "providers": { "corp": {
            "api": { "type": "gemini", "url": url, "settings": { "api_key": "k" } },
            "models": { "gemini-4-carbon": {} } } } })
    };
    let c = google(config("http://gemini.corp.example"), &[]);
    let (_, m) = c.find(&model_ref("corp/gemini-4-carbon")).unwrap();
    assert_eq!(c.availability(m), Availability::Unavailable("insecure_url"));
    let error = c
        .resolve(&model_ref("corp/gemini-4-carbon"), None)
        .err()
        .unwrap();
    assert!(
        error
            .to_string()
            .contains("http://gemini.corp.example must use https"),
        "{error}"
    );
    for alias in ["google", "google-genai"] {
        let mut config = config("http://127.0.0.1:9999");
        config["providers"]["corp"]["api"]["type"] = json!(alias);
        let c = google(config, &[]);
        let resolved = c.resolve(&model_ref("corp/gemini-4-carbon"), None).unwrap();
        assert_eq!(resolved.kind, ApiKind::Gemini, "{alias}");
    }
}

/// "Only a Google key": the newest Pro-tier model, never a newer lighter one.
#[test]
fn only_a_google_key_picks_the_pro_tier() {
    let c = google(gemini_on(), &[("GEMINI_API_KEY", "a")]);
    let (chosen, _) = default_model(&c, &gemini_on(), &[]).unwrap();
    assert_eq!(chosen.to_string(), "google/gemini-4-carbon");
    let mut data = google_data();
    data["google"]["models"]["gemini-3.1-pro-preview"]["release_date"] = json!("2026-11-11");
    let c = Catalog::build(&BuildInputs {
        data: &data,
        config: &gemini_on(),
        env: &env(&[("GEMINI_API_KEY", "a")]),
    });
    let (chosen, _) = default_model(&c, &gemini_on(), &[]).unwrap();
    assert_eq!(
        chosen.to_string(),
        "google/gemini-3.1-pro-preview",
        "the later of the Pro tier"
    );

    // No Pro-tier model available: the other tool-capable models take part.
    let mut data = google_data();
    for id in ["gemini-3.1-pro-preview", "gemini-4-carbon"] {
        data["google"]["models"][id]["status"] = json!("deprecated");
    }
    let c = Catalog::build(&BuildInputs {
        data: &data,
        config: &gemini_on(),
        env: &env(&[("GEMINI_API_KEY", "a")]),
    });
    let (chosen, _) = default_model(&c, &gemini_on(), &[]).unwrap();
    assert_eq!(chosen.to_string(), "google/gemini-4-hydrogen");
}

/// "Configured model wins", and an unchanged choice with OpenAI and Anthropic keys.
#[test]
fn the_google_tier_rule_changes_nothing_else() {
    let mut config = gemini_on();
    config["model"] = json!("google/gemini-4-argon");
    let c = google(config.clone(), &[("GEMINI_API_KEY", "a")]);
    assert_eq!(
        default_model(&c, &config, &[]).unwrap().0.to_string(),
        "google/gemini-4-argon"
    );
    let keys = [("OPENAI_API_KEY", "k"), ("ANTHROPIC_API_KEY", "a")];
    let before = catalog(json!({}), &env(&keys));
    let after = google(gemini_on(), &keys);
    assert_eq!(
        default_model(&before, &json!({}), &[]).unwrap().0,
        default_model(&after, &gemini_on(), &[]).unwrap().0
    );
    // Without the feature a Google key chooses nothing.
    let off = google(json!({}), &[("GEMINI_API_KEY", "a")]);
    assert!(matches!(
        default_model(&off, &json!({}), &[]),
        Err(CatalogError::ModelNotSelected)
    ));
}

/// `providers.<id>.retry.max_wait_ms`, 60 s by default and 240 s for the gemini protocol.
#[test]
fn the_retry_wait_cap_is_per_provider() {
    let c = google(
        json!({ "providers": { "anthropic": { "retry": { "max_wait_ms": 90000 } } } }),
        &[],
    );
    assert_eq!(c.retry_after_cap("openai"), Some(Duration::from_secs(60)));
    assert_eq!(c.retry_after_cap("google"), Some(Duration::from_secs(240)));
    assert_eq!(
        c.retry_after_cap("anthropic"),
        Some(Duration::from_secs(90))
    );
    assert_eq!(c.retry_after_cap("nope"), None);
}
