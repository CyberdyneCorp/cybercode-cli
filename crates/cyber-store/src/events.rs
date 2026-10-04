//! Event types, the registry of known schemas, upcasters and projectors.

use std::collections::HashMap;
use std::sync::Arc;

use rusqlite::{OptionalExtension, Transaction, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::StoreError;

/// A versioned event type `<domain>.<name>.<version>`, for example `session.prompt.admitted.1`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventType {
    pub base: String,
    pub version: u32,
}

impl EventType {
    pub fn parse(kind: &str) -> Result<Self, StoreError> {
        let invalid = || StoreError::InvalidEventType(kind.to_string());
        let (base, version) = kind.rsplit_once('.').ok_or_else(invalid)?;
        let version: u32 = version.parse().map_err(|_| invalid())?;
        let segments: Vec<&str> = base.split('.').collect();
        let valid_segment = |s: &&str| {
            !s.is_empty()
                && s.bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
        };
        if version == 0 || segments.len() < 2 || !segments.iter().all(valid_segment) {
            return Err(invalid());
        }
        Ok(Self {
            base: base.to_string(),
            version,
        })
    }

    pub fn kind(&self) -> String {
        format!("{}.{}", self.base, self.version)
    }
}

/// An event to append.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NewEvent {
    pub kind: String,
    pub data: Value,
    pub causation_id: Option<String>,
}

impl NewEvent {
    pub fn new(kind: impl Into<String>, data: Value) -> Self {
        Self {
            kind: kind.into(),
            data,
            causation_id: None,
        }
    }
}

/// An event as committed. `kind` is upcast to the current registered version on read.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct StoredEvent {
    pub id: String,
    pub aggregate_id: String,
    pub seq: i64,
    pub kind: String,
    pub data: Value,
    pub time_ms: i64,
    pub causation_id: Option<String>,
}

/// Optimistic concurrency for an append.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Expected {
    Any,
    /// The aggregate's latest seq must equal this value (`-1` for an empty aggregate).
    Seq(i64),
}

type Validator = Arc<dyn Fn(&Value) -> Result<(), String> + Send + Sync>;
type Upcaster = Arc<dyn Fn(Value) -> Value + Send + Sync>;
/// Runs inside the append transaction; an error rolls back the events and projections.
pub type Projector =
    Arc<dyn Fn(&Transaction<'_>, &StoredEvent) -> Result<(), String> + Send + Sync>;

/// Known event types. Writing an unregistered type is a defect that fails the transaction.
#[derive(Clone, Default)]
pub struct EventRegistry {
    validators: HashMap<String, Option<Validator>>,
    current: HashMap<String, u32>,
    upcasters: HashMap<(String, u32), Upcaster>,
    projectors: Vec<Projector>,
}

impl EventRegistry {
    pub fn register(&mut self, kind: &str) -> Result<&mut Self, StoreError> {
        self.insert(kind, None)
    }

    pub fn register_with(
        &mut self,
        kind: &str,
        validator: impl Fn(&Value) -> Result<(), String> + Send + Sync + 'static,
    ) -> Result<&mut Self, StoreError> {
        self.insert(kind, Some(Arc::new(validator)))
    }

    /// Convert data stored as `from_kind` (version N) to version N+1.
    pub fn upcaster(
        &mut self,
        from_kind: &str,
        upcast: impl Fn(Value) -> Value + Send + Sync + 'static,
    ) -> Result<&mut Self, StoreError> {
        let t = EventType::parse(from_kind)?;
        self.upcasters.insert((t.base, t.version), Arc::new(upcast));
        Ok(self)
    }

    pub fn projector(
        &mut self,
        projector: impl Fn(&Transaction<'_>, &StoredEvent) -> Result<(), String> + Send + Sync + 'static,
    ) -> &mut Self {
        self.projectors.push(Arc::new(projector));
        self
    }

    fn insert(
        &mut self,
        kind: &str,
        validator: Option<Validator>,
    ) -> Result<&mut Self, StoreError> {
        let t = EventType::parse(kind)?;
        let current = self.current.entry(t.base.clone()).or_insert(t.version);
        *current = (*current).max(t.version);
        self.validators.insert(t.kind(), validator);
        Ok(self)
    }

    pub(crate) fn check(&self, event: &NewEvent) -> Result<(), StoreError> {
        EventType::parse(&event.kind)?;
        match self.validators.get(&event.kind) {
            None => Err(StoreError::UnregisteredEvent(event.kind.clone())),
            Some(Some(validate)) => {
                validate(&event.data).map_err(|reason| StoreError::InvalidEvent {
                    kind: event.kind.clone(),
                    reason,
                })
            }
            Some(None) => Ok(()),
        }
    }

    /// Apply registered upcasters until the current version is reached.
    pub(crate) fn upcast(&self, kind: &str, mut data: Value) -> (String, Value) {
        let Ok(mut t) = EventType::parse(kind) else {
            return (kind.to_string(), data);
        };
        while let Some(up) = self.upcasters.get(&(t.base.clone(), t.version)) {
            data = up(data);
            t.version += 1;
        }
        (t.kind(), data)
    }
}

pub(crate) fn append_in_tx(
    tx: &Transaction<'_>,
    registry: &EventRegistry,
    aggregate_id: &str,
    expected: Expected,
    events: Vec<NewEvent>,
) -> Result<Vec<StoredEvent>, StoreError> {
    let current: i64 = tx
        .query_row(
            "SELECT seq FROM event_sequence WHERE aggregate_id = ?1",
            [aggregate_id],
            |r| r.get(0),
        )
        .optional()?
        .unwrap_or(-1);
    if let Expected::Seq(seq) = expected
        && seq != current
    {
        return Err(StoreError::Concurrency {
            aggregate: aggregate_id.into(),
            expected: seq,
            actual: current,
        });
    }
    let now = now_ms();
    let mut stored = Vec::with_capacity(events.len());
    for (offset, event) in (1_i64..).zip(events) {
        let row = StoredEvent {
            id: cyber_core::ids::new_id("evt"),
            aggregate_id: aggregate_id.to_string(),
            seq: current + offset,
            kind: event.kind,
            data: event.data,
            time_ms: now,
            causation_id: event.causation_id,
        };
        insert_event(tx, &row)?;
        for project in &registry.projectors {
            project(tx, &row).map_err(|reason| StoreError::Projector {
                kind: row.kind.clone(),
                reason,
            })?;
        }
        stored.push(row);
    }
    if let Some(last) = stored.last() {
        tx.execute(
            "INSERT INTO event_sequence (aggregate_id, seq) VALUES (?1, ?2)
             ON CONFLICT(aggregate_id) DO UPDATE SET seq = excluded.seq",
            params![aggregate_id, last.seq],
        )?;
    }
    Ok(stored)
}

fn insert_event(tx: &Transaction<'_>, e: &StoredEvent) -> Result<(), StoreError> {
    tx.execute(
        "INSERT INTO event (id, aggregate_id, seq, type, data, time, causation_id)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            e.id,
            e.aggregate_id,
            e.seq,
            e.kind,
            e.data.to_string(),
            e.time_ms,
            e.causation_id
        ],
    )?;
    Ok(())
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_type_format() {
        assert_eq!(
            EventType::parse("session.prompt.admitted.1").unwrap().base,
            "session.prompt.admitted"
        );
        for bad in [
            "session",
            "session.1",
            "Session.x.1",
            "a.b.0",
            "a.b.x",
            "a..b.1",
        ] {
            assert!(EventType::parse(bad).is_err(), "{bad} should be rejected");
        }
    }
}
