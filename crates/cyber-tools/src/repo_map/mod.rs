//! The repository map (`code-intelligence` → Repository map …): a local index of each
//! checkout's definitions and references, ranked with personalized PageRank and rendered
//! to a token budget. It never runs repository code, starts no language server and never
//! leaves the machine.

pub mod build;
pub mod extract;
pub mod graph;
pub mod lang;
pub mod rank;
pub mod render;
pub mod store;
pub mod walk;

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use cyber_core::config::RepoMapSettings;
use serde::Serialize;

use self::build::{BuildError, Knobs, Report};
use self::store::{IndexedFile, Store};

/// A refresh runs when the last one is older than this, or a path is dirty.
pub const DEBOUNCE: Duration = Duration::from_secs(10);
/// How long a caller waits for a refresh of an already built index before answering
/// from the last snapshot, marked `stale`.
pub const STAT_BOUND: Duration = Duration::from_secs(2);
/// How long a `repo_map` call waits for a running build of an unbuilt index.
pub const TOOL_WAIT: Duration = Duration::from_secs(10);
/// Arguments the tool accepts per list.
pub const MAX_FOCUS: usize = 20;
pub const MIN_TOKENS: usize = 256;
pub const MAX_TOKENS: usize = 8192;
pub const DEFAULT_TOOL_TOKENS: usize = 2048;
pub const DEFAULT_CONTEXT_TOKENS: usize = 1024;
/// Session files personalizing the ranking.
pub const SESSION_FILES: usize = 20;

/// What to rank and render.
#[derive(Debug, Clone, Default)]
pub struct MapQuery {
    /// Location-relative directory or file the map is restricted to.
    pub path: Option<String>,
    pub focus_paths: Vec<String>,
    pub focus_symbols: Vec<String>,
    /// The Session's most recently read or edited paths, most recent first.
    pub session_files: Vec<String>,
    pub tokens: usize,
}

pub use cyber_server::http::{
    RepoMapFile as FileInfo, RepoMapIndex as IndexInfo, RepoMapSymbol as SymbolInfo,
};

/// A rendered map.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct MapOutput {
    pub text: String,
    pub files: Vec<FileInfo>,
    pub index: IndexInfo,
    /// Focus paths that are missing or excluded.
    #[serde(skip)]
    pub not_indexed: Vec<String>,
}

impl MapOutput {
    /// The API and `--format json` shape.
    pub fn view(&self) -> cyber_server::http::RepoMapView {
        cyber_server::http::RepoMapView {
            text: self.text.clone(),
            files: self.files.clone(),
            index: self.index.clone(),
        }
    }

    /// The tool's result: header, `not indexed:` notes, then the map.
    pub fn tool_text(&self) -> String {
        if self.index.files == 0 {
            return "No indexed files".into();
        }
        let mut out = format!(
            "Repository map: {} of {} files, {} symbols, ~{} tokens (index {})\n",
            self.files.len(),
            self.index.files,
            self.index.symbols,
            render::estimate_tokens(self.text.len()),
            self.index.status
        );
        for path in &self.not_indexed {
            out.push_str(&format!("not indexed: {path}\n"));
        }
        out.push_str(&self.text);
        out
    }
}

/// Paths (Location-relative) left out of a map.
pub type Hidden = Arc<dyn Fn(&str) -> bool + Send + Sync>;

/// Called after every build with its report; it must only see counts.
pub type BuildObserver = dyn Fn(&Report) + Send + Sync;

/// Per-process coordination of every checkout's index.
pub struct RepoMaps {
    cache_dir: PathBuf,
    checkouts: Mutex<HashMap<PathBuf, Arc<Checkout>>>,
    observer: Option<Arc<BuildObserver>>,
    knobs: Mutex<Knobs>,
}

#[derive(Default)]
struct Checkout {
    state: Mutex<CheckoutState>,
    done: tokio::sync::Notify,
}

#[derive(Default)]
struct CheckoutState {
    running: bool,
    /// Another process holds the build lock.
    busy: bool,
    dirty: bool,
    last_refresh: Option<Instant>,
    /// Indexed files at a committed generation.
    snapshot: Option<(i64, Arc<Vec<IndexedFile>>)>,
}

impl RepoMaps {
    pub fn new(cache_dir: PathBuf) -> Self {
        Self {
            cache_dir,
            checkouts: Mutex::default(),
            observer: None,
            knobs: Mutex::default(),
        }
    }

    pub fn with_observer(mut self, observer: Arc<BuildObserver>) -> Self {
        self.observer = Some(observer);
        self
    }

    pub fn cache_dir(&self) -> &Path {
        &self.cache_dir
    }

    /// Test knobs (slow walk, eviction cap).
    pub fn set_knobs(&self, knobs: Knobs) {
        *self.knobs.lock().unwrap_or_else(PoisonError::into_inner) = knobs;
    }

    fn checkout(&self, root: &Path) -> Arc<Checkout> {
        self.checkouts
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .entry(root.to_path_buf())
            .or_default()
            .clone()
    }

    /// Mark `path` dirty in every checkout containing it (edit tools, shell patches).
    pub fn mark_dirty(&self, path: &Path) {
        let checkouts = self
            .checkouts
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        for (root, checkout) in checkouts.iter() {
            if path.starts_with(root) {
                checkout
                    .state
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .dirty = true;
            }
        }
    }

    /// Start a background refresh when due (or `force`d); never waits.
    pub fn refresh(self: &Arc<Self>, root: &Path, settings: &RepoMapSettings, force: bool) {
        let checkout = self.checkout(root);
        {
            let mut state = checkout
                .state
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            let due = force
                || state.dirty
                || state.last_refresh.is_none_or(|at| at.elapsed() >= DEBOUNCE);
            if state.running || !due {
                return;
            }
            state.running = true;
            state.dirty = false;
        }
        let maps = Arc::clone(self);
        let root = root.to_path_buf();
        let settings = settings.clone();
        let knobs = Knobs {
            reextract: force,
            ..*self.knobs.lock().unwrap_or_else(PoisonError::into_inner)
        };
        let run = move || maps.build_now(&root, &settings, knobs, &checkout);
        match tokio::runtime::Handle::try_current() {
            Ok(handle) => {
                handle.spawn_blocking(run);
            }
            Err(_) => {
                std::thread::spawn(run);
            }
        }
    }

    /// Run one build on this thread and settle the checkout's state.
    fn build_now(
        &self,
        root: &Path,
        settings: &RepoMapSettings,
        knobs: Knobs,
        checkout: &Checkout,
    ) {
        let result = build::run(&self.cache_dir, root, settings, knobs);
        {
            let mut state = checkout
                .state
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            state.running = false;
            state.busy = matches!(result, Err(BuildError::Busy));
            state.last_refresh = Some(Instant::now());
        }
        checkout.done.notify_waiters();
        match result {
            Ok(report) => {
                if let Some(observer) = &self.observer {
                    observer(&report);
                }
            }
            // The error can name files: logs carry no paths (data stays local).
            Err(BuildError::Failed(_)) => cyber_core::log::warn(
                "repo_map",
                "repository index build failed",
                serde_json::json!({}),
            ),
            Err(BuildError::Busy) => {}
        }
    }

    /// Build synchronously (CLI without a server), then report.
    pub fn build_blocking(
        &self,
        root: &Path,
        settings: &RepoMapSettings,
        reextract: bool,
    ) -> Result<(), String> {
        let checkout = self.checkout(root);
        checkout
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .running = true;
        let knobs = Knobs {
            reextract,
            ..*self.knobs.lock().unwrap_or_else(PoisonError::into_inner)
        };
        self.build_now(root, settings, knobs, &checkout);
        Ok(())
    }

    /// Refresh when due, wait up to `wait` (or the stat bound once an index exists), and
    /// answer from committed state.
    pub async fn query(
        self: &Arc<Self>,
        root: &Path,
        settings: &RepoMapSettings,
        query: &MapQuery,
        hidden: Hidden,
        wait: Duration,
    ) -> Result<MapOutput, String> {
        self.refresh(root, settings, false);
        let checkout = self.checkout(root);
        let built = Store::open(&self.cache_dir)?
            .checkout_row(&store::root_id(root))?
            .is_some_and(|row| row.refreshed_at.is_some());
        let limit = if built { STAT_BOUND.min(wait) } else { wait };
        let deadline = tokio::time::Instant::now() + limit;
        loop {
            let notified = checkout.done.notified();
            if !checkout
                .state
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .running
            {
                break;
            }
            if tokio::time::timeout_at(deadline, notified).await.is_err() {
                break;
            }
        }
        let maps = Arc::clone(self);
        let root = root.to_path_buf();
        let query = query.clone();
        tokio::task::spawn_blocking(move || maps.answer(&root, &query, &*hidden))
            .await
            .map_err(|_| "repository map ranking did not complete".to_owned())?
    }

    /// Rank and render from committed state, without refreshing.
    pub fn answer(
        &self,
        root: &Path,
        query: &MapQuery,
        hidden: &(dyn Fn(&str) -> bool + Sync),
    ) -> Result<MapOutput, String> {
        let index = self.status(root)?;
        let files = self.snapshot(root)?;
        let mut output = compose(&files, query, hidden);
        output.index = index;
        Ok(output)
    }

    /// The indexed files at the latest committed generation, cached per checkout.
    fn snapshot(&self, root: &Path) -> Result<Arc<Vec<IndexedFile>>, String> {
        let db = Store::open(&self.cache_dir)?;
        let id = store::root_id(root);
        let generation = db.checkout_row(&id)?.map_or(-1, |row| row.generation);
        let checkout = self.checkout(root);
        if let Some((cached, files)) = &checkout
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .snapshot
            && *cached == generation
        {
            return Ok(files.clone());
        }
        let files = Arc::new(db.indexed(&id)?);
        checkout
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .snapshot = Some((generation, files.clone()));
        Ok(files)
    }

    /// The committed index status, `building` or `stale` while a refresh runs.
    pub fn status(&self, root: &Path) -> Result<IndexInfo, String> {
        let db = Store::open(&self.cache_dir)?;
        let id = store::root_id(root);
        let row = db.checkout_row(&id)?;
        drop(db);
        let files = self.snapshot(root)?;
        let db = Store::open(&self.cache_dir)?;
        let mut info = IndexInfo {
            status: row
                .as_ref()
                .map_or_else(|| "building".into(), |r| r.status.clone()),
            files: files.len(),
            symbols: files
                .iter()
                .filter_map(|f| f.tags.as_ref())
                .map(|t| t.defs.len())
                .sum(),
            excluded: db.excluded(&id)?.into_iter().collect(),
            languages: BTreeMap::new(),
            refreshed_at: row.as_ref().and_then(|r| r.refreshed_at),
            build_ms: row.as_ref().and_then(|r| r.build_ms),
        };
        for file in files.iter() {
            *info.languages.entry(file.lang.clone()).or_default() += 1;
        }
        let state = self.checkout(root);
        let state = state.state.lock().unwrap_or_else(PoisonError::into_inner);
        if state.running || state.busy {
            info.status = if info.refreshed_at.is_some() {
                "stale".into()
            } else {
                "building".into()
            };
        } else if info.status == "empty" {
            info.status = "building".into();
        }
        Ok(info)
    }

    /// Delete a checkout's rows (`cyber lsp map --clear`).
    pub fn clear(&self, root: &Path) -> Result<(), String> {
        let db = Store::open(&self.cache_dir)?;
        db.clear(&store::root_id(root))?;
        if let Some(checkout) = self
            .checkouts
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(root)
        {
            let mut state = checkout
                .state
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            state.snapshot = None;
            state.last_refresh = None;
        }
        Ok(())
    }

    /// The cache database path and its size in bytes.
    pub fn cache_usage(&self) -> (PathBuf, u64) {
        let path = store::index_path(&self.cache_dir);
        let size = std::fs::metadata(&path).map_or(0, |m| m.len());
        (path, size)
    }
}

/// Whether automatic indexing is refused at `location`: the home directory or a
/// filesystem root.
pub fn refused_scope(location: &Path, home: &Path) -> bool {
    location == home || location.parent().is_none()
}

/// The checkout root and the in-map restriction for a Location-relative `path` (empty
/// for none). In the home directory or a filesystem root a `path` is required, and it
/// becomes the root.
pub fn checkout_scope(
    location: &Path,
    home: &Path,
    path: &str,
) -> Result<(PathBuf, Option<String>), String> {
    let location = canonical(location);
    if !refused_scope(&location, &canonical(home)) {
        return Ok((location, (!path.is_empty()).then(|| path.to_owned())));
    }
    if path.is_empty() {
        return Err("RepoMapScopeError: the Location is the home directory or a filesystem root; map a project subdirectory instead (set path)".into());
    }
    let root = canonical(&location.join(path));
    if !root.is_dir() {
        return Err(format!("RepoMapScopeError: {path:?} is not a directory"));
    }
    Ok((root, None))
}

pub fn canonical(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

/// A Location-relative path argument, normalized: no absolute paths and no `..`.
pub fn relative_arg(path: &str) -> Result<String, String> {
    let normalized = path.replace('\\', "/");
    let escapes = Path::new(path).is_absolute()
        || normalized.starts_with('/')
        || normalized.split('/').any(|part| part == "..");
    if escapes {
        return Err(format!(
            "repository map paths are Location-relative; {path:?} is outside the Location"
        ));
    }
    Ok(normalized
        .trim_start_matches("./")
        .trim_end_matches('/')
        .to_owned())
}

/// A token budget clamped to 256–8192.
pub fn clamp_tokens(tokens: i64) -> usize {
    tokens.clamp(MIN_TOKENS as i64, MAX_TOKENS as i64) as usize
}

/// Rank and render `files` for `query`; `hidden` paths are left out entirely.
pub fn compose(
    files: &[IndexedFile],
    query: &MapQuery,
    hidden: &(dyn Fn(&str) -> bool + Sync),
) -> MapOutput {
    let scope = query
        .path
        .as_deref()
        .map(|p| p.trim_matches('/'))
        .filter(|p| !p.is_empty() && *p != ".");
    let kept: Vec<&IndexedFile> = files
        .iter()
        .filter(|f| scope.is_none_or(|s| under(&f.path, s)))
        .filter(|f| !hidden(&f.path))
        .collect();
    let tags: Vec<Option<&extract::Tags>> = kept.iter().map(|f| f.tags.as_deref()).collect();
    let focus: BTreeSet<String> = query.focus_symbols.iter().cloned().collect();
    let graph = graph::build(&tags, &focus, graph::MAX_EDGES);
    let weights = personalization(&kept, &tags, query, &focus);
    let ranks = rank::pagerank(&graph, &rank::personalization(&weights));
    let mut ranked = rank::definitions(&graph, &tags, &ranks);
    // Focus files come first, their definitions in rank order.
    let focus_paths: Vec<&str> = query.focus_paths.iter().map(|p| normalize(p)).collect();
    let in_focus = |file: usize| {
        focus_paths
            .iter()
            .any(|p| !p.is_empty() && under(&kept[file].path, p))
    };
    ranked.sort_by_key(|def| !in_focus(def.1));
    let render_files: Vec<render::RenderFile<'_>> = kept
        .iter()
        .zip(&tags)
        .map(|(f, t)| render::RenderFile {
            path: &f.path,
            tags: *t,
        })
        .collect();
    let rendered = render::render(&render_files, &ranked, &ranks, query.tokens);
    let infos = rendered
        .files
        .iter()
        .map(|(file, defs)| FileInfo {
            path: kept[*file].path.clone(),
            rank: ranks[*file],
            symbols: defs
                .iter()
                .filter_map(|d| tags[*file].map(|t| &t.defs[*d]))
                .map(|d| SymbolInfo {
                    name: d.name.clone(),
                    kind: d.kind.as_str().into(),
                    line: d.line,
                    signature: d.signature.clone(),
                })
                .collect(),
        })
        .collect();
    let not_indexed = query
        .focus_paths
        .iter()
        .filter(|p| !kept.iter().any(|f| under(&f.path, normalize(p))))
        .cloned()
        .collect();
    MapOutput {
        text: rendered.text,
        files: infos,
        index: IndexInfo::default(),
        not_indexed,
    }
}

fn normalize(path: &str) -> &str {
    let path = path.trim_start_matches("./").trim_matches('/');
    if path == "." { "" } else { path }
}

/// `path` is `scope` or lies below it.
fn under(path: &str, scope: &str) -> bool {
    scope.is_empty()
        || path == scope
        || path
            .strip_prefix(scope)
            .is_some_and(|rest| rest.starts_with('/'))
}

/// 1.0 for focus paths and files defining a focus symbol, 0.5 for files referencing a
/// focus symbol and for recent Session files; the largest weight applies.
fn personalization(
    files: &[&IndexedFile],
    tags: &[Option<&extract::Tags>],
    query: &MapQuery,
    focus: &BTreeSet<String>,
) -> Vec<f64> {
    let session: Vec<&str> = query
        .session_files
        .iter()
        .take(SESSION_FILES)
        .map(|p| normalize(p))
        .collect();
    let focus_paths: Vec<&str> = query.focus_paths.iter().map(|p| normalize(p)).collect();
    files
        .iter()
        .zip(tags)
        .map(|(file, tags)| {
            let mut weight: f64 = 0.0;
            if focus_paths
                .iter()
                .any(|p| !p.is_empty() && under(&file.path, p))
            {
                weight = 1.0;
            }
            if let Some(tags) = tags {
                if tags.defs.iter().any(|d| focus.contains(&d.name)) {
                    weight = weight.max(1.0);
                }
                if focus.iter().any(|name| tags.refs.contains_key(name)) {
                    weight = weight.max(0.5);
                }
            }
            if session.iter().any(|p| *p == file.path) {
                weight = weight.max(0.5);
            }
            weight
        })
        .collect()
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
