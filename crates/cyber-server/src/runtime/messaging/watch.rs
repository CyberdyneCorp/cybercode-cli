//! Idle notifications (`cross-session-messaging` → Idle notifications; design section 10).
//!
//! A watch subscribes a Session to exactly one notice about a target: when the target's
//! Drain ends with nothing promotable, when it is archived or deleted (`exited`), or at
//! once when it is already idle. Subscribing writes only the subscriber's
//! `messaging.watch.created.1`; the target never runs a Turn for it. A notice is admitted
//! to the subscriber as `msgx_watch_<watch>` together with `messaging.watch.fired.1` in
//! one commit, so a crash cannot leave a notice without its record or the reverse; a
//! watch whose fire a crash (or a busy subscriber) skipped is fired by the sweep, which
//! runs at startup and every 60 s and also expires watches older than 12 hours with an
//! expiry notice. Every check and fire on a target holds that target's send lock, so a
//! watch created as the target goes idle is fired exactly once.

use std::sync::PoisonError;
use std::time::Duration;

use cyber_store::NewEvent;
use serde::Serialize;
use sha2::{Digest, Sha256};

use super::Messaging;
use super::address::Target;
use super::inbound::Verdict;
use super::records::{
    self, WATCH_CREATED, WATCH_EXPIRED, WATCH_FIRED, WatchCreated, WatchExpired, WatchFired,
    WatchRow,
};
use super::render;
use super::send::{BUSY_RETRY, Sender, SentTo, retry_busy};
use crate::runtime::events::event;
use crate::runtime::model::{AssistantEntry, Entry, InputStatus, SessionState};
use crate::runtime::{Admission, Delivery, Runtime, RuntimeError};

/// How long a subscription waits for its target before it expires.
pub const WATCH_TTL_MS: i64 = 12 * 60 * 60 * 1000;
/// Active subscriptions one Session may hold (`RateLimitedError { reason: "watch_quota" }`).
pub const MAX_ACTIVE_WATCHES: usize = 30;
/// How often the sweep expires subscriptions and fires missed notices.
pub const SWEEP_EVERY: Duration = Duration::from_secs(60);
/// The longest status line a notice carries, in characters.
pub const STATUS_LIMIT: usize = 200;

const EXITED: &str = "exited";
const INTERRUPTED: &str = "interrupted";
/// The status of a run that ended without any text.
const IDLE: &str = "idle";
const WATCH_QUOTA: &str = "watch_quota";

/// The state of a subscription.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
#[schemars(rename = "WatchStatus")]
pub enum WatchStatus {
    /// Waiting for the target to go idle.
    Active,
    /// The notice was admitted to the subscriber.
    Fired,
    /// It ended without an idle notice (12 hours passed, or the chain limit).
    Expired,
}

/// An idle-notification subscription (`v1.messaging.watch`, `watch_session`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, schemars::JsonSchema)]
pub struct Watch {
    pub watch_id: String,
    pub target: SentTo,
    pub status: WatchStatus,
    /// When an unfired subscription expires (RFC 3339).
    pub expires_at: String,
    /// The notice admitted to the subscriber, once fired or expired with one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notice_id: Option<String>,
    /// The ID was used before; this is that subscription and nothing was written.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub replayed: bool,
}

/// One subscription to create.
#[derive(Debug, Clone)]
pub struct WatchRequest {
    /// A `wch_` ID; a repeated ID returns the existing subscription.
    pub id: Option<String>,
    /// The subscribing Session.
    pub subscriber: String,
    /// The target's address, resolved from the subscriber.
    pub target: String,
    /// The Session a permission check approved for `target`, as for a send.
    pub expected_target: Option<String>,
    /// The address notices name the target by, when `target` is not the one the
    /// subscriber used (a send's `notify_when_idle` resolves the target by its ID).
    pub address: Option<String>,
}

/// What a sweep did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Swept {
    /// Subscriptions past their 12 hours, ended with the expiry notice.
    pub expired: usize,
    /// Notices admitted for targets found idle.
    pub fired: usize,
}

/// The watch ID a send with `notify_when_idle` creates: its message ID's, so every retry
/// of the send finds the same subscription.
pub fn watch_id_for(message_id: &str) -> String {
    format!(
        "wch_{}",
        message_id.strip_prefix("msgx_").unwrap_or(message_id)
    )
}

/// The watch ID of a retried request (the `watch_session` call): `wch_` and the first 20
/// hex characters of the SHA-256 of the principal and the key.
pub fn idempotent_watch_id(principal: &str, key: &str) -> String {
    let digest = Sha256::digest(format!("{principal}\0{key}").as_bytes());
    format!("wch_{}", &format!("{digest:x}")[..20])
}

/// The inbox message ID of a subscription's notice: one per watch, idle or expiry.
pub fn notice_id(watch_id: &str) -> String {
    format!("msgx_watch_{watch_id}")
}

/// Whether `id` is a caller-supplied watch ID: `wch_` and 1–120 ASCII letters, digits,
/// `_` or `-`.
pub fn valid_watch_id(id: &str) -> bool {
    id.strip_prefix("wch_").is_some_and(|rest| {
        (1..=120).contains(&rest.len())
            && rest
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    })
}

/// The first non-empty line of `text`, at most [`STATUS_LIMIT`] characters (cut with `…`).
pub fn status_text(text: &str) -> String {
    let line = text
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or_default();
    if line.chars().count() <= STATUS_LIMIT {
        return line.to_owned();
    }
    let mut cut: String = line.chars().take(STATUS_LIMIT - 1).collect();
    cut.push('…');
    cut
}

/// The status line of the target's last run (the assistant steps since its newest input)
/// and when it finished: `interrupted`, `failed: <error>`, else the first line of its last
/// text, else `idle`.
pub(crate) fn last_run(state: &SessionState) -> (String, Option<i64>) {
    let run: Vec<&AssistantEntry> = state
        .entries
        .iter()
        .rev()
        .take_while(|entry| !matches!(entry, Entry::User { .. }))
        .filter_map(|entry| match entry {
            Entry::Assistant(step) => Some(step),
            _ => None,
        })
        .collect();
    let Some(last) = run.first() else {
        return (IDLE.into(), None);
    };
    let interrupted =
        last.error_kind.as_deref() == Some(INTERRUPTED) || (!last.finished && last.error.is_none());
    let status = match &last.error {
        _ if interrupted => INTERRUPTED.into(),
        Some(error) => status_text(&format!("failed: {error}")),
        None => run
            .iter()
            .map(|step| status_text(&step.text))
            .find(|line| !line.is_empty())
            .unwrap_or_else(|| IDLE.into()),
    };
    (status, last.ended_ms)
}

/// Whether the inbox holds input a Drain would still promote (`held` rows do not count).
fn promotable(state: &SessionState) -> bool {
    state
        .inbox
        .iter()
        .any(|row| row.status == InputStatus::Pending && row.delivery != Delivery::Hold)
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

fn rfc3339(ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(ms)
        .unwrap_or_default()
        .to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

/// A target that is idle for its watches.
#[derive(Debug, Clone)]
struct Idle {
    status: String,
    finished_at: String,
    /// The chain hops of the run that ended (0 when none did).
    hops: u32,
}

impl Idle {
    fn exited() -> Self {
        Self {
            status: EXITED.into(),
            finished_at: rfc3339(now_ms()),
            hops: 0,
        }
    }
}

fn warn(what: &str, error: &RuntimeError) {
    cyber_core::log::warn(
        "messaging",
        what,
        serde_json::json!({ "error": error.to_string() }),
    );
}

fn watch_of(row: &WatchRow, replayed: bool) -> Watch {
    let status = match row.status.as_str() {
        "active" => WatchStatus::Active,
        "fired" => WatchStatus::Fired,
        _ => WatchStatus::Expired,
    };
    Watch {
        watch_id: row.id.clone(),
        target: SentTo {
            address: row.target_address.clone(),
            session_id: Some(row.target_id.clone()),
        },
        status,
        expires_at: rfc3339(row.expires_ms),
        notice_id: row.notice_id.clone(),
        replayed,
    }
}

/// Read queries on `messaging_watch`.
mod query {
    use rusqlite::{Connection, OptionalExtension, params};

    use super::records::{WATCH_COLUMNS, WatchRow, watch_row};

    pub fn by_id(db: &Connection, id: &str) -> rusqlite::Result<Option<WatchRow>> {
        db.query_row(
            &format!("SELECT {WATCH_COLUMNS} FROM messaging_watch WHERE id = ?1"),
            [id],
            watch_row,
        )
        .optional()
    }

    pub fn active_on(db: &Connection, target: &str) -> rusqlite::Result<Vec<WatchRow>> {
        let mut stmt = db.prepare(&format!(
            "SELECT {WATCH_COLUMNS} FROM messaging_watch
             WHERE target_id = ?1 AND status = 'active' AND target_peer IS NULL
             ORDER BY created_ms, id"
        ))?;
        stmt.query_map([target], watch_row)?.collect()
    }

    pub fn active_count(db: &Connection, subscriber: &str) -> rusqlite::Result<usize> {
        db.query_row(
            "SELECT COUNT(*) FROM messaging_watch WHERE subscriber_id = ?1 AND status = 'active'",
            [subscriber],
            |r| r.get::<_, i64>(0),
        )
        .map(|count| usize::try_from(count).unwrap_or(usize::MAX))
    }

    /// Active watches whose `expires_ms` is at or before `now_ms`, oldest first.
    pub fn due(db: &Connection, now_ms: i64) -> rusqlite::Result<Vec<WatchRow>> {
        let mut stmt = db.prepare(&format!(
            "SELECT {WATCH_COLUMNS} FROM messaging_watch
             WHERE status = 'active' AND expires_ms <= ?1 ORDER BY expires_ms, id"
        ))?;
        stmt.query_map(params![now_ms], watch_row)?.collect()
    }

    /// The targets that have an active watch.
    pub fn watched_targets(db: &Connection) -> rusqlite::Result<Vec<String>> {
        let mut stmt = db.prepare(
            "SELECT DISTINCT target_id FROM messaging_watch
             WHERE status = 'active' AND target_peer IS NULL ORDER BY target_id",
        )?;
        stmt.query_map([], |r| r.get(0))?.collect()
    }
}

impl Messaging {
    /// Subscribe `subscriber` to one notice when `target` next goes idle or exits
    /// (`watch_session`, `POST /messaging/watch`, `notify_when_idle`). Nothing happens in
    /// the target; an idle target fires at once.
    pub async fn watch(&self, request: WatchRequest) -> Result<Watch, RuntimeError> {
        let subscriber = request.subscriber.clone();
        self.sender_enabled(&Sender::Session(subscriber.clone()))?;
        if let Some(id) = request.id.as_deref().filter(|id| !valid_watch_id(id)) {
            return Err(RuntimeError::Invalid(format!(
                "watch ID {id:?} is not wch_ followed by 1-120 letters, digits, _ or -"
            )));
        }
        let target = self.resolve(Some(&subscriber), &request.target)?;
        if let Some(peer) = target.peer.clone() {
            return self.watch_remote(request, target, &peer).await;
        }
        let target_id = target.candidate.session_id.clone();
        if let Some(expected) = request
            .expected_target
            .as_deref()
            .filter(|expected| *expected != target_id)
        {
            return Err(RuntimeError::Conflict(format!(
                "`{}` now names {target_id} instead of the approved {expected}",
                request.target
            )));
        }
        self.check_watchable(&subscriber, &target)?;
        let id = request.id.unwrap_or_else(|| cyber_core::ids::new_id("wch"));
        let lock = self.runtime.message_lock(&target_id);
        let _watching = lock.lock().await;
        if let Some(row) = self.runtime.watch_by_id(&id)? {
            if row.subscriber_id != subscriber || row.target_id != target_id {
                return Err(RuntimeError::Conflict(format!(
                    "watch {id} already exists for another subscriber or target"
                )));
            }
            return Ok(watch_of(&row, true));
        }
        self.runtime.check_watch_quota(&subscriber, None)?;
        let created = WatchCreated {
            watch_id: id.clone(),
            target: request.address.unwrap_or(target.address),
            target_id: target_id.clone(),
            expires_ms: now_ms().saturating_add(WATCH_TTL_MS),
            target_peer: None,
        };
        self.runtime
            .record_message(&subscriber, event(WATCH_CREATED, &created))
            .await?;
        if let Some(idle) = self.runtime.idle_now(&target_id, 0, None).await? {
            self.runtime.fire_locked(&target_id, &idle).await;
        }
        let row = self
            .runtime
            .watch_by_id(&id)?
            .ok_or_else(|| RuntimeError::Corrupt(format!("watch {id} was not projected")))?;
        Ok(watch_of(&row, false))
    }

    /// A notice carries the target's last output line, so a target that would refuse the
    /// subscriber's messages (its Location disables messaging, or its inbound rules or
    /// Session override refuse this sender) cannot be watched either. An archived target
    /// can: its notice is `exited`.
    fn check_watchable(&self, subscriber: &str, target: &Target) -> Result<(), RuntimeError> {
        if target.archived {
            return Ok(());
        }
        let addresses = self.sender_addresses(&Sender::Session(subscriber.to_owned()))?;
        let settings = self.settings(Some(&target.candidate.location.directory));
        match super::inbound_verdict(
            target,
            settings.as_ref(),
            &addresses,
            super::inbound::Reach::SameMachine,
        ) {
            Verdict::Refuse(reason) => Err(RuntimeError::Forbidden {
                reason,
                message: format!(
                    "`{}` refuses messages from this Session ({reason}), so it cannot be watched",
                    target.address
                ),
            }),
            Verdict::Deliver | Verdict::Hold => Ok(()),
        }
    }

    /// Expire every active watch whose 12 hours ended by `now_ms` (with the expiry notice),
    /// then fire the watches of targets found idle: the notices a crash or a busy
    /// subscriber left unsent. Runs at startup and every [`SWEEP_EVERY`]; `now_ms` is
    /// injected so tests can move the clock.
    pub async fn sweep_watches(&self, now_ms: i64) -> Result<Swept, RuntimeError> {
        let runtime = &self.runtime;
        let mut swept = Swept::default();
        let due = runtime
            .inner
            .store
            .read(move |db| Ok(query::due(db, now_ms)?))?;
        for watch in due {
            let lock = runtime.message_lock(&watch.target_id);
            let _watching = lock.lock().await;
            // It may have fired while the lock was taken.
            if !runtime.still_active(&watch.id)? {
                continue;
            }
            match runtime.expire(&watch).await {
                Ok(()) => swept.expired += 1,
                Err(error) => warn("watch expiry failed", &error),
            }
        }
        let targets = runtime
            .inner
            .store
            .read(|db| Ok(query::watched_targets(db)?))?;
        for target in targets {
            let lock = runtime.message_lock(&target);
            let _watching = lock.lock().await;
            if let Some(idle) = runtime.idle_now(&target, 0, None).await? {
                swept.fired += runtime.fire_locked(&target, &idle).await;
            }
        }
        Ok(swept)
    }
}

impl Runtime {
    fn watch_by_id(&self, id: &str) -> Result<Option<WatchRow>, RuntimeError> {
        let id = id.to_owned();
        Ok(self
            .inner
            .store
            .read(move |db| Ok(query::by_id(db, &id)?))?)
    }

    fn still_active(&self, id: &str) -> Result<bool, RuntimeError> {
        Ok(self
            .watch_by_id(id)?
            .is_some_and(|row| row.status == "active"))
    }

    /// The 31st active watch of a subscriber is refused; `existing` is a watch ID a retry
    /// would reuse, which needs no new slot.
    pub(crate) fn check_watch_quota(
        &self,
        subscriber: &str,
        existing: Option<&str>,
    ) -> Result<(), RuntimeError> {
        if let Some(id) = existing
            && self.watch_by_id(id)?.is_some()
        {
            return Ok(());
        }
        let probe = subscriber.to_owned();
        let active = self
            .inner
            .store
            .read(move |db| Ok(query::active_count(db, &probe)?))?;
        if active < MAX_ACTIVE_WATCHES {
            return Ok(());
        }
        Err(RuntimeError::RateLimited {
            reason: WATCH_QUOTA,
            message: format!(
                "{subscriber} already has {MAX_ACTIVE_WATCHES} active idle notification subscriptions"
            ),
        })
    }

    /// The target as its watches see it: exited when deleted or archived, idle when no
    /// Drain runs and nothing promotable waits, else `None`. `status` overrides the status
    /// line of the last run (a failed or interrupted Drain).
    async fn idle_now(
        &self,
        target: &str,
        hops: u32,
        status: Option<String>,
    ) -> Result<Option<Idle>, RuntimeError> {
        let handle = match self.inner.handle(target).await {
            Ok(handle) => handle,
            Err(RuntimeError::SessionNotFound(_)) => return Ok(Some(Idle::exited())),
            Err(error) => return Err(error),
        };
        if self.is_running(target) {
            return Ok(None);
        }
        let state = handle.state.lock().await;
        if state.info.archived {
            return Ok(Some(Idle::exited()));
        }
        if promotable(&state) {
            return Ok(None);
        }
        let (line, ended) = last_run(&state);
        Ok(Some(Idle {
            status: status.unwrap_or(line),
            finished_at: rfc3339(ended.unwrap_or_else(now_ms)),
            hops,
        }))
    }

    /// Fire every active watch on `target`, whose send lock the caller holds. A watch that
    /// fails stays active for the sweep; the others still fire. Returns the notices sent.
    async fn fire_locked(&self, target: &str, idle: &Idle) -> usize {
        let probe = target.to_owned();
        let watches = match self
            .inner
            .store
            .read(move |db| Ok(query::active_on(db, &probe)?))
        {
            Ok(watches) => watches,
            Err(error) => {
                warn("reading watches failed", &RuntimeError::from(error));
                return 0;
            }
        };
        let mut fired = 0;
        for watch in watches {
            match self.fire(&watch, idle).await {
                Ok(true) => fired += 1,
                Ok(false) => {}
                Err(error) => warn("idle notice failed", &error),
            }
        }
        fired
    }

    /// The `messaging.max_hops` of the subscriber's Location; `None` when messaging is
    /// disabled there or its settings cannot be read.
    fn subscriber_max_hops(&self, subscriber: &str) -> Option<u32> {
        let messaging = self.messaging();
        let directory = messaging.session_directory(subscriber).ok()?;
        messaging
            .settings(Some(&directory))
            .filter(|settings| settings.enabled)
            .map(|settings| settings.max_hops)
    }

    /// Admit the idle notice with `messaging.watch.fired.1`, or, past the chain limit or
    /// where messaging is disabled, expire the watch without one. Returns whether a
    /// notice was sent.
    async fn fire(&self, watch: &WatchRow, idle: &Idle) -> Result<bool, RuntimeError> {
        let hops = idle.hops.saturating_add(1);
        let reason = match self.subscriber_max_hops(&watch.subscriber_id) {
            None => Some("disabled"),
            Some(max) if hops > max => Some("chain_limit"),
            Some(_) => None,
        };
        if let Some(reason) = reason {
            let expired = WatchExpired {
                watch_id: watch.id.clone(),
                reason: Some(reason.into()),
                notice_id: None,
            };
            self.record_message(&watch.subscriber_id, event(WATCH_EXPIRED, &expired))
                .await?;
            return Ok(false);
        }
        let notice = notice_id(&watch.id);
        let fired = WatchFired {
            watch_id: watch.id.clone(),
            notice_id: notice.clone(),
            status: idle.status.clone(),
            finished_at: idle.finished_at.clone(),
            hops,
        };
        let text = render::idle_notice(&watch.target_address, &idle.finished_at, &idle.status);
        self.admit_notice(watch, notice, text, event(WATCH_FIRED, &fired))
            .await?;
        Ok(true)
    }

    /// End a watch past its 12 hours: the expiry notice with `messaging.watch.expired.1`,
    /// or the record alone where messaging is disabled for the subscriber.
    async fn expire(&self, watch: &WatchRow) -> Result<(), RuntimeError> {
        let notice = notice_id(&watch.id);
        let enabled = self.subscriber_max_hops(&watch.subscriber_id).is_some();
        let expired = WatchExpired {
            watch_id: watch.id.clone(),
            reason: Some(if enabled { "expired" } else { "disabled" }.into()),
            notice_id: enabled.then(|| notice.clone()),
        };
        if !enabled {
            return self
                .record_message(&watch.subscriber_id, event(WATCH_EXPIRED, &expired))
                .await;
        }
        let text = render::expiry_notice(&watch.target_address, &watch.id);
        self.admit_notice(watch, notice, text, event(WATCH_EXPIRED, &expired))
            .await
    }

    /// Admit a notice to the subscriber with its watch record in one commit. It wakes an
    /// idle subscriber; the subscriber's inbound rules and rate limits do not apply (it
    /// asked for the notice). A settling child subscriber is retried once.
    async fn admit_notice(
        &self,
        watch: &WatchRow,
        notice: String,
        text: String,
        record: NewEvent,
    ) -> Result<(), RuntimeError> {
        let admission = Admission {
            message_id: Some(notice),
            source: "session".into(),
            resume: true,
            ..Admission::text(text, Delivery::Steer)
        };
        let admitted = retry_busy(BUSY_RETRY, || {
            self.admit_message(
                &watch.subscriber_id,
                admission.clone(),
                vec![record.clone()],
            )
        })
        .await?;
        admitted
            .map(drop)
            .ok_or_else(|| RuntimeError::Busy(watch.subscriber_id.clone()))
    }

    /// Whether any watch on `target` is active (checked before taking its send lock).
    fn watched(&self, target: &str) -> Result<bool, RuntimeError> {
        let probe = target.to_owned();
        Ok(self.inner.store.read(move |db| {
            Ok(db
                .prepare(
                    "SELECT 1 FROM messaging_watch WHERE target_id = ?1 AND status = 'active'",
                )?
                .exists([probe])?)
        })?)
    }

    /// The target's Drain ended (`drain::run`): fire its watches if it is idle now, with
    /// the ended run's `hops` and, for a failed or interrupted Drain, its `status`. A
    /// Drain stopped by shutdown fires nothing; the startup sweep does.
    pub(crate) async fn drain_ended(&self, target: &str, hops: u32, status: Option<String>) {
        if self.is_shutting_down() {
            return;
        }
        match self.watched(target) {
            Ok(true) => {}
            Ok(false) => return,
            Err(error) => return warn("reading watches failed", &error),
        }
        let lock = self.message_lock(target);
        let _watching = lock.lock().await;
        match self.idle_now(target, hops, status).await {
            Ok(Some(idle)) => {
                self.fire_locked(target, &idle).await;
            }
            Ok(None) => {}
            Err(error) => warn("idle check failed", &error),
        }
    }

    /// The target was archived: its watches fire with `exited`.
    pub(crate) async fn target_archived(&self, target: &str) {
        if !self.watched(target).unwrap_or(true) {
            return;
        }
        let lock = self.message_lock(target);
        let _watching = lock.lock().await;
        self.fire_locked(target, &Idle::exited()).await;
    }

    /// The Sessions `ids` were deleted: watches of surviving subscribers on them fire
    /// with `exited`. The caller holds their send locks.
    pub(crate) async fn targets_deleted(&self, ids: &[String]) {
        for id in ids {
            self.fire_locked(id, &Idle::exited()).await;
        }
    }

    /// Run the sweep once now; must run during application construction, after
    /// `recover_messages`. Fires the notices a crash left unsent.
    pub async fn recover_watches(&self) -> Result<Swept, RuntimeError> {
        self.messaging().sweep_watches(now_ms()).await
    }

    /// Run the sweep every [`SWEEP_EVERY`] until shutdown.
    pub fn spawn_watch_sweep(&self) {
        let weak = self.downgrade();
        let closed = self.inner.closed.clone();
        let task = tokio::spawn(async move {
            loop {
                tokio::select! {
                    () = closed.cancelled() => return,
                    () = tokio::time::sleep(SWEEP_EVERY) => {}
                }
                let Some(runtime) = weak.upgrade() else {
                    return;
                };
                if let Err(error) = runtime.messaging().sweep_watches(now_ms()).await {
                    warn("watch sweep failed", &error);
                }
            }
        });
        let mut owners = self
            .inner
            .background
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        owners.retain(|owner| !owner.is_finished());
        owners.push(task);
    }
}

#[path = "watch_remote.rs"]
mod remote;

pub use remote::REMOTE_POLL_EVERY;

#[cfg(test)]
#[path = "watch_tests.rs"]
mod tests;
