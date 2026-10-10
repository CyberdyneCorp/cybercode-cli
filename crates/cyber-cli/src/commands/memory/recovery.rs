//! Explicit local recovery never clears durable database admission ownership.
use super::*;
use cyber_core::{memory::MemoryRecoveryReview, paths::DatabaseLocation};
use rusqlite::{Connection, OpenFlags};
#[cfg(windows)]
mod native;

pub(super) fn validate_review(review: &str) -> Result<(), CliError> {
    if review.len() != 64
        || !review
            .bytes()
            .all(|b| b.is_ascii_digit() || matches!(b, b'a'..=b'f'))
    {
        return Err(CliError::usage(
            "Recovery requires the fingerprint from memory recovery",
        ));
    }
    Ok(())
}

pub(super) fn report(
    review: Option<MemoryRecoveryReview>,
    global: &GlobalArgs,
) -> Result<(), CliError> {
    if output::is_json(global.format) {
        return output::json(&review);
    }
    let Some(review) = review else {
        println!("No memory recovery pending");
        return Ok(());
    };
    println!("Transaction: {}", review.receipt.id);
    println!(
        "{}: {}",
        if review.receipt.deleted {
            "Delete"
        } else {
            "Save"
        },
        review.receipt.name
    );
    println!("Files committed: {}", review.completed);
    println!("Review fingerprint: {}", review.fingerprint);
    if let Some(note) = review.proposed_note {
        println!(
            "Proposed note:\n{}",
            note.render_for_write()
                .map_err(|e| CliError::runtime(e.to_string()))?
        );
    }
    Ok(())
}

/// Called under the retained storage scope claim; production admission shares that claim.
/// Read-only inspection cannot migrate a database or clear unresolved execution ownership.
pub(super) fn database_admission(ctx: &Context, project: &str) -> Result<(), CliError> {
    let DatabaseLocation::File(path) = ctx.database() else {
        return Err(CliError::runtime(
            "Memory recovery cannot verify in-memory database ownership",
        ));
    };
    match std::fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.is_file() => {}
        Ok(_) => return Err(unavailable()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(_) => return Err(unavailable()),
    }
    #[cfg(windows)]
    let identity = native::DatabaseInspection::new(&path).map_err(|_| unavailable())?;
    let db = Connection::open_with_flags(
        &path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )
    .map_err(|_| unavailable())?;
    db.busy_timeout(std::time::Duration::ZERO)
        .map_err(|_| unavailable())?;
    let table: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='memory_mutation')",
        [], |row| row.get(0),
    ).map_err(|_| unavailable())?;
    if !table {
        #[cfg(windows)]
        identity.verify().map_err(|_| unavailable())?;
        return Ok(());
    }
    let pending: bool = db
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM memory_mutation WHERE project_id=?1 AND result IS NULL)",
            [project],
            |row| row.get(0),
        )
        .map_err(|_| unavailable())?;
    #[cfg(windows)]
    identity.verify().map_err(|_| unavailable())?;
    if pending {
        return Err(CliError::runtime(
            "Memory database admission is unresolved; database/file reconciliation is required",
        ));
    }
    Ok(())
}

fn unavailable() -> CliError {
    CliError::runtime("Memory recovery cannot verify database admission ownership")
}
