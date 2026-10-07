//! Durable command intent and settlement for managed worktree setup.

use std::io;
use std::sync::Arc;

use cyber_core::worktrees::Managed;
use cyber_store::{EventRegistry, Expected, NewEvent, Store};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const STARTED: &str = "worktree.setup.command_started.1";
const RETRY: &str = "worktree.setup.command_retry_authorized.1";
const SETTLED: &str = "worktree.setup.command_settled.1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum CommandResult {
    Exited {
        code: Option<i32>,
    },
    Failed {
        message: String,
    },
    /// Command preparation failed before entering the process launch boundary.
    NotDispatched {
        message: String,
    },
}

#[derive(Debug, PartialEq, Eq)]
pub enum CommandDecision {
    Dispatch,
    Recorded(CommandResult),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum CommandStatus {
    NotStarted,
    Pending,
    Finished { result: CommandResult },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct SetupSnapshot {
    pub revision: i64,
    pub digest: String,
    pub commands: Vec<CommandStatus>,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetupRecoveryRequest {
    pub revision: i64,
    pub digest: String,
    /// Omit to continue only undispatched commands or acknowledge already completed setup.
    pub retry_index: Option<usize>,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ChildSetupInspection {
    pub session_id: String,
    pub worktree_id: String,
    pub setup_pending: bool,
    pub journal: SetupSnapshot,
}

#[derive(Debug)]
pub struct CommandAttempt {
    pub decision: CommandDecision,
    pub started_revision: Option<i64>,
}

#[derive(Serialize, Deserialize)]
struct RetryAuthorized {
    digest: String,
    index: usize,
    session_id: String,
    reason: String,
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
        .register_with(RETRY, |data| {
            let retry: RetryAuthorized =
                serde_json::from_value(data.clone()).map_err(|e| e.to_string())?;
            validate_reason(&retry.reason).map_err(|e| e.to_string())
        })
        .expect("valid setup retry event type");
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

    pub fn snapshot(&self) -> io::Result<SetupSnapshot> {
        let (revision, slots) = self.replay()?;
        Ok(SetupSnapshot {
            revision,
            digest: self.digest.clone(),
            commands: slots
                .iter()
                .map(|slot| match slot {
                    Slot::Empty { .. } => CommandStatus::NotStarted,
                    Slot::Pending { .. } => CommandStatus::Pending,
                    Slot::Finished { result, .. } => CommandStatus::Finished {
                        result: result.clone(),
                    },
                })
                .collect(),
        })
    }

    pub fn check_review(&self, revision: i64, digest: &str) -> io::Result<()> {
        let (seq, _) = self.replay()?;
        if seq != revision || digest != self.digest {
            return Err(invalid("Setup recovery review is stale"));
        }
        Ok(())
    }

    /// Trusted lifecycle callers must hold native activity and child ownership.
    /// Retry requires a failed exit or verified failure before launch, never an unknown outcome.
    pub fn retry_failed(
        &self,
        revision: i64,
        digest: &str,
        index: usize,
        reason: &str,
    ) -> io::Result<()> {
        validate_reason(reason)?;
        self.check_review(revision, digest)?;
        let (seq, slots) = self.replay()?;
        if seq != revision {
            return Err(invalid("Setup recovery review is stale"));
        }
        check_retry(&slots, index)?;
        self.append(
            seq,
            RETRY,
            &RetryAuthorized {
                digest: self.digest.clone(),
                index,
                session_id: self.session_id.clone(),
                reason: reason.into(),
            },
        )
    }

    pub fn start(&self, index: usize) -> io::Result<CommandDecision> {
        self.start_attempt(index).map(|attempt| attempt.decision)
    }

    pub fn start_attempt(&self, index: usize) -> io::Result<CommandAttempt> {
        let (seq, slots) = self.replay()?;
        let slot = slots
            .get(index)
            .ok_or_else(|| invalid("Invalid setup command index"))?;
        match slot {
            Slot::Pending { .. } => {
                return Err(invalid("Setup outcome unknown; recovery is required"));
            }
            Slot::Finished { result, .. } => {
                return Ok(CommandAttempt {
                    decision: CommandDecision::Recorded(result.clone()),
                    started_revision: None,
                });
            }
            Slot::Empty { .. } => {}
        }
        check_prefix(&slots, index)?;
        self.append(
            seq,
            STARTED,
            &Started {
                digest: self.digest.clone(),
                index,
                session_id: self.session_id.clone(),
            },
        )?;
        Ok(CommandAttempt {
            decision: CommandDecision::Dispatch,
            started_revision: Some(seq + 1),
        })
    }

    /// Legacy settlement is allowed only for commands which have never been retried.
    pub fn finish(&self, index: usize, result: CommandResult) -> io::Result<()> {
        self.finish_owned(index, None, result)
    }

    /// The dispatch revision prevents a late acknowledgement settling a newer attempt.
    pub fn finish_attempt(
        &self,
        index: usize,
        started_revision: i64,
        result: CommandResult,
    ) -> io::Result<()> {
        self.finish_owned(index, Some(started_revision), result)
    }

    fn finish_owned(
        &self,
        index: usize,
        expected: Option<i64>,
        result: CommandResult,
    ) -> io::Result<()> {
        let (seq, slots) = self.replay()?;
        let slot = slots
            .get(index)
            .ok_or_else(|| invalid("Invalid setup command index"))?;
        let (started, retried) = match slot {
            Slot::Pending { started, retried }
            | Slot::Finished {
                started, retried, ..
            } => (*started, *retried),
            _ => return Err(invalid("Setup settlement does not match a pending command")),
        };
        if expected.is_some_and(|revision| revision != started) || (expected.is_none() && retried) {
            return Err(invalid("Setup settlement belongs to another attempt"));
        }
        match slot {
            Slot::Pending { .. } => {}
            Slot::Finished { result: saved, .. } if *saved == result => return Ok(()),
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
        let mut slots = vec![Slot::Empty { retried: false }; self.commands];
        loop {
            let page = self
                .store
                .read_events(&self.aggregate, seq, 500)
                .map_err(io::Error::other)?;
            for event in page.events {
                apply(&mut slots, &self.digest, &event.kind, event.seq, event.data)?;
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
    Empty {
        retried: bool,
    },
    Pending {
        started: i64,
        retried: bool,
    },
    Finished {
        started: i64,
        retried: bool,
        result: CommandResult,
    },
}

fn apply(
    slots: &mut [Slot],
    expected: &str,
    kind: &str,
    seq: i64,
    data: serde_json::Value,
) -> io::Result<()> {
    match kind {
        STARTED => {
            let event: Started = serde_json::from_value(data).map_err(io::Error::other)?;
            check_prefix(slots, event.index)?;
            let slot = slot(slots, expected, &event.digest, event.index)?;
            let Slot::Empty { retried } = slot else {
                return Err(invalid("Duplicate setup command intent"));
            };
            *slot = Slot::Pending {
                started: seq,
                retried: *retried,
            };
        }
        SETTLED => {
            let event: Settled = serde_json::from_value(data).map_err(io::Error::other)?;
            let slot = slot(slots, expected, &event.digest, event.index)?;
            let Slot::Pending { started, retried } = slot else {
                return Err(invalid("Setup result has no pending intent"));
            };
            *slot = Slot::Finished {
                started: *started,
                retried: *retried,
                result: event.result,
            };
        }
        RETRY => {
            let event: RetryAuthorized = serde_json::from_value(data).map_err(io::Error::other)?;
            validate_reason(&event.reason)?;
            check_retry(slots, event.index)?;
            *slot(slots, expected, &event.digest, event.index)? = Slot::Empty { retried: true };
        }
        _ => return Err(invalid("Unexpected setup journal event")),
    }
    Ok(())
}

fn check_prefix(slots: &[Slot], index: usize) -> io::Result<()> {
    let prefix = slots
        .get(..index)
        .ok_or_else(|| invalid("Invalid setup command index"))?;
    if prefix.iter().any(|slot| {
        !matches!(
            slot,
            Slot::Finished {
                result: CommandResult::Exited { code: Some(0) },
                ..
            }
        )
    }) {
        return Err(invalid("Earlier setup commands have not succeeded"));
    }
    Ok(())
}

fn check_retry(slots: &[Slot], index: usize) -> io::Result<()> {
    check_prefix(slots, index)?;
    match slots.get(index) {
        Some(Slot::Finished {
            result: CommandResult::Exited { code: Some(0) },
            ..
        }) => return Err(invalid("Successful setup commands cannot be retried")),
        Some(Slot::Finished {
            result: CommandResult::Exited { .. } | CommandResult::NotDispatched { .. },
            ..
        }) => {}
        _ => return Err(invalid("Setup retry requires a settled failed command")),
    }
    if slots[index + 1..]
        .iter()
        .any(|slot| !matches!(slot, Slot::Empty { .. }))
    {
        return Err(invalid("Later setup command intent prevents retry"));
    }
    Ok(())
}

fn validate_reason(reason: &str) -> io::Result<()> {
    if reason.trim().is_empty() || reason.len() > 1024 {
        return Err(invalid("Setup retry reason must contain 1-1024 bytes"));
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
