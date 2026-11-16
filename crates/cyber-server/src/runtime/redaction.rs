//! Runtime-built text for the model (`tool-registry` → Tool output redaction): Job and
//! workflow handbacks embed raw output in JSON, so the strings are redacted before they
//! are encoded, where an escape such as `\n` would hide a token's word boundary.

use cyber_core::hooks::HookDecision;
use serde_json::Value;

use super::ToolHost;

/// Longest hook reason or notice kept, in bytes, after redaction (`hooks` → Hook
/// output redaction). Hook stderr may be captured up to 1 MiB; a reason longer than
/// this is cut on a character boundary and marked.
pub const HOOK_TEXT_LIMIT: usize = 4096;

/// Appended to a hook reason or notice cut at [`HOOK_TEXT_LIMIT`].
pub const HOOK_TEXT_TRUNCATED: &str = "… [hook output truncated]";

/// Every string leaf of `value`, redacted for the Location at `directory`.
pub(super) fn redact_value(tools: &dyn ToolHost, directory: &str, value: &mut Value) {
    match value {
        Value::String(text) => {
            let (redacted, _) = tools.redact_model_text(directory, std::mem::take(text));
            *text = redacted;
        }
        Value::Array(items) => {
            for item in items {
                redact_value(tools, directory, item);
            }
        }
        Value::Object(map) => {
            for item in map.values_mut() {
                redact_value(tools, directory, item);
            }
        }
        _ => {}
    }
}

/// `text` redacted for the Location at `directory`.
pub(super) fn redact_text(tools: &dyn ToolHost, directory: &str, text: String) -> String {
    tools.redact_model_text(directory, text).0
}

/// Taint evidence may be reused in a later classifier request. Redact its data
/// fields with the same Location policy without changing detector kinds or version.
pub(super) fn redact_untrusted(
    tools: &dyn ToolHost,
    directory: &str,
    record: &mut cyber_core::untrusted::UntrustedRecord,
) {
    let origin = redact_text(tools, directory, std::mem::take(&mut record.origin));
    record.origin = cyber_core::untrusted::visible_value(&origin);
    for signal in &mut record.signals {
        signal.excerpt = redact_text(tools, directory, std::mem::take(&mut signal.excerpt))
            .chars()
            .take(cyber_core::untrusted::EXCERPT_LIMIT)
            .collect();
    }
}

/// Hook-supplied `text` (a block reason, stop reason or notice) redacted for the
/// Location at `directory`, then bounded to [`HOOK_TEXT_LIMIT`]. Redaction runs first,
/// so a cut never leaves part of a secret the detectors would have matched whole.
pub(super) fn hook_text(tools: &dyn ToolHost, directory: &str, text: String) -> String {
    let mut text = redact_text(tools, directory, text);
    if text.len() > HOOK_TEXT_LIMIT {
        let mut end = HOOK_TEXT_LIMIT;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
        text.push_str(HOOK_TEXT_TRUNCATED);
    }
    text
}

/// `decision` with its free-text reasons passed through [`hook_text`]. The input
/// rewrite and context are left as the hook returned them.
pub(super) fn hook_decision(
    tools: &dyn ToolHost,
    directory: &str,
    mut decision: HookDecision,
) -> HookDecision {
    decision.reason = decision
        .reason
        .map(|reason| hook_text(tools, directory, reason));
    decision.stop_reason = decision
        .stop_reason
        .map(|reason| hook_text(tools, directory, reason));
    decision
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::NoTools;

    #[test]
    fn hook_text_redacts_then_bounds_on_a_char_boundary() {
        let token = "ghp_abcdefghijklmnop0123456789";
        let shown = hook_text(&NoTools, "/repo", format!("refused for {token}"));
        assert_eq!(shown, "refused for [REDACTED:github_token]");
        let long = format!("{token} {}", "é".repeat(HOOK_TEXT_LIMIT));
        let shown = hook_text(&NoTools, "/repo", long);
        assert!(shown.starts_with("[REDACTED:github_token] é"), "{shown}");
        assert!(shown.ends_with(HOOK_TEXT_TRUNCATED), "{shown}");
        assert!(shown.len() <= HOOK_TEXT_LIMIT + HOOK_TEXT_TRUNCATED.len());
    }

    #[test]
    fn hook_decision_redacts_reasons_and_keeps_the_rewrite() {
        let token = "ghp_abcdefghijklmnop0123456789";
        let decision = HookDecision {
            reason: Some(format!("deny {token}")),
            stop_reason: Some(format!("stop {token}")),
            updated_input: Some(serde_json::json!({"command": token})),
            ..HookDecision::default()
        };
        let decision = hook_decision(&NoTools, "/repo", decision);
        assert_eq!(
            decision.reason.as_deref(),
            Some("deny [REDACTED:github_token]")
        );
        assert_eq!(
            decision.stop_reason.as_deref(),
            Some("stop [REDACTED:github_token]")
        );
        assert_eq!(decision.updated_input.unwrap()["command"], token);
    }
}
