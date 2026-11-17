//! Lose only the acknowledgement of the exact terminal-outcome commit.
use std::{
    cell::RefCell,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use cyber_store::Store;

#[derive(Clone)]
struct Target {
    database: PathBuf,
    session: String,
    message: String,
    reached: Arc<AtomicBool>,
}

// Every Store owns its writer thread; parallel fixtures cannot arm one another.
thread_local! {
    static TARGET: RefCell<Option<Target>> = const { RefCell::new(None) };
}

pub fn install(store: &Store, database: &Path, session: &str, message: &str) -> Arc<AtomicBool> {
    let reached = Arc::new(AtomicBool::new(false));
    let target = Target {
        database: database.to_owned(),
        session: session.into(),
        message: message.into(),
        reached: reached.clone(),
    };
    store
        .transaction(move |connection| {
            TARGET.with(|slot| *slot.borrow_mut() = Some(target));
            connection.wal_hook(Some(lose_terminal_ack));
            Ok(())
        })
        .unwrap();
    reached
}

fn lose_terminal_ack(_: &rusqlite::hooks::Wal, _: i32) -> rusqlite::Result<()> {
    let Some(target) = TARGET.with(|slot| slot.borrow().clone()) else {
        return Ok(());
    };
    // WAL hooks run after commit. Read through a separate read-only connection,
    // never recursively through Store or through a cached runtime projection.
    let reader = rusqlite::Connection::open_with_flags(
        &target.database,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )?;
    let committed: bool = reader.query_row(
        "SELECT EXISTS(SELECT 1 FROM event WHERE aggregate_id=?1 AND type='session.structured_output.settled.1' AND json_extract(data,'$.message_id')=?2)",
        rusqlite::params![target.session, target.message], |row| row.get(0),
    )?;
    if !committed {
        return Ok(());
    }
    TARGET.with(|slot| slot.borrow_mut().take());
    target.reached.store(true, Ordering::SeqCst);
    Err(rusqlite::Error::SqliteFailure(
        rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_ERROR),
        Some("injected structured terminal commit acknowledgement loss".into()),
    ))
}
