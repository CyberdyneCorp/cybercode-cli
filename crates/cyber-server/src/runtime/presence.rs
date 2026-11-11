//! Client presence (`goals` → Goal mode safety): which live event streams are open, so
//! the runtime can tell whether anyone could answer a Session's request. An instance
//! stream counts for its Location (or every Location with `scope=all`); a Session stream
//! counts for its Session. When the last stream of a scope closes, its time is kept.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};
use tokio::sync::Notify;

/// What an open stream watches.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Watching {
    /// Every Location (`/event?scope=all`).
    All,
    /// One Location's instance stream, by directory.
    Location(String),
    /// One Session's stream.
    Session(String),
}

#[derive(Debug, Default)]
struct Counts {
    open: HashMap<Watching, usize>,
    /// When the last stream of a scope closed.
    closed: HashMap<Watching, Instant>,
    leases: HashMap<String, Lease>,
    managed: HashMap<String, String>,
    watching_leases: bool,
    native_epochs: HashMap<String, u64>,
}

/// Open streams by scope.
#[derive(Debug, Default, Clone)]
pub struct Presence(Arc<Mutex<Counts>>, Arc<Notify>);

pub const INTERACTIVE_TTL: Duration = Duration::from_secs(45);
const MAX_LEASES: usize = 128;
const MAX_SESSION_LEASES: usize = 16;
const MAX_MANAGED_SESSIONS: usize = 8192;
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct InteractiveClient {
    pub client_id: String,
    pub session_id: String,
    /// Expiration in milliseconds since the Unix epoch. Authority uses a monotonic clock.
    pub expires_at: i64,
}
#[derive(Debug)]
struct Lease {
    receipt: InteractiveClient,
    directory: String,
    deadline: tokio::time::Instant,
    kind: ClientKind,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ClientKind {
    Session,
    Input,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InteractiveClientError {
    Missing,
    Limit,
    Unavailable,
}
/// Exact authority scope; Session IDs alone cannot carry presence across Locations.
#[derive(Debug, Clone)]
pub(crate) struct InteractiveScope {
    pub session_id: String,
    pub directory: String,
}

/// Counts one open stream until dropped.
#[derive(Debug)]
pub struct PresenceGuard {
    presence: Presence,
    scope: Watching,
}

impl Presence {
    /// Count a stream open until the guard is dropped.
    pub fn attach(&self, scope: Watching) -> PresenceGuard {
        let mut counts = self.lock();
        expire_locked(&mut counts);
        *counts.open.entry(scope.clone()).or_default() += 1;
        drop(counts);
        self.1.notify_one();
        PresenceGuard {
            presence: self.clone(),
            scope,
        }
    }

    /// The last time a client watched any of `scopes` (or [`Watching::All`]): now while
    /// one is open, `None` when none ever was.
    pub fn last_seen(&self, scopes: &[Watching]) -> Option<Instant> {
        let counts = self.lock();
        if counts.leases.values().any(|lease| {
            lease.kind == ClientKind::Session
                && lease.deadline > tokio::time::Instant::now()
                && scopes.iter().any(
                    |scope| matches!(scope,Watching::Session(id) if id==&lease.receipt.session_id),
                )
        }) {
            return Some(Instant::now());
        }
        let mut scopes = scopes.iter().chain(std::iter::once(&Watching::All));
        if scopes
            .clone()
            .any(|scope| counts.open.get(scope).is_some_and(|n| *n > 0))
        {
            return Some(Instant::now());
        }
        scopes
            .by_ref()
            .filter_map(|scope| counts.closed.get(scope).copied())
            .max()
    }

    pub(crate) fn create(
        &self,
        session: &str,
        directory: &str,
    ) -> Result<InteractiveClient, InteractiveClientError> {
        self.create_kind(session, directory, ClientKind::Session)
    }
    pub(crate) fn create_kind(
        &self,
        session: &str,
        directory: &str,
        kind: ClientKind,
    ) -> Result<InteractiveClient, InteractiveClientError> {
        let mut counts = self.lock();
        expire_locked(&mut counts);
        if counts.leases.len() >= MAX_LEASES
            || counts
                .leases
                .values()
                .filter(|l| l.receipt.session_id == session)
                .count()
                >= MAX_SESSION_LEASES
            || kind == ClientKind::Session
                && counts.managed.len() >= MAX_MANAGED_SESSIONS
                && !counts.managed.contains_key(session)
        {
            return Err(InteractiveClientError::Limit);
        }
        let receipt = receipt(cyber_core::ids::new_id("icl"), session);
        if kind == ClientKind::Session {
            counts
                .managed
                .insert(session.to_owned(), directory.to_owned());
        }
        counts.leases.insert(
            receipt.client_id.clone(),
            Lease {
                receipt: receipt.clone(),
                directory: directory.to_owned(),
                deadline: tokio::time::Instant::now() + INTERACTIVE_TTL,
                kind,
            },
        );
        drop(counts);
        self.1.notify_one();
        Ok(receipt)
    }
    pub(crate) fn renew(
        &self,
        session: &str,
        directory: &str,
        id: &str,
    ) -> Result<InteractiveClient, InteractiveClientError> {
        self.renew_kind(session, directory, id, ClientKind::Session)
    }
    pub(crate) fn renew_kind(
        &self,
        session: &str,
        directory: &str,
        id: &str,
        kind: ClientKind,
    ) -> Result<InteractiveClient, InteractiveClientError> {
        let mut counts = self.lock();
        expire_locked(&mut counts);
        let lease = counts
            .leases
            .get_mut(id)
            .filter(|l| {
                l.receipt.session_id == session && l.directory == directory && l.kind == kind
            })
            .ok_or(InteractiveClientError::Missing)?;
        lease.receipt = receipt(id.to_owned(), session);
        lease.deadline = tokio::time::Instant::now() + INTERACTIVE_TTL;
        let result = lease.receipt.clone();
        drop(counts);
        self.1.notify_one();
        Ok(result)
    }
    pub(crate) fn release(
        &self,
        session: &str,
        directory: &str,
        id: &str,
    ) -> Result<(), InteractiveClientError> {
        self.release_kind(session, directory, id, ClientKind::Session)
    }
    pub(crate) fn release_kind(
        &self,
        session: &str,
        directory: &str,
        id: &str,
        kind: ClientKind,
    ) -> Result<(), InteractiveClientError> {
        let mut counts = self.lock();
        expire_locked(&mut counts);
        if counts.leases.get(id).is_some_and(|l| {
            l.receipt.session_id != session || l.directory != directory || l.kind != kind
        }) {
            return Err(InteractiveClientError::Missing);
        }
        counts.leases.remove(id);
        if kind == ClientKind::Session {
            native_loss(&mut counts, session);
        }
        drop(counts);
        self.1.notify_one();
        Ok(())
    }
    pub(crate) fn managed(&self, session: &str) -> bool {
        self.lock().managed.contains_key(session)
    }
    pub(crate) fn manage(
        &self,
        session: &str,
        directory: &str,
    ) -> Result<(), InteractiveClientError> {
        let mut counts = self.lock();
        if counts.managed.len() >= MAX_MANAGED_SESSIONS && !counts.managed.contains_key(session) {
            return Err(InteractiveClientError::Limit);
        }
        counts
            .managed
            .insert(session.to_owned(), directory.to_owned());
        Ok(())
    }
    pub(crate) fn unmanage(&self, session: &str) {
        self.lock().managed.remove(session);
    }
    pub(crate) fn start_watch(&self) -> bool {
        let mut counts = self.lock();
        if counts.watching_leases {
            return false;
        }
        counts.watching_leases = true;
        true
    }
    pub(crate) async fn changed(&self) {
        self.1.notified().await;
    }
    pub(crate) fn next_expiry(&self) -> Option<tokio::time::Instant> {
        self.lock().leases.values().map(|l| l.deadline).min()
    }
    pub(crate) fn expire(&self) {
        expire_locked(&mut self.lock());
    }
    pub(crate) fn with_attendance<T>(
        &self,
        scopes: &[InteractiveScope],
        legacy: bool,
        apply: impl FnOnce(bool) -> T,
    ) -> T {
        let counts = self.lock();
        apply(attended(&counts, scopes, legacy))
    }
    pub(super) fn with_input_admission<T>(
        &self,
        session: &str,
        directory: &str,
        client: Option<&str>,
        apply: impl FnOnce(bool) -> T,
    ) -> T {
        let counts = self.lock();
        let live = client.is_none_or(|id| input_live(&counts, session, directory, id));
        apply(live)
    }
    pub(super) fn with_request_attendance<T>(
        &self,
        scopes: &[InteractiveScope],
        legacy: bool,
        authority: &super::input_clients::Attendance,
        apply: impl FnOnce(bool) -> T,
    ) -> T {
        if matches!(authority, super::input_clients::Attendance::Legacy) {
            return self.with_attendance(scopes, legacy, apply);
        }
        let counts = self.lock();
        let live = match authority {
            super::input_clients::Attendance::Legacy => attended(&counts, scopes, legacy),
            super::input_clients::Attendance::Unknown => false,
            super::input_clients::Attendance::Bound {
                session,
                directory,
                client,
            } => client.as_deref().is_some_and(|id| {
                scopes.first().is_some_and(|scope| {
                    &scope.session_id == session && &scope.directory == directory
                }) && input_live(&counts, session, directory, id)
            }),
        };
        apply(live)
    }
    /// Preliminary same-Session check for synchronous tool adapters. Opening a real
    /// request additionally checks all authoritative ancestor/team routes.
    pub(crate) fn session_attended(&self, session: &str, legacy: bool) -> bool {
        let counts = self.lock();
        let Some(directory) = counts.managed.get(session) else {
            return legacy;
        };
        attended(
            &counts,
            &[InteractiveScope {
                session_id: session.into(),
                directory: directory.clone(),
            }],
            legacy,
        )
    }
    pub(crate) fn native_epoch(&self, session: &str) -> u64 {
        let mut counts = self.lock();
        expire_locked(&mut counts);
        *counts.native_epochs.entry(session.to_owned()).or_default()
    }
    pub(crate) fn live_session(&self, session: &str) -> bool {
        let counts = self.lock();
        counts
            .open
            .get(&Watching::Session(session.to_owned()))
            .is_some_and(|count| *count > 0)
            || counts.leases.values().any(|lease| {
                lease.kind == ClientKind::Session
                    && lease.receipt.session_id == session
                    && lease.deadline > tokio::time::Instant::now()
            })
    }
    fn lock(&self) -> std::sync::MutexGuard<'_, Counts> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl Drop for PresenceGuard {
    fn drop(&mut self) {
        let mut counts = self.presence.lock();
        let open = counts.open.entry(self.scope.clone()).or_default();
        *open = open.saturating_sub(1);
        if *open == 0 {
            counts.open.remove(&self.scope);
            counts.closed.insert(self.scope.clone(), Instant::now());
        }
        if let Watching::Session(session) = &self.scope {
            native_loss(&mut counts, session);
        }
        drop(counts);
        self.presence.1.notify_one();
    }
}
fn receipt(client_id: String, session: &str) -> InteractiveClient {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(i64::MAX as u128) as i64;
    InteractiveClient {
        client_id,
        session_id: session.to_owned(),
        expires_at: now.saturating_add(INTERACTIVE_TTL.as_millis() as i64),
    }
}
fn expire_locked(counts: &mut Counts) {
    let now = tokio::time::Instant::now();
    let mut expired = HashSet::new();
    counts.leases.retain(|_, lease| {
        if lease.deadline <= now {
            if lease.kind == ClientKind::Session {
                expired.insert(lease.receipt.session_id.clone());
            }
            false
        } else {
            true
        }
    });
    for session in expired {
        native_loss(counts, &session);
        counts
            .closed
            .insert(Watching::Session(session), Instant::now());
    }
}
// Called while holding Counts: losing the last exact Session attachment is
// permanent for existing grants, even if another attachment arrives before polling.
fn native_loss(counts: &mut Counts, session: &str) {
    if counts
        .open
        .get(&Watching::Session(session.to_owned()))
        .is_some_and(|n| *n > 0)
        || counts.leases.values().any(|lease| {
            lease.kind == ClientKind::Session
                && lease.receipt.session_id == session
                && lease.deadline > tokio::time::Instant::now()
        })
    {
        return;
    }
    let epoch = counts.native_epochs.entry(session.to_owned()).or_default();
    *epoch = epoch.wrapping_add(1);
}
fn attended(counts: &Counts, scopes: &[InteractiveScope], legacy: bool) -> bool {
    let managed = scopes
        .iter()
        .any(|s| counts.managed.contains_key(&s.session_id));
    if !managed {
        return legacy;
    }
    if legacy
        && (counts.open.get(&Watching::All).is_some_and(|n| *n > 0)
            || scopes.iter().any(|s| {
                counts
                    .open
                    .get(&Watching::Session(s.session_id.clone()))
                    .is_some_and(|n| *n > 0)
                    || counts
                        .open
                        .get(&Watching::Location(s.directory.clone()))
                        .is_some_and(|n| *n > 0)
            }))
    {
        return true;
    }
    counts.leases.values().any(|l| {
        l.kind == ClientKind::Session
            && l.deadline > tokio::time::Instant::now()
            && scopes
                .iter()
                .any(|s| s.session_id == l.receipt.session_id && s.directory == l.directory)
    })
}

fn input_live(counts: &Counts, session: &str, directory: &str, id: &str) -> bool {
    counts.leases.get(id).is_some_and(|lease| {
        lease.kind == ClientKind::Input
            && lease.receipt.session_id == session
            && lease.directory == directory
            && lease.deadline > tokio::time::Instant::now()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_epoch_records_last_disconnect_before_reconnect_without_polling() {
        let presence = Presence::default();
        let session = "ses_native";
        let first = presence.attach(Watching::Session(session.into()));
        let second = presence.attach(Watching::Session(session.into()));
        let epoch = presence.native_epoch(session);
        drop(first);
        assert_eq!(presence.native_epoch(session), epoch);
        drop(second);
        let _new = presence.attach(Watching::Session(session.into()));
        assert_ne!(presence.native_epoch(session), epoch);
    }
    #[tokio::test(start_paused = true)]
    async fn native_epoch_records_expiry_before_new_lease_can_restore_attendance() {
        let presence = Presence::default();
        let session = "ses_native";
        presence.create(session, "/repo").unwrap();
        let epoch = presence.native_epoch(session);
        tokio::time::advance(INTERACTIVE_TTL + Duration::from_secs(1)).await;
        presence.create(session, "/repo").unwrap();
        assert!(presence.live_session(session));
        assert_ne!(presence.native_epoch(session), epoch);
    }
    #[test]
    fn a_scope_is_present_while_a_stream_is_open_and_remembered_after() {
        let presence = Presence::default();
        let session = Watching::Session("ses_1".into());
        let location = Watching::Location("/repo".into());
        assert_eq!(
            presence.last_seen(&[session.clone(), location.clone()]),
            None
        );
        let guard = presence.attach(location.clone());
        let seen = presence.last_seen(&[session.clone(), location.clone()]);
        assert!(seen.is_some());
        assert_eq!(
            presence.last_seen(&[Watching::Location("/other".into())]),
            None
        );
        drop(guard);
        let after = presence.last_seen(std::slice::from_ref(&location)).unwrap();
        assert!(after >= seen.unwrap());
        let _all = presence.attach(Watching::All);
        assert!(
            presence
                .last_seen(&[Watching::Location("/other".into())])
                .is_some()
        );
    }
}

#[cfg(test)]
mod input_tests {
    use super::*;
    use crate::runtime::input_clients::Attendance;
    fn scope() -> Vec<InteractiveScope> {
        vec![InteractiveScope {
            session_id: "session".into(),
            directory: "/repo".into(),
        }]
    }
    fn owned(client: &str) -> Attendance {
        Attendance::Bound {
            session: "session".into(),
            directory: "/repo".into(),
            client: Some(client.into()),
        }
    }
    #[tokio::test(start_paused = true)]
    async fn input_expiry_never_borrows_other_presence_or_changes_native_authority() {
        let presence = Presence::default();
        presence.manage("session", "/repo").unwrap();
        let a = presence
            .create_kind("session", "/repo", ClientKind::Input)
            .unwrap();
        let b = presence
            .create_kind("session", "/repo", ClientKind::Input)
            .unwrap();
        let epoch = presence.native_epoch("session");
        assert!(!presence.live_session("session"));
        assert_eq!(
            presence.last_seen(&[Watching::Session("session".into())]),
            None
        );
        assert!(!presence.session_attended("session", true));
        assert!(
            presence.with_request_attendance(&scope(), true, &owned(&a.client_id), |live| live)
        );
        assert!(matches!(
            presence.renew("session", "/repo", &a.client_id),
            Err(InteractiveClientError::Missing)
        ));
        assert!(matches!(
            presence.release("session", "/repo", &a.client_id),
            Err(InteractiveClientError::Missing)
        ));
        tokio::time::advance(Duration::from_secs(30)).await;
        presence
            .renew_kind("session", "/repo", &b.client_id, ClientKind::Input)
            .unwrap();
        tokio::time::advance(Duration::from_secs(16)).await;
        presence.expire();
        let _observer = presence.attach(Watching::All);
        let _browser = presence.create("session", "/repo").unwrap();
        assert!(
            !presence.with_request_attendance(&scope(), true, &owned(&a.client_id), |live| live)
        );
        assert!(
            presence.with_request_attendance(&scope(), true, &owned(&b.client_id), |live| live)
        );
        assert!(
            presence
                .renew_kind("session", "/repo", &a.client_id, ClientKind::Input)
                .is_err()
        );
        assert_eq!(presence.native_epoch("session"), epoch);
        assert!(
            presence.with_input_admission("session", "/other", Some(&b.client_id), |live| !live)
        );
    }
    #[tokio::test]
    async fn input_and_browser_leases_share_capacity_without_sharing_authority() {
        let presence = Presence::default();
        for _ in 0..MAX_SESSION_LEASES {
            presence
                .create_kind("session", "/repo", ClientKind::Input)
                .unwrap();
        }
        assert!(matches!(
            presence.create("session", "/repo"),
            Err(InteractiveClientError::Limit)
        ));
        assert!(matches!(
            presence.create_kind("session", "/repo", ClientKind::Input),
            Err(InteractiveClientError::Limit)
        ));
        assert!(!presence.live_session("session"));
    }
}
