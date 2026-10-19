//! Markdown agent files (`compat-import` → Claude Code sources, OpenCode sources):
//! `.claude/agents/*.md`, `.opencode/agents/*.md` and `.opencode/modes/*.md` become
//! native `agents.<name>` profiles. The body is the system prompt; frontmatter fields map
//! one by one, and a field without an equivalent is reported, never guessed.
use super::adapted::Adapted;
use super::provenance::{SourceReference, child};
use super::{SourceFile, SourceKind, SourceTool};
use serde_json::{Map, Value, json};
use std::path::Path;

const FRONTMATTER_LIMIT: usize = 16 * 1024;
const PROMPT_LIMIT: usize = 64 * 1024;

/// Convert one agent file's `text` into `adapted`.
pub(super) fn convert(source: &SourceFile, text: &str, adapted: &mut Adapted) {
    let path = &source.path;
    let Some((frontmatter, body)) = split(text) else {
        adapted.skip_file(
            path,
            "source",
            "not imported",
            "agent frontmatter is malformed or too large",
        );
        return;
    };
    if body.len() > PROMPT_LIMIT {
        adapted.skip_file(path, "body", "not imported", "agent prompt exceeds 64 KiB");
        return;
    }
    let converted = match source.tool {
        SourceTool::Claude => claude(path, &frontmatter, body, adapted),
        _ => opencode(source, frontmatter, body, adapted),
    };
    let Some((name, agent)) = converted else {
        return;
    };
    if !adapted.config.get("agents").is_some_and(Value::is_object) {
        adapted.config["agents"] = json!({});
    }
    adapted.config["agents"][&name] = agent;
    adapted.fields.push((
        child("/agents", &name),
        vec![SourceReference {
            source: path.clone(),
            field: "agent".into(),
        }],
    ));
}

/// A plugin's agent file (`plugins-marketplace` → Component discovery defaults), in the
/// Claude Code agent format: `(name, native profile, notes on fields left out)`, or why
/// the agent cannot load.
pub(crate) fn plugin_agent(
    path: &Path,
    text: &str,
) -> Result<(String, Value, Vec<String>), String> {
    let (frontmatter, body) =
        split(text).ok_or("agent frontmatter is malformed or exceeds 16 KiB")?;
    if body.len() > PROMPT_LIMIT {
        return Err("agent prompt exceeds 64 KiB".into());
    }
    let mut adapted = Adapted::new();
    let converted = claude(path, &frontmatter, body, &mut adapted);
    let notes: Vec<String> = adapted
        .pending
        .iter()
        .map(|pending| format!("{}: {}", pending.field, pending.reason))
        .collect();
    match converted {
        Some((name, agent)) => Ok((name, agent, notes)),
        None => Err(notes.join("; ")),
    }
}

/// YAML frontmatter as a JSON object (empty without one) and the body after it.
fn split(text: &str) -> Option<(Map<String, Value>, &str)> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let Some(rest) = text
        .strip_prefix("---\n")
        .or_else(|| text.strip_prefix("---\r\n"))
    else {
        return Some((Map::new(), text));
    };
    let mut offset = 0;
    for line in rest.split_inclusive('\n') {
        if line.trim_end_matches(['\r', '\n']) == "---" {
            let yaml = &rest[..offset];
            let body = rest[offset + line.len()..].trim_start_matches(['\r', '\n']);
            if yaml.trim().is_empty() {
                return Some((Map::new(), body));
            }
            let value: Value = serde_yaml_ng::from_str(yaml).ok()?;
            return Some((value.as_object()?.clone(), body));
        }
        offset += line.len();
        if offset > FRONTMATTER_LIMIT {
            return None;
        }
    }
    None
}

fn stem(path: &Path) -> Option<String> {
    path.file_stem()?.to_str().map(str::to_owned)
}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && !["max_concurrent", "max_depth", "result_max_bytes"].contains(&name)
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
}

/// `.claude/agents/<name>.md`: `name`, `description`, `tools`, `disallowedTools`,
/// `model`, `color`, `permissionMode` and `skills`; the agent is a subagent.
fn claude(
    path: &Path,
    frontmatter: &Map<String, Value>,
    body: &str,
    adapted: &mut Adapted,
) -> Option<(String, Value)> {
    let name = match frontmatter.get("name") {
        Some(name) => name.as_str().map(str::to_owned),
        None => stem(path),
    };
    let Some(name) = name.filter(|n| valid_name(n)) else {
        adapted.skip_file(
            path,
            "frontmatter:name",
            "not imported",
            "agent name is unsupported",
        );
        return None;
    };
    let mut agent = json!({"mode":"subagent","system":body});
    for (key, value) in frontmatter {
        let field = format!("frontmatter:{key}");
        match claude_field(key, value, &mut agent) {
            Field::Mapped => (),
            Field::Skipped(reason) => adapted.skip_file(path, &field, "not imported", reason),
            Field::Refused(reason) => {
                adapted.skip_file(path, &field, "not imported", reason);
                return None;
            }
        }
    }
    if crate::config::resolve_agents(&json!({"agents":{&name:&agent}})).is_err() {
        adapted.skip_file(
            path,
            "source",
            "not imported",
            "converted agent fails native validation",
        );
        return None;
    }
    Some((name, agent))
}

enum Field {
    Mapped,
    /// Left out; the rest of the agent is still imported.
    Skipped(&'static str),
    /// The agent cannot be imported without it.
    Refused(&'static str),
}

fn claude_field(key: &str, value: &Value, agent: &mut Value) -> Field {
    let text = value.as_str();
    match key {
        "name" => Field::Mapped,
        "description" | "color" => match text {
            Some(text) => {
                agent[key] = json!(text);
                Field::Mapped
            }
            None => Field::Skipped("expected a string"),
        },
        "model" => match text {
            Some("inherit") => Field::Mapped,
            Some(model) => match super::claude_settings::model(model) {
                Some(model) => {
                    agent["model"] = json!(model);
                    Field::Mapped
                }
                None => Field::Skipped("model is not a Claude model or alias; the agent inherits"),
            },
            None => Field::Skipped("expected a string"),
        },
        "permissionMode" => match text.and_then(super::claude_settings::mode) {
            Some(mode) => {
                agent["permission_mode"] = json!(mode);
                Field::Mapped
            }
            None => Field::Refused("permission mode requires manual conversion"),
        },
        "tools" => match super::claude_settings::tools(value) {
            Some((tools, unknown)) => {
                agent["tools"]["allow"] = json!(tools);
                if unknown.is_empty() {
                    Field::Mapped
                } else {
                    Field::Skipped("allowed tools without a Cyber Code equivalent were left out")
                }
            }
            None => Field::Refused("tools must be a list of tool names"),
        },
        "disallowedTools" => match super::claude_settings::tools(value) {
            Some((tools, unknown)) if unknown.is_empty() => {
                agent["tools"]["deny"] = json!(tools);
                Field::Mapped
            }
            _ => Field::Refused("denied tools without a Cyber Code equivalent would be allowed"),
        },
        "skills" => match value.as_array().filter(|a| a.iter().all(Value::is_string)) {
            Some(skills) => {
                agent["skills"] = json!(skills);
                Field::Mapped
            }
            None => Field::Skipped("skills must be a list of names"),
        },
        _ => Field::Skipped("agent field has no Cyber Code equivalent"),
    }
}

/// `.opencode/agents/<id>.md` (default mode `all`) and `.opencode/modes/<id>.md` (a
/// primary agent), through the inline OpenCode agent converter.
fn opencode(
    source: &SourceFile,
    mut frontmatter: Map<String, Value>,
    body: &str,
    adapted: &mut Adapted,
) -> Option<(String, Value)> {
    let path = &source.path;
    let nested = path.parent().and_then(Path::file_name).is_none_or(|dir| {
        !["agents", "agent", "modes", "mode"].contains(&dir.to_str().unwrap_or(""))
    });
    let Some(id) = stem(path).filter(|_| !nested) else {
        adapted.skip_file(
            path,
            "source",
            "not imported",
            "nested agent files need manual conversion",
        );
        return None;
    };
    if frontmatter.contains_key("prompt") {
        adapted.skip_file(
            path,
            "frontmatter:prompt",
            "not imported",
            "the body is the prompt",
        );
        return None;
    }
    frontmatter.insert("prompt".into(), json!(body));
    let primary = source.kind == SourceKind::Mode;
    match super::opencode_settings::markdown_agent(&id, &Value::Object(frontmatter), primary) {
        Ok((agent, result)) => {
            for pending in result.not_imported {
                let field = pending.field.trim_start_matches('/');
                let field = if field == "prompt" {
                    "body".to_owned()
                } else {
                    format!("frontmatter:{field}")
                };
                adapted.skip_file(path, &field, "not imported", pending.reason);
            }
            agent.map(|agent| (id, agent))
        }
        Err(error) => {
            adapted.skip_file(path, "source", "not imported", error.reason);
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::SourceLayer;
    use super::*;
    use std::path::PathBuf;

    fn file(tool: SourceTool, kind: SourceKind, path: &str) -> SourceFile {
        SourceFile {
            tool,
            layer: SourceLayer::Project,
            kind,
            path: PathBuf::from(path),
        }
    }

    #[test]
    fn claude_agents_map_frontmatter_and_report_the_rest() {
        let mut adapted = Adapted::new();
        convert(
            &file(
                SourceTool::Claude,
                SourceKind::Agent,
                "/r/.claude/agents/reviewer.md",
            ),
            "---\nname: reviewer\ndescription: Reviews diffs\ntools: Read, Grep, Glob, Frobnicate\nmodel: sonnet\ncolor: blue\nmemory: project\n---\nYou review code.\n",
            &mut adapted,
        );
        assert_eq!(
            adapted.config["agents"]["reviewer"],
            json!({"mode":"subagent","system":"You review code.\n","description":"Reviews diffs",
                "tools":{"allow":["read","grep","glob"]},"model":"anthropic/claude-sonnet-5-5","color":"blue"})
        );
        let skipped: Vec<_> = adapted.pending.iter().map(|p| p.field.as_str()).collect();
        assert_eq!(skipped, ["frontmatter:tools", "frontmatter:memory"]);
        convert(
            &file(
                SourceTool::Claude,
                SourceKind::Agent,
                "/r/.claude/agents/x.md",
            ),
            "---\ndescription: X\ndisallowedTools: Frobnicate\n---\nx",
            &mut adapted,
        );
        assert!(adapted.config["agents"].get("x").is_none());
    }

    #[test]
    fn opencode_agents_and_modes_use_the_file_name_and_their_default_mode() {
        let mut adapted = Adapted::new();
        convert(
            &file(
                SourceTool::OpenCode,
                SourceKind::Agent,
                "/r/.opencode/agents/docs.md",
            ),
            "---\ndescription: Writes docs\nmodel: anthropic/claude-sonnet-5-5\ntemperature: 0.2\n---\nWrite docs.",
            &mut adapted,
        );
        convert(
            &file(
                SourceTool::OpenCode,
                SourceKind::Mode,
                "/r/.opencode/modes/focus.md",
            ),
            "Stay focused.",
            &mut adapted,
        );
        assert_eq!(adapted.config["agents"]["docs"]["mode"], "all");
        assert_eq!(adapted.config["agents"]["docs"]["system"], "Write docs.");
        assert_eq!(adapted.config["agents"]["focus"]["mode"], "primary");
        assert_eq!(adapted.pending[0].field, "frontmatter:temperature");
    }
}
