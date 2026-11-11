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
    lines.extend(prepared_request(&inspection["last_prepared_request"]));
    lines.extend(effective_sources(inspection));
    lines.join("\n")
}

fn prepared_request(prepared: &Value) -> Vec<String> {
    if !prepared.is_object() {
        return Vec::new();
    }
    let mut lines = vec![
        String::new(),
        format!(
            "Last prepared request · {}",
            crate::messaging::clean_line(prepared["step_id"].as_str().unwrap_or("unknown"))
        ),
        format!(
            "Estimated complete input · {} tokens",
            prepared["estimated_input_tokens"]
                .as_u64()
                .map_or_else(|| "unknown".into(), |n| n.to_string())
        ),
        format!(
            "Model · {}/{} · Epoch {}",
            crate::messaging::clean_line(prepared["provider"].as_str().unwrap_or("unknown")),
            crate::messaging::clean_line(prepared["model"].as_str().unwrap_or("unknown")),
            prepared["epoch"]
                .as_u64()
                .map_or_else(|| "unknown".into(), |n| n.to_string())
        ),
        format!(
            "Estimated prepared window use · {}",
            prepared["estimated_input_utilization"]
                .as_f64()
                .map_or_else(
                    || "unknown".into(),
                    |fraction| format!("{:.1}%", fraction * 100.0)
                )
        ),
    ];
    for source in prepared["sources"].as_array().into_iter().flatten() {
        let id = crate::messaging::clean_line(source["id"].as_str().unwrap_or("unknown"));
        let tokens = source["estimated_tokens"].as_u64().unwrap_or(0);
        lines.push(format!("{id} · ~{tokens} tokens"));
    }
    lines
}

fn effective_sources(inspection: &Value) -> Vec<String> {
    let mut lines = vec![String::new(), "Recorded context sources".into()];
    for source in inspection["effective_sources"]
        .as_array()
        .into_iter()
        .flatten()
    {
        let field = |name| crate::messaging::clean_line(source[name].as_str().unwrap_or("unknown"));
        let tokens = source["estimated_tokens"]
            .as_u64()
            .map_or_else(|| "unknown".into(), |n| format!("~{n}"));
        lines.push(format!(
            "{} · {} · {} · {tokens} tokens",
            field("id"),
            field("origin"),
            field("status").replace('_', " ")
        ));
        if source["pending_update"].as_bool() == Some(true) {
            lines.push("  Update pending at the next safe boundary".into());
        }
        for path in source["file_paths"].as_array().into_iter().flatten() {
            lines.push(format!(
                "  {}",
                crate::messaging::clean_line(path.as_str().unwrap_or(""))
            ));
        }
        let omitted = source["omitted_file_paths"].as_u64().unwrap_or(0);
        if omitted > 0 {
            lines.push(format!("  Additional paths omitted · {omitted}"));
        }
    }
    let omitted = inspection["omitted_sources"].as_u64().unwrap_or(0);
    if omitted > 0 {
        lines.push(format!("Additional sources omitted · {omitted}"));
    }
    lines
}

#[cfg(test)]
mod tests {
    #[test]
    fn inspection_displays_pending_and_retained_sources_with_safe_path_text() {
        let text = super::format(&serde_json::json!({
            "epoch": 3,
            "inspection": {
                "last_prepared_request": {
                    "step_id": "msg_1", "epoch": 2, "provider": "test", "model": "main",
                    "estimated_input_tokens": 1280, "estimated_input_utilization": 0.5,
                    "sources": [{"id": "tool_schemas", "estimated_tokens": 280}]
                },
                "effective_sources": [{
                    "id": "core/instructions", "origin": "instruction_files",
                    "status": "retained_unavailable", "estimated_tokens": 42,
                    "pending_update": true, "file_paths": ["/repo/CYBER\n.md\u{001b}"],
                    "omitted_file_paths": 2
                }],
                "omitted_sources": 1
            }
        }));
        assert!(text.contains("Estimated window use: unknown"));
        assert!(text.contains("Last prepared request · msg_1"));
        assert!(text.contains("Estimated complete input · 1280 tokens"));
        assert!(text.contains("Model · test/main · Epoch 2"));
        assert!(text.contains("Estimated prepared window use · 50.0%"));
        assert!(text.contains("tool_schemas · ~280 tokens"));
        assert!(text.contains("retained unavailable · ~42 tokens"));
        assert!(text.contains("Update pending at the next safe boundary"));
        assert!(text.contains("/repo/CYBER .md"));
        assert!(text.contains("Additional paths omitted · 2"));
        assert!(text.contains("Additional sources omitted · 1"));
        assert!(!text.contains('\u{001b}'));
    }
}
