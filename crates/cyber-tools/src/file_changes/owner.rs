//! Host-owned bounded native observers. Registry locks never span a scan, journal
//! append, hook callback or task join. The journal is the acknowledged baseline.
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError, Weak, atomic::AtomicBool};
use std::time::{Duration, Instant};

use cyber_core::hooks::{
    HookCatalog,
    file_observations::{self as records, Delta, Renewal, Update},
};
use cyber_server::runtime::{SessionInfo, file_observations::FileWatch};
use tokio_util::sync::CancellationToken;

use super::{
    native::{Hints, Native},
    root::{Exclusions, Root},
    scan::{self, Limits},
    state::{Change, Kind, Reconciled, Scan, State},
};
use crate::BuiltinHost;

const MAX_LOCATIONS: usize = 32;
const MAX_SESSIONS: usize = 256;
const TICK: Duration = Duration::from_millis(150);
const RECONCILE: Duration = Duration::from_secs(3);
#[derive(Default)]
struct Membership {
    sessions: BTreeSet<String>,
    /// Keep only the newest committed acknowledgement per current Session.
    /// Both maps share the 256-Session bound; input bursts cannot grow a queue.
    renewals: BTreeMap<String, Renewal>,
    explicit: Hints,
}
type Members = Arc<Mutex<Membership>>;
impl Membership {
    fn remove(&mut self, session: &str) {
        self.sessions.remove(session);
        self.renewals.remove(session);
    }
    fn is_empty(&self) -> bool {
        self.sessions.is_empty()
    }
}

struct Entry {
    members: Members,
    cancel: CancellationToken,
    task: tokio::task::JoinHandle<()>,
}
#[derive(Default)]
struct Registry {
    entries: BTreeMap<PathBuf, Entry>,
    closed: bool,
}
#[derive(Default)]
pub(crate) struct Observers(Mutex<Registry>);

impl Registry {
    fn remove_other_memberships(&self, path: &Path, session: &str) {
        for (key, entry) in &self.entries {
            if key == path {
                continue;
            }
            let mut members = entry.members.lock().unwrap_or_else(PoisonError::into_inner);
            members.remove(session);
            if members.is_empty() {
                entry.cancel.cancel();
            }
        }
    }
}
impl Entry {
    fn add(&self, host: &BuiltinHost, session: &str) {
        let mut members = self.members.lock().unwrap_or_else(PoisonError::into_inner);
        if members.sessions.contains(session) || members.sessions.len() < MAX_SESSIONS {
            members.sessions.insert(session.into());
        } else {
            notice(
                host,
                session,
                "FileChanged coverage unavailable: Session limit reached",
            );
        }
    }
}

impl Observers {
    pub fn refresh(&self, host: &BuiltinHost, info: &SessionInfo) {
        if info.archived {
            self.remove(&info.id);
            return;
        }
        let Ok(path) = Path::new(&info.directory).canonicalize() else {
            return;
        };
        let Ok(executor) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let mut registry = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        if registry.closed {
            return;
        }
        registry
            .entries
            .retain(|_, entry| !entry.task.is_finished());
        registry.remove_other_memberships(&path, &info.id);
        if let Some(entry) = registry.entries.get(&path)
            && !entry.cancel.is_cancelled()
        {
            entry.add(host, &info.id);
            return;
        }
        let previous = registry.entries.remove(&path);
        if registry.entries.len() >= MAX_LOCATIONS {
            notice(
                host,
                &info.id,
                "FileChanged coverage unavailable: active Location limit reached",
            );
            return;
        }
        let members = Arc::new(Mutex::new(Membership {
            sessions: BTreeSet::from([info.id.clone()]),
            renewals: BTreeMap::new(),
            explicit: Hints::default(),
        }));
        let cancel = CancellationToken::new();
        let weak = host.weak.clone();
        let root = path.clone();
        let directory = info.directory.clone();
        let membership = Arc::clone(&members);
        let owner = cancel.clone();
        let task = executor.spawn(async move {
            if let Some(previous) = previous {
                let _ = previous.task.await;
            }
            run(weak, root, directory, membership, owner).await;
        });
        registry.entries.insert(
            path,
            Entry {
                members,
                cancel,
                task,
            },
        );
    }
    /// Successful file/formatter mutations contribute hints, never a writer claim
    /// or a permission. The pinned metadata scan still decides what actually exists.
    pub fn changed(&self, location: &Path, path: &Path) {
        let Ok(root) = location.canonicalize() else {
            return;
        };
        let Ok(relative) = path
            .strip_prefix(location)
            .or_else(|_| path.strip_prefix(&root))
        else {
            return;
        };
        let registry = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(entry) = registry.entries.get(&root) else {
            return;
        };
        if entry.cancel.is_cancelled() {
            return;
        }
        entry
            .members
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .explicit
            .record(relative);
    }
    pub fn acknowledge(&self, host: &BuiltinHost, info: &SessionInfo, event_id: &str) {
        self.refresh(host, info);
        let Ok(path) = Path::new(&info.directory).canonicalize() else {
            return;
        };
        let registry = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(entry) = registry.entries.get(&path) else {
            return;
        };
        if entry.cancel.is_cancelled() {
            return;
        }
        let mut members = entry.members.lock().unwrap_or_else(PoisonError::into_inner);
        if members.sessions.contains(&info.id) {
            members.renewals.insert(
                info.id.clone(),
                Renewal {
                    session_id: info.id.clone(),
                    event_id: event_id.into(),
                },
            );
        }
    }
    pub fn remove(&self, session: &str) {
        let registry = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        for entry in registry.entries.values() {
            let mut members = entry.members.lock().unwrap_or_else(PoisonError::into_inner);
            members.remove(session);
            if members.is_empty() {
                entry.cancel.cancel();
            }
        }
    }
    pub async fn shutdown(&self) {
        let entries = {
            let mut registry = self.0.lock().unwrap_or_else(PoisonError::into_inner);
            registry.closed = true;
            std::mem::take(&mut registry.entries)
        };
        for entry in entries.values() {
            entry.cancel.cancel();
        }
        for (_, entry) in entries {
            let _ = entry.task.await;
        }
    }
}

fn notice(host: &BuiltinHost, session: &str, message: &str) {
    if let Some(runtime) = host.runtime() {
        runtime.hook_notice(session, "FileChanged", message);
    }
}
fn members(members: &Members) -> Vec<String> {
    members
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .sessions
        .iter()
        .cloned()
        .collect()
}
fn announce(host: &BuiltinHost, sessions: &[String], message: &str) {
    for session in sessions {
        notice(host, session, message);
    }
}

impl BuiltinHost {
    async fn observes_files(&self, directory: &str, sessions: &[String]) -> bool {
        let configured = self
            .hook_config
            .get()
            .and_then(|config| (config.resolve)(Path::new(directory)).ok())
            .and_then(|resolved| HookCatalog::from_config(&resolved).ok())
            .is_some_and(|catalog| {
                catalog
                    .definitions
                    .iter()
                    .chain(catalog.skipped_for("FileChanged"))
                    .any(|definition| definition.event == "FileChanged")
            });
        if configured {
            return true;
        }
        for session in sessions {
            if self
                .client_definitions(session)
                .await
                .iter()
                .any(|definition| definition.event == "FileChanged")
            {
                return true;
            }
        }
        false
    }
    fn observation_exclusions(&self, path: &Path) -> Exclusions {
        let database: Option<String> = self
            .opts
            .store
            .read(|db| {
                Ok(db.query_row(
                    "SELECT file FROM pragma_database_list WHERE name='main'",
                    [],
                    |row| row.get(0),
                )?)
            })
            .ok();
        let files = database
            .filter(|path| !path.is_empty())
            .into_iter()
            .flat_map(|path| {
                [
                    PathBuf::from(&path),
                    PathBuf::from(format!("{path}-wal")),
                    PathBuf::from(format!("{path}-shm")),
                ]
            })
            .collect();
        Exclusions::new(
            path,
            &self.opts.home,
            vec![
                self.opts.tool_output_dir.clone(),
                self.opts.temp_dir.clone(),
                self.opts.global_config_dir.clone(),
            ],
            files,
            crate::permissions::is_protected,
        )
    }
}

async fn snapshot(root: &Root, excluded: Exclusions) -> Result<Scan, String> {
    let directory = root
        .directory
        .try_clone()
        .map_err(|error| error.to_string())?;
    tokio::task::spawn_blocking(move || {
        scan::scan(
            &directory,
            &|path| excluded.excludes(path),
            &Limits {
                entries: records::MAX_FILES,
                depth: 64,
                path_bytes: 4096,
            },
            &AtomicBool::new(false),
        )
    })
    .await
    .map_err(|error| error.to_string())
}
fn names(scan: &Scan) -> BTreeMap<String, records::Stamp> {
    scan.files
        .iter()
        .filter_map(|(path, stamp)| {
            let name = path.to_str()?.replace(std::path::MAIN_SEPARATOR, "/");
            records::relative_name(&name).then(|| (name, stamp.clone()))
        })
        .collect()
}

struct Observation {
    root: Root,
    native: Native,
    journal: FileWatch,
    last_scan: Instant,
    warned_partial: bool,
    warned_budget: bool,
}
impl Observation {
    async fn start(host: &BuiltinHost, path: &Path, directory: &str) -> Result<Self, String> {
        let root = Root::open(path).map_err(|error| error.to_string())?;
        let mut native = Native::start(&root.path, 32).map_err(|error| error.to_string())?;
        let initial = snapshot(&root, host.observation_exclusions(path)).await?;
        if !root.current() {
            return Err("root changed before initial observation".into());
        }
        native.update_directories(&initial.directories, initial.complete);
        let runtime = host.runtime().ok_or("runtime unavailable")?;
        let journal = runtime
            .claim_file_watch(directory.into(), root.identity.clone(), names(&initial))
            .await
            .map_err(|error| error.to_string())?;
        Ok(Self {
            root,
            native,
            journal,
            last_scan: Instant::now() - RECONCILE,
            warned_partial: false,
            warned_budget: false,
        })
    }
    async fn renew(
        &mut self,
        host: &BuiltinHost,
        membership: &Members,
        cancel: &CancellationToken,
    ) {
        let renewals = {
            let mut members = membership.lock().unwrap_or_else(PoisonError::into_inner);
            std::mem::take(&mut members.renewals)
        };
        for (_, renewal) in renewals {
            if cancel.is_cancelled() || !self.root.current() {
                break;
            }
            let session = renewal.session_id.clone();
            let paused = self.warned_budget || self.journal.state().remaining == 0;
            let result = self
                .journal
                .record(Delta {
                    generation: self.journal.state().generation.clone(),
                    updates: vec![],
                    remaining: records::FEEDBACK_LIMIT,
                    dispatch: false,
                    renewal: Some(renewal),
                })
                .await;
            match result {
                Ok(_) => {
                    self.warned_budget = false;
                    if paused {
                        notice(
                            host,
                            &session,
                            "FileChanged handlers resumed after committed user input",
                        );
                    }
                }
                Err(error) => notice(
                    host,
                    &session,
                    &format!("FileChanged feedback renewal unavailable: {error}"),
                ),
            }
        }
    }
    async fn reconcile(
        &mut self,
        host: &BuiltinHost,
        sessions: &[String],
        membership: &Members,
        cancel: &CancellationToken,
    ) -> Result<(), String> {
        if !self.root.current() {
            return Err("Location root incarnation changed".into());
        }
        let mut hint = self.hints();
        hint.merge({
            let mut members = membership.lock().unwrap_or_else(PoisonError::into_inner);
            std::mem::take(&mut members.explicit)
        });
        if hint.paths.is_empty() && !hint.rescan && self.last_scan.elapsed() < RECONCILE {
            return Ok(());
        }
        let excluded = host.observation_exclusions(&self.root.path);
        let scan = snapshot(&self.root, excluded.clone()).await?;
        self.last_scan = Instant::now();
        if cancel.is_cancelled() {
            return Ok(());
        }
        if !self.root.current() {
            return Err("Location root changed during reconciliation".into());
        }
        let native_complete = self
            .native
            .update_directories(&scan.directories, scan.complete);
        let suppressed = !scan.suppressed.is_empty();
        let files = names(&scan);
        let baseline = self
            .journal
            .state()
            .files
            .iter()
            .map(|(path, stamp)| (PathBuf::from(path), stamp.clone()))
            .collect();
        let mut state = State::baseline(baseline);
        let Reconciled::Changes { changes, complete } = state.reconcile_bounded(
            scan,
            &hint.paths,
            records::MAX_FILES,
            2 * records::MAX_FILES,
        ) else {
            return Err("bounded metadata state cannot reconcile this tree".into());
        };
        self.coverage_notice(
            host,
            sessions,
            native_complete && complete && !hint.rescan && !suppressed,
        );
        // A newly protected path is forgotten durably without disclosing an
        // observational deletion to handlers or spending their allowance.
        let (private, mut changes): (Vec<_>, Vec<_>) = changes
            .into_iter()
            .partition(|change| excluded.excludes(&change.path));
        self.retire_paths(&private, cancel).await?;
        // Deletions first keep replacement batches within the retained-file bound.
        changes.sort_by_key(|change| (!matches!(change.kind, Kind::Removed), change.path.clone()));
        let mut pending = changes.as_slice();
        while !pending.is_empty() && !cancel.is_cancelled() {
            let remaining = self.journal.state().remaining as usize;
            let limit = if remaining == 0 {
                records::MAX_BATCH
            } else {
                remaining.min(records::MAX_BATCH)
            };
            let (batch, rest) = pending.split_at(pending.len().min(limit));
            self.record_batch(host, sessions, cancel, batch, &files)
                .await?;
            pending = rest;
        }
        Ok(())
    }
    async fn retire_paths(
        &mut self,
        private: &[Change],
        cancel: &CancellationToken,
    ) -> Result<(), String> {
        for batch in private.chunks(records::MAX_BATCH) {
            if cancel.is_cancelled() {
                break;
            }
            if !self.root.current() {
                return Err("Location root changed before protected-state retirement".into());
            }
            let updates = batch
                .iter()
                .map(|change| Update {
                    path: change
                        .path
                        .to_string_lossy()
                        .replace(std::path::MAIN_SEPARATOR, "/"),
                    stamp: None,
                })
                .collect();
            self.journal
                .record(Delta {
                    generation: self.journal.state().generation.clone(),
                    updates,
                    remaining: self.journal.state().remaining,
                    dispatch: false,
                    renewal: None,
                })
                .await
                .map_err(|error| error.to_string())?;
        }
        Ok(())
    }
    fn hints(&self) -> super::native::Hints {
        let mut hint = self.native.receive(Duration::ZERO);
        for _ in 1..32 {
            let next = self.native.receive(Duration::ZERO);
            if next.paths.is_empty() && !next.rescan {
                break;
            }
            hint.merge(next);
        }
        hint
    }
    fn coverage_notice(&mut self, host: &BuiltinHost, sessions: &[String], complete: bool) {
        if !complete && !self.warned_partial {
            announce(
                host,
                sessions,
                "FileChanged observation is reconciling; native loss or scan limits may leave partial coverage",
            );
        }
        self.warned_partial = !complete;
    }
    async fn record_batch(
        &mut self,
        host: &BuiltinHost,
        sessions: &[String],
        cancel: &CancellationToken,
        batch: &[Change],
        files: &BTreeMap<String, records::Stamp>,
    ) -> Result<(), String> {
        if !self.root.current() {
            return Err("Location root changed before observation commit".into());
        }
        let updates: Vec<_> = batch
            .iter()
            .map(|change| {
                let path = change
                    .path
                    .to_string_lossy()
                    .replace(std::path::MAIN_SEPARATOR, "/");
                Update {
                    stamp: files.get(&path).cloned(),
                    path,
                }
            })
            .collect();
        let dispatch = updates.len() as u32 <= self.journal.state().remaining;
        let remaining =
            self.journal.state().remaining - if dispatch { updates.len() as u32 } else { 0 };
        let event = self
            .journal
            .record(Delta {
                generation: self.journal.state().generation.clone(),
                updates,
                remaining,
                dispatch,
                renewal: None,
            })
            .await
            .map_err(|error| error.to_string())?;
        if dispatch {
            return self.deliver(host, sessions, cancel, &event).await;
        }
        if !self.warned_budget {
            announce(
                host,
                sessions,
                "FileChanged handlers paused: feedback allowance exhausted; send new user input to resume. Observations continue",
            );
            self.warned_budget = true;
        }
        Ok(())
    }
    // Retain exclusive actor ownership across callbacks. The native receiver is
    // Send, not Sync; no shared Observation reference may span an await.
    async fn deliver(
        &mut self,
        host: &BuiltinHost,
        sessions: &[String],
        cancel: &CancellationToken,
        event: &cyber_store::StoredEvent,
    ) -> Result<(), String> {
        for session in sessions {
            if cancel.is_cancelled() {
                break;
            }
            if !self.root.current() {
                return Err("Location root changed before hook delivery".into());
            }
            if let Err(error) = self
                .journal
                .dispatch_to_cancellable(event, session, cancel.clone())
                .await
            {
                notice(
                    host,
                    session,
                    &format!("FileChanged delivery unavailable: {error}"),
                );
            }
        }
        Ok(())
    }
}

#[derive(Default)]
struct RunState {
    observer: Option<Observation>,
    failure: Option<String>,
}
impl RunState {
    async fn tick(
        &mut self,
        host: &BuiltinHost,
        path: &Path,
        directory: &str,
        membership: &Members,
        cancel: &CancellationToken,
    ) -> bool {
        let sessions = members(membership);
        if sessions.is_empty() {
            return false;
        }
        if !host.observes_files(directory, &sessions).await {
            self.observer = None;
            return true;
        }
        if self.observer.is_none() {
            self.observer =
                start_observation(host, path, directory, &sessions, &mut self.failure).await;
        }
        let Some(current) = self.observer.as_mut() else {
            return true;
        };
        current.renew(host, membership, cancel).await;
        if let Err(error) = current.reconcile(host, &sessions, membership, cancel).await {
            announce(
                host,
                &sessions,
                &format!("FileChanged generation stopped: {error}"),
            );
            // A replacement gets a new fenced baseline after bounded backoff.
            self.observer = None;
            tokio::select! { _ = cancel.cancelled() => return false, _ = tokio::time::sleep(RECONCILE) => {} }
        }
        true
    }
}

async fn run(
    host: Weak<BuiltinHost>,
    path: PathBuf,
    directory: String,
    membership: Members,
    cancel: CancellationToken,
) {
    let mut state = RunState::default();
    loop {
        tokio::select! { _ = cancel.cancelled() => break, _ = tokio::time::sleep(TICK) => {} }
        let Some(host) = host.upgrade() else {
            break;
        };
        if !state
            .tick(&host, &path, &directory, &membership, &cancel)
            .await
        {
            break;
        }
    }
}

async fn start_observation(
    host: &BuiltinHost,
    path: &Path,
    directory: &str,
    sessions: &[String],
    failure: &mut Option<String>,
) -> Option<Observation> {
    match Observation::start(host, path, directory).await {
        Ok(started) => {
            *failure = None;
            Some(started)
        }
        Err(error) => {
            if failure.as_ref() != Some(&error) {
                announce(
                    host,
                    sessions,
                    &format!("FileChanged observation unavailable: {error}"),
                );
                *failure = Some(error);
            }
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn explicit_mutations_are_scoped_coalesced_bounded_and_closed_with_the_owner() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let membership = Arc::new(Mutex::new(Membership::default()));
        let cancel = CancellationToken::new();
        let owner = cancel.clone();
        let task = tokio::spawn(async move { owner.cancelled().await });
        let observers = Observers(Mutex::new(Registry {
            entries: BTreeMap::from([(
                root.clone(),
                Entry {
                    members: Arc::clone(&membership),
                    cancel,
                    task,
                },
            )]),
            closed: false,
        }));
        observers.changed(&root, &root.join("nested/file"));
        observers.changed(&root, &root.join("nested/file"));
        observers.changed(&root, &root.parent().unwrap().join("outside"));
        {
            let members = membership.lock().unwrap();
            assert_eq!(
                members.explicit.paths,
                BTreeSet::from([PathBuf::from("nested/file")])
            );
        }
        for index in 0..1100 {
            observers.changed(&root, &root.join(format!("file-{index}")));
        }
        {
            let members = membership.lock().unwrap();
            assert_eq!(members.explicit.paths.len(), 1024);
            assert!(members.explicit.rescan);
        }
        observers.shutdown().await;
        observers.changed(&root, &root.join("after-shutdown"));
        assert!(
            !membership
                .lock()
                .unwrap()
                .explicit
                .paths
                .contains(Path::new("after-shutdown"))
        );
    }
}
