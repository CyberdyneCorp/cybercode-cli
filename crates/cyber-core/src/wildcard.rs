//! Permission pattern matching (`permissions-modes` → Wildcard matching).
//!
//! Patterns match whole strings: `*` matches any sequence (including `/` and newlines) and
//! `?` one character. Backslashes are normalized to `/`. A pattern ending in ` *` also matches
//! the bare prefix, so `git log *` matches `git log`. Matching is case-insensitive on Windows.

pub fn matches(pattern: &str, value: &str) -> bool {
    let pattern = normalize(pattern);
    let value = normalize(value);
    if let Some(prefix) = pattern.strip_suffix(" *")
        && value == prefix
    {
        return true;
    }
    glob(
        &pattern.chars().collect::<Vec<_>>(),
        &value.chars().collect::<Vec<_>>(),
    )
}

fn normalize(s: &str) -> String {
    let s = s.replace('\\', "/");
    if cfg!(windows) { s.to_lowercase() } else { s }
}

/// Iterative glob with single-star backtracking: linear in practice, no recursion.
fn glob(p: &[char], v: &[char]) -> bool {
    let (mut pi, mut vi) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    while vi < v.len() {
        match p.get(pi) {
            Some('*') => {
                star = Some((pi, vi));
                pi += 1;
            }
            Some('?') => {
                pi += 1;
                vi += 1;
            }
            Some(c) if *c == v[vi] => {
                pi += 1;
                vi += 1;
            }
            _ => match star {
                Some((sp, sv)) => {
                    pi = sp + 1;
                    vi = sv + 1;
                    star = Some((sp, sv + 1));
                }
                None => return false,
            },
        }
    }
    p[pi..].iter().all(|c| *c == '*')
}

#[cfg(test)]
mod tests {
    use super::matches;

    #[test]
    fn whole_string_semantics() {
        assert!(matches("git log *", "git log"));
        assert!(matches("git log *", "git log --oneline"));
        assert!(!matches("git log *", "git logx"));
        assert!(matches("*", "rm -rf /\nsecond line"));
        assert!(matches("src/*", "src/a/b.rs"), "* crosses slashes");
        assert!(matches("*.env.*", "app/.env.local"));
        assert!(!matches("*.env", "app/.env.local"));
        assert!(matches("a?c", "abc") && !matches("a?c", "ac"));
        assert!(matches("C:/work/*", "C:\\work\\x"));
        assert!(!matches("npm test", "npm test --watch"));
    }
}
