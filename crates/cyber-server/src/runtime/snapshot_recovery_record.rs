//! Exact durable native owners; a Session-wide unknown outcome is never an acknowledgement target.
use super::{SnapshotOrigin, activity};
use cyber_store::StoreError;
use rusqlite::{Connection, OptionalExtension};

pub(super) const FENCED: &str = "Native snapshot ownership fences ordinary native admission";

pub(super) fn check_session_admission(
    db: &Connection,
    source: &str,
    exclude: &str,
) -> Result<(), String> {
    let fenced: bool = db.query_row("SELECT EXISTS(SELECT 1 FROM event e WHERE e.type='native.activity.changed.1' AND e.aggregate_id<>?1 AND json_extract(e.data,'$.session_id')=?2 AND json_extract(e.data,'$.snapshot_worker')=1 AND json_extract(e.data,'$.status')<>'settled' AND NOT EXISTS(SELECT 1 FROM event n WHERE n.aggregate_id=e.aggregate_id AND n.seq>e.seq))", [exclude, source], |row| row.get(0)).map_err(|error| error.to_string())?;
    if fenced {
        return Err(FENCED.into());
    }
    Ok(())
}

pub(super) fn check_ordinary_admission(
    db: &Connection,
    event: &cyber_store::StoredEvent,
) -> Result<(), String> {
    if event.kind != activity::CHANGED || event.data["status"] != "pending" {
        return Ok(());
    }
    let source = event.data["session_id"]
        .as_str()
        .ok_or("Missing native activity source")?;
    let snapshot = event.data["snapshot_worker"] == true;
    let fenced: bool = db.query_row("SELECT EXISTS(SELECT 1 FROM event e WHERE e.type='native.activity.changed.1' AND e.aggregate_id<>?1 AND json_extract(e.data,'$.session_id')=?2 AND json_extract(e.data,'$.status')<>'settled' AND (?3 OR json_extract(e.data,'$.snapshot_worker')=1) AND NOT EXISTS(SELECT 1 FROM event n WHERE n.aggregate_id=e.aggregate_id AND n.seq>e.seq))", rusqlite::params![event.aggregate_id, source, snapshot], |row| row.get(0)).map_err(|error| error.to_string())?;
    if fenced {
        return Err(FENCED.into());
    }
    Ok(())
}

/// The only closed-boundary admission exception is an exact, reserved snapshot
/// resolution. This cannot authorize an ordinary hook/process/child launch.
pub(super) fn project_pending(
    db: &Connection,
    event: &cyber_store::StoredEvent,
) -> Result<bool, String> {
    if event.kind != activity::CHANGED
        || event.data["status"] != "pending"
        || event.data["snapshot_request"].is_null()
    {
        return Ok(false);
    }
    let record: activity::Record =
        serde_json::from_value(event.data.clone()).map_err(|error| error.to_string())?;
    let request = record
        .snapshot_request
        .as_ref()
        .ok_or("Missing recovery request")?;
    super::snapshot_recovery::validate_request(&request.confirmation)
        .map_err(|error| error.to_string())?;
    let authority = request
        .authority
        .as_ref()
        .ok_or("Missing captured recovery authority")?;
    let (_, original) = attest(db, &request.original).map_err(|error| error.to_string())?;
    let created: String = db
        .query_row(
            "SELECT id FROM event WHERE aggregate_id=?1 AND type=?2 ORDER BY seq LIMIT 1",
            [&record.session_id, super::events::CREATED],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    if !record.snapshot_worker
        || record.binary_document.is_some()
        || record.snapshot_receipt.is_some()
        || record.snapshot_preparation_receipt.is_some()
        || original.status == "settled"
        || request
            .original
            .binding
            .lock_identity
            .as_deref()
            .is_none_or(str::is_empty)
        || record.session_id != request.original.session_id
        || created != request.original.session_created_id
        || event.aggregate_id == request.original.activity_id
        || event.aggregate_id
            != super::snapshot_recovery::request_owner(&created, &request.confirmation.request_id)
        || record.admission_bindings.as_deref() != Some(authority.bindings())
        || authority
            .bindings()
            .first()
            .map(|binding| binding.id.as_str())
            != Some(record.session_id.as_str())
        || !authority.current(db).map_err(|error| error.to_string())?
    {
        return Err("Invalid snapshot-only recovery reservation authority".into());
    }
    match event.seq {
        0 if record.phase == "reserved" && record.snapshot_origin.is_none() => {}
        1 if record.phase == "launching" => {
            let data: String = db
                .query_row(
                    "SELECT data FROM event WHERE aggregate_id=?1 AND seq=0 AND type=?2",
                    [&event.aggregate_id, activity::CHANGED],
                    |row| row.get(0),
                )
                .map_err(|error| error.to_string())?;
            let reserved: activity::Record =
                serde_json::from_str(&data).map_err(|error| error.to_string())?;
            let origin = record
                .snapshot_origin
                .as_ref()
                .ok_or("Missing resolution origin")?;
            if reserved.snapshot_request != record.snapshot_request
                || reserved.snapshot_lifetime != record.snapshot_lifetime
                || reserved.snapshot_source_admitted != record.snapshot_source_admitted
                || reserved.status != "pending"
                || origin.activity_id != event.aggregate_id
                || origin.session_id != record.session_id
                || origin.session_created_id != created
                || origin.worktree_id != request.original.worktree_id
                || origin.binding.directory != request.original.binding.directory
                || origin.binding.root_identity != request.original.binding.root_identity
                || origin.binding.namespace_identity != request.original.binding.namespace_identity
                || origin.binding.lock_identity.is_none()
                || origin.binding.lock_identity != request.original.binding.lock_identity
                || origin.binding.journal_id == request.original.binding.journal_id
            {
                return Err("Resolution launch differs from its exact request reservation".into());
            }
        }
        2 if record.phase == "snapshot_source_admitted"
            && record.snapshot_source_admitted == Some(true) =>
        {
            let data: String = db
                .query_row(
                    "SELECT data FROM event WHERE aggregate_id=?1 AND seq=1 AND type=?2",
                    [&event.aggregate_id, activity::CHANGED],
                    |row| row.get(0),
                )
                .map_err(|error| error.to_string())?;
            let launched: activity::Record =
                serde_json::from_str(&data).map_err(|error| error.to_string())?;
            if launched.status != "pending"
                || launched.phase != "launching"
                || launched.snapshot_source_admitted != Some(false)
                || launched.snapshot_origin != record.snapshot_origin
                || launched.snapshot_lifetime != record.snapshot_lifetime
                || launched.snapshot_request != record.snapshot_request
            {
                return Err("Snapshot source admission differs from its exact launch".into());
            }
        }
        _ => return Err("Invalid snapshot recovery launch sequence".into()),
    }
    Ok(true)
}

pub(super) fn attest(
    db: &Connection,
    origin: &SnapshotOrigin,
) -> Result<(i64, activity::Record), StoreError> {
    let launched = db
        .query_row(
            "SELECT data FROM event WHERE aggregate_id=?1 AND seq=1 AND type=?2",
            [&origin.activity_id, activity::CHANGED],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    let latest = db
        .query_row(
            "SELECT seq,type,data FROM event WHERE aggregate_id=?1 ORDER BY seq DESC LIMIT 1",
            [&origin.activity_id],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )
        .optional()?;
    let Some((launched, (seq, kind, latest))) = launched.zip(latest) else {
        return Err(StoreError::InvalidAppend(
            "Exact snapshot origin is unavailable".into(),
        ));
    };
    let launched: activity::Record = serde_json::from_str(&launched)
        .map_err(|error| StoreError::InvalidAppend(error.to_string()))?;
    let latest: activity::Record = serde_json::from_str(&latest)
        .map_err(|error| StoreError::InvalidAppend(error.to_string()))?;
    if kind != activity::CHANGED
        || [&launched, &latest].iter().any(|record| {
            !record.snapshot_worker
                || record.binary_document.is_some()
                || record.session_id != origin.session_id
                || record.snapshot_origin.as_ref() != Some(origin)
        })
        || launched.snapshot_request != latest.snapshot_request
        || launched.snapshot_lifetime != latest.snapshot_lifetime
        || !matches!(latest.status.as_str(), "pending" | "unknown" | "settled")
    {
        return Err(StoreError::InvalidAppend(
            "Native activity differs from the exact snapshot origin".into(),
        ));
    }
    source_admission(db, origin, &launched, &latest)?;
    Ok((seq, latest))
}

fn source_admission(
    db: &Connection,
    origin: &SnapshotOrigin,
    launched: &activity::Record,
    latest: &activity::Record,
) -> Result<(), StoreError> {
    let invalid = || StoreError::InvalidAppend("Snapshot source-admission evidence changed".into());
    match (
        launched.snapshot_source_admitted,
        latest.snapshot_source_admitted,
    ) {
        (None, None) => return Ok(()),
        (Some(false), Some(false) | Some(true)) => {}
        _ => return Err(invalid()),
    }
    let data: String = db.query_row(
        "SELECT data FROM event WHERE aggregate_id=?1 AND seq=0 AND type=?2",
        [&origin.activity_id, activity::CHANGED],
        |row| row.get(0),
    )?;
    let reserved: activity::Record = serde_json::from_str(&data).map_err(|_| invalid())?;
    if reserved.snapshot_source_admitted != Some(false)
        || !reserved.snapshot_worker
        || reserved.binary_document.is_some()
        || reserved.status != "pending"
        || reserved.phase != "reserved"
        || reserved.session_id != origin.session_id
        || reserved.snapshot_request != launched.snapshot_request
        || reserved.snapshot_lifetime.is_none()
        || reserved
            .snapshot_lifetime
            .as_ref()
            .is_some_and(|lifetime| lifetime.version != 2)
        || reserved.snapshot_lifetime != launched.snapshot_lifetime
        || reserved.snapshot_origin.is_some()
    {
        return Err(invalid());
    }
    if latest.snapshot_source_admitted == Some(false) {
        let admitted: bool = db.query_row("SELECT EXISTS(SELECT 1 FROM event WHERE aggregate_id=?1 AND json_extract(data,'$.snapshot_source_admitted')=1)", [&origin.activity_id], |row| row.get(0))?;
        if admitted {
            return Err(invalid());
        }
    } else {
        let data: String = db.query_row(
            "SELECT data FROM event WHERE aggregate_id=?1 AND seq=2 AND type=?2",
            [&origin.activity_id, activity::CHANGED],
            |row| row.get(0),
        )?;
        let admitted: activity::Record = serde_json::from_str(&data).map_err(|_| invalid())?;
        if admitted.phase != "snapshot_source_admitted"
            || admitted.status != "pending"
            || admitted.snapshot_source_admitted != Some(true)
            || admitted.snapshot_origin.as_ref() != Some(origin)
            || admitted.snapshot_lifetime != launched.snapshot_lifetime
            || admitted.snapshot_request != launched.snapshot_request
        {
            return Err(invalid());
        }
    }
    Ok(())
}
