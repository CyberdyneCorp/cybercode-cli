//! Clean-room check for the base prompts (`system-context` → Base prompt coverage: no
//! base prompt reproduces another product's prompt text).
//!
//! The fixture holds hashes of every 12-word run of the reference product's text assets
//! (prompts, templates, grammars, tool text), built by `scripts/prompt_shingles.py`;
//! this file mirrors its normalization. Checked here: every base prompt, and any
//! `.lark` grammar in the workspace crates, such as the `apply_patch` grammar.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

const WIDTH: usize = 12;

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

fn fixture() -> BTreeSet<String> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("reference_prompt_shingles.txt");
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
    // The same vector is pinned in scripts/tests/test_prompt_shingles.py.
    assert_eq!(shingle_hash(&run), "aba257081516");
    assert_eq!(words("don’t café"), ["don", "t", "caf"]);
}

/// Our base prompts and every `.lark` grammar under `crates/`.
fn our_assets() -> Vec<PathBuf> {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut assets: Vec<PathBuf> = std::fs::read_dir(manifest.join("prompts").join("base"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "md"))
        .collect();
    collect_grammars(&manifest.join(".."), &mut assets);
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

#[test]
fn no_asset_shares_a_twelve_word_run_with_the_reference_assets() {
    let known = fixture();
    assert!(known.len() > 50_000, "the fixture looks truncated");
    let assets = our_assets();
    let prompts = assets
        .iter()
        .filter(|path| path.extension().is_some_and(|ext| ext == "md"))
        .count();
    assert_eq!(prompts, 5, "core plus four family addenda");
    assert!(
        assets.iter().any(|path| path.ends_with("apply_patch.lark")),
        "the apply_patch grammar is checked: {assets:?}"
    );
    for path in &assets {
        let run = words(&std::fs::read_to_string(path).unwrap());
        for window in run.windows(WIDTH) {
            assert!(
                !known.contains(&shingle_hash(window)),
                "{} matches a reference asset: {:?}",
                path.display(),
                window.join(" ")
            );
        }
    }
}
