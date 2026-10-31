//! The bounded writer queue (`storage-events` → Local writer ownership and backpressure).
//!
//! One thread owns the write connection. Callers submit jobs; admission waits at most the
//! configured timeout and then fails with `StorageBusyError` without acknowledging anything.
//! A fatal storage error stops every later mutation.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

use crossbeam_channel::{SendTimeoutError, Sender, bounded};
use rusqlite::{Connection, Transaction, TransactionBehavior};

use crate::error::StoreError;

type Job = Box<dyn FnOnce(&mut Connection) + Send>;

pub(crate) struct Writer {
    sender: Option<Sender<Job>>,
    handle: Option<JoinHandle<()>>,
    failed: Arc<AtomicBool>,
    timeout: Duration,
}

impl Writer {
    pub fn spawn(conn: Connection, capacity: usize, timeout: Duration) -> Result<Self, StoreError> {
        let (sender, receiver) = bounded::<Job>(capacity);
        let handle = std::thread::Builder::new()
            .name("cyber-writer".into())
            .spawn(move || {
                let mut conn = conn;
                for job in receiver {
                    job(&mut conn);
                }
            })?;
        Ok(Self {
            sender: Some(sender),
            handle: Some(handle),
            failed: Arc::default(),
            timeout,
        })
    }

    /// Run `f` on the writer thread and wait for its result.
    pub fn submit<T, F>(&self, f: F) -> Result<T, StoreError>
    where
        T: Send + 'static,
        F: FnOnce(&mut Connection) -> Result<T, StoreError> + Send + 'static,
    {
        if self.failed.load(Ordering::SeqCst) {
            return Err(stopped());
        }
        let (reply, result) = bounded(1);
        let failed = Arc::clone(&self.failed);
        let job: Job = Box::new(move |conn| {
            let outcome = if failed.load(Ordering::SeqCst) {
                Err(stopped())
            } else {
                f(conn)
            };
            if outcome.as_ref().is_err_and(StoreError::is_fatal) {
                failed.store(true, Ordering::SeqCst);
            }
            let _ = reply.send(outcome);
        });
        let sender = self
            .sender
            .as_ref()
            .ok_or_else(|| StoreError::Unavailable("writer stopped".into()))?;
        sender
            .send_timeout(job, self.timeout)
            .map_err(|e| match e {
                SendTimeoutError::Timeout(_) => StoreError::Busy,
                SendTimeoutError::Disconnected(_) => {
                    StoreError::Unavailable("writer stopped".into())
                }
            })?;
        result
            .recv()
            .map_err(|_| StoreError::Unavailable("writer stopped before replying".into()))?
    }

    /// Run `f` in an IMMEDIATE transaction; commit only when it succeeds.
    pub fn transaction<T, F>(&self, f: F) -> Result<T, StoreError>
    where
        T: Send + 'static,
        F: FnOnce(&Transaction<'_>) -> Result<T, StoreError> + Send + 'static,
    {
        self.submit(move |conn| {
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let value = f(&tx)?;
            // Inspect the transaction before its Drop can roll it back. SQLite
            // can report an error after COMMIT completed (for example, a WAL
            // hook error); that outcome must fence the writer regardless of code.
            if let Err(error) = tx.execute_batch("COMMIT") {
                if tx.is_autocommit() {
                    return Err(StoreError::Unavailable(format!(
                        "commit outcome unknown: {error}"
                    )));
                }
                return Err(error.into());
            }
            Ok(value)
        })
    }

    pub fn queue_depth(&self) -> usize {
        self.sender.as_ref().map_or(0, Sender::len)
    }

    pub fn has_failed(&self) -> bool {
        self.failed.load(Ordering::SeqCst)
    }
}

impl Drop for Writer {
    fn drop(&mut self) {
        self.sender.take();
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

fn stopped() -> StoreError {
    StoreError::Unavailable("a previous storage failure stopped new mutations".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    #[test]
    fn full_queue_times_out_with_busy() {
        let writer = Arc::new(
            Writer::spawn(
                Connection::open_in_memory().unwrap(),
                1,
                Duration::from_millis(100),
            )
            .unwrap(),
        );
        let (started_tx, started) = bounded::<()>(1);
        let (release, gate) = bounded::<()>(1);

        let blocker = {
            let w = Arc::clone(&writer);
            std::thread::spawn(move || {
                w.submit(move |_| {
                    started_tx.send(()).unwrap();
                    gate.recv().unwrap();
                    Ok(())
                })
            })
        };
        started.recv().unwrap();
        let queued = {
            let w = Arc::clone(&writer);
            std::thread::spawn(move || w.submit(|_| Ok(7)))
        };
        let deadline = Instant::now() + Duration::from_secs(5);
        while writer.queue_depth() < 1 && Instant::now() < deadline {
            std::thread::yield_now();
        }

        assert!(matches!(writer.submit(|_| Ok(())), Err(StoreError::Busy)));

        release.send(()).unwrap();
        assert!(blocker.join().unwrap().is_ok());
        assert_eq!(queued.join().unwrap().unwrap(), 7);
    }

    #[test]
    fn committed_wal_error_stops_later_mutations() {
        fn lost_ack(_: &rusqlite::hooks::Wal, _: i32) -> rusqlite::Result<()> {
            Err(rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_ERROR),
                Some("injected lost commit acknowledgement".into()),
            ))
        }

        let directory = tempfile::tempdir().unwrap();
        let database = directory.path().join("writer.db");
        let conn = Connection::open(&database).unwrap();
        conn.execute_batch("PRAGMA journal_mode=WAL; CREATE TABLE committed (value INTEGER)")
            .unwrap();
        let writer = Writer::spawn(conn, 1, Duration::from_secs(1)).unwrap();
        writer
            .transaction(|tx| {
                tx.wal_hook(Some(lost_ack));
                Ok(())
            })
            .unwrap();
        let result = writer.transaction(|tx| {
            tx.execute("INSERT INTO committed VALUES (7)", [])?;
            Ok(7)
        });
        // A second connection proves this was committed, not a before-write fault.
        let reader = Connection::open(&database).unwrap();
        assert_eq!(
            reader
                .query_row("SELECT value FROM committed", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            7
        );
        assert!(
            matches!(result, Err(StoreError::Unavailable(_))),
            "an unacknowledged completed transaction was not fatal: {result:?}"
        );
        assert!(writer.has_failed());
        assert!(matches!(
            writer.transaction(|tx| {
                tx.execute("INSERT INTO committed VALUES (8)", [])?;
                Ok(())
            }),
            Err(StoreError::Unavailable(_))
        ));
        assert_eq!(
            reader
                .query_row("SELECT count(*) FROM committed", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            1
        );
    }

    #[test]
    fn rejected_commit_rolls_back_without_stopping_the_writer() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "PRAGMA foreign_keys=ON;
             CREATE TABLE parent (id INTEGER PRIMARY KEY);
             CREATE TABLE child (parent_id INTEGER REFERENCES parent(id)
                 DEFERRABLE INITIALLY DEFERRED)",
        )
        .unwrap();
        let writer = Writer::spawn(conn, 1, Duration::from_secs(1)).unwrap();
        let result = writer.transaction(|tx| {
            tx.execute("INSERT INTO child VALUES (7)", [])?;
            Ok(())
        });
        assert!(matches!(result, Err(StoreError::Sqlite(_))), "{result:?}");
        assert!(!writer.has_failed());
        assert_eq!(
            writer
                .submit(|conn| Ok(
                    conn.query_row("SELECT count(*) FROM child", [], |row| row.get::<_, i64>(0))?
                ))
                .unwrap(),
            0
        );
        writer
            .transaction(|tx| {
                tx.execute("INSERT INTO parent VALUES (7)", [])?;
                tx.execute("INSERT INTO child VALUES (7)", [])?;
                Ok(())
            })
            .unwrap();
        assert!(!writer.has_failed());
    }
}
