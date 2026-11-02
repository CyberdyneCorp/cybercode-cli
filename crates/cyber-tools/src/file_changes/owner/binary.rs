//! Binary acknowledgements are hints to the existing native producer. They do
//! not arm a new observer, renew input provenance or directly run callbacks.
use super::*;
use cyber_core::binary::editor::AcknowledgedChange;
use cyber_server::runtime::binary_effects::State as HandoffState;

pub(super) struct Pending {
    change: Arc<AcknowledgedChange>,
    member: CancellationToken,
}
impl Observers {
    pub(crate) fn binary_changed(
        &self,
        host: &BuiltinHost,
        change: Arc<AcknowledgedChange>,
    ) -> Result<(), String> {
        let runtime = host.runtime().ok_or("runtime unavailable")?;
        let queued = runtime
            .queue_binary_change(&change)
            .map_err(|error| error.to_string())?;
        if !matches!(queued.state, HandoffState::Queued) {
            runtime
                .reconcile_binary_handoff(change.key())
                .map_err(|error| error.to_string())?;
            return Ok(());
        }
        // Registry/membership locks protect only this bounded queue. No journal
        // IO, native metadata check or callback takes place under these locks.
        let accepted = {
            let registry = self.0.lock().unwrap_or_else(PoisonError::into_inner);
            registry
                .entries
                .get(&change.owner().directory)
                .filter(|entry| !entry.cancel.is_cancelled())
                .map(|entry| {
                    let mut members = entry.members.lock().unwrap_or_else(PoisonError::into_inner);
                    let member = members
                        .sessions
                        .get(&change.owner().session_id)
                        .filter(|cancel| !cancel.is_cancelled())
                        .cloned();
                    match member {
                        Some(_) if members.binary.contains_key(change.key()) => Ok(true),
                        Some(member) if members.binary.len() < 8 => {
                            members.binary.insert(
                                change.key().into(),
                                Pending {
                                    change: change.clone(),
                                    member,
                                },
                            );
                            Ok(true)
                        }
                        Some(_) => Err("Binary observation queue limit reached"),
                        None => Ok(false),
                    }
                })
                .unwrap_or(Ok(false))
        }?;
        if !accepted {
            runtime
                .suppress_binary_change(change.key(), "no_observer")
                .map_err(|error| error.to_string())?;
        }
        Ok(())
    }
}
fn take(membership: &Members) -> BTreeMap<String, Pending> {
    std::mem::take(
        &mut membership
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .binary,
    )
}
pub(super) fn suppress_pending(host: &BuiltinHost, membership: &Members) {
    let pending = take(membership);
    let Some(runtime) = host.runtime() else {
        return;
    };
    for (key, pending) in pending {
        if let Err(error) = runtime.suppress_binary_change(&key, "no_observer") {
            notice(
                host,
                &pending.change.owner().session_id,
                &format!("Binary observation disposition remains pending: {error}"),
            );
        }
    }
}
impl Observation {
    pub(super) fn claim_binary(
        &self,
        host: &BuiltinHost,
        membership: &Members,
    ) -> Vec<Arc<AcknowledgedChange>> {
        let pending = take(membership);
        let Some(runtime) = host.runtime() else {
            return vec![];
        };
        let mut claimed = vec![];
        for (key, pending) in pending {
            let result = if pending.member.is_cancelled() {
                runtime.suppress_binary_change(&key, "origin_expired")
            } else {
                runtime.claim_binary_observation(&self.journal, &pending.change)
            };
            match result {
                Ok(record) => {
                    if let HandoffState::Claimed { path, .. } = record.state {
                        membership
                            .lock()
                            .unwrap_or_else(PoisonError::into_inner)
                            .explicit
                            .record(Path::new(&path));
                        // Keep every native pin through reconciliation and its
                        // durable observation handoff, including after HTTP close.
                        claimed.push(pending.change);
                    }
                }
                Err(error) => notice(
                    host,
                    &pending.change.owner().session_id,
                    &format!("Binary file observation remains pending: {error}"),
                ),
            }
        }
        claimed
    }
    pub(super) fn finish_binary(&self, host: &BuiltinHost, changes: &[Arc<AcknowledgedChange>]) {
        let Some(runtime) = host.runtime() else {
            return;
        };
        for change in changes {
            let key = change.key();
            let session = &change.owner().session_id;
            let result = runtime.reconcile_binary_handoff(key).and_then(|record| {
                if matches!(record.state, HandoffState::Claimed { .. }) {
                    runtime.suppress_binary_change(key, "observation_unavailable")
                } else {
                    Ok(record)
                }
            });
            if let Err(error) = result {
                notice(
                    host,
                    session,
                    &format!("Binary native observation reference remains pending: {error}"),
                );
            }
        }
    }
}
