//! Location-scoped MCP native ownership, independent of Session aggregates.
use std::path::{Path, PathBuf};
use std::sync::Arc;

use cyber_store::{EventRegistry, Expected, NewEvent, Store, StoreError, StoredEvent};
use rusqlite::{OptionalExtension, Transaction, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const CHANGED: &str = "mcp.status.changed.1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum McpConnectionStatus {
    Connecting,
    Connected,
    Disabled,
    Failed,
    NeedsAuth,
    NeedsClientRegistration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum McpConnectionPhase {
    Admitted,
    Preparing,
    Launching,
    Running,
    Settled,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct McpConnectionRecord {
    pub id: String,
    pub directory: PathBuf,
    pub checkout_root: PathBuf,
    pub name: String,
    pub digest: String,
    pub worktree_ids: Vec<String>,
    pub status: McpConnectionStatus,
    pub phase: McpConnectionPhase,
    pub acknowledged: Option<bool>,
    pub error: Option<String>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Change {
    record: McpConnectionRecord,
    previous_owner_key: Option<String>,
    next_owner_hash: String,
}

/// Observes only successfully committed records, without private owner capabilities.
pub type McpConnectionObserver = Arc<dyn Fn(&McpConnectionRecord, i64) + Send + Sync>;

/// A committed ownership observation, not fresh process liveness or recovery authority.
#[derive(Debug, Clone, PartialEq, Serialize, schemars::JsonSchema)]
pub struct McpStatusUpdate {
    pub connection_id: String,
    pub directory: PathBuf,
    pub name: String,
    pub status: McpConnectionStatus,
    pub phase: McpConnectionPhase,
    pub acknowledged: Option<bool>,
    /// Public diagnostic; private launch details remain in local logs.
    pub error: Option<String>,
}

impl From<&McpConnectionRecord> for McpStatusUpdate {
    fn from(record: &McpConnectionRecord) -> Self {
        Self {
            connection_id: record.id.clone(),
            directory: record.directory.clone(),
            name: record.name.clone(),
            status: record.status,
            phase: record.phase,
            acknowledged: record.acknowledged,
            error: record.error.as_ref().map(|_| {
                match record.phase {
                    McpConnectionPhase::Settled => "MCP connection stopped",
                    McpConnectionPhase::Unknown => "MCP native settlement is unverified",
                    _ => "MCP connection failed; inspect local logs",
                }
                .into()
            }),
        }
    }
}

/// A receipt is not execution authority or native proof; the caller retains both.
/// Every transition consumes an unpublished owner key and installs a new commitment.
pub struct McpConnectionOwner {
    store: Arc<Store>,
    record: McpConnectionRecord,
    key: String,
    seq: i64,
    terminal: bool,
    observer: Option<McpConnectionObserver>,
}

impl McpConnectionOwner {
    pub fn admit(
        store: Arc<Store>,
        directory: &Path,
        checkout_root: &Path,
        name: &str,
        digest: &str,
    ) -> Result<Self, StoreError> {
        Self::admit_observed(store, directory, checkout_root, name, digest, None)
    }

    pub fn admit_observed(
        store: Arc<Store>,
        directory: &Path,
        checkout_root: &Path,
        name: &str,
        digest: &str,
        observer: Option<McpConnectionObserver>,
    ) -> Result<Self, StoreError> {
        let record = McpConnectionRecord {
            id: cyber_core::ids::new_id("mcs"),
            directory: std::fs::canonicalize(directory).map_err(StoreError::Io)?,
            checkout_root: std::fs::canonicalize(checkout_root).map_err(StoreError::Io)?,
            name: name.into(),
            digest: digest.into(),
            worktree_ids: Vec::new(),
            status: McpConnectionStatus::Connecting,
            phase: McpConnectionPhase::Admitted,
            acknowledged: None,
            error: None,
        };
        let key = new_key();
        store.append(
            &record.id,
            Expected::Seq(-1),
            vec![change(&record, None, &key)],
        )?;
        let owner = Self {
            store,
            record,
            key,
            seq: 0,
            terminal: false,
            observer,
        };
        owner.notify();
        Ok(owner)
    }

    pub fn record(&self) -> &McpConnectionRecord {
        &self.record
    }

    /// Persist before any native preparation, including managed checkout inspection.
    pub fn preparing(&mut self) -> Result<(), StoreError> {
        let mut record = self.record.clone();
        record.phase = McpConnectionPhase::Preparing;
        self.commit(record)
    }

    /// Pin verified managed identities before native server spawn.
    pub fn launching(&mut self, worktree_ids: Vec<String>) -> Result<(), StoreError> {
        let mut record = self.record.clone();
        record.phase = McpConnectionPhase::Launching;
        record.worktree_ids = worktree_ids;
        self.commit(record)
    }

    pub fn connected(&mut self) -> Result<(), StoreError> {
        let mut record = self.record.clone();
        record.phase = McpConnectionPhase::Running;
        record.status = McpConnectionStatus::Connected;
        self.commit(record)
    }

    /// Use actual retained process/proxy evidence; committing must precede cleanup.
    pub fn finish(
        &mut self,
        acknowledged: bool,
        error: String,
    ) -> Result<McpConnectionRecord, StoreError> {
        let mut record = self.record.clone();
        record.phase = if acknowledged {
            McpConnectionPhase::Settled
        } else {
            McpConnectionPhase::Unknown
        };
        record.status = McpConnectionStatus::Failed;
        record.acknowledged = Some(acknowledged);
        record.error = Some(error);
        self.commit(record)?;
        self.terminal = true;
        Ok(self.record.clone())
    }

    fn notify(&self) {
        if let Some(observer) = &self.observer {
            observer(&self.record, self.seq);
        }
    }

    fn commit(&mut self, record: McpConnectionRecord) -> Result<(), StoreError> {
        let next = new_key();
        self.store.append(
            &record.id,
            Expected::Seq(self.seq),
            vec![change(&record, Some(self.key.clone()), &next)],
        )?;
        self.record = record;
        self.key = next;
        self.seq += 1;
        self.notify();
        Ok(())
    }
}

impl Drop for McpConnectionOwner {
    fn drop(&mut self) {
        if self.terminal {
            return;
        }
        let acknowledged = self.record.phase == McpConnectionPhase::Admitted;
        let diagnostic = if acknowledged {
            "MCP admission disposed before native preparation"
        } else {
            "MCP owner disposed before verified native settlement"
        };
        if let Err(error) = self.finish(acknowledged, diagnostic.into()) {
            cyber_core::log::error(
                "mcp",
                &error.to_string(),
                serde_json::json!({"connection_id":self.record.id}),
            );
        }
    }
}

fn new_key() -> String {
    format!(
        "{}_{}",
        cyber_core::ids::new_id("key"),
        cyber_core::ids::new_id("key")
    )
}
fn hash(key: &str) -> String {
    format!("sha256:{:x}", Sha256::digest(key.as_bytes()))
}
fn change(
    record: &McpConnectionRecord,
    previous_owner_key: Option<String>,
    next: &str,
) -> NewEvent {
    NewEvent::new(
        CHANGED,
        serde_json::to_value(Change {
            record: record.clone(),
            previous_owner_key,
            next_owner_hash: hash(next),
        })
        .expect("MCP receipt serializes"),
    )
}

pub fn mcp_connections(
    store: &Store,
    directory: &Path,
) -> Result<Vec<McpConnectionRecord>, StoreError> {
    let directory = std::fs::canonicalize(directory).map_err(StoreError::Io)?;
    store.read(move |connection| {
        let mut statement = connection
            .prepare("SELECT data FROM mcp_connection WHERE directory=?1 ORDER BY name,id")?;
        let data = statement
            .query_map([directory.to_string_lossy()], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        data.into_iter()
            .map(|data| {
                serde_json::from_str(&data).map_err(|error| StoreError::Projector {
                    kind: CHANGED.into(),
                    reason: error.to_string(),
                })
            })
            .collect()
    })
}

pub(super) fn register(registry: &mut EventRegistry) {
    registry.register(CHANGED).expect("valid MCP event");
    registry.projector(project);
}

fn project(tx: &Transaction<'_>, event: &StoredEvent) -> Result<(), String> {
    if event.kind != CHANGED {
        return Ok(());
    }
    let change: Change =
        serde_json::from_value(event.data.clone()).map_err(|error| error.to_string())?;
    let record = &change.record;
    validate(record, event)?;
    if !valid_hash(&change.next_owner_hash) {
        return Err("Invalid MCP ownership commitment".into());
    }
    let data = serde_json::to_string(record).map_err(|error| error.to_string())?;
    if event.seq == 0 {
        if record.phase != McpConnectionPhase::Admitted
            || change.previous_owner_key.is_some()
            || !record.worktree_ids.is_empty()
        {
            return Err(
                "MCP ownership must start at admission without borrowed native identities".into(),
            );
        }
        let unresolved: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM mcp_connection WHERE directory=?1 AND name=?2 AND phase!='settled')",params![record.directory.to_string_lossy(),record.name], |row| row.get(0)).map_err(|error| error.to_string())?;
        if unresolved {
            return Err("MCP connection has unresolved ownership".into());
        }
        tx.execute("INSERT INTO mcp_connection(id,directory,name,phase,owner_hash,data) VALUES (?1,?2,?3,'admitted',?4,?5)",params![record.id,record.directory.to_string_lossy(),record.name,change.next_owner_hash,data]).map_err(|error| error.to_string())?;
        return Ok(());
    }
    let previous: Option<(String, String)> = tx
        .query_row(
            "SELECT data,owner_hash FROM mcp_connection WHERE id=?1",
            [&record.id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(|error| error.to_string())?;
    let (previous, owner_hash) = previous.ok_or("MCP ownership is absent")?;
    let previous: McpConnectionRecord =
        serde_json::from_str(&previous).map_err(|error| error.to_string())?;
    if change.previous_owner_key.as_deref().map(hash).as_deref() != Some(&owner_hash)
        || change.next_owner_hash == owner_hash
    {
        return Err("MCP transition does not hold the current native owner capability".into());
    }
    verify_transition(&previous, record)?;
    let phase = serde_json::to_value(record.phase).map_err(|error| error.to_string())?;
    tx.execute(
        "UPDATE mcp_connection SET phase=?2,owner_hash=?3,data=?4 WHERE id=?1",
        params![
            record.id,
            phase.as_str().ok_or("Invalid MCP phase")?,
            change.next_owner_hash,
            data
        ],
    )
    .map_err(|error| error.to_string())?;
    Ok(())
}

fn validate(record: &McpConnectionRecord, event: &StoredEvent) -> Result<(), String> {
    if record.id != event.aggregate_id
        || !cyber_core::ids::has_prefix(&record.id, "mcs")
        || record.name.is_empty()
        || record.name.len() > 48
        || !record
            .name
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"_-".contains(&c))
        || !valid_hash(&record.digest)
    {
        return Err("Invalid MCP connection identity".into());
    }
    if !record.directory.is_absolute()
        || !record.checkout_root.is_absolute()
        || !record.directory.starts_with(&record.checkout_root)
        || record
            .worktree_ids
            .iter()
            .any(|id| !cyber_core::ids::has_prefix(id, "wt"))
    {
        return Err("Invalid MCP Location or managed binding".into());
    }
    let valid = match record.phase {
        McpConnectionPhase::Admitted
        | McpConnectionPhase::Preparing
        | McpConnectionPhase::Launching => {
            record.status == McpConnectionStatus::Connecting
                && record.acknowledged.is_none()
                && record.error.is_none()
        }
        McpConnectionPhase::Running => {
            record.status == McpConnectionStatus::Connected
                && record.acknowledged.is_none()
                && record.error.is_none()
        }
        McpConnectionPhase::Settled => {
            record.status == McpConnectionStatus::Failed
                && record.acknowledged == Some(true)
                && record.error.is_some()
        }
        McpConnectionPhase::Unknown => {
            record.status == McpConnectionStatus::Failed
                && record.acknowledged == Some(false)
                && record.error.is_some()
        }
    };
    if !valid {
        return Err("MCP status contradicts native ownership phase".into());
    }
    Ok(())
}

fn verify_transition(
    previous: &McpConnectionRecord,
    next: &McpConnectionRecord,
) -> Result<(), String> {
    use McpConnectionPhase::*;
    if previous.id != next.id
        || previous.directory != next.directory
        || previous.checkout_root != next.checkout_root
        || previous.name != next.name
        || previous.digest != next.digest
        || (previous.worktree_ids != next.worktree_ids
            && !(previous.phase == Preparing && next.phase == Launching))
    {
        return Err("MCP transition changed pinned identity".into());
    }
    let valid = matches!(
        (previous.phase, next.phase),
        (Admitted, Preparing)
            | (Preparing, Launching)
            | (Launching, Running)
            | (
                Admitted | Preparing | Launching | Running,
                Settled | Unknown
            )
            | (Unknown, Settled)
    );
    if !valid {
        return Err("MCP ownership is terminal or transition is invalid".into());
    }
    Ok(())
}

fn valid_hash(value: &str) -> bool {
    value
        .strip_prefix("sha256:")
        .is_some_and(|value| value.len() == 64 && value.bytes().all(|c| c.is_ascii_hexdigit()))
}
