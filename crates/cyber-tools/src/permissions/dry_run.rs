//! Rule dry run (`permissions-modes` → Rule dry run): the effect of each resource of a
//! request, the matching rule and the layer it came from, decided exactly as a Session
//! would, without asking or running anything.

use serde::Serialize;

use super::engine::{Decision, Effect, Mode, Policy, Request, Rule, evaluate_scoped};

/// One evaluated resource.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Explained {
    pub action: String,
    pub resource: String,
    /// The final effect, after saved approvals, the Mode and every ceiling.
    pub effect: Effect,
    /// Why: the layer of the deciding rule, or what overrode it.
    pub reason: String,
    /// The last matching rule, when one matched.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rule: Option<Rule>,
    /// The final decision's message, when it is a denial.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

impl Policy {
    /// Explain every resource of `req` separately.
    pub fn explain(&self, req: &Request) -> Vec<Explained> {
        req.resources
            .iter()
            .map(|resource| self.explain_one(req, resource))
            .collect()
    }

    fn explain_one(&self, req: &Request, resource: &str) -> Explained {
        let single = Request {
            resources: vec![resource.to_string()],
            ..req.clone()
        };
        let (ruled, rule) =
            evaluate_scoped(&self.rules, &req.action, resource, req.tool.as_deref());
        let saved = ruled == Effect::Ask && self.saved_allows(&single);
        let decision = self.decide(&single);
        let (effect, message) = match &decision {
            Decision::Allow => (Effect::Allow, None),
            Decision::Ask => (Effect::Ask, None),
            Decision::Deny(reason) => (Effect::Deny, Some(reason.clone())),
        };
        let layered = if saved { Effect::Allow } else { ruled };
        let reason = match (self.ceiling_reason(&single, &decision), saved, rule) {
            (Some(ceiling), _, _) => ceiling.to_string(),
            _ if effect != layered => format!("{} mode", self.mode.as_str()),
            (None, true, _) => "saved approval, project".to_string(),
            (None, false, Some(rule)) => layer_name(&rule.source),
            (None, false, None) => "default rules".to_string(),
        };
        Explained {
            action: req.action.clone(),
            resource: resource.to_string(),
            effect,
            reason,
            rule: rule.cloned(),
            message,
        }
    }

    /// A ceiling that decided `req` whatever its rule said.
    fn ceiling_reason(&self, req: &Request, decision: &Decision) -> Option<&'static str> {
        let asks = *decision == Decision::Ask;
        if req.removal_risk.is_some() && !matches!(decision, Decision::Allow) {
            Some("critical-path removal")
        } else if self.touches_protected(req) && asks {
            Some("protected path")
        } else if req.ask_floor && asks {
            Some("excluded command")
        } else if matches!(decision, Decision::Deny(reason) if reason == "denied by a user rule") {
            Some("user deny ceiling")
        } else {
            None
        }
    }
}

/// A readable name for a rule's source layer.
pub fn layer_name(source: &str) -> String {
    let (kind, rest) = source.split_once(':').unwrap_or((source, ""));
    match kind {
        "default" => "default rules".into(),
        "global" => "global config".into(),
        "project" => "project config".into(),
        "session" => "session rules".into(),
        "agent" => format!("agent {rest}"),
        "profile" => format!("profile {rest}"),
        "managed" => "org policy".into(),
        "cli" => "command line".into(),
        "env" => "environment config".into(),
        _ => source.into(),
    }
}

impl Mode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::AcceptEdits => "accept-edits",
            Self::Plan => "plan",
            Self::Auto => "auto",
            Self::DontAsk => "dont-ask",
            Self::Bypass => "bypass",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::permissions::defaults;

    fn policy(mode: Mode) -> Policy {
        let mut rules = defaults(&[], true);
        rules.push(Rule::new(
            "bash",
            "git push *",
            Effect::Deny,
            "global:/u/cyber.jsonc",
        ));
        Policy {
            rules,
            saved: vec![Rule::new("bash", "npm test *", Effect::Allow, "saved")],
            mode,
            parent_modes: Vec::new(),
            location: "/repo".into(),
            home: "/home/u".into(),
            plan_file: "/repo/.cyber/plans/x.md".into(),
            ownership: None,
        }
    }

    fn bash(resources: &[&str]) -> Request {
        Request {
            action: "bash".into(),
            resources: resources.iter().map(|r| r.to_string()).collect(),
            ..Request::default()
        }
    }

    #[test]
    fn each_resource_names_its_effect_and_layer() {
        let rows = policy(Mode::Default).explain(&bash(&["npm test", "ls", "git push origin"]));
        let summary: Vec<(&str, Effect, &str)> = rows
            .iter()
            .map(|r| (r.resource.as_str(), r.effect, r.reason.as_str()))
            .collect();
        assert_eq!(
            summary,
            [
                ("npm test", Effect::Allow, "saved approval, project"),
                ("ls", Effect::Ask, "default rules"),
                ("git push origin", Effect::Deny, "global config"),
            ]
        );
        let dont_ask = policy(Mode::DontAsk).explain(&bash(&["ls"]));
        assert_eq!(
            (dont_ask[0].effect, dont_ask[0].reason.as_str()),
            (Effect::Deny, "dont-ask mode")
        );
        assert_eq!(
            dont_ask[0].message.as_deref(),
            Some("Not pre-approved (dont-ask mode)")
        );
    }
}
