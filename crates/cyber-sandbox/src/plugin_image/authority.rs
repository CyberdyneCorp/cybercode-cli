//! One owner per host incarnation, with a serialized final synchronous effect.
use std::sync::{Arc, Mutex, PoisonError};

use super::Error;

#[derive(Debug)]
struct Authority {
    gate: Mutex<()>,
    signal: tokio::sync::watch::Sender<bool>,
}

/// The single owner of a host incarnation. Dropping it permanently revokes all
/// leases, even though they retain the private signal used for panic revocation.
#[derive(Debug)]
pub struct Generation {
    authority: Arc<Authority>,
}
impl Generation {
    pub fn new() -> Self {
        let (signal, _) = tokio::sync::watch::channel(false);
        Self {
            authority: Arc::new(Authority {
                gate: Mutex::new(()),
                signal,
            }),
        }
    }
    pub fn lease(&self) -> Lease {
        Lease {
            authority: Arc::clone(&self.authority),
            signal: self.authority.signal.subscribe(),
        }
    }
    /// Commit only a lease issued by this same owner, while revocation is
    /// excluded. Borrowing the owner also prevents moving/dropping it inside the
    /// callback. If revocation won the gate, the callback is never called. Returning a future
    /// does not authorize its later execution. Do not call revoke/commit for this
    /// same authority from inside the callback; this gate is not reentrant.
    ///
    /// The callback owns native admission and child lifetime. A successful commit
    /// does not keep the generation live: its process supervisor must observe
    /// revocation and stop/reap the process before releasing protected resources.
    ///
    /// ```compile_fail
    /// use cyber_sandbox::plugin_image::Generation;
    /// let owner = Generation::new();
    /// let lease = owner.lease();
    /// owner.commit(&lease, || drop(owner)); // Cannot drop the borrowed owner.
    /// ```
    pub fn commit<T>(&self, lease: &Lease, start: impl FnOnce() -> T) -> Result<T, Error> {
        if !Arc::ptr_eq(&self.authority, &lease.authority) {
            return Err(Error::Revoked);
        }
        let _guard = match self.authority.gate.lock() {
            Ok(guard) => guard,
            Err(_) => {
                self.authority.signal.send_replace(true);
                return Err(Error::Revoked);
            }
        };
        lease.check()?;
        let _panic_revocation = PanicRevocation(&self.authority.signal);
        Ok(start())
    }
    /// Wait for any committed synchronous start, then permanently revoke.
    /// Process termination/reaping remains the process owner's responsibility.
    pub fn revoke(&self) {
        let _guard = self
            .authority
            .gate
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        self.authority.signal.send_replace(true);
    }
}
impl Default for Generation {
    fn default() -> Self {
        Self::new()
    }
}
impl Drop for Generation {
    fn drop(&mut self) {
        self.revoke();
    }
}

/// Read-only authority. Cloning a lease cannot renew its owner generation.
#[derive(Debug, Clone)]
pub struct Lease {
    authority: Arc<Authority>,
    signal: tokio::sync::watch::Receiver<bool>,
}
impl Lease {
    pub fn is_current(&self) -> bool {
        !*self.signal.borrow()
    }
    pub(super) fn check(&self) -> Result<(), Error> {
        if self.is_current() {
            Ok(())
        } else {
            Err(Error::Revoked)
        }
    }
    /// Observe permanent revocation, including owner drop and commit panic.
    pub async fn revoked(&self) {
        let mut signal = self.signal.clone();
        let _ = signal.wait_for(|revoked| *revoked).await;
    }
}

struct PanicRevocation<'a>(&'a tokio::sync::watch::Sender<bool>);
impl Drop for PanicRevocation<'_> {
    fn drop(&mut self) {
        if std::thread::panicking() {
            self.0.send_replace(true);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::Duration;

    #[test]
    fn revoked_and_dropped_authorities_never_enter_a_commit() {
        for drop_owner in [false, true] {
            let owner = Generation::new();
            let lease = owner.lease();
            if drop_owner {
                drop(owner);
            } else {
                owner.revoke();
                assert_eq!(
                    owner.commit(&lease, || panic!("revoked start executed")),
                    Err(Error::Revoked)
                );
            }
            let fresh = Generation::new();
            assert_eq!(
                fresh.commit(&lease, || panic!("revoked start executed")),
                Err(Error::Revoked)
            );
            assert_eq!(fresh.commit(&fresh.lease(), || 42), Ok(42));
            assert!(!lease.is_current());
        }
    }

    #[test]
    fn a_different_owner_cannot_commit_a_still_live_lease() {
        let original = Generation::new();
        let lease = original.lease();
        let other = Generation::new();
        assert_eq!(
            other.commit(&lease, || panic!("foreign generation start executed")),
            Err(Error::Revoked)
        );
        assert!(lease.is_current());
        assert_eq!(original.commit(&lease, || 42), Ok(42));
    }

    #[tokio::test]
    async fn a_panicking_commit_revokes_all_leases_and_wakes_supervision() {
        let owner = Generation::new();
        let lease = owner.lease();
        let observer = lease.clone();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            owner.commit(&lease, || panic!("controlled startup panic"))
        }));
        assert!(result.is_err());
        tokio::time::timeout(Duration::from_secs(1), observer.revoked())
            .await
            .unwrap();
        assert!(!observer.is_current());
        assert!(!owner.lease().is_current());
        assert_eq!(
            owner.commit(&observer, || panic!("poisoned start executed")),
            Err(Error::Revoked)
        );
        owner.revoke(); // Poison recovery cannot renew authority or panic on drop.
    }

    #[test]
    fn revocation_serializes_with_an_already_committed_start() {
        let owner = Arc::new(Generation::new());
        let lease = owner.lease();
        let worker_owner = Arc::clone(&owner);
        let revoking_owner = Arc::clone(&owner);
        let worker_lease = lease.clone();
        let (entered, entrance) = mpsc::channel();
        let (release, released) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            worker_owner.commit(&worker_lease, || {
                // In addition to the observable ordering below, require mutual
                // exclusion deterministically even on a heavily delayed test host.
                let protected = matches!(
                    worker_lease.authority.gate.try_lock(),
                    Err(std::sync::TryLockError::WouldBlock)
                );
                entered.send(()).unwrap();
                released.recv_timeout(Duration::from_secs(5)).unwrap();
                (protected, worker_lease.is_current())
            })
        });
        entrance.recv_timeout(Duration::from_secs(5)).unwrap();
        let (requested, request) = mpsc::channel();
        let (done, completion) = mpsc::channel();
        let revoker = std::thread::spawn(move || {
            requested.send(()).unwrap();
            revoking_owner.revoke();
            done.send(()).unwrap();
        });
        request.recv_timeout(Duration::from_secs(5)).unwrap();
        let early = completion.recv_timeout(Duration::from_millis(100));
        release.send(()).unwrap();
        // Settle both threads before assertions, including the missing-gate control.
        let result = worker.join().unwrap().unwrap();
        revoker.join().unwrap();
        assert_eq!(
            early,
            Err(mpsc::RecvTimeoutError::Timeout),
            "revocation returned while start was still active"
        );
        assert_eq!(
            result,
            (true, true),
            "committed start lost its generation gate"
        );
        assert!(!lease.is_current());
        assert_eq!(
            owner.commit(&lease, || panic!("late start executed")),
            Err(Error::Revoked)
        );
    }

    #[cfg(unix)]
    #[test]
    fn actual_process_start_is_refused_after_revocation() {
        let root = tempfile::tempdir().unwrap();
        let owner = Generation::new();
        let lease = owner.lease();
        let marker = root.path().join("committed");
        let mut child = owner
            .commit(&lease, || {
                std::process::Command::new("/usr/bin/touch")
                    .arg(&marker)
                    .spawn()
            })
            .unwrap()
            .unwrap();
        owner.revoke();
        assert!(child.wait().unwrap().success());
        assert!(marker.is_file());
        let refused = root.path().join("refused");
        assert!(matches!(
            owner.commit(&lease, || std::process::Command::new("/usr/bin/touch")
                .arg(&refused)
                .spawn()),
            Err(Error::Revoked)
        ));
        assert!(!refused.exists());
    }
}
