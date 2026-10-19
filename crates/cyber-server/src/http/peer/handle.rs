//! A running peer listener as the main API sees it: where it listens, the identity it
//! presents, and the key rotation of `cyber peers invite --rotate-key`.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError, RwLock};

use super::PeerInfo;
use super::attach::Attaches;
use super::tls::{CertSlot, PeerIdentity, SubjectNames};

/// Called with the new fingerprint after a rotation (the server rewrites `server.json` and
/// republishes its mDNS record).
pub type RotateHook = Box<dyn Fn(&str) + Send + Sync>;

pub struct PeerHandle {
    /// The bound address.
    pub address: SocketAddr,
    /// `false` only for a plain listener on loopback.
    pub tls: bool,
    info: Arc<RwLock<PeerInfo>>,
    /// The listener's approved attaches.
    attaches: Arc<Attaches>,
    /// The presented certificate; `None` for a plain listener.
    slot: Option<Arc<CertSlot>>,
    /// The identity directory and the certificate's names, for a rotation.
    dir: PathBuf,
    names: SubjectNames,
    on_rotate: Mutex<Vec<RotateHook>>,
    /// Serializes rotations.
    rotating: Mutex<()>,
}

impl std::fmt::Debug for PeerHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PeerHandle")
            .field("address", &self.address)
            .field("tls", &self.tls)
            .finish_non_exhaustive()
    }
}

impl PeerHandle {
    pub fn new(
        address: SocketAddr,
        identity: &PeerIdentity,
        slot: Option<Arc<CertSlot>>,
        dir: PathBuf,
        names: SubjectNames,
    ) -> Self {
        let info = PeerInfo {
            machine: names.machine.clone(),
            fingerprint: identity.fingerprint().to_string(),
            share_paths: false,
            attach_wait: super::ATTACH_WAIT,
        };
        Self {
            address,
            tls: slot.is_some(),
            info: Arc::new(RwLock::new(info)),
            attaches: Arc::default(),
            slot,
            dir,
            names,
            on_rotate: Mutex::new(Vec::new()),
            rotating: Mutex::new(()),
        }
    }

    /// `https://<address>`, or `http://` for a plain listener.
    pub fn url(&self) -> String {
        let scheme = if self.tls { "https" } else { "http" };
        format!("{scheme}://{}", self.address)
    }

    /// `peers.share_paths` for the listings this listener answers.
    #[must_use]
    pub fn share_paths(self, share: bool) -> Self {
        self.info
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .share_paths = share;
        self
    }

    /// How long an attach waits for `remote.attach`; 2 minutes unless a test shortens it.
    #[must_use]
    pub fn attach_wait(self, wait: std::time::Duration) -> Self {
        self.info
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .attach_wait = wait;
        self
    }

    pub fn info(&self) -> PeerInfo {
        self.info
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    pub(super) fn shared_info(&self) -> Arc<RwLock<PeerInfo>> {
        Arc::clone(&self.info)
    }

    pub(super) fn attaches(&self) -> Arc<Attaches> {
        Arc::clone(&self.attaches)
    }

    /// End every attach of the grant named `name`, and its open event streams, after the
    /// grant was revoked.
    pub fn end_attaches(&self, name: &str) -> usize {
        self.attaches.end_grant(name)
    }

    /// Add a hook run after every rotation, in the order added.
    pub fn on_rotate(&self, hook: RotateHook) {
        self.on_rotate
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(hook);
    }

    /// Replace the key with a new one and present it from the next handshake on; returns
    /// the new fingerprint. Peers that pinned the old key fail with `fingerprint_mismatch`
    /// until they are paired again.
    pub fn rotate_key(&self) -> Result<String, String> {
        let _rotating = self.rotating.lock().unwrap_or_else(PoisonError::into_inner);
        let identity = PeerIdentity::rotate(&self.dir, &self.names)?;
        if let Some(slot) = &self.slot {
            slot.present(&identity)?;
        }
        let fingerprint = identity.fingerprint().to_string();
        self.info
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .fingerprint
            .clone_from(&fingerprint);
        for hook in self
            .on_rotate
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
        {
            hook(&fingerprint);
        }
        Ok(fingerprint)
    }
}
