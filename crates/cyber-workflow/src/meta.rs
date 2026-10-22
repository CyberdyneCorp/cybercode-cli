//! Static extraction of `export const meta = { ... }` (`workflows` → Workflow module format).
//!
//! `meta` is read without running the script, so it must be a pure literal: objects,
//! arrays, strings, template literals without substitutions, numbers, booleans and `null`.
//! Variables, calls, spreads, computed keys and interpolation fail with
//! `ParseError: meta must be a pure literal` at the offending token.

use serde::Serialize;
use serde_json::{Map, Number, Value};

use crate::diagnostic::{Diagnostic, Position};
use crate::scan::{Tok, Token};

pub const NOT_LITERAL: &str = "meta must be a pure literal";
/// Objects and arrays nested deeper than this fail with [`TOO_DEEP`], so a hostile literal
/// cannot overflow the stack of the recursive reader.
pub const MAX_DEPTH: usize = 64;
pub const TOO_DEEP: &str = "meta nests objects and arrays more than 64 levels deep";
const MISSING: &str = "a Workflow module must declare `export const meta = { name, description }`";

/// One declared phase.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Phase {
    pub title: String,
}

/// The validated `meta` export.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Meta {
    pub name: String,
    pub description: String,
    pub phases: Vec<Phase>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub args: Option<Value>,
    /// `meta.tools`: the direct read-only tools the script may call (`workflows` → Direct
    /// read-only tools).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<String>,
    /// The literal as written, including keys later milestones read (budgets, tools).
    #[serde(skip)]
    pub raw: Value,
}

/// The tools a script may call directly; mutating tools are never exposed.
pub const DIRECT_TOOLS: [&str; 3] = ["read", "glob", "grep"];

/// A tool allowlist (`meta.tools` or an invocation's `tools`): names from
/// [`DIRECT_TOOLS`], without duplicates.
pub fn tool_list(value: &Value) -> Option<Vec<String>> {
    let mut tools: Vec<String> = Vec::new();
    for item in value.as_array()? {
        let name = item.as_str().filter(|name| DIRECT_TOOLS.contains(name))?;
        if !tools.iter().any(|known| known == name) {
            tools.push(name.to_owned());
        }
    }
    Some(tools)
}

/// Whether `name` is a valid Workflow name: `^[a-z0-9][a-z0-9._-]{0,63}$`.
pub fn valid_name(name: &str) -> bool {
    let mut bytes = name.bytes();
    bytes
        .next()
        .is_some_and(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        && name.len() <= 64
        && bytes.all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"._-".contains(&b))
}

/// Find the top-level `export const meta =` declaration and read its literal.
pub fn extract(tokens: &[Token]) -> Result<Meta, Diagnostic> {
    let start = declaration(tokens).ok_or_else(|| {
        Diagnostic::parse(
            MISSING,
            tokens
                .first()
                .map_or(Position { line: 1, column: 1 }, |t| t.pos),
        )
    })?;
    let mut reader = Reader {
        tokens,
        i: start,
        depth: 0,
    };
    let at = reader.position();
    let value = reader.value()?;
    reader.end()?;
    validate(value, at)
}

/// Index of the first token after `export const meta =` at the top level.
fn declaration(tokens: &[Token]) -> Option<usize> {
    let mut depth = 0i32;
    for (i, token) in tokens.iter().enumerate() {
        match token.tok {
            Tok::Punct('{' | '(' | '[') => depth += 1,
            Tok::Punct('}' | ')' | ']') => depth -= 1,
            _ => {}
        }
        if depth == 0 && is_meta_declaration(&tokens[i..]) {
            return Some(i + 4);
        }
    }
    None
}

fn is_meta_declaration(tokens: &[Token]) -> bool {
    matches!(
        tokens,
        [
            Token { tok: Tok::Ident(export), .. },
            Token { tok: Tok::Ident(binding), .. },
            Token { tok: Tok::Ident(meta), .. },
            Token { tok: Tok::Punct('='), .. },
            ..
        ] if export == "export" && matches!(binding.as_str(), "const" | "let" | "var") && meta == "meta"
    )
}

struct Reader<'a> {
    tokens: &'a [Token],
    i: usize,
    /// Objects and arrays open at the current token.
    depth: usize,
}

impl Reader<'_> {
    fn position(&self) -> Position {
        self.tokens
            .get(self.i)
            .or(self.tokens.last())
            .map_or(Position { line: 1, column: 1 }, |t| t.pos)
    }

    fn not_literal(&self) -> Diagnostic {
        Diagnostic::parse(NOT_LITERAL, self.position())
    }

    fn next(&mut self) -> Result<&Tok, Diagnostic> {
        let token = self.tokens.get(self.i).ok_or_else(|| self.not_literal())?;
        self.i += 1;
        Ok(&token.tok)
    }

    fn peek_is(&self, c: char) -> bool {
        matches!(self.tokens.get(self.i), Some(Token { tok: Tok::Punct(p), .. }) if *p == c)
    }

    fn value(&mut self) -> Result<Value, Diagnostic> {
        let at = self.i;
        let value = match self.next()?.clone() {
            Tok::Punct('{') => return self.nested(at, Self::object),
            Tok::Punct('[') => return self.nested(at, Self::array),
            Tok::Punct('-') => match self.next()? {
                Tok::Number(text) => number(text).map(|n| -n),
                _ => None,
            },
            Tok::Str(text) | Tok::Template(Some(text)) => return Ok(Value::String(text)),
            Tok::Number(text) => number(&text),
            Tok::Ident(word) => return keyword(&word).ok_or_else(|| self.not_literal_at(at)),
            _ => None,
        };
        value
            .and_then(Number::from_f64)
            .map(integral)
            .ok_or_else(|| self.not_literal_at(at))
    }

    fn not_literal_at(&self, index: usize) -> Diagnostic {
        self.at(index).not_literal()
    }

    fn at(&self, index: usize) -> Reader<'_> {
        Reader {
            tokens: self.tokens,
            i: index,
            depth: self.depth,
        }
    }

    /// Read an object or array opened at `at`, within [`MAX_DEPTH`].
    fn nested(
        &mut self,
        at: usize,
        read: fn(&mut Self) -> Result<Value, Diagnostic>,
    ) -> Result<Value, Diagnostic> {
        if self.depth >= MAX_DEPTH {
            return Err(Diagnostic::parse(TOO_DEEP, self.at(at).position()));
        }
        self.depth += 1;
        let value = read(self);
        self.depth -= 1;
        value
    }

    fn object(&mut self) -> Result<Value, Diagnostic> {
        let mut map = Map::new();
        while !self.peek_is('}') {
            let at = self.i;
            let key = match self.next()? {
                Tok::Ident(key) | Tok::Str(key) | Tok::Number(key) => key.clone(),
                _ => return Err(self.not_literal_at(at)),
            };
            if !self.peek_is(':') {
                return Err(self.not_literal());
            }
            self.i += 1;
            let value = self.value()?;
            map.insert(key, value);
            if !self.separator('}')? {
                break;
            }
        }
        self.i += 1;
        Ok(Value::Object(map))
    }

    fn array(&mut self) -> Result<Value, Diagnostic> {
        let mut items = Vec::new();
        while !self.peek_is(']') {
            items.push(self.value()?);
            if !self.separator(']')? {
                break;
            }
        }
        self.i += 1;
        Ok(Value::Array(items))
    }

    /// After an element: true after a comma, false before the closing bracket.
    fn separator(&mut self, close: char) -> Result<bool, Diagnostic> {
        if self.peek_is(',') {
            self.i += 1;
            return Ok(true);
        }
        if self.peek_is(close) {
            return Ok(false);
        }
        Err(self.not_literal())
    }

    /// The literal must end the statement: `;`, end of input, or a new line that does not
    /// continue the expression.
    fn end(&self) -> Result<(), Diagnostic> {
        let Some(next) = self.tokens.get(self.i) else {
            return Ok(());
        };
        let previous_line = self.tokens[self.i - 1].end_line;
        let ends = match &next.tok {
            Tok::Punct(';') => true,
            Tok::Ident(_) | Tok::Str(_) | Tok::Number(_) => next.pos.line > previous_line,
            _ => false,
        };
        if ends {
            Ok(())
        } else {
            Err(self.not_literal())
        }
    }
}

fn keyword(word: &str) -> Option<Value> {
    match word {
        "true" => Some(Value::Bool(true)),
        "false" => Some(Value::Bool(false)),
        "null" => Some(Value::Null),
        _ => None,
    }
}

fn number(text: &str) -> Option<f64> {
    let text = text.replace('_', "");
    let radix = |prefix: [&str; 2], radix| {
        prefix
            .iter()
            .find_map(|p| text.strip_prefix(p))
            .map(|digits| u64::from_str_radix(digits, radix).ok().map(|n| n as f64))
    };
    radix(["0x", "0X"], 16)
        .or_else(|| radix(["0o", "0O"], 8))
        .or_else(|| radix(["0b", "0B"], 2))
        .unwrap_or_else(|| text.parse::<f64>().ok().filter(|n| n.is_finite()))
}

/// Whole numbers become JSON integers, as `JSON.stringify` would print them.
fn integral(n: Number) -> Value {
    match n.as_f64() {
        Some(f) if f.fract() == 0.0 && f.abs() < 9.0e15 => Value::from(f as i64),
        _ => Value::Number(n),
    }
}

fn validate(raw: Value, at: Position) -> Result<Meta, Diagnostic> {
    let invalid = |message: &str| Diagnostic::parse(message, at);
    let map = raw
        .as_object()
        .ok_or_else(|| invalid("meta must be an object literal"))?;
    let name = map
        .get("name")
        .and_then(Value::as_str)
        .filter(|name| valid_name(name))
        .ok_or_else(|| invalid("meta.name must match ^[a-z0-9][a-z0-9._-]{0,63}$"))?;
    let description = map
        .get("description")
        .and_then(Value::as_str)
        .filter(|d| !d.trim().is_empty())
        .ok_or_else(|| invalid("meta.description must be a non-empty string"))?;
    let phases = match map.get("phases") {
        None => Vec::new(),
        Some(phases) => phases
            .as_array()
            .and_then(|items| items.iter().map(phase).collect::<Option<Vec<_>>>())
            .ok_or_else(|| invalid("meta.phases must be an array of { title } objects"))?,
    };
    let args = match map.get("args") {
        None => None,
        Some(Value::Object(args)) => Some(Value::Object(args.clone())),
        Some(_) => return Err(invalid("meta.args must be an object")),
    };
    let tools = match map.get("tools") {
        None => Vec::new(),
        Some(tools) => tool_list(tools)
            .ok_or_else(|| invalid("meta.tools must be an array naming read, glob or grep"))?,
    };
    Ok(Meta {
        name: name.into(),
        description: description.into(),
        phases,
        args,
        tools,
        raw,
    })
}

fn phase(value: &Value) -> Option<Phase> {
    let title = value.get("title")?.as_str()?.trim();
    (!title.is_empty()).then(|| Phase {
        title: title.into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scan::tokenize;

    fn meta(source: &str) -> Result<Meta, Diagnostic> {
        extract(&tokenize(source).unwrap())
    }

    #[test]
    fn reads_a_pure_literal() {
        let meta = meta(
            "// header\nexport const meta = {\n  name: 'audit', \"description\": `Audit auth`,\n  phases: [{ title: \"Scan\" }, { title: 'Verify', },],\n  args: { type: 'object' }, max_agents: 0x10, ratio: -1.5e1, ok: true, none: null,\n};\nexport default async function () {}\n",
        )
        .unwrap();
        assert_eq!(meta.name, "audit");
        assert_eq!(meta.description, "Audit auth");
        assert_eq!(
            meta.phases
                .iter()
                .map(|p| p.title.as_str())
                .collect::<Vec<_>>(),
            ["Scan", "Verify"]
        );
        assert_eq!(meta.args, Some(serde_json::json!({"type": "object"})));
        assert_eq!(meta.raw["max_agents"], 16);
        assert_eq!(meta.raw["ratio"], -15);
        assert_eq!(meta.raw["none"], Value::Null);
    }

    #[test]
    fn computed_values_fail_at_the_offending_token() {
        for (source, at) in [
            (
                "const x = 'a';\nexport const meta = { name: `audit-${x}`, description: 'd' };",
                "2:29",
            ),
            ("export const meta = { name: n, description: 'd' };", "1:29"),
            (
                "export const meta = { name: 'a', description: f() };",
                "1:47",
            ),
            ("export const meta = { ...base, name: 'a' };", "1:23"),
            ("export const meta = { [k]: 1 };", "1:23"),
            (
                "export const meta = { name: 'a', description: 'b' }.x;",
                "1:52",
            ),
            (
                "export const meta = { name: 'a', description: 'b' } || {};",
                "1:53",
            ),
            (
                "export const meta = { name: 'a' + 'b', description: 'b' };",
                "1:33",
            ),
        ] {
            let error = meta(source).unwrap_err();
            assert_eq!(error.message, NOT_LITERAL, "{source}");
            assert_eq!(error.position.unwrap().to_string(), at, "{source}");
        }
    }

    #[test]
    fn deep_nesting_fails_without_overflowing_the_stack() {
        let deep = |open: &str, close: &str, levels: usize| {
            format!(
                "export const meta = {{ name: 'a', description: 'd', x: {}1{} }};",
                open.repeat(levels),
                close.repeat(levels)
            )
        };
        // The top-level object is one level, so 63 more fit.
        assert!(meta(&deep("[", "]", MAX_DEPTH - 1)).is_ok());
        assert!(meta(&deep("{a:", "}", MAX_DEPTH - 1)).is_ok());
        for source in [
            deep("[", "]", MAX_DEPTH),
            deep("[", "]", 400_000),
            deep("{a:", "}", 400_000),
        ] {
            let error = meta(&source).unwrap_err();
            assert_eq!(error.message, TOO_DEEP);
            assert_eq!(error.position.unwrap().line, 1);
        }
    }

    #[test]
    fn nested_or_missing_meta_is_not_a_declaration() {
        let error = meta("function f() { export const meta = { name: 'a', description: 'b' } }")
            .unwrap_err();
        assert_eq!(error.message, MISSING);
        assert!(
            meta("export const meta = { name: 'a', description: 'b' }\nexport default 1").is_ok()
        );
    }

    #[test]
    fn required_fields_are_validated() {
        for (source, message) in [
            ("export const meta = [];", "meta must be an object literal"),
            (
                "export const meta = { description: 'd' };",
                "meta.name must match",
            ),
            (
                "export const meta = { name: 'Bad Name', description: 'd' };",
                "meta.name must match",
            ),
            (
                "export const meta = { name: 'a', description: ' ' };",
                "meta.description",
            ),
            (
                "export const meta = { name: 'a', description: 'd', phases: ['x'] };",
                "meta.phases",
            ),
            (
                "export const meta = { name: 'a', description: 'd', args: 1 };",
                "meta.args",
            ),
        ] {
            let error = meta(source).unwrap_err();
            assert!(error.message.starts_with(message), "{source}: {error}");
        }
    }
}
