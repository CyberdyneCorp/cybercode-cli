use globset::{GlobBuilder,GlobSetBuilder};

#[test]
fn every_glob_must_match_not_just_every_strategy() {
    let groups: &[&[&str]]=&[
        &[], &["a"], &["a","b"], &["a","a"], &["**/a","**/b"],
        &["*.rs","*.toml"], &["*.rs","*.rs"], &["foo*","bar*"],
        &["foo*","fo*"], &["*foo","*bar"], &["*oo","*foo"],
        &["a*/**/*.rs","b*/**/*.rs"], &["a?","b?"], &["[ab]*","a*"],
        &["src/**/*.rs","*.c","src/lib.rs"], &["**/*.rs","src/**"],
        &["*foo*","*bar*"], &["**/a.c","**/a.c"], &["*.RS","src/**"],
    ];
    let paths=["","a","b","a.c","a.rs","src/a.rs","src/lib.rs","src/a/b.c",
        "foo","bar","foobar","barfoo","fooo","a/x.rs","b/x.rs","ax","bx",
        "x/y/a","x/y/b","MAIN.RS","src/MAIN.RS","src/a.toml"];
    for patterns in groups {
        for insensitive in [false,true] {
            let mut builder=GlobSetBuilder::new();
            let mut individual=Vec::new();
            for pattern in *patterns {
                let glob=GlobBuilder::new(pattern).case_insensitive(insensitive).build().unwrap();
                individual.push(glob.compile_matcher());builder.add(glob);
            }
            let set=builder.build().unwrap();
            for path in paths {
                let expected=individual.iter().all(|matcher|matcher.is_match(path));
                assert_eq!(set.matches_all(path),expected,"patterns={patterns:?} path={path:?} case_insensitive={insensitive}");
                assert_eq!(set.matches_all_candidate(&globset::Candidate::new(path)),expected);
                let indices:Vec<_>=individual.iter().enumerate().filter_map(|(i,m)|m.is_match(path).then_some(i)).collect();
                assert_eq!(set.matches(path),indices,"ordinary matching changed");
                assert_eq!(set.is_match(path),!indices.is_empty());
            }
        }
    }
}
