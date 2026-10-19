//! Annotated member edits of one config document (`compat-import` → Idempotent re-import).
//!
//! An inserted member gets a line comment `// <marker> <note>` on the line above it, and
//! every other byte of the document, comments included, is kept. Objects are indented two
//! spaces per level. The caller parses the edited text and compares it with the value it
//! expects before writing anything.
use serde_json::{Value, json};

use super::edit::{find_member, skip_whitespace, string_end, value_end};
use super::jsonc;

/// Objects nested deeper than this are not searched for annotations.
const MAX_DEPTH: usize = 64;

/// A member preceded by an annotation comment: its JSON pointer and the rest of the
/// comment line after the marker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Annotation {
    pub pointer: String,
    pub note: String,
}

struct Member {
    name: String,
    /// Where the text before the member's name starts: just after `{` or `,`.
    gap: usize,
    name_start: usize,
    value_start: usize,
    value_end: usize,
}

fn malformed() -> String {
    "config is not well-formed JSON".into()
}

/// Every member of every object whose leading comment holds `marker`.
pub(crate) fn annotations(text: &str, marker: &str) -> Result<Vec<Annotation>, String> {
    let blank = jsonc::to_json(text);
    let bytes = blank.as_bytes();
    let open = root(bytes)?;
    let mut out = Vec::new();
    collect(text, bytes, open, "", marker, &mut out, 0)?;
    Ok(out)
}

fn collect(
    text: &str,
    bytes: &[u8],
    open: usize,
    prefix: &str,
    marker: &str,
    out: &mut Vec<Annotation>,
    depth: usize,
) -> Result<(), String> {
    if depth >= MAX_DEPTH {
        return Ok(());
    }
    for member in members(bytes, open)? {
        let pointer = pointer_child(prefix, &member.name);
        if let Some(at) = marker_offset(text, &member, open, marker) {
            let line_end = line_end(text, at, member.name_start);
            out.push(Annotation {
                pointer: pointer.clone(),
                note: text[at + marker.len()..line_end].trim().to_owned(),
            });
        }
        if bytes.get(member.value_start) == Some(&b'{') {
            collect(
                text,
                bytes,
                member.value_start,
                &pointer,
                marker,
                out,
                depth + 1,
            )?;
        }
    }
    Ok(())
}

/// `text` with `value` inserted as the member at `path`, preceded by `// <marker> <note>`.
/// Absent parent objects are created, and the annotation goes on the outermost member
/// created; `note` gets that member's index in `path` and its value. A parent that is not an object, or a
/// member that already exists, is refused.
pub(crate) fn insert(
    text: &str,
    path: &[String],
    value: &Value,
    marker: &str,
    note: &dyn Fn(usize, &Value) -> String,
) -> Result<String, String> {
    let blank = jsonc::to_json(text);
    let bytes = blank.as_bytes();
    let (mut object, mut depth) = (root(bytes)?, 0);
    while depth + 1 < path.len() {
        let Some((_, start, _)) = find_member(bytes, object + 1, &path[depth])? else {
            break;
        };
        if bytes.get(start) != Some(&b'{') {
            return Err(format!("`{}` is not an object", path[depth]));
        }
        object = start;
        depth += 1;
    }
    let key = path.get(depth).ok_or("an empty member path")?;
    if find_member(bytes, object + 1, key)?.is_some() {
        return Err(format!("`{key}` is already set"));
    }
    let nested = path[depth + 1..]
        .iter()
        .rev()
        .fold(value.clone(), |inner, name| json!({ name: inner }));
    let indent = "  ".repeat(depth + 1);
    let next = skip_whitespace(bytes, object + 1);
    let separator = if bytes.get(next) == Some(&b'}') {
        format!("\n{}", "  ".repeat(depth))
    } else {
        ",".into()
    };
    let entry = format!(
        "\n{indent}// {marker} {}\n{indent}{}: {}{separator}",
        note(depth, &nested),
        Value::from(key.as_str()),
        pretty(&nested, &indent)?
    );
    Ok(format!(
        "{}{entry}{}",
        &text[..=object],
        &text[object + 1..]
    ))
}

/// `text` with the annotated member at `path` set to `value` and its annotation note
/// replaced by `note`. A member without an annotation is refused.
pub(crate) fn replace(
    text: &str,
    path: &[String],
    value: &Value,
    marker: &str,
    note: &str,
) -> Result<String, String> {
    let blank = jsonc::to_json(text);
    let bytes = blank.as_bytes();
    let (last, parents) = path.split_last().ok_or("an empty member path")?;
    let mut object = root(bytes)?;
    for name in parents {
        let start = find_member(bytes, object + 1, name)?
            .map(|(_, start, _)| start)
            .filter(|start| bytes.get(*start) == Some(&b'{'))
            .ok_or_else(|| format!("`{name}` is not an object"))?;
        object = start;
    }
    let member = members(bytes, object)?
        .into_iter()
        .find(|m| &m.name == last)
        .ok_or_else(|| format!("`{last}` is not set"))?;
    let at = marker_offset(text, &member, object, marker)
        .ok_or_else(|| format!("`{last}` has no import annotation"))?;
    let comment_end = line_end(text, at, member.name_start);
    let indent = "  ".repeat(path.len());
    Ok(format!(
        "{}{marker} {note}{}{}{}",
        &text[..at],
        &text[comment_end..member.value_start],
        pretty(value, &indent)?,
        &text[member.value_end..]
    ))
}

fn root(bytes: &[u8]) -> Result<usize, String> {
    let open = skip_whitespace(bytes, 0);
    if bytes.get(open) != Some(&b'{') {
        return Err("config must be a JSON object".into());
    }
    Ok(open)
}

/// The members of the object opened at `open`, in order.
fn members(bytes: &[u8], open: usize) -> Result<Vec<Member>, String> {
    let mut out = Vec::new();
    let mut at = open + 1;
    loop {
        let gap = at;
        at = skip_whitespace(bytes, at);
        match bytes.get(at) {
            Some(b'}') => return Ok(out),
            Some(b'"') => {}
            _ => return Err(malformed()),
        }
        let name_end = string_end(bytes, at).ok_or_else(malformed)?;
        let colon = skip_whitespace(bytes, name_end);
        if bytes.get(colon) != Some(&b':') {
            return Err(malformed());
        }
        let value_start = skip_whitespace(bytes, colon + 1);
        let end = value_end(bytes, value_start).ok_or_else(malformed)?;
        let name = serde_json::from_slice(&bytes[at..name_end]).map_err(|_| malformed())?;
        out.push(Member {
            name,
            gap,
            name_start: at,
            value_start,
            value_end: end,
        });
        at = skip_whitespace(bytes, end);
        if bytes.get(at) == Some(&b',') {
            at += 1;
        }
    }
}

/// Where `marker` starts in the comment lines just above `member`. A comment on the line
/// of the previous member's comma belongs to that member, so it is skipped.
fn marker_offset(text: &str, member: &Member, open: usize, marker: &str) -> Option<usize> {
    let gap = &text[member.gap..member.name_start];
    let own = if member.gap == open + 1 {
        0
    } else {
        gap.find('\n')?
    };
    gap[own..]
        .rfind(marker)
        .map(|offset| member.gap + own + offset)
}

fn line_end(text: &str, at: usize, limit: usize) -> usize {
    text[at..limit]
        .find('\n')
        .map_or(limit, |offset| at + offset)
}

fn pointer_child(prefix: &str, name: &str) -> String {
    format!("{prefix}/{}", name.replace('~', "~0").replace('/', "~1"))
}

/// `value` pretty-printed, continuation lines indented by `indent`.
fn pretty(value: &Value, indent: &str) -> Result<String, String> {
    let text = serde_json::to_string_pretty(value).map_err(|e| e.to_string())?;
    Ok(text.replace('\n', &format!("\n{indent}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    const MARK: &str = "x-imported-from:";

    fn segments(path: &[&str]) -> Vec<String> {
        path.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn inserted_members_keep_comments_and_carry_their_annotation() {
        let text = "{\n  // mine\n  \"model\": \"a/b\", // trailing\n  \"mcp\": { \"x\": {\"type\": \"local\"} },\n}\n";
        let edited = insert(text, &segments(&["mode"]), &json!("plan"), MARK, &|_, _| {
            "src one".into()
        })
        .unwrap();
        let edited = insert(
            &edited,
            &segments(&["mcp", "y"]),
            &json!({"type":"remote","url":"https://y"}),
            MARK,
            &|_, _| "src two".into(),
        )
        .unwrap();
        let edited = insert(
            &edited,
            &segments(&["providers", "corp", "name"]),
            &json!("Corp"),
            MARK,
            &|_, v| format!("src three {v}"),
        )
        .unwrap();
        assert!(
            edited.contains("// mine") && edited.contains("// trailing"),
            "{edited}"
        );
        let parsed = jsonc::parse("t", &edited).unwrap();
        assert_eq!(parsed["mode"], "plan");
        assert_eq!(parsed["mcp"]["y"]["url"], "https://y");
        assert_eq!(parsed["mcp"]["x"]["type"], "local");
        assert_eq!(parsed["providers"]["corp"]["name"], "Corp");
        assert_eq!(
            annotations(&edited, MARK).unwrap(),
            vec![
                Annotation {
                    pointer: "/providers".into(),
                    note: "src three {\"corp\":{\"name\":\"Corp\"}}".into()
                },
                Annotation {
                    pointer: "/mode".into(),
                    note: "src one".into()
                },
                Annotation {
                    pointer: "/mcp/y".into(),
                    note: "src two".into()
                },
            ]
        );
        assert!(
            insert(&edited, &segments(&["mode"]), &json!("x"), MARK, &|_, _| {
                "n".into()
            })
            .is_err()
        );
        assert!(
            insert(
                &edited,
                &segments(&["model", "x"]),
                &json!(1),
                MARK,
                &|_, _| "n".into()
            )
            .is_err()
        );
    }

    #[test]
    fn a_comment_after_the_previous_comma_is_not_the_next_members_annotation() {
        let text = "{\"a\": 1, // x-imported-from: wrong\n \"b\": 2}";
        assert!(annotations(text, MARK).unwrap().is_empty());
    }

    #[test]
    fn replacing_an_annotated_member_updates_its_value_and_note_only() {
        let text = insert(
            "{\"mcp\": {}}",
            &segments(&["mcp", "x"]),
            &json!({"a":1}),
            MARK,
            &|_, _| "old".into(),
        )
        .unwrap();
        let replaced = replace(
            &text,
            &segments(&["mcp", "x"]),
            &json!({"a":2}),
            MARK,
            "new",
        )
        .unwrap();
        let parsed = jsonc::parse("t", &replaced).unwrap();
        assert_eq!(parsed["mcp"]["x"]["a"], 2);
        assert_eq!(annotations(&replaced, MARK).unwrap()[0].note, "new");
        let plain = "{\"mcp\": {\"x\": {\"a\": 1}}}";
        assert!(replace(plain, &segments(&["mcp", "x"]), &json!(2), MARK, "n").is_err());
    }
}
