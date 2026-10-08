//! Checkout-scoped observed decisions; resetting counters never deletes audit events.
use cyber_store::{Expected, NewEvent, Store, StoreError, StoredEvent};
use rusqlite::{OptionalExtension, Transaction, params};
use serde::Serialize;
use sha2::{Digest, Sha256};

pub(super) const RESET: &str = "permission.auto_statistics_reset.1";

fn aggregate(root: &str) -> String {
    format!("ast_{:x}", Sha256::digest(root.as_bytes()))
}

#[derive(Debug, Serialize, Default, PartialEq, Eq)]
pub struct AutoStatistics {
    pub checkout_root: String,
    pub allowed: i64,
    pub blocked: i64,
    pub fallback: i64,
    pub classifier: i64,
    pub policy: i64,
    pub recorded_since: Option<i64>,
    pub reset_at: Option<i64>,
}

pub fn show(store: &Store, checkout_root: &str) -> Result<AutoStatistics, StoreError> {
    let root = checkout_root.to_owned();
    store.read(move |db| {
        let value = db
            .query_row(
                "SELECT allowed,blocked,fallback,classifier,policy,recorded_since,reset_at
            FROM permission_auto_statistics WHERE checkout_root=?1",
                [&root],
                |row| {
                    Ok(AutoStatistics {
                        checkout_root: root.clone(),
                        allowed: row.get(0)?,
                        blocked: row.get(1)?,
                        fallback: row.get(2)?,
                        classifier: row.get(3)?,
                        policy: row.get(4)?,
                        recorded_since: row.get(5)?,
                        reset_at: row.get(6)?,
                    })
                },
            )
            .optional()?;
        Ok(value.unwrap_or(AutoStatistics {
            checkout_root: root,
            ..Default::default()
        }))
    })
}

pub fn reset(store: &Store, checkout_root: &str) -> Result<(), StoreError> {
    store.append(
        &aggregate(checkout_root),
        Expected::Any,
        vec![NewEvent::new(
            RESET,
            serde_json::json!({"checkout_root":checkout_root}),
        )],
    )?;
    Ok(())
}

pub(super) fn project(tx: &Transaction<'_>, event: &StoredEvent) -> Result<(), String> {
    if event.kind == RESET {
        let root = event.data["checkout_root"]
            .as_str()
            .ok_or("Missing statistics reset scope")?;
        if !std::path::Path::new(root).is_absolute() || event.aggregate_id != aggregate(root) {
            return Err("Statistics reset has an invalid checkout identity".into());
        }
        tx.execute("INSERT INTO permission_auto_statistics(checkout_root,recorded_since,reset_at)
            VALUES (?1,?2,?2) ON CONFLICT(checkout_root) DO UPDATE SET allowed=0,blocked=0,
            fallback=0,classifier=0,policy=0,recorded_since=excluded.recorded_since,reset_at=excluded.reset_at",
            params![root,event.time_ms]).map_err(|e|e.to_string())?;
        return Ok(());
    }
    if event.kind != super::events::AUTO_DECIDED {
        return Ok(());
    }
    if event
        .data
        .get("checkout_root")
        .is_none_or(serde_json::Value::is_null)
    {
        return Ok(());
    }
    let decision: super::AutoDecision =
        serde_json::from_value(event.data.clone()).map_err(|e| e.to_string())?;
    // Historical decisions have no trustworthy checkout scope; do not infer one from a moved Session.
    let Some(root) = decision.checkout_root else {
        return Ok(());
    };
    if !std::path::Path::new(&root).is_absolute() {
        return Err("Auto statistics require an absolute checkout scope".into());
    }
    let (allow, block, fallback) = match decision.decision {
        super::AutoEffect::Allow => (1, 0, 0),
        super::AutoEffect::Block => (0, 1, 0),
        super::AutoEffect::Fallback => (0, 0, 1),
    };
    let classifier = i64::from(decision.model.is_some());
    let policy =
        i64::from(decision.model.is_none() && decision.decision != super::AutoEffect::Fallback);
    tx.execute("INSERT INTO permission_auto_statistics
        (checkout_root,allowed,blocked,fallback,classifier,policy,recorded_since)
        VALUES (?1,?2,?3,?4,?5,?6,?7) ON CONFLICT(checkout_root) DO UPDATE SET
        allowed=allowed+excluded.allowed,blocked=blocked+excluded.blocked,fallback=fallback+excluded.fallback,
        classifier=classifier+excluded.classifier,policy=policy+excluded.policy",
        params![root,allow,block,fallback,classifier,policy,event.time_ms]).map_err(|e|e.to_string())?;
    Ok(())
}
