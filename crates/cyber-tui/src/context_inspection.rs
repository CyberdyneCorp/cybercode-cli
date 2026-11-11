//! Format the metadata-only Session context response for the scrollable panel.

use serde_json::Value;

pub(crate) fn format(data: &Value) -> String {
    let inspection = &data["inspection"];
    if !inspection.is_object() {
        return "Context estimates are unavailable from this server.".into();
    }
    let count = |value: &Value| {
        value
            .as_u64()
            .map_or_else(|| "unknown".into(), |n| n.to_string())
    };
    let mut lines = vec![
        format!("Context Epoch {}", count(&data["epoch"])),
        format!(
            "Estimated stored input: {} tokens",
            count(&inspection["estimated_input_tokens"])
        ),
        format!("Model window: {} tokens", count(&inspection["window"])),
        format!(
            "Last measured request: {} tokens",
            count(&inspection["last_request_tokens"])
        ),
        format!(
            "Queued inputs: {} · Held inputs: {}",
            count(&inspection["queued_inputs"]),
            count(&inspection["held_inputs"])
        ),
        format!(
            "Delivered scoped rules: {}",
            count(&inspection["delivered_rule_count"])
        ),
    ];
    let utilization = inspection["estimated_input_utilization"]
        .as_f64()
        .map_or_else(
            || "unknown".into(),
            |fraction| format!("{:.1}%", fraction * 100.0),
        );
    lines.push(format!("Estimated window use: {utilization}"));
    if inspection["epoch_pending"].as_bool() == Some(true) {
        lines.push("The next turn will rebuild the Context Epoch.".into());
    }
    lines.push("Estimate excludes tool schemas and turn-only instructions.".into());
    if let Some(sources) = inspection["sources"].as_array() {
        lines.push(String::new());
        for source in sources {
            if let (Some(id), Some(origin), Some(tokens)) = (
                source["id"].as_str(),
                source["origin"].as_str(),
                source["estimated_tokens"].as_u64(),
            ) {
                lines.push(format!("{id} · {origin} · ~{tokens} tokens"));
            }
        }
    }
    lines.join("\n")
}
