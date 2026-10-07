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
        let reference = selection.resolve(options)?;
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
