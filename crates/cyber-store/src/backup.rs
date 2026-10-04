//! Online backup and integrity checks (`storage-events` → Backup).

use std::path::Path;

use rusqlite::{Connection, OpenFlags};

use crate::StoreError;

/// Copy `source` to `dest` with SQLite's online backup API. Runs while a server writes:
/// all pages are copied in one step, so the copy is one consistent snapshot.
pub fn backup(source: &Path, dest: &Path) -> Result<(), StoreError> {
    if dest.exists() {
        return Err(StoreError::Io(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            format!("{} already exists", dest.display()),
        )));
    }
    let src = Connection::open_with_flags(
        source,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    let mut out = Connection::open(dest)?;
    {
        use rusqlite::backup::{Backup, StepResult};
        let job = Backup::new(&src, &mut out)?;
        // -1 copies every page in one step, which reads one consistent snapshot.
        loop {
            match job.step(-1)? {
                StepResult::Done => break,
                StepResult::More => {}
                _ => std::thread::sleep(std::time::Duration::from_millis(10)),
            }
        }
    }
    // A self-contained file: no WAL beside it.
    out.pragma_update(None, "journal_mode", "DELETE")?;
    Ok(())
}

/// `PRAGMA integrity_check`: `ok`, or the problems found.
pub fn integrity_check(path: &Path) -> Result<String, StoreError> {
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let rows: Vec<String> = conn
        .prepare("PRAGMA integrity_check")?
        .query_map([], |r| r.get(0))?
        .collect::<Result<_, _>>()?;
    Ok(rows.join("; "))
}

/// Rebuild the database file to reclaim space; needs exclusive access.
pub fn vacuum(path: &Path) -> Result<(), StoreError> {
    let conn = Connection::open(path)?;
    conn.execute_batch("VACUUM")?;
    Ok(())
}
