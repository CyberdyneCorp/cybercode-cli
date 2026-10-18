//! The activity indicator's state (`tui` → Nyan progress indicator): what the Session is
//! working on, whether its progress is known, and the animation frames, capped at
//! [`MAX_FPS`] and only scheduled while the indicator is working, focused and on screen.
//!
//! Turns and compaction feed it from the Session's events. Workflow runs and byte
//! transfers feed it through [`Progress::set_workflow`] and [`Progress::set_transfer`].

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crate::app::{App, Overlay};

/// The most animation frames per second.
pub const MAX_FPS: u64 = 12;
/// The shortest time between two frames: 84 ms gives at most 12 in any second.
pub const FRAME: Duration = Duration::from_millis(1000 / MAX_FPS + 1);
/// Screen readers hear the elapsed time in steps of this many seconds.
pub const SPOKEN_STEP_SECS: u64 = 10;

/// `tui.spinner`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Spinner {
    #[default]
    Nyan,
    NyanTall,
    Braille,
    Dots,
    Off,
}

impl Spinner {
    /// The values `tui.spinner` takes, default first.
    pub const VALUES: &'static [&'static str] = &["nyan", "nyan-tall", "braille", "dots", "off"];

    /// A value the preferences did not validate is the default.
    pub fn parse(value: &str) -> Self {
        match value {
            "nyan-tall" => Self::NyanTall,
            "braille" => Self::Braille,
            "dots" => Self::Dots,
            "off" => Self::Off,
            _ => Self::Nyan,
        }
    }
}

/// Agents done over total of the workflow runs this Session started.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowProgress {
    /// The run's name, or `3 workflows` for several; untrusted, cleaned when drawn.
    pub label: String,
    pub done: u64,
    pub total: u64,
}

/// Bytes moved of a download or upload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Transfer {
    /// `Downloading model`; untrusted, cleaned when drawn.
    pub label: String,
    pub done: u64,
    pub total: u64,
}

/// What the indicator shows, most specific first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Activity<'a> {
    Workflow(&'a WorkflowProgress),
    Transfer(&'a Transfer),
    /// Compacting the context, since then.
    Compaction(Duration),
    /// A Turn (Drain) running, since then.
    Turn(Duration),
}

impl Activity<'_> {
    /// `(done, total)` when the progress is known.
    pub fn known(self) -> Option<(u64, u64)> {
        match self {
            Self::Workflow(w) if w.total > 0 => Some((w.done.min(w.total), w.total)),
            Self::Transfer(t) if t.total > 0 => Some((t.done.min(t.total), t.total)),
            _ => None,
        }
    }

    /// Known progress as 0–1.
    pub fn fraction(self) -> Option<f64> {
        self.known().map(|(done, total)| done as f64 / total as f64)
    }
}

#[derive(Debug, Default)]
pub struct Progress {
    /// Animation frames drawn while working; the trail scrolls one cell per frame.
    pub frame: u64,
    last_frame: Option<Instant>,
    /// When the running Turn was first seen.
    turn_since: Option<Instant>,
    compaction_since: Option<Instant>,
    workflow: Option<WorkflowProgress>,
    transfer: Option<Transfer>,
    /// The last frame drew the indicator.
    on_screen: AtomicBool,
}

impl Progress {
    /// The workflow-run progress of this Session; `None` when no run is active.
    pub fn set_workflow(&mut self, progress: Option<WorkflowProgress>) {
        self.workflow = progress;
    }

    /// A download or upload's byte progress; `None` when it ended.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "no client transfer reports byte progress yet")
    )]
    pub fn set_transfer(&mut self, transfer: Option<Transfer>) {
        self.transfer = transfer;
    }

    /// Compaction starts and ends with its durable events; an idle Session compacts
    /// nothing.
    pub fn observe(&mut self, kind: &str, now: Instant) {
        if kind.starts_with("session.compaction.started") {
            self.compaction_since.get_or_insert(now);
        } else if kind.starts_with("session.compaction.completed")
            || kind.starts_with("session.compaction.failed")
            || kind == "session.idle"
        {
            self.compaction_since = None;
        }
    }

    /// Another Session is shown.
    pub fn forget_session(&mut self) {
        self.turn_since = None;
        self.compaction_since = None;
        self.workflow = None;
        self.transfer = None;
    }

    pub(crate) fn set_on_screen(&self, shown: bool) {
        self.on_screen.store(shown, Ordering::Relaxed);
    }

    fn on_screen(&self) -> bool {
        self.on_screen.load(Ordering::Relaxed)
    }
}

/// What a running Turn is doing right now, for the label and whether the cat runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TurnPhase {
    /// The model streams or reasons.
    Thinking,
    /// A tool runs: its name and one-line summary (untrusted, cleaned when drawn).
    Running(String),
    /// A permission request (`true`) or a question waits on the user, with what it is
    /// about (untrusted, cleaned when drawn). Nothing animates.
    Waiting { permission: bool, about: String },
}

impl App {
    /// What the running Turn is doing: waiting on the user wins over a running tool,
    /// which wins over thinking.
    pub fn turn_phase(&self) -> TurnPhase {
        if let Some(request) = self.requests.first() {
            return match request {
                crate::model::Request::Permission {
                    action,
                    patterns,
                    resources,
                    ..
                } => TurnPhase::Waiting {
                    permission: true,
                    about: [action.as_str()]
                        .into_iter()
                        .chain(patterns.first().or(resources.first()).map(String::as_str))
                        .collect::<Vec<_>>()
                        .join(" "),
                },
                crate::model::Request::Question { questions, .. } => TurnPhase::Waiting {
                    permission: false,
                    about: questions
                        .first()
                        .map(|q| q.question.clone())
                        .unwrap_or_default(),
                },
            };
        }
        self.running_tool()
            .map_or(TurnPhase::Thinking, TurnPhase::Running)
    }

    /// `bash ps -axo …` for the last tool of the transcript that has not finished.
    fn running_tool(&self) -> Option<String> {
        self.items.iter().rev().find_map(|item| match item {
            crate::model::Item::Assistant { tools, .. } => tools
                .iter()
                .rev()
                .find(|t| matches!(t.display_status(), "running" | "pending"))
                .map(|t| format!("{} {}", t.name, t.summary()).trim_end().to_owned()),
            _ => None,
        })
    }

    /// A permission request or question waits on the user.
    pub fn waiting_on_user(&self) -> bool {
        !self.requests.is_empty()
    }

    /// The Session is compacting (between its durable started and ended events).
    pub fn compacting(&self) -> bool {
        self.progress.compaction_since.is_some()
    }

    /// What the Session is working on, if anything.
    pub fn activity(&self) -> Option<Activity<'_>> {
        let p = &self.progress;
        let now = self.now();
        let since = |at: Instant| now.saturating_duration_since(at);
        if let Some(workflow) = &p.workflow {
            return Some(Activity::Workflow(workflow));
        }
        if let Some(transfer) = &p.transfer {
            return Some(Activity::Transfer(transfer));
        }
        if let Some(at) = p.compaction_since {
            return Some(Activity::Compaction(since(at)));
        }
        self.session
            .running
            .then(|| Activity::Turn(p.turn_since.map(since).unwrap_or_default()))
    }

    pub fn spinner(&self) -> Spinner {
        Spinner::parse(self.prefs.get("spinner"))
    }

    /// Note when a Turn started; called before every frame.
    pub fn progress_sync(&mut self) {
        self.sync_workflow_indicators();
        let now = self.now();
        let p = &mut self.progress;
        if !self.session.running {
            p.turn_since = None;
        } else if p.turn_since.is_none() {
            p.turn_since = Some(now);
        }
        if !self.working() {
            self.progress.last_frame = None;
        }
    }

    /// Something animates: the indicator's activity, or in the Workflow monitor the
    /// selected run while it runs (no other row ever animates).
    fn working(&self) -> bool {
        if matches!(self.overlay, Overlay::Workflows) {
            self.workflows.animated().is_some()
        } else {
            self.activity().is_some()
        }
    }

    /// Frames advance only while working, with motion allowed, the terminal focused (until
    /// it reports otherwise), nothing waiting on the user, and the indicator (or the
    /// monitor's animated bar) on screen.
    pub fn animating(&self) -> bool {
        self.working()
            && self.spinner() != Spinner::Off
            && !self.reduced_motion
            && self.focused
            && !matches!(self.overlay, Overlay::Settings | Overlay::Files)
            && !(self.waiting_on_user() && !matches!(self.overlay, Overlay::Workflows))
            && self.progress.on_screen()
    }

    /// When the next animation frame is due, if one should be drawn at all.
    pub fn next_frame(&self) -> Option<Instant> {
        self.frame_due_at(self.now())
    }

    /// The next frame's time as seen at `now`: right away for the first frame, then one
    /// [`FRAME`] after the last. One clock reading serves both the schedule and the
    /// check, so the first frame is never "due later" than the moment it is checked.
    fn frame_due_at(&self, now: Instant) -> Option<Instant> {
        if !self.animating() {
            return None;
        }
        Some(self.progress.last_frame.map_or(now, |at| at + FRAME))
    }

    /// Advance one frame when it is due; true when it did.
    pub fn advance_frame(&mut self) -> bool {
        let now = self.now();
        if self.frame_due_at(now).is_none_or(|due| now < due) {
            return false;
        }
        self.progress.frame = self.progress.frame.wrapping_add(1);
        self.progress.last_frame = Some(now);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn working() -> App {
        let mut app = App::new(Default::default(), Vec::new(), "cyber");
        app.session.running = true;
        app.clock = Some(app.started());
        app.progress.set_on_screen(true);
        app
    }

    /// Regression: with the real clock the first frame was scheduled from a second clock
    /// reading, later than the one it was checked against, so no frame ever advanced and
    /// the run loop woke continuously.
    #[test]
    fn frames_advance_when_every_clock_read_is_later() {
        let mut app = working();
        app.clock_step = Some(Duration::from_micros(1));
        assert!(app.advance_frame(), "the first frame is due at once");
        assert_eq!(app.progress.frame, 1);
        assert!(!app.advance_frame(), "the next waits a frame interval");
        app.clock = Some(app.started() + FRAME);
        assert!(app.advance_frame());
        assert_eq!(app.progress.frame, 2);
    }

    #[test]
    fn at_most_twelve_frames_in_any_second() {
        let mut app = working();
        let start = app.started();
        let mut frames = Vec::new();
        for ms in 0..5_000 {
            app.clock = Some(start + Duration::from_millis(ms));
            if app.advance_frame() {
                frames.push(ms);
            }
        }
        assert!(frames.len() >= 55, "{}", frames.len());
        for (i, first) in frames.iter().enumerate() {
            let in_second = frames[i..].iter().take_while(|&&at| at < first + 1000);
            assert!(in_second.count() as u64 <= MAX_FPS, "from {first} ms");
        }
    }

    #[test]
    fn no_frames_without_work_motion_focus_or_the_indicator_on_screen() {
        let mut app = working();
        assert!(app.next_frame().is_some());
        app.reduced_motion = true;
        assert_eq!(app.next_frame(), None);
        assert!(!app.advance_frame());
        app.reduced_motion = false;
        app.focused = false;
        assert_eq!(app.next_frame(), None);
        app.focused = true;
        app.overlay = Overlay::Settings;
        assert_eq!(app.next_frame(), None);
        app.overlay = Overlay::None;
        app.progress.set_on_screen(false);
        assert_eq!(app.next_frame(), None);
        app.progress.set_on_screen(true);
        app.prefs.set("spinner", "off", crate::prefs::Origin::Saved);
        assert_eq!(app.next_frame(), None);
        app.prefs
            .set("spinner", "nyan", crate::prefs::Origin::Saved);
        app.session.running = false;
        assert_eq!(app.next_frame(), None);
        assert_eq!(app.progress.frame, 0);
    }

    #[test]
    fn activities_in_order_and_compaction_from_events() {
        let mut app = working();
        let start = app.started();
        app.progress_sync();
        app.clock = Some(start + Duration::from_millis(12_400));
        assert_eq!(
            app.activity(),
            Some(Activity::Turn(Duration::from_millis(12_400)))
        );
        app.progress.observe(
            "session.compaction.started.1",
            start + Duration::from_secs(10),
        );
        assert_eq!(
            app.activity(),
            Some(Activity::Compaction(Duration::from_millis(2_400)))
        );
        app.progress.set_transfer(Some(Transfer {
            label: "Uploading".into(),
            done: 30,
            total: 0,
        }));
        assert_eq!(app.activity().unwrap().known(), None, "no total is unknown");
        app.progress.set_workflow(Some(WorkflowProgress {
            label: "fanout".into(),
            done: 7,
            total: 10,
        }));
        assert_eq!(app.activity().unwrap().fraction(), Some(0.7));
        app.progress.set_workflow(None);
        app.progress.set_transfer(None);
        app.progress
            .observe("session.compaction.completed.1", app.now());
        assert!(matches!(app.activity(), Some(Activity::Turn(_))));
        app.progress
            .observe("session.compaction.started.1", app.now());
        app.progress.observe("session.idle", app.now());
        app.session.running = false;
        app.progress_sync();
        assert_eq!(app.activity(), None);
    }

    #[test]
    fn unknown_spinner_values_are_the_default() {
        for value in Spinner::VALUES {
            assert_eq!(
                Spinner::parse(value) == Spinner::Nyan,
                *value == "nyan",
                "{value}"
            );
        }
        assert_eq!(Spinner::parse("rainbow"), Spinner::Nyan);
    }
}
