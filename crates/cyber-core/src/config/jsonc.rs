//! JSONC: JSON with comments and trailing commas. Comments and trailing commas are blanked
//! byte for byte, so parser line and column numbers still point into the original text.

use serde_json::Value;

use super::ConfigError;

pub fn parse(path: &str, source: &str) -> Result<Value, ConfigError> {
    serde_json::from_str(&to_json(source)).map_err(|e| ConfigError::Parse {
        path: path.to_string(),
        line: e.line(),
        column: e.column(),
        message: strip_position(&e.to_string()),
    })
}

pub fn to_json(source: &str) -> String {
    let cleaned = blank_trailing_commas(blank_comments(source.as_bytes()));
    // Only ASCII bytes are replaced, and only with spaces, so the result stays UTF-8.
    String::from_utf8(cleaned).unwrap_or_default()
}

fn strip_position(message: &str) -> String {
    message
        .split(" at line ")
        .next()
        .unwrap_or(message)
        .to_string()
}

fn blank_comments(source: &[u8]) -> Vec<u8> {
    let mut out = source.to_vec();
    let mut i = 0;
    let mut in_string = false;
    while i < out.len() {
        let byte = out[i];
        if in_string {
            i += string_step(byte, &mut in_string);
            continue;
        }
        i = match (byte, out.get(i + 1)) {
            (b'"', _) => {
                in_string = true;
                i + 1
            }
            (b'/', Some(b'/')) => blank_line_comment(&mut out, i),
            (b'/', Some(b'*')) => blank_block_comment(&mut out, i),
            _ => i + 1,
        };
    }
    out
}

/// Advance inside a string literal: skip escaped bytes, leave the string on `"`.
fn string_step(byte: u8, in_string: &mut bool) -> usize {
    match byte {
        b'\\' => 2,
        b'"' => {
            *in_string = false;
            1
        }
        _ => 1,
    }
}

fn blank_line_comment(out: &mut [u8], start: usize) -> usize {
    let mut j = start;
    while j < out.len() && out[j] != b'\n' {
        out[j] = b' ';
        j += 1;
    }
    j
}

fn blank_block_comment(out: &mut [u8], start: usize) -> usize {
    let mut j = start;
    while j < out.len() {
        if out[j] == b'*' && out.get(j + 1) == Some(&b'/') && j > start {
            out[j] = b' ';
            out[j + 1] = b' ';
            return j + 2;
        }
        if out[j] != b'\n' {
            out[j] = b' ';
        }
        j += 1;
    }
    j
}

fn blank_trailing_commas(mut out: Vec<u8>) -> Vec<u8> {
    let mut i = 0;
    let mut in_string = false;
    while i < out.len() {
        let byte = out[i];
        if in_string {
            i += string_step(byte, &mut in_string);
            continue;
        }
        if byte == b'"' {
            in_string = true;
        } else if byte == b',' && closes_next(&out, i + 1) {
            out[i] = b' ';
        }
        i += 1;
    }
    out
}

fn closes_next(out: &[u8], from: usize) -> bool {
    out[from..]
        .iter()
        .find(|b| !b.is_ascii_whitespace())
        .is_some_and(|b| *b == b'}' || *b == b']')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn comments_and_trailing_commas() {
        let src = "{\n  // line\n  \"a\": \"x//y\", /* block */\n  \"b\": [1, 2,],\n}";
        let v = parse("t", src).unwrap();
        assert_eq!(v["a"], "x//y");
        assert_eq!(v["b"], serde_json::json!([1, 2]));
    }

    #[test]
    fn escaped_quote_does_not_end_string() {
        let v = parse("t", r#"{"a": "q\" // not a comment"}"#).unwrap();
        assert_eq!(v["a"], "q\" // not a comment");
    }

    #[test]
    fn error_position_points_into_original() {
        let src = "{\n  // c\n  \"a\": 1\n  \"b\": 2\n}";
        match parse("/repo/cyber.jsonc", src) {
            Err(ConfigError::Parse { line, path, .. }) => {
                assert_eq!(line, 4);
                assert_eq!(path, "/repo/cyber.jsonc");
            }
            other => panic!("expected parse error, got {other:?}"),
        }
    }
}
