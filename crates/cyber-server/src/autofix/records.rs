//! A subscription snapshot and its audit event commit in one writer transaction.
use cyber_core::autofix::Subscription;
use cyber_store::{EventRegistry, Expected, NewEvent, Store, StoreError, StoredEvent};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use sha2::{Digest, Sha256};
use std::sync::Arc;

pub const EVENT: &str = "autofix.subscription.changed.1";
#[derive(Clone)]
pub struct Repository {
    store: Arc<Store>,
}
fn invalid(message: impl Into<String>) -> StoreError {
    StoreError::InvalidEvent {
        kind: EVENT.into(),
        reason: message.into(),
    }
}
fn decode(text: String) -> Result<Subscription, StoreError> {
    serde_json::from_str(&text).map_err(|e| StoreError::Unavailable(e.to_string()))
}
fn read(db: &Connection, id: &str) -> Result<Subscription, StoreError> {
    let text: Option<String> = db
        .query_row(
            "SELECT data FROM autofix_subscription WHERE id=?1",
            [id],
            |r| r.get(0),
        )
        .optional()?;
    decode(text.ok_or_else(|| invalid("subscription not found"))?)
}
fn event(s: &Subscription) -> Result<NewEvent, StoreError> {
    Ok(NewEvent::new(
        EVENT,
        serde_json::to_value(s).map_err(|e| invalid(e.to_string()))?,
    ))
}
pub(crate) fn register(registry: &mut EventRegistry) {
    registry
        .register_with(EVENT, |data| {
            let value: Subscription =
                serde_json::from_value(data.clone()).map_err(|e| e.to_string())?;
            value.config.validate()?;
            if value.revision == 0 {
                return Err("invalid subscription revision".into());
            }
            Ok(())
        })
        .expect("valid auto-fix event");
    registry.projector(project);
}
fn project(tx: &Transaction<'_>, e: &StoredEvent) -> Result<(), String> {
    if e.kind != EVENT {
        return Ok(());
    }
    let s: Subscription = serde_json::from_value(e.data.clone()).map_err(|e| e.to_string())?;
    let status = serde_json::to_value(&s.status).map_err(|e| e.to_string())?;
    tx.execute("INSERT INTO autofix_subscription(id,directory,repository_id,pr,status,revision,data) VALUES(?1,?2,?3,?4,?5,?6,?7) ON CONFLICT(id) DO UPDATE SET status=excluded.status,revision=excluded.revision,data=excluded.data",params![s.id,s.directory,s.repository_id,s.config.pr,status.as_str(),s.revision,e.data.to_string()]).map_err(|e|e.to_string())?;
    Ok(())
}
impl Repository {
    pub fn new(store: Arc<Store>) -> Self {
        Self { store }
    }
    pub fn bind_session(
        &self,
        subscription: &str,
        attempt: &str,
        session: &str,
        directory: &str,
    ) -> Result<(), StoreError> {
        let (subscription, attempt, session, directory) = (
            subscription.to_owned(),
            attempt.to_owned(),
            session.to_owned(),
            directory.to_owned(),
        );
        self.store.transaction(move|tx|{
            let value=read(tx,&subscription)?;
            let active=value.active().ok_or_else(||invalid("attempt no longer active"))?;
            if active.id!=attempt || value.status!=cyber_core::autofix::Status::Enabled {return Err(invalid("attempt no longer authorized"))}
            tx.execute("INSERT INTO autofix_session(session_id,subscription_id,attempt_id,directory) VALUES(?1,?2,?3,?4)",params![session,subscription,attempt,directory])?;
            Ok(())
        })
    }
    pub fn id(directory: &str, key: &str) -> String {
        format!(
            "afx_{:x}",
            Sha256::digest(format!("autofix:{directory}\0{key}"))
        )
    }
    pub fn get(&self, id: &str) -> Result<Subscription, StoreError> {
        let id = id.to_owned();
        self.store.read(move |db| read(db, &id))
    }
    pub fn list(&self, directory: Option<&str>) -> Result<Vec<Subscription>, StoreError> {
        let directory = directory.map(str::to_owned);
        self.store.read(move|db|{
            let mut stmt=db.prepare("SELECT data FROM autofix_subscription WHERE (?1 IS NULL OR directory=?1) ORDER BY id LIMIT 101")?;
            let rows=stmt.query_map([directory],|r|r.get::<_,String>(0))?.map(|r|decode(r?)).collect::<Result<Vec<_>,_>>()?;
            if rows.len()>100{return Err(invalid("subscription capacity exceeded"))}
            Ok(rows)
        })
    }
    pub fn create(&self, s: Subscription) -> Result<Subscription, StoreError> {
        let id = s.id.clone();
        let (_,result)=self.store.append_checked(&id,Expected::Any,move|tx|{
            if let Ok(old)=read(tx,&s.id) {
                if old.directory!=s.directory || old.config!=s.config || old.repository_id!=s.repository_id {return Err(invalid("enable key reused with different arguments"))}
                return Ok((vec![],old))
            }
            let count:u64=tx.query_row("SELECT COUNT(*) FROM autofix_subscription",[],|r|r.get(0))?;
            if count>=100{return Err(invalid("subscription limit reached (100)"))}
            let exists:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM autofix_subscription WHERE directory=?1 AND repository_id=?2 AND pr=?3 AND status NOT IN ('disabled','closed'))",params![s.directory,s.repository_id,s.config.pr],|r|r.get(0))?;
            if exists{return Err(invalid("this PR already has a subscription"))}
            Ok((vec![event(&s)?],s))
        })?;
        Ok(result)
    }
    /// Revision fencing serializes owner controls, polling and worker transitions.
    /// The closure contains decisions only, never network or process execution.
    pub fn update<F>(
        &self,
        id: &str,
        revision: u64,
        now: i64,
        f: F,
    ) -> Result<Subscription, StoreError>
    where
        F: FnOnce(&mut Subscription) -> Result<(), String> + Send + 'static,
    {
        let id = id.to_owned();
        let aggregate = id.clone();
        let (_, result) = self
            .store
            .append_checked(&aggregate, Expected::Any, move |tx| {
                let mut s = read(tx, &id)?;
                if s.revision != revision {
                    return Err(invalid("subscription revision changed"));
                }
                f(&mut s).map_err(invalid)?;
                s.revision = s
                    .revision
                    .checked_add(1)
                    .ok_or_else(|| invalid("revision overflow"))?;
                s.updated_ms = now;
                Ok((vec![event(&s)?], s))
            })?;
        Ok(result)
    }
}
