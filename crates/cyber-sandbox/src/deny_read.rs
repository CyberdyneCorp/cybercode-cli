//! `deny_read` globs of a sandbox profile (`sandbox` → Named sandbox profiles).
//!
//! A pattern is anchored: `~/` expands to home, a relative pattern is relative to the
//! Location. `**/` matches any number of directories, `**` any sequence, `*` and `?` stay
//! within one path component. A match also covers everything below it, so a denied
//! directory hides its contents. The same regular expression drives the Seatbelt profile,
//! the bubblewrap expansion and the hiding in `read`, `glob` and `grep`.

use std::path::{Path, PathBuf};

/// Directory entries visited while expanding one pattern for bubblewrap: enough for a
/// checkout with build output and installed packages under `**/`. A pattern that visits
/// more must be narrowed; the launch fails closed instead of skipping matches.
const MAX_EXPANSION_ENTRIES: usize = 1_000_000;

/// Compiled `deny_read` patterns.
#[derive(Debug, Clone, Default)]
pub struct DenyRead {
    patterns: Vec<(String, String, regex::Regex)>,
}

impl DenyRead {
    /// Anchor and compile `patterns`. Location and home should be canonical.
    pub fn new(patterns: &[String], location: &Path, home: &Path) -> Self {
        let patterns = patterns
            .iter()
            .map(|raw| {
                let anchored = anchor(raw, location, home);
                let (source, compiled) = compile(&anchored);
                (anchored, source, compiled)
            })
            .collect();
        Self { patterns }
    }

    pub fn is_empty(&self) -> bool {
        self.patterns.is_empty()
    }

    /// Anchored glob patterns, for `cyber sandbox explain`.
    pub fn anchored(&self) -> Vec<String> {
        self.patterns.iter().map(|(a, _, _)| a.clone()).collect()
    }

    /// Regular expressions for the Seatbelt profile.
    pub fn regexes(&self) -> Vec<String> {
        self.patterns.iter().map(|(_, r, _)| r.clone()).collect()
    }

    /// Exact expressions for the directories whose rename or removal would move a match
    /// out of its pattern: each prefix of a pattern ending in a component with a literal
    /// part (`config`, `conf*`). A `*` or `**` component matches any new name.
    pub fn ancestor_regexes(&self) -> Vec<String> {
        let mut out = Vec::new();
        for (anchored, _, _) in &self.patterns {
            let parts: Vec<&str> = anchored.split('/').collect();
            for end in 1..parts.len() {
                if matches!(parts[end - 1], "" | "*" | "**") {
                    continue;
                }
                let exact = glob_regex(&parts[..end].join("/"), false);
                if regex::Regex::new(&exact).is_ok() && !out.contains(&exact) {
                    out.push(exact);
                }
            }
        }
        out
    }

    /// Whether `path` (or, when it exists, its canonical form) is hidden.
    pub fn denies(&self, path: &Path) -> bool {
        if self.patterns.is_empty() {
            return false;
        }
        let lexical = slash(path);
        let canonical = std::fs::canonicalize(path).ok().map(|p| slash(&p));
        self.patterns.iter().any(|(_, _, re)| {
            re.is_match(&lexical) || canonical.as_deref().is_some_and(|c| re.is_match(c))
        })
    }

    /// Existing paths that match, for platforms that bind over files instead of matching
    /// patterns. Symbolic links are not followed. Fails when a pattern visits too much.
    pub fn expand(&self) -> Result<Vec<PathBuf>, String> {
        let mut out = Vec::new();
        for (anchored, _, re) in &self.patterns {
            let base = literal_base(anchored);
            expand_one(&base, re, anchored, &mut out)?;
        }
        out.sort();
        out.dedup();
        Ok(out)
    }
}

/// Never drop a deny: an expression that does not compile hides the pattern's whole
/// literal base instead, and failing that, everything.
fn compile(anchored: &str) -> (String, regex::Regex) {
    let full = pattern_regex(anchored);
    if let Ok(re) = regex::Regex::new(&full) {
        return (full, re);
    }
    let base = pattern_regex(&slash(&literal_base(anchored)));
    match regex::Regex::new(&base) {
        Ok(re) => (base, re),
        Err(_) => (
            "^/".into(),
            regex::Regex::new("^/").expect("a literal expression compiles"),
        ),
    }
}

fn expand_one(
    base: &Path,
    re: &regex::Regex,
    anchored: &str,
    out: &mut Vec<PathBuf>,
) -> Result<(), String> {
    let mut stack = vec![base.to_path_buf()];
    let mut visited = 0usize;
    while let Some(path) = stack.pop() {
        visited += 1;
        if visited > MAX_EXPANSION_ENTRIES {
            return Err(format!(
                "deny_read pattern {anchored} matches too many paths to enforce; narrow it"
            ));
        }
        let Ok(meta) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        if re.is_match(&slash(&path)) {
            out.push(path);
            continue;
        }
        if meta.is_dir()
            && (path == base || !git_objects(&path))
            && let Ok(entries) = std::fs::read_dir(&path)
        {
            stack.extend(entries.flatten().map(|entry| entry.path()));
        }
    }
    Ok(())
}

/// Git's object store holds only hash-named, compressed objects, never a file under its
/// own name (committed contents stay readable through git whatever `deny_read` says), so
/// the walk does not descend into it unless a pattern's literal base lies inside it.
fn git_objects(path: &Path) -> bool {
    path.file_name().is_some_and(|name| name == "objects")
        && path
            .parent()
            .and_then(Path::file_name)
            .is_some_and(|name| name == ".git")
}

/// The longest leading part of a pattern without glob characters, as a directory.
fn literal_base(anchored: &str) -> PathBuf {
    let cut = anchored.find(['*', '?']).unwrap_or(anchored.len());
    let literal = &anchored[..cut];
    let dir = match literal.rfind('/') {
        Some(0) => "/",
        Some(index) if cut < anchored.len() => &literal[..index],
        _ => literal,
    };
    PathBuf::from(if dir.is_empty() { "/" } else { dir })
}

fn anchor(raw: &str, location: &Path, home: &Path) -> String {
    let raw = plain(&raw.replace('\\', "/"));
    let joined = if let Some(rest) = raw.strip_prefix("~/") {
        format!("{}/{rest}", slash(home))
    } else if raw == "~" {
        slash(home)
    } else if is_absolute(&raw) {
        raw
    } else {
        format!("{}/{}", slash(location), raw.trim_start_matches("./"))
    };
    joined.trim_end_matches('/').to_string()
}

fn is_absolute(raw: &str) -> bool {
    raw.starts_with('/') || (raw.len() > 2 && raw.as_bytes()[1] == b':')
}

/// A POSIX extended regular expression both Seatbelt and the `regex` crate accept, for
/// a match and everything below it.
fn pattern_regex(anchored: &str) -> String {
    glob_regex(anchored, true)
}

/// The expression of `anchored`, with its descendants or for the path alone.
fn glob_regex(anchored: &str, descendants: bool) -> String {
    // A trailing `/**` names the directory itself; descendants are always covered.
    let mut anchored = anchored;
    while let Some(dir) = anchored.strip_suffix("/**") {
        anchored = dir;
    }
    let chars: Vec<char> = anchored.chars().collect();
    let mut out = String::from("^");
    let mut i = 0;
    while i < chars.len() {
        let next = chars.get(i + 1).copied();
        match (chars[i], next) {
            ('*', Some('*')) if chars.get(i + 2) == Some(&'/') => {
                out.push_str("(.*/)?");
                i += 3;
                continue;
            }
            ('*', Some('*')) => {
                out.push_str(".*");
                i += 2;
                continue;
            }
            ('*', _) => out.push_str("[^/]*"),
            ('?', _) => out.push_str("[^/]"),
            // Quotes and control characters would end the Seatbelt string literal.
            (c, _) if c == '"' || c.is_control() => out.push('.'),
            (c, _) if "\\.+()|^$[]{}".contains(c) => {
                out.push('\\');
                out.push(c);
            }
            (c, _) => out.push(c),
        }
        i += 1;
    }
    out.push_str(if descendants { "(/.*)?$" } else { "$" });
    out
}

fn slash(path: &Path) -> String {
    plain(&path.display().to_string().replace('\\', "/"))
}

/// A Windows verbatim path (`\\?\C:\repo`, `\\?\UNC\host\share`, as `canonicalize`
/// returns them) in its ordinary form, so patterns and paths compare whichever form they
/// came in. Its `?` would otherwise also end a pattern's literal base.
fn plain(slashed: &str) -> String {
    if let Some(rest) = slashed.strip_prefix("//?/UNC/") {
        format!("//{rest}")
    } else if let Some(rest) = slashed.strip_prefix("//?/") {
        rest.to_string()
    } else {
        slashed.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn deny(patterns: &[&str]) -> DenyRead {
        let patterns: Vec<String> = patterns.iter().map(|p| p.to_string()).collect();
        DenyRead::new(&patterns, Path::new("/repo"), Path::new("/home/u"))
    }

    #[test]
    fn patterns_anchor_at_the_location_and_home_and_cover_descendants() {
        let d = deny(&["**/*.pem", "~/.aws/**", "secrets"]);
        assert_eq!(
            d.anchored(),
            ["/repo/**/*.pem", "/home/u/.aws/**", "/repo/secrets"]
        );
        for hidden in [
            "/repo/key.pem",
            "/repo/a/b/key.pem",
            "/home/u/.aws/credentials",
            "/home/u/.aws",
            "/repo/secrets/token",
        ] {
            assert!(d.denies(Path::new(hidden)), "{hidden}");
        }
        for visible in [
            "/repo/key.pem.txt",
            "/other/key.pem",
            "/home/u/.awsx",
            "/repo/src/a.rs",
        ] {
            assert!(!d.denies(Path::new(visible)), "{visible}");
        }
    }

    #[test]
    fn ancestors_with_a_literal_part_are_guarded_exactly() {
        let d = deny(&["config/*.key", "**/*.pem", "conf*/x/*/a.key"]);
        assert_eq!(
            d.ancestor_regexes(),
            [
                "^/repo$",
                "^/repo/config$",
                "^/repo/conf[^/]*$",
                "^/repo/conf[^/]*/x$"
            ]
        );
    }

    #[test]
    fn single_star_stays_within_a_component() {
        let d = deny(&["config/*.key"]);
        assert!(d.denies(Path::new("/repo/config/a.key")));
        assert!(!d.denies(Path::new("/repo/config/sub/a.key")));
        assert_eq!(
            literal_base("/repo/config/*.key"),
            PathBuf::from("/repo/config")
        );
        assert_eq!(
            literal_base("/repo/secrets"),
            PathBuf::from("/repo/secrets")
        );
    }

    /// Regression: `**/*.pem` over a checkout with more than 20,000 entries (build
    /// output, packages) still expands, so every sandboxed launch does not fail.
    #[test]
    fn expansion_of_a_large_checkout_succeeds() {
        let tmp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        for dir in 0..50 {
            let dir = root.join(format!("target/d{dir}"));
            std::fs::create_dir_all(&dir).unwrap();
            for file in 0..500 {
                std::fs::write(dir.join(format!("f{file}.o")), "").unwrap();
            }
        }
        std::fs::create_dir_all(root.join(".git/objects/ab")).unwrap();
        std::fs::write(root.join(".git/objects/ab/cdef"), "").unwrap();
        std::fs::write(root.join("target/d7/key.pem"), "k").unwrap();
        let d = DenyRead::new(&["**/*.pem".into()], &root, Path::new("/home/u"));
        let found: Vec<PathBuf> = d
            .expand()
            .unwrap()
            .iter()
            .map(|p| std::fs::canonicalize(p).unwrap())
            .collect();
        assert_eq!(found, vec![root.join("target/d7/key.pem")]);
    }

    /// Regression: on Windows `canonicalize` returns `\\?\C:\…`; a pattern anchored at
    /// such a Location still hides the ordinary path of a match, and the other way round.
    #[test]
    fn verbatim_and_ordinary_windows_paths_match_the_same_patterns() {
        let verbatim = deny_at(&["**/*.pem"], r"\\?\C:\repo");
        assert!(verbatim.denies(Path::new(r"C:\repo\keys\a.pem")));
        assert!(verbatim.denies(Path::new(r"\\?\C:\repo\a.pem")));
        assert!(!verbatim.denies(Path::new(r"C:\repo\a.txt")));
        assert_eq!(verbatim.anchored(), ["C:/repo/**/*.pem"]);
        let ordinary = deny_at(&["**/*.pem"], r"C:\repo");
        assert!(ordinary.denies(Path::new(r"\\?\C:\repo\a.pem")));
        let unc = deny_at(&["secrets"], r"\\?\UNC\host\share");
        assert!(unc.denies(Path::new(r"\\host\share\secrets\k")));
        assert_eq!(literal_base("C:/repo/**/*.pem"), PathBuf::from("C:/repo"));
    }

    fn deny_at(patterns: &[&str], location: &str) -> DenyRead {
        let patterns: Vec<String> = patterns.iter().map(|p| p.to_string()).collect();
        DenyRead::new(&patterns, Path::new(location), Path::new("/home/u"))
    }

    #[cfg(unix)]
    #[test]
    fn expansion_finds_existing_matches_without_following_links() {
        let tmp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        std::fs::create_dir_all(root.join("a/b")).unwrap();
        std::fs::write(root.join("key.pem"), "k").unwrap();
        std::fs::write(root.join("a/b/other.pem"), "k").unwrap();
        std::fs::write(root.join("a/readme"), "r").unwrap();
        let d = DenyRead::new(&["**/*.pem".into()], &root, Path::new("/home/u"));
        assert_eq!(
            d.expand().unwrap(),
            vec![root.join("a/b/other.pem"), root.join("key.pem")]
        );
    }
}
