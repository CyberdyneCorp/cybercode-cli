//! Production auto-mode admission, after the ordinary rule and Mode ceilings.
use crate::host::{Ctx, canonical};
use crate::permissions::{Mode, Request, is_protected};
use crate::tools::ToolError;
use cyber_server::runtime::{AutoEffect, AutoReview};
use serde_json::{Value, json};

impl Ctx<'_> {
    /// False means manual approval is still required; an Allow never saves an approval.
    pub(crate) async fn review_auto_permission(
        &self,
        request: &Request,
        metadata: &mut Value,
    ) -> Result<bool, ToolError> {
        let protected =
            self.policy.touches_protected(request) || self.auto_protected_resource(request);
        if self.policy.mode == Mode::Auto && protected {
            if !metadata.is_object() {
                *metadata = json!({});
            }
            metadata["requires_confirmation"] = json!(true);
        }
        if !self.policy.auto_review_allowed(request)
            || metadata["requires_confirmation"] == true
            || protected
        {
            return Ok(false);
        }
        let review = AutoReview {
            action: request.action.clone(),
            resources: request.resources.clone(),
            tool: self.inv.name.clone(),
            input: self.inv.input.clone(),
            policy: String::new(),
        };
        let decision = tokio::select! {
            _ = self.cancel.cancelled() => return Err(ToolError::Aborted),
            decision = self.inv.asker.review_auto(review, self.cancel.clone()) =>
                decision.map_err(|error| ToolError::Failed(format!("Auto review failed: {error}")))?,
        };
        match decision.decision {
            AutoEffect::Allow => Ok(true),
            AutoEffect::Block => Err(ToolError::Failed(format!(
                "Blocked by auto mode: {}",
                decision.reason
            ))),
            AutoEffect::Fallback => Ok(false),
        }
    }

    fn auto_protected_resource(&self, request: &Request) -> bool {
        if !matches!(
            request.action.as_str(),
            "read" | "edit" | "glob" | "grep" | "list"
        ) {
            return false;
        }
        request.resources.iter().any(|resource| {
            let resolved = self.resolve(resource);
            let path = canonical(&resolved);
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_default();
            (name == ".env" || name.starts_with(".env.")) && name != ".env.example"
                || is_protected(&path, &self.policy.location, &self.policy.home)
        })
    }
}
