//! Cross-session messaging on one server (`cross-session-messaging`).
//!
//! A message is two durable records, one per Session side (`in` in the target, `out` in
//! the sender), projected into `xsession_message`; idle subscriptions live in
//! `messaging_watch`. Both tables are rebuildable from the `messaging.*` events.

pub mod address;
pub mod collaboration;
pub use collaboration::{CollaborationAuthorizer, CollaborationOrigin};
pub mod history;
pub(crate) mod hops;
pub mod inbound;
pub(crate) mod inbox;
pub mod limits;
pub mod listing;
pub mod peer;
pub mod peer_error;
pub mod receive;
pub mod records;
mod recovery;
mod remote;
pub mod render;
pub mod send;
pub mod tracker;
pub mod watch;

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use cyber_core::config::MessagingSettings;
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};

pub use address::{AddressError, AddressKind, Candidate, REACHABLE_LIMIT, SessionLocation, Target};
pub use history::{HistoryPage, HistoryQuery, MessageRecord, Order};
pub use inbound::{InboundOverride, OverrideReason};
pub use limits::MAX_MESSAGE_BYTES;
pub use listing::{PeerSessionStatus, RemoteReach};
pub use peer::{PeerClient, PeerTarget, RemoteHello};
pub use peer_error::{PeerAuthReason, PeerError, PeerErrorTag, PeerScope};
pub use records::{Direction, MessageChain, MessageRow, WatchRow};
pub use send::{Outcome, SendRequest, SendResult, Sender, SentTo};
pub use tracker::Tracked;
pub use watch::{Swept, Watch, WatchRequest, WatchStatus};

use super::{Runtime, RuntimeError};
use inbound::{Reach, SenderAddresses, Verdict};
pub use receive::{PeerMessage, PeerOutcome, PeerSendResult};

/// What `target` does with a message from a sender known by `addresses`, under its
/// Location's `settings` (`None`: unreadable, so disabled): an archived target or a
/// disabled Location refuses before the inbound rules run.
fn inbound_verdict(
    target: &Target,
    settings: Option<&MessagingSettings>,
    addresses: &SenderAddresses,
    reach: Reach,
) -> Verdict {
    match settings {
        _ if target.archived => Verdict::Refuse("archived"),
        Some(settings) if settings.enabled => {
            inbound::decide(&settings.inbound, target.inbound.as_ref(), addresses, reach)
        }
        _ => Verdict::Refuse("disabled"),
    }
}

impl Runtime {
    /// The message records of a Session (both directions), in record order.
    pub fn message_rows(&self, session_id: &str) -> Result<Vec<MessageRow>, RuntimeError> {
        let id = session_id.to_owned();
        Ok(self
            .inner
            .store
            .read(move |db| Ok(records::message_rows(db, &id)?))?)
    }

    /// The idle-notification subscriptions a Session created.
    pub fn watch_rows(&self, subscriber_id: &str) -> Result<Vec<WatchRow>, RuntimeError> {
        let id = subscriber_id.to_owned();
        Ok(self
            .inner
            .store
            .read(move |db| Ok(records::watch_rows(db, &id)?))?)
    }

    /// Set or clear a Session's `messaging.inbound` override (`PATCH /sessions/:id`). It
    /// replaces only the default of its Location; sender patterns still win.
    pub async fn set_inbound(
        &self,
        session_id: &str,
        messaging: Option<InboundOverride>,
    ) -> Result<(), RuntimeError> {
        let handle = self.inner.handle(session_id).await?;
        let mut state = handle.state.lock().await;
        if state.info.messaging == messaging {
            return Ok(());
        }
        let update = super::events::MessagingUpdated { messaging };
        self.inner.commit_locked(
            &mut state,
            vec![super::events::event(
                super::events::MESSAGING_UPDATED,
                &update,
            )],
        )?;
        Ok(())
    }

    /// Rebuild `xsession_message` and `messaging_watch` from the `messaging.*` events.
    /// Returns the number of events re-projected.
    pub fn rebuild_messaging(&self) -> Result<usize, RuntimeError> {
        Ok(self.inner.store.reproject(
            &records::TABLES,
            records::PREFIX,
            Arc::new(records::project),
        )?)
    }
}

/// What a reachable Session is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ReachableState {
    Idle,
    /// A Drain runs.
    Running,
    /// A permission request or question of the Session waits for a client.
    WaitingInput,
    /// A configured peer that did not answer: one `*@<peer>` entry.
    Offline,
}

/// One entry of the reachable listing (`list_sessions`, `GET /messaging/sessions`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ReachableSession {
    /// What to pass as `to`: the name when it resolves to this Session, else the ID.
    pub address: String,
    pub session_id: String,
    pub name: String,
    pub kind: AddressKind,
    pub state: ReachableState,
    pub location: SessionLocation,
    /// Whether a message from this caller would be accepted: false for an archived
    /// Session, where messaging is disabled, and where its inbound decision for this
    /// caller is `refuse`.
    #[serde(rename = "canReceive")]
    pub can_receive: bool,
}

/// The messaging facade every surface (tools, HTTP, MCP serve, the CLI over HTTP) uses.
#[derive(Clone)]
pub struct Messaging {
    runtime: Runtime,
}

impl Runtime {
    pub fn messaging(&self) -> Messaging {
        Messaging {
            runtime: self.clone(),
        }
    }
}

impl Messaging {
    /// Resolve `to` relative to the sending Session `from` (none for a client).
    /// An `X@P` address resolves to the configured peer `P` (`PeerNotConfiguredError`
    /// without one); the peer resolves `X` when the message arrives.
    pub fn resolve(&self, from: Option<&str>, to: &str) -> Result<Target, RuntimeError> {
        let (from, to) = (from.map(str::to_owned), to.to_owned());
        self.caller_exists(from.as_deref())?;
        if let Some((session, peer)) = send::remote_address(&to) {
            self.peer_target(&peer)?;
            return Ok(Target::remote(&session, &peer));
        }
        let found = self
            .runtime
            .inner
            .store
            .read(move |db| Ok(address::resolve(db, from.as_deref(), &to)?))?;
        found.map_err(RuntimeError::Address)
    }

    /// The targets of a send from `from` to `to`: for a team member's `all`, every other
    /// member of its team that is not `stopped`, each addressed by its member name (none
    /// is `AddressNotFoundError`); else the one target [`Self::resolve`] finds. Each target
    /// is sent its own message (`agent-teams` → Teammate messaging).
    pub fn resolve_targets(
        &self,
        from: Option<&str>,
        to: &str,
    ) -> Result<Vec<Target>, RuntimeError> {
        if let Some(caller) = from
            && to.strip_prefix('@').unwrap_or(to) == address::ALL
        {
            self.caller_exists(Some(caller))?;
            let caller = caller.to_owned();
            let all = self
                .runtime
                .inner
                .store
                .read(move |db| Ok(address::team_all(db, &caller)?))?;
            match all {
                Some(targets) if targets.is_empty() => {
                    return Err(RuntimeError::Address(AddressError::NotFound(to.into())));
                }
                Some(targets) => return Ok(targets),
                None => {}
            }
        }
        self.resolve(from, to).map(|target| vec![target])
    }

    /// The Sessions `from` can message, newest first, at most [`REACHABLE_LIMIT`], in
    /// `directory` when given. The caller itself is never listed; a client caller is the
    /// `api` client.
    pub fn reachable(
        &self,
        from: Option<&str>,
        directory: Option<&str>,
    ) -> Result<Vec<ReachableSession>, RuntimeError> {
        let sender = match from {
            Some(id) => Sender::Session(id.to_owned()),
            None => Sender::Client(send::DEFAULT_CLIENT.into()),
        };
        self.reachable_as(&sender, directory)
    }

    /// [`Self::reachable`] for any sender: `canReceive` is false where the target's
    /// inbound decision for this sender is `refuse`, messaging is disabled or the target
    /// is archived.
    pub fn reachable_as(
        &self,
        sender: &Sender,
        directory: Option<&str>,
    ) -> Result<Vec<ReachableSession>, RuntimeError> {
        let from = sender.session_id().map(str::to_owned);
        self.caller_exists(from.as_deref())?;
        let addresses = self.sender_addresses(sender)?;
        let directory = directory.map(str::to_owned);
        let targets = self.runtime.inner.store.read(move |db| {
            Ok(address::reachable(
                db,
                from.as_deref(),
                directory.as_deref(),
            )?)
        })?;
        Ok(self.describe(targets, sender, &addresses))
    }

    /// The root Session of this server `id` names (its ID, or a name that resolves to one
    /// Session) as [`Self::reachable_as`] would list it for `sender`, without the listing's
    /// limit; `None` when nothing (or a subagent or an archived Session) has that address.
    pub fn reachable_one_as(
        &self,
        sender: &Sender,
        id: &str,
    ) -> Result<Option<ReachableSession>, RuntimeError> {
        let addresses = self.sender_addresses(sender)?;
        let id = id.to_owned();
        let found = self
            .runtime
            .inner
            .store
            .read(move |db| Ok(address::resolve(db, None, &id)?))?;
        let Ok(target) = found else {
            return Ok(None);
        };
        if target.archived || target.candidate.kind != AddressKind::Local {
            return Ok(None);
        }
        Ok(self.describe(vec![target], sender, &addresses).pop())
    }

    /// `targets` as listing entries for `sender`: what each is doing, and whether it would
    /// accept a message from `sender`.
    fn describe(
        &self,
        targets: Vec<Target>,
        sender: &Sender,
        addresses: &SenderAddresses,
    ) -> Vec<ReachableSession> {
        let mut settings: HashMap<String, Option<MessagingSettings>> = HashMap::new();
        let running = self.runtime.running_sessions();
        let waiting: HashSet<String> = self
            .runtime
            .pending_requests(None)
            .into_iter()
            .flat_map(|r| {
                let routes = r.routed_to.into_iter().map(|route| route.session_id);
                std::iter::once(r.session_id).chain(routes)
            })
            .collect();
        targets
            .into_iter()
            .map(|t| {
                let id = &t.candidate.session_id;
                let state = if waiting.contains(id) {
                    ReachableState::WaitingInput
                } else if running.contains(id) {
                    ReachableState::Running
                } else {
                    ReachableState::Idle
                };
                let directory = &t.candidate.location.directory;
                let located = settings
                    .entry(directory.clone())
                    .or_insert_with(|| self.settings(Some(directory)));
                let can_receive = !matches!(
                    inbound_verdict(&t, located.as_ref(), addresses, sender.reach()),
                    Verdict::Refuse(_)
                );
                ReachableSession {
                    address: t.address,
                    session_id: t.candidate.session_id,
                    name: t.candidate.name,
                    kind: t.candidate.kind,
                    state,
                    location: t.candidate.location,
                    can_receive,
                }
            })
            .collect()
    }

    /// Whether message `id` has a record (its `in` record in any Session, or `sender`'s
    /// own `out` record), so that sending it again replays its outcome.
    pub fn recorded(&self, id: &str, sender: &Sender) -> Result<bool, RuntimeError> {
        let (id, sender) = (id.to_owned(), sender.session_id().map(str::to_owned));
        Ok(self
            .runtime
            .inner
            .store
            .read(move |db| Ok(records::recorded(db, &id, sender.as_deref())?))?
            .is_some())
    }

    /// The addresses a sender's inbound patterns match: a Session's (see
    /// [`address::sender_addresses`]); `client:<label>` and `client` for an API client;
    /// `mcp:<label>` and `client` for an MCP serve client without a caller Session.
    fn sender_addresses(&self, sender: &Sender) -> Result<SenderAddresses, RuntimeError> {
        match sender {
            Sender::Session(id) => {
                let id = id.clone();
                Ok(self
                    .runtime
                    .inner
                    .store
                    .read(move |db| Ok(address::sender_addresses(db, &id)?))?)
            }
            Sender::Client(_) | Sender::Mcp(_) => Ok(SenderAddresses::trusted(vec![
                sender.key(),
                "client".into(),
            ])),
            // Named inside the grant's namespace, which no other machine can claim.
            Sender::Peer { .. } => Ok(SenderAddresses::trusted(sender.peer_addresses())),
            Sender::Collaboration(origin) => Ok(SenderAddresses {
                trusted: Vec::new(),
                other: vec![origin.label()],
            }),
        }
    }

    /// The messaging settings at `directory` (`None`: the process scope); `None` when
    /// they cannot be read, which callers treat as disabled.
    fn settings(&self, directory: Option<&str>) -> Option<MessagingSettings> {
        self.runtime.inner.tools.messaging_settings(directory).ok()
    }

    /// Whether messaging is enabled at `directory` (`None`: the process scope: the
    /// environment, the managed policy and the global configuration). Settings that
    /// cannot be read disable it.
    pub fn enabled(&self, directory: Option<&str>) -> bool {
        self.settings(directory)
            .is_some_and(|settings| settings.enabled)
    }

    /// The Location of a Session of this server.
    fn session_directory(&self, id: &str) -> Result<String, RuntimeError> {
        let probe = id.to_owned();
        let directory = self.runtime.inner.store.read(move |db| {
            Ok(db
                .query_row(
                    "SELECT directory FROM session WHERE id = ?1",
                    [probe],
                    |row| row.get::<_, String>(0),
                )
                .optional()?)
        })?;
        directory.ok_or_else(|| RuntimeError::SessionNotFound(id.to_owned()))
    }

    /// Step 1 of a send: messaging is enabled where the sender runs (its Location, or the
    /// process scope for a client). Nothing is written otherwise. Returns those settings.
    fn sender_enabled(&self, sender: &Sender) -> Result<MessagingSettings, RuntimeError> {
        let directory = match sender {
            Sender::Session(id) => Some(self.session_directory(id)?),
            // A peer's message is received under this machine's process scope.
            Sender::Client(_) | Sender::Mcp(_) | Sender::Peer { .. } | Sender::Collaboration(_) => {
                None
            }
        };
        self.settings(directory.as_deref())
            .filter(|settings| settings.enabled)
            .ok_or(RuntimeError::Disabled("messaging"))
    }

    /// A named caller must be a Session of this server.
    fn caller_exists(&self, from: Option<&str>) -> Result<(), RuntimeError> {
        let Some(id) = from.map(str::to_owned) else {
            return Ok(());
        };
        let probe = id.clone();
        let exists = self.runtime.inner.store.read(move |db| {
            Ok(db
                .prepare("SELECT 1 FROM session WHERE id = ?1")?
                .exists([probe])?)
        })?;
        if exists {
            Ok(())
        } else {
            Err(RuntimeError::SessionNotFound(id))
        }
    }
}
