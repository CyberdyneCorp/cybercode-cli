//! The `chatgpt` provider a stored ChatGPT login adds (`provider-catalog` → ChatGPT
//! subscription provider). Its models are the ones the backend lists for the account,
//! cached beside the login; they are unpriced because the plan pays.

use std::collections::BTreeMap;

use super::{
    Capabilities, Limits, Model, ModelProfile, ProfilePatch, Provider, RequestOverlay, Variant,
};
use crate::adapters::ApiKind;
use crate::chatgpt::{PROVIDER_ID, PlanModel, Session};
use crate::types::Reasoning;

pub fn insert(providers: &mut BTreeMap<String, Provider>, session: &Session) {
    let models = session
        .models()
        .load()
        .iter()
        .map(|plan| (plan.slug.clone(), model(plan)))
        .collect();
    providers.insert(
        PROVIDER_ID.into(),
        Provider {
            id: PROVIDER_ID.into(),
            name: "ChatGPT (subscription)".into(),
            kind: Some(ApiKind::ChatgptCodex),
            url: Some(session.backend.clone()),
            env: Vec::new(),
            api_key: None,
            auth_none: false,
            request: RequestOverlay::default(),
            idle_ms: None,
            disabled: false,
            models,
        },
    );
}

fn model(plan: &PlanModel) -> Model {
    let variants = plan
        .efforts
        .iter()
        .map(|effort| {
            let variant = Variant {
                reasoning: Some(Reasoning::Effort(effort.clone())),
                request: RequestOverlay::default(),
            };
            (effort.clone(), variant)
        })
        .collect();
    Model {
        id: plan.slug.clone(),
        provider_id: PROVIDER_ID.into(),
        name: plan.name.clone(),
        family: None,
        api_id: plan.slug.clone(),
        api_kind: None,
        api_url: None,
        capabilities: Capabilities {
            tools: true,
            vision: plan.vision,
            pdf: false,
            reasoning: !plan.efforts.is_empty(),
            structured_output: true,
            temperature: false,
            prompt_cache: true,
            prefers_apply_patch: plan.slug.starts_with("gpt-"),
        },
        limits: Limits {
            context: plan.context_window,
            input: None,
            output: 0,
        },
        cost: None,
        variants,
        status: None,
        released_at: None,
        disabled: false,
        request: RequestOverlay::default(),
        profile: ModelProfile::default(),
        profile_catalog: plan.profile.clone(),
        profile_config: ProfilePatch::default(),
    }
}
