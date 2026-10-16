//! Syntax highlighting (`tui` → Syntax-highlighted code blocks): syntect with the pure-Rust
//! fancy-regex engine and two-face's extra syntaxes (TypeScript, TOML, Dockerfile, …),
//! mapped onto the theme's `syntax_*` tokens through a scope-prefix table, so every theme
//! colors code without bundled syntect themes.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

use ratatui::style::{Color, Modifier, Style};
use syntect::parsing::{ParseState, Scope, ScopeStack, SyntaxReference, SyntaxSet};
use syntect::util::LinesWithEndings;

use crate::theme::Tokens;

/// Longer blocks render plain.
pub const MAX_LINES: usize = 2_000;
/// A block with a longer line renders plain.
pub const MAX_LINE_CHARS: usize = 1_000;

/// One highlighted line: styled runs without the line ending.
pub type Styled = Vec<(Style, String)>;

static SYNTAXES: OnceLock<SyntaxSet> = OnceLock::new();
static LOADING: AtomicBool = AtomicBool::new(false);

fn load() -> SyntaxSet {
    two_face::syntax::extra_newlines()
}

/// Whether the syntax set has loaded (without starting the load).
pub fn ready() -> bool {
    SYNTAXES.get().is_some()
}

/// The syntax set, or `None` while it is still loading. The first call starts the load
/// on its own thread, so neither startup nor a frame ever waits for it; the next redraw
/// after it lands highlights.
pub fn syntaxes() -> Option<&'static SyntaxSet> {
    if let Some(set) = SYNTAXES.get() {
        return Some(set);
    }
    if !LOADING.swap(true, Ordering::AcqRel) {
        let spawned = std::thread::Builder::new()
            .name("cyber-syntaxes".into())
            .spawn(|| {
                let _ = SYNTAXES.set(load());
            });
        if spawned.is_err() {
            LOADING.store(false, Ordering::Release);
        }
    }
    None
}

/// Load the syntax set on this thread (tests, which need a deterministic first frame).
#[cfg(test)]
pub fn preload() -> &'static SyntaxSet {
    SYNTAXES.get_or_init(load)
}

/// Fence names that are neither a syntax's name nor one of its extensions.
const ALIASES: &[(&str, &str)] = &[
    ("py", "python"),
    ("python3", "python"),
    ("sh", "bash"),
    ("shell", "bash"),
    ("zsh", "bash"),
    ("console", "bash"),
    ("shellscript", "bash"),
    ("rs", "rust"),
    ("ts", "typescript"),
    ("js", "javascript"),
    ("node", "javascript"),
    ("golang", "go"),
    ("c++", "cpp"),
    ("yml", "yaml"),
    ("docker", "dockerfile"),
    ("jsonc", "json"),
    ("json5", "json"),
    ("md", "markdown"),
    ("ps1", "powershell"),
    ("pwsh", "powershell"),
    ("text", "txt"),
    ("plaintext", "txt"),
];

/// The syntax a fence info string names (`ts`, `py`, `rust,ignore`, `{.python}`), if any.
pub fn for_fence<'a>(set: &'a SyntaxSet, info: &str) -> Option<&'a SyntaxReference> {
    let token = fence_language(info)?.to_ascii_lowercase();
    let name = ALIASES
        .iter()
        .find(|(alias, _)| *alias == token)
        .map_or(token.as_str(), |(_, name)| name);
    set.find_syntax_by_token(name)
}

/// The language word of a fence info string, as the label shows it.
pub fn fence_language(info: &str) -> Option<&str> {
    info.trim()
        .trim_start_matches(['{', '.'])
        .split(|c: char| c.is_whitespace() || matches!(c, ',' | '{' | '}'))
        .next()
        .filter(|word| !word.is_empty())
}

/// The syntax of a file path: its name (`Dockerfile`, `Makefile`), then its extension.
pub fn for_path<'a>(set: &'a SyntaxSet, path: &str) -> Option<&'a SyntaxReference> {
    let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
    set.find_syntax_by_extension(name).or_else(|| {
        let (_, ext) = name.rsplit_once('.')?;
        set.find_syntax_by_extension(ext)
    })
}

/// Whether `code` is within the highlighting limits.
pub fn within_limits(code: &str) -> bool {
    let mut lines = 0;
    for line in code.lines() {
        lines += 1;
        if lines > MAX_LINES || line.chars().count() > MAX_LINE_CHARS {
            return false;
        }
    }
    true
}

/// `code` colored by scope, one entry per line; `None` when it is over the limits or the
/// grammar fails, so the caller renders it plain.
pub fn highlight(
    set: &SyntaxSet,
    syntax: &SyntaxReference,
    code: &str,
    t: &Tokens,
) -> Option<Vec<Styled>> {
    if !within_limits(code) {
        return None;
    }
    let table = table();
    let mut state = ParseState::new(syntax);
    let mut stack = ScopeStack::new();
    let mut out = Vec::new();
    for line in LinesWithEndings::from(code) {
        let ops = state.parse_line(line, set).ok()?;
        let mut runs: Styled = Vec::new();
        let mut at = 0;
        for (index, op) in &ops {
            push(&mut runs, style_of(table, &stack, t), &line[at..*index]);
            at = *index;
            stack.apply(op).ok()?;
        }
        push(&mut runs, style_of(table, &stack, t), &line[at..]);
        out.push(runs);
    }
    Some(out)
}

/// Append `text` (without its line ending) to `runs`, merging with an equal style.
fn push(runs: &mut Styled, style: Style, text: &str) {
    let text = text.trim_end_matches(['\n', '\r']);
    if text.is_empty() {
        return;
    }
    match runs.last_mut() {
        Some((last, content)) if *last == style => content.push_str(text),
        _ => runs.push((style, text.to_owned())),
    }
}

type Pick = fn(&Tokens) -> Color;

/// Scope prefixes to tokens, most specific first; the innermost scope that matches wins.
const SCOPES: &[(&str, Pick, bool)] = &[
    ("comment", |t| t.syntax_comment, true),
    ("punctuation.definition.comment", |t| t.syntax_comment, true),
    ("string", |t| t.syntax_string, false),
    ("punctuation.definition.string", |t| t.syntax_string, false),
    ("constant.character", |t| t.syntax_string, false),
    ("constant.numeric", |t| t.syntax_number, false),
    ("constant.language", |t| t.syntax_keyword, false),
    ("constant", |t| t.syntax_number, false),
    ("keyword.operator", |t| t.syntax_operator, false),
    ("keyword", |t| t.syntax_keyword, false),
    ("storage.type.primitive", |t| t.syntax_type, false),
    ("storage", |t| t.syntax_keyword, false),
    ("entity.name.function", |t| t.syntax_function, false),
    ("variable.function", |t| t.syntax_function, false),
    ("support.function", |t| t.syntax_function, false),
    (
        "meta.function-call.identifier",
        |t| t.syntax_function,
        false,
    ),
    ("entity.name.type", |t| t.syntax_type, false),
    ("entity.name.class", |t| t.syntax_type, false),
    ("entity.name.struct", |t| t.syntax_type, false),
    ("entity.name.enum", |t| t.syntax_type, false),
    ("entity.name.trait", |t| t.syntax_type, false),
    ("entity.name.interface", |t| t.syntax_type, false),
    ("entity.other.inherited-class", |t| t.syntax_type, false),
    ("support.type", |t| t.syntax_type, false),
    ("support.class", |t| t.syntax_type, false),
    ("entity.name.tag", |t| t.syntax_keyword, false),
    ("entity.other.attribute-name", |t| t.syntax_function, false),
    ("entity.name", |t| t.syntax_function, false),
    ("support", |t| t.syntax_function, false),
    ("variable", |t| t.syntax_variable, false),
    ("markup.heading", |t| t.syntax_keyword, false),
    ("punctuation", |t| t.syntax_punctuation, false),
];

fn table() -> &'static [(Scope, Pick, bool)] {
    static TABLE: OnceLock<Vec<(Scope, Pick, bool)>> = OnceLock::new();
    TABLE.get_or_init(|| {
        SCOPES
            .iter()
            .filter_map(|&(prefix, pick, italic)| Some((Scope::new(prefix).ok()?, pick, italic)))
            .collect()
    })
}

fn style_of(table: &[(Scope, Pick, bool)], stack: &ScopeStack, t: &Tokens) -> Style {
    let found = stack.as_slice().iter().rev().find_map(|scope| {
        table
            .iter()
            .find(|(prefix, _, _)| prefix.is_prefix_of(*scope))
    });
    let Some((_, pick, italic)) = found else {
        return Style::default().fg(t.md_code);
    };
    let style = Style::default().fg(pick(t));
    if *italic {
        style.add_modifier(Modifier::ITALIC)
    } else {
        style
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tokens() -> Tokens {
        crate::theme::Catalog::default()
            .get("cyber")
            .resolve(false, crate::theme::Depth::TrueColor)
    }

    fn style_at(lines: &[Styled], row: usize, text: &str) -> Style {
        lines[row]
            .iter()
            .find(|(_, s)| s.contains(text))
            .unwrap_or_else(|| panic!("{text:?} not in {:?}", lines[row]))
            .0
    }

    #[test]
    fn python_is_colored_by_scope() {
        let set = preload();
        let t = tokens();
        let syntax = for_fence(set, "py").expect("python");
        let code = "# comment\nfor i in range(10):\n    print(\"x\", 1)\n";
        let lines = highlight(set, syntax, code, &t).expect("highlighted");
        assert_eq!(lines.len(), 3);
        let comment = style_at(&lines, 0, "comment");
        assert_eq!(comment.fg, Some(t.syntax_comment));
        assert!(comment.add_modifier.contains(Modifier::ITALIC));
        assert_eq!(style_at(&lines, 1, "for").fg, Some(t.syntax_keyword));
        assert_eq!(style_at(&lines, 1, "range").fg, Some(t.syntax_function));
        assert_eq!(style_at(&lines, 1, "10").fg, Some(t.syntax_number));
        assert_eq!(style_at(&lines, 2, "x").fg, Some(t.syntax_string));
        let text: String = lines[1].iter().map(|(_, s)| s.as_str()).collect();
        assert_eq!(text, "for i in range(10):");
    }

    #[test]
    fn aliases_extensions_and_extra_syntaxes_resolve() {
        let set = preload();
        let name = |s: Option<&SyntaxReference>| s.map(|s| s.name.clone());
        assert_eq!(name(for_fence(set, "rs")).as_deref(), Some("Rust"));
        assert_eq!(
            name(for_fence(set, "sh")).as_deref(),
            Some("Bourne Again Shell (bash)")
        );
        assert_eq!(name(for_fence(set, "ts")).as_deref(), Some("TypeScript"));
        assert_eq!(name(for_fence(set, "TOML")).as_deref(), Some("TOML"));
        assert_eq!(
            name(for_fence(set, "dockerfile")).as_deref(),
            Some("Dockerfile")
        );
        assert_eq!(name(for_fence(set, "rust,ignore")).as_deref(), Some("Rust"));
        assert_eq!(name(for_fence(set, "{.python}")).as_deref(), Some("Python"));
        assert_eq!(name(for_path(set, "src/main.rs")).as_deref(), Some("Rust"));
        assert_eq!(
            name(for_path(set, r"C:\x\Dockerfile")).as_deref(),
            Some("Dockerfile")
        );
        assert_eq!(
            name(for_path(set, "web/app.tsx")).as_deref(),
            Some("TypeScriptReact")
        );
        assert!(for_fence(set, "no-such-language").is_none());
        assert!(for_fence(set, "").is_none());
        assert!(for_path(set, "README").is_none());
    }

    #[test]
    fn typescript_highlights_with_the_fancy_engine() {
        let set = preload();
        let t = tokens();
        let syntax = for_fence(set, "ts").unwrap();
        let lines = highlight(set, syntax, "const n: number = 1; // one\n", &t).unwrap();
        assert_eq!(style_at(&lines, 0, "const").fg, Some(t.syntax_keyword));
        assert_eq!(style_at(&lines, 0, "one").fg, Some(t.syntax_comment));
    }

    #[test]
    fn oversized_blocks_are_not_highlighted() {
        let set = preload();
        let t = tokens();
        let syntax = for_fence(set, "python").unwrap();
        let long_line = "x".repeat(MAX_LINE_CHARS + 1);
        assert!(highlight(set, syntax, &long_line, &t).is_none());
        let many = "x = 1\n".repeat(MAX_LINES + 1);
        assert!(highlight(set, syntax, &many, &t).is_none());
        assert!(highlight(set, syntax, &"x = 1\n".repeat(MAX_LINES), &t).is_some());
    }

    #[test]
    fn every_scope_prefix_parses() {
        assert_eq!(table().len(), SCOPES.len());
    }
}
