//! The store: open with durability settings, append, replay and tail.

use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use crossbeam_channel::{Receiver, RecvTimeoutError, Sender, unbounded};
use cyber_core::paths::DatabaseLocation;
use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use serde::Serialize;
use serde_json::Value;

use crate::error::StoreError;
use crate::events::{EventRegistry, Expected, NewEvent, StoredEvent, append_in_tx};
use crate::migrate::{self, MIGRATIONS, Outcome};
use crate::writer::Writer;
use crate::{MAX_PAGE_LIMIT, fs_check};

const BUSY_TIMEOUT: Duration = Duration::from_millis(5000);

pub struct StoreOptions {
    pub location: DatabaseLocation,
    pub registry: EventRegistry,
    /// Writer queue capacity (default 1024 transactions).
    pub queue_capacity: usize,
    /// Admission timeout before `StorageBusyError` (default 5 s).
    pub admission_timeout: Duration,
    /// Test hook: cap the database size to simulate a full disk.
    #[doc(hidden)]
    pub max_page_count: Option<u32>,
}

impl StoreOptions {
    pub fn new(location: DatabaseLocation, registry: EventRegistry) -> Self {
        Self {
            location,
            registry,
            queue_capacity: 1024,
            admission_timeout: Duration::from_secs(5),
            max_page_count: None,
        }
    }
}

/// `full` for file databases in WAL mode with `synchronous=FULL`; `ephemeral` in memory.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Durability {
    Full,
    Ephemeral,
}

/// Effective durability settings of the writer connection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Pragmas {
    pub journal_mode: String,
    pub synchronous: i64,
    pub foreign_keys: bool,
}

/// One page of an aggregate's events.
#[derive(Debug, Clone, Serialize)]
pub struct EventPage {
    pub events: Vec<StoredEvent>,
    pub has_more: bool,
}

/// Notification that an aggregate's events up to `seq` committed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Committed {
    pub aggregate_id: String,
    pub seq: i64,
}

pub struct Store {
    writer: Writer,
    reader: Option<Mutex<Connection>>,
    registry: Arc<EventRegistry>,
    subscribers: Mutex<Vec<Sender<Committed>>>,
    durability: Durability,
    too_new: Option<Vec<String>>,
}

impl Store {
    pub fn open(options: StoreOptions) -> Result<Self, StoreError> {
        let mut conn = open_writer(&options)?;
        let too_new = match migrate::migrate(&mut conn, MIGRATIONS)? {
            Outcome::Current => None,
            Outcome::TooNew(ids) => Some(ids),
        };
        let (reader, durability) = match &options.location {
            DatabaseLocation::File(path) => {
                (Some(Mutex::new(open_reader(path)?)), Durability::Full)
            }
            DatabaseLocation::Memory => (None, Durability::Ephemeral),
        };
        Ok(Self {
            writer: Writer::spawn(conn, options.queue_capacity, options.admission_timeout)?,
            reader,
            registry: Arc::new(options.registry),
            subscribers: Mutex::default(),
            durability,
            too_new,
        })
    }

    pub fn durability(&self) -> Durability {
        self.durability
    }

    /// Unknown migration IDs when the database was written by a newer version.
    pub fn too_new(&self) -> Option<&[String]> {
        self.too_new.as_deref()
    }

    pub fn queue_depth(&self) -> usize {
        self.writer.queue_depth()
    }

    /// Whether a fatal storage error has stopped new mutations.
    pub fn has_failed(&self) -> bool {
        self.writer.has_failed()
    }

    /// Run `f` in an IMMEDIATE transaction on the writer. The result is returned only after
    /// commit. `f` must not perform network I/O, tool execution or user waits.
    pub fn transaction<T, F>(&self, f: F) -> Result<T, StoreError>
    where
        T: Send + 'static,
        F: FnOnce(&rusqlite::Transaction<'_>) -> Result<T, StoreError> + Send + 'static,
    {
        if let Some(ids) = &self.too_new {
            return Err(StoreError::TooNew(ids.clone()));
        }
        self.writer.transaction(f)
    }

    /// Append events to one aggregate with gapless sequence numbers.
    pub fn append(
        &self,
        aggregate_id: &str,
        expected: Expected,
        events: Vec<NewEvent>,
    ) -> Result<Vec<StoredEvent>, StoreError> {
        for event in &events {
            self.registry.check(event)?;
        }
        let registry = Arc::clone(&self.registry);
        let aggregate = aggregate_id.to_string();
        let stored =
            self.transaction(move |tx| append_in_tx(tx, &registry, &aggregate, expected, events))?;
        self.notify(&stored);
        Ok(stored)
    }

    /// Decide and append events in one writer transaction. The callback must not perform
    /// external effects; projection and the decision either commit together or roll back.
    pub fn append_checked<T, F>(
        &self,
        aggregate_id: &str,
        expected: Expected,
        decide: F,
    ) -> Result<(Vec<StoredEvent>, T), StoreError>
    where
        T: Send + 'static,
        F: FnOnce(&rusqlite::Transaction<'_>) -> Result<(Vec<NewEvent>, T), StoreError>
            + Send
            + 'static,
    {
        let registry = Arc::clone(&self.registry);
        let aggregate = aggregate_id.to_owned();
        let (stored, result) = self.transaction(move |tx| {
            let (events, result) = decide(tx)?;
            for event in &events {
                registry.check(event)?;
            }
            let stored = append_in_tx(tx, &registry, &aggregate, expected, events)?;
            Ok((stored, result))
        })?;
        self.notify(&stored);
        Ok((stored, result))
    }

    /// Events of `aggregate_id` with `seq > after` (use `-1` for all), upcast to current versions.
    pub fn read_events(
        &self,
        aggregate_id: &str,
        after: i64,
        limit: u32,
    ) -> Result<EventPage, StoreError> {
        if limit == 0 || limit > MAX_PAGE_LIMIT {
            return Err(StoreError::InvalidLimit(limit));
        }
        let aggregate = aggregate_id.to_string();
        let rows = self.read(move |conn| read_rows(conn, &aggregate, after, limit + 1))?;
        let has_more = rows.len() > limit as usize;
        let events = rows
            .into_iter()
            .take(limit as usize)
            .map(|row| self.upcast(row))
            .collect::<Result<_, _>>()?;
        Ok(EventPage { events, has_more })
    }

    /// Latest committed seq of an aggregate.
    pub fn aggregate_seq(&self, aggregate_id: &str) -> Result<Option<i64>, StoreError> {
        let aggregate = aggregate_id.to_string();
        self.read(move |conn| {
            Ok(conn
                .query_row(
                    "SELECT seq FROM event_sequence WHERE aggregate_id = ?1",
                    [aggregate],
                    |r| r.get(0),
                )
                .optional()?)
        })
    }

    /// Commit notifications for every aggregate, delivered after commit.
    pub fn subscribe(&self) -> Receiver<Committed> {
        let (tx, rx) = unbounded();
        self.subscribers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(tx);
        rx
    }

    /// Replay events after `after`, then follow new commits without gaps.
    pub fn tail(&self, aggregate_id: &str, after: i64) -> Tail<'_> {
        Tail {
            store: self,
            aggregate_id: aggregate_id.to_string(),
            last: after,
            commits: self.subscribe(),
        }
    }

    /// Durability settings of the writer connection.
    pub fn pragmas(&self) -> Result<Pragmas, StoreError> {
        self.writer.submit(|conn| {
            Ok(Pragmas {
                journal_mode: conn.query_row("PRAGMA journal_mode", [], |r| r.get(0))?,
                synchronous: conn.query_row("PRAGMA synchronous", [], |r| r.get(0))?,
                foreign_keys: conn.query_row("PRAGMA foreign_keys", [], |r| r.get::<_, i64>(0))?
                    == 1,
            })
        })
    }

    /// Run a read-only query on the reader connection (or the writer for in-memory stores).
    pub fn read<T, F>(&self, f: F) -> Result<T, StoreError>
    where
        T: Send + 'static,
        F: FnOnce(&Connection) -> Result<T, StoreError> + Send + 'static,
    {
        match &self.reader {
            Some(reader) => f(&reader.lock().unwrap_or_else(PoisonError::into_inner)),
            // In-memory databases are private to the writer connection.
            None => self.writer.submit(move |conn| f(conn)),
        }
    }

    fn notify(&self, events: &[StoredEvent]) {
        let Some(last) = events.last() else { return };
        let commit = Committed {
            aggregate_id: last.aggregate_id.clone(),
            seq: last.seq,
        };
        self.subscribers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .retain(|s| s.send(commit.clone()).is_ok());
    }

    fn upcast(&self, row: RawEvent) -> Result<StoredEvent, StoreError> {
        let data: Value =
            serde_json::from_str(&row.data).map_err(|e| StoreError::CorruptEvent {
                id: row.id.clone(),
                reason: e.to_string(),
            })?;
        let (kind, data) = self.registry.upcast(&row.kind, data);
        Ok(StoredEvent {
            id: row.id,
            aggregate_id: row.aggregate_id,
            seq: row.seq,
            kind,
            data,
            time_ms: row.time_ms,
            causation_id: row.causation_id,
        })
    }
}

/// A gapless follower of one aggregate (`storage-events` → Event replay and subscription).
pub struct Tail<'a> {
    store: &'a Store,
    aggregate_id: String,
    last: i64,
    commits: Receiver<Committed>,
}

impl Tail<'_> {
    /// The next events after the last one returned, waiting up to `timeout` for a commit.
    /// Returns an empty batch on timeout.
    pub fn next_batch(&mut self, timeout: Duration) -> Result<Vec<StoredEvent>, StoreError> {
        loop {
            let page = self
                .store
                .read_events(&self.aggregate_id, self.last, MAX_PAGE_LIMIT)?;
            if let Some(last) = page.events.last() {
                self.last = last.seq;
                return Ok(page.events);
            }
            match self.commits.recv_timeout(timeout) {
                Ok(_) => continue,
                Err(RecvTimeoutError::Timeout) => return Ok(Vec::new()),
                Err(RecvTimeoutError::Disconnected) => {
                    return Err(StoreError::Unavailable("store closed".into()));
                }
            }
        }
    }

    pub fn last_seq(&self) -> i64 {
        self.last
    }
}

struct RawEvent {
    id: String,
    aggregate_id: String,
    seq: i64,
    kind: String,
    data: String,
    time_ms: i64,
    causation_id: Option<String>,
}

fn read_rows(
    conn: &Connection,
    aggregate: &str,
    after: i64,
    limit: u32,
) -> Result<Vec<RawEvent>, StoreError> {
    let mut stmt = conn.prepare_cached(
        "SELECT id, aggregate_id, seq, type, data, time, causation_id FROM event
         WHERE aggregate_id = ?1 AND seq > ?2 ORDER BY seq LIMIT ?3",
    )?;
    let rows = stmt
        .query_map(params![aggregate, after, limit], |r| {
            Ok(RawEvent {
                id: r.get(0)?,
                aggregate_id: r.get(1)?,
                seq: r.get(2)?,
                kind: r.get(3)?,
                data: r.get(4)?,
                time_ms: r.get(5)?,
                causation_id: r.get(6)?,
            })
        })?
        .collect::<Result<_, _>>()?;
    Ok(rows)
}

fn open_writer(options: &StoreOptions) -> Result<Connection, StoreError> {
    let conn = match &options.location {
        DatabaseLocation::Memory => Connection::open_in_memory()?,
        DatabaseLocation::File(path) => {
            prepare_parent(path)?;
            Connection::open(path)?
        }
    };
    let wal = matches!(options.location, DatabaseLocation::File(_));
    configure_writer(&conn, wal, options.max_page_count)?;
    Ok(conn)
}

fn prepare_parent(path: &Path) -> Result<(), StoreError> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    std::fs::create_dir_all(parent)?;
    match fs_check::network_filesystem(parent) {
        Some(fs) => Err(StoreError::NetworkFilesystem(fs)),
        None => Ok(()),
    }
}

/// Every writer connection sets WAL, FULL synchronous and foreign keys, and verifies them.
fn configure_writer(
    conn: &Connection,
    wal: bool,
    max_page_count: Option<u32>,
) -> Result<(), StoreError> {
    conn.busy_timeout(BUSY_TIMEOUT)?;
    if wal {
        let mode: String = conn.query_row("PRAGMA journal_mode = WAL", [], |r| r.get(0))?;
        if !mode.eq_ignore_ascii_case("wal") {
            return Err(StoreError::Unavailable(format!(
                "could not enable WAL (journal_mode={mode})"
            )));
        }
    }
    conn.execute_batch(
        "PRAGMA synchronous = FULL; PRAGMA cache_size = -64000; PRAGMA foreign_keys = ON;",
    )?;
    let synchronous: i64 = conn.query_row("PRAGMA synchronous", [], |r| r.get(0))?;
    if synchronous != 2 {
        return Err(StoreError::Unavailable(format!(
            "synchronous is {synchronous}, expected FULL (2)"
        )));
    }
    if wal {
        conn.query_row("PRAGMA wal_checkpoint(PASSIVE)", [], |_| Ok(()))?;
    }
    if let Some(pages) = max_page_count {
        conn.query_row(&format!("PRAGMA max_page_count = {pages}"), [], |_| Ok(()))?;
    }
    Ok(())
}

fn open_reader(path: &Path) -> Result<Connection, StoreError> {
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    conn.busy_timeout(BUSY_TIMEOUT)?;
    conn.execute_batch("PRAGMA query_only = ON; PRAGMA cache_size = -64000;")?;
    Ok(conn)
}
