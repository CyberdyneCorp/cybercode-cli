//! Parsing of the models.dev `api.json` document.

use std::collections::BTreeMap;

use serde_json::Value;

use super::{Capabilities, Cost, CostTier, Limits, Model, Provider, RequestOverlay, Variant};
use crate::adapters::ApiKind;
use crate::json::str_at;
use crate::types::Reasoning;

pub fn parse(data: &Value) -> BTreeMap<String, Provider> {
    data.as_object()
        .into_iter()
        .flatten()
        .map(|(id, v)| (id.clone(), provider(id, v)))
        .collect()
}

fn provider(id: &str, v: &Value) -> Provider {
    let kind = v
        .get("npm")
        .and_then(Value::as_str)
        .and_then(ApiKind::from_npm);
    let url = v
        .get("api")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| kind.and_then(ApiKind::default_url).map(str::to_string));
    let models = v
        .get("models")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
        .map(|(mid, m)| (mid.clone(), model(id, mid, m, kind)))
        .collect();
    Provider {
        id: id.into(),
        name: v.get("name").and_then(Value::as_str).unwrap_or(id).into(),
        kind,
        url,
        env: v
            .get("env")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|e| e.as_str().map(str::to_string))
            .collect(),
        api_key: None,
        auth_none: false,
        request: RequestOverlay::default(),
        disabled: false,
        models,
    }
}

fn model(provider_id: &str, id: &str, v: &Value, provider_kind: Option<ApiKind>) -> Model {
    let flag = |key: &str| v.get(key).and_then(Value::as_bool).unwrap_or(false);
    let input = |modality: &str| {
        v.pointer("/modalities/input")
            .and_then(Value::as_array)
            .is_some_and(|a| a.iter().any(|m| m == modality))
    };
    let api_kind = v
        .pointer("/provider/npm")
        .and_then(Value::as_str)
        .and_then(ApiKind::from_npm);
    let kind = api_kind.or(provider_kind);
    let cost = v.get("cost").and_then(cost);
    let limits = Limits {
        context: v
            .pointer("/limit/context")
            .and_then(Value::as_u64)
            .unwrap_or(0),
        input: v.pointer("/limit/input").and_then(Value::as_u64),
        output: v
            .pointer("/limit/output")
            .and_then(Value::as_u64)
            .unwrap_or(0),
    };
    Model {
        id: id.into(),
        provider_id: provider_id.into(),
        name: v.get("name").and_then(Value::as_str).unwrap_or(id).into(),
        family: v.get("family").and_then(Value::as_str).map(str::to_string),
        api_id: id.into(),
        api_kind,
        api_url: v
            .pointer("/provider/api")
            .and_then(Value::as_str)
            .map(str::to_string),
        capabilities: Capabilities {
            tools: flag("tool_call"),
            vision: input("image"),
            pdf: input("pdf"),
            reasoning: flag("reasoning"),
            structured_output: flag("structured_output"),
            temperature: flag("temperature"),
            prompt_cache: cost.as_ref().is_some_and(|c| c.cache_read.is_some()),
            prefers_apply_patch: kind == Some(ApiKind::OpenaiResponses) && id.starts_with("gpt-"),
        },
        limits,
        cost,
        variants: variants(v, flag("reasoning")),
        status: v.get("status").and_then(Value::as_str).map(str::to_string),
        released_at: v
            .get("release_date")
            .and_then(Value::as_str)
            .map(str::to_string),
        disabled: false,
        request: RequestOverlay::default(),
    }
}

pub(super) fn cost(v: &Value) -> Option<Cost> {
    let price = |key: &str| v.get(key).and_then(Value::as_f64);
    let (input, output) = (price("input")?, price("output")?);
    let mut tiers: Vec<CostTier> = v
        .get("tiers")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|t| tier(t, t.pointer("/tier/size").and_then(Value::as_u64)?))
        .collect();
    if let Some(over) = v.get("context_over_200k") {
        tiers.extend(tier(over, 200_000));
    }
    tiers.sort_by_key(|t| t.above_input_tokens);
    Some(Cost {
        input,
        output,
        reasoning: price("reasoning"),
        cache_read: price("cache_read"),
        cache_write: price("cache_write"),
        tiers,
    })
}

fn tier(v: &Value, above: u64) -> Option<CostTier> {
    Some(CostTier {
        above_input_tokens: above,
        input: v.get("input")?.as_f64()?,
        output: v.get("output")?.as_f64()?,
        cache_read: v.get("cache_read").and_then(Value::as_f64),
        cache_write: v.get("cache_write").and_then(Value::as_f64),
    })
}

/// Reasoning variants from `reasoning_options`, or low/medium/high efforts by default.
fn variants(v: &Value, reasoning: bool) -> BTreeMap<String, Variant> {
    if !reasoning {
        return BTreeMap::new();
    }
    let mut out = BTreeMap::new();
    for option in v
        .get("reasoning_options")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        match str_at(option, "/type") {
            "effort" => effort_variants(option, &mut out),
            "budget_tokens" => budget_variants(option.get("max").and_then(Value::as_u64), &mut out),
            "toggle" => {
                out.insert("off".into(), variant(Reasoning::Off));
                out.insert("on".into(), variant(Reasoning::Effort("medium".into())));
            }
            _ => {}
        }
    }
    if out.is_empty() {
        for level in ["low", "medium", "high"] {
            out.insert(level.into(), variant(Reasoning::Effort(level.into())));
        }
    }
    out
}

fn effort_variants(option: &Value, out: &mut BTreeMap<String, Variant>) {
    for value in option
        .get("values")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
    {
        let reasoning = if value == "none" {
            Reasoning::Off
        } else {
            Reasoning::Effort(value.into())
        };
        out.insert(value.into(), variant(reasoning));
    }
}

fn budget_variants(max: Option<u64>, out: &mut BTreeMap<String, Variant>) {
    let cap = max.unwrap_or(64_000);
    for (name, tokens) in [
        ("low", 4_096),
        ("medium", 16_384),
        ("high", 32_768),
        ("max", cap),
    ] {
        let budget = u32::try_from(tokens.min(cap)).unwrap_or(u32::MAX);
        out.insert(name.into(), variant(Reasoning::BudgetTokens(budget)));
    }
}

fn variant(reasoning: Reasoning) -> Variant {
    Variant {
        reasoning: Some(reasoning),
        request: RequestOverlay::default(),
    }
}
