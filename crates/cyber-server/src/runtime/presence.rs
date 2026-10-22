//! Client presence (`goals` → Goal mode safety): which live event streams are open, so
//! the runtime can tell whether anyone could answer a Session's request. An instance
//! stream counts for its Location (or every Location with `scope=all`); a Session stream
//! counts for its Session. When the last stream of a scope closes, its time is kept.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Instant;

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
}

/// Open streams by scope.
#[derive(Debug, Default, Clone)]
pub struct Presence(Arc<Mutex<Counts>>);

/// Counts one open stream until dropped.
#[derive(Debug)]
pub struct PresenceGuard {
    presence: Presence,
    scope: Watching,
}

impl Presence {
    /// Count a stream open until the guard is dropped.
    pub fn attach(&self, scope: Watching) -> PresenceGuard {
        *self.lock().open.entry(scope.clone()).or_default() += 1;
        PresenceGuard {
            presence: self.clone(),
            scope,
        }
    }

    /// The last time a client watched any of `scopes` (or [`Watching::All`]): now while
    /// one is open, `None` when none ever was.
    pub fn last_seen(&self, scopes: &[Watching]) -> Option<Instant> {
        let counts = self.lock();
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
