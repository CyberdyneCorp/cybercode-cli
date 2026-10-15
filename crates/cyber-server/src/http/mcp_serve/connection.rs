//! MCP connections: `Mcp-Session-Id`s minted on `initialize`, kept in memory only and
//! ended by `DELETE`, a server restart or 30 idle minutes. Ending one ends its waits
//! (never its Sessions).

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use axum::http::HeaderMap;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use super::super::{ApiError, AppState, EmbeddedClient};
use crate::runtime::{RunMarker, Runtime};

/// An MCP session with no request in progress for this long has expired.
const IDLE: Duration = Duration::from_secs(30 * 60);

/// The time source of the session table; tests inject one to expire sessions.
pub type Clock = Arc<dyn Fn() -> Instant + Send + Sync>;

/// One MCP connection: the in-process client its tools call the routes through, its
/// attribution label and the Sessions it has created.
pub(super) struct Connection {
    pub client: EmbeddedClient,
    pub state: AppState,
    /// `mcp_client`: attribution only, never an identity.
    pub label: String,
    /// Sessions created (or being created) by this MCP connection.
    created: AtomicU32,
    /// Waiting `cyber_prompt` and `cyber_prompt_wait` calls (`max_waits`).
    waits: AtomicU32,
    /// In-flight `tools/call` requests by JSON-RPC id, for `notifications/cancelled`.
    calls: Mutex<HashMap<String, CancellationToken>>,
    /// Cancelled when the MCP session ends: every wait of the connection ends with it.
    closed: CancellationToken,
    /// State shared by every connection of the server.
    pub shared: Arc<Shared>,
}

/// What MCP serve tracks across its connections.
#[derive(Default)]
pub(super) struct Shared {
    runs: Mutex<Runs>,
}

/// `max_running_sessions` bookkeeping.
#[derive(Default)]
struct Runs {
    /// Sessions whose run an MCP prompt started, with that run: a later run of the
    /// Session (another marker) is not counted.
    started: HashMap<String, RunMarker>,
    /// Prompts between their concurrency check and their admission.
    reserved: usize,
}

impl Shared {
    /// Reserve a run start while fewer than `limit` Sessions driven through MCP serve run
    /// (the `running` Sessions in `created`, or whose current run an MCP prompt started)
    /// counting the reservations still in flight. `Err` carries that count.
    pub fn reserve_run(
        self: &Arc<Self>,
        runtime: &Runtime,
        running: &HashSet<String>,
        created: &HashSet<String>,
        limit: usize,
    ) -> Result<RunSlot, usize> {
        let mut runs = lock(&self.runs);
        runs.started
            .retain(|id, run| runtime.run_marker(id).as_ref() == Some(run));
        let driven = running
            .iter()
            .filter(|id| created.contains(*id) || runs.started.contains_key(*id))
            .count();
        let count = driven + runs.reserved;
        if count >= limit {
            return Err(count);
        }
        runs.reserved += 1;
        Ok(RunSlot {
            shared: Arc::clone(self),
        })
    }
}

/// A reserved run start; dropping it without [`RunSlot::started`] gives it back.
pub(super) struct RunSlot {
    shared: Arc<Shared>,
}

impl RunSlot {
    /// The prompt was admitted to `session_id`: its run (if still running) counts from
    /// now on, in place of the reservation.
    pub fn started(self, runtime: &Runtime, session_id: &str) {
        if let Some(run) = runtime.run_marker(session_id) {
            lock(&self.shared.runs)
                .started
                .insert(session_id.to_owned(), run);
        }
    }
}

impl Drop for RunSlot {
    fn drop(&mut self) {
        let mut runs = lock(&self.shared.runs);
        runs.reserved = runs.reserved.saturating_sub(1);
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Connection {
    /// Reserve one Session creation under `limit`; `None` when the quota is used up.
    /// Dropping the reservation without `keep` gives the slot back.
    pub fn reserve_creation(self: &Arc<Self>, limit: u32) -> Option<Creation> {
        reserve(&self.created, limit).then(|| Creation {
            connection: Arc::clone(self),
            kept: false,
        })
    }

    /// Reserve one waiting call under `limit`; the slot is freed when the guard drops.
    pub fn reserve_wait(self: &Arc<Self>, limit: u32) -> Option<WaitSlot> {
        reserve(&self.waits, limit).then(|| WaitSlot(Arc::clone(self)))
    }

    /// Track an in-flight `tools/call`; its token is cancelled by
    /// `notifications/cancelled` for `id` or by the end of the MCP session.
    pub fn track(self: &Arc<Self>, id: &Value) -> Tracked {
        let token = self.closed.child_token();
        lock(&self.calls).insert(id.to_string(), token.clone());
        Tracked {
            connection: Arc::clone(self),
            id: id.to_string(),
            token,
        }
    }

    /// `notifications/cancelled`: end the wait of request `id`, if any.
    pub fn cancel(&self, id: &Value) {
        if let Some(token) = lock(&self.calls).get(&id.to_string()) {
            token.cancel();
        }
    }
}

fn reserve(counter: &AtomicU32, limit: u32) -> bool {
    let mut n = counter.load(Ordering::SeqCst);
    while n < limit {
        match counter.compare_exchange_weak(n, n + 1, Ordering::SeqCst, Ordering::SeqCst) {
            Ok(_) => return true,
            Err(current) => n = current,
        }
    }
    false
}

/// A reserved Session creation of one MCP connection.
pub(super) struct Creation {
    connection: Arc<Connection>,
    kept: bool,
}

impl Creation {
    /// The Session was created: it counts toward the connection's quota.
    pub fn keep(mut self) {
        self.kept = true;
    }
}

impl Drop for Creation {
    fn drop(&mut self) {
        if !self.kept {
            self.connection.created.fetch_sub(1, Ordering::SeqCst);
        }
    }
}

/// One of the connection's `max_waits` slots, held while a call waits.
pub(super) struct WaitSlot(Arc<Connection>);

impl Drop for WaitSlot {
    fn drop(&mut self) {
        self.0.waits.fetch_sub(1, Ordering::SeqCst);
    }
}

/// An in-flight `tools/call` and its cancellation.
pub(super) struct Tracked {
    connection: Arc<Connection>,
    id: String,
    pub token: CancellationToken,
}

impl Drop for Tracked {
    fn drop(&mut self) {
        let mut calls = lock(&self.connection.calls);
        if calls.get(&self.id).is_some_and(|t| t == &self.token) {
            calls.remove(&self.id);
        }
    }
}

struct Slot {
    connection: Arc<Connection>,
    last_seen: Instant,
    /// Requests of this MCP session in progress; an active session never expires.
    active: usize,
}

/// The MCP sessions of one server.
pub struct McpServeSessions {
    table: Mutex<HashMap<String, Slot>>,
    clock: Clock,
    shared: Arc<Shared>,
}

impl Default for McpServeSessions {
    fn default() -> Self {
        Self::with_clock(Arc::new(Instant::now))
    }
}

impl McpServeSessions {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_clock(clock: Clock) -> Self {
        Self {
            table: Mutex::default(),
            clock,
            shared: Arc::default(),
        }
    }

    /// MCP sessions that have not expired.
    pub fn count(&self) -> usize {
        let now = (self.clock)();
        self.lock().values().filter(|slot| live(slot, now)).count()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Slot>> {
        self.table.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Mint an MCP session for an `initialize` request from the client labelled `label`.
    pub(super) fn open(&self, state: &AppState, label: String) -> String {
        let fields = serde_json::json!({ "mcp_client": label });
        let connection = Connection {
            client: EmbeddedClient::new(super::super::router(state.clone())),
            state: state.clone(),
            label,
            created: AtomicU32::new(0),
            waits: AtomicU32::new(0),
            calls: Mutex::default(),
            closed: CancellationToken::new(),
            shared: Arc::clone(&self.shared),
        };
        let id = cyber_core::ids::new_id("mcp");
        let now = (self.clock)();
        let mut table = self.lock();
        table.retain(|_, slot| {
            let keep = live(slot, now);
            if !keep {
                slot.connection.closed.cancel();
            }
            keep
        });
        table.insert(
            id.clone(),
            Slot {
                connection: Arc::new(connection),
                last_seen: now,
                active: 0,
            },
        );
        cyber_core::log::info(
            "mcp_serve",
            "MCP session opened",
            serde_json::json!({ "mcp_session": id, "mcp_client": fields["mcp_client"] }),
        );
        id
    }

    /// Start a request of a live MCP session; `None` when it is unknown or expired.
    pub(super) fn enter(self: &Arc<Self>, id: &str) -> Option<Active> {
        let now = (self.clock)();
        let mut table = self.lock();
        if !table.get(id).is_some_and(|slot| live(slot, now)) {
            if let Some(slot) = table.remove(id) {
                slot.connection.closed.cancel();
            }
            return None;
        }
        let slot = table.get_mut(id)?;
        slot.active += 1;
        slot.last_seen = now;
        Some(Active {
            sessions: Arc::clone(self),
            id: id.to_owned(),
            connection: Arc::clone(&slot.connection),
        })
    }

    /// End an MCP session; false when it was unknown or had expired.
    pub(super) fn close(&self, id: &str) -> bool {
        let now = (self.clock)();
        let removed = self.lock().remove(id);
        if let Some(slot) = &removed {
            slot.connection.closed.cancel();
        }
        let closed = removed.is_some_and(|slot| live(&slot, now));
        if closed {
            cyber_core::log::info(
                "mcp_serve",
                "MCP session closed",
                serde_json::json!({ "mcp_session": id }),
            );
        }
        closed
    }
}

fn live(slot: &Slot, now: Instant) -> bool {
    slot.active > 0 || now.saturating_duration_since(slot.last_seen) < IDLE
}

/// A request in progress; dropping it records the MCP session's last activity.
pub(super) struct Active {
    sessions: Arc<McpServeSessions>,
    id: String,
    pub connection: Arc<Connection>,
}

impl Drop for Active {
    fn drop(&mut self) {
        let now = (self.sessions.clock)();
        if let Some(slot) = self.sessions.lock().get_mut(&self.id) {
            slot.active = slot.active.saturating_sub(1);
            slot.last_seen = now;
        }
    }
}

/// The `Mcp-Session-Id` of a request after `initialize`.
pub(super) fn session_id(headers: &HeaderMap) -> Result<&str, ApiError> {
    let Some(value) = headers.get("mcp-session-id") else {
        return Err(
            ApiError::invalid("Mcp-Session-Id is required after initialize")
                .with_reason("mcp_session_required"),
        );
    };
    value.to_str().map_err(|_| unknown())
}

/// `404` for an MCP session that is unknown, ended or expired: initialize again.
pub(super) fn unknown() -> ApiError {
    ApiError::not_found(
        "NotFoundError",
        "unknown or expired MCP session; initialize again",
    )
    .with_reason("mcp_session")
}
