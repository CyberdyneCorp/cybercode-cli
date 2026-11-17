//! Read-only inventory of retained private construction stages. Names only:
//! nothing is opened, locked, adopted or removed, and the inventory grants no
//! recovery, acknowledgement or cleanup authority. A live constructor's stage
//! is listed too; liveness is decided only by native lock admission.
use super::super::{Managed, invalid, use_directory};
use std::io;

const CLAIM: &str = "preparing_";
const ACKNOWLEDGEMENT: &str = "ack_preparing_";
const LIMIT: usize = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnapshotConstructionKind {
    /// A checkout claim stage that was never published as `<owner>.lock`.
    CheckoutClaim,
    /// A checkout acknowledgement stage that may lack a stable `.snapshot-ack`.
    CheckoutAcknowledgement,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotConstruction {
    pub worktree_id: String,
    /// The constructing activity, when the stage name is well formed.
    pub owner_id: Option<String>,
    pub kind: SnapshotConstructionKind,
    pub name: String,
}

/// Owner of a `<prefix><len>_<owner>_<ulid>` construction stage name.
pub(super) fn stage_owner<'a>(name: &'a str, prefix: &str) -> io::Result<&'a str> {
    let tail = name
        .strip_prefix(prefix)
        .ok_or_else(|| invalid("Invalid construction stage"))?;
    let (length, tail) = tail
        .split_once('_')
        .ok_or_else(|| invalid("Invalid construction stage length"))?;
    let length: usize = length.parse().map_err(invalid)?;
    let owner = tail
        .get(..length)
        .ok_or_else(|| invalid("Invalid construction stage owner"))?;
    super::super::safe_id(owner, "op")?;
    let nonce = tail
        .get(length..)
        .and_then(|tail| tail.strip_prefix('_'))
        .ok_or_else(|| invalid("Invalid construction stage nonce"))?;
    if nonce.len() != 26
        || !nonce.bytes().all(|byte| byte.is_ascii_alphanumeric())
        || name != format!("{prefix}{}_{owner}_{nonce}", owner.len())
    {
        return Err(invalid("Invalid construction stage name"));
    }
    Ok(owner)
}

pub(super) fn classify(name: &str) -> Option<(SnapshotConstructionKind, Option<String>)> {
    let (kind, prefix) = if name.starts_with(ACKNOWLEDGEMENT) {
        (
            SnapshotConstructionKind::CheckoutAcknowledgement,
            ACKNOWLEDGEMENT,
        )
    } else if name.starts_with(CLAIM) {
        (SnapshotConstructionKind::CheckoutClaim, CLAIM)
    } else {
        return None;
    };
    Some((kind, stage_owner(name, prefix).ok().map(str::to_owned)))
}

/// Same-owner unknown constructor evidence fences removal until a construction
/// recovery protocol exists; this message names it so operators can find it.
pub(in crate::worktrees::activity) fn fenced(name: &str) -> io::Error {
    let owner = classify(name)
        .and_then(|(_, owner)| owner)
        .unwrap_or_else(|| "an unknown owner".into());
    invalid(format!(
        "Checkout construction {name:?} by {owner} has same-owner unknown constructor evidence; \
         it stays fenced and blocks worktree removal until construction recovery is available"
    ))
}

pub(in crate::worktrees::activity) fn inventory(
    managed: &Managed,
) -> io::Result<Vec<SnapshotConstruction>> {
    let directory = use_directory(managed, false)?;
    let entries = match std::fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(vec![]),
        Err(error) => return Err(error),
    };
    let mut found = vec![];
    for (index, entry) in entries.enumerate() {
        if index >= LIMIT {
            return Err(invalid("Checkout activity inventory exceeds limit"));
        }
        let name = entry?.file_name();
        let name = name.to_string_lossy();
        if let Some((kind, owner_id)) = classify(&name) {
            found.push(SnapshotConstruction {
                worktree_id: managed.id.clone(),
                owner_id,
                kind,
                name: name.into_owned(),
            });
        }
    }
    found.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(found)
}
