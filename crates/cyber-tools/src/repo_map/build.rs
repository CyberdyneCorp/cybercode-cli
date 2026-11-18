//! One index build or refresh of a checkout (`code-intelligence` → Repository map build
//! limits and freshness): walk and stat, hash what changed, extract what has no stored
//! tags, commit in batches of 500 and stop at the build deadline.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use cyber_core::config::RepoMapSettings;
use sha2::{Digest, Sha256};

use super::extract::{self, Failure};
use super::lang::{Lang, hex};
use super::store::{self, FileRow, Store};
use super::walk::{self, Entry, Selector};

/// Files per committed batch.
pub const BATCH: usize = 500;

/// Test and benchmark knobs; zero in production.
#[derive(Debug, Clone, Copy, Default)]
pub struct Knobs {
    /// A pause per walked entry, simulating a slow filesystem.
    pub walk_delay: Duration,
    /// Size cap for garbage collection; `None` uses [`store::SIZE_CAP`].
    pub size_cap: Option<u64>,
    /// Extract every file again, ignoring stored tags (`--rebuild`).
    pub reextract: bool,
    /// Treat the deadline as reached after this many committed batches.
    pub stop_after_batches: Option<usize>,
}

/// What one build did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Report {
    pub status: String,
    pub files: usize,
    pub symbols: usize,
    /// Files parsed by this build (not reused from stored tags).
    pub parsed: usize,
    pub duration_ms: u64,
}

impl Report {
    /// The counts the telemetry exporter receives.
    pub fn telemetry(&self) -> cyber_server::runtime::RepoMapBuild {
        cyber_server::runtime::RepoMapBuild {
            status: self.status.clone(),
            files: self.files as u64,
            symbols: self.symbols as u64,
            parsed: self.parsed as u64,
            duration_ms: self.duration_ms,
        }
    }
}

pub enum BuildError {
    /// Another process holds the checkout's lock.
    Busy,
    Failed(String),
}

/// Parser threads: `min(4, max(1, cpus / 2))`.
pub fn threads() -> usize {
    let cpus = std::thread::available_parallelism().map_or(1, |n| n.get());
    (cpus / 2).clamp(1, 4)
}

/// Build or refresh the index of `root` under the cross-process lock.
pub fn run(
    cache_dir: &Path,
    root: &Path,
    settings: &RepoMapSettings,
    knobs: Knobs,
) -> Result<Report, BuildError> {
    let started = Instant::now();
    let deadline = started + Duration::from_millis(settings.build_timeout_ms);
    let mut db = Store::open(cache_dir).map_err(BuildError::Failed)?;
    let row = db.checkout(root).map_err(BuildError::Failed)?;
    let _lock = lock(cache_dir, &row.root_id)?;
    let selector = Selector::new(settings);
    let walked = walk::walk(
        root,
        &selector,
        settings.max_files,
        Some(deadline),
        knobs.walk_delay,
    );
    let existing = db.file_rows(&row.root_id).map_err(BuildError::Failed)?;
    let mut work = Work::new(root, &selector, &existing);
    work.reextract = knobs.reextract;
    work.stop_after = knobs.stop_after_batches;
    for entry in &walked.entries {
        work.classify(entry);
    }
    if !walked.interrupted {
        let seen: HashSet<&str> = walked.entries.iter().map(|e| e.path.as_str()).collect();
        work.removed = existing
            .keys()
            .filter(|path| !seen.contains(path.as_str()))
            .cloned()
            .collect();
        work.removed.sort();
    }
    let mut timed_out = walked.interrupted;
    let parsed = work.commit(&mut db, &row.root_id, deadline, &mut timed_out)?;
    let status = if timed_out {
        "partial"
    } else if walked.truncated {
        "truncated"
    } else {
        "complete"
    };
    let duration_ms = started.elapsed().as_millis() as u64;
    db.finish(&row.root_id, status, duration_ms as i64)
        .map_err(BuildError::Failed)?;
    db.gc(knobs.size_cap.unwrap_or(store::SIZE_CAP), &row.root_id)
        .map_err(BuildError::Failed)?;
    let indexed = db.indexed(&row.root_id).map_err(BuildError::Failed)?;
    Ok(Report {
        status: status.into(),
        files: indexed.len(),
        symbols: indexed
            .iter()
            .filter_map(|f| f.tags.as_ref())
            .map(|t| t.defs.len())
            .sum(),
        parsed,
        duration_ms,
    })
}

/// The checkout's cross-process lock file, held for the build.
fn lock(cache_dir: &Path, root_id: &str) -> Result<std::fs::File, BuildError> {
    let path = cache_dir.join("repo-map").join(format!("{root_id}.lock"));
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .map_err(|e| BuildError::Failed(format!("{}: {e}", path.display())))?;
    match file.try_lock() {
        Ok(()) => Ok(file),
        Err(std::fs::TryLockError::WouldBlock) => Err(BuildError::Busy),
        Err(std::fs::TryLockError::Error(e)) => Err(BuildError::Failed(e.to_string())),
    }
}

/// Rows to write: unchanged files are skipped, path-excluded ones written as they are,
/// and changed candidates read, hashed and extracted.
struct Work<'a> {
    root: &'a Path,
    selector: &'a Selector,
    existing: &'a HashMap<String, FileRow>,
    /// Rows that need no reading.
    plain: Vec<FileRow>,
    /// Candidates whose content must be read.
    changed: Vec<Entry>,
    removed: Vec<String>,
    reextract: bool,
    stop_after: Option<usize>,
}

impl<'a> Work<'a> {
    fn new(root: &'a Path, selector: &'a Selector, existing: &'a HashMap<String, FileRow>) -> Self {
        Self {
            root,
            selector,
            existing,
            plain: Vec::new(),
            changed: Vec::new(),
            removed: Vec::new(),
            reextract: false,
            stop_after: None,
        }
    }

    fn classify(&mut self, entry: &Entry) {
        let previous = self.existing.get(&entry.path);
        if let Some(reason) = entry.excluded {
            let row = FileRow {
                path: entry.path.clone(),
                size: entry.size,
                mtime_ns: entry.mtime_ns,
                sha: None,
                lang: None,
                excluded: Some(reason.as_str().into()),
            };
            if previous != Some(&row) {
                self.plain.push(row);
            }
            return;
        }
        let unchanged = !self.reextract
            && previous.is_some_and(|row| {
                row.size == entry.size
                    && row.mtime_ns == entry.mtime_ns
                    && row.excluded.as_deref().is_none_or(content_based)
            });
        if !unchanged {
            self.changed.push(entry.clone());
        }
    }

    /// Commit plain rows, then changed files in batches until the deadline. Returns the
    /// number of files parsed.
    fn commit(
        &mut self,
        db: &mut Store,
        id: &str,
        deadline: Instant,
        timed_out: &mut bool,
    ) -> Result<usize, BuildError> {
        for chunk in self.plain.chunks(BATCH) {
            db.commit(id, chunk, &[], &[]).map_err(BuildError::Failed)?;
        }
        for chunk in self.removed.chunks(BATCH) {
            db.commit(id, &[], &[], chunk).map_err(BuildError::Failed)?;
        }
        let mut parsed = 0;
        for (batch, chunk) in self.changed.chunks(BATCH).enumerate() {
            let stopped = self.stop_after.is_some_and(|n| batch >= n);
            if *timed_out || stopped || Instant::now() > deadline {
                *timed_out = true;
                break;
            }
            let (rows, blobs, count) =
                process(self.root, self.selector, chunk, db, self.reextract)?;
            parsed += count;
            db.commit(id, &rows, &blobs, &[])
                .map_err(BuildError::Failed)?;
        }
        Ok(parsed)
    }
}

/// Reasons decided by reading the file: they stay valid while size and mtime do.
fn content_based(reason: &str) -> bool {
    matches!(reason, "binary" | "generated" | store::PARSE_TIMEOUT)
}

/// A changed file after reading: its row and, when it needs parsing, its bytes.
struct Read {
    row: FileRow,
    parse: Option<(Lang, Vec<u8>)>,
}

type Blob = (String, String, String, Vec<u8>);

fn process(
    root: &Path,
    selector: &Selector,
    chunk: &[Entry],
    db: &Store,
    reextract: bool,
) -> Result<(Vec<FileRow>, Vec<Blob>, usize), BuildError> {
    let mut reads = parallel(chunk, |entry| read(root, selector, entry));
    // Content already extracted (by this or another checkout) is not parsed again.
    for read in reads.iter_mut().flatten().filter(|_| !reextract) {
        let Some((lang, _)) = &read.parse else {
            continue;
        };
        let sha = read.row.sha.as_deref().unwrap_or_default();
        if db
            .has_blob(sha, lang.extractor_version())
            .map_err(BuildError::Failed)?
        {
            read.parse = None;
        }
    }
    let extracted = parallel(&reads, |read| {
        let (lang, bytes) = read.as_ref()?.parse.as_ref()?;
        Some(extract::extract(*lang, bytes))
    });
    let mut rows = Vec::new();
    let mut blobs = Vec::new();
    let mut parsed = 0;
    for (read, result) in reads.into_iter().zip(extracted) {
        let Some(mut read) = read else {
            continue;
        };
        if let (Some(result), Some((lang, _))) = (result, &read.parse) {
            parsed += 1;
            match result {
                Ok(tags) => blobs.push((
                    read.row.sha.clone().unwrap_or_default(),
                    lang.extractor_version().to_owned(),
                    lang.id().to_owned(),
                    serde_json::to_vec(&tags).unwrap_or_default(),
                )),
                Err(Failure::Timeout) => read.row.excluded = Some(store::PARSE_TIMEOUT.into()),
                Err(Failure::Unparsable) => read.row.lang = Some(super::lang::OTHER.into()),
            }
        }
        rows.push(read.row);
    }
    Ok((rows, blobs, parsed))
}

/// Read, check and hash one changed file; `None` when it vanished.
fn read(root: &Path, selector: &Selector, entry: &Entry) -> Option<Read> {
    let path: PathBuf = walk::absolute(root, &entry.path);
    let bytes = std::fs::read(&path).ok()?;
    let mut row = FileRow {
        path: entry.path.clone(),
        size: entry.size,
        mtime_ns: entry.mtime_ns,
        sha: None,
        lang: None,
        excluded: None,
    };
    if bytes.len() as u64 > selector.max_file_bytes() {
        row.excluded = Some(walk::Reason::TooLarge.as_str().into());
        return Some(Read { row, parse: None });
    }
    if let Some(reason) = walk::content_reason(&bytes, selector.is_included(&entry.path)) {
        row.excluded = Some(reason.as_str().into());
        return Some(Read { row, parse: None });
    }
    let lang = Lang::of(Path::new(&entry.path));
    row.sha = Some(hex(&Sha256::digest(&bytes)));
    row.lang = Some(lang.map_or(super::lang::OTHER, Lang::id).into());
    let parse = lang.map(|lang| (lang, bytes));
    Some(Read { row, parse })
}

/// Map `items` on [`threads`] scoped threads, keeping input order.
fn parallel<T: Sync, R: Send>(items: &[T], f: impl Fn(&T) -> Option<R> + Sync) -> Vec<Option<R>> {
    let next = AtomicUsize::new(0);
    let workers = threads().min(items.len()).max(1);
    let mut results: Vec<Option<R>> = Vec::with_capacity(items.len());
    results.resize_with(items.len(), || None);
    let parts: Vec<Vec<(usize, R)>> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..workers)
            .map(|_| {
                scope.spawn(|| {
                    let mut done = Vec::new();
                    loop {
                        let index = next.fetch_add(1, Ordering::Relaxed);
                        let Some(item) = items.get(index) else {
                            return done;
                        };
                        if let Some(result) = f(item) {
                            done.push((index, result));
                        }
                    }
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|h| h.join().unwrap_or_default())
            .collect()
    });
    for (index, result) in parts.into_iter().flatten() {
        results[index] = Some(result);
    }
    results
}
