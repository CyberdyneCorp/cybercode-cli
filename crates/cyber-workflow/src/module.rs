//! A loaded Workflow module: source, digest, static `meta` and its origin.

use std::path::PathBuf;

use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::diagnostic::{Diagnostic, DiagnosticKind, Position};
use crate::meta::{self, Meta};
use crate::scan::{Tok, Token, tokenize};

/// Largest accepted script source.
pub const MAX_SOURCE_BYTES: usize = 1024 * 1024;

/// Where a module came from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Origin {
    /// A saved module under a project or global `workflows/` directory.
    Saved { scope: Scope, path: PathBuf },
    /// A script passed inline (by the model or an API client).
    Inline,
    /// A Workflow embedded in the binary.
    Bundled { name: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    Global,
    Project,
}

/// A parsed Workflow ready to run.
#[derive(Debug, Clone, Serialize)]
pub struct WorkflowModule {
    pub meta: Meta,
    pub origin: Origin,
    /// `sha256:<hex>` of the source, recorded with every run revision.
    pub digest: String,
    #[serde(skip)]
    pub source: String,
}

impl WorkflowModule {
    /// Parse a module statically: size, forbidden imports, then a pure-literal `meta`.
    pub fn parse(source: impl Into<String>, origin: Origin) -> Result<Self, Diagnostic> {
        let source = source.into();
        if source.len() > MAX_SOURCE_BYTES {
            return Err(Diagnostic::parse(
                format!("script exceeds {MAX_SOURCE_BYTES} bytes"),
                Position { line: 1, column: 1 },
            ));
        }
        let tokens = tokenize(&source)?;
        forbidden_imports(&tokens)?;
        let meta = meta::extract(&tokens)?;
        Ok(Self {
            meta,
            origin,
            digest: digest(&source),
            source,
        })
    }

    /// An inline script.
    pub fn inline(source: impl Into<String>) -> Result<Self, Diagnostic> {
        Self::parse(source, Origin::Inline)
    }
}

pub fn digest(source: &str) -> String {
    let hash = Sha256::digest(source.as_bytes());
    let hex: String = hash.iter().map(|b| format!("{b:02x}")).collect();
    format!("sha256:{hex}")
}

/// Reject `import`, `export ... from` and `require(...)`: the host API is the only module.
fn forbidden_imports(tokens: &[Token]) -> Result<(), Diagnostic> {
    for (i, token) in tokens.iter().enumerate() {
        let Tok::Ident(word) = &token.tok else {
            continue;
        };
        if i > 0 && tokens[i - 1].tok == Tok::Punct('.') {
            continue;
        }
        let specifier = match word.as_str() {
            "import" => import_specifier(&tokens[i + 1..]),
            "require" if next_is(&tokens[i + 1..], '(') => Some(string_at(&tokens[i + 2..])),
            "export" => reexport_specifier(&tokens[i + 1..]),
            _ => None,
        };
        if let Some(specifier) = specifier {
            return Err(unsupported(word, specifier, token.pos));
        }
    }
    Ok(())
}

fn unsupported(word: &str, specifier: Option<String>, at: Position) -> Diagnostic {
    let what = match specifier {
        Some(name) => format!("{word} of {name:?}"),
        None => format!("dynamic {word}"),
    };
    Diagnostic::new(
        DiagnosticKind::UnsupportedSyntax,
        format!("{what} is not supported; Workflow scripts can only use the host API"),
    )
    .at(Some(at))
}

fn next_is(tokens: &[Token], c: char) -> bool {
    tokens.first().is_some_and(|t| t.tok == Tok::Punct(c))
}

fn string_at(tokens: &[Token]) -> Option<String> {
    match tokens.first().map(|t| &t.tok) {
        Some(Tok::Str(s) | Tok::Template(Some(s))) => Some(s.clone()),
        _ => None,
    }
}

/// `import(...)`, `import "x"` and `import ... from "x"`; `import.meta` is allowed.
fn import_specifier(rest: &[Token]) -> Option<Option<String>> {
    match rest.first().map(|t| &t.tok) {
        Some(Tok::Punct('.')) => None,
        Some(Tok::Punct('(')) => Some(string_at(&rest[1..])),
        _ => Some(from_specifier(rest)),
    }
}

/// The string after `from` in the rest of a statement.
fn from_specifier(rest: &[Token]) -> Option<String> {
    let statement = rest
        .iter()
        .position(|t| t.tok == Tok::Punct(';'))
        .map_or(rest, |end| &rest[..end]);
    if let Some(name) = string_at(statement) {
        return Some(name);
    }
    statement
        .iter()
        .position(|t| t.tok == Tok::Ident("from".into()))
        .and_then(|i| string_at(&statement[i + 1..]))
}

/// `export * from "x"` and `export { a } from "x"`.
fn reexport_specifier(rest: &[Token]) -> Option<Option<String>> {
    let after = match rest.first().map(|t| &t.tok) {
        Some(Tok::Punct('*')) => &rest[1..],
        Some(Tok::Punct('{')) => {
            let close = rest.iter().position(|t| t.tok == Tok::Punct('}'))?;
            &rest[close + 1..]
        }
        _ => return None,
    };
    let from = after
        .iter()
        .take(4)
        .position(|t| t.tok == Tok::Ident("from".into()))?;
    Some(string_at(&after[from + 1..]))
}

#[cfg(test)]
mod tests {
    use super::*;

    const META: &str = "export const meta = { name: 'a', description: 'd' };\n";

    fn error(body: &str) -> Diagnostic {
        WorkflowModule::inline(format!("{META}{body}")).unwrap_err()
    }

    #[test]
    fn forbidden_imports_name_the_module_and_location() {
        for (body, named, at) in [
            ("const fs = require('fs');", "require of \"fs\"", "2:12"),
            ("await import('node:fs');", "import of \"node:fs\"", "2:7"),
            (
                "import fs from \"node:fs\";",
                "import of \"node:fs\"",
                "2:1",
            ),
            (
                "import { readFile } from 'fs/promises';",
                "import of \"fs/promises\"",
                "2:1",
            ),
            ("import 'polyfill';", "import of \"polyfill\"", "2:1"),
            ("export * from 'os';", "export of \"os\"", "2:1"),
            ("export { a as b } from 'os';", "export of \"os\"", "2:1"),
            ("const m = 'fs'; require(m);", "dynamic require", "2:17"),
            ("await import(name);", "dynamic import", "2:7"),
        ] {
            let error = error(body);
            assert_eq!(error.kind, DiagnosticKind::UnsupportedSyntax, "{body}");
            assert!(error.message.starts_with(named), "{body}: {error}");
            assert_eq!(error.position.unwrap().to_string(), at, "{body}");
        }
    }

    #[test]
    fn harmless_lookalikes_are_allowed() {
        let module = WorkflowModule::inline(format!(
            "{META}const s = \"require('fs')\"; x.require('a'); log(import.meta); export {{ s }};\nexport default async () => 1;"
        ))
        .unwrap();
        assert_eq!(module.meta.name, "a");
        assert!(module.digest.starts_with("sha256:"));
        assert_eq!(module.digest.len(), "sha256:".len() + 64);
    }

    #[test]
    fn oversized_sources_are_refused() {
        let big = format!("{META}// {}", "x".repeat(MAX_SOURCE_BYTES));
        assert_eq!(
            WorkflowModule::inline(big).unwrap_err().kind,
            DiagnosticKind::ParseError
        );
    }
}
