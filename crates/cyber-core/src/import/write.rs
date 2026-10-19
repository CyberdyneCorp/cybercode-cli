//! Reviewed import writes (`compat-import` → Import command, Mapping report, Idempotent
//! re-import).
//!
//! A proposal becomes byte edits of one native document (`cyber.jsonc` at the target):
//! each entry it adds is inserted with an `x-imported-from` line comment naming its
//! sources and a digest of the imported value, and every other byte is kept. A later
//! import replaces an entry only while it still carries that annotation and still holds
//! the imported value; an edited entry is kept as `merged`.
use super::provenance::{self, Origins};
use super::{DiscoveryError, SourceRoots};
use crate::config::annotated;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::path::{Path, PathBuf};

pub(super) const MARKER: &str = "x-imported-from:";
const DIGEST: &str = "sha256:";

/// One edit of the target document.
pub(super) enum Change {
    Insert(Vec<String>, Value),
    Replace(Vec<String>, Value),
}

fn error(path: &Path, reason: &'static str) -> DiscoveryError {
    DiscoveryError {
        path: path.into(),
        reason,
    }
}

/// The digest an annotation records for an imported value.
pub(super) fn digest(value: &Value) -> String {
    let hash = Sha256::digest(value.to_string().as_bytes());
    hash[..8].iter().map(|b| format!("{b:02x}")).collect()
}

/// Pointer → recorded digest of every annotated entry of the target document.
pub(super) fn imported_digests(
    path: &Path,
    text: Option<&str>,
) -> Result<BTreeMap<String, String>, DiscoveryError> {
    let Some(text) = text else {
        return Ok(BTreeMap::new());
    };
    let annotations = annotated::annotations(text, MARKER)
        .map_err(|_| error(path, "native configuration annotations are unreadable"))?;
    Ok(annotations
        .into_iter()
        .filter_map(|a| {
            let digest = a.note.rsplit_once(DIGEST)?.1.trim().to_owned();
            Some((a.pointer, digest))
        })
        .collect())
}

/// The edits turning `existing` into `proposed`: absent keys are inserted, and the
/// `updated` entries (annotated entries of an earlier import) replaced whole. Any other
/// difference is refused.
pub(super) fn changes(
    target: &Path,
    existing: &Value,
    proposed: &Value,
    updated: &BTreeSet<String>,
) -> Result<Vec<Change>, DiscoveryError> {
    let mut out = Vec::new();
    let mut walk = Walk {
        updated,
        path: Vec::new(),
        out: &mut out,
    };
    if !walk.collect(existing, proposed) {
        return Err(error(target, "proposal changes an existing native key"));
    }
    Ok(out)
}

struct Walk<'a> {
    updated: &'a BTreeSet<String>,
    path: Vec<String>,
    out: &'a mut Vec<Change>,
}

impl Walk<'_> {
    /// `false` when a value differs outside an updated entry.
    fn collect(&mut self, existing: &Value, proposed: &Value) -> bool {
        for (key, value) in proposed.as_object().into_iter().flatten() {
            self.path.push(key.clone());
            let consistent = match existing.get(key) {
                None => {
                    self.out
                        .push(Change::Insert(self.path.clone(), value.clone()));
                    true
                }
                Some(old) if old == value => true,
                Some(_) if self.updated.contains(&pointer(&self.path)) => {
                    self.out
                        .push(Change::Replace(self.path.clone(), value.clone()));
                    true
                }
                Some(old) => old.is_object() && value.is_object() && self.collect(old, value),
            };
            self.path.pop();
            if !consistent {
                return false;
            }
        }
        true
    }
}

fn pointer(path: &[String]) -> String {
    path.iter()
        .fold(String::new(), |prefix, key| provenance::child(&prefix, key))
}

/// The empty document an import creates.
pub(super) fn empty_document() -> String {
    format!("{{\n  \"$schema\": \"{}\"\n}}\n", crate::config::SCHEMA_URL)
}

/// Where the annotation names a source: relative to the project root or `~`.
fn label(roots: &SourceRoots, path: &Path) -> String {
    let shown = if let Ok(rest) = path.strip_prefix(&roots.project_root) {
        rest.display().to_string()
    } else if let Ok(rest) = path.strip_prefix(&roots.home) {
        format!("~/{}", rest.display())
    } else {
        path.display().to_string()
    };
    shown.chars().flat_map(char::escape_debug).collect()
}

pub(super) struct Annotator<'a> {
    pub roots: &'a SourceRoots,
    pub origins: &'a Origins,
    pub secrets: &'a [String],
}

impl Annotator<'_> {
    fn note(&self, pointer: &str, value: &Value) -> String {
        let mut sources: Vec<_> = provenance::references(self.origins, pointer)
            .iter()
            .map(|r| label(self.roots, &r.source))
            .collect();
        sources.dedup();
        let mut note = sources.join("; ");
        for secret in self.secrets {
            note = note.replace(secret.as_str(), "***");
        }
        format!("{note} {DIGEST}{}", digest(value))
    }
}

/// The target document with `changes` applied, verified to parse to exactly the expected
/// value; `None` when nothing changes.
pub(super) fn edit_target(
    target: &Path,
    original: Option<&str>,
    changes: &[Change],
    annotator: &Annotator<'_>,
) -> Result<Option<String>, DiscoveryError> {
    if changes.is_empty() {
        return Ok(None);
    }
    let refuse = || error(target, "native configuration cannot be edited in place");
    let base = original.map_or_else(empty_document, str::to_owned);
    let mut expected = crate::config::parse_jsonc("import target", &base).map_err(|_| refuse())?;
    let mut text = base;
    // Members are inserted first in their object, so the last change goes in first.
    for change in changes.iter().rev() {
        text = match change {
            Change::Insert(path, value) => {
                annotated::insert(&text, path, value, MARKER, &|depth, inserted| {
                    annotator.note(&pointer(&path[..=depth]), inserted)
                })
            }
            Change::Replace(path, value) => annotated::replace(
                &text,
                path,
                value,
                MARKER,
                &annotator.note(&pointer(path), value),
            ),
        }
        .map_err(|_| refuse())?;
        set(&mut expected, change);
    }
    let parsed = crate::config::parse_jsonc("import target", &text).map_err(|_| refuse())?;
    if parsed != expected {
        return Err(refuse());
    }
    Ok(Some(text))
}

fn set(document: &mut Value, change: &Change) {
    let (Change::Insert(path, value) | Change::Replace(path, value)) = change;
    let Some((last, parents)) = path.split_last() else {
        return;
    };
    let mut object = document;
    for key in parents {
        if !object.get(key).is_some_and(Value::is_object) {
            object[key] = json!({});
        }
        object = &mut object[key];
    }
    object[last] = value.clone();
}

/// A unified diff of the target document, with every known secret redacted.
pub(super) fn file_diff(
    target: &Path,
    original: Option<&str>,
    edited: &str,
    secrets: &[String],
) -> String {
    let shown = target.display().to_string();
    let old_header = if original.is_some() {
        shown.clone()
    } else {
        "/dev/null".into()
    };
    let mut diff = similar::TextDiff::from_lines(original.unwrap_or(""), edited)
        .unified_diff()
        .header(&old_header, &shown)
        .to_string();
    let mut secrets: Vec<_> = secrets.iter().collect();
    secrets.sort_by_key(|s| std::cmp::Reverse(s.len()));
    for secret in secrets {
        if let Ok(encoded) = serde_json::to_string(secret) {
            diff = diff.replace(&encoded[1..encoded.len() - 1], "***");
        }
        diff = diff.replace(secret.as_str(), "***");
    }
    diff
}

/// Create `directory` when it is absent. An existing link or non-directory is refused
/// with `linked`, never followed.
fn ensure_directory(
    directory: &Path,
    linked: &'static str,
    unwritable: &'static str,
) -> Result<(), DiscoveryError> {
    match std::fs::symlink_metadata(directory) {
        Ok(meta) if super::discovery::is_link(&meta) || !meta.is_dir() => {
            Err(error(directory, linked))
        }
        Ok(_) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            std::fs::create_dir_all(directory).map_err(|_| error(directory, unwritable))
        }
        Err(_) => Err(error(directory, unwritable)),
    }
}

/// Replace `target` through a new sibling file, keeping its permissions. A missing
/// directory is created (a global import may be the first write of the global
/// configuration directory); a linked or non-regular target, or a linked directory, is
/// refused.
pub(super) fn replace_file(target: &Path, text: &str) -> Result<(), DiscoveryError> {
    const UNWRITABLE: &str = "native configuration could not be written";
    let unwritable = || error(target, UNWRITABLE);
    let existing = match std::fs::symlink_metadata(target) {
        Ok(meta) if super::discovery::is_link(&meta) || !meta.is_file() => {
            return Err(error(
                target,
                "linked native configuration binding is unsupported",
            ));
        }
        Ok(meta) => Some(meta.permissions()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(_) => return Err(unwritable()),
    };
    let parent = target.parent().ok_or_else(unwritable)?;
    ensure_directory(
        parent,
        "linked native configuration directory is unsupported",
        UNWRITABLE,
    )?;
    let name = target.file_name().ok_or_else(unwritable)?.to_string_lossy();
    let temporary = parent.join(format!(".{name}.import-{}.tmp", std::process::id()));
    let result = (|| {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
        drop(file);
        if let Some(permissions) = existing {
            std::fs::set_permissions(&temporary, permissions)?;
        }
        std::fs::rename(&temporary, target)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result.map_err(|_| unwritable())
}

/// Write `report` as a new `import-report-<timestamp>.md` in `directory`, which is created
/// when absent and must not be a link.
pub(super) fn write_report(directory: &Path, report: &str) -> Result<PathBuf, DiscoveryError> {
    const UNWRITABLE: &str = "import report could not be written";
    let unwritable = || error(directory, UNWRITABLE);
    ensure_directory(
        directory,
        "linked import report directory is unsupported",
        UNWRITABLE,
    )?;
    let stamp = chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
    for attempt in 0..100 {
        let name = if attempt == 0 {
            format!("import-report-{stamp}.md")
        } else {
            format!("import-report-{stamp}-{attempt}.md")
        };
        let path = directory.join(name);
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(mut file) => {
                file.write_all(report.as_bytes())
                    .and_then(|()| file.sync_all())
                    .map_err(|_| unwritable())?;
                return Ok(path);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(_) => return Err(unwritable()),
        }
    }
    Err(unwritable())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn written_documents_keep_bytes_and_reports_never_overwrite() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("cyber.jsonc");
        replace_file(&target, "{}\n").unwrap();
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "{}\n");
        let reports = dir.path().join(".cyber");
        let first = write_report(&reports, "one").unwrap();
        let second = write_report(&reports, "two").unwrap();
        assert_ne!(first, second);
        assert_eq!(std::fs::read_to_string(first).unwrap(), "one");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 2);
    }

    /// Regression: the first global import failed when the global configuration
    /// directory did not exist yet; a missing directory is now created.
    #[test]
    fn a_missing_target_directory_is_created() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("config/cyber/cyber.jsonc");
        replace_file(&target, "{}\n").unwrap();
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "{}\n");
        let file = dir.path().join("file");
        std::fs::write(&file, "").unwrap();
        assert!(replace_file(&file.join("cyber.jsonc"), "{}").is_err());
    }

    /// A link planted at the target or the report directory is refused, never followed.
    #[cfg(unix)]
    #[test]
    fn linked_targets_and_report_directories_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let outside = dir.path().join("outside");
        std::fs::create_dir(&outside).unwrap();
        std::fs::write(outside.join("victim"), "keep").unwrap();
        let target = dir.path().join("cyber.jsonc");
        std::os::unix::fs::symlink(outside.join("victim"), &target).unwrap();
        assert!(replace_file(&target, "{}").is_err());
        assert_eq!(
            std::fs::read_to_string(outside.join("victim")).unwrap(),
            "keep"
        );
        let reports = dir.path().join(".cyber");
        std::os::unix::fs::symlink(&outside, &reports).unwrap();
        assert!(write_report(&reports, "report").is_err());
        assert_eq!(std::fs::read_dir(&outside).unwrap().count(), 1);
    }

    #[test]
    fn changes_outside_updated_entries_are_refused() {
        let existing = json!({"model": "a/b", "mcp": {"x": {"command": "one"}}});
        let proposed = json!({"model": "a/b", "mcp": {"x": {"command": "two"}}, "mode": "plan"});
        let target = Path::new("cyber.jsonc");
        assert!(changes(target, &existing, &proposed, &BTreeSet::new()).is_err());
        let updated = BTreeSet::from(["/mcp/x".to_string()]);
        let found = changes(target, &existing, &proposed, &updated).unwrap();
        assert!(matches!(&found[0], Change::Replace(path, _) if path == &["mcp", "x"]));
        assert!(matches!(&found[1], Change::Insert(path, _) if path == &["mode"]));
    }
}
