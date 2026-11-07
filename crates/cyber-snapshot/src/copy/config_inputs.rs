//! Read-only configuration observations; never source or acknowledgement authority.
use super::{ownership, storage};
use std::collections::BTreeMap;
use std::io::Read;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

const FILE_LIMIT: u64 = 1024 * 1024;
const TOTAL_LIMIT: usize = 8 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub(crate) struct ConfigInputs {
    environment: Option<String>,
    files: BTreeMap<PathBuf, Option<FileState>>,
    contexts: BTreeMap<PathBuf, DirectoryState>,
    repository: Option<PathBuf>,
}
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
struct DirectoryState {
    canonical: PathBuf,
    identity: String,
}
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
struct FileState {
    canonical: PathBuf,
    parent_identity: String,
    generation: String,
    hash: String,
}

impl ConfigInputs {
    pub(crate) fn capture(paths: Vec<PathBuf>, environment: String) -> storage::Result<Self> {
        if environment_digest()? != environment {
            return Err("Git configuration environment changed".into());
        }
        let mut proof = Self::capture_files(paths)?;
        if environment_digest()? != environment {
            return Err("Git configuration environment changed".into());
        }
        proof.environment = Some(environment);
        Ok(proof)
    }

    /// Generated source bytes have no environment-dependent interpretation.
    pub(crate) fn capture_files(paths: Vec<PathBuf>) -> storage::Result<Self> {
        let mut files = BTreeMap::new();
        let mut bytes = 0;
        for path in paths {
            if !path.is_absolute() || path.as_os_str().len() > 8192 || files.len() >= 1024 {
                return Err("Git configuration input paths exceed limits".into());
            }
            if files.contains_key(&path) {
                continue;
            }
            let (state, contents) = read(&path)?;
            bytes += contents.len() + path.as_os_str().len();
            if bytes > TOTAL_LIMIT {
                return Err("Git configuration inputs exceed limit".into());
            }
            files.insert(path, state);
        }
        Ok(Self {
            environment: None,
            files,
            contexts: BTreeMap::new(),
            repository: None,
        })
    }

    /// Reopen bytes only when they still match this observation.
    pub(crate) fn bytes(&self, path: &Path) -> storage::Result<Vec<u8>> {
        let expected = self.files.get(path).ok_or("Unbound Git policy input")?;
        let (state, bytes) = read(path)?;
        if &state != expected {
            return Err("Git policy input changed while reading".into());
        }
        Ok(bytes)
    }

    pub(crate) fn fingerprint(&self) -> storage::Result<String> {
        Ok(storage::digest(
            &serde_json::to_vec(self).map_err(storage::fail)?,
        ))
    }

    pub(crate) fn merge(&mut self, other: Self) -> storage::Result<()> {
        if other.environment.as_ref().is_some_and(|environment| {
            self.environment
                .as_ref()
                .is_some_and(|old| old != environment)
        }) || other
            .repository
            .as_ref()
            .is_some_and(|path| self.repository.as_ref() != Some(path))
        {
            return Err("Git policy contexts disagree".into());
        }
        self.environment = self.environment.clone().or(other.environment);
        for (path, state) in other.files {
            if self.files.get(&path).is_some_and(|old| old != &state) {
                return Err("Git policy observations disagree".into());
            }
            self.files.insert(path, state);
        }
        for (path, state) in other.contexts {
            if self.contexts.get(&path).is_some_and(|old| old != &state) {
                return Err("Git policy directory observations disagree".into());
            }
            self.contexts.insert(path, state);
        }
        if self.files.len() > 1024 || self.contexts.len() > 8 {
            return Err("Combined Git policy inputs exceed limits".into());
        }
        self.verify()
    }

    pub(crate) fn bind_repository(&mut self, path: PathBuf) -> storage::Result<()> {
        private_repository(&path)?;
        self.repository = Some(path);
        Ok(())
    }

    pub(crate) fn bind_contexts(&mut self, paths: Vec<PathBuf>) -> storage::Result<()> {
        if paths.len() > 8 {
            return Err("Git configuration directory context exceeds limit".into());
        }
        for path in paths {
            let canonical = path.canonicalize().map_err(storage::fail)?;
            let directory = cap_std::fs::Dir::open_ambient_dir(&path, cap_std::ambient_authority())
                .map_err(storage::fail)?;
            let identity = storage::dir_identity(&directory)?;
            let fresh = cap_std::fs::Dir::open_ambient_dir(&path, cap_std::ambient_authority())
                .map_err(storage::fail)?;
            if path.canonicalize().map_err(storage::fail)? != canonical
                || storage::dir_identity(&fresh)? != identity
            {
                return Err("Git configuration directory context changed".into());
            }
            self.contexts.insert(
                path,
                DirectoryState {
                    canonical,
                    identity,
                },
            );
        }
        Ok(())
    }

    pub(crate) fn retains(&self, roots: &Self) -> bool {
        self.environment == roots.environment
            && self.repository == roots.repository
            && self.contexts == roots.contexts
            && roots
                .files
                .iter()
                .all(|(path, state)| self.files.get(path) == Some(state))
    }

    pub(crate) fn verify(&self) -> storage::Result<()> {
        let paths = self.files.keys().cloned().collect();
        let mut current = match &self.environment {
            Some(environment) => Self::capture(paths, environment.clone())?,
            None => Self::capture_files(paths)?,
        };
        current.bind_contexts(self.contexts.keys().cloned().collect())?;
        if let Some(repository) = &self.repository {
            current.bind_repository(repository.clone())?;
        }
        if current != *self {
            return Err("Git configuration inputs changed".into());
        }
        Ok(())
    }
}

fn open(path: &Path) -> std::io::Result<std::fs::File> {
    // Git configuration follows file links. This grants only read-only policy
    // proof; native source image reads keep their separate no-follow constraints.
    std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(path)
}

fn read(path: &Path) -> storage::Result<(Option<FileState>, Vec<u8>)> {
    let mut file = match open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok((None, vec![])),
        Err(_) => return Err("Cannot read Git configuration input".into()),
    };
    let metadata = file.metadata().map_err(storage::fail)?;
    if !metadata.is_file() || metadata.len() > FILE_LIMIT {
        return Err("Unsupported or over-limit Git configuration input".into());
    }
    let generation = ownership::generation(&file)?;
    let canonical = path.canonicalize().map_err(storage::fail)?;
    let parent = cap_std::fs::Dir::open_ambient_dir(
        canonical.parent().ok_or("Missing config parent")?,
        cap_std::ambient_authority(),
    )
    .map_err(storage::fail)?;
    let parent_identity = storage::dir_identity(&parent)?;
    let mut bytes = Vec::new();
    Read::by_ref(&mut file)
        .take(FILE_LIMIT + 1)
        .read_to_end(&mut bytes)
        .map_err(storage::fail)?;
    if bytes.len() as u64 != metadata.len()
        || ownership::generation(&file)? != generation
        || ownership::generation(&open(path).map_err(storage::fail)?)? != generation
        || path.canonicalize().map_err(storage::fail)? != canonical
        || storage::dir_identity(
            &cap_std::fs::Dir::open_ambient_dir(
                canonical.parent().ok_or("Missing config parent")?,
                cap_std::ambient_authority(),
            )
            .map_err(storage::fail)?,
        )? != parent_identity
    {
        return Err("Git configuration changed while reading".into());
    }
    Ok((
        Some(FileState {
            canonical,
            parent_identity,
            generation,
            hash: storage::digest(&bytes),
        }),
        bytes,
    ))
}

fn private_repository(path: &Path) -> storage::Result<()> {
    for name in ["commondir", "gitdir", "reftable"] {
        match std::fs::symlink_metadata(path.join(name)) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            _ => return Err("Unsupported private Git reference layout".into()),
        }
    }
    let head = path.join("HEAD");
    if std::fs::symlink_metadata(&head)
        .map_err(storage::fail)?
        .file_type()
        .is_symlink()
    {
        return Err("Linked private Git HEAD lacks native policy proof".into());
    }
    let (_, bytes) = read(&head)?;
    let head = std::str::from_utf8(&bytes).map_err(|_| "Invalid private Git HEAD")?;
    let target = head
        .trim_end_matches('\n')
        .strip_prefix("ref: ")
        .ok_or("Unsupported private Git HEAD")?;
    if !target.starts_with("refs/heads/") || !reference_name(target) {
        return Err("Unsupported private Git HEAD reference".into());
    }
    if std::fs::read_dir(path.join("refs/heads"))
        .map_err(storage::fail)?
        .next()
        .is_some()
    {
        return Err("Private Git branch references lack native policy proof".into());
    }
    let (_, packed) = read(&path.join("packed-refs"))?;
    packed_references(&packed)
}

fn packed_references(packed: &[u8]) -> storage::Result<()> {
    for line in packed
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty() && !line.starts_with(b"#"))
    {
        let line = std::str::from_utf8(line).map_err(|_| "Invalid packed Git references")?;
        if let Some(peeled) = line.strip_prefix('^') {
            if !object_id(peeled) {
                return Err("Invalid peeled Git reference".into());
            }
            continue;
        }
        let (object, name) = line.split_once(' ').ok_or("Invalid packed Git reference")?;
        if !object_id(object) || !reference_name(name) || name.starts_with("refs/heads/") {
            return Err("Unsupported packed private Git reference".into());
        }
    }
    Ok(())
}

fn object_id(value: &str) -> bool {
    matches!(value.len(), 40 | 64) && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}
fn reference_name(value: &str) -> bool {
    value.starts_with("refs/")
        && !["..", "@{", "//"].iter().any(|part| value.contains(part))
        && !value
            .bytes()
            .any(|byte| byte <= b' ' || byte == 127 || b"~^:?*[\\".contains(&byte))
        && value.split('/').all(|part| {
            !part.is_empty()
                && !part.starts_with('.')
                && !part.ends_with('.')
                && !part.ends_with(".lock")
        })
}

pub(crate) fn environment_digest() -> storage::Result<String> {
    let mut values = BTreeMap::new();
    let mut bytes = 0;
    for (key, value) in std::env::vars_os() {
        let key = key.to_str().ok_or("Non-UTF-8 environment key")?;
        if !(key.starts_with("GIT_")
            || key.starts_with("LC_")
            || matches!(key, "HOME" | "XDG_CONFIG_HOME" | "PATH" | "LANG"))
            || crate::git::GIT_ENV.contains(&key)
        {
            continue;
        }
        let value = value
            .to_str()
            .ok_or("Non-UTF-8 Git configuration environment")?;
        bytes += key.len() + value.len();
        if values.len() >= 1024 || bytes > 256 * 1024 || value.contains(['\n', '\r']) {
            return Err("Unsupported or over-limit Git configuration environment".into());
        }
        values.insert(key.to_owned(), value.to_owned());
    }
    // Only a digest enters capture fingerprints; configuration secrets stay out
    // of the handoff, checkpoint tags and Debug output.
    Ok(storage::digest(
        &serde_json::to_vec(&("git-config-environment-v1", values)).map_err(storage::fail)?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_state_binds_missing_candidates_replacements_and_limits() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("config");
        let missing =
            ConfigInputs::capture(vec![path.clone()], environment_digest().unwrap()).unwrap();
        missing.verify().unwrap();
        std::fs::write(&path, "same contents").unwrap();
        assert!(missing.verify().is_err());
        let observed =
            ConfigInputs::capture(vec![path.clone()], environment_digest().unwrap()).unwrap();
        observed.verify().unwrap();
        let replacement = temp.path().join("replacement");
        std::fs::write(&replacement, "same contents").unwrap();
        std::fs::rename(replacement, &path).unwrap();
        assert!(observed.verify().is_err());
        std::fs::write(&path, vec![0; FILE_LIMIT as usize + 1]).unwrap();
        assert!(ConfigInputs::capture(vec![path], environment_digest().unwrap()).is_err());
    }

    #[test]
    fn linked_config_remains_readable_but_same_byte_retargeting_is_stale() {
        let temp = tempfile::tempdir().unwrap();
        let first = temp.path().join("first");
        let second = temp.path().join("second");
        let route = temp.path().join("config");
        std::fs::write(&first, "same").unwrap();
        std::fs::write(&second, "same").unwrap();
        std::os::unix::fs::symlink(&first, &route).unwrap();
        let proof =
            ConfigInputs::capture(vec![route.clone()], environment_digest().unwrap()).unwrap();
        proof.verify().unwrap();
        std::fs::remove_file(&route).unwrap();
        std::os::unix::fs::symlink(&second, route).unwrap();
        assert!(proof.verify().is_err());
    }

    #[test]
    fn a_hardlinked_config_alias_still_binds_its_canonical_parent() {
        let temp = tempfile::tempdir().unwrap();
        let first = temp.path().join("first");
        let second = temp.path().join("second");
        std::fs::create_dir(&first).unwrap();
        std::fs::create_dir(&second).unwrap();
        std::fs::write(first.join("config"), "same").unwrap();
        std::fs::hard_link(first.join("config"), second.join("config")).unwrap();
        let route = temp.path().join("route");
        std::os::unix::fs::symlink(first.join("config"), &route).unwrap();
        let proof =
            ConfigInputs::capture(vec![route.clone()], environment_digest().unwrap()).unwrap();
        std::fs::remove_file(&route).unwrap();
        std::os::unix::fs::symlink(second.join("config"), route).unwrap();
        assert!(proof.verify().is_err());
    }

    #[test]
    fn private_reference_layout_allows_snapshot_packing_but_refuses_new_branch_or_admin_authority()
    {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        std::fs::create_dir_all(root.join("refs/heads")).unwrap();
        std::fs::write(root.join("HEAD"), "ref: refs/heads/main\n").unwrap();
        let mut proof = ConfigInputs::capture(vec![], environment_digest().unwrap()).unwrap();
        proof.bind_repository(root.clone()).unwrap();
        proof.verify().unwrap();
        let object = "a".repeat(40);
        std::fs::write(
            root.join("packed-refs"),
            format!("{object} refs/cyber/snapshots/{object}\n"),
        )
        .unwrap();
        proof.verify().unwrap();
        std::fs::write(root.join("refs/heads/main"), format!("{object}\n")).unwrap();
        assert!(proof.verify().is_err());
        std::fs::remove_file(root.join("refs/heads/main")).unwrap();
        std::fs::write(
            root.join("packed-refs"),
            format!("{object} refs/heads/main\n"),
        )
        .unwrap();
        assert!(proof.verify().is_err());
        std::fs::remove_file(root.join("packed-refs")).unwrap();
        std::fs::write(root.join("commondir"), "elsewhere\n").unwrap();
        assert!(proof.verify().is_err());
        std::fs::remove_file(root.join("commondir")).unwrap();
        std::fs::create_dir(root.join("reftable")).unwrap();
        assert!(proof.verify().is_err());
    }

    #[test]
    fn directory_context_binds_the_physical_conditional_include_route() {
        let temp = tempfile::tempdir().unwrap();
        let first = temp.path().join("first");
        let second = temp.path().join("second");
        let route = temp.path().join("git-dir");
        std::fs::create_dir(&first).unwrap();
        std::fs::create_dir(&second).unwrap();
        std::os::unix::fs::symlink(&first, &route).unwrap();
        let mut proof = ConfigInputs::capture(vec![], environment_digest().unwrap()).unwrap();
        proof.bind_contexts(vec![route.clone()]).unwrap();
        proof.verify().unwrap();
        std::fs::write(first.join("unrelated-object"), "object").unwrap();
        proof.verify().unwrap();
        std::fs::remove_file(&route).unwrap();
        std::os::unix::fs::symlink(&second, route).unwrap();
        assert!(proof.verify().is_err());
    }

    #[test]
    fn environment_is_bound_without_retaining_cleartext_values() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--ignored",
                "--exact",
                "copy::config_inputs::tests::environment_child",
            ])
            .env(
                "CYBER_CONFIG_PARENT_FINGERPRINT",
                environment_digest().unwrap(),
            )
            .env("GIT_CONFIG_COUNT", "1")
            .env("GIT_CONFIG_KEY_0", "fixture.private")
            .env("GIT_CONFIG_VALUE_0", "fixture-secret-value-unique")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    #[ignore = "subprocess fixture with isolated Git configuration environment"]
    fn environment_child() {
        let actual = environment_digest().unwrap();
        assert_ne!(
            actual,
            std::env::var("CYBER_CONFIG_PARENT_FINGERPRINT").unwrap()
        );
        let proof = ConfigInputs::capture(vec![], actual).unwrap();
        let encoded = serde_json::to_string(&proof).unwrap();
        assert!(!encoded.contains("fixture-secret-value-unique"));
        let mut changed = proof;
        changed.environment = Some("0".repeat(64));
        assert!(changed.verify().is_err());
    }
}
