//! Narrow edits of the user's global config document (`memory` → Memory toggles).
//!
//! Only one top-level key is replaced or inserted; every other byte of the document,
//! comments included, is kept. The edited text is parsed and compared with the original
//! before it replaces the file, so an edit that would change anything else is refused.
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

use serde_json::{Map, Value};

use super::{SCHEMA_URL, global_layer_paths, jsonc};

/// The largest global document an edit reads.
const MAX_DOCUMENT: u64 = 16 * 1024 * 1024;

/// Set top-level `key` of the highest-priority global document in `directory` to
/// `update(current)`, creating `cyber.jsonc` when no global document exists. Returns the
/// file written.
pub fn set_global_key(
    directory: &Path,
    key: &str,
    update: impl FnOnce(Option<&Value>) -> Value,
) -> Result<PathBuf, String> {
    static WRITER: Mutex<()> = Mutex::new(());
    let _writer = WRITER.lock().unwrap_or_else(PoisonError::into_inner);
    let target = target(directory)?;
    let original = read(&target)?;
    let document = parse(&original)?;
    let value = update(document.get(key));
    let edited = replace_top_level(&original, key, &value)?;
    verify(&document, &parse(&edited)?, key, &value)?;
    write(&target, &edited)?;
    Ok(target)
}

/// Set member `name` of the top-level object `key` in the config document `path` to
/// `value`, or remove it with `None`, keeping every other byte (comments included).
/// A missing document is created with `$schema`. Returns whether the member existed.
pub fn set_document_member(
    path: &Path,
    key: &str,
    name: &str,
    value: Option<&Value>,
) -> Result<bool, String> {
    static WRITER: Mutex<()> = Mutex::new(());
    let _writer = WRITER.lock().unwrap_or_else(PoisonError::into_inner);
    let target = if path.symlink_metadata().is_ok() {
        path.canonicalize()
            .map_err(|_| format!("{} is unavailable", path.display()))?
    } else {
        let parent = path.parent().unwrap_or(path);
        std::fs::create_dir_all(parent)
            .map_err(|_| format!("{} is unavailable", parent.display()))?;
        path.to_path_buf()
    };
    let original = read(&target)?;
    let document = parse(&original)?;
    let existed = document
        .get(key)
        .and_then(Value::as_object)
        .is_some_and(|members| members.contains_key(name));
    let mut expected = document.clone();
    let members = expected
        .entry(key)
        .or_insert_with(|| Value::Object(Map::new()));
    let Some(members) = members.as_object_mut() else {
        return Err(format!("`{key}` in {} is not an object", target.display()));
    };
    match value {
        Some(value) => {
            members.insert(name.into(), value.clone());
        }
        None => {
            members.remove(name);
        }
    }
    let edited = edit_member(&original, key, name, value)?;
    if parse(&edited)? != expected {
        return Err(format!("`{key}.{name}` cannot be edited in place"));
    }
    write(&target, &edited)?;
    Ok(existed)
}

/// `text` with member `name` of top-level object `key` set to `value` (or removed).
fn edit_member(text: &str, key: &str, name: &str, value: Option<&Value>) -> Result<String, String> {
    let blank = jsonc::to_json(text);
    let bytes = blank.as_bytes();
    let open = skip_whitespace(bytes, 0);
    if bytes.get(open) != Some(&b'{') {
        return Err("config must be a JSON object".into());
    }
    let Some((_, start, _)) = find_member(bytes, open + 1, key)? else {
        let Some(value) = value else {
            return Ok(text.to_owned());
        };
        let rendered = serde_json::to_string(value).map_err(|e| e.to_string())?;
        let next = skip_whitespace(bytes, open + 1);
        let separator = if bytes.get(next) == Some(&b'}') {
            "\n"
        } else {
            ","
        };
        let entry = format!(
            "\n  {}: {{\n    {}: {rendered}\n  }}{separator}",
            Value::from(key),
            Value::from(name)
        );
        return Ok(format!("{}{entry}{}", &text[..=open], &text[open + 1..]));
    };
    if bytes.get(start) != Some(&b'{') {
        return Err(format!("`{key}` is not an object"));
    }
    let member = find_member(bytes, start + 1, name)?;
    match (member, value) {
        (Some((_, value_start, value_end)), Some(value)) => {
            let rendered = serde_json::to_string(value).map_err(|e| e.to_string())?;
            Ok(format!(
                "{}{rendered}{}",
                &text[..value_start],
                &text[value_end..]
            ))
        }
        (Some((name_start, _, value_end)), None) => {
            Ok(remove_span(text, bytes, name_start, value_end))
        }
        (None, Some(value)) => {
            let rendered = serde_json::to_string(value).map_err(|e| e.to_string())?;
            let next = skip_whitespace(bytes, start + 1);
            let separator = if bytes.get(next) == Some(&b'}') {
                "\n  "
            } else {
                ","
            };
            let entry = format!("\n    {}: {rendered}{separator}", Value::from(name));
            Ok(format!("{}{entry}{}", &text[..=start], &text[start + 1..]))
        }
        (None, None) => Ok(text.to_owned()),
    }
}

/// Remove the member spanning `name_start..value_end` and exactly one adjoining comma,
/// leaving comments around it in place.
fn remove_span(text: &str, bytes: &[u8], name_start: usize, value_end: usize) -> String {
    let after = skip_whitespace(bytes, value_end);
    if bytes.get(after) == Some(&b',') {
        return format!("{}{}", &text[..name_start], &text[after + 1..]);
    }
    // The last member: drop the comma before it, if any.
    let before = bytes[..name_start]
        .iter()
        .rposition(|b| !b.is_ascii_whitespace())
        .filter(|&i| bytes[i] == b',');
    match before {
        Some(comma) => format!(
            "{}{}{}",
            &text[..comma],
            &text[comma + 1..name_start],
            &text[value_end..]
        ),
        None => format!("{}{}", &text[..name_start], &text[value_end..]),
    }
}

/// The global document a later layer cannot shadow: `cyber.jsonc`, else `cyber.json`.
/// A symlinked document is edited where it points.
fn target(directory: &Path) -> Result<PathBuf, String> {
    let [json, jsonc] = global_layer_paths(directory);
    let path = [jsonc.clone(), json]
        .into_iter()
        .find(|path| path.symlink_metadata().is_ok())
        .unwrap_or(jsonc);
    if path.symlink_metadata().is_ok() {
        return path
            .canonicalize()
            .map_err(|_| format!("{} is unavailable", path.display()));
    }
    std::fs::create_dir_all(directory)
        .map_err(|_| format!("{} is unavailable", directory.display()))?;
    Ok(path)
}

fn read(path: &Path) -> Result<String, String> {
    match std::fs::metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(format!("{{\n  \"$schema\": \"{SCHEMA_URL}\"\n}}\n"));
        }
        Err(_) => return Err(format!("{} is unavailable", path.display())),
        Ok(meta) if !meta.is_file() || meta.len() > MAX_DOCUMENT => {
            return Err(format!("{} is not an editable file", path.display()));
        }
        Ok(_) => {}
    }
    std::fs::read_to_string(path).map_err(|_| format!("{} is not readable UTF-8", path.display()))
}

fn parse(text: &str) -> Result<Map<String, Value>, String> {
    match jsonc::parse("global config", text) {
        Ok(Value::Object(map)) => Ok(map),
        Ok(_) => Err("global config must be a JSON object".into()),
        Err(error) => Err(error.to_string()),
    }
}

/// Every key but `key` is unchanged and `key` holds exactly `value`.
fn verify(
    before: &Map<String, Value>,
    after: &Map<String, Value>,
    key: &str,
    value: &Value,
) -> Result<(), String> {
    let others = |map: &Map<String, Value>| {
        map.iter()
            .filter(|(name, _)| name.as_str() != key)
            .map(|(name, value)| (name.clone(), value.clone()))
            .collect::<Map<_, _>>()
    };
    if after.get(key) != Some(value) || others(before) != others(after) {
        return Err(format!("global config `{key}` cannot be edited in place"));
    }
    Ok(())
}

/// Replace the file through a sibling temporary file, keeping the original permissions.
fn write(target: &Path, text: &str) -> Result<(), String> {
    let unavailable = || format!("{} could not be written", target.display());
    let parent = target.parent().ok_or_else(unavailable)?;
    let name = target
        .file_name()
        .ok_or_else(unavailable)?
        .to_string_lossy();
    let temporary = parent.join(format!(".{name}.{}.tmp", std::process::id()));
    let result = (|| {
        use std::io::Write;
        // A handle opened for writing: Windows refuses to flush a read-only one.
        let mut file = std::fs::File::create(&temporary)?;
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
        drop(file);
        if let Ok(meta) = std::fs::metadata(target) {
            std::fs::set_permissions(&temporary, meta.permissions())?;
        }
        std::fs::rename(&temporary, target)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result.map_err(|_| unavailable())
}

/// `text` with top-level `key` set to `value`, every other byte kept.
pub(crate) fn replace_top_level(text: &str, key: &str, value: &Value) -> Result<String, String> {
    // Comments and trailing commas are blanked byte for byte, so offsets match `text`.
    let blank = jsonc::to_json(text);
    let bytes = blank.as_bytes();
    let open = skip_whitespace(bytes, 0);
    if bytes.get(open) != Some(&b'{') {
        return Err("global config must be a JSON object".into());
    }
    let rendered = serde_json::to_string(value).map_err(|error| error.to_string())?;
    if let Some((start, end)) = find_value(bytes, open + 1, key)? {
        return Ok(format!("{}{rendered}{}", &text[..start], &text[end..]));
    }
    let next = skip_whitespace(bytes, open + 1);
    let separator = if bytes.get(next) == Some(&b'}') {
        "\n"
    } else {
        ","
    };
    let entry = format!("\n  {}: {rendered}{separator}", Value::from(key));
    Ok(format!("{}{entry}{}", &text[..=open], &text[open + 1..]))
}

/// The byte range of top-level member `key`'s value, scanning from just inside `{`.
fn find_value(bytes: &[u8], at: usize, key: &str) -> Result<Option<(usize, usize)>, String> {
    Ok(find_member(bytes, at, key)?.map(|(_, start, end)| (start, end)))
}

/// Where member `key` starts (its name), and its value's byte range, scanning an object
/// from just inside its `{`.
fn find_member(
    bytes: &[u8],
    mut at: usize,
    key: &str,
) -> Result<Option<(usize, usize, usize)>, String> {
    let malformed = || String::from("global config is not well-formed JSON");
    loop {
        at = skip_whitespace(bytes, at);
        match bytes.get(at) {
            Some(b'}') => return Ok(None),
            Some(b'"') => {}
            _ => return Err(malformed()),
        }
        let name_end = string_end(bytes, at).ok_or_else(malformed)?;
        let colon = skip_whitespace(bytes, name_end);
        if bytes.get(colon) != Some(&b':') {
            return Err(malformed());
        }
        let start = skip_whitespace(bytes, colon + 1);
        let end = value_end(bytes, start).ok_or_else(malformed)?;
        let name: String = serde_json::from_slice(&bytes[at..name_end]).map_err(|_| malformed())?;
        if name == key {
            return Ok(Some((at, start, end)));
        }
        at = skip_whitespace(bytes, end);
        if bytes.get(at) == Some(&b',') {
            at += 1;
        }
    }
}

fn skip_whitespace(bytes: &[u8], mut at: usize) -> usize {
    while bytes.get(at).is_some_and(u8::is_ascii_whitespace) {
        at += 1;
    }
    at
}

/// One past the closing quote of the string starting at `at`.
fn string_end(bytes: &[u8], at: usize) -> Option<usize> {
    let mut i = at + 1;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => i += 2,
            b'"' => return Some(i + 1),
            _ => i += 1,
        }
    }
    None
}

/// One past the end of the JSON value starting at `at`.
fn value_end(bytes: &[u8], at: usize) -> Option<usize> {
    match bytes.get(at)? {
        b'"' => string_end(bytes, at),
        b'{' | b'[' => container_end(bytes, at),
        _ => {
            let length = bytes[at..]
                .iter()
                .position(|b| matches!(b, b',' | b'}' | b']') || b.is_ascii_whitespace())
                .unwrap_or(bytes.len() - at);
            (length > 0).then_some(at + length)
        }
    }
}

/// One past the bracket closing the object or array opened at `at`.
fn container_end(bytes: &[u8], at: usize) -> Option<usize> {
    let mut depth = 0usize;
    let mut i = at;
    while i < bytes.len() {
        match bytes[i] {
            b'"' => {
                i = string_end(bytes, i)?;
                continue;
            }
            b'{' | b'[' => depth += 1,
            b'}' | b']' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i + 1);
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn replacing_a_member_keeps_comments_and_other_members() {
        let text = "{\n  // keep me\n  \"model\": \"a/b\", /* and me */\n  \"memory\": { \"enabled\": false, // inner\n  },\n  \"x\": [1, {\"memory\": 2}],\n}\n";
        let edited =
            replace_top_level(text, "memory", &json!({"enabled":true,"generate":false})).unwrap();
        assert!(edited.contains("// keep me"));
        assert!(edited.contains("/* and me */"));
        let parsed = jsonc::parse("t", &edited).unwrap();
        assert_eq!(parsed["memory"], json!({"enabled":true,"generate":false}));
        assert_eq!(parsed["x"], json!([1, {"memory": 2}]));
        assert_eq!(parsed["model"], "a/b");
    }

    #[test]
    fn inserting_into_empty_and_nonempty_objects() {
        for text in ["{}", "{ }\n", "{\"a\": 1}", "// c\n{\n  \"a\": \"}\"\n}"] {
            let edited = replace_top_level(text, "memory", &json!({"enabled":false})).unwrap();
            let parsed = jsonc::parse("t", &edited).unwrap();
            assert_eq!(parsed["memory"], json!({"enabled":false}), "{edited}");
        }
    }

    #[test]
    fn nested_keys_with_the_same_name_are_not_top_level() {
        let text = "{\"a\": {\"memory\": 1}, \"b\": \"\\\"memory\\\"\"}";
        let edited = replace_top_level(text, "memory", &json!(true)).unwrap();
        let parsed = jsonc::parse("t", &edited).unwrap();
        assert_eq!(parsed["a"]["memory"], 1);
        assert_eq!(parsed["memory"], true);
    }

    #[test]
    fn adding_and_removing_a_nested_member_keeps_comments() {
        let text = "{\n  // top\n  \"mcp\": {\n    // public servers\n    \"a\": { \"type\": \"remote\", \"url\": \"https://a/mcp\" } // keep\n  }\n}\n";
        let added = edit_member(
            text,
            "mcp",
            "b",
            Some(&json!({"type":"remote","url":"https://b/mcp"})),
        )
        .unwrap();
        assert!(
            added.contains("// top")
                && added.contains("// public servers")
                && added.contains("// keep")
        );
        let parsed = jsonc::parse("t", &added).unwrap();
        assert_eq!(parsed["mcp"]["b"]["url"], "https://b/mcp");
        assert_eq!(parsed["mcp"]["a"]["url"], "https://a/mcp");
        let replaced = edit_member(
            &added,
            "mcp",
            "a",
            Some(&json!({"type":"local","command":"x"})),
        )
        .unwrap();
        assert_eq!(
            jsonc::parse("t", &replaced).unwrap()["mcp"]["a"]["command"],
            "x"
        );
        for name in ["a", "b"] {
            let removed = edit_member(&added, "mcp", name, None).unwrap();
            let parsed = jsonc::parse("t", &removed).unwrap();
            assert!(parsed["mcp"].get(name).is_none(), "{removed}");
            assert_eq!(parsed["mcp"].as_object().unwrap().len(), 1, "{removed}");
            assert!(removed.contains("// public servers"), "{removed}");
        }
    }

    #[test]
    fn a_missing_object_is_created_and_a_non_object_refused() {
        let created = edit_member(
            "{\"model\": \"a/b\"}",
            "mcp",
            "c",
            Some(&json!({"type":"remote","url":"https://c"})),
        )
        .unwrap();
        assert_eq!(
            jsonc::parse("t", &created).unwrap()["mcp"]["c"]["url"],
            "https://c"
        );
        assert!(edit_member("{\"mcp\": []}", "mcp", "c", Some(&json!(1))).is_err());
        assert_eq!(edit_member("{}", "mcp", "c", None).unwrap(), "{}");
    }

    #[test]
    fn set_document_member_writes_the_file_and_reports_existence() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".cyber/cyber.jsonc");
        let server = json!({"type":"remote","url":"https://x/mcp"});
        assert!(!set_document_member(&path, "mcp", "x", Some(&server)).unwrap());
        assert!(set_document_member(&path, "mcp", "x", None).unwrap());
        let parsed = jsonc::parse("t", &std::fs::read_to_string(&path).unwrap()).unwrap();
        assert!(parsed["mcp"].get("x").is_none());
        assert!(parsed["$schema"].is_string());
    }

    #[test]
    fn non_objects_are_refused() {
        assert!(replace_top_level("[]", "memory", &json!(true)).is_err());
        assert!(replace_top_level("", "memory", &json!(true)).is_err());
    }

    #[test]
    fn file_edits_create_or_keep_the_document_and_refuse_invalid_text() {
        let dir = tempfile::tempdir().unwrap();
        let written = set_global_key(dir.path(), "memory", |_| json!({"enabled":false})).unwrap();
        assert_eq!(written, dir.path().join("cyber.jsonc"));
        let created = jsonc::parse("t", &std::fs::read_to_string(&written).unwrap()).unwrap();
        assert_eq!(created["$schema"], SCHEMA_URL);
        assert_eq!(created["memory"], json!({"enabled":false}));

        std::fs::remove_file(&written).unwrap();
        let json = dir.path().join("cyber.json");
        std::fs::write(&json, "{\"memory\":{\"enabled\":false,\"extra\":1}}").unwrap();
        let written = set_global_key(dir.path(), "memory", |current| {
            let mut value = current.cloned().unwrap();
            value["enabled"] = json!(true);
            value
        })
        .unwrap();
        assert_eq!(written, json.canonicalize().unwrap());
        assert_eq!(
            std::fs::read_to_string(&json).unwrap(),
            "{\"memory\":{\"enabled\":true,\"extra\":1}}"
        );

        std::fs::write(&json, "{\"memory\": ").unwrap();
        assert!(set_global_key(dir.path(), "memory", |_| json!({})).is_err());
        assert_eq!(std::fs::read_to_string(&json).unwrap(), "{\"memory\": ");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }
}
