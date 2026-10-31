//! Runtime-built text for the model (`tool-registry` → Tool output redaction): Job and
//! workflow handbacks embed raw output in JSON, so the strings are redacted before they
//! are encoded, where an escape such as `\n` would hide a token's word boundary.

use serde_json::Value;

use super::ToolHost;

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
