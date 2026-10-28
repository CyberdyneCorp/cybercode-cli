//! The `copilot` provider a stored GitHub Copilot login adds (`provider-catalog` → GitHub
//! Copilot subscription provider). Its models are the ones Copilot lists for the account,
//! cached beside the login; they are unpriced because the plan pays.

use std::collections::BTreeMap;

use super::{Capabilities, Limits, Model, ModelProfile, ProfilePatch, Provider, RequestOverlay};
use crate::adapters::ApiKind;
use crate::copilot::{API_URL, CopilotModel, PROVIDER_ID, Protocol, Session};

pub fn insert(providers: &mut BTreeMap<String, Provider>, session: &Session) {
    let listed = session.models().load();
    // The endpoint the last token named; requests always use the one minted for them.
    let url = listed
        .iter()
        .map(|model| model.api.as_str())
        .find(|api| !api.is_empty())
        .unwrap_or(API_URL);
    providers.insert(
        PROVIDER_ID.into(),
        Provider {
            id: PROVIDER_ID.into(),
            name: "GitHub Copilot (subscription)".into(),
            kind: Some(ApiKind::CopilotChat),
            url: Some(url.into()),
            env: Vec::new(),
            api_key: None,
            auth_none: false,
            request: RequestOverlay::default(),
            idle_ms: None,
            disabled: false,
            models: listed
                .iter()
                .map(|listed| (listed.slug.clone(), model(listed)))
                .collect(),
        },
    );
}

fn model(listed: &CopilotModel) -> Model {
    Model {
        id: listed.slug.clone(),
        provider_id: PROVIDER_ID.into(),
        name: listed.name.clone(),
        family: None,
        api_id: listed.slug.clone(),
        api_kind: None,
        api_url: None,
        capabilities: Capabilities {
            tools: listed.tools,
            vision: listed.vision,
            pdf: false,
            reasoning: false,
            structured_output: false,
            // Responses-only models (the GPT-5 family) reject a temperature.
            temperature: listed.protocol == Protocol::Chat,
            prompt_cache: false,
            prefers_apply_patch: listed.slug.starts_with("gpt-"),
        },
        limits: Limits {
            context: listed.context_window,
            input: None,
            output: listed.max_output,
        },
        cost: None,
        variants: BTreeMap::new(),
        status: None,
        released_at: None,
        disabled: false,
        request: RequestOverlay::default(),
        profile: ModelProfile::default(),
        profile_catalog: ProfilePatch::default(),
        profile_config: ProfilePatch::default(),
    }
}
