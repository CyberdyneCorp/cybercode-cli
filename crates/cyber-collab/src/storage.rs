//! Dedicated hub database. Every accepted mutation is committed by the bounded
//! FULL/WAL writer before its response is visible to a client.
pub(crate) mod visibility;
use crate::{error::Error, ids::*, model::*};
use cyber_account::IdentityLease;
use cyber_core::paths::DatabaseLocation;
use cyber_store::{EventRegistry, OwnershipLock, Store, StoreError, StoreOptions};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    path::Path,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
pub fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
pub fn encode<T: Serialize>(value: &T) -> Result<String, Error> {
    serde_json::to_string(value).map_err(|_| Error::storage())
}
pub fn decode<T: DeserializeOwned>(value: &str) -> Result<T, Error> {
    serde_json::from_str(value).map_err(|_| Error::storage())
}
impl From<rusqlite::Error> for Error {
    fn from(_: rusqlite::Error) -> Self {
        Self::storage()
    }
}

pub struct Database {
    store: Store,
    _lock: OwnershipLock,
    pub hub_id: HubId,
}
impl Database {
    pub fn open(path: &Path) -> Result<Self, Error> {
        private_database(path)?;
        let lock = OwnershipLock::acquire(&path.with_extension("hub.lock"))?;
        let mut options = StoreOptions::new(
            DatabaseLocation::File(path.into()),
            EventRegistry::default(),
        );
        options.queue_capacity = 128;
        let store = Store::open(options)?;
        let hub_id = store.transaction(|tx| {
            tx.execute_batch("CREATE TABLE IF NOT EXISTS ch_objects(kind TEXT NOT NULL,id TEXT NOT NULL,project TEXT NOT NULL,body TEXT NOT NULL,PRIMARY KEY(kind,id)); CREATE INDEX IF NOT EXISTS ch_scope ON ch_objects(kind,project); CREATE TABLE IF NOT EXISTS ch_operations(actor TEXT NOT NULL,operation TEXT NOT NULL,digest TEXT NOT NULL,result TEXT NOT NULL,created INTEGER NOT NULL,PRIMARY KEY(actor,operation)); CREATE TABLE IF NOT EXISTS ch_events(cursor INTEGER PRIMARY KEY AUTOINCREMENT,project TEXT NOT NULL,kind TEXT NOT NULL,body TEXT NOT NULL,created INTEGER NOT NULL); CREATE INDEX IF NOT EXISTS ch_event_project ON ch_events(project,cursor); CREATE TABLE IF NOT EXISTS ch_meta(key TEXT PRIMARY KEY,value TEXT NOT NULL); CREATE INDEX IF NOT EXISTS ch_delivery_pending ON ch_objects(project,json_extract(body,'$.target'),json_extract(body,'$.state'),id) WHERE kind='delivery'; CREATE INDEX IF NOT EXISTS ch_delivery_expiry ON ch_objects(project,json_extract(body,'$.state'),json_extract(body,'$.message.expires_at')) WHERE kind='delivery';")?;
            let version:Option<String>=tx.query_row("SELECT value FROM ch_meta WHERE key='schema_version'",[],|r|r.get(0)).optional()?;
            if version.as_ref().is_some_and(|v|v!="1"&&v!="2"){return Err(StoreError::TooNew(vec!["collaboration schema".into()]));}
            if version.as_deref()!=Some("2") { visibility::backfill(tx)?; }
            tx.execute("INSERT INTO ch_meta VALUES('schema_version','2') ON CONFLICT(key) DO UPDATE SET value='2'",[])?;
            let id: Option<String> = tx.query_row("SELECT value FROM ch_meta WHERE key='hub_id'", [], |r|r.get(0)).optional()?;
            match id { Some(id) => Ok(id), None => { let id = HubId::new().to_string();tx.execute("INSERT INTO ch_meta VALUES('hub_id',?1)",[&id])?;Ok(id) } }
        })?;
        Ok(Self {
            store,
            _lock: lock,
            hub_id: hub_id.parse().map_err(|_| Error::storage())?,
        })
    }
    pub fn read<T: Send + 'static>(
        &self,
        f: impl FnOnce(&Connection) -> Result<T, Error> + Send + 'static,
    ) -> Result<T, Error> {
        self.store.read(move |db| Ok(f(db))).map_err(Error::from)?
    }
    pub fn write<T: Send + 'static>(
        &self,
        f: impl FnOnce(&Connection) -> Result<T, Error> + Send + 'static,
    ) -> Result<T, Error> {
        let result = self.store.transaction(move |tx| match f(tx) {
            Ok(value) => Ok(Ok(value)),
            // Failed join-code attempts are durable security state, not a rollback.
            Err(error) if error.tag == "InvalidJoinCodeError" => Ok(Err(error)),
            Err(error) if error.tag == "StorageUnavailableError" => Err(StoreError::Unavailable(
                "hub transaction storage failed".into(),
            )),
            Err(error) => Err(StoreError::InvalidEvent {
                kind: "hub.mutation.v1".into(),
                reason: serde_json::to_string(&error).unwrap_or_default(),
            }),
        });
        match result {
            Ok(value) => value,
            Err(StoreError::InvalidEvent { reason, .. }) => serde_json::from_str(&reason)
                .map_err(|_| Error::storage())
                .and_then(Err),
            Err(error) => Err(error.into()),
        }
    }
}
fn private_database(path: &Path) -> Result<(), Error> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    std::fs::create_dir_all(parent).map_err(|_| Error::storage())?;
    if let Ok(metadata) = std::fs::symlink_metadata(path)
        && (!metadata.is_file() || metadata.file_type().is_symlink())
    {
        return Err(Error::invalid(
            "hub database must be a regular private file",
        ));
    }
    let mut options = std::fs::OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
        let directory = std::fs::symlink_metadata(parent).map_err(|_| Error::storage())?;
        if !directory.is_dir()
            || directory.mode() & 0o022 != 0
            || directory.uid() != rustix::process::geteuid().as_raw()
        {
            return Err(Error::invalid(
                "hub database directory must be owned by the operator and not writable by other users",
            ));
        }
        options.mode(0o600);
    }
    let file = options.open(path).map_err(|_| Error::storage())?;
    let metadata = file.metadata().map_err(|_| Error::storage())?;
    if !metadata.is_file()
        || std::fs::symlink_metadata(path)
            .map_err(|_| Error::storage())?
            .file_type()
            .is_symlink()
    {
        return Err(Error::invalid(
            "hub database must be a regular private file",
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.mode() & 0o077 != 0
            || metadata.nlink() != 1
            || metadata.uid() != rustix::process::geteuid().as_raw()
        {
            return Err(Error::invalid(
                "hub database must be operator-owned, mode 0600 and have a single link",
            ));
        }
    }
    Ok(())
}
pub fn get<T: DeserializeOwned>(db: &Connection, kind: &str, id: &str) -> Result<T, Error> {
    let body: Option<String> = db
        .query_row(
            "SELECT body FROM ch_objects WHERE kind=?1 AND id=?2",
            params![kind, id],
            |r| r.get(0),
        )
        .optional()?;
    decode(&body.ok_or_else(Error::missing)?)
}
pub fn list<T: DeserializeOwned>(
    db: &Connection,
    kind: &str,
    project: &str,
) -> Result<Vec<T>, Error> {
    let mut stmt = db.prepare(
        "SELECT body FROM ch_objects WHERE kind=?1 AND project=?2 ORDER BY id LIMIT 10000",
    )?;
    let rows = stmt.query_map(params![kind, project], |r| r.get::<_, String>(0))?;
    rows.map(|r| decode(&r?)).collect()
}
pub fn put<T: Serialize>(
    db: &Connection,
    kind: &str,
    id: &str,
    project: &str,
    value: &T,
) -> Result<(), Error> {
    let body = encode(value)?;
    let message = if kind == "message" {
        let message: Message = decode(&body)?;
        if message.id.as_str() != id || message.project.as_str() != project {
            return Err(Error::invalid("message identity binding mismatch"));
        }
        Some(message)
    } else {
        None
    };
    db.execute("INSERT INTO ch_objects(kind,id,project,body) VALUES(?1,?2,?3,?4) ON CONFLICT(kind,id) DO UPDATE SET body=excluded.body",params![kind,id,project,body])?;
    if let Some(message) = message {
        visibility::index(db, &message)?;
    }
    Ok(())
}
pub fn event<T: Serialize>(
    db: &Connection,
    project: &str,
    kind: &str,
    value: &T,
) -> Result<(), Error> {
    db.execute(
        "INSERT INTO ch_events(project,kind,body,created) VALUES(?1,?2,?3,?4)",
        params![project, kind, encode(value)?, now() as i64],
    )?;
    db.execute(
        "DELETE FROM ch_events WHERE created<?1",
        [now().saturating_sub(30 * 86400) as i64],
    )?;
    Ok(())
}

#[derive(Clone)]
pub struct Actor {
    pub proof: Arc<IdentityLease>,
    pub member: Option<Member>,
    pub agent: Option<Agent>,
}
impl Actor {
    pub fn key(&self) -> String {
        let a = self.proof.account();
        hash(
            serde_json::to_string(&(
                a.issuer.as_str(),
                a.sub.as_str(),
                self.agent.as_ref().map(|a| a.id.as_str()),
            ))
            .unwrap_or_default()
            .as_bytes(),
        )
    }
    pub fn participant(&self) -> Result<Participant, Error> {
        Ok(match &self.agent {
            Some(a) => Participant::Agent(a.id.clone()),
            None => Participant::Human(
                self.member
                    .as_ref()
                    .ok_or_else(Error::forbidden)?
                    .id
                    .clone(),
            ),
        })
    }
    pub fn scope(&self, db: &Connection, project: &str, write: bool) -> Result<Self, Error> {
        if !self.proof.is_current() {
            return Err(Error::new(
                "InactiveIdentityError",
                "live membership proof expired",
            ));
        }
        let p: Project = get(db, "project", project)?;
        let a = self.proof.account();
        let member = list::<Member>(db, "member", project)?
            .into_iter()
            .find(|m| m.issuer == a.issuer && m.subject == a.sub && m.active)
            .ok_or_else(Error::forbidden)?;
        match (&p.mode, member.kind) {
            (ProjectMode::Organization { organization_id }, MembershipKind::Internal) => {
                self.proof.require_organization(organization_id)?
            }
            (ProjectMode::Organization { .. }, MembershipKind::Guest) if p.guests_enabled => {}
            (ProjectMode::Friends, MembershipKind::Friend) => {}
            _ => return Err(Error::forbidden()),
        }
        if self.agent.is_some()
            && self
                .member
                .as_ref()
                .is_some_and(|prior| prior.id != member.id || prior.generation != member.generation)
        {
            return Err(Error::forbidden());
        }
        if write && !member.role.can_write() {
            return Err(Error::forbidden());
        }
        let agent = if let Some(agent) = &self.agent {
            let current: Agent = get(db, "agent", agent.id.as_str())?;
            if !current.active || current.owner != member.id || current.project.as_str() != project
            {
                return Err(Error::forbidden());
            }
            Some(current)
        } else {
            None
        };
        Ok(Self {
            proof: self.proof.clone(),
            member: Some(member),
            agent,
        })
    }
    pub fn owner(&self, db: &Connection, project: &str, mfa: bool) -> Result<Self, Error> {
        let actor = self.scope(db, project, true)?;
        if actor.agent.is_some() || actor.member.as_ref().is_none_or(|m| m.role != Role::Owner) {
            return Err(Error::forbidden());
        }
        actor.proof.require_step_up(mfa)?;
        Ok(actor)
    }
}

pub fn mutation<T: Serialize>(
    db: &Connection,
    actor: &Actor,
    route: &str,
    operation: &str,
    input: &Value,
    f: impl FnOnce() -> Result<T, Error>,
) -> Result<crate::api::MutationResult<Value>, Error> {
    if operation.len() < 8
        || operation.len() > 128
        || !operation
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
        return Err(Error::invalid("invalid operation ID"));
    }
    let digest = hash(encode(&(route, input))?.as_bytes());
    let scope = route
        .split_whitespace()
        .nth(1)
        .and_then(|p| p.strip_prefix("projects/"))
        .and_then(|p| p.split('/').next())
        .unwrap_or("global");
    let actor_key = hash(encode(&(actor.key(), scope))?.as_bytes());
    let old: Option<(String, String)> = db
        .query_row(
            "SELECT digest,result FROM ch_operations WHERE actor=?1 AND operation=?2",
            params![actor_key, operation],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    if let Some((prior, result)) = old {
        if prior != digest {
            return Err(Error::new(
                "OperationConflictError",
                "operation ID is bound to different input",
            ));
        }
        let data: Value = decode(&result)?;
        if let Some(error) = data.get("__hub_error") {
            return Err(serde_json::from_value(error.clone()).map_err(|_| Error::storage())?);
        }
        if let Some(project) = data
            .get("project")
            .and_then(|p| p.get("id"))
            .and_then(Value::as_str)
            && data.get("member").is_some_and(|m| !m.is_null())
        {
            actor.scope(db, project, false)?;
        }
        return Ok(crate::api::MutationResult {
            data,
            replayed: true,
        });
    }
    let (result, committed_error) = match f() {
        Ok(result) => (
            serde_json::to_value(result).map_err(|_| Error::storage())?,
            None,
        ),
        Err(error) if error.tag == "InvalidJoinCodeError" => {
            (serde_json::json!({"__hub_error":error}), Some(error))
        }
        Err(error) => return Err(error),
    };
    let mut cached = result.clone();
    redact(&mut cached);
    db.execute(
        "INSERT INTO ch_operations VALUES(?1,?2,?3,?4,?5)",
        params![actor_key, operation, digest, encode(&cached)?, now() as i64],
    )?;
    if let Some(error) = committed_error {
        return Err(error);
    }
    Ok(crate::api::MutationResult {
        data: result,
        replayed: false,
    })
}
fn redact(value: &mut Value) {
    match value {
        Value::Object(fields) => {
            for (key, value) in fields {
                if matches!(key.as_str(), "token" | "capability" | "code") {
                    *value = Value::Null
                } else {
                    redact(value)
                }
            }
        }
        Value::Array(items) => {
            for item in items {
                redact(item)
            }
        }
        _ => {}
    }
}
