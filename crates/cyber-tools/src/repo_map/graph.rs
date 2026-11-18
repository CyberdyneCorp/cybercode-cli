//! The file reference graph (`code-intelligence` → Repository map ranking): an edge from
//! A to B for each name A references and B defines, weighted by the name's multiplier
//! and the square root of A's reference count.

use std::collections::{BTreeMap, BTreeSet};

use super::extract::{MIN_NAME_CHARS, Tags};

/// Names defined in more files than this are ignored.
pub const MAX_DEFINERS: usize = 200;
/// Names defined in more files than this are damped.
pub const COMMON_DEFINERS: usize = 5;
pub const MAX_EDGES: usize = 2_000_000;

#[derive(Debug, Clone, PartialEq)]
pub struct Edge {
    pub from: usize,
    pub to: usize,
    pub name: usize,
    pub weight: f64,
}

#[derive(Debug, Default)]
pub struct Graph {
    pub nodes: usize,
    /// Names by id, in sorted order.
    pub names: Vec<String>,
    /// Sorted by (from, to, name).
    pub edges: Vec<Edge>,
    pub out_weight: Vec<f64>,
}

/// Names at least this long, written in snake_case or camelCase, are boosted.
pub const DISTINCTIVE_CHARS: usize = 8;
/// A name referenced from more than this share of the files (and at least
/// [`UBIQUITOUS_MIN_FILES`]) is damped: `map`, `clone` or `format` say little.
pub const UBIQUITOUS_SHARE: f64 = 0.05;
pub const UBIQUITOUS_MIN_FILES: usize = 20;

/// Whether `referrers` of `files` make a name ubiquitous.
pub fn ubiquitous(referrers: usize, files: usize) -> bool {
    referrers >= UBIQUITOUS_MIN_FILES && referrers as f64 > files as f64 * UBIQUITOUS_SHARE
}

/// `m(x)`: 10 for a focus symbol, 10 for a distinctive name, 0.1 for a private name,
/// 0.1 for a name defined in more than 5 files and 0.1 for a ubiquitous one,
/// multiplied together.
pub fn multiplier(name: &str, definers: usize, common: bool, focus: &BTreeSet<String>) -> f64 {
    let mut m = 1.0;
    if focus.contains(name) {
        m *= 10.0;
    }
    if distinctive(name) {
        m *= 10.0;
    }
    if name.starts_with('_') {
        m *= 0.1;
    }
    if definers > COMMON_DEFINERS {
        m *= 0.1;
    }
    if common {
        m *= 0.1;
    }
    m
}

/// A long compound name (`retry_policy`, `RetryPolicy`) says more than `get` or `run`.
pub fn distinctive(name: &str) -> bool {
    let compound = name.trim_start_matches('_').contains('_')
        || name
            .as_bytes()
            .windows(2)
            .any(|w| w[0].is_ascii_lowercase() && w[1].is_ascii_uppercase());
    name.chars().count() >= DISTINCTIVE_CHARS && compound
}

/// Which files define each rankable name.
fn definers<'a>(files: &[Option<&'a Tags>]) -> BTreeMap<&'a str, Vec<usize>> {
    let mut defined: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
    for (index, tags) in files.iter().enumerate() {
        for def in tags.iter().flat_map(|t| &t.defs) {
            let list = defined.entry(def.name.as_str()).or_default();
            if list.last() != Some(&index) {
                list.push(index);
            }
        }
    }
    defined
        .retain(|name, list| name.chars().count() >= MIN_NAME_CHARS && list.len() <= MAX_DEFINERS);
    defined
}

/// Build the graph over `files` (in path order).
pub fn build(files: &[Option<&Tags>], focus: &BTreeSet<String>, max_edges: usize) -> Graph {
    let defined = definers(files);
    let mut referrers: BTreeMap<&str, usize> = BTreeMap::new();
    for tags in files.iter().flatten() {
        for name in tags
            .refs
            .keys()
            .filter(|name| defined.contains_key(name.as_str()))
        {
            *referrers.entry(name.as_str()).or_default() += 1;
        }
    }
    let names: Vec<&str> = defined.keys().copied().collect();
    let ids: BTreeMap<&str, usize> = names.iter().enumerate().map(|(i, n)| (*n, i)).collect();
    let mut edges = Vec::new();
    for (from, tags) in files.iter().enumerate() {
        for (name, count) in tags.iter().flat_map(|t| &t.refs) {
            let Some(targets) = defined.get(name.as_str()) else {
                continue;
            };
            let common = ubiquitous(referrers[name.as_str()], files.len());
            let weight = multiplier(name, targets.len(), common, focus) * f64::from(*count).sqrt();
            for &to in targets.iter().filter(|to| **to != from) {
                edges.push(Edge {
                    from,
                    to,
                    name: ids[name.as_str()],
                    weight,
                });
            }
        }
    }
    cap_edges(&mut edges, names.len(), max_edges);
    edges.sort_by(|a, b| (a.from, a.to, a.name).cmp(&(b.from, b.to, b.name)));
    let mut out_weight = vec![0.0; files.len()];
    for edge in &edges {
        out_weight[edge.from] += edge.weight;
    }
    Graph {
        nodes: files.len(),
        names: names.into_iter().map(str::to_owned).collect(),
        edges,
        out_weight,
    }
}

/// Drop whole names, lowest total weight first (ties by name id), until at most `max`
/// edges remain.
fn cap_edges(edges: &mut Vec<Edge>, names: usize, max: usize) {
    if edges.len() <= max {
        return;
    }
    let mut totals = vec![(0.0f64, 0usize); names];
    for edge in edges.iter() {
        totals[edge.name].0 += edge.weight;
        totals[edge.name].1 += 1;
    }
    let mut order: Vec<usize> = (0..names).collect();
    order.sort_by(|a, b| totals[*a].0.total_cmp(&totals[*b].0).then(a.cmp(b)));
    let mut remaining = edges.len();
    let mut dropped = vec![false; names];
    for name in order {
        if remaining <= max {
            break;
        }
        dropped[name] = true;
        remaining -= totals[name].1;
    }
    edges.retain(|edge| !dropped[edge.name]);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repo_map::extract::{Def, Kind};

    fn tags(defs: &[&str], refs: &[(&str, u32)]) -> Tags {
        Tags {
            defs: defs
                .iter()
                .map(|name| Def {
                    name: (*name).into(),
                    kind: Kind::Function,
                    line: 1,
                    signature: format!("fn {name}()"),
                    scope: None,
                })
                .collect(),
            scopes: Vec::new(),
            refs: refs.iter().map(|(n, c)| ((*n).into(), *c)).collect(),
        }
    }

    #[test]
    fn private_and_common_names_damped() {
        let focus = BTreeSet::from(["focus".to_string()]);
        assert_eq!(multiplier("plain", 1, false, &focus), 1.0);
        assert_eq!(multiplier("focus", 1, false, &focus), 10.0);
        assert_eq!(multiplier("retry_policy", 1, false, &focus), 10.0);
        assert_eq!(multiplier("RetryPolicy", 1, false, &focus), 10.0);
        assert_eq!(multiplier("Retry", 1, false, &focus), 1.0, "too short");
        assert_eq!(
            multiplier("checkpoint", 1, false, &focus),
            1.0,
            "not compound"
        );
        assert!((multiplier("_private", 1, false, &focus) - 0.1).abs() < 1e-12);
        assert!((multiplier("common", 6, false, &focus) - 0.1).abs() < 1e-12);
        assert!((multiplier("_both", 6, false, &focus) - 0.01).abs() < 1e-12);
        assert!((multiplier("map", 1, true, &focus) - 0.1).abs() < 1e-12);
        assert!(!ubiquitous(19, 20), "too few files");
        assert!(ubiquitous(21, 400));
        assert!(!ubiquitous(20, 400), "exactly 5% is not more");
    }

    #[test]
    fn edges_weigh_references_and_skip_self_and_short_names() {
        let a = tags(&["alpha"], &[("beta", 4), ("alpha", 2), ("ab", 3)]);
        let b = tags(&["beta", "ab"], &[]);
        let graph = build(&[Some(&a), Some(&b)], &BTreeSet::new(), MAX_EDGES);
        assert_eq!(graph.edges.len(), 1);
        assert_eq!(graph.edges[0].from, 0);
        assert_eq!(graph.edges[0].to, 1);
        assert_eq!(graph.edges[0].weight, 2.0);
        assert_eq!(graph.out_weight, vec![2.0, 0.0]);
    }

    #[test]
    fn edge_cap_drops_lowest_weight_names_first() {
        let a = tags(&[], &[("heavy", 9), ("light", 1)]);
        let b = tags(&["heavy", "light"], &[]);
        let graph = build(&[Some(&a), Some(&b)], &BTreeSet::new(), 1);
        assert_eq!(graph.edges.len(), 1);
        assert_eq!(graph.names[graph.edges[0].name], "heavy");
    }
}
