//! Durable HTTP write evidence, never live mutation authority.
use super::*;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MemoryRequestStatus {
    pub id: String,
    pub directory: PathBuf,
    pub project_id: String,
    pub name: String,
    pub deleted: bool,
    pub request_fingerprint: String,
    pub journal: Option<MemoryJournalIdentity>,
    pub completed: Option<MemoryChange>,
}

/// Public durable admission identity for an HTTP request key; it grants no ownership.
pub fn memory_http_request_id(key: &str) -> String {
    format!("mwr_{:x}", Sha256::digest(format!("http:{key}").as_bytes()))
}

pub(in crate::runtime) fn status(
    store: &Store,
    key: &str,
) -> Result<Option<MemoryRequestStatus>, StoreError> {
    if !(1..=128).contains(&key.len()) || !key.bytes().all(|b| (0x21..=0x7e).contains(&b)) {
        return Err(refusal("Invalid memory request key"));
    }
    let id = memory_http_request_id(key);
    store.read(move |db| {
        let row: Option<(String, String, String, String, Option<String>)> = db
            .query_row(
                "SELECT project_id,request_hash,owner_hash,data,result FROM memory_mutation WHERE id=?1",
                [&id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
            )
            .optional()?;
        let Some((project, digest, owner, data, result)) = row else {
            return Ok(None);
        };
        let request: Request = serde_json::from_str(&data)
            .map_err(|_| refusal("Invalid persisted memory admission"))?;
        validate(&request, &id, &project, &digest, &owner)?;
        let completed: Option<MemoryChange> = result
            .map(|data| serde_json::from_str(&data))
            .transpose()
            .map_err(|_| refusal("Invalid persisted memory receipt"))?;
        if completed.as_ref().is_some_and(|change| {
            change.id != request.id
                || change.directory != request.directory
                || change.project_id != request.project_id
                || request.journal.as_ref().map(|journal| &journal.receipt) != Some(&change.receipt)
        }) {
            return Err(refusal("Memory receipt does not match admitted evidence"));
        }
        Ok(Some(MemoryRequestStatus {
            id: request.id,
            directory: request.directory,
            project_id: request.project_id,
            name: request.name,
            deleted: request.deleted,
            request_fingerprint: request.http_hash.expect("validated HTTP digest"),
            journal: request.journal,
            completed,
        }))
    })
}

fn validate(
    request: &Request,
    id: &str,
    project: &str,
    digest: &str,
    owner: &str,
) -> Result<(), StoreError> {
    let http = request
        .http_hash
        .as_deref()
        .ok_or_else(|| refusal("Memory admission has no HTTP request evidence"))?;
    if request.id != id
        || request.project_id != project
        || request.request_hash != digest
        || request.owner_hash != owner
        || !request.directory.is_absolute()
        || !valid_hash(owner)
        || !valid_hash(&format!("sha256:{http}"))
        || request_hash(
            &request.directory,
            project,
            &request.name,
            request.deleted,
            http,
        ) != digest
    {
        return Err(refusal("Inconsistent memory request evidence"));
    }
    cyber_core::memory::validate_name(&request.name)
        .map_err(|_| refusal("Invalid memory request name"))?;
    cyber_core::memory::directory(Path::new("/"), project)
        .map_err(|_| refusal("Invalid memory request scope"))?;
    if request.journal.as_ref().is_some_and(|journal| {
        journal.receipt.name != request.name
            || journal.receipt.deleted != request.deleted
            || !journal.receipt.id.starts_with("mem_")
            || !valid_hash(&format!("sha256:{}", journal.intent_fingerprint))
    }) {
        return Err(refusal("Memory journal does not match admitted evidence"));
    }
    Ok(())
}
