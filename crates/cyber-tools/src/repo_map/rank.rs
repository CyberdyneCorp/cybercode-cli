//! Personalized PageRank over the file graph and the definition ranks derived from it
//! (`code-intelligence` → Repository map ranking). Every sum runs on one thread in a
//! fixed order, so equal inputs give identical `f64` results everywhere.

use super::extract::Tags;
use super::graph::Graph;

pub const DAMPING: f64 = 0.85;
pub const MAX_ITERATIONS: usize = 100;
pub const TOLERANCE: f64 = 1e-6;
/// Share of its file's rank an unreferenced definition receives, split over the file's
/// definitions.
pub const UNREFERENCED_SHARE: f64 = 0.001;

/// Normalize personalization weights; all zero (or empty) gives the uniform vector.
pub fn personalization(weights: &[f64]) -> Vec<f64> {
    let total: f64 = weights.iter().sum();
    if total <= 0.0 {
        let n = weights.len().max(1) as f64;
        return vec![1.0 / n; weights.len()];
    }
    weights.iter().map(|w| w / total).collect()
}

/// PageRank with damping 0.85, at most 100 iterations, stopping when the L1 change is
/// below 1e-6; dangling rank is redistributed by `p`.
pub fn pagerank(graph: &Graph, p: &[f64]) -> Vec<f64> {
    let n = graph.nodes;
    let mut rank = p.to_vec();
    for _ in 0..MAX_ITERATIONS {
        let dangling: f64 = (0..n)
            .filter(|i| graph.out_weight[*i] <= 0.0)
            .map(|i| rank[i])
            .sum();
        let mut next: Vec<f64> = p
            .iter()
            .map(|pi| (1.0 - DAMPING) * pi + DAMPING * dangling * pi)
            .collect();
        for edge in &graph.edges {
            next[edge.to] += DAMPING * rank[edge.from] * edge.weight / graph.out_weight[edge.from];
        }
        let change: f64 = next.iter().zip(&rank).map(|(a, b)| (a - b).abs()).sum();
        rank = next;
        if change < TOLERANCE {
            break;
        }
    }
    rank
}

/// A definition's rank: `(rank, file index, definition index)`.
pub type RankedDef = (f64, usize, usize);

/// Each definition collects the shares of the edges for its name into its file; an
/// unreferenced one gets `file rank × 0.001 / definitions in the file`. Sorted by rank
/// (descending), then path order, then line.
pub fn definitions(graph: &Graph, files: &[Option<&Tags>], rank: &[f64]) -> Vec<RankedDef> {
    // Incoming share per (file, name id).
    let mut incoming: std::collections::HashMap<(usize, usize), f64> = Default::default();
    for edge in &graph.edges {
        *incoming.entry((edge.to, edge.name)).or_default() +=
            rank[edge.from] * edge.weight / graph.out_weight[edge.from];
    }
    let name_ids: std::collections::HashMap<&str, usize> = graph
        .names
        .iter()
        .enumerate()
        .map(|(i, n)| (n.as_str(), i))
        .collect();
    let mut ranked = Vec::new();
    for (file, tags) in files.iter().enumerate() {
        let Some(tags) = tags else {
            continue;
        };
        let floor = rank[file] * UNREFERENCED_SHARE / tags.defs.len().max(1) as f64;
        for (index, def) in tags.defs.iter().enumerate() {
            let share = name_ids
                .get(def.name.as_str())
                .and_then(|id| incoming.get(&(file, *id)))
                .copied()
                .unwrap_or(0.0);
            ranked.push((if share > 0.0 { share } else { floor }, file, index));
        }
    }
    ranked.sort_by(|a, b| {
        b.0.total_cmp(&a.0)
            .then(a.1.cmp(&b.1))
            .then_with(|| line(files, a).cmp(&line(files, b)))
            .then(a.2.cmp(&b.2))
    });
    ranked
}

fn line(files: &[Option<&Tags>], def: &RankedDef) -> u32 {
    files[def.1].map_or(0, |tags| tags.defs[def.2].line)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;
    use crate::repo_map::extract::{Def, Kind};
    use crate::repo_map::graph::{MAX_EDGES, build};

    fn tags(defs: &[&str], refs: &[(&str, u32)]) -> Tags {
        Tags {
            defs: defs
                .iter()
                .enumerate()
                .map(|(i, name)| Def {
                    name: (*name).into(),
                    kind: Kind::Function,
                    line: i as u32 + 1,
                    signature: format!("fn {name}()"),
                    scope: None,
                })
                .collect(),
            scopes: Vec::new(),
            refs: refs.iter().map(|(n, c)| ((*n).into(), *c)).collect(),
        }
    }

    #[test]
    fn ranks_sum_to_one_and_follow_references() {
        let a = tags(&["main"], &[("helper", 3)]);
        let b = tags(&["helper"], &[]);
        let c = tags(&["unused"], &[]);
        let files = [Some(&a), Some(&b), Some(&c)];
        let graph = build(&files, &BTreeSet::new(), MAX_EDGES);
        let rank = pagerank(&graph, &personalization(&[0.0; 3]));
        assert!((rank.iter().sum::<f64>() - 1.0).abs() < 1e-6);
        assert!(rank[1] > rank[0] && rank[1] > rank[2], "{rank:?}");
    }

    #[test]
    fn unreferenced_entry_point_still_ranked() {
        let a = tags(&["main", "run"], &[("helper", 1)]);
        let b = tags(&["helper"], &[]);
        let files = [Some(&a), Some(&b)];
        let graph = build(&files, &BTreeSet::new(), MAX_EDGES);
        let rank = pagerank(&graph, &personalization(&[0.0; 2]));
        let defs = definitions(&graph, &files, &rank);
        assert_eq!(defs.len(), 3);
        assert_eq!((defs[0].1, defs[0].2), (1, 0), "helper first");
        let main = defs.iter().find(|d| (d.1, d.2) == (0, 0)).unwrap();
        assert!((main.0 - rank[0] * UNREFERENCED_SHARE / 2.0).abs() < 1e-15);
        assert!(main.0 > 0.0);
    }

    #[test]
    fn personalization_biases_ranks() {
        let a = tags(&["alpha"], &[]);
        let b = tags(&["beta"], &[]);
        let files = [Some(&a), Some(&b)];
        let graph = build(&files, &BTreeSet::new(), MAX_EDGES);
        let rank = pagerank(&graph, &personalization(&[1.0, 0.0]));
        assert!(rank[0] > 0.99, "{rank:?}");
    }
}
