//! Owner-local metadata. Capabilities and account credentials never enter these tables.
use cyber_store::{Store, StoreError};
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::sync::Arc;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Binding {
    pub checkout: String,
    pub hub_url: String,
    pub hub_id: String,
    pub project: String,
    pub repository: cyber_collab::repository::Repository,
    #[serde(default = "default_remote")]
    pub remote: String,
}
fn default_remote() -> String {
    "origin".into()
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Publication {
    pub session_id: String,
    pub hub_id: String,
    pub project: String,
    pub publication_key: String,
    pub agent_id: Option<String>,
    pub runtime: Option<String>,
    pub alias: String,
    pub active: bool,
    #[serde(default)]
    pub issuer: String,
    #[serde(default)]
    pub subject: String,
    #[serde(default)]
    pub expires_at: u64,
}
/// A fresh membership/policy proof determines whether an old replay checkpoint is reusable.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CursorScope {
    pub checkout: String,
    pub identity: String,
    pub hub: String,
    pub project: String,
    pub member: String,
    pub generation: u64,
    pub project_revision: u64,
}
impl CursorScope {
    fn key(&self) -> Result<String, StoreError> {
        Ok(format!(
            "{:x}",
            Sha256::digest(
                encode(&(&self.checkout, &self.identity, &self.hub, &self.project))?.as_bytes()
            )
        ))
    }
    fn epoch(&self) -> Result<String, StoreError> {
        encode(&(&self.member, self.generation, self.project_revision))
    }
}
#[derive(Clone)]
pub struct LocalStore(pub Arc<Store>);
fn invalid(text: impl Into<String>) -> StoreError {
    StoreError::InvalidEvent {
        kind: "collaboration.local".into(),
        reason: text.into(),
    }
}
fn encode<T: Serialize>(value: &T) -> Result<String, StoreError> {
    serde_json::to_string(value).map_err(|_| invalid("invalid collaboration metadata"))
}
fn decode<T: serde::de::DeserializeOwned>(value: String) -> Result<T, StoreError> {
    serde_json::from_str(&value).map_err(|_| invalid("corrupt collaboration metadata"))
}
impl LocalStore {
    pub fn new(store: Arc<Store>) -> Result<Self, StoreError> {
        store.transaction(|tx| { tx.execute_batch(
            "CREATE TABLE IF NOT EXISTS collaboration_binding(checkout TEXT PRIMARY KEY, body TEXT NOT NULL);
             CREATE TABLE IF NOT EXISTS collaboration_publication(session TEXT PRIMARY KEY, hub TEXT NOT NULL, project TEXT NOT NULL, body TEXT NOT NULL);
             CREATE TABLE IF NOT EXISTS collaboration_delivery(hub TEXT NOT NULL, project TEXT NOT NULL, message TEXT NOT NULL, target TEXT NOT NULL, hash TEXT NOT NULL, inbox TEXT NOT NULL, admitted INTEGER NOT NULL DEFAULT 0, PRIMARY KEY(hub,project,message,target));
             CREATE TABLE IF NOT EXISTS collaboration_outbox(operation TEXT PRIMARY KEY, identity TEXT NOT NULL, hub TEXT NOT NULL, project TEXT NOT NULL, body TEXT NOT NULL);
             CREATE TABLE IF NOT EXISTS collaboration_cursor(binding TEXT PRIMARY KEY,epoch TEXT NOT NULL,cursor INTEGER NOT NULL);
             CREATE TABLE IF NOT EXISTS collaboration_receipt(operation TEXT PRIMARY KEY, hub TEXT NOT NULL, project TEXT NOT NULL, state TEXT NOT NULL, detail TEXT, digest TEXT NOT NULL);"
        )?; Ok(()) })?;
        Ok(Self(store))
    }
    pub fn cursor(&self, scope: CursorScope) -> Result<u64, StoreError> {
        let key = scope.key()?;
        let epoch = scope.epoch()?;
        self.0.read(move |db| {
            let value: Option<(String, i64)> = db
                .query_row(
                    "SELECT epoch,cursor FROM collaboration_cursor WHERE binding=?1",
                    [key],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?;
            match value {
                Some((old, cursor)) if old == epoch => {
                    u64::try_from(cursor).map_err(|_| invalid("invalid collaboration cursor"))
                }
                _ => Ok(0),
            }
        })
    }
    /// Checkpoint metadata only. Event replay/snapshot contents never enter the inbox.
    pub fn checkpoint(&self, scope: CursorScope, cursor: u64) -> Result<(), StoreError> {
        let key = scope.key()?;
        let epoch = scope.epoch()?;
        let cursor = i64::try_from(cursor).map_err(|_| invalid("invalid collaboration cursor"))?;
        self.0.transaction(move|tx|{tx.execute("INSERT INTO collaboration_cursor VALUES(?1,?2,?3) ON CONFLICT(binding) DO UPDATE SET epoch=excluded.epoch,cursor=excluded.cursor",(key,epoch,cursor))?;Ok(())})
    }
    pub fn bind(&self, binding: Binding) -> Result<(), StoreError> {
        let body = encode(&binding)?;
        self.0.transaction(move |tx| { tx.execute("INSERT INTO collaboration_binding VALUES(?1,?2) ON CONFLICT(checkout) DO UPDATE SET body=excluded.body", (&binding.checkout,body))?; Ok(()) })
    }
    pub fn binding(&self, checkout: &str) -> Result<Option<Binding>, StoreError> {
        let checkout = checkout.to_owned();
        self.0.read(move |db| {
            db.query_row(
                "SELECT body FROM collaboration_binding WHERE checkout=?1",
                [checkout],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .map(decode)
            .transpose()
        })
    }
    pub fn bindings(&self) -> Result<Vec<Binding>, StoreError> {
        self.0.read(|db| {
            let mut statement = db.prepare("SELECT body FROM collaboration_binding")?;
            statement
                .query_map([], |row| row.get::<_, String>(0))?
                .map(|row| decode(row?))
                .collect()
        })
    }
    /// The first publication permanently pins a Session's accumulated context. Even an
    /// unpublished Session may never be republished into another project or hub.
    pub fn publish(&self, publication: Publication) -> Result<(), StoreError> {
        let body = encode(&publication)?;
        self.0.transaction(move |tx| {
            let old: Option<(String,String)> = tx.query_row("SELECT hub,project FROM collaboration_publication WHERE session=?1", [&publication.session_id], |r| Ok((r.get(0)?,r.get(1)?))).optional()?;
            if old.is_some_and(|(hub,project)| hub != publication.hub_id || project != publication.project) {
                return Err(invalid("PublishedSessionProjectError: create a new Session to publish into another project"));
            }
            tx.execute("INSERT INTO collaboration_publication VALUES(?1,?2,?3,?4) ON CONFLICT(session) DO UPDATE SET body=excluded.body", (&publication.session_id,&publication.hub_id,&publication.project,body))?;
            Ok(())
        })
    }
    pub fn publication(&self, session: &str) -> Result<Option<Publication>, StoreError> {
        let session = session.to_owned();
        self.0.read(move |db| {
            db.query_row(
                "SELECT body FROM collaboration_publication WHERE session=?1",
                [session],
                |r| r.get::<_, String>(0),
            )
            .optional()?
            .map(decode)
            .transpose()
        })
    }
    pub fn publications(&self) -> Result<Vec<Publication>, StoreError> {
        self.0.read(|db| {
            let mut statement = db.prepare("SELECT body FROM collaboration_publication")?;
            statement
                .query_map([], |r| r.get::<_, String>(0))?
                .map(|row| decode(row?))
                .collect()
        })
    }
    /// Commits before Runtime admission/remote acknowledgement. Exact replays get the
    /// same inbox ID; changed content for the same identity is rejected.
    pub fn delivery(
        &self,
        hub: &str,
        project: &str,
        message: &str,
        target: &str,
        hash: &str,
    ) -> Result<String, StoreError> {
        let (hub, project, message, target, hash) = (
            hub.to_owned(),
            project.to_owned(),
            message.to_owned(),
            target.to_owned(),
            hash.to_owned(),
        );
        let key =
            serde_json::to_vec(&(&hub, &project, &message, &target)).expect("strings serialize");
        let inbox = format!("msgx_{:x}", Sha256::digest(key));
        self.0.transaction(move |tx| {
            let old: Option<String> = tx.query_row("SELECT hash FROM collaboration_delivery WHERE hub=?1 AND project=?2 AND message=?3 AND target=?4", (&hub,&project,&message,&target), |r|r.get(0)).optional()?;
            if old.is_some_and(|old|old != hash) { return Err(invalid("CollaborationContentConflictError: delivery content changed")); }
            tx.execute("INSERT OR IGNORE INTO collaboration_delivery(hub,project,message,target,hash,inbox) VALUES(?1,?2,?3,?4,?5,?6)", (&hub,&project,&message,&target,&hash,&inbox))?;
            Ok(inbox)
        })
    }
    pub fn enqueue(
        &self,
        operation: String,
        identity: String,
        hub: String,
        project: String,
        body: serde_json::Value,
    ) -> Result<(), StoreError> {
        let body = encode(&body)?;
        self.0.transaction(move |tx| {
            let old: Option<(String, String, String, String)> = tx
                .query_row(
                    "SELECT identity,hub,project,body FROM collaboration_outbox WHERE operation=?1",
                    [&operation],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
                )
                .optional()?;
            let value = (identity, hub, project, body);
            let digest = format!("{:x}", Sha256::digest(encode(&value)?.as_bytes()));
            let prior: Option<String> = tx
                .query_row(
                    "SELECT digest FROM collaboration_receipt WHERE operation=?1",
                    [&operation],
                    |r| r.get(0),
                )
                .optional()?;
            if prior.is_some_and(|prior| prior != digest) {
                return Err(invalid(
                    "OperationConflictError: completed operation ID was reused",
                ));
            }
            tx.execute(
                "INSERT OR IGNORE INTO collaboration_receipt VALUES(?1,?2,?3,'queued',NULL,?4)",
                (&operation, &value.1, &value.2, digest),
            )?;
            if old.is_some_and(|old| old != value) {
                return Err(invalid("OperationConflictError: operation ID was reused"));
            }
            tx.execute(
                "INSERT OR IGNORE INTO collaboration_outbox VALUES(?1,?2,?3,?4,?5)",
                (&operation, &value.0, &value.1, &value.2, &value.3),
            )?;
            Ok(())
        })
    }
    pub fn pending(
        &self,
        identity: String,
    ) -> Result<Vec<(String, String, String, serde_json::Value)>, StoreError> {
        self.0.read(move |db| {
            let mut statement=db.prepare("SELECT operation,hub,project,body FROM collaboration_outbox WHERE identity=?1 ORDER BY rowid LIMIT 100")?;
            statement.query_map([identity],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,String>(3)?)))?.map(|row|{let (op,hub,project,body)=row?;Ok((op,hub,project,decode(body)?))}).collect()
        })
    }
    pub fn delivery_outcomes(
        &self,
        hub: String,
        project: String,
        target: String,
    ) -> Result<Vec<(String, String, String, String)>, StoreError> {
        self.0.read(move |db| {
            let mut statement=db.prepare("SELECT d.message,d.hash,m.outcome,d.inbox FROM collaboration_delivery d JOIN xsession_message m ON m.message_id=d.inbox AND m.direction='in' WHERE d.hub=?1 AND d.project=?2 AND d.target=?3 AND m.outcome IN ('released','dropped','refused','delivered')")?;
            statement.query_map((hub,project,target),|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?)))?.map(|row|Ok(row?)).collect()
        })
    }
    pub fn receipts(
        &self,
        hub: String,
        project: String,
    ) -> Result<Vec<serde_json::Value>, StoreError> {
        self.0.read(move|db|{
            let mut statement=db.prepare("SELECT operation,state,detail FROM collaboration_receipt WHERE hub=?1 AND project=?2 ORDER BY rowid DESC LIMIT 100")?;
            statement.query_map((hub,project),|r|Ok(serde_json::json!({"operation_id":r.get::<_,String>(0)?,"state":r.get::<_,String>(1)?,"detail":r.get::<_,Option<String>>(2)?})))?.map(|r|Ok(r?)).collect()
        })
    }
    pub fn reject(&self, operation: String, detail: String) -> Result<(), StoreError> {
        self.0.transaction(move |tx| {
            tx.execute(
                "UPDATE collaboration_receipt SET state='rejected',detail=?2 WHERE operation=?1",
                (&operation, detail),
            )?;
            tx.execute(
                "DELETE FROM collaboration_outbox WHERE operation=?1",
                [operation],
            )?;
            Ok(())
        })
    }
    pub fn complete(&self, operation: String) -> Result<(), StoreError> {
        self.0.transaction(move |tx| {
            tx.execute(
                "UPDATE collaboration_receipt SET state='applied',detail=NULL WHERE operation=?1",
                [&operation],
            )?;
            tx.execute(
                "DELETE FROM collaboration_outbox WHERE operation=?1",
                [operation],
            )?;
            Ok(())
        })
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn store() -> LocalStore {
        LocalStore::new(Arc::new(
            Store::open(cyber_store::StoreOptions::new(
                cyber_core::paths::DatabaseLocation::Memory,
                cyber_store::EventRegistry::default(),
            ))
            .unwrap(),
        ))
        .unwrap()
    }
    #[test]
    fn replay_cursor_is_bound_to_identity_checkout_and_current_membership_epoch() {
        let store = store();
        let scope = CursorScope {
            checkout: "/checkout".into(),
            identity: "issuer/sub".into(),
            hub: "hub".into(),
            project: "project".into(),
            member: "member".into(),
            generation: 1,
            project_revision: 1,
        };
        store.checkpoint(scope.clone(), 42).unwrap();
        assert_eq!(store.cursor(scope.clone()).unwrap(), 42);
        let mut changed = scope.clone();
        changed.generation += 1;
        assert_eq!(store.cursor(changed).unwrap(), 0);
        let mut changed = scope.clone();
        changed.project_revision += 1;
        assert_eq!(store.cursor(changed).unwrap(), 0);
        let mut changed = scope.clone();
        changed.identity = "other/sub".into();
        assert_eq!(store.cursor(changed).unwrap(), 0);
        let mut changed = scope;
        changed.checkout = "/other".into();
        assert_eq!(store.cursor(changed).unwrap(), 0);
    }
    #[test]
    fn unpublishing_preserves_project_tombstone() {
        let store = store();
        let mut publication = Publication {
            session_id: "ses_test".into(),
            hub_id: "hub1".into(),
            project: "project1".into(),
            publication_key: "nonce".into(),
            agent_id: None,
            runtime: None,
            alias: "reviewer".into(),
            active: true,
            issuer: "issuer".into(),
            subject: "sub".into(),
            expires_at: 0,
        };
        store.publish(publication.clone()).unwrap();
        publication.active = false;
        store.publish(publication.clone()).unwrap();
        publication.project = "project2".into();
        assert!(store.publish(publication).is_err());
        assert_eq!(
            store.publication("ses_test").unwrap().unwrap().project,
            "project1"
        );
    }
    #[test]
    fn replay_is_target_scoped_and_content_bound() {
        let store = store();
        let a = store.delivery("hub", "p", "m", "agent-a", "hash").unwrap();
        assert_eq!(
            a,
            store.delivery("hub", "p", "m", "agent-a", "hash").unwrap()
        );
        assert_ne!(
            a,
            store.delivery("hub", "p", "m", "agent-b", "hash").unwrap()
        );
        assert!(store.delivery("hub", "p", "m", "agent-a", "other").is_err());
    }
    #[test]
    fn durable_operations_cannot_change_identity_or_payload() {
        let store = store();
        let enqueue = |identity: &str, body| {
            store.enqueue("op".into(), identity.into(), "hub".into(), "p".into(), body)
        };
        enqueue("issuer/sub", serde_json::json!({"text":"hello"})).unwrap();
        enqueue("issuer/sub", serde_json::json!({"text":"hello"})).unwrap();
        assert!(enqueue("other/sub", serde_json::json!({"text":"hello"})).is_err());
        assert!(enqueue("issuer/sub", serde_json::json!({"text":"changed"})).is_err());
        store
            .reject("op".into(), "RevisionConflictError".into())
            .unwrap();
        assert!(store.pending("issuer/sub".into()).unwrap().is_empty());
        assert_eq!(
            store.receipts("hub".into(), "p".into()).unwrap()[0]["state"],
            "rejected"
        );
        assert!(
            enqueue(
                "issuer/sub",
                serde_json::json!({"text":"changed after completion"})
            )
            .is_err()
        );
    }
}
