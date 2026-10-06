//! Durable command intent and settlement for managed worktree setup.

use std::io;
use std::sync::Arc;

use cyber_core::worktrees::Managed;
use cyber_store::{EventRegistry, Expected, NewEvent, Store};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const STARTED: &str = "worktree.setup.command_started.1";
const SETTLED: &str = "worktree.setup.command_settled.1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum CommandResult {
    Exited { code: Option<i32> },
    Failed { message: String },
}

#[derive(Debug, PartialEq, Eq)]
pub enum CommandDecision {
    Dispatch,
    Recorded(CommandResult),
}

#[derive(Serialize, Deserialize)]
struct Started {
    digest: String,
    index: usize,
    session_id: String,
}

#[derive(Serialize, Deserialize)]
struct Settled {
    digest: String,
    index: usize,
    result: CommandResult,
}

pub(crate) fn register(registry: &mut EventRegistry) {
    registry
        .register_with(STARTED, |data| {
            serde_json::from_value::<Started>(data.clone())
                .map(|_| ())
                .map_err(|e| e.to_string())
        })
        .expect("valid setup event type");
    registry
        .register_with(SETTLED, |data| {
            serde_json::from_value::<Settled>(data.clone())
                .map(|_| ())
                .map_err(|e| e.to_string())
        })
        .expect("valid setup event type");
}

pub struct SetupJournal {
    store: Arc<Store>,
    aggregate: String,
    digest: String,
    commands: usize,
    session_id: String,
}

impl SetupJournal {
    /// Check previous intent even when the current recipe contains no commands.
    pub fn validate(&self) -> io::Result<()> {
        self.replay().map(|_| ())
    }

    /// Stable worktree identity excludes the recipe: changing commands cannot hide
    /// an earlier unfinished intent. Source commands are hashed, not stored verbatim.
    pub fn new(
        store: Arc<Store>,
        managed: &Managed,
        commands: &[String],
        session_id: &str,
    ) -> io::Result<Self> {
        if managed.id.is_empty() || !managed.ready {
            return Err(invalid(
                "Setup requires ready ownership with a creation ID; recovery is required",
            ));
        }
        let identity = serde_json::to_vec(&(
            &managed.id,
            &managed.common_dir,
            &managed.path,
            &managed.branch,
            &managed.base,
            &managed.name,
        ))
        .map_err(io::Error::other)?;
        let recipe = serde_json::to_vec(&(managed, commands)).map_err(io::Error::other)?;
        Ok(Self {
            store,
            aggregate: format!("wts_{:x}", Sha256::digest(identity)),
            digest: format!("{:x}", Sha256::digest(recipe)),
            commands: commands.len(),
            session_id: session_id.into(),
        })
    }

    pub fn start(&self, index: usize) -> io::Result<CommandDecision> {
        let (seq, slots) = self.replay()?;
        let slot = slots
            .get(index)
            .ok_or_else(|| invalid("Invalid setup command index"))?;
        match slot {
            Slot::Pending => return Err(invalid("Setup outcome unknown; recovery is required")),
            Slot::Finished(result) => return Ok(CommandDecision::Recorded(result.clone())),
            Slot::Empty => {}
        }
        if slots[..index].iter().any(|slot| {
            !matches!(
                slot,
                Slot::Finished(CommandResult::Exited { code: Some(0) })
            )
        }) {
            return Err(invalid("Earlier setup commands have not succeeded"));
        }
        let data = Started {
            digest: self.digest.clone(),
            index,
            session_id: self.session_id.clone(),
        };
        self.append(seq, STARTED, &data)?;
        Ok(CommandDecision::Dispatch)
    }

    /// Call only after the dispatched command and its owned process tree settle.
    /// A missing acknowledgement remains pending rather than allowing another launch.
    pub fn finish(&self, index: usize, result: CommandResult) -> io::Result<()> {
        let (seq, slots) = self.replay()?;
        match slots.get(index) {
            Some(Slot::Pending) => {}
            Some(Slot::Finished(saved)) if *saved == result => return Ok(()),
            _ => return Err(invalid("Setup settlement does not match a pending command")),
        }
        self.append(
            seq,
            SETTLED,
            &Settled {
                digest: self.digest.clone(),
                index,
                result,
            },
        )
    }

    fn append(&self, seq: i64, kind: &str, data: &impl Serialize) -> io::Result<()> {
        self.store
            .append(
                &self.aggregate,
                Expected::Seq(seq),
                vec![NewEvent::new(
                    kind,
                    serde_json::to_value(data).map_err(io::Error::other)?,
                )],
            )
            .map_err(io::Error::other)?;
        Ok(())
    }

    fn replay(&self) -> io::Result<(i64, Vec<Slot>)> {
        let mut seq = -1;
        let mut slots = vec![Slot::Empty; self.commands];
        loop {
            let page = self
                .store
                .read_events(&self.aggregate, seq, 500)
                .map_err(io::Error::other)?;
            for event in page.events {
                apply(&mut slots, &self.digest, &event.kind, event.data)?;
                seq = event.seq;
            }
            if !page.has_more {
                return Ok((seq, slots));
            }
        }
    }
}

#[derive(Clone)]
enum Slot {
    Empty,
    Pending,
    Finished(CommandResult),
}

fn apply(
    slots: &mut [Slot],
    expected: &str,
    kind: &str,
    data: serde_json::Value,
) -> io::Result<()> {
    match kind {
        STARTED => {
            let event: Started = serde_json::from_value(data).map_err(io::Error::other)?;
            let slot = slot(slots, expected, &event.digest, event.index)?;
            if !matches!(slot, Slot::Empty) {
                return Err(invalid("Duplicate setup command intent"));
            }
            *slot = Slot::Pending;
        }
        SETTLED => {
            let event: Settled = serde_json::from_value(data).map_err(io::Error::other)?;
            let slot = slot(slots, expected, &event.digest, event.index)?;
            if !matches!(slot, Slot::Pending) {
                return Err(invalid("Setup result has no pending intent"));
            }
            *slot = Slot::Finished(event.result);
        }
        _ => return Err(invalid("Unexpected setup journal event")),
    }
    Ok(())
}

fn slot<'a>(
    slots: &'a mut [Slot],
    expected: &str,
    digest: &str,
    index: usize,
) -> io::Result<&'a mut Slot> {
    if expected != digest {
        return Err(invalid(
            "Setup configuration or ownership changed; recovery is required",
        ));
    }
    slots
        .get_mut(index)
        .ok_or_else(|| invalid("Invalid persisted setup command index"))
}

fn invalid(message: &str) -> io::Error {
    io::Error::other(message)
}
