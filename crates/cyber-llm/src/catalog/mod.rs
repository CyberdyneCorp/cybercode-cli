//! The provider catalog (`provider-catalog`): records, availability and resolution.

mod config;
mod cost;
mod credentials;
mod model_ref;
mod models_dev;
mod roles;
mod source;

use std::collections::BTreeMap;

use cyber_core::env::EnvSource;
use serde::Serialize;
use serde_json::Value;

use crate::adapters::{self, Adapter, ApiKind, Endpoint};
use crate::json::{deep_merge, strip_credentials};
use crate::types::{LlmRequest, Reasoning};

pub use cost::compute as compute_cost;
pub use credentials::{Credential, CredentialSource};
pub use model_ref::ModelRef;
pub use roles::{ModelRole, default_model, recent_models, record_recent, role_ref};
pub use source::{Loaded, Origin, SourceOptions, load as load_source};

#[derive(Debug, thiserror::Error)]
pub enum CatalogError {
    #[error("Provider not found: {0}")]
    ProviderNotFound(String),
    #[error("Model not found: {0}")]
    ModelNotFound(String),
    #[error("invalid model reference {0:?}: expected provider/model[#variant]")]
    InvalidRef(String),
    #[error("model {model} is unavailable: {reason}")]
    Unavailable { model: String, reason: String },
    #[error("VariantUnavailableError: {model} has no variant {variant:?} (available: {available})")]
    VariantUnavailable {
        model: String,
        variant: String,
        available: String,
    },
    #[error("ModelNotSelectedError: no available model; connect a provider or configure one")]
    ModelNotSelected,
    #[error("catalog source: {0}")]
    Source(String),
}

/// Extra headers and body merged into requests, in layer order.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct RequestOverlay {
    pub headers: BTreeMap<String, String>,
    pub body: Value,
}

impl RequestOverlay {
    pub fn from_config(value: Option<&Value>) -> Self {
        let Some(value) = value else {
            return Self::default();
        };
        let headers = value
            .get("headers")
            .and_then(Value::as_object)
            .map(|h| {
                h.iter()
                    .filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_string())))
                    .collect()
            })
            .unwrap_or_default();
        Self {
            headers,
            body: value.get("body").cloned().unwrap_or(Value::Null),
        }
    }

    pub fn layer(&mut self, other: &RequestOverlay) {
        for (name, value) in &other.headers {
            self.headers
                .retain(|key, _| !key.eq_ignore_ascii_case(name));
            self.headers.insert(name.clone(), value.clone());
        }
        if self.body.is_null() {
            self.body = other.body.clone();
        } else {
            deep_merge(&mut self.body, &other.body);
        }
    }

    /// Apply a later layer to an already resolved provider/model/variant template.
    pub fn apply_to(&self, request: &mut LlmRequest) {
        if self.headers.is_empty() && self.body.is_null() {
            return;
        }
        deep_merge(&mut request.body, &self.body);
        strip_credentials(&mut request.body);
        for (name, value) in &self.headers {
            request
                .headers
                .retain(|(key, _)| !key.eq_ignore_ascii_case(name));
            request.headers.push((name.clone(), value.clone()));
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Capabilities {
    pub tools: bool,
    pub vision: bool,
    pub pdf: bool,
    pub reasoning: bool,
    pub structured_output: bool,
    pub temperature: bool,
    pub prompt_cache: bool,
    pub prefers_apply_patch: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize)]
pub struct Limits {
    pub context: u64,
    pub input: Option<u64>,
    pub output: u64,
}

/// Prices in USD per million tokens.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Cost {
    pub input: f64,
    pub output: f64,
    pub reasoning: Option<f64>,
    pub cache_read: Option<f64>,
    pub cache_write: Option<f64>,
    /// Prices that apply once the prompt exceeds `above_input_tokens`.
    pub tiers: Vec<CostTier>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct CostTier {
    pub above_input_tokens: u64,
    pub input: f64,
    pub output: f64,
    pub cache_read: Option<f64>,
    pub cache_write: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Variant {
    pub reasoning: Option<Reasoning>,
    pub request: RequestOverlay,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Model {
    pub id: String,
    pub provider_id: String,
    pub name: String,
    pub family: Option<String>,
    /// Identifier sent to the provider.
    pub api_id: String,
    /// Per-model protocol override (models.dev `provider.npm`).
    pub api_kind: Option<ApiKind>,
    pub api_url: Option<String>,
    pub capabilities: Capabilities,
    pub limits: Limits,
    /// `None` means unknown pricing (`unpriced`), never zero.
    pub cost: Option<Cost>,
    pub variants: BTreeMap<String, Variant>,
    pub status: Option<String>,
    pub released_at: Option<String>,
    pub disabled: bool,
    pub request: RequestOverlay,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Provider {
    pub id: String,
    pub name: String,
    pub kind: Option<ApiKind>,
    pub url: Option<String>,
    /// Environment variables that carry the provider's key.
    pub env: Vec<String>,
    #[serde(skip)]
    pub api_key: Option<String>,
    /// `api.settings.auth: "none"`: no credential needed.
    pub auth_none: bool,
    pub request: RequestOverlay,
    pub disabled: bool,
    pub models: BTreeMap<String, Model>,
}

/// Why a model can or cannot be used.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "status", content = "reason", rename_all = "lowercase")]
pub enum Availability {
    Available,
    Unavailable(&'static str),
}

/// A model row for listings.
#[derive(Debug, Clone, Serialize)]
pub struct Listing<'a> {
    #[serde(rename = "ref")]
    pub model_ref: String,
    pub availability: Availability,
    pub model: &'a Model,
}

/// Fully resolved target for one request.
pub struct Resolved {
    pub model_ref: ModelRef,
    pub kind: ApiKind,
    pub endpoint: Endpoint,
    /// Request template: model ID, reasoning, output cap and layered overlays.
    pub request: LlmRequest,
}

impl Resolved {
    pub fn adapter(&self) -> Box<dyn Adapter> {
        adapters::adapter(self.kind, self.endpoint.clone())
    }
}

pub struct BuildInputs<'a> {
    /// models.dev `api.json` data.
    pub data: &'a Value,
    /// The resolved Cyber configuration.
    pub config: &'a Value,
    pub env: &'a dyn EnvSource,
}

pub struct Catalog {
    pub providers: BTreeMap<String, Provider>,
    credentials: BTreeMap<String, Credential>,
    experimental: bool,
}

impl Catalog {
    pub fn build(inputs: &BuildInputs<'_>) -> Self {
        let mut providers = models_dev::parse(inputs.data);
        if let Some(configured) = inputs.config.get("providers").and_then(Value::as_object) {
            config::apply(&mut providers, configured);
        }
        let credentials = credentials::resolve_all(&providers, inputs.env);
        Self {
            providers,
            credentials,
            experimental: inputs.env.flag("CYBER_ENABLE_EXPERIMENTAL_MODELS"),
        }
    }

    pub fn find(&self, model_ref: &ModelRef) -> Result<(&Provider, &Model), CatalogError> {
        let provider = self
            .providers
            .get(&model_ref.provider)
            .ok_or_else(|| CatalogError::ProviderNotFound(model_ref.provider.clone()))?;
        let model = provider
            .models
            .get(&model_ref.model)
            .ok_or_else(|| CatalogError::ModelNotFound(model_ref.base()))?;
        Ok((provider, model))
    }

    pub fn credential(&self, provider_id: &str) -> Option<&Credential> {
        self.credentials.get(provider_id)
    }

    pub fn availability(&self, model: &Model) -> Availability {
        let Some(provider) = self.providers.get(&model.provider_id) else {
            return Availability::Unavailable("provider_not_found");
        };
        let reason = if provider.disabled {
            Some("provider_disabled")
        } else if model.disabled {
            Some("disabled")
        } else if model.api_kind.or(provider.kind).is_none() {
            Some("adapter_unsupported")
        } else if model.api_url.as_ref().or(provider.url.as_ref()).is_none() {
            Some("missing_url")
        } else if !self.credentials.contains_key(&provider.id) {
            Some("missing_credentials")
        } else {
            status_reason(model.status.as_deref(), self.experimental)
        };
        reason.map_or(Availability::Available, Availability::Unavailable)
    }

    pub fn is_available(&self, model: &Model) -> bool {
        self.availability(model) == Availability::Available
    }

    /// Every model, available first, then by reference.
    pub fn list(&self, provider: Option<&str>) -> Result<Vec<Listing<'_>>, CatalogError> {
        if let Some(p) = provider.filter(|p| !self.providers.contains_key(*p)) {
            return Err(CatalogError::ProviderNotFound(p.to_string()));
        }
        let mut rows: Vec<Listing<'_>> = self
            .providers
            .values()
            .filter(|p| provider.is_none_or(|id| id == p.id))
            .flat_map(|p| p.models.values())
            .map(|m| Listing {
                model_ref: format!("{}/{}", m.provider_id, m.id),
                availability: self.availability(m),
                model: m,
            })
            .collect();
        rows.sort_by(|a, b| {
            let rank = |l: &Listing<'_>| l.availability != Availability::Available;
            rank(a)
                .cmp(&rank(b))
                .then_with(|| a.model_ref.cmp(&b.model_ref))
        });
        Ok(rows)
    }

    /// Resolve a model reference into an adapter target with layered request options:
    /// provider, model, variant, then `agent`.
    pub fn resolve(
        &self,
        model_ref: &ModelRef,
        agent: Option<&RequestOverlay>,
    ) -> Result<Resolved, CatalogError> {
        let (provider, model) = self.find(model_ref)?;
        if let Availability::Unavailable(reason) = self.availability(model) {
            return Err(CatalogError::Unavailable {
                model: model_ref.base(),
                reason: reason.into(),
            });
        }
        let variant = self.variant(model_ref, model)?;
        let mut overlay = provider.request.clone();
        overlay.layer(&model.request);
        if let Some(v) = variant {
            overlay.layer(&v.request);
        }
        if let Some(a) = agent {
            overlay.layer(a);
        }
        strip_credentials(&mut overlay.body);
        let url = model
            .api_url
            .clone()
            .or_else(|| provider.url.clone())
            .unwrap_or_default();
        let key = self
            .credentials
            .get(&provider.id)
            .and_then(|c| c.api_key.clone());
        let request = LlmRequest {
            model: model.api_id.clone(),
            max_output_tokens: (model.limits.output > 0)
                .then(|| model.limits.output.min(32_000) as u32),
            reasoning: variant.and_then(|v| v.reasoning.clone()),
            body: overlay.body,
            headers: overlay.headers.into_iter().collect(),
            cache: true,
            ..LlmRequest::default()
        };
        Ok(Resolved {
            model_ref: model_ref.clone(),
            kind: model
                .api_kind
                .or(provider.kind)
                .expect("checked by availability"),
            endpoint: Endpoint::new(url, key),
            request,
        })
    }

    fn variant<'m>(
        &self,
        model_ref: &ModelRef,
        model: &'m Model,
    ) -> Result<Option<&'m Variant>, CatalogError> {
        let Some(name) = &model_ref.variant else {
            return Ok(None);
        };
        model
            .variants
            .get(name)
            .map(Some)
            .ok_or_else(|| CatalogError::VariantUnavailable {
                model: model_ref.base(),
                variant: name.clone(),
                available: model
                    .variants
                    .keys()
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", "),
            })
    }
}

fn status_reason(status: Option<&str>, experimental: bool) -> Option<&'static str> {
    match status {
        Some("deprecated") => Some("deprecated"),
        Some("alpha") if !experimental => Some("experimental"),
        _ => None,
    }
}
