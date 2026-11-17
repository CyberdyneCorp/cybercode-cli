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
    Action, ReasonCode,
    backend::{Backend, EventClass, Health, HostLease, NativeTarget, Revocation, Selector},
    capability::{Evidence, SCHEMA_VERSION, legacy_schema_version},
    chord,
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
    /// 1 for frames recorded before the platform-neutral contract.
    #[serde(default = "legacy_schema_version")]
    pub schema_version: u32,
    pub id: String,
    pub session_id: String,
    pub grant_id: String,
    pub captured_at: u64,
    pub target: NativeTarget,
    pub sha256: String,
    pub bytes: u64,
    pub created_ms: i64,
}

/// Input event counts; never their contents.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[schemars(rename = "ComputerEventCounts")]
pub struct EventCounts {
    pub requested: u32,
    pub posted: u32,
}

/// Whether pressed keys and buttons were released after the action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[schemars(rename = "ComputerCleanup")]
pub enum Cleanup {
    NotNeeded,
    Released,
    Uncertain,
}

/// Version 1 rows keep exactly the fields they were written with; the fields added
/// by version 2 are omitted when empty, so a replayed version 1 receipt is returned
/// as written. Receipts never carry typed text, key characters or window titles.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[schemars(rename = "ComputerReceipt")]
pub struct Receipt {
    pub operation_id: String,
    pub action: String,
    pub outcome: String,
    pub replayed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub frame: Option<FrameInfo>,
    #[serde(default = "legacy_schema_version")]
    pub schema_version: u32,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub platform: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub backend: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub grant_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub target_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frame_id: Option<String>,
    /// Why the action was refused, failed or became unknown.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<ReasonCode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub events: Option<EventCounts>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cleanup: Option<Cleanup>,
    /// Assurance checks the adapter reported as passed before the last event.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checks: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub warnings: Option<Vec<ReasonCode>>,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub started_ms: i64,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub settled_ms: i64,
}

fn is_zero(value: &i64) -> bool {
    *value == 0
}

/// References shared by the receipts of one admitted action.
struct ReceiptScope<'a> {
    operation: &'a str,
    action: String,
    grant_id: &'a str,
    target: &'a NativeTarget,
    started_ms: i64,
}

impl ReceiptScope<'_> {
    fn receipt(&self, outcome: &str) -> Receipt {
        let source = self.target.described_source();
        Receipt {
            operation_id: self.operation.into(),
            action: self.action.clone(),
            outcome: outcome.into(),
            schema_version: SCHEMA_VERSION,
            platform: source.platform,
            backend: source.backend,
            grant_id: self.grant_id.into(),
            target_id: self.target.target_id.clone(),
            started_ms: self.started_ms,
            settled_ms: wall_ms(),
            ..Receipt::default()
        }
    }
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
    pub schema_version: u32,
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
    pub schema_version: u32,
    pub id: String,
    pub session_id: String,
    pub target: NativeTarget,
    pub expires_at: i64,
    pub unknown_operations: Vec<UnknownOperation>,
    pub warning: String,
    /// Support evidence of the running configuration, shown on the review card.
    pub evidence: Evidence,
    /// Whether `allow` may re-run this selector; otherwise only this review can arm.
    pub re_resolvable: bool,
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
    last_input: std::sync::atomic::AtomicBool,
    clock: Instant,
    previews: Mutex<BTreeMap<String, Reviewed>>,
    preview_generation: std::sync::atomic::AtomicU64,
    /// Process-wide display-topology epoch; frames from an older epoch are stale.
    topology: std::sync::atomic::AtomicU64,
    /// The reviewed target of the current grant, with its server-issued references.
    armed: Mutex<Option<(String, String, NativeTarget)>>,
}

/// Prepare private capture storage before any model/plugin process can launch.
/// Bootstrap registers this directory in its home-scoped sandbox hidden paths;
/// modes alone do not isolate same-user subprocesses or full-access owner shells.
pub fn prepare_artifact_root(path: &std::path::Path) -> Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            return Err(ComputerError::Storage);
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            std::fs::create_dir_all(path)?
        }
        Err(_) => return Err(ComputerError::Storage),
    }
    let metadata = std::fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(ComputerError::Storage);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
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
        let enabled = self.enabled();
        Ok(HealthReport {
            enabled,
            grant: self.global_grant(),
            health: reported(health, enabled),
        })
    }
    pub fn require_stopped(&self) -> Result<()> {
        if self.controller.has_authority(self.now()) {
            return Err(cyber_computer::Error::Consent("stop native computer control before changing administrative authority or answering other tools").into());
        }
        Ok(())
    }
    /// Fences every grant first, then tears down platform scopes.
    pub fn stop_all(&self) {
        self.preview_generation
            .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
        self.controller.stop();
        self.release_host_lease(None);
        let reviews = self
            .previews
            .lock()
            .map(|mut previews| std::mem::take(&mut *previews))
            .unwrap_or_default();
        let armed = self.armed.lock().ok().and_then(|mut armed| armed.take());
        self.end_scopes(reviews.into_values().map(|review| review.preview.target));
        self.end_scopes(armed.map(|(_, _, target)| target));
    }
    fn end_scopes(&self, targets: impl IntoIterator<Item = NativeTarget>) {
        for target in targets {
            self.backend.end_scope(&target);
        }
    }
    /// Wires the backend's revocation sink to this runtime. Binding loss fences the
    /// grant before any platform teardown; a topology change only stales frames.
    pub fn install_revocation_sink(self: &Arc<Self>) {
        let weak = Arc::downgrade(self);
        self.backend
            .set_revocation_sink(Arc::new(move |event: Revocation| {
                if let Some(computer) = weak.upgrade() {
                    computer.revoked(&event);
                }
            }));
    }
    fn revoked(&self, event: &Revocation) {
        if event.class == EventClass::TopologyChange {
            self.topology
                .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
            return;
        }
        let unrelated = event.identity.as_ref().is_some_and(|identity| {
            self.armed.lock().ok().is_some_and(|armed| {
                armed
                    .as_ref()
                    .is_some_and(|(_, _, target)| &target.identity != identity)
            })
        });
        if !unrelated {
            self.stop_all();
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
        prepare_artifact_root(&artifacts)?;
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
            last_input: std::sync::atomic::AtomicBool::new(false),
            clock: Instant::now(),
            previews: Mutex::new(BTreeMap::new()),
            preview_generation: std::sync::atomic::AtomicU64::new(0),
            topology: std::sync::atomic::AtomicU64::new(0),
            armed: Mutex::new(None),
        })
    }
    fn set_presence_epoch(&mut self, epoch: Arc<dyn Fn(&str) -> u64 + Send + Sync>) {
        Arc::get_mut(&mut self.controller)
            .expect("unshared computer controller")
            .set_presence_epoch(epoch.clone());
        self.presence_epoch = epoch;
    }
    fn checked_health(&self, prompt: bool) -> Result<Health> {
        let health = self.backend.health(prompt)?.normalize();
        let had_input = self
            .last_input
            .swap(health.input.available, std::sync::atomic::Ordering::AcqRel);
        if !health.capture.available || (had_input && !health.input.available) {
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
        self.admission_with(&health, session, target, permitted, capture)
    }
    /// Admission requires capture for every action and input for input actions.
    fn admission_with<'a>(
        &self,
        health: &Health,
        session: &'a str,
        target: &'a Target,
        permitted: bool,
        capture: bool,
    ) -> Result<Admission<'a>> {
        self.require_enabled()?;
        if !health.supported {
            return Err(cyber_computer::Error::UnsupportedPlatform.into());
        }
        Ok(Admission {
            session,
            now: self.now(),
            human_present: (self.attendance)(session),
            permitted,
            os_consent: health.capture.available && (capture || health.input.available),
            target,
        })
    }
    pub fn status(&self, session: &str) -> Result<Status> {
        let enabled = self.enabled();
        Ok(Status {
            schema_version: SCHEMA_VERSION,
            enabled,
            health: reported(self.checked_health(false)?, enabled),
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
        let health = self.checked_health(false)?;
        if !health.supported {
            return Err(cyber_computer::Error::UnsupportedPlatform.into());
        }
        let kind = selector.validate_for(&health.selectors)?;
        let generation = self
            .preview_generation
            .load(std::sync::atomic::Ordering::Acquire);
        let cancelled = || {
            self.preview_generation
                .load(std::sync::atomic::Ordering::Acquire)
                != generation
        };
        let target = self.stamp(self.backend.review(&selector, &cancelled)?, None);
        let scoped = target.clone();
        // A review that is not registered has its platform scope torn down at once.
        self.register_review(session, selector, kind, &health, target)
            .inspect_err(|_| self.backend.end_scope(&scoped))
    }
    fn register_review(
        &self,
        session: &str,
        selector: Selector,
        kind: cyber_computer::capability::SelectorKind,
        health: &Health,
        target: NativeTarget,
    ) -> Result<Preview> {
        let scope = target.target();
        let facts = self.admission_with(health, session, &scope, true, true)?;
        if !facts.os_consent {
            return Err(
                cyber_computer::Error::Consent(cyber_computer::consent::SCREEN_RECORDING).into(),
            );
        }
        if !(self.attendance)(session) {
            return Err(cyber_computer::Error::Consent(
                "attach a terminal or browser to this Session before arming",
            )
            .into());
        }
        let preview = Preview { schema_version:SCHEMA_VERSION, evidence:health.evidence, re_resolvable:kind.re_resolvable(), id:cyber_core::ids::new_id("cup"), session_id:session.into(), target, expires_at:wall_ms()+30_000, unknown_operations:self.journal.unknown_all()?, warning:"Native capture supplies visible application/display content to the selected model. Input controls the host application outside the workspace filesystem sandbox. Review this exact scope; Stop revokes future input, not already delivered events.".into() };
        let mut previews = self.previews.lock().map_err(|_| ComputerError::Storage)?;
        let now = Instant::now();
        let (live, expired): (BTreeMap<_, _>, BTreeMap<_, _>) = std::mem::take(&mut *previews)
            .into_iter()
            .partition(|(_, value)| value.deadline > now);
        *previews = live;
        let full = previews.len() >= 16;
        if !full {
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
        }
        drop(previews);
        // An unconfirmed review past its deadline has its platform scope torn down.
        self.end_scopes(expired.into_values().map(|review| review.preview.target));
        if full {
            return Err(cyber_computer::Error::Limit("too many pending target reviews").into());
        }
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
        let scoped = reviewed.preview.target.clone();
        // A consumed review that does not arm has its platform scope torn down.
        self.arm_reviewed(session, reviewed, ttl_seconds, acknowledge_unknown)
            .inspect_err(|_| self.backend.end_scope(&scoped))
    }
    fn arm_reviewed(
        &self,
        session: &str,
        reviewed: Reviewed,
        ttl_seconds: Option<u64>,
        acknowledge_unknown: bool,
    ) -> Result<GrantView> {
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
        // `resolve` never shows UI: a pick or candidate selector re-checks only the
        // scope its review created, and the live binding must match what was shown.
        let current = self.backend.resolve(&reviewed.selector)?;
        if !current.same_binding(&reviewed.preview.target) {
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
        *self.armed.lock().map_err(|_| ComputerError::Storage)? = Some((
            grant.id.clone(),
            session.into(),
            reviewed.preview.target.clone(),
        ));
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
        let reviews = self
            .previews
            .lock()
            .map(|mut previews| {
                let (gone, kept) = std::mem::take(&mut *previews)
                    .into_iter()
                    .partition(|(_, value)| value.preview.session_id == session);
                *previews = kept;
                gone
            })
            .unwrap_or_else(|_| BTreeMap::new());
        self.end_scopes(reviews.into_values().map(|review| review.preview.target));
        let armed = self.armed.lock().ok().and_then(|mut armed| {
            let owned = armed.as_ref().is_some_and(|armed| armed.1 == session);
            if owned { armed.take() } else { None }
        });
        self.end_scopes(armed.map(|(_, _, target)| target));
    }
    /// Adds the server-issued references to a backend resolution. `reviewed` carries
    /// the references of the armed review; a fresh review gets a new `target_id`.
    fn stamp(&self, mut target: NativeTarget, reviewed: Option<&NativeTarget>) -> NativeTarget {
        match reviewed {
            Some(reviewed) => {
                target.target_id = reviewed.target_id.clone();
                target.source = reviewed.source.clone();
            }
            None => target.target_id = cyber_core::ids::new_id("cut"),
        }
        if target.source.is_none() {
            target.source = Some(target.described_source());
        }
        target.topology_epoch = self.topology.load(std::sync::atomic::Ordering::Acquire);
        target
    }
    fn armed_target(&self, grant: &str) -> Option<NativeTarget> {
        self.armed
            .lock()
            .ok()?
            .as_ref()
            .filter(|(id, _, _)| id == grant)
            .map(|(_, _, target)| target.clone())
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
        let reviewed = self.armed_target(&grant.id);
        let target = self.stamp(self.backend.resolve(&selector)?, reviewed.as_ref());
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
        let started_ms = wall_ms();
        self.journal.begin(
            session,
            operation,
            "screenshot",
            &fingerprint,
            json!({"action":"screenshot","grant":grant.id,"target":scope}),
            started_ms,
        )?;
        // Once capture begins, every outcome gets a durable settlement. Cancellation
        // or private artifact failures must never leave a replayable native request.
        let captured = (|| -> Result<FrameInfo> {
            if permit.cancelled() {
                return Err(cyber_computer::Error::NotArmed.into());
            }
            let mut capture = self
                .backend
                .capture(&target)
                .inspect_err(|_| self.stop(session))?;
            capture.validate()?;
            if !capture.target.same_binding(&target) {
                return Err(cyber_computer::Error::StaleFrame.into());
            }
            capture.target = target.clone();
            let frame = self
                .controller
                .frame(&permit, self.now(), capture.target.geometry()?)?;
            self.store_frame(frame, grant.id.clone(), capture)
        })();
        let scope = ReceiptScope {
            operation,
            action: "screenshot".into(),
            grant_id: &grant.id,
            target: &target,
            started_ms,
        };
        let receipt = match captured {
            Ok(frame) => Receipt {
                frame_id: Some(frame.id.clone()),
                frame: Some(frame),
                ..scope.receipt("captured")
            },
            Err(_) if permit.cancelled() => scope.receipt("cancelled"),
            Err(error) => Receipt {
                reason: Some(error.reason().unwrap_or(ReasonCode::CaptureUnavailable)),
                ..scope.receipt("unknown")
            },
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
        let (frame_info, target) = self.journal.read_frame(
            session,
            frame_id,
            wall_ms(),
            (self.policy)().retention_days,
        )?;
        // Policy refusals happen before admission, journal intent and native calls,
        // and leave the grant armed (F8c).
        let health = self.checked_health(false)?;
        self.refuse_before_admission(&health, &target, &action)?;
        let _host_lease = self.worker_lease(session)?;
        let current = self.backend.resolve(&target.selector())?;
        if !current.same_binding(&target) {
            return Err(cyber_computer::Error::StaleFrame.into());
        }
        let scope = current.target();
        let kind = action.kind().to_owned();
        let permit = self.controller.input(
            self.admission_with(&health, session, &scope, permitted, false)?,
            frame_id,
            action,
            &current.geometry()?,
        )?;
        let audit = json!({"frame_id":frame_id,"input":audit["input"],"grant_id":frame_info.grant_id,"target":target.for_journal(),"permitted":permitted});
        let started_ms = wall_ms();
        self.journal
            .begin(session, operation, &kind, &fingerprint, audit, started_ms)?;
        let scope = ReceiptScope {
            operation,
            action: kind,
            grant_id: &frame_info.grant_id,
            target: &target,
            started_ms,
        };
        let receipt = Receipt {
            frame_id: Some(frame_info.id.clone()),
            ..self.dispatch(session, &scope, &target, &permit)
        };
        if self.journal.settle(&receipt, wall_ms()).is_err() {
            self.stop(session);
            return Err(ComputerError::Unknown);
        }
        Ok(receipt)
    }
    /// Unsupported actions, chords and stale topology are refused with a reason code
    /// before admission. A capture-only host refuses input here unless the reason is a
    /// missing OS permission, which admission handles as it always has (and revokes).
    fn refuse_before_admission(
        &self,
        health: &Health,
        target: &NativeTarget,
        action: &Action,
    ) -> Result<()> {
        if !health.input.available
            && !health
                .input
                .reasons
                .contains(&ReasonCode::OsPermissionMissing)
        {
            let code = health
                .input
                .reasons
                .first()
                .copied()
                .unwrap_or(ReasonCode::ActionUnsupportedOnPlatform);
            return Err(cyber_computer::Error::Refused(
                code,
                "this host cannot perform input actions in its current mode",
            )
            .into());
        }
        if target.topology_epoch != self.topology.load(std::sync::atomic::Ordering::Acquire) {
            return Err(cyber_computer::Error::Refused(
                ReasonCode::FrameStale,
                "the display layout changed; capture a fresh screenshot",
            )
            .into());
        }
        if let Action::Key { chord } = action {
            let source = target.described_source();
            chord::evaluate(
                chord::Platform::from_name(&source.platform),
                chord::Scope::of(source.kind),
                chord,
            )?;
        }
        Ok(())
    }
    fn dispatch(
        &self,
        session: &str,
        scope: &ReceiptScope<'_>,
        target: &NativeTarget,
        permit: &cyber_computer::control::InputPermit,
    ) -> Receipt {
        if permit.cancelled() {
            return Receipt {
                cleanup: Some(Cleanup::NotNeeded),
                ..scope.receipt("cancelled")
            };
        }
        let error = match self.backend.input(target, permit) {
            Ok(()) => {
                return Receipt {
                    cleanup: Some(Cleanup::NotNeeded),
                    ..scope.receipt("posted")
                };
            }
            Err(error) => error,
        };
        self.stop(session);
        let events = match error {
            cyber_computer::Error::PartialInput { requested, posted } => {
                Some(EventCounts { requested, posted })
            }
            _ => None,
        };
        Receipt {
            reason: Some(error.reason().unwrap_or(ReasonCode::InputUnavailable)),
            events,
            cleanup: Some(Cleanup::Uncertain),
            ..scope.receipt("unknown")
        }
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
            return Ok(Selector::app(app));
        }
        if let Some(display) = identity
            .strip_prefix("display:")
            .and_then(|value| value.parse().ok())
        {
            return Ok(Selector::display(display));
        }
        Err(ComputerError::Invalid("native target identity is invalid"))
    }
}

/// Health as clients see it: a disabled feature makes both capabilities unavailable.
fn reported(health: Health, enabled: bool) -> Health {
    if enabled { health } else { health.disabled() }
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
