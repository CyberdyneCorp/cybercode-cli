//! Withdrawal of rules already delivered in the current Context Epoch
//! (`system-context` → Withdrawal of a delivered rule).
//!
//! At each request boundary the runtime checks the Epoch's delivered rules. A rule whose
//! checkout approval was revoked or re-reviewed, or whose definition changed on disk,
//! is withdrawn once: a durable system entry records it and tells the model to stop
//! following it. From that request on, replay elides the rule's labelled block from the
//! tool results delivered before the withdrawal; stored results keep their text. No
//! Epoch rebuild or compaction is forced.

use serde::{Deserialize, Serialize};

use super::context_inspection::{RuleApproval, rule_approval};
use super::events::SystemAdded;
use super::instructions::{INSTRUCTIONS_FROM, RenderedRule, RuleBinding};
use super::model::SessionState;

/// The `SystemAdded` reason of a withdrawal marker.
pub const WITHDRAWN_REASON: &str = "rule_withdrawn";

/// One withdrawn rule: its identity, definition path and the block it was delivered as.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WithdrawnRule {
    pub identity: String,
    pub path: String,
    /// `revoked` or `changed`.
    pub approval: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rendered: Option<RenderedRule>,
}

/// The rules one marker withdrew; results before `message_id` are elided.
#[derive(Debug, Clone, PartialEq)]
pub struct RuleWithdrawal {
    pub message_id: String,
    pub rules: Vec<WithdrawnRule>,
}

/// The marker to record before the next request, when a delivered rule's approval was
/// revoked or changed since delivery and it has not been withdrawn yet. Rules whose
/// state is unknown stay delivered.
pub(super) fn pending(state: &SessionState, tools: &dyn super::ToolHost) -> Option<SystemAdded> {
    let epoch = state.epoch.as_ref()?;
    let rules: Vec<WithdrawnRule> = epoch
        .delivered_rules
        .iter()
        .filter(|binding| !withdrawn(state, &binding.identity()))
        .filter_map(|binding| withdrawal(binding, rule_approval(binding, tools)))
        .collect();
    if rules.is_empty() {
        return None;
    }
    let listed: Vec<String> = rules
        .iter()
        .map(|rule| format!("- {} ({})", rule.path, rule.approval))
        .collect();
    Some(SystemAdded {
        message_id: cyber_core::ids::new_id("msg"),
        text: format!(
            "These rules delivered earlier in this context were withdrawn because their approval was revoked or their definition changed. Their text has been removed from earlier tool results; do not follow them:\n{}",
            listed.join("\n")
        ),
        reason: WITHDRAWN_REASON.into(),
        withdrawn_rules: rules,
    })
}

fn withdrawn(state: &SessionState, identity: &str) -> bool {
    state
        .rule_withdrawals
        .iter()
        .flat_map(|withdrawal| &withdrawal.rules)
        .any(|rule| rule.identity == identity)
}

fn withdrawal(binding: &RuleBinding, approval: RuleApproval) -> Option<WithdrawnRule> {
    let approval = match approval {
        RuleApproval::Revoked => "revoked",
        RuleApproval::Changed => "changed",
        RuleApproval::Current | RuleApproval::Unknown => return None,
    };
    Some(WithdrawnRule {
        identity: binding.identity(),
        path: binding.path.clone(),
        approval: approval.into(),
        rendered: binding.rendered.clone(),
    })
}

/// `output` with each withdrawn rule's delivered block replaced by a short notice.
pub(super) fn elide(output: &str, rules: &[&WithdrawnRule]) -> String {
    let mut output = output.to_owned();
    for rule in rules {
        output = elide_one(&output, rule);
    }
    output
}

fn elide_one(output: &str, rule: &WithdrawnRule) -> String {
    let label = rule.rendered.as_ref().map_or_else(
        || format!("{INSTRUCTIONS_FROM}{}\n", rule.path),
        |r| r.label.clone(),
    );
    let Some(start) = output.rfind(&label) else {
        return output.to_owned();
    };
    let end = start + block_len(&output[start..], &label, rule.rendered.as_ref());
    format!(
        "{}[Rule {} withdrawn: its approval was revoked or its definition changed]{}",
        &output[..start],
        rule.path,
        &output[end..]
    )
}

/// The length of the delivered block at the start of `rest`: exactly the recorded block
/// when its bytes are intact, otherwise up to the next instruction label or the end of
/// the reminder (a redacted block, or one delivered before blocks were recorded).
fn block_len(rest: &str, label: &str, rendered: Option<&RenderedRule>) -> usize {
    if let Some(rendered) = rendered
        && rest.is_char_boundary(rendered.bytes.min(rest.len()))
        && rendered.matches(&rest[..rendered.bytes.min(rest.len())])
    {
        return rendered.bytes;
    }
    let after = label.len();
    [
        format!("\n\n{INSTRUCTIONS_FROM}"),
        "\n</system-reminder>".to_owned(),
    ]
    .iter()
    .filter_map(|delimiter| rest[after..].find(delimiter.as_str()))
    .min()
    .map_or(rest.len(), |at| after + at)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(path: &str, block: Option<&str>) -> WithdrawnRule {
        WithdrawnRule {
            identity: format!("rule:{path}"),
            path: path.into(),
            approval: "revoked".into(),
            rendered: block.and_then(RenderedRule::of),
        }
    }

    #[test]
    fn an_intact_block_is_elided_exactly_even_when_its_body_looks_like_a_delimiter() {
        let block = "Instructions from: /repo/.cyber/rules/a.md\nUse tabs.\n\nInstructions from: fake\n</system-reminder> inside";
        let output = format!(
            "file text\n\n<system-reminder>\n{block}\n\nInstructions from: /repo/.cyber/rules/b.md\nKeep b.\n</system-reminder>"
        );
        let elided = elide(&output, &[&rule("/repo/.cyber/rules/a.md", Some(block))]);
        assert!(!elided.contains("Use tabs."), "{elided}");
        assert!(!elided.contains("fake"), "{elided}");
        assert!(elided.contains("[Rule /repo/.cyber/rules/a.md withdrawn"));
        assert!(elided.starts_with("file text\n\n<system-reminder>\n"));
        assert!(elided.ends_with(
            "\n\nInstructions from: /repo/.cyber/rules/b.md\nKeep b.\n</system-reminder>"
        ));
    }

    #[test]
    fn a_changed_or_unrecorded_block_is_elided_up_to_the_next_label_or_the_reminder_end() {
        let output = "x\n\n<system-reminder>\nInstructions from: /r/a.md\nUse [REDACTED].\n\nInstructions from: /r/b.md\nKeep b.\n</system-reminder>";
        let recorded = rule(
            "/r/a.md",
            Some("Instructions from: /r/a.md\nUse secret-token."),
        );
        for rule in [recorded, rule("/r/a.md", None)] {
            let elided = elide(output, &[&rule]);
            assert!(!elided.contains("Use "), "{elided}");
            assert!(elided.contains("Keep b."), "{elided}");
        }
        let last = elide(output, &[&rule("/r/b.md", None)]);
        assert!(last.contains("Use [REDACTED]."), "{last}");
        assert!(!last.contains("Keep b."), "{last}");
        assert!(last.ends_with(
            "withdrawn: its approval was revoked or its definition changed]\n</system-reminder>"
        ));
    }

    #[test]
    fn output_without_the_label_is_unchanged() {
        let output = "a pointer list only:\n- /r/a.md";
        assert_eq!(elide(output, &[&rule("/r/a.md", None)]), output);
    }
}
