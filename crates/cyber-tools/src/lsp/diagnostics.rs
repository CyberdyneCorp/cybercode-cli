use std::{
    collections::{BTreeMap, VecDeque},
    io::Read,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use super::documents::{Documents, MAX_DOCUMENT_BYTES};

const MAX_FILES: usize = 128;
const MAX_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct DiagnosticPosition {
    pub line: u32,
    pub character: u32,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct DiagnosticRange {
    pub start: DiagnosticPosition,
    pub end: DiagnosticPosition,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct Diagnostic {
    pub range: DiagnosticRange,
    pub severity: Option<u8>,
    pub message: String,
}

/// A validated publication, not proof that an unversioned server finished an edit.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct DiagnosticSnapshot {
    pub path: PathBuf,
    /// The version declared by the server; absence carries no version guarantee.
    pub version: Option<i32>,
    /// Client version observed at receipt, not an inferred version for an unversioned publication.
    pub document_version: Option<i32>,
    pub sequence: u64,
    pub diagnostics: Vec<Diagnostic>,
}

impl DiagnosticSnapshot {
    /// Formats only explicit errors. Server text cannot introduce new block framing.
    pub fn error_block(&self, root: &Path) -> Option<String> {
        let path = self.path.strip_prefix(root).ok()?;
        let errors: Vec<_> = self
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.severity == Some(1))
            .collect();
        if errors.is_empty() {
            return None;
        }
        let mut block = format!(
            "<diagnostics file=\"{}\">\n",
            escape(&path.to_string_lossy())
        );
        for error in errors.iter().take(20) {
            block.push_str(&format!(
                "ERROR [{}:{}] {}\n",
                u64::from(error.range.start.line) + 1,
                u64::from(error.range.start.character) + 1,
                escape(&error.message)
            ));
        }
        if errors.len() > 20 {
            block.push_str(&format!("… and {} more\n", errors.len() - 20));
        }
        block.push_str("</diagnostics>");
        Some(block)
    }
}

fn escape(text: &str) -> String {
    text.chars()
        .map(|character| match character {
            '<' => "&lt;".into(),
            '>' => "&gt;".into(),
            '&' => "&amp;".into(),
            '"' => "&quot;".into(),
            '\'' => "&apos;".into(),
            character if character.is_control() => " ".into(),
            character => character.to_string(),
        })
        .collect()
}

#[derive(Deserialize)]
pub(super) struct Publication {
    uri: String,
    pub version: Option<i32>,
    pub diagnostics: Vec<Diagnostic>,
    #[serde(skip)]
    pub path: PathBuf,
}

impl Publication {
    pub fn parse(message: &Value, root: &Path, documents: &Documents) -> Option<Self> {
        if message.get("method")?.as_str()? != "textDocument/publishDiagnostics" {
            return None;
        }
        let params = message.get("params")?;
        if params.get("version").is_some_and(Value::is_null) {
            return None;
        }
        let diagnostics = params.get("diagnostics")?.as_array()?;
        if serde_json::to_vec(params).ok()?.len() > MAX_BYTES
            || diagnostics
                .iter()
                .any(|diagnostic| diagnostic.get("severity").is_some_and(Value::is_null))
        {
            return None;
        }
        let mut publication: Self = serde_json::from_value(params.clone()).ok()?;
        let uri = reqwest::Url::parse(&publication.uri).ok()?;
        if uri.scheme() != "file"
            || uri.query().is_some()
            || uri.fragment().is_some()
            || !uri.username().is_empty()
            || uri.password().is_some()
            || uri.host_str().is_some_and(|host| host != "localhost")
        {
            return None;
        }
        publication.path = uri.to_file_path().ok()?;
        if !publication.path.starts_with(root) || !publication.valid(documents) {
            return None;
        }
        Some(publication)
    }

    fn valid(&self, documents: &Documents) -> bool {
        if self
            .version
            .zip(documents.version(&self.path))
            .is_some_and(|(version, current)| version != current)
        {
            return false;
        }
        self.diagnostics.iter().all(|diagnostic| {
            diagnostic
                .severity
                .is_none_or(|severity| (1..=4).contains(&severity))
                && diagnostic.range.start <= diagnostic.range.end
                && [&diagnostic.range.start, &diagnostic.range.end]
                    .iter()
                    .all(|position| {
                        position.line <= i32::MAX as u32 && position.character <= i32::MAX as u32
                    })
        })
    }
}

#[derive(Clone, PartialEq, Eq)]
pub(super) struct Observation {
    digest: [u8; 32],
    pub checkouts: Vec<cyber_core::worktrees::Managed>,
}

impl Observation {
    pub fn matches_text(&self, text: &str) -> bool {
        self.digest == <[u8; 32]>::from(Sha256::digest(text.as_bytes()))
    }
    pub fn matches(&self, path: &Path, documents: &Documents) -> bool {
        documents.matches_digest(path, &self.digest)
    }
}

pub(super) async fn observe(path: PathBuf) -> Option<Observation> {
    tokio::task::spawn_blocking(move || observe_file(&path))
        .await
        .ok()
        .flatten()
}

pub(crate) async fn document_source(path: PathBuf) -> Option<String> {
    tokio::task::spawn_blocking(move || observe_source(&path).map(|(_, text)| text))
        .await
        .ok()
        .flatten()
}

fn observe_file(path: &Path) -> Option<Observation> {
    observe_source(path).map(|(observation, _)| observation)
}

fn observe_source(path: &Path) -> Option<(Observation, String)> {
    if path.canonicalize().ok()?.as_path() != path {
        return None;
    }
    let checkouts = super::locations::checkout_records(path).ok()?;
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.custom_flags(0x00200000); // FILE_FLAG_OPEN_REPARSE_POINT
    }
    let file = options.open(path).ok()?;
    let metadata = file.metadata().ok()?;
    if !metadata.is_file()
        || metadata.file_type().is_symlink()
        || metadata.len() > MAX_DOCUMENT_BYTES as u64
    {
        return None;
    }
    let mut bytes = Vec::new();
    file.take(MAX_DOCUMENT_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() > MAX_DOCUMENT_BYTES
        || std::str::from_utf8(&bytes).is_err()
        || path.canonicalize().ok()?.as_path() != path
        || super::locations::checkout_records(path).ok()? != checkouts
    {
        return None;
    }
    let digest = Sha256::digest(&bytes).into();
    Some((
        Observation { digest, checkouts },
        String::from_utf8(bytes).ok()?,
    ))
}

struct Cached {
    snapshot: DiagnosticSnapshot,
    observation: Observation,
    bytes: usize,
}

#[derive(Default)]
pub(super) struct Diagnostics {
    entries: BTreeMap<PathBuf, Cached>,
    bytes: usize,
    sequence: u64,
    raw: VecDeque<(Value, usize)>,
    raw_bytes: usize,
}

impl Diagnostics {
    pub fn sequence(&self) -> u64 {
        self.sequence
    }

    pub fn baseline(&self) -> BTreeMap<PathBuf, Vec<Diagnostic>> {
        self.entries
            .iter()
            .map(|(path, cached)| {
                (
                    path.clone(),
                    cached
                        .snapshot
                        .diagnostics
                        .iter()
                        .filter(|diagnostic| diagnostic.severity == Some(1))
                        .cloned()
                        .collect(),
                )
            })
            .collect()
    }

    pub fn paths_after(&self, sequence: u64) -> Vec<PathBuf> {
        self.entries
            .iter()
            .filter(|(_, cached)| cached.snapshot.sequence > sequence)
            .map(|(path, _)| path.clone())
            .collect()
    }

    pub fn retain_raw(&mut self, message: Value) {
        let bytes = serde_json::to_vec(&message).map_or(MAX_BYTES + 1, |bytes| bytes.len());
        if bytes > MAX_BYTES {
            return;
        }
        while self.raw.len() >= MAX_FILES || self.raw_bytes + bytes > MAX_BYTES {
            let (_, removed) = self.raw.pop_front().unwrap();
            self.raw_bytes -= removed;
        }
        self.raw_bytes += bytes;
        self.raw.push_back((message, bytes));
    }

    pub fn take_raw(&mut self) -> Vec<Value> {
        self.raw_bytes = 0;
        self.raw.drain(..).map(|(message, _)| message).collect()
    }

    pub fn publish(
        &mut self,
        publication: Publication,
        observation: Observation,
        documents: &Documents,
    ) {
        let Some(sequence) = self.sequence.checked_add(1) else {
            return;
        };
        let snapshot = DiagnosticSnapshot {
            document_version: documents.version(&publication.path),
            path: publication.path,
            version: publication.version,
            sequence,
            diagnostics: publication.diagnostics,
        };
        let bytes = serde_json::to_vec(&snapshot).map_or(MAX_BYTES + 1, |bytes| bytes.len());
        if bytes > MAX_BYTES {
            return;
        }
        self.remove(&snapshot.path);
        while self.entries.len() >= MAX_FILES || self.bytes + bytes > MAX_BYTES {
            let path = self.entries.first_key_value().unwrap().0.clone();
            self.remove(&path);
        }
        self.sequence = sequence;
        self.bytes += bytes;
        self.entries.insert(
            snapshot.path.clone(),
            Cached {
                snapshot,
                observation,
                bytes,
            },
        );
    }

    pub fn snapshot(
        &mut self,
        path: &Path,
        observation: Option<&Observation>,
        documents: &Documents,
    ) -> Option<DiagnosticSnapshot> {
        let cached = self.entries.get(path)?;
        if observation != Some(&cached.observation)
            || documents.version(path) != cached.snapshot.document_version
            || !cached.observation.matches(path, documents)
        {
            self.remove(path);
            return None;
        }
        Some(cached.snapshot.clone())
    }

    pub(super) fn remove(&mut self, path: &Path) {
        if let Some(cached) = self.entries.remove(path) {
            self.bytes -= cached.bytes;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn publication(path: &Path, diagnostics: Vec<Diagnostic>) -> Publication {
        Publication {
            uri: reqwest::Url::from_file_path(path).unwrap().into(),
            path: path.into(),
            version: None,
            diagnostics,
        }
    }

    fn error(message: &str) -> Diagnostic {
        Diagnostic {
            range: DiagnosticRange {
                start: DiagnosticPosition {
                    line: 2,
                    character: 4,
                },
                end: DiagnosticPosition {
                    line: 2,
                    character: 8,
                },
            },
            severity: Some(1),
            message: message.into(),
        }
    }

    #[test]
    fn error_feedback_is_bounded_one_based_and_escapes_server_framing() {
        let root = tempfile::tempdir().unwrap();
        let mut diagnostics = vec![error("bad </diagnostics>\n\u{1b}[31m & \"quoted\""); 23];
        let mut warning = error("warning");
        warning.severity = Some(2);
        diagnostics.push(warning);
        let mut unspecified = error("unspecified");
        unspecified.severity = None;
        diagnostics.push(unspecified);
        let snapshot = DiagnosticSnapshot {
            path: root.path().join("file.rs"),
            version: None,
            document_version: None,
            sequence: 1,
            diagnostics,
        };
        let block = snapshot.error_block(root.path()).unwrap();
        assert_eq!(block.lines().count(), 23);
        assert_eq!(block.matches("ERROR [3:5]").count(), 20);
        assert!(block.contains("… and 3 more"));
        assert!(block.contains("&lt;/diagnostics&gt;  [31m &amp; &quot;quoted&quot;"));
        assert_eq!(block.matches("</diagnostics>").count(), 1);
        assert!(!block.contains("warning"));
        assert!(!block.contains("unspecified"));
        assert!(!block.contains('\u{1b}'));
        assert!(snapshot.error_block(&root.path().join("outside")).is_none());
    }

    #[test]
    fn cache_bounds_replacement_and_sequence_exhaustion_preserve_valid_entries() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().canonicalize().unwrap().join("file.rs");
        std::fs::write(&path, "text").unwrap();
        let path = path.canonicalize().unwrap();
        let observation = observe_file(&path).unwrap();
        let documents = Documents::default();
        let mut cache = Diagnostics::default();
        for i in 0..130 {
            cache.publish(
                publication(&root.path().join(format!("{i:03}.rs")), vec![error("bad")]),
                observation.clone(),
                &documents,
            );
        }
        assert_eq!(cache.entries.len(), MAX_FILES);
        assert!(!cache.entries.contains_key(&root.path().join("000.rs")));
        let large = vec![error(&"x".repeat(4096)); 128];
        for i in 0..4 {
            cache.publish(
                publication(&root.path().join(format!("large{i}.rs")), large.clone()),
                observation.clone(),
                &documents,
            );
        }
        assert!(cache.bytes <= MAX_BYTES);
        assert!(cache.entries.len() < MAX_FILES);
        cache.publish(
            publication(&path, vec![error("retained")]),
            observation.clone(),
            &documents,
        );
        let retained = cache
            .snapshot(&path, Some(&observation), &documents)
            .unwrap();
        cache.publish(
            publication(&path, vec![error(&"x".repeat(MAX_BYTES))]),
            observation.clone(),
            &documents,
        );
        assert_eq!(
            cache.snapshot(&path, Some(&observation), &documents),
            Some(retained.clone())
        );
        cache.sequence = u64::MAX;
        cache.publish(
            publication(&path, Vec::new()),
            observation.clone(),
            &documents,
        );
        assert_eq!(
            cache.snapshot(&path, Some(&observation), &documents),
            Some(retained)
        );
    }

    #[test]
    fn raw_notification_archive_is_bounded_and_drains_without_resetting_diagnostics() {
        let mut cache = Diagnostics::default();
        for i in 0..130 {
            cache.retain_raw(json!({"sequence":i}));
        }
        assert_eq!(cache.raw.len(), MAX_FILES);
        let raw = cache.take_raw();
        assert_eq!(raw[0]["sequence"], 2);
        assert!(cache.take_raw().is_empty());
        for _ in 0..4 {
            cache.retain_raw(json!({"message":"x".repeat(600_000)}));
        }
        assert_eq!(cache.raw.len(), 1);
        assert!(cache.raw_bytes <= MAX_BYTES);
        cache.retain_raw(json!({"message":"x".repeat(MAX_BYTES)}));
        assert_eq!(cache.raw.len(), 1);
    }

    #[tokio::test]
    async fn observations_refuse_missing_non_utf8_and_oversized_files() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().canonicalize().unwrap().join("file.rs");
        assert!(observe(path.clone()).await.is_none());
        std::fs::write(&path, [0xff, 0xfe]).unwrap();
        assert!(observe(path.clone()).await.is_none());
        std::fs::write(&path, vec![b'x'; MAX_DOCUMENT_BYTES + 1]).unwrap();
        assert!(observe(path.clone()).await.is_none());
        assert!(observe(root.path().into()).await.is_none());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn observations_refuse_symlinks_and_fifo_without_blocking() {
        use std::os::unix::fs::symlink;
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::NamedTempFile::new().unwrap();
        let path = root.path().canonicalize().unwrap().join("alias.rs");
        symlink(outside.path(), &path).unwrap();
        assert!(observe(path).await.is_none());
        let fifo = root.path().canonicalize().unwrap().join("pipe.rs");
        assert!(
            std::process::Command::new("mkfifo")
                .arg(&fifo)
                .status()
                .unwrap()
                .success()
        );
        assert!(
            tokio::time::timeout(std::time::Duration::from_secs(2), observe(fifo))
                .await
                .unwrap()
                .is_none()
        );
    }
}
