//! Watches on peer Sessions (`cross-session-messaging` → Peer watches and listing; design
//! section 6).
//!
//! `watch_session({ target: "X@P" })` asks `P` for `X`'s status once: a Session `P` does
//! not show this machine is not found, and an unreachable peer fails the watch. The
//! subscription is an ordinary `messaging_watch` row with `target_peer` set and
//! `target_id` `<ses_>@P`, so the local idle checks never see it. A poller asks every
//! active peer target's status every 15 s and fires its watches when it is idle (`exited`
//! when the peer no longer shows it), so a watch created while the target runs fires
//! within 15 s of the change. Peers are asked in parallel; after the first target of a
//! peer that does not answer, that peer's other targets wait for the next pass. A peer
//! that does not answer leaves the watch active; the sweep expires it after its 12 hours
//! like any other.

use std::collections::BTreeMap;

use super::*;
use crate::runtime::messaging::address::AddressError;
use crate::runtime::messaging::{PeerSessionStatus, ReachableState};

/// How often the poller asks peers for the status of watched Sessions.
pub const REMOTE_POLL_EVERY: Duration = Duration::from_secs(15);
/// The poller's lock, among the send locks; no address has this form.
const LOCK: &str = "\0remote-watches";

/// The target as its watches see it: idle when the peer says so, with the end of its last
/// run (that machine's clock) as `finished_at`.
fn peer_idle(status: &PeerSessionStatus) -> Option<Idle> {
    (status.state == ReachableState::Idle).then(|| Idle {
        status: IDLE.into(),
        finished_at: rfc3339(status.last_idle_ms.unwrap_or_else(now_ms)),
        hops: 0,
    })
}

/// A retried request for watch `row`: the same subscriber and peer replay it.
fn replayed(row: &WatchRow, subscriber: &str, peer: &str) -> Result<Watch, RuntimeError> {
    if row.subscriber_id != subscriber || row.target_peer.as_deref() != Some(peer) {
        return Err(RuntimeError::Conflict(format!(
            "watch {} already exists for another subscriber or target",
            row.id
        )));
    }
    Ok(watch_of(row, true))
}

/// The active peer targets: `(target_id, peer)`.
fn watched_peers(db: &rusqlite::Connection) -> rusqlite::Result<Vec<(String, String)>> {
    let mut stmt = db.prepare(
        "SELECT DISTINCT target_id, target_peer FROM messaging_watch
         WHERE status = 'active' AND target_peer IS NOT NULL ORDER BY target_id",
    )?;
    stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect()
}

fn active_on_peer(db: &rusqlite::Connection, target: &str) -> rusqlite::Result<Vec<WatchRow>> {
    let mut stmt = db.prepare(&format!(
        "SELECT {} FROM messaging_watch
         WHERE target_id = ?1 AND status = 'active' AND target_peer IS NOT NULL
         ORDER BY created_ms, id",
        records::WATCH_COLUMNS
    ))?;
    stmt.query_map([target], records::watch_row)?.collect()
}

impl Messaging {
    /// [`Messaging::watch`] of `target`, a Session of peer `peer`.
    pub(super) async fn watch_remote(
        &self,
        request: WatchRequest,
        target: Target,
        peer: &str,
    ) -> Result<Watch, RuntimeError> {
        let subscriber = request.subscriber.clone();
        if let Some(expected) = request
            .expected_target
            .as_deref()
            .filter(|expected| *expected != target.candidate.session_id)
        {
            return Err(RuntimeError::Conflict(format!(
                "`{}` now names {} instead of the approved {expected}",
                request.target, target.candidate.session_id
            )));
        }
        let id = request
            .id
            .clone()
            .unwrap_or_else(|| cyber_core::ids::new_id("wch"));
        if let Some(row) = self.runtime.watch_by_id(&id)? {
            return replayed(&row, &subscriber, peer);
        }
        self.runtime.check_watch_quota(&subscriber, None)?;
        let session = target
            .address
            .rsplit_once('@')
            .map_or(target.address.as_str(), |(session, _)| session);
        let status = self
            .remote_status(peer, session)
            .await?
            .ok_or_else(|| RuntimeError::Address(AddressError::NotFound(target.address.clone())))?;
        let target_id = format!("{}@{peer}", status.session_id);
        let lock = self.runtime.message_lock(&target_id);
        let _watching = lock.lock().await;
        if let Some(row) = self.runtime.watch_by_id(&id)? {
            return replayed(&row, &subscriber, peer);
        }
        let created = WatchCreated {
            watch_id: id.clone(),
            target: request.address.unwrap_or(target.address),
            target_id,
            expires_ms: now_ms().saturating_add(WATCH_TTL_MS),
            target_peer: Some(peer.to_owned()),
        };
        self.runtime
            .record_message(&subscriber, event(WATCH_CREATED, &created))
            .await?;
        let projected = || {
            self.runtime
                .watch_by_id(&id)?
                .ok_or_else(|| RuntimeError::Corrupt(format!("watch {id} was not projected")))
        };
        if let Some(idle) = peer_idle(&status)
            && let Err(error) = self.runtime.fire(&projected()?, &idle).await
        {
            warn("idle notice failed", &error);
        }
        Ok(watch_of(&projected()?, false))
    }

    /// Ask the peers for the status of every watched peer Session and fire the watches of
    /// those that are idle (or no longer shown: `exited`). Returns the notices sent. Runs
    /// every [`REMOTE_POLL_EVERY`].
    pub async fn poll_remote_watches(&self) -> Result<usize, RuntimeError> {
        let lock = self.runtime.message_lock(LOCK);
        let _polling = lock.lock().await;
        let targets = self.runtime.inner.store.read(|db| Ok(watched_peers(db)?))?;
        let mut by_peer: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for (target_id, peer) in targets {
            by_peer.entry(peer).or_default().push(target_id);
        }
        let passes = by_peer
            .into_iter()
            .map(|(peer, targets)| self.poll_peer_watches(peer, targets));
        Ok(futures::future::join_all(passes).await.into_iter().sum())
    }

    /// Ask `peer` for each of its watched `targets` in turn, firing the idle ones, until
    /// one request fails: the rest are asked again at the next pass.
    async fn poll_peer_watches(&self, peer: String, targets: Vec<String>) -> usize {
        let mut fired = 0;
        for target_id in targets {
            let Some(session) = target_id.strip_suffix(&format!("@{peer}")) else {
                continue;
            };
            let idle = match self.remote_status(&peer, session).await {
                Ok(Some(status)) => peer_idle(&status),
                Ok(None) => Some(Idle::exited()),
                // Unreachable or refused: asked again later; the sweep expires it.
                Err(_) => break,
            };
            if let Some(idle) = idle {
                fired += self.fire_peer_target(&target_id, &idle).await;
            }
        }
        fired
    }

    /// Fire the active watches on peer target `target_id` under its send lock.
    async fn fire_peer_target(&self, target_id: &str, idle: &Idle) -> usize {
        let lock = self.runtime.message_lock(target_id);
        let _watching = lock.lock().await;
        let probe = target_id.to_owned();
        let watches = match self
            .runtime
            .inner
            .store
            .read(move |db| Ok(active_on_peer(db, &probe)?))
        {
            Ok(watches) => watches,
            Err(error) => {
                warn("reading watches failed", &RuntimeError::from(error));
                return 0;
            }
        };
        let mut fired = 0;
        for watch in watches {
            match self.runtime.fire(&watch, idle).await {
                Ok(true) => fired += 1,
                Ok(false) => {}
                Err(error) => warn("idle notice failed", &error),
            }
        }
        fired
    }
}

impl Runtime {
    /// Poll watched peer Sessions every [`REMOTE_POLL_EVERY`] until shutdown.
    pub fn spawn_remote_watch_poller(&self) {
        let weak = self.downgrade();
        let closed = self.inner.closed.clone();
        let task = tokio::spawn(async move {
            loop {
                tokio::select! {
                    () = closed.cancelled() => return,
                    () = tokio::time::sleep(REMOTE_POLL_EVERY) => {}
                }
                let Some(runtime) = weak.upgrade() else {
                    return;
                };
                if let Err(error) = runtime.messaging().poll_remote_watches().await {
                    warn("peer watch polling failed", &error);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_an_idle_peer_session_fires_its_watches() {
        let status = |state| PeerSessionStatus {
            session_id: "ses_1".into(),
            state,
            last_idle_ms: Some(0),
        };
        let idle = peer_idle(&status(ReachableState::Idle)).unwrap();
        assert_eq!(
            (idle.status.as_str(), idle.finished_at.as_str()),
            ("idle", "1970-01-01T00:00:00Z")
        );
        for state in [ReachableState::Running, ReachableState::WaitingInput] {
            assert!(peer_idle(&status(state)).is_none(), "{state:?}");
        }
    }
}
