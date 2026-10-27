//! The official stateless text/function Responses contract. No token-estimation
//! heuristic is treated as a bound. Context is the catalog's declared model ceiling;
//! output/reasoning share the API's transmitted max_output_tokens allowance.

use super::super::Endpoint;
use crate::reservations::{
    BoundUnavailable, PreparedReservation, RequestBound, digest, unavailable,
};
use crate::{LlmRequest, Usage};
use serde_json::{Value, json};

pub(super) fn prepare(
    endpoint: &Endpoint,
    mut request: LlmRequest,
    context: u64,
) -> Result<PreparedReservation, BoundUnavailable> {
    official_route(endpoint, &request)?;
    if context == 0 {
        return Err(unavailable("the model has no declared context ceiling"));
    }
    pin_contract(&mut request)?;
    let body = super::build_body(&request);
    validate_body(&body, &request.model)?;
    let output = body["max_output_tokens"]
        .as_u64()
        .filter(|v| *v > 0 && *v <= context)
        .ok_or_else(|| {
            unavailable("a positive transmitted output cap within context is required")
        })?;
    let bound = RequestBound {
        classes: Usage {
            input: context,
            cache_read: context,
            output,
            reasoning: output,
            cache_write: 0,
        },
        total_tokens: context,
        input_group: context,
        output_group: output,
        source: "openai_responses_declared_context_default_tier_v1".into(),
        normalized_sha256: digest(&body)?,
    };
    Ok(PreparedReservation { request, bound })
}

fn official_route(endpoint: &Endpoint, request: &LlmRequest) -> Result<(), BoundUnavailable> {
    let url = reqwest::Url::parse(&endpoint.base_url)
        .map_err(|_| unavailable("invalid provider route"))?;
    if url.scheme() != "https"
        || url.host_str() != Some("api.openai.com")
        || url.port().is_some_and(|p| p != 443)
        || !url.username().is_empty()
        || url.password().is_some()
        || url.path().trim_end_matches('/') != "/v1"
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(unavailable(
            "reserved Responses requires the official OpenAI route; compatible gateways have no verified bound contract",
        ));
    }
    for (name, _) in endpoint.headers.iter().chain(&request.headers) {
        if !matches!(
            name.to_ascii_lowercase().as_str(),
            "openai-organization" | "openai-project"
        ) {
            return Err(unavailable(
                "custom routing or extension headers are unsupported for reserved requests",
            ));
        }
    }
    Ok(())
}

fn pin_contract(request: &mut LlmRequest) -> Result<(), BoundUnavailable> {
    if request.body.is_null() {
        request.body = json!({});
    }
    let body = request
        .body
        .as_object_mut()
        .ok_or_else(|| unavailable("request overlay must be an object"))?;
    for (key, value) in [
        ("service_tier", json!("default")),
        ("truncation", json!("disabled")),
        ("access_programs", json!({"cyber":"standard"})),
    ] {
        if body.get(key).is_some_and(|old| old != &value) {
            return Err(unavailable(
                "nonstandard service tier, access program or server context shaping is unsupported",
            ));
        }
        body.insert(key.into(), value);
    }
    Ok(())
}

fn validate_body(body: &Value, model: &str) -> Result<(), BoundUnavailable> {
    let fields = body
        .as_object()
        .ok_or_else(|| unavailable("normalized request is not an object"))?;
    const KEYS: &[&str] = &[
        "model",
        "input",
        "instructions",
        "tools",
        "stream",
        "store",
        "max_output_tokens",
        "temperature",
        "reasoning",
        "prompt_cache_key",
        "text",
        "tool_choice",
        "parallel_tool_calls",
        "service_tier",
        "truncation",
        "access_programs",
    ];
    if fields.keys().any(|key| !KEYS.contains(&key.as_str()))
        || body["model"] != model
        || body["stream"] != true
        || body["store"] != false
    {
        return Err(unavailable(
            "opaque extension, state, model override or nonstateless request cannot be reserved",
        ));
    }
    validate_input(&body["input"])?;
    if body.get("tools").is_some_and(|v| {
        v.as_array()
            .is_none_or(|tools| tools.iter().any(|t| t["type"] != "function"))
    }) {
        return Err(unavailable(
            "built-in or opaque provider tools have additional unbounded billing",
        ));
    }
    if body
        .get("tool_choice")
        .is_some_and(|v| v.is_object() && v["type"] != "function")
    {
        return Err(unavailable("opaque provider tool selection is unsupported"));
    }
    Ok(())
}

fn validate_input(input: &Value) -> Result<(), BoundUnavailable> {
    if input.is_string() {
        return Ok(());
    }
    let items = input
        .as_array()
        .ok_or_else(|| unavailable("opaque input is unsupported"))?;
    for item in items {
        match item["type"].as_str() {
            Some("function_call") if item["arguments"].is_string() => {}
            Some("function_call_output") if item["output"].is_string() => {}
            None | Some("message")
                if matches!(
                    item["role"].as_str(),
                    Some("user" | "assistant" | "system" | "developer")
                ) =>
            {
                if item["content"].is_string() {
                    continue;
                }
                let parts = item["content"]
                    .as_array()
                    .ok_or_else(|| unavailable("opaque message content is unsupported"))?;
                if parts.iter().any(|p| {
                    !matches!(p["type"].as_str(), Some("input_text" | "output_text"))
                        || !p["text"].is_string()
                }) {
                    return Err(unavailable(
                        "media, file or opaque content has no accepted token reservation bound",
                    ));
                }
            }
            _ => return Err(unavailable("opaque server-side input state is unsupported")),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Message;
    fn request() -> LlmRequest {
        LlmRequest {
            model: "gpt-test".into(),
            max_output_tokens: Some(100),
            messages: vec![Message::user_text("hello")],
            ..LlmRequest::default()
        }
    }
    #[test]
    fn bounds_are_from_final_body_and_pin_supported_billing_contract() {
        let endpoint = Endpoint::new("https://api.openai.com/v1", None);
        let mut request = request();
        request.body = json!({"max_output_tokens":50});
        let prepared = prepare(&endpoint, request, 1000).unwrap();
        assert_eq!(prepared.bound.classes.output, 50);
        assert_eq!(prepared.bound.total_tokens, 1000);
        assert_eq!(
            super::super::build_body(&prepared.request)["service_tier"],
            "default"
        );
        assert_eq!(
            prepared.bound.normalized_sha256,
            digest(&super::super::build_body(&prepared.request)).unwrap()
        );
    }
    #[test]
    fn bundled_openai_catalog_prices_admit_offline_production_shape() {
        let mut json = String::new();
        use std::io::Read;
        flate2::read::GzDecoder::new(&include_bytes!("../../../snapshot/models.json.gz")[..])
            .read_to_string(&mut json)
            .unwrap();
        let data: Value = serde_json::from_str(&json).unwrap();
        let env = std::collections::HashMap::<String, String>::new();
        let catalog = crate::catalog::Catalog::build(&crate::catalog::BuildInputs {
            data: &data,
            config: &json!({}),
            env: &env,
        });
        let model = &catalog.providers["openai"].models["gpt-4.1"];
        let request = LlmRequest {
            model: model.api_id.clone(),
            max_output_tokens: Some(1000),
            messages: vec![crate::Message::user_text("hello")],
            ..Default::default()
        };
        let prepared = prepare(
            &Endpoint::new("https://api.openai.com/v1", None),
            request,
            model.limits.context,
        )
        .unwrap();
        assert_eq!(prepared.bound.classes.cache_write, 0);
        let upper = prepared
            .bound
            .cost_microusd(model.cost.as_ref().unwrap())
            .unwrap();
        assert!(
            upper > 0 && upper <= 5_000_000,
            "the bundled priced model can fit a USD5 reservation"
        );
        assert!(prepared.bound.total_tokens >= 1000);
    }
    #[test]
    fn opaque_model_tier_state_tools_and_media_refuse() {
        let endpoint = Endpoint::new("https://api.openai.com/v1", None);
        for body in [
            json!({"model":"other"}),
            json!({"service_tier":"priority"}),
            json!({"previous_response_id":"r1"}),
            json!({"tools":[{"type":"web_search"}]}),
            json!({"input":[{"role":"user","content":[{"type":"input_image","image_url":"opaque"}]}]}),
        ] {
            let mut request = request();
            request.body = body;
            assert!(prepare(&endpoint, request, 1000).is_err());
        }
        assert!(prepare(&Endpoint::new("http://localhost/v1", None), request(), 1000).is_err());
        assert!(prepare(&endpoint, request(), 0).is_err());
    }
}
