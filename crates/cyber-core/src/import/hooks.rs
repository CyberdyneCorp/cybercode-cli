//! Hook definitions of Claude Code settings (`hooks`) and Codex (`hooks.json`, `notify`),
//! as native `hooks` (`compat-import` → Claude Code sources, Codex sources).
//!
//! The source shapes match the native one (`event → [{ matcher, hooks: [handler] }]`), so
//! conversion renames rather than reinterprets: event names map one to one, PascalCase
//! tool matchers become native tool names, `statusMessage` becomes `status_message` and
//! `CLAUDE_PROJECT_DIR` in a command becomes `CYBER_PROJECT_DIR`. Anything else — an
//! event with no native equivalent, a matcher that is not a plain tool list, a handler
//! type or field without a native counterpart — is skipped and reported under
//! `not imported`; no hook is ever run during import. Imported project hooks still need
//! hook trust before they run.
use super::adapted::Adapted;
use super::provenance;
use serde_json::{Map, Value, json};

/// Events whose matcher selects a tool name.
const TOOL_EVENTS: [&str; 5] = [
    "PreToolUse",
    "PostToolUse",
    "PostToolUseFailure",
    "PermissionRequest",
    "PermissionDenied",
];

const EVENT_GROUPS: usize = 64;

/// Source tool names and their native equivalents; native names map to themselves.
pub(super) fn native_tool(name: &str) -> Option<&'static str> {
    Some(match name {
        "Bash" | "bash" | "shell" | "local_shell" => "bash",
        "Read" | "read" => "read",
        "Write" | "write" => "write",
        "Edit" | "MultiEdit" | "edit" => "edit",
        "NotebookEdit" | "notebook_edit" => "notebook_edit",
        "Glob" | "glob" => "glob",
        "Grep" | "grep" => "grep",
        "LS" | "list" => "list",
        "WebFetch" | "webfetch" => "webfetch",
        "WebSearch" | "websearch" => "websearch",
        "Task" | "Agent" | "agent" => "agent",
        "TodoWrite" | "todo" => "todo",
        "Skill" | "skill" => "skill",
        "PowerShell" | "powershell" => "powershell",
        "apply_patch" => "apply_patch",
        _ => return None,
    })
}

/// Convert the `hooks` object found at raw pointer `base` into `adapted.config.hooks`,
/// appending after hooks already converted there.
pub(super) fn convert(hooks: &Value, base: &str, adapted: &mut Adapted) {
    let Some(events) = hooks.as_object() else {
        adapted.skip(base, "not imported", "hooks must be an object of events");
        return;
    };
    for (event, groups) in events {
        let raw = provenance::child(base, event);
        if !crate::config::is_event_name(event) {
            adapted.skip(
                raw,
                "not imported",
                "hook event has no Cyber Code equivalent",
            );
            continue;
        }
        let Some(groups) = groups.as_array().filter(|g| g.len() <= EVENT_GROUPS) else {
            adapted.skip(raw, "not imported", "expected at most 64 hook groups");
            continue;
        };
        for (index, group) in groups.iter().enumerate() {
            let raw = provenance::child(&raw, &index.to_string());
            if let Some(native) = convert_group(event, group, &raw, adapted) {
                push_group(adapted, event, native, &[raw.as_str()]);
            }
        }
    }
}

/// A native `Notification` command hook running Codex's `notify` argv.
pub(super) fn notify(argv: &Value, adapted: &mut Adapted) {
    let parts: Option<Vec<&str>> = argv
        .as_array()
        .filter(|items| !items.is_empty() && items.len() <= 64)
        .and_then(|items| items.iter().map(Value::as_str).collect());
    let Some(parts) =
        parts.filter(|p| p.iter().all(|s| !s.is_empty() && !s.contains(['\0', '\n'])))
    else {
        adapted.skip(
            "/notify",
            "not imported",
            "notify must be a nonempty argument list",
        );
        return;
    };
    let command = parts.iter().map(|part| quote(part)).collect::<Vec<_>>();
    let command = if cfg!(windows) {
        format!("& {}", command.join(" "))
    } else {
        command.join(" ")
    };
    let group = json!({"hooks":[{"type":"command","command":command,
        "description":"Codex notify; the event JSON arrives on stdin, not as an argument"}]});
    push_group(adapted, "Notification", group, &["/notify"]);
}

fn push_group(adapted: &mut Adapted, event: &str, group: Value, raw: &[&str]) {
    if !adapted.config.get("hooks").is_some_and(Value::is_object) {
        adapted.config["hooks"] = json!({});
    }
    let list = adapted.config["hooks"]
        .as_object_mut()
        .unwrap()
        .entry(event)
        .or_insert_with(|| json!([]));
    let list = list.as_array_mut().unwrap();
    let pointer = provenance::child(&provenance::child("/hooks", event), &list.len().to_string());
    list.push(group);
    adapted.raw(pointer, raw);
}

fn convert_group(event: &str, group: &Value, raw: &str, adapted: &mut Adapted) -> Option<Value> {
    let Some(fields) = group.as_object() else {
        adapted.skip(raw, "not imported", "hook group must be an object");
        return None;
    };
    if let Some(key) = fields
        .keys()
        .find(|k| !["matcher", "hooks"].contains(&k.as_str()))
    {
        adapted.skip(
            provenance::child(raw, key),
            "not imported",
            "hook group field has no Cyber Code equivalent",
        );
        return None;
    }
    let mut native = Map::new();
    match fields.get("matcher").map(|m| matcher(event, m)) {
        None | Some(Ok(None)) => (),
        Some(Ok(Some(matcher))) => {
            native.insert("matcher".into(), json!(matcher));
        }
        Some(Err(reason)) => {
            adapted.skip(provenance::child(raw, "matcher"), "not imported", reason);
            return None;
        }
    }
    let handlers = handlers(fields.get("hooks"), raw, adapted);
    if handlers.is_empty() {
        return None;
    }
    native.insert("hooks".into(), Value::Array(handlers));
    let native = Value::Object(native);
    let check = json!({"hooks":{event:[native.clone()]}});
    if crate::config::HookSettings::from_config(&check).is_err() {
        adapted.skip(
            raw,
            "not imported",
            "converted hook group fails native validation",
        );
        return None;
    }
    Some(native)
}

fn handlers(value: Option<&Value>, raw: &str, adapted: &mut Adapted) -> Vec<Value> {
    let raw = provenance::child(raw, "hooks");
    let Some(items) = value.and_then(Value::as_array).filter(|h| h.len() <= 64) else {
        adapted.skip(raw, "not imported", "expected at most 64 hook handlers");
        return Vec::new();
    };
    let mut out = Vec::new();
    for (index, item) in items.iter().enumerate() {
        match handler(item) {
            Ok(handler) => out.push(handler),
            Err(reason) => adapted.skip(
                provenance::child(&raw, &index.to_string()),
                "not imported",
                reason,
            ),
        }
    }
    out
}

fn handler(value: &Value) -> Result<Value, &'static str> {
    let fields = value.as_object().ok_or("hook handler must be an object")?;
    let kind = fields.get("type").and_then(Value::as_str);
    let body = match kind {
        Some("command") => "command",
        Some("prompt") => "prompt",
        Some("http") => "url",
        _ => return Err("hook handler type has no Cyber Code equivalent"),
    };
    let mut native = json!({"type": kind});
    for (key, value) in fields {
        let target = match key.as_str() {
            "type" => continue,
            "command" | "prompt" | "url" if key == body => key.as_str(),
            "timeout" => "timeout",
            "async" => "async",
            "once" => "once",
            "statusMessage" | "status_message" => "status_message",
            "headers" => return Err("hook headers can hold credentials; port them by hand"),
            _ => return Err("hook handler field has no Cyber Code equivalent"),
        };
        native[target] = value.clone();
    }
    let text = native
        .get(body)
        .and_then(Value::as_str)
        .ok_or("hook handler is missing its command, prompt or url")?;
    if text.contains("{env:") || text.contains("{file:") {
        return Err("hook handler holds a substitution placeholder");
    }
    if body == "command" {
        native["command"] = json!(text.replace("CLAUDE_PROJECT_DIR", "CYBER_PROJECT_DIR"));
    }
    Ok(native)
}

/// A native matcher for a source matcher: `None` matches everything. Tool events accept
/// `|`-separated tool names and `mcp__` names (a trailing `.*` becomes `*`); other events
/// accept `|`-separated plain words.
fn matcher(event: &str, value: &Value) -> Result<Option<String>, &'static str> {
    let text = value.as_str().ok_or("hook matcher must be a string")?;
    if text.is_empty() || text == "*" || text == ".*" {
        return Ok(None);
    }
    let tool = TOOL_EVENTS.contains(&event);
    let mut names: Vec<String> = Vec::new();
    for token in text.split('|') {
        let name = if tool {
            tool_matcher(token)?
        } else if !token.is_empty()
            && token
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
        {
            token.to_owned()
        } else {
            return Err("hook matcher is not a plain list of names; convert it by hand");
        };
        if !names.contains(&name) {
            names.push(name);
        }
    }
    Ok(Some(if names.len() == 1 {
        names.remove(0)
    } else {
        format!("{{{}}}", names.join(","))
    }))
}

fn tool_matcher(token: &str) -> Result<String, &'static str> {
    if let Some(name) = native_tool(token) {
        return Ok(name.to_owned());
    }
    let (stem, wildcard) = token
        .strip_suffix(".*")
        .map_or((token, false), |s| (s, true));
    if stem.starts_with("mcp__")
        && stem
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
    {
        return Ok(if wildcard {
            format!("{stem}*")
        } else {
            stem.to_owned()
        });
    }
    Err("hook matcher names a tool without a Cyber Code equivalent")
}

/// One argument, quoted for the hook shell (POSIX `sh`, or PowerShell on Windows).
fn quote(part: &str) -> String {
    let plain = part
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b"_@%+=:,./-".contains(&b));
    if plain && !cfg!(windows) {
        part.to_owned()
    } else if cfg!(windows) {
        format!("'{}'", part.replace('\'', "''"))
    } else {
        format!("'{}'", part.replace('\'', "'\\''"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_matchers_and_handlers_convert_and_the_rest_is_reported() {
        let mut adapted = Adapted::new();
        convert(
            &json!({
                "PreToolUse":[
                    {"matcher":"Edit|MultiEdit|Write","hooks":[{"type":"command","command":"$CLAUDE_PROJECT_DIR/guard.sh","statusMessage":"Guarding"}]},
                    {"matcher":"mcp__github__.*","hooks":[{"type":"command","command":"gh.sh","timeout":5}]},
                    {"matcher":"^Bash$","hooks":[{"type":"command","command":"x"}]}
                ],
                "SessionStart":[{"matcher":"startup|resume","hooks":[{"type":"command","command":"hello"},{"type":"agent","prompt":"p"}]}],
                "BeforeEverything":[{"hooks":[{"type":"command","command":"x"}]}]
            }),
            "/hooks",
            &mut adapted,
        );
        assert_eq!(
            adapted.config,
            json!({"hooks":{
                "PreToolUse":[
                    {"matcher":"{edit,write}","hooks":[{"type":"command","command":"$CYBER_PROJECT_DIR/guard.sh","status_message":"Guarding"}]},
                    {"matcher":"mcp__github__*","hooks":[{"type":"command","command":"gh.sh","timeout":5}]}
                ],
                "SessionStart":[{"matcher":"{startup,resume}","hooks":[{"type":"command","command":"hello"}]}]
            }})
        );
        let skipped: Vec<_> = adapted
            .pending
            .iter()
            .map(|p| (p.field.as_str(), p.reason))
            .collect();
        assert_eq!(
            skipped,
            [
                (
                    "/hooks/PreToolUse/2/matcher",
                    "hook matcher names a tool without a Cyber Code equivalent"
                ),
                (
                    "/hooks/SessionStart/0/hooks/1",
                    "hook handler type has no Cyber Code equivalent"
                ),
                (
                    "/hooks/BeforeEverything",
                    "hook event has no Cyber Code equivalent"
                ),
            ]
        );
        assert_eq!(adapted.fields[1].0, "/hooks/PreToolUse/1");
        assert_eq!(adapted.fields[1].1[0].field, "/hooks/PreToolUse/1");
    }

    #[cfg(not(windows))]
    #[test]
    fn notify_argv_becomes_a_quoted_notification_command() {
        let mut adapted = Adapted::new();
        notify(
            &json!(["/opt/My App/notify", "turn-ended", "it's"]),
            &mut adapted,
        );
        assert_eq!(
            adapted.config["hooks"]["Notification"][0]["hooks"][0]["command"],
            "'/opt/My App/notify' turn-ended 'it'\\''s'"
        );
        assert_eq!(adapted.fields[0].1[0].field, "/notify");
    }
}
