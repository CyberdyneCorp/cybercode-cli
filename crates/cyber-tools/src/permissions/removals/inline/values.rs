//! Bounded literal expressions and API names for inline removal analysis.

use super::{Frame, RemovalScope};
use std::path::{Path, PathBuf};
use tree_sitter::Node;

fn text<'a>(node: Node<'_>, src: &'a [u8]) -> Option<&'a str> {
    node.utf8_text(src).ok()
}

pub(super) fn arguments(node: Node<'_>) -> Vec<Node<'_>> {
    let Some(args) = node.child_by_field_name("arguments") else {
        return Vec::new();
    };
    let mut cursor = args.walk();
    args.named_children(&mut cursor)
        .filter(|n| n.kind() != "comment")
        .collect()
}

pub(super) fn target<'a>(args: &[Node<'a>]) -> Option<Node<'a>> {
    args.iter().find_map(|n| {
        if n.kind() == "keyword_argument" {
            n.child_by_field_name("value")
        } else {
            Some(*n)
        }
    })
}

pub(super) fn removal_target<'a>(args: &[Node<'a>], src: &[u8]) -> Option<Node<'a>> {
    args.iter()
        .find(|n| n.kind() != "keyword_argument")
        .copied()
        .or_else(|| {
            args.iter().find_map(|n| {
                let name = n.child_by_field_name("name")?.utf8_text(src).ok()?;
                matches!(name, "path" | "filename")
                    .then(|| n.child_by_field_name("value"))
                    .flatten()
            })
        })
}

pub(super) fn name(node: Node<'_>, src: &[u8], frame: &Frame) -> Option<String> {
    name_at(node, src, frame, 0)
}

fn name_at(node: Node<'_>, src: &[u8], frame: &Frame, depth: usize) -> Option<String> {
    if depth > 16 || node.end_byte() - node.start_byte() > 64 * 1024 {
        return None;
    }
    match node.kind() {
        "identifier" | "property_identifier" | "dotted_name" => {
            let raw = text(node, src)?;
            if let Some(alias) = frame.aliases.get(raw) {
                return Some(alias.clone());
            }
            (!frame.values.contains_key(raw)).then(|| raw.into())
        }
        "attribute" | "member_expression" => {
            let object = node.child_by_field_name("object")?;
            let field = node
                .child_by_field_name("attribute")
                .or_else(|| node.child_by_field_name("property"))?;
            Some(format!(
                "{}.{}",
                name_at(object, src, frame, depth + 1).unwrap_or_default(),
                text(field, src)?
            ))
        }
        "call_expression" => {
            let function = node.child_by_field_name("function")?;
            if text(function, src)? != "require" {
                return None;
            }
            let args = arguments(node);
            module(&string(*args.first()?, src)?).map(str::to_string)
        }
        _ => None,
    }
}

pub(super) fn module(value: &str) -> Option<&str> {
    match value {
        "fs" | "node:fs" => Some("fs"),
        "fs/promises" | "node:fs/promises" => Some("fs.promises"),
        "path" | "node:path" => Some("path"),
        "child_process" | "node:child_process" => Some("child_process"),
        _ => None,
    }
}

pub(super) fn value(
    node: Node<'_>,
    src: &[u8],
    scope: &RemovalScope<'_>,
    frame: &Frame,
    depth: usize,
) -> Option<String> {
    if depth > 16 || node.end_byte() - node.start_byte() > 64 * 1024 {
        return None;
    }
    match node.kind() {
        "string" | "template_string" => string(node, src),
        "identifier" => frame.values.get(text(node, src)?).cloned().flatten(),
        "parenthesized_expression" => value(node.named_child(0)?, src, scope, frame, depth + 1),
        "binary_operator" | "binary_expression" => binary(node, src, scope, frame, depth + 1),
        "call" | "call_expression" => call_value(node, src, scope, frame, depth + 1),
        _ => None,
    }
    .filter(|v| v.len() <= 64 * 1024)
}

fn binary(
    node: Node<'_>,
    src: &[u8],
    scope: &RemovalScope<'_>,
    frame: &Frame,
    depth: usize,
) -> Option<String> {
    let left = value(node.child_by_field_name("left")?, src, scope, frame, depth)?;
    let right = value(node.child_by_field_name("right")?, src, scope, frame, depth)?;
    match text(node.child_by_field_name("operator")?, src)? {
        "+" => Some(format!("{left}{right}")),
        "/" => Some(Path::new(&left).join(right).to_string_lossy().into()),
        _ => None,
    }
}

fn call_value(
    node: Node<'_>,
    src: &[u8],
    scope: &RemovalScope<'_>,
    frame: &Frame,
    depth: usize,
) -> Option<String> {
    let function = node.child_by_field_name("function")?;
    let name = name(function, src, frame)?;
    let args = arguments(node);
    match name.as_str() {
        "pathlib.Path" | "pathlib.PosixPath" | "Path" => {
            if args.is_empty() {
                Some(".".into())
            } else {
                join(&args, src, scope, frame, depth)
            }
        }
        "str" => value(target(&args)?, src, scope, frame, depth),
        "os.getcwd" | "pathlib.Path.cwd" | "process.cwd" if !frame.shell.directory_changed => {
            Some(scope.workdir.to_string_lossy().into())
        }
        "pathlib.Path.home" => Some(scope.home.to_string_lossy().into()),
        "os.path.join" => join(&args, src, scope, frame, depth),
        "path.join" => js_join(&args, src, scope, frame, depth),
        "path.resolve" | "os.path.abspath" if !frame.shell.directory_changed => {
            let joined = if args.is_empty() && name == "path.resolve" {
                ".".into()
            } else {
                join(&args, src, scope, frame, depth)?
            };
            Some(
                crate::bash_analysis::normalize(&scope.workdir.join(joined))
                    .to_string_lossy()
                    .into(),
            )
        }
        "os.path.expanduser" => {
            expand_home(value(target(&args)?, src, scope, frame, depth)?, scope)
        }
        _ => method_value(function, &name, src, scope, frame, depth),
    }
}

fn join(
    args: &[Node<'_>],
    src: &[u8],
    scope: &RemovalScope<'_>,
    frame: &Frame,
    depth: usize,
) -> Option<String> {
    let mut path = PathBuf::new();
    for arg in args {
        path.push(value(*arg, src, scope, frame, depth)?);
    }
    (!args.is_empty()).then(|| path.to_string_lossy().into())
}

fn js_join(
    args: &[Node<'_>],
    src: &[u8],
    scope: &RemovalScope<'_>,
    frame: &Frame,
    depth: usize,
) -> Option<String> {
    let parts: Vec<_> = args
        .iter()
        .map(|n| value(*n, src, scope, frame, depth))
        .collect::<Option<_>>()?;
    Some(
        crate::bash_analysis::normalize(Path::new(&parts.join("/")))
            .to_string_lossy()
            .into(),
    )
}

fn method_value(
    function: Node<'_>,
    name: &str,
    src: &[u8],
    scope: &RemovalScope<'_>,
    frame: &Frame,
    depth: usize,
) -> Option<String> {
    let object = function.child_by_field_name("object")?;
    let path = value(object, src, scope, frame, depth)?;
    match name.rsplit('.').next()? {
        "expanduser" => expand_home(path, scope),
        "resolve" | "absolute" if !frame.shell.directory_changed => Some(
            crate::host::canonical(&scope.workdir.join(path))
                .to_string_lossy()
                .into(),
        ),
        _ => None,
    }
}

fn expand_home(path: String, scope: &RemovalScope<'_>) -> Option<String> {
    if path == "~" {
        return Some(scope.home.to_string_lossy().into());
    }
    if let Some(tail) = path.strip_prefix("~/") {
        return Some(scope.home.join(tail).to_string_lossy().into());
    }
    (!path.starts_with('~')).then_some(path)
}

pub(super) fn known_call(
    node: Node<'_>,
    name: &str,
    src: &[u8],
    scope: &RemovalScope<'_>,
    frame: &Frame,
) -> bool {
    matches!(
        name,
        "print" | "len" | "range" | "console.log" | "console.info" | "console.error"
    ) || value(node, src, scope, frame, 0).is_some()
        || (name == "require"
            && arguments(node)
                .first()
                .and_then(|n| string(*n, src))
                .is_some_and(|s| module(&s).is_some()))
}

pub(super) fn string(node: Node<'_>, src: &[u8]) -> Option<String> {
    let raw = text(node, src)?;
    let start = raw.find(['\'', '"', '`'])?;
    let prefix = &raw[..start];
    if !matches!(prefix.to_ascii_lowercase().as_str(), "" | "r" | "u") {
        return None;
    }
    let quote = raw.as_bytes()[start] as char;
    let delimiter = if raw[start..].starts_with(&quote.to_string().repeat(3)) {
        quote.to_string().repeat(3)
    } else {
        quote.to_string()
    };
    let body = raw[start..]
        .strip_prefix(&delimiter)?
        .strip_suffix(&delimiter)?;
    if quote == '`' && body.contains("${") {
        return None;
    }
    if prefix.eq_ignore_ascii_case("r") {
        return Some(body.into());
    }
    decode(body)
}

fn decode(body: &str) -> Option<String> {
    let mut chars = body.chars();
    let mut out = String::new();
    while let Some(c) = chars.next() {
        if c == '\\' {
            out.push(escape(&mut chars)?);
        } else {
            out.push(c);
        }
    }
    Some(out)
}

fn escape(chars: &mut std::str::Chars<'_>) -> Option<char> {
    match chars.next()? {
        '\\' => Some('\\'),
        '\'' => Some('\''),
        '"' => Some('"'),
        '`' => Some('`'),
        'n' => Some('\n'),
        'r' => Some('\r'),
        't' => Some('\t'),
        'x' => hex(chars, 2),
        'u' => hex(chars, 4),
        'U' => hex(chars, 8),
        _ => None,
    }
}

fn hex(chars: &mut std::str::Chars<'_>, length: usize) -> Option<char> {
    let mut code = 0;
    for _ in 0..length {
        code = code * 16 + chars.next()?.to_digit(16)?;
    }
    char::from_u32(code)
}
