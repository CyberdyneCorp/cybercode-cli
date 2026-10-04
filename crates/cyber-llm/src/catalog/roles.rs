//! Model roles and default model resolution
//! (`provider-catalog` → Model roles, Default model resolution).

use std::path::Path;

use serde_json::{Value, json};

use super::{Catalog, CatalogError, ModelRef};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelRole {
    Default,
    Small,
    Advisor,
    Evaluator,
    Compaction,
    Title,
}

impl ModelRole {
    pub fn key(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::Small => "small",
            Self::Advisor => "advisor",
            Self::Evaluator => "evaluator",
            Self::Compaction => "compaction",
            Self::Title => "title",
        }
    }

    pub const ALL: [ModelRole; 6] = [
        Self::Default,
        Self::Small,
        Self::Advisor,
        Self::Evaluator,
        Self::Compaction,
        Self::Title,
    ];
}

/// The configured reference for a role, following the canonical fallbacks.
/// `advisor` has no fallback: unset means disabled.
pub fn role_ref(config: &Value, role: ModelRole) -> Option<String> {
    let explicit = |key: &str| {
        config
            .pointer(&format!("/model_roles/{key}"))
            .and_then(Value::as_str)
            .map(str::to_string)
    };
    let default = || {
        explicit("default").or_else(|| {
            config
                .get("model")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
    };
    let small = || {
        explicit("small").or_else(|| {
            config
                .get("small_model")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
    };
    match role {
        ModelRole::Default => default(),
        ModelRole::Small => small().or_else(default),
        ModelRole::Advisor => explicit("advisor"),
        ModelRole::Evaluator | ModelRole::Compaction | ModelRole::Title => {
            explicit(role.key()).or_else(small).or_else(default)
        }
    }
}

/// The configured default model when available, else the most recently used available
/// model, else the newest available tool-capable model. Warnings name an unavailable
/// configured model.
pub fn default_model(
    catalog: &Catalog,
    config: &Value,
    recent: &[String],
) -> Result<(ModelRef, Vec<String>), CatalogError> {
    let mut warnings = Vec::new();
    if let Some(configured) = role_ref(config, ModelRole::Default) {
        match usable(catalog, &configured) {
            Ok(r) => return Ok((r, warnings)),
            Err(e) => warnings.push(format!("configured model {configured} is unavailable: {e}")),
        }
    }
    if let Some(r) = recent.iter().find_map(|r| usable(catalog, r).ok()) {
        return Ok((r, warnings));
    }
    let newest = catalog
        .providers
        .values()
        .flat_map(|p| p.models.values())
        .filter(|m| m.capabilities.tools && catalog.is_available(m))
        .max_by(|a, b| {
            a.released_at
                .cmp(&b.released_at)
                .then_with(|| b.id.cmp(&a.id))
        });
    match newest {
        Some(m) => Ok((
            ModelRef {
                provider: m.provider_id.clone(),
                model: m.id.clone(),
                variant: None,
            },
            warnings,
        )),
        None => Err(CatalogError::ModelNotSelected),
    }
}

fn usable(catalog: &Catalog, value: &str) -> Result<ModelRef, CatalogError> {
    let r = ModelRef::parse(value)?;
    let (_, model) = catalog.find(&r)?;
    match catalog.availability(model) {
        super::Availability::Available => Ok(r),
        super::Availability::Unavailable(reason) => Err(CatalogError::Unavailable {
            model: r.base(),
            reason: reason.into(),
        }),
    }
}

/// Recently used model references from `<state>/model.json`, newest first.
pub fn recent_models(path: &Path) -> Vec<String> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .and_then(|v| v.get("recent").cloned())
        .and_then(|r| serde_json::from_value(r).ok())
        .unwrap_or_default()
}

/// Move `model_ref` to the front of the recent list (at most 10 entries).
pub fn record_recent(path: &Path, model_ref: &str) -> std::io::Result<()> {
    let mut recent = recent_models(path);
    recent.retain(|r| r != model_ref);
    recent.insert(0, model_ref.to_string());
    recent.truncate(10);
    let text = serde_json::to_string_pretty(&json!({ "recent": recent }))
        .map_err(std::io::Error::other)?;
    cyber_core::trust::write_private_atomic(path, text.as_bytes())
}
