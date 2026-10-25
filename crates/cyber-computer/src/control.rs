//! Process-local authority. Owner API authentication, current Mode/permission
//! decisions, OS consent and durable native intents are supplied by the host.
//! No grant/frame type is deserializable from a model or HTTP request.

use std::collections::{BTreeMap, VecDeque};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
};

use serde::Serialize;
use std::time::{Duration, Instant};

type Attendance = dyn Fn(&str) -> bool + Send + Sync;

use crate::{Action, Error, Geometry};

const FRAME_MS: u64 = 10_000;
const MAX_GRANT_MS: u64 = 900_000;
const MAX_ACTIONS: u32 = 300;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, schemars::JsonSchema)]
#[schemars(rename = "ComputerTarget")]
pub struct Target {
    /// Bundle/display identity resolved by the native backend, never by a model.
    pub identity: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ImagePoint, Rect};

    fn target() -> Target {
        Target {
            identity: "app:fixture:42:100:window:7".into(),
        }
    }
    fn geometry() -> Geometry {
        Geometry::new(
            100,
            100,
            Rect {
                x: 0.0,
                y: 0.0,
                width: 100.0,
                height: 100.0,
            },
            0,
        )
        .unwrap()
    }
    fn admission<'a>(target: &'a Target, now: u64) -> Admission<'a> {
        Admission {
            session: "ses_owner",
            now,
            human_present: true,
            permitted: true,
            os_consent: true,
            target,
        }
    }
    fn action() -> Action {
        Action::Move {
            point: ImagePoint { x: 20.0, y: 30.0 },
        }
    }
    fn frame(controller: &Controller, target: &Target, now: u64) -> Frame {
        let permit = controller.capture(admission(target, now)).unwrap();
        controller.frame(&permit, now, geometry()).unwrap()
    }

    #[test]
    fn grants_require_all_current_owner_permission_and_human_facts() {
        let controller = Controller::new(false);
        let target = target();
        assert!(matches!(
            controller.arm(admission(&target, 10), None),
            Err(Error::Disabled)
        ));
        controller.set_enabled(true);
        for fact in 0..3 {
            let mut request = admission(&target, 10);
            match fact {
                0 => request.human_present = false,
                1 => request.permitted = false,
                _ => request.os_consent = false,
            }
            assert!(controller.arm(request, None).is_err());
        }
        assert!(
            controller
                .arm(admission(&target, 10), Some(900001))
                .is_err()
        );
        controller.arm(admission(&target, 10), None).unwrap();
        assert!(matches!(
            controller.arm(admission(&target, 11), None),
            Err(Error::Busy)
        ));
    }

    #[test]
    fn frame_is_bound_to_its_controller_session_target_geometry_and_age() {
        let a = Controller::new(true);
        let b = Controller::new(true);
        let target = target();
        a.arm(admission(&target, 100), None).unwrap();
        b.arm(admission(&target, 100), None).unwrap();
        let permit = a.capture(admission(&target, 100)).unwrap();
        assert!(matches!(
            b.frame(&permit, 100, geometry()),
            Err(Error::NotArmed)
        ));
        let frame = a.frame(&permit, 100, geometry()).unwrap();
        drop(permit);
        let mut child = admission(&target, 101);
        child.session = "ses_child";
        assert!(matches!(
            a.input(child, &frame.id, action(), &geometry()),
            Err(Error::NotArmed)
        ));
        let other = Target {
            identity: "another-window".into(),
        };
        assert!(matches!(
            a.input(admission(&other, 101), &frame.id, action(), &geometry()),
            Err(Error::StaleFrame)
        ));
        assert!(matches!(
            a.input(admission(&target, 10101), &frame.id, action(), &geometry()),
            Err(Error::StaleFrame)
        ));
        let moved = Geometry::new(
            100,
            100,
            Rect {
                x: 1.0,
                y: 0.0,
                width: 100.0,
                height: 100.0,
            },
            0,
        )
        .unwrap();
        assert!(matches!(
            a.input(admission(&target, 101), &frame.id, action(), &moved),
            Err(Error::StaleFrame)
        ));
    }

    #[test]
    fn emergency_stop_does_not_wait_for_a_native_worker_or_transfer_its_lease() {
        let controller = Arc::new(Controller::new(true));
        let target = target();
        controller.arm(admission(&target, 100), None).unwrap();
        let frame = frame(&controller, &target, 100);
        let permit = controller
            .input(admission(&target, 101), &frame.id, action(), &geometry())
            .unwrap();
        let other = Arc::clone(&controller);
        std::thread::spawn(move || other.stop()).join().unwrap();
        assert!(permit.cancelled());
        assert!(controller.current(102).is_none());
        assert!(matches!(
            controller.arm(admission(&target, 102), None),
            Err(Error::Busy)
        ));
        drop(permit);
        controller.arm(admission(&target, 102), None).unwrap();
        assert!(matches!(
            controller.input(admission(&target, 103), &frame.id, action(), &geometry()),
            Err(Error::StaleFrame)
        ));
    }

    #[test]
    fn input_is_rate_limited_and_a_backwards_clock_never_resets_the_window() {
        let controller = Controller::new(true);
        let target = target();
        controller.arm(admission(&target, 100), None).unwrap();
        let frame = frame(&controller, &target, 100);
        for _ in 0..10 {
            drop(
                controller
                    .input(admission(&target, 200), &frame.id, action(), &geometry())
                    .unwrap(),
            );
        }
        assert!(matches!(
            controller.input(admission(&target, 200), &frame.id, action(), &geometry()),
            Err(Error::Limit(_))
        ));
        assert!(matches!(
            controller.input(admission(&target, 199), &frame.id, action(), &geometry()),
            Err(Error::Input(_))
        ));
        drop(
            controller
                .input(admission(&target, 1200), &frame.id, action(), &geometry())
                .unwrap(),
        );
    }

    #[test]
    fn permits_expire_and_lose_human_presence_without_waiting_for_another_call() {
        let present = Arc::new(AtomicBool::new(true));
        let watched = Arc::clone(&present);
        let controller =
            Controller::with_attendance(true, Arc::new(move |_| watched.load(Ordering::Acquire)));
        let target = target();
        controller.arm(admission(&target, 100), Some(1000)).unwrap();
        let permit = controller.capture(admission(&target, 100)).unwrap();
        present.store(false, Ordering::Release);
        assert!(permit.cancelled());
        drop(permit);
        controller.stop();
        present.store(true, Ordering::Release);
        controller.arm(admission(&target, 200), Some(1)).unwrap();
        let permit = controller.capture(admission(&target, 200)).unwrap();
        std::thread::sleep(Duration::from_millis(2));
        assert!(permit.cancelled());
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, schemars::JsonSchema)]
pub struct Grant {
    pub id: String,
    pub session_id: String,
    pub target: Target,
    pub expires_at: u64,
    generation: u64,
    created_at: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, schemars::JsonSchema)]
pub struct Frame {
    pub id: String,
    pub session_id: String,
    pub target: Target,
    pub captured_at: u64,
    pub geometry: Geometry,
    generation: u64,
}

#[derive(Default)]
struct State {
    grant: Option<Grant>,
    frames: BTreeMap<String, Frame>,
    actions: u32,
    recent: VecDeque<u64>,
}

/// Cancellation never waits for the native-operation lock or a model Turn.
pub struct Controller {
    enabled: AtomicBool,
    generation: Arc<AtomicU64>,
    active: Arc<AtomicUsize>,
    state: Mutex<State>,
    attendance: Arc<Attendance>,
}

/// Kept by the native worker until it really exits, including after a caller
/// timeout. A second Session cannot acquire input while the old worker runs.
pub struct CapturePermit {
    generation: Arc<AtomicU64>,
    expected: u64,
    active: Arc<AtomicUsize>,
    grant: Grant,
    deadline: Instant,
    attendance: Arc<Attendance>,
}

impl CapturePermit {
    pub fn cancelled(&self) -> bool {
        self.generation.load(Ordering::Acquire) != self.expected
            || Instant::now() >= self.deadline
            || !(self.attendance)(&self.grant.session_id)
    }
    pub fn target(&self) -> &Target {
        &self.grant.target
    }
    pub fn session(&self) -> &str {
        &self.grant.session_id
    }
}

impl Drop for CapturePermit {
    fn drop(&mut self) {
        self.active.fetch_sub(1, Ordering::AcqRel);
    }
}

pub struct InputPermit {
    capture: CapturePermit,
    action: Action,
    frame: Frame,
}

impl InputPermit {
    pub fn cancelled(&self) -> bool {
        self.capture.cancelled()
    }
    pub fn action(&self) -> &Action {
        &self.action
    }
    pub fn frame(&self) -> &Frame {
        &self.frame
    }
    pub fn target(&self) -> &Target {
        self.capture.target()
    }
}

/// These facts must be computed by the trusted host at admission, not copied
/// from a request body. They do not replace owner authentication or the journal.
pub struct Admission<'a> {
    pub session: &'a str,
    pub now: u64,
    pub human_present: bool,
    pub permitted: bool,
    pub os_consent: bool,
    pub target: &'a Target,
}

impl Controller {
    pub fn new(enabled: bool) -> Self {
        Self::with_attendance(enabled, Arc::new(|_| true))
    }

    pub fn with_attendance(enabled: bool, attendance: Arc<Attendance>) -> Self {
        Self {
            enabled: AtomicBool::new(enabled),
            generation: Arc::new(AtomicU64::new(0)),
            active: Arc::new(AtomicUsize::new(0)),
            state: Mutex::new(State::default()),
            attendance,
        }
    }

    pub fn set_enabled(&self, enabled: bool) {
        self.enabled.store(enabled, Ordering::Release);
        if !enabled {
            self.stop();
        }
    }

    /// Only the authenticated owner path may call this; model tools cannot arm.
    pub fn arm(&self, admission: Admission<'_>, ttl_ms: Option<u64>) -> Result<Grant, Error> {
        self.ready(&admission)?;
        let ttl = ttl_ms.unwrap_or(300_000);
        if ttl == 0
            || ttl > MAX_GRANT_MS
            || admission.session.is_empty()
            || admission.target.identity.is_empty()
        {
            return Err(Error::Input("invalid owner grant scope or lifetime"));
        }
        let expires_at = admission
            .now
            .checked_add(ttl)
            .ok_or(Error::Input("grant lifetime overflows"))?;
        let mut state = self.lock()?;
        if self.active.load(Ordering::Acquire) != 0
            || state
                .grant
                .as_ref()
                .is_some_and(|grant| self.live(grant, admission.now))
        {
            return Err(Error::Busy);
        }
        let generation = self.generation.fetch_add(1, Ordering::AcqRel) + 1;
        let grant = Grant {
            id: format!("cug_{}", ulid::Ulid::new()),
            session_id: admission.session.into(),
            target: admission.target.clone(),
            expires_at,
            generation,
            created_at: admission.now,
        };
        *state = State {
            grant: Some(grant.clone()),
            ..State::default()
        };
        Ok(grant)
    }

    pub fn busy(&self) -> bool {
        self.active.load(Ordering::Acquire) != 0
    }

    pub fn stop(&self) {
        self.generation.fetch_add(1, Ordering::AcqRel);
    }

    pub fn current(&self, now: u64) -> Option<Grant> {
        self.lock()
            .ok()?
            .grant
            .as_ref()
            .filter(|grant| self.live(grant, now))
            .cloned()
    }

    pub fn stop_session(&self, session: &str) {
        if let Ok(state) = self.state.lock()
            && state
                .grant
                .as_ref()
                .is_some_and(|grant| grant.session_id == session)
        {
            self.stop();
        }
    }

    pub fn capture(&self, admission: Admission<'_>) -> Result<CapturePermit, Error> {
        self.ready(&admission)?;
        let state = self.lock()?;
        let grant = self.authorized(&state, &admission)?.clone();
        if self.active.load(Ordering::Acquire) != 0 {
            return Err(Error::Busy);
        }
        Ok(self.permit(grant, admission.now))
    }

    /// Called only after the native backend captures under a live permit. Check
    /// generation again so a late capture cannot populate a replacement grant.
    pub fn frame(
        &self,
        permit: &CapturePermit,
        now: u64,
        geometry: Geometry,
    ) -> Result<Frame, Error> {
        let mut state = self.lock()?;
        if !Arc::ptr_eq(&self.generation, &permit.generation)
            || permit.cancelled()
            || !self.live(&permit.grant, now)
            || !state
                .grant
                .as_ref()
                .is_some_and(|grant| grant.id == permit.grant.id)
        {
            return Err(Error::NotArmed);
        }
        state.frames.retain(|_, frame| {
            now.checked_sub(frame.captured_at)
                .is_some_and(|age| age <= FRAME_MS)
        });
        if state.frames.len() >= 16 {
            return Err(Error::Limit("at most 16 fresh frames per grant"));
        }
        let frame = Frame {
            id: format!("cuf_{}", ulid::Ulid::new()),
            session_id: permit.grant.session_id.clone(),
            target: permit.grant.target.clone(),
            captured_at: now,
            geometry,
            generation: permit.expected,
        };
        state.frames.insert(frame.id.clone(), frame.clone());
        Ok(frame)
    }

    pub fn input(
        &self,
        admission: Admission<'_>,
        frame_id: &str,
        action: Action,
        current_geometry: &Geometry,
    ) -> Result<InputPermit, Error> {
        self.ready(&admission)?;
        let mut state = self.lock()?;
        let grant = self.authorized(&state, &admission)?.clone();
        if self.active.load(Ordering::Acquire) != 0 {
            return Err(Error::Busy);
        }
        let frame = state.frames.get(frame_id).ok_or(Error::StaleFrame)?.clone();
        self.check_frame(&frame, &grant, admission.now, current_geometry)?;
        action.validate(&frame.geometry)?;
        Self::rate(&mut state, admission.now)?;
        Ok(InputPermit {
            capture: self.permit(grant, admission.now),
            frame,
            action,
        })
    }

    fn ready(&self, admission: &Admission<'_>) -> Result<(), Error> {
        if !self.enabled.load(Ordering::Acquire) {
            return Err(Error::Disabled);
        }
        if !admission.human_present || !(self.attendance)(admission.session) {
            return Err(Error::Consent("an attached human is required"));
        }
        if !admission.permitted {
            return Err(Error::Consent(
                "current Mode or permission rules deny this action",
            ));
        }
        if !admission.os_consent {
            return Err(Error::Consent("native OS consent is required"));
        }
        Ok(())
    }

    fn live(&self, grant: &Grant, now: u64) -> bool {
        self.enabled.load(Ordering::Acquire)
            && (self.attendance)(&grant.session_id)
            && grant.generation == self.generation.load(Ordering::Acquire)
            && now >= grant.created_at
            && now < grant.expires_at
    }

    fn authorized<'a>(
        &self,
        state: &'a State,
        admission: &Admission<'_>,
    ) -> Result<&'a Grant, Error> {
        let grant = state.grant.as_ref().ok_or(Error::NotArmed)?;
        if !self.live(grant, admission.now) || grant.session_id != admission.session {
            return Err(Error::NotArmed);
        }
        if &grant.target != admission.target {
            return Err(Error::StaleFrame);
        }
        Ok(grant)
    }

    fn permit(&self, grant: Grant, now: u64) -> CapturePermit {
        self.active.fetch_add(1, Ordering::AcqRel);
        CapturePermit {
            generation: self.generation.clone(),
            expected: grant.generation,
            active: self.active.clone(),
            deadline: Instant::now() + Duration::from_millis(grant.expires_at.saturating_sub(now)),
            attendance: self.attendance.clone(),
            grant,
        }
    }

    fn check_frame(
        &self,
        frame: &Frame,
        grant: &Grant,
        now: u64,
        geometry: &Geometry,
    ) -> Result<(), Error> {
        if frame.generation != grant.generation
            || frame.session_id != grant.session_id
            || frame.target != grant.target
            || &frame.geometry != geometry
            || !now
                .checked_sub(frame.captured_at)
                .is_some_and(|age| age <= FRAME_MS)
        {
            return Err(Error::StaleFrame);
        }
        Ok(())
    }

    fn rate(state: &mut State, now: u64) -> Result<(), Error> {
        if state.actions >= MAX_ACTIONS {
            return Err(Error::Limit("grant input action limit reached (300)"));
        }
        if state.recent.back().is_some_and(|at| *at > now) {
            return Err(Error::Input("monotonic clock moved backwards"));
        }
        state
            .recent
            .retain(|at| now.checked_sub(*at).is_none_or(|age| age < 1000));
        if state.recent.len() >= 10 {
            return Err(Error::Limit("input rate limit reached (10/s)"));
        }
        state.recent.push_back(now);
        state.actions += 1;
        Ok(())
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, State>, Error> {
        self.state
            .lock()
            .map_err(|_| Error::Consent("native control state is unavailable"))
    }
}
