//! `--output-schema <file>` (`exec-mode` → Structured final output): the schema is read
//! and checked before any Session exists, so a bad file exits 2 without work.

use std::path::Path;

use serde_json::Value;

use crate::error::CliError;

/// 64 KiB, the server's bound on a prompt's schema.
const MAX_BYTES: u64 = 64 * 1024;

pub fn load(path: Option<&Path>) -> Result<Option<Value>, CliError> {
    let Some(path) = path else {
        return Ok(None);
    };
    let shown = path.display();
    let size = std::fs::metadata(path)
        .map_err(|e| CliError::usage(format!("--output-schema {shown}: {e}")))?
        .len();
    if size > MAX_BYTES {
        return Err(CliError::usage(format!(
            "--output-schema {shown}: larger than 64 KiB"
        )));
    }
    let text = std::fs::read_to_string(path)
        .map_err(|e| CliError::usage(format!("--output-schema {shown}: {e}")))?;
    let schema: Value = serde_json::from_str(&text)
        .map_err(|e| CliError::usage(format!("--output-schema {shown}: not JSON: {e}")))?;
    cyber_server::runtime::check_root_schema(&schema)
        .map_err(|e| CliError::usage(format!("--output-schema {shown}: {e}")))?;
    Ok(Some(schema))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bad_schema_is_a_usage_error_before_any_work() {
        let dir = tempfile::tempdir().unwrap();
        let write = |name: &str, text: &str| {
            let path = dir.path().join(name);
            std::fs::write(&path, text).unwrap();
            path
        };
        let good = write("good.json", r#"{"type":"object","required":["a"]}"#);
        assert!(load(Some(&good)).unwrap().is_some());
        assert_eq!(load(None).unwrap(), None);
        for (name, text, needle) in [
            ("text.json", "not json", "not JSON"),
            ("string.json", r#"{"type":"string"}"#, "object schema"),
            (
                "remote.json",
                r#"{"type":"object","$ref":"http://x/y"}"#,
                "local references",
            ),
        ] {
            let error = load(Some(&write(name, text))).unwrap_err();
            assert_eq!(error.code, 2, "{name}");
            assert!(error.message.contains(needle), "{name}: {}", error.message);
        }
        let missing = load(Some(&dir.path().join("none.json"))).unwrap_err();
        assert_eq!(missing.code, 2);
    }
}
