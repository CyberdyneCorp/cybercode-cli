//! Merging the `providers` config key over catalog data
//! (`provider-catalog` → Configured providers and models).

use std::collections::BTreeMap;

use serde_json::{Map, Value};

use super::{Capabilities, Limits, Model, Provider, RequestOverlay, Variant, models_dev};
use crate::adapters::ApiKind;
use crate::types::Reasoning;

pub fn apply(providers: &mut BTreeMap<String, Provider>, configured: &Map<String, Value>) {
    for (id, value) in configured {
        let provider = providers
            .entry(id.clone())
            .or_insert_with(|| custom_provider(id));
        apply_provider(provider, value);
    }
}

fn custom_provider(id: &str) -> Provider {
    Provider {
        id: id.into(),
        name: id.into(),
        kind: None,
        url: None,
        env: Vec::new(),
        api_key: None,
        auth_none: false,
        request: RequestOverlay::default(),
        disabled: false,
        models: BTreeMap::new(),
    }
}

fn apply_provider(p: &mut Provider, v: &Value) {
    if let Some(name) = v.get("name").and_then(Value::as_str) {
        p.name = name.into();
    }
    if let Some(api) = v.get("api") {
        apply_api(p, api);
    }
    if let Some(env) = v.get("env").and_then(Value::as_array) {
        p.env = env
            .iter()
            .filter_map(|e| e.as_str().map(str::to_string))
            .collect();
    }
    p.request
        .layer(&RequestOverlay::from_config(v.get("request")));
    if let Some(disabled) = v.get("disabled").and_then(Value::as_bool) {
        p.disabled = disabled;
    }
    for (mid, mv) in v
        .get("models")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
    {
        let kind = p.kind;
        let model = p
            .models
            .entry(mid.clone())
            .or_insert_with(|| custom_model(&p.id, mid, kind));
        apply_model(model, mv);
    }
}

fn apply_api(p: &mut Provider, api: &Value) {
    if let Some(kind) = api.get("type").and_then(Value::as_str) {
        p.kind = ApiKind::parse(kind);
    }
    if let Some(url) = api.get("url").and_then(Value::as_str) {
        p.url = Some(url.into());
    }
    if p.url.is_none() {
        p.url = p.kind.and_then(ApiKind::default_url).map(str::to_string);
    }
    let settings = &api["settings"];
    if let Some(key) = settings
        .get("api_key")
        .and_then(Value::as_str)
        .filter(|k| !k.is_empty())
    {
        p.api_key = Some(key.into());
    }
    p.auth_none |= settings.get("auth").and_then(Value::as_str) == Some("none");
}

/// A model absent from the catalog: text in/out, tools enabled, unknown pricing.
fn custom_model(provider_id: &str, id: &str, kind: Option<ApiKind>) -> Model {
    Model {
        id: id.into(),
        provider_id: provider_id.into(),
        name: id.into(),
        family: None,
        api_id: id.into(),
        api_kind: None,
        api_url: None,
        capabilities: Capabilities {
            tools: true,
            prefers_apply_patch: kind == Some(ApiKind::OpenaiResponses) && id.starts_with("gpt-"),
            ..Capabilities::default()
        },
        limits: Limits::default(),
        cost: None,
        variants: BTreeMap::new(),
        status: None,
        released_at: None,
        disabled: false,
        request: RequestOverlay::default(),
    }
}

fn apply_model(m: &mut Model, v: &Value) {
    if let Some(name) = v.get("name").and_then(Value::as_str) {
        m.name = name.into();
    }
    if let Some(api_id) = v.get("id").and_then(Value::as_str) {
        m.api_id = api_id.into();
    }
    apply_limits(&mut m.limits, v.get("limits"));
    apply_capabilities(&mut m.capabilities, v.get("capabilities"));
    if let Some(cost) = v.get("cost") {
        m.cost = models_dev::cost(cost);
    }
    if let Some(disabled) = v.get("disabled").and_then(Value::as_bool) {
        m.disabled = disabled;
    }
    m.request
        .layer(&RequestOverlay::from_config(v.get("request")));
    for (name, variant) in v
        .get("variants")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
    {
        apply_variant(&mut m.variants, name, variant);
    }
}

fn apply_limits(limits: &mut Limits, v: Option<&Value>) {
    let Some(v) = v else { return };
    let get = |k: &str| v.get(k).and_then(Value::as_u64);
    limits.context = get("context").unwrap_or(limits.context);
    limits.output = get("output").unwrap_or(limits.output);
    limits.input = get("input").or(limits.input);
}

fn apply_capabilities(c: &mut Capabilities, v: Option<&Value>) {
    let Some(map) = v.and_then(Value::as_object) else {
        return;
    };
    for (key, value) in map {
        let Some(flag) = value.as_bool() else {
            continue;
        };
        let slot = match key.as_str() {
            "tools" => &mut c.tools,
            "vision" => &mut c.vision,
            "pdf" => &mut c.pdf,
            "reasoning" => &mut c.reasoning,
            "structured_output" => &mut c.structured_output,
            "temperature" => &mut c.temperature,
            "prompt_cache" => &mut c.prompt_cache,
            "prefers_apply_patch" => &mut c.prefers_apply_patch,
            _ => continue,
        };
        *slot = flag;
    }
}

/// `{ effort | budget_tokens | reasoning: "off", request?, disabled? }` merged by name.
fn apply_variant(variants: &mut BTreeMap<String, Variant>, name: &str, v: &Value) {
    if v.get("disabled").and_then(Value::as_bool) == Some(true) {
        variants.remove(name);
        return;
    }
    let entry = variants.entry(name.into()).or_insert_with(|| Variant {
        reasoning: None,
        request: RequestOverlay::default(),
    });
    if let Some(effort) = v.get("effort").and_then(Value::as_str) {
        entry.reasoning = Some(Reasoning::Effort(effort.into()));
    } else if let Some(n) = v.get("budget_tokens").and_then(Value::as_u64) {
        entry.reasoning = Some(Reasoning::BudgetTokens(
            u32::try_from(n).unwrap_or(u32::MAX),
        ));
    } else if v.get("reasoning").and_then(Value::as_str) == Some("off") {
        entry.reasoning = Some(Reasoning::Off);
    }
    entry
        .request
        .layer(&RequestOverlay::from_config(v.get("request")));
}
