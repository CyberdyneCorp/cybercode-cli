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

    /// The role named by its configuration key.
    pub fn parse(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|role| role.key() == key)
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

/// The model a Workflow tier naming `role` uses (`workflows` → Model tiers and mixing).
/// Unlike [`role_ref`], an unset role falls back to the default model explicitly, so the
/// `verify` tier (`advisor`) still runs when no advisor is configured. `None` means no
/// model is configured at all; the agent then inherits its parent's model.
pub fn tier_role_ref(config: &Value, role: ModelRole) -> Option<String> {
    role_ref(config, role).or_else(|| role_ref(config, ModelRole::Default))
}

/// The configured default model when available, else the most recently used available
/// model, else the newest available tool-capable model of a provider with a credential.
/// A keyless local endpoint from the catalog (a loopback URL such as LM Studio's) is not
/// picked on its own: nothing shows it is running, and a first run would fail on a
/// connection error. It is picked once the user configures it under `providers`.
/// Warnings name an unavailable configured model.
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
        .filter(|m| chosen_provider(catalog, config, &m.provider_id))
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

/// A provider the automatic choice may use: one with a real credential, or one the user
/// configured.
fn chosen_provider(catalog: &Catalog, config: &Value, provider: &str) -> bool {
    let keyless = catalog
        .credential(provider)
        .is_some_and(|c| c.source == super::CredentialSource::NotRequired);
    !keyless || config["providers"].get(provider).is_some()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tier_roles_fall_back_to_the_default_model() {
        let config = json!({"model_roles":{"default":"p/big","small":"p/small"}});
        assert_eq!(role_ref(&config, ModelRole::Advisor), None);
        assert_eq!(
            tier_role_ref(&config, ModelRole::Advisor).as_deref(),
            Some("p/big")
        );
        assert_eq!(
            tier_role_ref(&config, ModelRole::Small).as_deref(),
            Some("p/small")
        );
        let advised = json!({"model":"p/big","model_roles":{"advisor":"p/wise"}});
        assert_eq!(
            tier_role_ref(&advised, ModelRole::Advisor).as_deref(),
            Some("p/wise")
        );
        assert_eq!(tier_role_ref(&json!({}), ModelRole::Advisor), None);
    }

    #[test]
    fn roles_parse_from_their_keys() {
        for role in ModelRole::ALL {
            assert_eq!(ModelRole::parse(role.key()), Some(role));
        }
        assert_eq!(ModelRole::parse("verify"), None);
    }
}
