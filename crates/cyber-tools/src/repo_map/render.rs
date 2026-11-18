//! Rendering and the token budget (`code-intelligence` → Repository map rendering and
//! token budget). Tokens are estimated as `ceil(bytes / 4)`; the renderer keeps the
//! largest prefix of ranked definitions whose rendering fits, found by binary search
//! since a rendering only grows with its prefix.

use std::collections::{BTreeMap, BTreeSet};

use super::extract::Tags;
use super::rank::RankedDef;

/// Scope rows printed above a selected member, and the deepest indentation.
pub const MAX_DEPTH: usize = 2;
const INDENT: &str = "    ";
pub const OTHER_FILES: &str = "other files:\n";

pub fn estimate_tokens(bytes: usize) -> usize {
    bytes.div_ceil(4)
}

/// A file to render, in path order.
pub struct RenderFile<'a> {
    pub path: &'a str,
    pub tags: Option<&'a Tags>,
}

/// What one rendering shows.
#[derive(Debug, Default)]
pub struct Rendered {
    pub text: String,
    /// Shown files in output order, with their selected definition indexes in line order.
    pub files: Vec<(usize, Vec<usize>)>,
}

/// Fit `ranked` and the remaining files (by `file_rank`) into `budget_tokens`.
pub fn render(
    files: &[RenderFile<'_>],
    ranked: &[RankedDef],
    file_rank: &[f64],
    budget_tokens: usize,
) -> Rendered {
    let budget = budget_tokens * 4;
    let (mut lo, mut hi) = (0usize, ranked.len());
    while lo < hi {
        let mid = lo + (hi - lo).div_ceil(2);
        if prefix(files, &ranked[..mid]).text.len() <= budget {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    let mut rendered = prefix(files, &ranked[..lo]);
    other_files(files, file_rank, budget, &mut rendered);
    rendered
}

/// The rendering of exactly the definitions in `selected`.
pub fn prefix(files: &[RenderFile<'_>], selected: &[RankedDef]) -> Rendered {
    let mut order: Vec<usize> = Vec::new();
    let mut chosen: BTreeMap<usize, BTreeSet<usize>> = BTreeMap::new();
    for (_, file, def) in selected {
        if !chosen.contains_key(file) {
            order.push(*file);
        }
        chosen.entry(*file).or_default().insert(*def);
    }
    let mut out = Rendered::default();
    for file in order {
        let Some(tags) = files[file].tags else {
            continue;
        };
        let defs = &chosen[&file];
        out.text.push_str(files[file].path);
        out.text.push_str(":\n");
        for row in rows(tags, defs) {
            out.text.push_str(&row.1);
        }
        let mut by_line: Vec<usize> = defs.iter().copied().collect();
        by_line.sort_by_key(|i| (tags.defs[*i].line, *i));
        out.files.push((file, by_line));
    }
    out
}

/// The rows of one file: selected definitions plus up to two enclosing scope rows each,
/// in line order, a row shared by a scope and a definition printed once.
fn rows(tags: &Tags, defs: &BTreeSet<usize>) -> BTreeSet<(u32, String)> {
    let mut rows = BTreeSet::new();
    for index in defs {
        let def = &tags.defs[*index];
        let chain = ancestors(tags, def.scope);
        rows.insert((def.line, row(def.line, chain.len(), &def.signature)));
        for (level, scope) in chain.iter().enumerate().take(MAX_DEPTH) {
            let scope = &tags.scopes[*scope];
            let depth = chain.len() - level - 1;
            rows.insert((scope.line, row(scope.line, depth, &scope.signature)));
        }
    }
    rows
}

/// Enclosing scopes, innermost first.
fn ancestors(tags: &Tags, mut scope: Option<u32>) -> Vec<usize> {
    let mut chain = Vec::new();
    while let Some(index) = scope {
        let index = index as usize;
        if chain.contains(&index) || index >= tags.scopes.len() {
            break;
        }
        chain.push(index);
        scope = tags.scopes[index].parent;
    }
    chain
}

fn row(line: u32, depth: usize, signature: &str) -> String {
    format!(
        "  {line}: {}{signature}\n",
        INDENT.repeat(depth.min(MAX_DEPTH))
    )
}

/// Files without selected rows, by file rank (then path order), while they fit.
fn other_files(files: &[RenderFile<'_>], file_rank: &[f64], budget: usize, out: &mut Rendered) {
    let shown: BTreeSet<usize> = out.files.iter().map(|(file, _)| *file).collect();
    let mut rest: Vec<usize> = (0..files.len()).filter(|f| !shown.contains(f)).collect();
    rest.sort_by(|a, b| file_rank[*b].total_cmp(&file_rank[*a]).then(a.cmp(b)));
    let mut header = false;
    for file in rest {
        let line = format!("{}\n", files[file].path);
        let extra = if header { 0 } else { OTHER_FILES.len() };
        if out.text.len() + extra + line.len() > budget {
            break;
        }
        if !header {
            out.text.push_str(OTHER_FILES);
            header = true;
        }
        out.text.push_str(&line);
        out.files.push((file, Vec::new()));
    }
}

#[cfg(test)]
#[path = "render_tests.rs"]
mod tests;
