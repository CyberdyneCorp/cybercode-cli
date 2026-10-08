//! Production auto-mode admission, after the ordinary rule and Mode ceilings.
use crate::host::{Ctx, canonical};
use crate::permissions::{Mode, Request, is_protected};
use crate::tools::ToolError;
use cyber_core::config::{AutoFallback, AutoModeSettings};
use cyber_server::runtime::{AutoEffect, AutoReview};
use serde_json::{Value, json};

impl Ctx<'_> {
    /// False means manual approval is still required; an Allow never saves an approval.
    pub(crate) async fn review_auto_permission(
        &self,
        request: &Request,
        metadata: &mut Value,
        needs_approval: bool,
    ) -> Result<bool, ToolError> {
        self.check_ancestor_auto_blocks(request).await?;
        if self.policy.mode != Mode::Auto {
            return Ok(false);
        }
        self.review_local_auto_permission(request, metadata, needs_approval)
            .await
    }

    async fn check_ancestor_auto_blocks(&self, request: &Request) -> Result<(), ToolError> {
        let inherited = self
            .host
            .inherited_permissions(&self.inv.session_id)
            .await
            .map_err(ToolError::Failed)?;
        if let Some((parent, _)) = inherited.auto_blocks.iter().find(|(_, rule)| {
            request
                .resources
                .iter()
                .any(|resource| rule.matches(&request.action, resource))
        }) {
            let reason =
                format!("Matched ancestor {parent} permissions.auto_mode.rules.always_block");
            let review = AutoReview {
                action: request.action.clone(),
                resources: request.resources.clone(),
                tool: self.inv.name.clone(),
                input: self.inv.input.clone(),
                policy: String::new(),
            };
            tokio::select! {
                _ = self.cancel.cancelled() => return Err(ToolError::Aborted),
                result = self.inv.asker.decide_auto_rule(review, AutoEffect::Block, reason.clone(), self.cancel.clone()) => {
                    result.map_err(|error| ToolError::Failed(format!("Auto review failed: {error}")))?;
                }
            }
            return Err(ToolError::Failed(format!("Blocked by auto mode: {reason}")));
        }
        Ok(())
    }

    async fn review_local_auto_permission(
        &self,
        request: &Request,
        metadata: &mut Value,
        needs_approval: bool,
    ) -> Result<bool, ToolError> {
        let (config, _) = (self.host.opts.config)(&self.location).map_err(ToolError::Failed)?;
        let settings = AutoModeSettings::from_config(&config).map_err(ToolError::Failed)?;
        let blocked = request.resources.iter().any(|resource| {
            settings
                .rules
                .always_block
                .iter()
                .rev()
                .any(|rule| rule.matches(&request.action, resource))
        });
        let protected =
            self.policy.touches_protected(request) || self.auto_protected_resource(request);
        if protected {
            if !metadata.is_object() {
                *metadata = json!({});
            }
            metadata["requires_confirmation"] = json!(true);
        }
        let eligible = needs_approval
            && self.policy.auto_review_allowed(request)
            && metadata["requires_confirmation"] != true
            && !protected;
        if !blocked && !eligible {
            return Ok(false);
        }
        let allowed = eligible
            && !request.resources.is_empty()
            && request.resources.iter().all(|resource| {
                settings
                    .rules
                    .always_allow
                    .iter()
                    .rev()
                    .any(|rule| rule.matches(&request.action, resource))
            });
        let rule = if blocked {
            Some((
                AutoEffect::Block,
                "Matched permissions.auto_mode.rules.always_block",
            ))
        } else if eligible
            && self
                .inv
                .asker
                .auto_override_matches(&self.inv.name, &self.inv.input)
        {
            Some((
                AutoEffect::Allow,
                "User confirmed one-shot classifier override",
            ))
        } else if allowed {
            Some((
                AutoEffect::Allow,
                "Matched permissions.auto_mode.rules.always_allow",
            ))
        } else if request.read_only
            && self.host.read_only_tool(&self.inv.name)
            && !settings.classify_read_only
        {
            Some((AutoEffect::Allow, "Read-only classification is disabled"))
        } else {
            None
        };
        let review = AutoReview {
            action: request.action.clone(),
            resources: request.resources.clone(),
            tool: self.inv.name.clone(),
            input: self.inv.input.clone(),
            policy: settings.policy,
        };
        let decide = async {
            match rule {
                Some((effect, reason)) => {
                    self.inv
                        .asker
                        .decide_auto_rule(review, effect, reason.into(), self.cancel.clone())
                        .await
                }
                None => {
                    self.inv
                        .asker
                        .review_auto(review, self.cancel.clone())
                        .await
                }
            }
        };
        let decision = tokio::select! {
            _ = self.cancel.cancelled() => return Err(ToolError::Aborted),
            decision = decide =>
                decision.map_err(|error| ToolError::Failed(format!("Auto review failed: {error}")))?,
        };
        match decision.decision {
            AutoEffect::Allow => Ok(true),
            AutoEffect::Block => Err(ToolError::Failed(format!(
                "Blocked by auto mode: {}",
                decision.reason
            ))),
            AutoEffect::Fallback if settings.fallback == AutoFallback::Deny => {
                Err(ToolError::Failed(format!(
                    "Blocked by auto mode: {} (fallback is deny)",
                    decision.reason
                )))
            }
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
