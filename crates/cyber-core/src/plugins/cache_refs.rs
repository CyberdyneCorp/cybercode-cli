//! Who uses a cached plugin tree (`plugins-marketplace` → Plugin sources). The cache is
//! shared by every scope, checkout and user configuration directory, so each tree keeps a
//! `<tree>.refs` ledger of the files that record its digest (a scope file, or a project's
//! lockfile). A tree is deleted only once no listed file still records its digest.

use std::path::{Path, PathBuf};

fn ledger(tree: &Path) -> PathBuf {
    let mut name = tree.file_name().unwrap_or_default().to_os_string();
    name.push(".refs");
    tree.with_file_name(name)
}

fn read(tree: &Path) -> Vec<PathBuf> {
    std::fs::read_to_string(ledger(tree))
        .ok()
        .and_then(|text| serde_json::from_str::<Vec<PathBuf>>(&text).ok())
        .unwrap_or_default()
}

fn write(tree: &Path, holders: &[PathBuf]) -> Result<(), String> {
    let path = ledger(tree);
    let text = serde_json::to_string_pretty(holders).map_err(|e| e.to_string())?;
    let staged = path.with_extension(format!("refs-{}", std::process::id()));
    std::fs::write(&staged, text).map_err(|e| format!("{}: {e}", staged.display()))?;
    std::fs::rename(&staged, &path).map_err(|e| format!("{}: {e}", path.display()))
}

/// The holders that still record `digest`; a moved or edited file no longer counts.
fn live(tree: &Path, digest: &str) -> Vec<PathBuf> {
    read(tree)
        .into_iter()
        .filter(|holder| std::fs::read_to_string(holder).is_ok_and(|text| text.contains(digest)))
        .collect()
}

/// Record that `holder` now records `digest` for `tree`.
pub(super) fn retain(tree: &Path, digest: &str, holder: &Path) -> Result<(), String> {
    let mut holders = live(tree, digest);
    if !holders.iter().any(|known| known == holder) {
        holders.push(holder.to_path_buf());
    }
    write(tree, &holders)
}

/// Drop `holder`'s reference, deleting `tree` once nothing records `digest` any more.
/// A tree with no ledger at all is someone else's and is left alone.
pub(super) fn release(tree: &Path, digest: &str, holder: &Path) -> Result<(), String> {
    if !ledger(tree).is_file() {
        return Ok(());
    }
    let holders: Vec<PathBuf> = live(tree, digest)
        .into_iter()
        .filter(|known| known != holder)
        .collect();
    if !holders.is_empty() {
        return write(tree, &holders);
    }
    delete(tree)
}

/// Delete a tree this process fetched but nobody committed.
pub(super) fn discard(tree: &Path, digest: &str) -> Result<(), String> {
    if live(tree, digest).is_empty() {
        delete(tree)?;
    }
    Ok(())
}

fn delete(tree: &Path) -> Result<(), String> {
    if tree.is_dir() {
        std::fs::remove_dir_all(tree).map_err(|e| format!("{}: {e}", tree.display()))?;
    }
    match std::fs::remove_file(ledger(tree)) {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
            Err(format!("{}: {error}", ledger(tree).display()))
        }
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tree_is_deleted_only_when_no_holder_records_its_digest() {
        let dir = tempfile::tempdir().unwrap();
        let tree = dir.path().join("kit/1.0.0-ab");
        std::fs::create_dir_all(&tree).unwrap();
        let (a, b) = (dir.path().join("a.json"), dir.path().join("b.json"));
        for holder in [&a, &b] {
            std::fs::write(holder, "sha256:ab").unwrap();
            retain(&tree, "sha256:ab", holder).unwrap();
        }
        release(&tree, "sha256:ab", &a).unwrap();
        assert!(tree.is_dir(), "b still records the digest");
        // A holder that no longer records the digest does not keep the tree.
        std::fs::write(&b, "{}").unwrap();
        discard(&tree, "sha256:ab").unwrap();
        assert!(!tree.exists());
        assert!(!ledger(&tree).exists());
    }

    #[test]
    fn release_leaves_a_tree_without_a_ledger_alone() {
        let dir = tempfile::tempdir().unwrap();
        let tree = dir.path().join("kit/1.0.0-ab");
        std::fs::create_dir_all(&tree).unwrap();
        release(&tree, "sha256:ab", &dir.path().join("a.json")).unwrap();
        assert!(tree.is_dir());
    }
}
