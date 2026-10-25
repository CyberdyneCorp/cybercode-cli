//! Existing private roots that need one outermost filesystem overlay each.
use std::path::PathBuf;

pub(crate) fn outermost(paths: impl IntoIterator<Item = PathBuf>) -> std::io::Result<Vec<PathBuf>> {
    let mut canonical = Vec::new();
    for path in paths {
        if path.try_exists()? {
            canonical.push(std::fs::canonicalize(path)?);
        }
    }
    // Ancestors must precede descendants regardless of registration order. A
    // sealed parent overlay has no child mountpoints to create or mount over.
    canonical.sort_by(|a, b| {
        a.components()
            .count()
            .cmp(&b.components().count())
            .then_with(|| a.cmp(b))
    });
    let mut roots: Vec<PathBuf> = Vec::new();
    for path in canonical {
        if !roots.iter().any(|root| path.starts_with(root)) {
            roots.push(path);
        }
    }
    Ok(roots)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn private_parents_cover_children_without_losing_sibling_roots() {
        let temp = tempfile::tempdir().unwrap();
        let state = temp.path().join("state");
        let child = state.join("peer-tls");
        let sibling = temp.path().join("state-other");
        std::fs::create_dir_all(&child).unwrap();
        std::fs::create_dir_all(&sibling).unwrap();
        let key = child.join("key.pem");
        std::fs::write(&key, "private-test-key").unwrap();
        let roots = outermost([
            key,
            child,
            sibling.clone(),
            state.clone(),
            state.clone(),
            temp.path().join("missing"),
        ])
        .unwrap();
        assert_eq!(roots.len(), 2);
        assert!(roots.contains(&state.canonicalize().unwrap()));
        assert!(roots.contains(&sibling.canonicalize().unwrap()));
    }

    #[cfg(unix)]
    #[test]
    fn canonical_aliases_cannot_add_mounts_beneath_a_sealed_parent() {
        let temp = tempfile::tempdir().unwrap();
        let state = temp.path().join("state");
        let child = state.join("peer-tls");
        std::fs::create_dir_all(&child).unwrap();
        let alias = temp.path().join("state-alias");
        std::os::unix::fs::symlink(&state, &alias).unwrap();
        assert_eq!(
            outermost([alias.join("peer-tls"), child, alias, state.clone()]).unwrap(),
            vec![state.canonicalize().unwrap()]
        );
    }
}
