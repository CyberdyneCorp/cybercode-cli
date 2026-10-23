//! `--format html`: the server's export document (`GET /sessions/{id}/export`) as one
//! self-contained page. It has inline CSS and no script, no external resource and no
//! anchor; a Content Security Policy forbids loading anything if the page is opened from
//! an untrusted place. Every string from the document is escaped, Markdown goes through
//! [`super::markdown`], tool calls and reasoning are `<details>` blocks, and the colors
//! follow `prefers-color-scheme`.

use std::collections::HashMap;
use std::fmt::Write as _;

use serde_json::Value;

use super::markdown;

/// Tool output longer than this many characters is cut, with a note; the JSON export
/// keeps all of it.
pub const OUTPUT_LIMIT: usize = 20_000;

/// The footer note of a `--sanitize` export.
const SANITIZED: &str = "Sanitized: prompts, responses, paths, URLs, tool inputs and outputs and metadata are replaced by numbered redaction markers.";

const CSP: &str = "default-src 'none'; style-src 'unsafe-inline'";

/// The page for `document`; `generator` (`cyber <version>`) is named in the footer with
/// the document's `exported_ms`.
pub fn render(document: &Value, generator: &str) -> String {
    let title = text(&document["session"]["title"])
        .or_else(|| text(&document["session"]["id"]))
        .unwrap_or("Session");
    let mut page = String::with_capacity(16 * 1024);
    page.push_str("<!doctype html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n");
    let _ = writeln!(
        page,
        "<meta http-equiv=\"Content-Security-Policy\" content=\"{CSP}\">"
    );
    page.push_str("<meta name=\"referrer\" content=\"no-referrer\">\n");
    page.push_str("<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n");
    page.push_str("<meta name=\"color-scheme\" content=\"light dark\">\n");
    let _ = writeln!(page, "<title>{} · Cyber Code</title>", escape(title));
    let _ = writeln!(page, "<style>{STYLE}</style>\n</head>\n<body>\n<main>");
    page.push_str(&header(document, title));
    page.push_str(&conversation(document));
    page.push_str(&closing(document, generator));
    page.push_str("</main>\n</body>\n</html>\n");
    page
}

/// HTML-escape `text` for element content and attribute values.
pub fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}

fn header(document: &Value, title: &str) -> String {
    let session = &document["session"];
    let messages = list(&document["messages"]);
    let mut facts = vec![
        ("Location", show(&session["directory"])),
        ("Session", show(&session["id"])),
        ("Models", escape(&models(session, messages).join(", "))),
        ("Started", time(&session["created_ms"])),
        ("Ended", time(&ended(messages))),
        ("Turns", turns(messages).to_string()),
        ("Tokens", tokens(&document["totals"]["usage"])),
        ("Cost", cost(&document["totals"])),
    ];
    if document["branches"] == "all" {
        facts.push(("Branches", "every branch, in creation order".to_owned()));
    }
    format!(
        "<header class=\"session\">\n<h1>{}</h1>\n{}</header>\n",
        escape(title),
        definitions(&facts)
    )
}

fn definitions(facts: &[(&str, String)]) -> String {
    let mut out = String::from("<dl class=\"facts\">\n");
    for (name, value) in facts {
        let _ = writeln!(out, "<dt>{name}</dt><dd>{value}</dd>");
    }
    out.push_str("</dl>\n");
    out
}

/// The messages in order, children after the call that started them, the Goals and
/// Workflow Runs, then the children no call names.
fn conversation(document: &Value) -> String {
    let (mut placed, unplaced) = place_children(document);
    let mut out = String::new();
    for message in list(&document["messages"]) {
        match message["kind"].as_str() {
            Some("user") => out.push_str(&user(message)),
            Some("assistant") => out.push_str(&assistant(message, &mut placed)),
            _ => out.push_str(&system(message)),
        }
    }
    out.push_str(&goals(&document["goals"]));
    out.push_str(&workflow_runs(&document["workflow_runs"]));
    if !unplaced.is_empty() {
        out.push_str("<section class=\"children\">\n<h2>Child Sessions</h2>\n");
        for child in unplaced {
            out.push_str(&child_session(child));
        }
        out.push_str("</section>\n");
    }
    out
}

/// Children by the call whose input or output names them, and the rest.
fn place_children(document: &Value) -> (HashMap<String, Vec<&Value>>, Vec<&Value>) {
    let calls: Vec<&Value> = list(&document["messages"])
        .iter()
        .flat_map(|m| list(&m["tools"]))
        .collect();
    let mut placed: HashMap<String, Vec<&Value>> = HashMap::new();
    let mut unplaced = Vec::new();
    for child in list(&document["children"]) {
        let id = text(&child["session"]["id"]).unwrap_or_default();
        let starter = calls.iter().find(|call| {
            !id.is_empty()
                && (call["output"].as_str().is_some_and(|o| o.contains(id))
                    || call["input"].to_string().contains(id))
        });
        match starter.and_then(|call| text(&call["call_id"])) {
            Some(call) => placed.entry(call.to_owned()).or_default().push(child),
            None => unplaced.push(child),
        }
    }
    (placed, unplaced)
}

fn user(message: &Value) -> String {
    let mut meta = vec![
        "<span class=\"role\">User</span>".to_owned(),
        time(&message["promoted_ms"]),
    ];
    if let Some(source) = text(&message["source"]).filter(|s| *s != "user") {
        meta.push(format!("<span>via {}</span>", escape(source)));
    }
    let body: String = list(&message["parts"]).iter().map(part).collect();
    article("user", &meta, &body)
}

fn part(part: &Value) -> String {
    match part["type"].as_str() {
        Some("text") => markdown::to_html(text(&part["text"]).unwrap_or_default()),
        Some("image") => {
            let bytes = part["data"].as_str().map_or(0, |d| d.len() / 4 * 3);
            format!(
                "<p class=\"placeholder\">[image: {}, about {} bytes]</p>\n",
                show(&part["media_type"]),
                bytes
            )
        }
        Some("reasoning") => reasoning(text(&part["text"]).unwrap_or_default()),
        _ => format!("<pre>{}</pre>\n", escape(&pretty(part))),
    }
}

fn assistant(message: &Value, children: &mut HashMap<String, Vec<&Value>>) -> String {
    let mut meta = vec![
        "<span class=\"role\">Assistant</span>".to_owned(),
        format!("<span>{}</span>", escape(&model(message))),
    ];
    if let Some(agent) = text(&message["agent"]) {
        meta.push(format!("<span>@{}</span>", escape(agent)));
    }
    meta.push(time(&message["started_ms"]));
    if let Some(took) = duration(&message["started_ms"], &message["ended_ms"]) {
        meta.push(format!("<span>{took}</span>"));
    }
    let mut body = String::new();
    if let Some(thinking) = text(&message["reasoning"]) {
        body.push_str(&reasoning(thinking));
    }
    if let Some(answer) = text(&message["text"]) {
        body.push_str(&markdown::to_html(answer));
    }
    for call in list(&message["tools"]) {
        body.push_str(&tool(call));
        let id = text(&call["call_id"]).unwrap_or_default();
        for child in children.remove(id).unwrap_or_default() {
            body.push_str(&child_session(child));
        }
    }
    if let Some(error) = text(&message["error"]) {
        let kind = text(&message["error_kind"]).unwrap_or("error");
        let _ = writeln!(
            body,
            "<p class=\"error\">Step failed ({}): {}</p>",
            escape(kind),
            escape(error)
        );
    }
    article("assistant", &meta, &body)
}

fn system(message: &Value) -> String {
    let meta = ["<span class=\"role\">System</span>".to_owned()];
    let body = format!(
        "<pre class=\"wrap\">{}</pre>\n",
        escape(text(&message["text"]).unwrap_or_default())
    );
    article("system", &meta, &body)
}

fn article(role: &str, meta: &[String], body: &str) -> String {
    format!(
        "<article class=\"msg {role}\">\n<div class=\"meta\">{}</div>\n<div class=\"body\">\n{body}</div>\n</article>\n",
        meta.join(" ")
    )
}

fn reasoning(thinking: &str) -> String {
    format!(
        "<details class=\"reasoning\"><summary>Reasoning</summary>\n<div class=\"body\">\n{}</div></details>\n",
        markdown::to_html(thinking)
    )
}

fn tool(call: &Value) -> String {
    let status = text(&call["status"]).unwrap_or("unknown");
    let class = match status {
        "ok" => "ok",
        "error" | "interrupted" | "outcome_unknown" => "failed",
        _ => "pending",
    };
    let mut out = format!(
        "<details class=\"tool {class}\"><summary><span class=\"tool-name\">{}</span> <span class=\"status\">{}</span>",
        escape(text(&call["name"]).unwrap_or("tool")),
        escape(status)
    );
    if call["attempt"].as_u64().unwrap_or(1) > 1 {
        let _ = write!(out, " <span>attempt {}</span>", number(&call["attempt"]));
    }
    out.push_str("</summary>\n<h4>Input</h4>\n");
    let input = match &call["input"] {
        Value::Null => text(&call["arguments"]).unwrap_or_default().to_owned(),
        input => pretty(input),
    };
    let _ = writeln!(out, "<pre>{}</pre>", escape(&input));
    if let Some(output) = text(&call["output"]) {
        out.push_str("<h4>Output</h4>\n");
        out.push_str(&tool_output(output));
    }
    out.push_str("</details>\n");
    out
}

fn tool_output(output: &str) -> String {
    let total = output.chars().count();
    if total <= OUTPUT_LIMIT {
        return format!("<pre>{}</pre>\n", escape(output));
    }
    let kept: String = output.chars().take(OUTPUT_LIMIT).collect();
    format!(
        "<pre>{}</pre>\n<p class=\"note\">Output truncated: showing {OUTPUT_LIMIT} of {total} characters. The JSON export has all of it.</p>\n",
        escape(&kept)
    )
}

fn goals(goals: &Value) -> String {
    let goals = list(goals);
    if goals.is_empty() {
        return String::new();
    }
    let mut out = String::from("<section class=\"goals\">\n<h2>Goals</h2>\n<ul>\n");
    for goal in goals {
        let _ = writeln!(
            out,
            "<li>{} <span class=\"status\">{}</span> <span>{} turns · ${:.4}</span></li>",
            show(&goal["condition"]),
            show(&goal["status"]),
            number(&goal["turns"]),
            goal["cost_usd"].as_f64().unwrap_or(0.0)
        );
    }
    out.push_str("</ul>\n</section>\n");
    out
}

fn workflow_runs(runs: &Value) -> String {
    let runs = list(runs);
    if runs.is_empty() {
        return String::new();
    }
    let mut out = String::from("<section class=\"runs\">\n<h2>Workflow Runs</h2>\n<ul>\n");
    for run in runs {
        let _ = writeln!(
            out,
            "<li>{} <span class=\"status\">{}</span> <code>{}</code></li>",
            show(&run["name"]),
            show(&run["status"]),
            show(&run["id"])
        );
    }
    out.push_str("</ul>\n</section>\n");
    out
}

/// A child Session, nested: its facts and its own conversation.
fn child_session(child: &Value) -> String {
    let session = &child["session"];
    let title = text(&session["title"])
        .or_else(|| text(&session["id"]))
        .unwrap_or("Child Session");
    let facts = [
        ("Session", show(&session["id"])),
        ("Agent", show(&session["agent"])),
        ("Tokens", tokens(&child["totals"]["usage"])),
        ("Cost", cost(&child["totals"])),
    ];
    format!(
        "<section class=\"child\">\n<details open><summary>Subagent: {}</summary>\n{}{}</details>\n</section>\n",
        escape(title),
        definitions(&facts),
        conversation(child)
    )
}

fn closing(document: &Value, generator: &str) -> String {
    let mut out = format!(
        "<footer>Exported by {} on {}.",
        escape(generator),
        date(document["exported_ms"].as_i64().unwrap_or(0))
    );
    if document["sanitized"] == true {
        out.push(' ');
        out.push_str(SANITIZED);
    }
    out.push_str("</footer>\n");
    out
}

/// Every `provider/model` the assistant used, in order of first use, or the Session's.
fn models(session: &Value, messages: &[Value]) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    for message in messages.iter().filter(|m| m["kind"] == "assistant") {
        let name = model(message);
        if !name.is_empty() && !seen.contains(&name) {
            seen.push(name);
        }
    }
    if seen.is_empty() {
        seen.extend(text(&session["model"]).map(str::to_owned));
    }
    seen
}

fn model(message: &Value) -> String {
    match (text(&message["provider"]), text(&message["model"])) {
        (Some(provider), Some(model)) => format!("{provider}/{model}"),
        (None, Some(model)) => model.to_owned(),
        _ => String::new(),
    }
}

/// The last step's end.
fn ended(messages: &[Value]) -> Value {
    messages
        .iter()
        .filter_map(|m| m["ended_ms"].as_i64())
        .max()
        .map_or(Value::Null, Value::from)
}

fn turns(messages: &[Value]) -> usize {
    messages.iter().filter(|m| m["kind"] == "user").count()
}

fn tokens(usage: &Value) -> String {
    format!(
        "{} in · {} out · {} reasoning · {} cache read · {} cache write",
        number(&usage["input"]),
        number(&usage["output"]),
        number(&usage["reasoning"]),
        number(&usage["cache_read"]),
        number(&usage["cache_write"])
    )
}

fn cost(totals: &Value) -> String {
    let mut out = format!("${:.4}", totals["cost"].as_f64().unwrap_or(0.0));
    let unpriced = totals["unpriced_steps"].as_u64().unwrap_or(0);
    if unpriced > 0 {
        let steps = if unpriced == 1 { "step" } else { "steps" };
        let _ = write!(out, " (at least; {unpriced} unpriced {steps})");
    }
    out
}

fn time(value: &Value) -> String {
    match value.as_i64() {
        Some(ms) => format!("<time>{}</time>", date(ms)),
        None => String::new(),
    }
}

fn date(ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(ms)
        .map(|at| at.format("%Y-%m-%d %H:%M:%S UTC").to_string())
        .unwrap_or_default()
}

fn duration(started: &Value, ended: &Value) -> Option<String> {
    let ms = ended.as_i64()? - started.as_i64()?;
    (ms >= 0).then(|| format!("{:.1} s", ms as f64 / 1000.0))
}

fn number(value: &Value) -> String {
    value.as_u64().unwrap_or(0).to_string()
}

/// A string value escaped, or empty.
fn show(value: &Value) -> String {
    escape(text(value).unwrap_or_default())
}

fn text(value: &Value) -> Option<&str> {
    value.as_str().filter(|s| !s.is_empty())
}

fn list(value: &Value) -> &[Value] {
    value.as_array().map_or(&[], Vec::as_slice)
}

fn pretty(value: &Value) -> String {
    serde_json::to_string_pretty(value).unwrap_or_default()
}

const STYLE: &str = r#"
:root { --bg: #ffffff; --fg: #1f2328; --muted: #59636e; --line: #d1d9e0; --panel: #f6f8fa;
  --user: #ddf4ff; --add: #1a7f37; --error: #cf222e; color-scheme: light dark; }
@media (prefers-color-scheme: dark) {
  :root { --bg: #0d1117; --fg: #e6edf3; --muted: #9198a1; --line: #3d444d; --panel: #151b23;
    --user: #12263a; --add: #3fb950; --error: #f85149; }
}
* { box-sizing: border-box; }
body { margin: 0; background: var(--bg); color: var(--fg);
  font: 15px/1.55 -apple-system, BlinkMacSystemFont, "Segoe UI", Helvetica, Arial, sans-serif; }
main { max-width: 960px; margin: 0 auto; padding: 24px 16px 48px; }
h1 { font-size: 1.6em; margin: 0 0 12px; overflow-wrap: anywhere; }
h2 { font-size: 1.2em; } h3, h4 { font-size: 1em; margin: 12px 0 4px; }
.facts { display: grid; grid-template-columns: max-content 1fr; gap: 2px 16px; margin: 0;
  color: var(--muted); }
.facts dt { font-weight: 600; } .facts dd { margin: 0; overflow-wrap: anywhere; }
header.session { border-bottom: 1px solid var(--line); padding-bottom: 16px; margin-bottom: 16px; }
.msg { border: 1px solid var(--line); border-radius: 8px; margin: 14px 0; overflow: hidden; }
.msg.user { background: var(--user); }
.msg .meta { display: flex; flex-wrap: wrap; gap: 10px; padding: 6px 12px; font-size: 0.85em;
  color: var(--muted); border-bottom: 1px solid var(--line); }
.msg .role { font-weight: 700; color: var(--fg); }
.msg > .body { padding: 4px 12px 10px; overflow-wrap: anywhere; }
pre, code { font: 13px/1.45 ui-monospace, SFMono-Regular, Menlo, Consolas, monospace; }
pre { background: var(--panel); border: 1px solid var(--line); border-radius: 6px; padding: 8px 10px;
  overflow-x: auto; white-space: pre; }
pre.wrap, .raw { white-space: pre-wrap; }
:not(pre) > code { background: var(--panel); padding: 1px 4px; border-radius: 4px; }
details { border: 1px solid var(--line); border-radius: 6px; margin: 8px 0; padding: 0 10px; }
details > summary { cursor: pointer; padding: 6px 0; color: var(--muted); }
details[open] > summary { border-bottom: 1px solid var(--line); margin-bottom: 6px; }
.tool-name { font-weight: 700; color: var(--fg); font-family: ui-monospace, Menlo, Consolas, monospace; }
.tool.failed .status, .error { color: var(--error); }
.tool.ok .status { color: var(--add); }
.link { text-decoration: underline; } .url, .note, .placeholder { color: var(--muted); }
.child { margin-left: 12px; border-left: 3px solid var(--line); padding-left: 10px; }
blockquote { margin: 0; padding-left: 12px; border-left: 3px solid var(--line); color: var(--muted); }
table { border-collapse: collapse; } th, td { border: 1px solid var(--line); padding: 3px 8px; }
footer { margin-top: 32px; color: var(--muted); font-size: 0.85em; }
"#;

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn long_output_is_truncated_with_a_note() {
        let call = json!({ "call_id": "c", "name": "bash", "status": "ok",
            "input": { "command": "yes" }, "output": "y".repeat(OUTPUT_LIMIT + 5) });
        let html = tool(&call);
        assert!(html.contains("showing 20000 of 20005 characters"), "{html}");
        assert!(!html.contains(&"y".repeat(OUTPUT_LIMIT + 1)));
    }

    #[test]
    fn page_is_escaped_and_self_contained() {
        let doc = json!({
            "session": { "id": "ses_1", "title": "<b>t</b>", "directory": "/x\"y" },
            "messages": [
                { "kind": "user", "id": "m1", "parts": [{ "type": "image", "media_type": "image/png",
                  "data": "AAAA" }] },
                { "kind": "system", "id": "m2", "text": "<script>alert(1)</script>" },
            ],
            "sanitized": true,
            "exported_ms": 0,
        });
        let page = render(&doc, "cyber 9.9.9");
        assert!(page.contains("&lt;b&gt;t&lt;/b&gt;"), "{page}");
        assert!(page.contains("/x&quot;y"), "{page}");
        assert!(page.contains("[image: image/png, about 3 bytes]"), "{page}");
        assert!(page.contains("&lt;script&gt;"), "{page}");
        assert!(
            !page.contains("<script") && !page.contains("url("),
            "{page}"
        );
        assert!(page.contains("Exported by cyber 9.9.9 on 1970-01-01 00:00:00 UTC."));
        assert!(page.contains("Sanitized:"));
    }
}
