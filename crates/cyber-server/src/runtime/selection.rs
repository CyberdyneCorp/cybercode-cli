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

    pub fn resolve(&self, options: &AgentInference) -> Result<String, RuntimeError> {
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
        if (self.agent_default || model.variant.is_none()) && options.variant.is_some() {
            model.variant.clone_from(&options.variant);
        }
        Ok(model.to_string())
    }

    pub fn persisted(&self, effective: &str) -> Option<Self> {
        (self.agent_default || self.reference != effective).then(|| self.clone())
    }
}

impl Inner {
    pub(crate) fn resolve_selection(
        &self,
        info: &SessionInfo,
        selection: &ModelSelection,
    ) -> Result<(String, ResolvedModel), RuntimeError> {
        let turn = super::drain::turn_context_for_info(info, false);
        let options = self
            .tools
            .agent_inference(&turn)
            .map_err(RuntimeError::Invalid)?;
        let reference = selection.resolve(&options)?;
        let mut resolved = self.resolve(&reference)?;
        options.request.apply_to(&mut resolved.template);
        Ok((reference, resolved))
    }
}
