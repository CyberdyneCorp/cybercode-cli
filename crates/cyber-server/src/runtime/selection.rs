//! The authored model choice stays separate from profile-derived effective metadata.

use cyber_llm::catalog::ModelRef;
use serde::{Deserialize, Serialize};

use super::{AgentInference, Inner, ResolvedModel, RuntimeError, SessionInfo};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ModelSelection {
    pub reference: String,
    pub agent_default: bool,
}

impl ModelSelection {
    pub fn explicit(reference: String) -> Self {
        Self {
            reference,
            agent_default: false,
        }
    }

    /// The reference to use. Its variant is `derived` when an agent profile chose it or the
    /// whole reference is the agent's default, so an effort the model lacks is clamped.
    pub fn resolve(&self, options: &AgentInference) -> Result<Chosen, RuntimeError> {
        let reference = if self.agent_default {
            options.model.as_deref().unwrap_or(&self.reference)
        } else {
            &self.reference
        };
        if reference.is_empty() {
            return Err(RuntimeError::Invalid(
                "No model is configured; pass `model` or set an agent/default model in config"
                    .into(),
            ));
        }
        let mut model =
            ModelRef::parse(reference).map_err(|error| RuntimeError::Model(error.to_string()))?;
        let derived = (self.agent_default || model.variant.is_none()) && options.variant.is_some();
        if derived {
            model.variant.clone_from(&options.variant);
        }
        Ok(Chosen {
            reference: model.to_string(),
            derived: derived || self.agent_default,
        })
    }

    pub fn persisted(&self, effective: &str) -> Option<Self> {
        (self.agent_default || self.reference != effective).then(|| self.clone())
    }
}

pub(crate) struct Chosen {
    pub reference: String,
    pub derived: bool,
}

pub(crate) struct SelectedModel {
    pub reference: String,
    pub model: ResolvedModel,
    pub steps: Option<u64>,
    pub permission_mode: Option<String>,
}

impl Inner {
    pub(crate) fn resolve_selection(
        &self,
        info: &SessionInfo,
        selection: &ModelSelection,
    ) -> Result<SelectedModel, RuntimeError> {
        let turn = super::drain::turn_context_for_info(info, false);
        let options = self
            .tools
            .agent_inference(&turn)
            .map_err(RuntimeError::Invalid)?;
        self.resolve_options(info, selection, &options)
    }

    pub(crate) fn resolve_options(
        &self,
        info: &SessionInfo,
        selection: &ModelSelection,
        options: &AgentInference,
    ) -> Result<SelectedModel, RuntimeError> {
        let chosen = selection.resolve(options)?;
        let reference = if chosen.derived {
            self.resolver.clamp_effort(&chosen.reference)
        } else {
            chosen.reference
        };
        let mut model = self.resolve(&reference)?;
        options.request.apply_to(&mut model.template);
        Ok(SelectedModel {
            reference,
            model,
            steps: options
                .steps
                .or_else(|| info.parent_id.as_ref().map(|_| 50)),
            permission_mode: options.permission_mode.clone(),
        })
    }
}

pub(crate) fn validate_mode(mode: &str) -> Result<(), RuntimeError> {
    if matches!(
        mode,
        "default" | "accept-edits" | "plan" | "auto" | "dont-ask" | "bypass"
    ) {
        Ok(())
    } else {
        Err(RuntimeError::Invalid(format!("unknown mode {mode}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn agent(variant: Option<&str>) -> AgentInference {
        AgentInference {
            variant: variant.map(str::to_owned),
            ..AgentInference::default()
        }
    }

    /// Only an agent's variant, or the agent's default model, may be clamped to the model.
    #[test]
    fn a_variant_is_derived_only_when_the_agent_chose_it() {
        let explicit = ModelSelection::explicit("corp/coder#xhigh".into());
        let chosen = explicit.resolve(&agent(Some("low"))).unwrap();
        assert_eq!(
            (chosen.reference.as_str(), chosen.derived),
            ("corp/coder#xhigh", false)
        );
        let bare = ModelSelection::explicit("corp/coder".into());
        let chosen = bare.resolve(&agent(Some("xhigh"))).unwrap();
        assert_eq!(
            (chosen.reference.as_str(), chosen.derived),
            ("corp/coder#xhigh", true)
        );
        assert!(!bare.resolve(&agent(None)).unwrap().derived);
        let default = ModelSelection {
            reference: "corp/coder#xhigh".into(),
            agent_default: true,
        };
        assert!(default.resolve(&agent(None)).unwrap().derived);
    }
}
