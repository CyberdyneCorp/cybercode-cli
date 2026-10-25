//! A local native capability. No hub/peer route can arm it. All OS work runs on a
//! blocking worker while Stop and presence cancellation remain independent.
mod artifacts;
mod error;
mod journal;
mod runtime;
#[cfg(test)]
mod tests;

pub use error::{ComputerError, Result};
pub use runtime::ComputerPolicy;

use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Instant,
};

use cyber_computer::{
    Action,
    backend::{Backend, Health, HostLease, NativeTarget, Selector},
    control::{Admission, Controller, Target},
};
use cyber_store::Store;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};

type Policy = dyn Fn() -> ComputerPolicy + Send + Sync;
type Attendance = dyn Fn(&str) -> bool + Send + Sync;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[schemars(rename = "ComputerUnknownOperation")]
pub struct UnknownOperation {
    pub session_id: String,
    pub operation_id: String,
    pub action: String,
    pub started_ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[schemars(rename = "ComputerFrameInfo")]
pub struct FrameInfo {
    pub id: String,
    pub session_id: String,
    pub grant_id: String,
    pub captured_at: u64,
    pub target: NativeTarget,
    pub sha256: String,
    pub bytes: u64,
    pub created_ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[schemars(rename = "ComputerReceipt")]
pub struct Receipt {
    pub operation_id: String,
    pub action: String,
    pub outcome: String,
    pub replayed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub frame: Option<FrameInfo>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
#[schemars(rename = "ComputerGrantView")]
pub struct GrantView {
    pub id: String,
    pub session_id: String,
    pub target: Target,
    pub expires_at: i64,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
#[schemars(rename = "ComputerStatus")]
pub struct Status {
    pub enabled: bool,
    pub latest_frame: Option<FrameInfo>,
    pub health: Health,
    pub grant: Option<GrantView>,
    pub unknown_operations: Vec<UnknownOperation>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
#[schemars(rename = "ComputerHealthReport")]
pub struct HealthReport {
    pub enabled: bool,
    pub grant: Option<GrantView>,
    pub health: Health,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
#[schemars(rename = "ComputerPreview")]
pub struct Preview {
    pub id: String,
    pub session_id: String,
    pub target: NativeTarget,
    pub expires_at: i64,
    pub unknown_operations: Vec<UnknownOperation>,
    pub warning: String,
}

struct Reviewed {
    preview: Preview,
    selector: Selector,
    generation: u64,
    deadline: Instant,
    presence_epoch: u64,
}

pub struct Computer {
    controller: Arc<Controller>,
    host_lease: Arc<Mutex<Option<(String, String, HostLease)>>>,
    backend: Arc<dyn Backend>,
    journal: journal::Journal,
    artifacts: PathBuf,
    artifact_io: Mutex<()>,
    policy: Arc<Policy>,
    attendance: Arc<Attendance>,
    presence_epoch: Arc<dyn Fn(&str) -> u64 + Send + Sync>,
    last_accessibility: std::sync::atomic::AtomicBool,
    clock: Instant,
    previews: Mutex<BTreeMap<String, Reviewed>>,
    preview_generation: std::sync::atomic::AtomicU64,
}

/// Runs in the authoritative Session-deletion transaction. Minimal native
/// outcome/unknown-action ledger rows remain for reconciliation, never pixels.
pub(super) fn mark_session_deleted(
    tx: &rusqlite::Transaction<'_>,
    session: &str,
) -> rusqlite::Result<()> {
    tx.execute(
        "INSERT OR IGNORE INTO computer_deleted_session(session_id,deleted_ms) VALUES(?1,?2)",
        rusqlite::params![session, wall_ms()],
    )?;
    tx.execute(
        "UPDATE computer_frame SET expired=1 WHERE session_id=?1",
        [session],
    )?;
    Ok(())
}

impl Computer {
    pub fn offered(&self) -> bool {
        (self.policy)().enabled
            && self
                .backend
                .health(false)
                .is_ok_and(|health| health.supported)
    }
    pub fn health(&self, prompt: bool) -> Result<HealthReport> {
        if prompt {
            self.require_enabled()?;
        }
        let health = self.checked_health(prompt)?;
        Ok(HealthReport {
            enabled: self.enabled(),
            grant: self.global_grant(),
            health,
        })
    }
    pub fn require_stopped(&self) -> Result<()> {
        if self.controller.has_authority(self.now()) {
            return Err(cyber_computer::Error::Consent("stop native computer control before changing administrative authority or answering other tools").into());
        }
        Ok(())
    }
    pub fn stop_all(&self) {
        self.preview_generation
            .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
        self.controller.stop();
        self.release_host_lease(None);
        if let Ok(mut previews) = self.previews.lock() {
            previews.clear();
        }
    }
    pub fn new(
        store: Arc<Store>,
        artifacts: PathBuf,
        policy: Arc<Policy>,
        attendance: Arc<Attendance>,
        backend: Arc<dyn Backend>,
    ) -> Result<Self> {
        Self::with_supervisor(
            store,
            artifacts,
            policy,
            attendance,
            backend,
            Arc::new(|| true),
        )
    }
    pub fn with_supervisor(
        store: Arc<Store>,
        artifacts: PathBuf,
        policy: Arc<Policy>,
        attendance: Arc<Attendance>,
        backend: Arc<dyn Backend>,
        supervisor: Arc<dyn Fn() -> bool + Send + Sync>,
    ) -> Result<Self> {
        let journal = journal::Journal(store);
        journal.recover()?;
        let (checked_policy, checked_attendance) = (Arc::clone(&policy), Arc::clone(&attendance));
        let guard =
            Arc::new(move |session: &str| checked_policy().enabled && checked_attendance(session));
        Ok(Self {
            controller: Arc::new(Controller::with_supervisor(
                policy().enabled,
                guard,
                supervisor,
            )),
            host_lease: Arc::new(Mutex::new(None)),
            backend,
            journal,
            artifacts,
            artifact_io: Mutex::new(()),
            policy,
            attendance,
            presence_epoch: Arc::new(|_| 0),
            last_accessibility: std::sync::atomic::AtomicBool::new(false),
            clock: Instant::now(),
            previews: Mutex::new(BTreeMap::new()),
            preview_generation: std::sync::atomic::AtomicU64::new(0),
        })
    }
    fn set_presence_epoch(&mut self, epoch: Arc<dyn Fn(&str) -> u64 + Send + Sync>) {
        Arc::get_mut(&mut self.controller)
            .expect("unshared computer controller")
            .set_presence_epoch(epoch.clone());
        self.presence_epoch = epoch;
    }
    fn checked_health(&self, prompt: bool) -> Result<Health> {
        let health = self.backend.health(prompt)?;
        let had_access = self
            .last_accessibility
            .swap(health.accessibility, std::sync::atomic::Ordering::AcqRel);
        if !health.screen_capture || (had_access && !health.accessibility) {
            self.stop_all();
        }
        Ok(health)
    }
    fn now(&self) -> u64 {
        self.clock.elapsed().as_millis().min(u64::MAX as u128) as u64
    }
    fn enabled(&self) -> bool {
        let enabled = (self.policy)().enabled;
        self.controller.set_enabled(enabled);
        if !enabled {
            self.release_host_lease(None);
        }
        enabled
    }
    fn require_enabled(&self) -> Result<()> {
        if !self.enabled() {
            return Err(cyber_computer::Error::Disabled.into());
        }
        Ok(())
    }
    fn admission<'a>(
        &self,
        session: &'a str,
        target: &'a Target,
        permitted: bool,
        capture: bool,
    ) -> Result<Admission<'a>> {
        self.require_enabled()?;
        let health = self.checked_health(false)?;
        if !health.supported {
            return Err(cyber_computer::Error::UnsupportedPlatform.into());
        }
        Ok(Admission {
            session,
            now: self.now(),
            human_present: (self.attendance)(session),
            permitted,
            os_consent: health.screen_capture && (capture || health.accessibility),
            target,
        })
    }
    pub fn status(&self, session: &str) -> Result<Status> {
        Ok(Status {
            enabled: self.enabled(),
            health: self.checked_health(false)?,
            latest_frame: self
                .journal
                .latest_frame(session, (self.policy)().retention_days)?,
            grant: self.grant(session),
            unknown_operations: self.journal.unknown(session)?,
        })
    }
    fn global_grant(&self) -> Option<GrantView> {
        let now = self.now();
        let Some(grant) = self.controller.current(now) else {
            self.controller.stop();
            self.release_host_lease(None);
            return None;
        };
        Some(GrantView {
            id: grant.id,
            session_id: grant.session_id,
            target: grant.target,
            expires_at: wall_ms()
                .saturating_add(grant.expires_at.saturating_sub(now).min(i64::MAX as u64) as i64),
        })
    }
    fn grant(&self, session: &str) -> Option<GrantView> {
        self.global_grant()
            .filter(|grant| grant.session_id == session)
    }
    pub fn setup(&self) -> Result<Health> {
        self.require_enabled()?;
        self.checked_health(true)
    }
    pub fn preview(&self, session: &str, selector: Selector) -> Result<Preview> {
        self.require_enabled()?;
        selector.validate()?;
        let target = self.backend.resolve(&selector)?;
        let scope = target.target();
        let facts = self.admission(session, &scope, true, true)?;
        if !facts.os_consent {
            return Err(
                cyber_computer::Error::Consent("Screen Recording permission is required").into(),
            );
        }
        if !(self.attendance)(session) {
            return Err(cyber_computer::Error::Consent(
                "attach a terminal or browser to this Session before arming",
            )
            .into());
        }
        let preview = Preview { id:cyber_core::ids::new_id("cup"), session_id:session.into(), target, expires_at:wall_ms()+30_000, unknown_operations:self.journal.unknown_all()?, warning:"Native capture supplies visible application/display content to the selected model. Input controls the host application outside the workspace filesystem sandbox. Review this exact scope; Stop revokes future input, not already delivered events.".into() };
        let mut previews = self.previews.lock().map_err(|_| ComputerError::Storage)?;
        previews.retain(|_, value| value.deadline > Instant::now());
        if previews.len() >= 16 {
            return Err(cyber_computer::Error::Limit("too many pending target reviews").into());
        }
        previews.insert(
            preview.id.clone(),
            Reviewed {
                preview: preview.clone(),
                selector,
                generation: self
                    .preview_generation
                    .load(std::sync::atomic::Ordering::Acquire),
                deadline: Instant::now() + std::time::Duration::from_secs(30),
                presence_epoch: (self.presence_epoch)(session),
            },
        );
        Ok(preview)
    }
    pub fn allow(
        &self,
        session: &str,
        preview: &str,
        ttl_seconds: Option<u64>,
        acknowledge_unknown: bool,
    ) -> Result<GrantView> {
        self.require_enabled()?;
        let reviewed = self
            .previews
            .lock()
            .map_err(|_| ComputerError::Storage)?
            .remove(preview)
            .ok_or(ComputerError::Conflict)?;
        if reviewed.presence_epoch != (self.presence_epoch)(session)
            || reviewed.preview.session_id != session
            || reviewed.deadline <= Instant::now()
            || reviewed.generation
                != self
                    .preview_generation
                    .load(std::sync::atomic::Ordering::Acquire)
        {
            return Err(ComputerError::Conflict);
        }
        if self.controller.busy() {
            return Err(cyber_computer::Error::Busy.into());
        }
        let current = self.backend.resolve(&reviewed.selector)?;
        if current != reviewed.preview.target {
            return Err(ComputerError::Conflict);
        }
        if !reviewed.preview.unknown_operations.is_empty() && !acknowledge_unknown {
            return Err(ComputerError::Unknown);
        }
        let scope = current.target();
        let ttl = ttl_seconds
            .map(|seconds| {
                seconds
                    .checked_mul(1000)
                    .ok_or(ComputerError::Invalid("invalid grant duration"))
            })
            .transpose()?;
        let facts = self.admission(session, &scope, true, true)?;
        // Release stale grants before acquiring a process-wide native lock.
        if self.controller.current(self.now()).is_none() {
            self.controller.stop();
            self.release_host_lease(None);
        }
        let lease = self.backend.acquire_lease()?;
        self.journal
            .acknowledge(&reviewed.preview.unknown_operations)?;
        if reviewed.deadline <= Instant::now()
            || reviewed.presence_epoch != (self.presence_epoch)(session)
        {
            return Err(ComputerError::Conflict);
        }
        let grant = self
            .controller
            .arm_at_epoch(facts, ttl, reviewed.presence_epoch)?;
        *self.host_lease.lock().map_err(|_| ComputerError::Storage)? =
            Some((grant.id.clone(), session.into(), lease));
        self.expire_host_lease(
            grant.id.clone(),
            grant.expires_at.saturating_sub(self.now()),
        );
        // Stop increments the review epoch before revoking the controller. A
        // Stop racing between review validation and arm must also fence the new
        // grant; checking again closes the check/arm gap without blocking Stop.
        if reviewed.deadline <= Instant::now()
            || reviewed.presence_epoch != (self.presence_epoch)(session)
            || reviewed.generation
                != self
                    .preview_generation
                    .load(std::sync::atomic::Ordering::Acquire)
        {
            self.controller.stop_session(session);
            self.release_host_lease(Some(session));
            return Err(ComputerError::Conflict);
        }
        self.grant(session).ok_or(ComputerError::Conflict)
    }
    pub fn stop(&self, session: &str) {
        self.preview_generation
            .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
        self.controller.stop_session(session);
        self.release_host_lease(Some(session));
        if let Ok(mut previews) = self.previews.lock() {
            previews.retain(|_, value| value.preview.session_id != session);
        }
    }
    pub fn screenshot(&self, session: &str, operation: &str) -> Result<Receipt> {
        validate_operation(operation)?;
        let fingerprint = digest(b"computer:screenshot:v1");
        if let Some(receipt) = self.journal.replay(session, operation, &fingerprint)? {
            return Ok(receipt);
        }
        let grant = self
            .controller
            .current(self.now())
            .filter(|grant| grant.session_id == session)
            .ok_or(cyber_computer::Error::NotArmed)?;
        let _host_lease = self.worker_lease(session)?;
        let selector = self.selector_for_identity(&grant.target)?;
        let target = self.backend.resolve(&selector)?;
        let scope = target.target();
        let permit = self
            .controller
            .capture(self.admission(session, &scope, true, true)?)?;
        self.prune()?;
        if self.journal.stored_bytes()? > 256 * 1024 * 1024 - 8 * 1024 * 1024 {
            return Err(
                cyber_computer::Error::Limit("private capture storage is full (256 MiB)").into(),
            );
        }
        self.journal.begin(
            session,
            operation,
            "screenshot",
            &fingerprint,
            json!({"action":"screenshot","grant":grant.id,"target":scope}),
            wall_ms(),
        )?;
        // Once capture begins, every outcome gets a durable settlement. Cancellation
        // or private artifact failures must never leave a replayable native request.
        let captured = (|| -> Result<FrameInfo> {
            if permit.cancelled() {
                return Err(cyber_computer::Error::NotArmed.into());
            }
            let capture = self
                .backend
                .capture(&target)
                .inspect_err(|_| self.stop(session))?;
            capture.validate()?;
            if capture.target != target {
                return Err(cyber_computer::Error::StaleFrame.into());
            }
            let frame = self
                .controller
                .frame(&permit, self.now(), capture.target.geometry()?)?;
            self.store_frame(frame, grant.id, capture)
        })();
        let receipt = Receipt {
            operation_id: operation.into(),
            action: "screenshot".into(),
            outcome: if captured.is_ok() {
                "captured"
            } else if permit.cancelled() {
                "cancelled"
            } else {
                "unknown"
            }
            .into(),
            replayed: false,
            frame: captured.ok(),
        };
        if self.journal.settle(&receipt, wall_ms()).is_err() {
            self.stop(session);
            return Err(ComputerError::Unknown);
        }
        Ok(receipt)
    }
    pub fn input(
        &self,
        session: &str,
        operation: &str,
        frame_id: &str,
        action: Action,
        permitted: bool,
    ) -> Result<Receipt> {
        validate_operation(operation)?;
        let audit = json!({"frame_id":frame_id,"input":action.audit_value()});
        let fingerprint = digest(audit.to_string().as_bytes());
        if let Some(receipt) = self.journal.replay(session, operation, &fingerprint)? {
            return Ok(receipt);
        }
        let _host_lease = self.worker_lease(session)?;
        let (frame_info, target) = self.journal.read_frame(
            session,
            frame_id,
            wall_ms(),
            (self.policy)().retention_days,
        )?;
        let current = self.backend.resolve(&target.selector())?;
        if current != target {
            return Err(cyber_computer::Error::StaleFrame.into());
        }
        let scope = current.target();
        let kind = action.kind().to_owned();
        let permit = self.controller.input(
            self.admission(session, &scope, permitted, false)?,
            frame_id,
            action,
            &current.geometry()?,
        )?;
        let audit = json!({"frame_id":frame_id,"input":audit["input"],"grant_id":frame_info.grant_id,"target":target,"permitted":permitted});
        self.journal
            .begin(session, operation, &kind, &fingerprint, audit, wall_ms())?;
        let outcome = if permit.cancelled() {
            "cancelled"
        } else {
            match self.backend.input(&target, &permit) {
                Ok(()) => "posted",
                Err(_) => {
                    self.stop(session);
                    "unknown"
                }
            }
        };
        let receipt = Receipt {
            operation_id: operation.into(),
            action: kind,
            outcome: outcome.into(),
            replayed: false,
            frame: None,
        };
        if self.journal.settle(&receipt, wall_ms()).is_err() {
            self.stop(session);
            return Err(ComputerError::Unknown);
        }
        Ok(receipt)
    }
    fn worker_lease(&self, session: &str) -> Result<HostLease> {
        self.host_lease
            .lock()
            .map_err(|_| ComputerError::Storage)?
            .as_ref()
            .filter(|(_, owner, _)| owner == session)
            .map(|(_, _, lease)| Arc::clone(lease))
            .ok_or(cyber_computer::Error::NotArmed.into())
    }
    fn release_host_lease(&self, session: Option<&str>) {
        if let Ok(mut held) = self.host_lease.lock()
            && held
                .as_ref()
                .is_some_and(|(_, owner, _)| session.is_none_or(|session| owner == session))
        {
            held.take();
        }
    }
    fn expire_host_lease(&self, id: String, milliseconds: u64) {
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            let controller = Arc::downgrade(&self.controller);
            let held = Arc::downgrade(&self.host_lease);
            runtime.spawn(async move {
                tokio::time::sleep(std::time::Duration::from_millis(milliseconds)).await;
                if let Some(controller) = controller.upgrade() {
                    controller.revoke_grant(&id);
                }
                if let Some(held) = held.upgrade()
                    && let Ok(mut held) = held.lock()
                    && held.as_ref().is_some_and(|(grant, _, _)| grant == &id)
                {
                    held.take();
                }
            });
        }
    }
    fn selector_for_identity(&self, target: &Target) -> Result<Selector> {
        let identity = &target.identity;
        if let Some(rest) = identity.strip_prefix("app:") {
            let app = rest.split(':').next().unwrap_or_default();
            return Ok(Selector {
                app: Some(app.into()),
                display: None,
            });
        }
        if let Some(display) = identity
            .strip_prefix("display:")
            .and_then(|value| value.parse().ok())
        {
            return Ok(Selector {
                app: None,
                display: Some(display),
            });
        }
        Err(ComputerError::Invalid("native target identity is invalid"))
    }
}

fn validate_operation(operation: &str) -> Result<()> {
    if operation.is_empty()
        || operation.len() > 200
        || !operation.bytes().all(|byte| byte.is_ascii_graphic())
    {
        return Err(ComputerError::Invalid("invalid operation identity"));
    }
    Ok(())
}
fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn wall_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(i64::MAX as u128) as i64
}
