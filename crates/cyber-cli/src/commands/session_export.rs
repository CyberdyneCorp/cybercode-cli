//! `cyber sessions export` (`storage-events` → Session export): the server's export as
//! JSON (the default) or Markdown, written to stdout.

use std::fmt::Write as _;

use serde_json::Value;

/// The Markdown of an export: a heading per Session, its messages in order with their
/// tool calls, its Goals and Workflow Runs, then its child Sessions one level deeper.
pub fn markdown(export: &Value) -> String {
    let mut out = String::new();
    session(&mut out, export, 1);
    out
}

fn session(out: &mut String, export: &Value, depth: usize) {
    let s = &export["session"];
    let heading = "#".repeat(depth.min(5));
    let _ = writeln!(out, "{heading} {}\n", text(&s["title"]));
    let usage = &export["totals"]["usage"];
    let _ = writeln!(
        out,
        "- Session: `{}`\n- Location: {}\n- Agent: {} · Model: {} · Mode: {}\n- Tokens: {} in · {} out · Cost: ${:.4}\n",
        text(&s["id"]),
        text(&s["directory"]),
        text(&s["agent"]),
        text(&s["model"]),
        text(&s["mode"]),
        usage["input"].as_u64().unwrap_or(0),
        usage["output"].as_u64().unwrap_or(0),
        export["totals"]["cost"].as_f64().unwrap_or(0.0),
    );
    let sub = "#".repeat((depth + 1).min(6));
    let messages = export["messages"].as_array().cloned().unwrap_or_default();
    if !messages.is_empty() {
        let _ = writeln!(out, "{sub} Messages\n");
        messages.iter().for_each(|m| message(out, m));
    }
    goals(out, export, &sub);
    runs(out, export, &sub);
    for child in export["children"].as_array().into_iter().flatten() {
        session(out, child, depth + 1);
    }
}

fn message(out: &mut String, m: &Value) {
    match m["kind"].as_str().unwrap_or_default() {
        "user" => {
            let _ = writeln!(out, "**User** ({})\n", text(&m["source"]));
            for part in m["parts"].as_array().into_iter().flatten() {
                if let Some(body) = part["text"].as_str() {
                    let _ = writeln!(out, "{}\n", body.trim_end());
                }
            }
        }
        "assistant" => {
            let elapsed = match (m["started_ms"].as_i64(), m["ended_ms"].as_i64()) {
                (Some(start), Some(end)) => format!(" · {:.1}s", (end - start) as f64 / 1000.0),
                _ => String::new(),
            };
            let _ = writeln!(out, "**Assistant** ({}{elapsed})\n", text(&m["model"]));
            let body = m["text"].as_str().unwrap_or_default().trim_end();
            if !body.is_empty() {
                let _ = writeln!(out, "{body}\n");
            }
            for tool in m["tools"].as_array().into_iter().flatten() {
                let _ = writeln!(
                    out,
                    "- tool `{}` ({})",
                    text(&tool["name"]),
                    text(&tool["status"])
                );
            }
            if m["tools"].as_array().is_some_and(|t| !t.is_empty()) {
                out.push('\n');
            }
        }
        _ => {}
    }
}

fn goals(out: &mut String, export: &Value, sub: &str) {
    let goals = export["goals"].as_array().cloned().unwrap_or_default();
    if goals.is_empty() {
        return;
    }
    let _ = writeln!(out, "{sub} Goals\n");
    for g in &goals {
        let _ = writeln!(
            out,
            "- ◎ {} — {} · {} turns · ${:.4}",
            text(&g["condition"]),
            text(&g["status"]),
            g["turns"].as_u64().unwrap_or(0),
            g["cost_usd"].as_f64().unwrap_or(0.0)
        );
    }
    out.push('\n');
}

fn runs(out: &mut String, export: &Value, sub: &str) {
    let runs = export["workflow_runs"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    if runs.is_empty() {
        return;
    }
    let _ = writeln!(out, "{sub} Workflow runs\n");
    for r in &runs {
        let _ = writeln!(
            out,
            "- `{}` {} — {} · {} agents · ${:.4}",
            text(&r["id"]),
            text(&r["name"]),
            text(&r["status"]),
            r["agents"]["total"].as_u64().unwrap_or(0),
            r["cost_usd"].as_f64().unwrap_or(0.0)
        );
    }
    out.push('\n');
}

fn text(value: &Value) -> &str {
    value.as_str().unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn the_markdown_lists_messages_goals_runs_and_children() {
        let export = json!({
            "session": {"id": "ses_1", "title": "Fix login", "directory": "/repo", "agent": "build",
                "model": "corp/main", "mode": "default"},
            "totals": {"usage": {"input": 100, "output": 20}, "cost": 0.01},
            "messages": [
                {"kind": "user", "source": "session", "parts": [{"type": "text", "text": "fix it"}]},
                {"kind": "assistant", "model": "corp/main", "text": "done", "started_ms": 0,
                    "ended_ms": 1500, "tools": [{"name": "edit", "status": "ok"}]}
            ],
            "goals": [{"condition": "tests pass", "status": "met", "turns": 2, "cost_usd": 0.0}],
            "workflow_runs": [{"id": "run_1", "name": "audit", "status": "completed",
                "agents": {"total": 3}, "cost_usd": 0.5}],
            "children": [{"session": {"id": "ses_2", "title": "Explore"}, "totals": {},
                "messages": []}]
        });
        let md = markdown(&export);
        for needle in [
            "# Fix login",
            "**User** (session)\n\nfix it",
            "**Assistant** (corp/main · 1.5s)\n\ndone",
            "- tool `edit` (ok)",
            "## Goals",
            "- ◎ tests pass — met · 2 turns",
            "- `run_1` audit — completed · 3 agents",
            "## Explore",
        ] {
            assert!(md.contains(needle), "{needle} in:\n{md}");
        }
    }
}
