use super::*;
use crate::repo_map::extract::{Def, Kind, Scope};

fn def(name: &str, line: u32, scope: Option<u32>) -> Def {
    Def {
        name: name.into(),
        kind: Kind::Function,
        line,
        signature: format!("pub fn {name}()"),
        scope,
    }
}

/// `src/server/mod.rs` with `impl Server` at line 88 and its method `start` at 91.
fn server() -> Tags {
    Tags {
        defs: vec![def("start", 91, Some(0)), def("stop", 120, Some(0))],
        scopes: vec![Scope {
            line: 88,
            signature: "impl Server".into(),
            parent: None,
        }],
        refs: Default::default(),
    }
}

#[test]
fn container_line_shown() {
    let tags = server();
    let files = [RenderFile {
        path: "src/server/mod.rs",
        tags: Some(&tags),
    }];
    let rendered = prefix(&files, &[(1.0, 0, 0)]);
    assert_eq!(
        rendered.text,
        "src/server/mod.rs:\n  88: impl Server\n  91:     pub fn start()\n"
    );
}

#[test]
fn nesting_is_capped_at_two_levels() {
    let tags = Tags {
        defs: vec![def("deep", 40, Some(2))],
        scopes: vec![
            Scope {
                line: 1,
                signature: "mod a".into(),
                parent: None,
            },
            Scope {
                line: 10,
                signature: "mod b".into(),
                parent: Some(0),
            },
            Scope {
                line: 20,
                signature: "impl C".into(),
                parent: Some(1),
            },
        ],
        refs: Default::default(),
    };
    let files = [RenderFile {
        path: "x.rs",
        tags: Some(&tags),
    }];
    let rendered = prefix(&files, &[(1.0, 0, 0)]);
    assert_eq!(
        rendered.text,
        "x.rs:\n  10:     mod b\n  20:         impl C\n  40:         pub fn deep()\n"
    );
}

/// A deterministic pseudo-random repository of `files` files.
fn repository(files: usize, defs_per_file: usize, seed: u64) -> (Vec<String>, Vec<Tags>) {
    let mut state = seed;
    let mut next = move || {
        state = state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        state >> 33
    };
    let paths: Vec<String> = (0..files).map(|i| format!("src/m{i:05}.rs")).collect();
    let tags = (0..files)
        .map(|_| Tags {
            defs: (0..defs_per_file)
                .map(|j| def(&format!("f{}", next() % 100_000), j as u32 * 3 + 1, None))
                .collect(),
            scopes: Vec::new(),
            refs: Default::default(),
        })
        .collect();
    (paths, tags)
}

#[test]
fn budget_respected() {
    let (paths, tags) = repository(3_000, 10, 7);
    let files: Vec<RenderFile<'_>> = paths
        .iter()
        .zip(&tags)
        .map(|(path, tags)| RenderFile {
            path,
            tags: Some(tags),
        })
        .collect();
    let mut ranked: Vec<RankedDef> = Vec::new();
    for (f, t) in tags.iter().enumerate() {
        for d in 0..t.defs.len() {
            ranked.push((1.0 / (1 + f * 10 + d) as f64, f, d));
        }
    }
    assert_eq!(ranked.len(), 30_000);
    let file_rank: Vec<f64> = (0..files.len()).map(|f| 1.0 / (1 + f) as f64).collect();
    let rendered = render(&files, &ranked, &file_rank, 1024);
    assert!(rendered.text.len() <= 4096, "{}", rendered.text.len());
    assert!(rendered.text.len() > 3500, "the budget is used");
    assert!(rendered.text.ends_with('\n'), "ends after a complete row");
}

#[test]
fn render_is_monotonic_in_prefix_length() {
    for seed in 0..20 {
        let (paths, tags) = repository(12, 6, seed);
        let mut tags = tags;
        // Nest some definitions in scopes.
        for t in &mut tags {
            t.scopes.push(Scope {
                line: 2,
                signature: "impl Thing".into(),
                parent: None,
            });
            for d in t.defs.iter_mut().skip(1).step_by(2) {
                d.scope = Some(0);
            }
        }
        let files: Vec<RenderFile<'_>> = paths
            .iter()
            .zip(&tags)
            .map(|(path, tags)| RenderFile {
                path,
                tags: Some(tags),
            })
            .collect();
        let mut ranked: Vec<RankedDef> = Vec::new();
        let mut state = seed + 1;
        for (f, t) in tags.iter().enumerate() {
            for d in 0..t.defs.len() {
                state = state
                    .wrapping_mul(2862933555777941757)
                    .wrapping_add(3037000493);
                ranked.push(((state >> 11) as f64, f, d));
            }
        }
        ranked.sort_by(|a, b| b.0.total_cmp(&a.0));
        let mut previous = 0;
        for k in 0..=ranked.len() {
            let len = prefix(&files, &ranked[..k]).text.len();
            assert!(len >= previous, "seed {seed}: prefix {k} shrank");
            previous = len;
        }
    }
}

#[test]
fn other_files_follow_while_budget_remains() {
    let tags = server();
    let files = [
        RenderFile {
            path: "src/server/mod.rs",
            tags: Some(&tags),
        },
        RenderFile {
            path: "rtl/core.sv",
            tags: None,
        },
        RenderFile {
            path: "rtl/alu.sv",
            tags: None,
        },
    ];
    let rendered = render(&files, &[(1.0, 0, 0)], &[0.5, 0.1, 0.4], 64);
    assert_eq!(
        rendered.text,
        "src/server/mod.rs:\n  88: impl Server\n  91:     pub fn start()\nother files:\nrtl/alu.sv\nrtl/core.sv\n"
    );
    let tight = render(&files, &[(1.0, 0, 0)], &[0.5, 0.1, 0.4], 18);
    assert!(!tight.text.contains("rtl/core.sv"), "{}", tight.text);
}

#[test]
fn token_estimate_rounds_up() {
    assert_eq!(estimate_tokens(0), 0);
    assert_eq!(estimate_tokens(1), 1);
    assert_eq!(estimate_tokens(8), 2);
    assert_eq!(estimate_tokens(9), 3);
}
