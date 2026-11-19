//! Definition and reference tags of one file (`code-intelligence` → Repository map symbol
//! extraction). Parsing never runs repository code and never starts a language server.

use std::collections::{BTreeMap, HashSet};
use std::ops::ControlFlow;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tree_sitter::{Node, ParseOptions, Parser, Query, QueryCursor, StreamingIterator};

use super::lang::Lang;

/// A parse slower than this lists the file by path only (`parse_timeout`).
pub const PARSE_TIMEOUT: Duration = Duration::from_millis(250);
/// Characters a signature keeps, the ellipsis included.
pub const SIGNATURE_CHARS: usize = 160;
/// Reference names shorter than this never feed ranking, so they are not stored.
pub const MIN_NAME_CHARS: usize = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Class,
    Struct,
    Enum,
    Interface,
    Type,
    Function,
    Method,
    Constant,
    Macro,
    Module,
}

impl Kind {
    fn parse(text: &str) -> Option<Self> {
        Some(match text {
            "class" => Self::Class,
            "struct" => Self::Struct,
            "enum" => Self::Enum,
            "interface" => Self::Interface,
            "type" => Self::Type,
            "function" => Self::Function,
            "method" => Self::Method,
            "constant" => Self::Constant,
            "macro" => Self::Macro,
            "module" => Self::Module,
            _ => return None,
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Class => "class",
            Self::Struct => "struct",
            Self::Enum => "enum",
            Self::Interface => "interface",
            Self::Type => "type",
            Self::Function => "function",
            Self::Method => "method",
            Self::Constant => "constant",
            Self::Macro => "macro",
            Self::Module => "module",
        }
    }

    /// Kinds whose node encloses other definitions.
    fn is_scope(self) -> bool {
        matches!(
            self,
            Self::Class | Self::Struct | Self::Enum | Self::Interface | Self::Module
        )
    }
}

/// One definition: its name, kind, 1-based line, signature and enclosing scope.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Def {
    pub name: String,
    pub kind: Kind,
    pub line: u32,
    pub signature: String,
    /// Index into [`Tags::scopes`] of the innermost enclosing scope.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<u32>,
}

/// A node enclosing definitions (`impl`, `class`, `mod`), printed above selected members.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Scope {
    pub line: u32,
    pub signature: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<u32>,
}

/// The tags of one file.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tags {
    pub defs: Vec<Def>,
    pub scopes: Vec<Scope>,
    /// Referenced names and how often each occurs.
    pub refs: BTreeMap<String, u32>,
}

/// Why a supported file has no tags.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Failure {
    Timeout,
    Unparsable,
}

/// A definition or scope capture before scopes are resolved.
struct Candidate {
    start: usize,
    end: usize,
    pattern: usize,
    kind: Option<Kind>,
    name: String,
    name_start: usize,
    line: u32,
    signature: String,
}

pub fn extract(lang: Lang, source: &[u8]) -> Result<Tags, Failure> {
    extract_within(lang, source, PARSE_TIMEOUT)
}

pub(super) fn extract_within(lang: Lang, source: &[u8], limit: Duration) -> Result<Tags, Failure> {
    let query = lang.query().ok_or(Failure::Unparsable)?;
    let mut parser = Parser::new();
    parser
        .set_language(&lang.grammar())
        .map_err(|_| Failure::Unparsable)?;
    let started = Instant::now();
    let mut timed_out = false;
    let mut progress = |_: &tree_sitter::ParseState| {
        if started.elapsed() > limit {
            timed_out = true;
            return ControlFlow::Break(());
        }
        ControlFlow::Continue(())
    };
    let options = ParseOptions::new().progress_callback(&mut progress);
    let tree = parser.parse_with_options(
        &mut |offset, _| source.get(offset..).unwrap_or_default(),
        None,
        Some(options),
    );
    let Some(tree) = tree else {
        return Err(if timed_out {
            Failure::Timeout
        } else {
            Failure::Unparsable
        });
    };
    let (candidates, references) = captures(query, tree.root_node(), source);
    let names: HashSet<usize> = candidates.iter().map(|c| c.name_start).collect();
    let refs = match references {
        Some(refs) => count_refs(refs, &names),
        None => identifier_refs(tree.root_node(), source, &names),
    };
    Ok(assemble(candidates, refs))
}

/// A captured reference: the name node's start and its text.
type Reference<'a> = (usize, &'a str);

/// Count captured references, except definition names; one node captured by several
/// patterns counts once.
fn count_refs(refs: Vec<Reference<'_>>, names: &HashSet<usize>) -> BTreeMap<String, u32> {
    let mut seen = HashSet::new();
    let mut counts = BTreeMap::new();
    for (start, text) in refs {
        if !names.contains(&start) && seen.insert(start) {
            count(&mut counts, text);
        }
    }
    counts
}

/// Definitions and scopes, plus the references when the query captures any.
fn captures<'a>(
    query: &Query,
    root: Node<'_>,
    source: &'a [u8],
) -> (Vec<Candidate>, Option<Vec<Reference<'a>>>) {
    let names = query.capture_names();
    let has_refs = names.iter().any(|name| name.starts_with("reference."));
    let mut refs = Vec::new();
    let mut candidates = Vec::new();
    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(query, root, source);
    while let Some(found) = matches.next() {
        let mut name = None;
        let mut target = None;
        for capture in found.captures() {
            let label = names[capture.index as usize];
            if label == "name" {
                name = Some(capture.node);
            } else {
                target = Some((label, capture.node));
            }
        }
        let Some((label, node)) = target else {
            continue;
        };
        if label.starts_with("reference.") {
            if let Some(name) = name
                && let Some(text) = text_of(name, source)
            {
                refs.push((name.start_byte(), text));
            }
            continue;
        }
        if let Some(candidate) = candidate(label, node, name, found.pattern_index, source) {
            candidates.push(candidate);
        }
    }
    (candidates, has_refs.then_some(refs))
}

fn candidate(
    label: &str,
    node: Node<'_>,
    name: Option<Node<'_>>,
    pattern: usize,
    source: &[u8],
) -> Option<Candidate> {
    let kind = match label {
        "container" => None,
        _ => Some(Kind::parse(label.strip_prefix("definition.")?)?),
    };
    let (name_text, name_start) = match (kind, name) {
        (Some(_), Some(name)) => (text_of(name, source)?.to_owned(), name.start_byte()),
        (Some(_), None) => return None,
        (None, _) => (String::new(), usize::MAX),
    };
    Some(Candidate {
        start: node.start_byte(),
        end: node.end_byte(),
        pattern,
        kind,
        name: name_text,
        name_start,
        line: node.start_position().row as u32 + 1,
        signature: signature(node, source),
    })
}

fn text_of<'a>(node: Node<'_>, source: &'a [u8]) -> Option<&'a str> {
    std::str::from_utf8(source.get(node.byte_range())?).ok()
}

fn count(refs: &mut BTreeMap<String, u32>, name: &str) {
    if name.chars().count() >= MIN_NAME_CHARS {
        *refs.entry(name.to_owned()).or_default() += 1;
    }
}

/// The text from the node's start to its `body` field (or the end of its first line),
/// without comments, whitespace runs collapsed and capped at [`SIGNATURE_CHARS`].
pub(super) fn signature(node: Node<'_>, source: &[u8]) -> String {
    let start = node.start_byte();
    let end = match node.child_by_field_name("body") {
        Some(body) => body.start_byte(),
        None => {
            let rest = &source[start..node.end_byte()];
            start + rest.iter().position(|b| *b == b'\n').unwrap_or(rest.len())
        }
    };
    let mut comments = Vec::new();
    collect_comments(node, start, end, &mut comments);
    let mut kept = Vec::with_capacity(end - start);
    let mut at = start;
    for (from, to) in comments {
        if from > at {
            kept.extend_from_slice(&source[at..from]);
        }
        kept.push(b' ');
        at = at.max(to);
    }
    if end > at {
        kept.extend_from_slice(&source[at..end]);
    }
    let text = String::from_utf8_lossy(&kept);
    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    truncate(&collapsed)
}

fn collect_comments(node: Node<'_>, start: usize, end: usize, out: &mut Vec<(usize, usize)>) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.start_byte() >= end || child.end_byte() <= start {
            continue;
        }
        if child.kind().contains("comment") {
            out.push((child.start_byte().max(start), child.end_byte().min(end)));
        } else if child.child_count() > 0 {
            collect_comments(child, start, end, out);
        }
    }
}

fn truncate(text: &str) -> String {
    if text.chars().count() <= SIGNATURE_CHARS {
        return text.to_owned();
    }
    let mut out: String = text.chars().take(SIGNATURE_CHARS - 1).collect();
    out.push('…');
    out
}

/// Every identifier-like leaf, except definition names, counts as a reference. Matching
/// is by name only; references only feed ranking.
fn identifier_refs(root: Node<'_>, source: &[u8], names: &HashSet<usize>) -> BTreeMap<String, u32> {
    let mut refs = BTreeMap::new();
    let mut cursor = root.walk();
    loop {
        let node = cursor.node();
        if node.child_count() == 0 {
            if is_identifier(node)
                && !names.contains(&node.start_byte())
                && let Some(text) = text_of(node, source)
            {
                count(&mut refs, text);
            }
        } else if cursor.goto_first_child() {
            continue;
        }
        while !cursor.goto_next_sibling() {
            if !cursor.goto_parent() {
                return refs;
            }
        }
    }
}

fn is_identifier(node: Node<'_>) -> bool {
    node.is_named() && (node.kind().ends_with("identifier") || node.kind() == "word")
}

/// Deduplicate definitions per node (the first pattern wins), resolve scopes and turn
/// functions inside class-like scopes into methods.
fn assemble(candidates: Vec<Candidate>, refs: BTreeMap<String, u32>) -> Tags {
    let mut by_node: BTreeMap<(usize, usize), Candidate> = BTreeMap::new();
    for candidate in candidates {
        match by_node.get(&(candidate.start, candidate.end)) {
            Some(kept) if kept.pattern <= candidate.pattern => {}
            _ => {
                by_node.insert((candidate.start, candidate.end), candidate);
            }
        }
    }
    // Scopes: containers and scope-kind definitions, outermost first.
    let mut scope_nodes: Vec<&Candidate> = by_node
        .values()
        .filter(|c| c.kind.is_none_or(Kind::is_scope))
        .collect();
    scope_nodes.sort_by(|a, b| a.start.cmp(&b.start).then(b.end.cmp(&a.end)));
    let enclosing = |start: usize, end: usize| {
        scope_nodes
            .iter()
            .rposition(|s| s.start <= start && s.end >= end && (s.start, s.end) != (start, end))
            .map(|i| i as u32)
    };
    let scopes: Vec<Scope> = scope_nodes
        .iter()
        .map(|s| Scope {
            line: s.line,
            signature: s.signature.clone(),
            parent: enclosing(s.start, s.end),
        })
        .collect();
    let mut defs: Vec<Def> = by_node
        .values()
        .filter_map(|c| {
            let kind = c.kind?;
            let scope = enclosing(c.start, c.end);
            let member = scope.is_some_and(|i| scope_nodes[i as usize].kind != Some(Kind::Module));
            Some(Def {
                name: c.name.clone(),
                kind: if kind == Kind::Function && member {
                    Kind::Method
                } else {
                    kind
                },
                line: c.line,
                signature: c.signature.clone(),
                scope,
            })
        })
        .collect();
    defs.sort_by(|a, b| a.line.cmp(&b.line).then_with(|| a.name.cmp(&b.name)));
    Tags { defs, scopes, refs }
}

#[cfg(test)]
#[path = "extract_tests.rs"]
mod tests;
