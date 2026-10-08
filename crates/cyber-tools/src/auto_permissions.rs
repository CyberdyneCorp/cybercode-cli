//! Production auto-mode admission, after the ordinary rule and Mode ceilings.
use crate::host::{Ctx, canonical};
use crate::permissions::{Mode, Request, is_protected};
use crate::tools::ToolError;
use cyber_core::config::{AutoFallback, AutoModeSettings};
use cyber_server::runtime::{AutoEffect, AutoReview, AutoReviewStage};
use serde_json::{Value, json};

impl Ctx<'_> {
    /// False means manual approval is still required; an Allow never saves an approval.
    pub(crate) async fn review_auto_permission(
        &self,
        request: &Request,
        metadata: &mut Value,
        needs_approval: bool,
    ) -> Result<bool, ToolError> {
        let inherited = self
            .host
            .inherited_permissions(&self.inv.session_id)
            .await
            .map_err(ToolError::Failed)?;
        let mut gates: Vec<(Option<String>, AutoModeSettings)> = inherited
            .auto
            .into_iter()
            .map(|(id, settings)| (Some(id), settings))
            .collect();
        if self.policy.mode == Mode::Auto {
            let (config, _) = (self.host.opts.config)(&self.location).map_err(ToolError::Failed)?;
            gates.insert(
                0,
                (
                    None,
                    AutoModeSettings::from_config(&config).map_err(ToolError::Failed)?,
                ),
            );
        }
        if gates.is_empty() {
            return Ok(false);
        }
        // All explicit blocks precede inference and overrides, including another gate's allow.
        let blocked = gates.iter().position(|(_, settings)| {
            request.resources.iter().any(|resource| {
                settings
                    .rules
                    .always_block
                    .iter()
                    .any(|rule| rule.matches(&request.action, resource))
            })
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
        if blocked.is_none() && !eligible {
            return Ok(false);
        }
        let stages = if let Some(index) = blocked {
            vec![self.auto_stage(request, &gates[index], true)]
        } else {
            gates
                .iter()
                .map(|gate| self.auto_stage(request, gate, false))
                .collect()
        };
        let decision = tokio::select! {
            _ = self.cancel.cancelled() => return Err(ToolError::Aborted),
            result = self.inv.asker.review_auto_stages(stages, self.cancel.clone()) => result.map_err(|error| ToolError::Failed(format!("Auto review failed: {error}")))?,
        };
        match decision.decision {
            AutoEffect::Allow => Ok(true),
            AutoEffect::Block => Err(ToolError::Failed(format!(
                "Blocked by auto mode: {}",
                decision.reason
            ))),
            AutoEffect::Fallback => {
                // A deny fallback anywhere in the intersection cannot become a manual allow.
                if gates
                    .iter()
                    .any(|(_, settings)| settings.fallback == AutoFallback::Deny)
                {
                    return Err(ToolError::Failed(format!(
                        "Blocked by auto mode: {} (fallback is deny)",
                        decision.reason
                    )));
                }
                if !metadata.is_object() {
                    *metadata = json!({});
                }
                metadata["requires_confirmation"] = json!(true);
                metadata["auto_review_session"] = json!(decision.reviewed_session_id);
                Ok(false)
            }
        }
    }

    fn auto_stage(
        &self,
        request: &Request,
        gate: &(Option<String>, AutoModeSettings),
        blocked: bool,
    ) -> AutoReviewStage {
        let (ancestor_id, settings) = gate;
        let allowed = !request.resources.is_empty()
            && request.resources.iter().all(|resource| {
                settings
                    .rules
                    .always_allow
                    .iter()
                    .any(|rule| rule.matches(&request.action, resource))
            });
        let rule = if blocked {
            let prefix = ancestor_id
                .as_ref()
                .map_or(String::new(), |id| format!("ancestor {id} "));
            Some((
                AutoEffect::Block,
                format!("Matched {prefix}permissions.auto_mode.rules.always_block"),
            ))
        } else if self
            .inv
            .asker
            .auto_override_matches(&self.inv.name, &self.inv.input)
        {
            Some((
                AutoEffect::Allow,
                "User confirmed one-shot classifier override".into(),
            ))
        } else if allowed {
            Some((
                AutoEffect::Allow,
                "Matched permissions.auto_mode.rules.always_allow".into(),
            ))
        } else if request.read_only
            && self.host.read_only_tool(&self.inv.name)
            && !settings.classify_read_only
        {
            Some((
                AutoEffect::Allow,
                "Read-only classification is disabled".into(),
            ))
        } else {
            None
        };
        AutoReviewStage {
            review: AutoReview {
                action: request.action.clone(),
                resources: request.resources.clone(),
                tool: self.inv.name.clone(),
                input: self.inv.input.clone(),
                policy: settings.policy.clone(),
            },
            ancestor_id: ancestor_id.clone(),
            rule,
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
