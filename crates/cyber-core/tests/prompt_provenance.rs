//! Clean-room check for the model-facing text (`system-context` → Base prompt coverage:
//! no base prompt reproduces another product's prompt text).
//!
//! The fixtures hold hashes of every 12-word run of the reference product's text assets
//! (prompts, templates, grammars, and the string literals of its Rust sources), and of
//! every 6-word run of its grammars, built by `scripts/prompt_shingles.py`; this file
//! mirrors its normalization. Checked here: every base prompt, any `.lark` grammar in
//! the workspace crates, and the string literals of the tool modules (descriptions and
//! the messages sent back to the model).

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

const WIDTH: usize = 12;
const GRAMMAR_WIDTH: usize = 6;

/// Lowercased runs of ASCII letters and digits; everything else separates words.
fn words(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(str::to_ascii_lowercase)
        .collect()
}

fn shingle_hash(run: &[String]) -> String {
    let digest = Sha256::digest(run.join(" ").as_bytes());
    format!("{digest:x}")[..12].to_string()
}

fn is_ident(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

/// The contents of the string literals before the first `#[cfg(test)]`, skipping `//`
/// comments and char literals; an escape is a space. Mirrors `rust_literals` in the
/// generator.
fn rust_literals(source: &str) -> Vec<String> {
    let text = source
        .split_once("#[cfg(test)]")
        .map_or(source, |(code, _)| code)
        .as_bytes();
    let mut found = Vec::new();
    let mut i = 0;
    while i < text.len() {
        if text[i..].starts_with(b"//") {
            i = find(text, b"\n", i).unwrap_or(text.len());
        } else if text[i] == b'\'' {
            i = char_literal_end(text, i);
        } else if let Some(hashes) = raw_hashes(text, i) {
            let start = i + hashes + 2;
            let mut closer = vec![b'"'];
            closer.extend(std::iter::repeat_n(b'#', hashes));
            let end = find(text, &closer, start).unwrap_or(text.len());
            found.push(String::from_utf8_lossy(&text[start..end]).into_owned());
            i = end + hashes + 1;
        } else if text[i] == b'"' {
            let (body, next) = quoted(text, i + 1);
            found.push(body);
            i = next;
        } else {
            i += 1;
        }
    }
    found
}

fn find(text: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    let from = from.min(text.len());
    text[from..]
        .windows(needle.len())
        .position(|window| window == needle)
        .map(|at| at + from)
}

/// Past a char literal at `i`, or past the quote of a lifetime.
fn char_literal_end(text: &[u8], i: usize) -> usize {
    if text.get(i + 1) == Some(&b'\\') {
        return find(text, b"'", i + 3).map_or(text.len(), |end| end + 1);
    }
    if text.get(i + 2) == Some(&b'\'') {
        i + 3
    } else {
        i + 1
    }
}

/// The `#` count of a raw string opener `r#*"` at `i`.
fn raw_hashes(text: &[u8], i: usize) -> Option<usize> {
    if text[i] != b'r' || (i > 0 && is_ident(text[i - 1])) {
        return None;
    }
    let hashes = text[i + 1..].iter().take_while(|b| **b == b'#').count();
    (text.get(i + 1 + hashes) == Some(&b'"')).then_some(hashes)
}

/// A string literal's body from `i` and the index past its closing quote.
fn quoted(text: &[u8], mut i: usize) -> (String, usize) {
    let mut body = Vec::new();
    while i < text.len() && text[i] != b'"' {
        if text[i] == b'\\' {
            body.push(b' ');
            i += 2;
        } else {
            body.push(text[i]);
            i += 1;
        }
    }
    (String::from_utf8_lossy(&body).into_owned(), i + 1)
}

/// The texts of one asset: a Rust file's string literals, else the whole file.
fn texts(path: &Path) -> Vec<String> {
    let content = std::fs::read_to_string(path).unwrap();
    if path.extension().is_some_and(|ext| ext == "rs") {
        rust_literals(&content)
    } else {
        vec![content]
    }
}

fn fixture(name: &str) -> BTreeSet<String> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(name);
    std::fs::read_to_string(path)
        .unwrap()
        .lines()
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(str::to_owned)
        .collect()
}

#[test]
fn normalization_matches_the_generator() {
    let run = words("One, two: three\n- four *five* six seven-eight nine ten eleven twelve.");
    assert_eq!(run.len(), WIDTH);
    // The same vectors are pinned in scripts/tests/test_prompt_shingles.py.
    assert_eq!(shingle_hash(&run), "aba257081516");
    assert_eq!(words("don’t café"), ["don", "t", "caf"]);
    let source = "let c = '\"'; let d = '\\''; let s: &'a str = \"one\\ntwo\"; // a \"note\n\
                  let r = r#\"raw \"x\" text\"#;\n#[cfg(test)]\nmod t { \"hidden\" }";
    assert_eq!(rust_literals(source), ["one two", "raw \"x\" text"]);
}

/// Our base prompts, every `.lark` grammar under `crates/`, and the tool modules.
fn our_assets() -> Vec<PathBuf> {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let in_dir = |dir: PathBuf, ext: &str| -> Vec<PathBuf> {
        std::fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| path.extension().is_some_and(|e| e == ext))
            .collect()
    };
    let mut assets = in_dir(manifest.join("prompts").join("base"), "md");
    collect_grammars(&manifest.join(".."), &mut assets);
    let tools = manifest
        .join("..")
        .join("cyber-tools")
        .join("src")
        .join("tools");
    assets.extend(in_dir(tools, "rs"));
    assets.sort();
    assets
}

fn collect_grammars(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        if path.is_dir() && !matches!(name.as_ref(), "target" | "node_modules") {
            collect_grammars(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "lark") {
            out.push(path);
        }
    }
}

fn assert_no_shared_run(path: &Path, text: &str, width: usize, known: &BTreeSet<String>) {
    let run = words(text);
    for window in run.windows(width) {
        assert!(
            !known.contains(&shingle_hash(window)),
            "{} matches a reference asset: {:?}",
            path.display(),
            window.join(" ")
        );
    }
}

#[test]
fn no_asset_shares_a_run_with_the_reference_assets() {
    let known = fixture("reference_prompt_shingles.txt");
    let grammar = fixture("reference_grammar_shingles.txt");
    assert!(known.len() > 100_000, "the fixture looks truncated");
    assert!(grammar.len() > 20, "the grammar fixture looks truncated");
    let assets = our_assets();
    let has = |ext: &str| {
        assets
            .iter()
            .filter(|p| p.extension().is_some_and(|e| e == ext))
            .count()
    };
    assert_eq!(has("md"), 5, "core plus four family addenda");
    assert!(has("rs") > 20, "the tool modules are checked: {assets:?}");
    assert!(
        assets.iter().any(|path| path.ends_with("apply_patch.lark")),
        "the apply_patch grammar is checked: {assets:?}"
    );
    for path in &assets {
        for text in texts(path) {
            assert_no_shared_run(path, &text, WIDTH, &known);
        }
        if path.extension().is_some_and(|ext| ext == "lark") {
            let text = std::fs::read_to_string(path).unwrap();
            assert_no_shared_run(path, &text, GRAMMAR_WIDTH, &grammar);
        }
    }
}
