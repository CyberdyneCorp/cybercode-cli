//! Fenced rule definitions and the exact bytes covered by workspace approval.
use crate::import::{SourceSnapshot, command_directory};
use globset::{GlobBuilder, GlobMatcher};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub const MAX_FILES: usize = 128;
pub const MAX_TOTAL_BYTES: usize = 4 * 1024 * 1024;
const MAX_ENTRIES: usize = 4096;
const MAX_HEADER: usize = 16 * 1024;

/// Physical checkout and Location incarnation retained across a file operation.
/// This is observation evidence, not permission to access a source or perform an effect.
pub struct RootBinding {
    declared: PathBuf,
    location: PathBuf,
    root: PathBuf,
    native: crate::import::DirectorySnapshot,
}
impl RootBinding {
    pub fn capture(location: &Path) -> Result<Self, String> {
        let declared = std::path::absolute(location).map_err(|_| "rule Location is invalid")?;
        let location = location
            .canonicalize()
            .map_err(|_| "rule Location is unavailable")?;
        let root = crate::config::project_root(&location);
        let native = crate::import::DirectorySnapshot::capture(&location)
            .map_err(|e| e.reason.to_owned())?;
        let binding = Self {
            declared,
            location,
            root,
            native,
        };
        binding.verify()?;
        Ok(binding)
    }
    pub fn root(&self) -> &Path {
        &self.root
    }
    pub fn verify(&self) -> Result<(), String> {
        if self.declared.canonicalize().ok().as_ref() != Some(&self.location) {
            return Err("declared rule Location no longer names the captured directory".into());
        }
        self.native.verify().map_err(|e| e.reason.to_owned())?;
        if crate::config::project_root(&self.location) != self.root {
            return Err("rule project root changed during the operation".into());
        }
        Ok(())
    }
}
impl std::fmt::Debug for RootBinding {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RootBinding")
            .field("root", &self.root)
            .finish_non_exhaustive()
    }
}

/// Metadata describing a successful read; these hashes are observations, never
/// authority to perform another operation or reopen a source after restart.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ReadEvidence {
    pub root: PathBuf,
    pub path: PathBuf,
    pub root_identity: String,
    pub file_identity: String,
}
pub struct ObservedRead {
    declared: PathBuf,
    physical: PathBuf,
    root: RootBinding,
    reader: crate::import::FileRead,
}
pub struct ReadReceipt {
    declared: PathBuf,
    root: RootBinding,
    native: crate::import::FileReadReceipt,
    evidence: ReadEvidence,
}
impl RootBinding {
    pub fn identity(&self) -> String {
        self.native
            .ancestor_identity(&self.root)
            .expect("project root is an ancestor")
    }
}
impl ObservedRead {
    pub fn open(location: &Path, path: &Path, authorized: &Path) -> Result<Self, String> {
        let root = RootBinding::capture(location)?;
        let declared = std::path::absolute(path).map_err(|_| "read path is invalid")?;
        let physical = path
            .canonicalize()
            .map_err(|_| "read target is unavailable")?;
        if physical != authorized {
            return Err("read path changed after authorization".into());
        }
        if !physical.starts_with(root.root()) {
            return Err("read is outside the captured root".into());
        }
        // Receipts are persisted as exact paths; do not mint lossy identities.
        if physical.to_str().is_none() || root.root().to_str().is_none() {
            return Err("read receipt paths must be UTF-8".into());
        }
        let reader = crate::import::FileRead::open(&physical).map_err(|e| e.reason.to_owned())?;
        root.verify()?;
        Ok(Self {
            declared,
            physical,
            root,
            reader,
        })
    }
    pub fn read(self) -> std::io::Result<(Vec<u8>, ReadReceipt)> {
        let (bytes, native) = self.reader.read()?;
        let evidence = ReadEvidence {
            root: self.root.root().into(),
            path: self.physical,
            root_identity: self.root.identity(),
            file_identity: native.identity(),
        };
        Ok((
            bytes,
            ReadReceipt {
                declared: self.declared,
                root: self.root,
                native,
                evidence,
            },
        ))
    }
}
impl ReadReceipt {
    pub fn evidence(&self) -> &ReadEvidence {
        &self.evidence
    }
    pub fn verify(&self) -> Result<(), String> {
        self.root.verify()?;
        if self.declared.canonicalize().ok().as_ref() != Some(&self.evidence.path) {
            return Err("declared read target changed since the operation".into());
        }
        self.native.verify().map_err(|e| e.reason.to_owned())
    }
}
impl std::fmt::Debug for ReadReceipt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ReadReceipt")
            .field("evidence", &self.evidence)
            .finish_non_exhaustive()
    }
}

pub struct Definition {
    pub path: PathBuf,
    pub digest: String,
    pub body: String,
    pub paths: Option<Vec<String>>,
    matchers: Vec<GlobMatcher>,
    snapshot: SourceSnapshot,
}

impl Definition {
    pub fn read(path: &Path) -> Result<Self, String> {
        let snapshot = SourceSnapshot::read_command(path).map_err(|e| e.reason.to_owned())?;
        let (frontmatter, body) = parse(snapshot.text().map_err(|e| e.reason.to_owned())?)?;
        let matchers = frontmatter.paths.as_ref().map_or(Ok(Vec::new()), |paths| {
            paths
                .iter()
                .map(|pattern| glob(pattern))
                .collect::<Result<Vec<_>, _>>()
        })?;
        let definition = Self {
            path: path.into(),
            digest: digest(snapshot.bytes()),
            body: body.to_owned(),
            paths: frontmatter.paths,
            matchers,
            snapshot,
        };
        definition.verify()?;
        Ok(definition)
    }

    pub fn verify(&self) -> Result<(), String> {
        self.snapshot.verify().map_err(|e| e.reason.to_owned())
    }

    pub fn byte_len(&self) -> usize {
        self.snapshot.bytes().len()
    }

    pub fn applies(&self, root: &Path, target: Option<&Path>) -> bool {
        if self.paths.is_none() {
            return true;
        }
        let Some(relative) = target
            .and_then(|target| target.strip_prefix(root).ok())
            .and_then(Path::to_str)
        else {
            return false;
        };
        let relative = relative.replace('\\', "/");
        self.matchers
            .iter()
            .any(|matcher| matcher.is_match(&relative))
    }
}

fn digest(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct Frontmatter {
    #[serde(default, deserialize_with = "paths")]
    paths: Option<Vec<String>>,
    #[serde(default)]
    description: Option<String>,
}

fn paths<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<Vec<String>>, D::Error> {
    Vec::<String>::deserialize(deserializer).map(Some)
}

fn parse(text: &str) -> Result<(Frontmatter, &str), String> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let Some(rest) = text
        .strip_prefix("---\n")
        .or_else(|| text.strip_prefix("---\r\n"))
    else {
        return Ok((Frontmatter::default(), text));
    };
    let mut offset = 0;
    for line in rest.split_inclusive('\n') {
        if offset > MAX_HEADER {
            return Err("rule frontmatter exceeds 16 KiB".into());
        }
        if line.trim_end_matches(['\r', '\n']) == "---" {
            let header = &rest[..offset];
            let fm = if header.trim().is_empty() {
                Frontmatter::default()
            } else {
                let value: serde_yaml_ng::Value = serde_yaml_ng::from_str(header)
                    .map_err(|e| format!("invalid rule frontmatter: {e}"))?;
                if !value.is_mapping() {
                    return Err("rule frontmatter must be a mapping".into());
                }
                serde_yaml_ng::from_value::<Frontmatter>(value)
                    .map_err(|e| format!("invalid rule frontmatter: {e}"))?
            };
            if fm
                .description
                .as_ref()
                .is_some_and(|text| text.len() > 1024)
            {
                return Err("rule description exceeds 1024 bytes".into());
            }
            if fm
                .paths
                .as_ref()
                .is_some_and(|paths| paths.is_empty() || paths.len() > 64)
            {
                return Err("rule paths must contain 1-64 project-relative globs".into());
            }
            return Ok((fm, &rest[offset + line.len()..]));
        }
        offset += line.len();
    }
    Err("rule frontmatter has no closing delimiter within 16 KiB".into())
}

fn glob(pattern: &str) -> Result<GlobMatcher, String> {
    if pattern.len() > 256 {
        return Err("rule paths must be bounded project-relative globs".into());
    }
    let pattern = pattern.strip_prefix("./").unwrap_or(pattern);
    if pattern.is_empty()
        || pattern.len() > 256
        || pattern.starts_with('/')
        || pattern.contains(['\\', ':'])
        || pattern.chars().any(char::is_control)
        || pattern.split('/').any(|part| part == ".." || part == ".")
    {
        return Err("rule paths must be bounded project-relative globs".into());
    }
    GlobBuilder::new(pattern)
        .literal_separator(true)
        .build()
        .map(|glob| glob.compile_matcher())
        .map_err(|e| format!("invalid rule glob: {e}"))
}

/// Direct Markdown children only; no linked directory chain or unbounded enumeration.
pub fn candidates(directory: &Path) -> Result<Vec<PathBuf>, String> {
    match std::fs::symlink_metadata(directory) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e.to_string()),
        Ok(_) => (),
    }
    let chain = command_directory(directory).map_err(|e| e.reason.to_owned())?;
    let entries = chain.last().unwrap().entries().map_err(|e| e.to_string())?;
    let mut paths = Vec::new();
    for (index, entry) in entries.enumerate() {
        if index >= MAX_ENTRIES {
            return Err("rule directory entry bound exceeded".into());
        }
        let entry = entry.map_err(|e| e.to_string())?;
        let path = directory.join(entry.file_name());
        if path.extension().is_some_and(|ext| ext == "md") {
            if path.to_str().is_none() {
                return Err("rule paths must be UTF-8".into());
            }
            paths.push(path);
        }
        if paths.len() > MAX_FILES {
            return Err("rule file bound exceeded".into());
        }
    }
    paths.sort();
    Ok(paths)
}

pub fn project_directories(location: &Path) -> Vec<PathBuf> {
    let location = location.canonicalize().unwrap_or_else(|_| location.into());
    let root = crate::config::project_root(&location);
    let mut dirs: Vec<_> = location
        .ancestors()
        .take_while(|dir| dir.starts_with(&root))
        .map(Path::to_path_buf)
        .collect();
    dirs.reverse();
    dirs.into_iter()
        .flat_map(|dir| [dir.join(".claude/rules"), dir.join(".cyber/rules")])
        .collect()
}

pub struct TrustInventory {
    pub definitions: Vec<(String, PathBuf, String)>,
    pub digests: BTreeMap<PathBuf, String>,
}

/// Invalid/unknown inventories have no authority. Their marker still invalidates an
/// earlier approval; later repair requires review of the now-readable bytes.
pub fn trust_subset(location: &Path) -> TrustInventory {
    let root = crate::config::project_root(location);
    let root = root.canonicalize().unwrap_or(root);
    let mut subset = Vec::new();
    let mut digests = BTreeMap::new();
    let mut total = 0usize;
    let mut count = 0;
    for directory in project_directories(location) {
        let files = match candidates(&directory) {
            Ok(files) => files,
            Err(reason) => {
                subset.push((
                    directory.display().to_string(),
                    directory,
                    format!("refused:{reason}"),
                ));
                continue;
            }
        };
        for path in files {
            count += 1;
            if count > MAX_FILES {
                return TrustInventory {
                    definitions: vec![(
                        "rules:unavailable".into(),
                        root,
                        "rule inventory bound exceeded".into(),
                    )],
                    digests: BTreeMap::new(),
                };
            }
            let value = match SourceSnapshot::read_command(&path) {
                Ok(snapshot) => {
                    total = total.saturating_add(snapshot.bytes().len());
                    if count > MAX_FILES || total > MAX_TOTAL_BYTES {
                        return TrustInventory {
                            definitions: vec![(
                                "rules:unavailable".into(),
                                root,
                                "rule inventory bound exceeded".into(),
                            )],
                            digests: BTreeMap::new(),
                        };
                    }
                    let value = digest(snapshot.bytes());
                    digests.insert(path.clone(), value.clone());
                    value
                }
                Err(error) => format!("refused:{}", error.reason),
            };
            subset.push((
                path.strip_prefix(&root)
                    .unwrap_or(&path)
                    .display()
                    .to_string(),
                path,
                value,
            ));
        }
    }
    TrustInventory {
        definitions: subset,
        digests,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn root_binding_tracks_native_ancestry_and_project_boundaries() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("checkout");
        let location = root.join("web/src");
        std::fs::create_dir_all(&location).unwrap();
        std::fs::create_dir(root.join(".git")).unwrap();
        let binding = RootBinding::capture(&location).unwrap();
        assert_eq!(binding.root(), root.canonicalize().unwrap());
        assert_eq!(
            binding.identity(),
            RootBinding::capture(&root).unwrap().identity()
        );
        std::fs::write(location.join("app.ts"), "ordinary edit").unwrap();
        assert!(binding.verify().is_ok());
        // A nested checkout changes scope even though the physical directories remain.
        std::fs::create_dir(root.join("web/.git")).unwrap();
        assert!(binding.verify().is_err());
        std::fs::remove_dir(root.join("web/.git")).unwrap();
        assert!(binding.verify().is_ok());
        std::fs::rename(root.join("web"), root.join("old-web")).unwrap();
        std::fs::create_dir_all(&location).unwrap();
        assert!(
            binding.verify().is_err(),
            "same path is a different ancestor"
        );
        assert!(RootBinding::capture(&location).unwrap().verify().is_ok());
    }

    #[test]
    fn root_binding_rejects_replaced_and_removed_checkouts() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("checkout");
        std::fs::create_dir(&root).unwrap();
        let binding = RootBinding::capture(&root).unwrap();
        std::fs::rename(&root, temp.path().join("retained")).unwrap();
        assert!(binding.verify().is_err());
        std::fs::create_dir(&root).unwrap();
        assert!(
            binding.verify().is_err(),
            "replacement must not borrow held identity"
        );
        assert!(RootBinding::capture(&root).unwrap().verify().is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn root_binding_accepts_a_declared_alias_but_refuses_later_link_substitution() {
        use std::os::unix::fs::symlink;
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("checkout");
        std::fs::create_dir(&root).unwrap();
        let alias = temp.path().join("alias");
        symlink(&root, &alias).unwrap();
        let binding = RootBinding::capture(&alias).unwrap();
        assert!(binding.verify().is_ok());
        let other = temp.path().join("other");
        std::fs::create_dir(&other).unwrap();
        std::fs::remove_file(&alias).unwrap();
        symlink(&other, &alias).unwrap();
        assert!(
            binding.verify().is_err(),
            "declared alias retargeted to another root"
        );
        std::fs::remove_file(&alias).unwrap();
        symlink(&root, &alias).unwrap();
        assert!(binding.verify().is_ok());
        let retained = temp.path().join("retained");
        std::fs::rename(&root, &retained).unwrap();
        symlink(&retained, &root).unwrap();
        assert!(
            binding.verify().is_err(),
            "native lookup must not follow a substituted link"
        );
    }

    #[test]
    fn actual_reads_retain_file_and_root_identity_without_the_definition_size_cap() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("repo");
        std::fs::create_dir(&root).unwrap();
        let path = root.join("large.txt");
        let content = vec![b'x'; 1024 * 1024 + 1];
        std::fs::write(&path, &content).unwrap();
        let reader = ObservedRead::open(&root, &path, &path.canonicalize().unwrap()).unwrap();
        let (bytes, receipt) = reader.read().unwrap();
        assert_eq!(bytes, content);
        assert!(receipt.verify().is_ok());
        assert_eq!(receipt.evidence().path, path.canonicalize().unwrap());
        assert_eq!(
            receipt.evidence().root_identity,
            RootBinding::capture(&root).unwrap().identity()
        );
        // Ordinary writes to the same inode do not erase the earlier read operation.
        std::fs::write(&path, "later content").unwrap();
        assert!(receipt.verify().is_ok());
        std::fs::rename(&path, root.join("retained.txt")).unwrap();
        std::fs::write(&path, &content).unwrap();
        assert!(
            receipt.verify().is_err(),
            "a new inode cannot borrow the actual read"
        );
    }

    #[test]
    fn workspace_hard_links_are_observable_but_definition_hard_links_are_refused() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("file.md");
        let link = temp.path().join("link.md");
        std::fs::write(&path, "workspace content").unwrap();
        std::fs::hard_link(&path, &link).unwrap();
        let (bytes, receipt) =
            ObservedRead::open(temp.path(), &link, &link.canonicalize().unwrap())
                .unwrap()
                .read()
                .unwrap();
        assert_eq!(bytes, b"workspace content");
        assert!(receipt.verify().is_ok());
        assert!(Definition::read(&link).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn observed_read_uses_its_open_handle_and_refuses_retargeted_aliases() {
        use std::os::unix::fs::symlink;
        let temp = tempfile::tempdir().unwrap();
        let a = temp.path().join("a.txt");
        let b = temp.path().join("b.txt");
        let alias = temp.path().join("alias.txt");
        std::fs::write(&a, "actual original bytes").unwrap();
        std::fs::write(&b, "different bytes").unwrap();
        symlink(&a, &alias).unwrap();
        let reader = ObservedRead::open(temp.path(), &alias, &a.canonicalize().unwrap()).unwrap();
        std::fs::remove_file(&alias).unwrap();
        symlink(&b, &alias).unwrap();
        let (bytes, receipt) = reader.read().unwrap();
        assert_eq!(bytes, b"actual original bytes");
        assert!(receipt.verify().is_err());
        assert!(ObservedRead::open(temp.path(), &alias, &a.canonicalize().unwrap()).is_err());
    }

    fn root() -> (tempfile::TempDir, PathBuf) {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        std::fs::create_dir(root.join(".git")).unwrap();
        std::fs::create_dir_all(root.join(".cyber/rules")).unwrap();
        (temp, root)
    }
    #[test]
    fn scopes_are_relative_to_the_project_and_unscoped_rules_are_baseline() {
        let (_temp, root) = root();
        let path = root.join(".cyber/rules/migrations.md");
        std::fs::write(
            &path,
            "---\npaths: [\"./db/migrations/**\"]\n---\nUse transactions.",
        )
        .unwrap();
        let rule = Definition::read(&path).unwrap();
        assert_eq!(rule.body, "Use transactions.");
        assert!(!rule.applies(&root, None));
        assert!(rule.applies(&root, Some(&root.join("db/migrations/0042.sql"))));
        assert!(!rule.applies(&root, Some(&root.join("db/other.sql"))));
        assert!(!rule.applies(
            &root,
            Some(&root.parent().unwrap().join("db/migrations/0042.sql"))
        ));
        std::fs::write(&path, "plain Markdown").unwrap();
        assert!(Definition::read(&path).unwrap().applies(&root, None));
        assert!(
            rule.verify().is_err(),
            "captured definition cannot silently change"
        );
    }
    #[test]
    fn invalid_frontmatter_never_falls_back_to_unscoped_guidance() {
        let (_temp, root) = root();
        let path = root.join(".cyber/rules/a.md");
        for header in [
            "[]",
            "paths: null",
            "paths: []",
            "paths: [../secrets/**]",
            "paths: ['/absolute/**']",
            "paths: ['C:/private/**']",
            "paths: ['[']",
            "paths: ['a\\b']",
            "permissions: allow",
            "paths: [3]",
        ] {
            std::fs::write(&path, format!("---\n{header}\n---\nNever baseline")).unwrap();
            assert!(Definition::read(&path).is_err(), "{header}");
        }
        std::fs::write(
            &path,
            format!(
                "---\ndescription: {}\n---\nbody",
                "a".repeat(MAX_HEADER + 1)
            ),
        )
        .unwrap();
        assert!(Definition::read(&path).is_err());
    }
    #[test]
    fn trust_inventory_tracks_full_definition_bytes_and_bounds_the_directory() {
        let (_temp, root) = root();
        let path = root.join(".cyber/rules/a.md");
        std::fs::write(&path, "---\npaths: ['a/**']\n---\nbody").unwrap();
        let first = trust_subset(&root);
        assert_eq!(
            first.digests[&path],
            Definition::read(&path).unwrap().digest
        );
        std::fs::write(&path, "---\npaths: ['b/**']\n---\nbody").unwrap();
        assert_ne!(first.definitions, trust_subset(&root).definitions);
        for i in 0..MAX_FILES {
            std::fs::write(root.join(format!(".cyber/rules/{i}.md")), "rule").unwrap();
        }
        assert!(candidates(&root.join(".cyber/rules")).is_err());
        assert!(trust_subset(&root).digests.is_empty());
    }
    #[cfg(unix)]
    #[test]
    fn linked_definitions_and_directories_have_no_rule_authority() {
        use std::os::unix::fs::symlink;
        let (_temp, root) = root();
        let source = root.join("source.md");
        std::fs::write(&source, "private").unwrap();
        let linked = root.join(".cyber/rules/link.md");
        symlink(&source, &linked).unwrap();
        assert!(Definition::read(&linked).is_err());
        assert!(!trust_subset(&root).digests.contains_key(&linked));
        std::fs::remove_file(&linked).unwrap();
        std::fs::hard_link(&source, &linked).unwrap();
        assert!(Definition::read(&linked).is_err());
        symlink(root.join(".cyber/rules"), root.join("alias")).unwrap();
        assert!(candidates(&root.join("alias")).is_err());
    }
}
