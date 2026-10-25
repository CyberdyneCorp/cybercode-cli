//! Sound alerts (`tui` → Sound alerts, Sound packs): the alert events, the packs and
//! `tui.sounds`, the rendered-file cache, and the controller that decides whether an
//! alert sounds, throttles bursts and hands the file to the platform's player.

pub(crate) mod alerts;
pub(crate) mod config;
pub(crate) mod overlay;
pub(crate) mod player;
pub(crate) mod synth;

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};

pub(crate) use player::{BellReason, Failure, Output, Player};

/// The events that sound, highest priority first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Alert {
    /// A permission or question request is waiting.
    Input,
    /// A Turn failed or `session.error` arrived.
    Error,
    /// A watched workflow run failed or was stopped.
    WorkflowFailed,
    /// The Session finished a Drain, or a notification arrived.
    Done,
    /// A watched workflow run completed.
    Workflow,
    /// A cross-session message was delivered or held.
    Message,
}

impl Alert {
    pub const ALL: [Alert; 6] = [
        Alert::Input,
        Alert::Error,
        Alert::WorkflowFailed,
        Alert::Done,
        Alert::Workflow,
        Alert::Message,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Alert::Input => "input",
            Alert::Error => "error",
            Alert::WorkflowFailed => "workflow_failed",
            Alert::Done => "done",
            Alert::Workflow => "workflow",
            Alert::Message => "message",
        }
    }

    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|alert| alert.name() == name)
    }

    /// What the event means, for the `/sounds` rows.
    pub fn describe(self) -> &'static str {
        match self {
            Alert::Input => "a permission or question is waiting",
            Alert::Error => "a Turn failed",
            Alert::WorkflowFailed => "a workflow run failed or stopped",
            Alert::Done => "the Session is idle",
            Alert::Workflow => "a workflow run completed",
            Alert::Message => "a message arrived or is held",
        }
    }

    fn index(self) -> usize {
        self as usize
    }
}

/// `tui.sounds.pack`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Pack {
    #[default]
    Cyber,
    Soft,
    Retro,
    /// The terminal bell only.
    Bell,
}

impl Pack {
    pub const ALL: [Pack; 4] = [Pack::Cyber, Pack::Soft, Pack::Retro, Pack::Bell];

    pub fn name(self) -> &'static str {
        match self {
            Pack::Cyber => "cyber",
            Pack::Soft => "soft",
            Pack::Retro => "retro",
            Pack::Bell => "bell",
        }
    }

    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|pack| pack.name() == name)
    }
}

/// `tui.sounds.when`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum When {
    /// Only while the terminal reports it is unfocused.
    #[default]
    Unfocused,
    Always,
}

impl When {
    pub const ALL: [When; 2] = [When::Unfocused, When::Always];

    pub fn name(self) -> &'static str {
        match self {
            When::Unfocused => "unfocused",
            When::Always => "always",
        }
    }

    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|when| when.name() == name)
    }
}

/// `tui.sounds.events.<event>`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Choice {
    /// The pack's sound.
    #[default]
    On,
    Off,
    /// Another pack's sound for this event.
    Pack(Pack),
    /// A user file, from the global `tui.jsonc` only.
    File(PathBuf),
}

impl Choice {
    pub fn on(&self) -> bool {
        *self != Choice::Off
    }
}

/// One [`Choice`] per [`Alert`], in its order.
pub type Events = [Choice; Alert::ALL.len()];

/// `tui.sounds`, resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    pub enabled: bool,
    pub pack: Pack,
    /// 0–100.
    pub volume: u8,
    pub when: When,
    pub play_over_ssh: bool,
    /// One per [`Alert`], in its order.
    pub events: Events,
}

pub const DEFAULT_VOLUME: u8 = 60;

impl Default for Settings {
    fn default() -> Self {
        Self {
            enabled: true,
            pack: Pack::Cyber,
            volume: DEFAULT_VOLUME,
            when: When::Unfocused,
            play_over_ssh: false,
            events: Default::default(),
        }
    }
}

impl Settings {
    pub fn event(&self, alert: Alert) -> &Choice {
        &self.events[alert.index()]
    }

    pub fn event_mut(&mut self, alert: Alert) -> &mut Choice {
        &mut self.events[alert.index()]
    }

    /// These settings under a project's restrictions.
    pub fn restricted(&self, limits: &Limits) -> Settings {
        let mut settings = self.clone();
        settings.enabled &= !limits.off;
        if let Some(volume) = limits.volume {
            settings.volume = settings.volume.min(volume);
        }
        for &alert in &limits.events_off {
            *settings.event_mut(alert) = Choice::Off;
        }
        settings
    }
}

/// What a project `tui.jsonc` may do to `tui.sounds`: only turn it down.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Limits {
    /// `sounds.enabled: false`.
    pub off: bool,
    /// The lowest `sounds.volume` a project set.
    pub volume: Option<u8>,
    /// Events a project set to `false`.
    pub events_off: Vec<Alert>,
}

/// The cached file of a pack's sound: `<pack>-<event>-v<SYNTH_VERSION>-<volume>.wav`.
pub fn cache_file(dir: &Path, pack: Pack, event: Alert, volume: u8) -> PathBuf {
    dir.join(format!(
        "{}-{}-v{}-{volume}.wav",
        pack.name(),
        event.name(),
        synth::SYNTH_VERSION
    ))
}

/// The cached file, rendered first when it is missing. The file is written to a
/// temporary name of its own and renamed, so a player never reads half of it; files of
/// other synthesizer versions are removed then, best effort. Two renders of one file
/// (an alert while `/sounds` pre-renders) write the same bytes, so either rename wins.
pub fn ensure(dir: &Path, pack: Pack, event: Alert, volume: u8) -> std::io::Result<PathBuf> {
    static WRITES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let file = cache_file(dir, pack, event, volume);
    if file.is_file() {
        return Ok(file);
    }
    std::fs::create_dir_all(dir)?;
    remove_stale(dir);
    let bytes = synth::wav(&synth::render(pack, event, volume));
    let temporary = dir.join(format!(
        ".{}.{}-{}.tmp",
        file.file_name().and_then(|n| n.to_str()).unwrap_or("sound"),
        std::process::id(),
        WRITES.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    std::fs::write(&temporary, bytes)?;
    match std::fs::rename(&temporary, &file) {
        Ok(()) => Ok(file),
        // Windows refuses to replace a file a player has open: the other render won.
        Err(_) if file.is_file() => {
            let _ = std::fs::remove_file(&temporary);
            Ok(file)
        }
        Err(error) => {
            let _ = std::fs::remove_file(&temporary);
            Err(error)
        }
    }
}

/// Remove rendered files of other synthesizer versions.
fn remove_stale(dir: &Path) {
    let current = format!("-v{}-", synth::SYNTH_VERSION);
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.ends_with(".wav") && !name.contains(&current) {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// What a player is asked to play.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Job {
    /// A user file.
    File(PathBuf),
    /// A pack's sound in the cache directory, rendered when missing.
    Render {
        dir: PathBuf,
        pack: Pack,
        event: Alert,
        volume: u8,
    },
}

impl Job {
    /// The file the player gets.
    #[cfg(test)]
    pub fn path(&self) -> PathBuf {
        match self {
            Job::File(path) => path.clone(),
            Job::Render {
                dir,
                pack,
                event,
                volume,
            } => cache_file(dir, *pack, *event, *volume),
        }
    }

    /// The file, rendered first when needed.
    pub fn prepare(&self) -> std::io::Result<PathBuf> {
        match self {
            Job::File(path) => Ok(path.clone()),
            Job::Render {
                dir,
                pack,
                event,
                volume,
            } => ensure(dir, *pack, *event, *volume),
        }
    }
}

/// What an alert did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// No sound: sounds are off, the event is off, the volume is 0, or the terminal is
    /// focused. The notification keeps its own bell.
    Silent,
    /// A sound plays, or one already covers this alert.
    Sound,
    /// The terminal bell stands in for the sound.
    Bell,
}

/// What an event plays.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Source {
    /// Nothing: the event is off or the volume is 0.
    Off,
    /// The terminal bell.
    Bell,
    Play(Job),
}

/// At most one sound in this window.
pub const THROTTLE: Duration = Duration::from_millis(1500);

/// The sound alert controller: `tui.sounds`, the detected player and the throttle.
pub struct Sounds {
    /// The global `tui.jsonc` with the `/sounds` choices: what `/sounds` edits.
    pub chosen: Settings,
    /// The events as the global `tui.jsonc` alone sets them: what `/sounds` restores
    /// when an event is turned back on, as a restart would.
    pub global_events: Events,
    /// The project restrictions on top.
    pub limits: Limits,
    output: Output,
    player: Box<dyn Player>,
    /// Where pack sounds are rendered; `None` plays the bell.
    cache: Option<PathBuf>,
    /// The sound chosen in this loop iteration, started by [`Sounds::flush`].
    pending: Option<(Alert, Job)>,
    /// The last sound started, and when.
    last: Option<(Alert, Instant)>,
    failed_tx: mpsc::Sender<Failure>,
    failed_rx: mpsc::Receiver<Failure>,
    /// Bell reasons already logged.
    logged: Vec<BellReason>,
    /// A failing player exit was logged.
    exit_logged: bool,
    /// Custom files already warned about.
    warned: Vec<PathBuf>,
    /// Custom files the player failed to play: their events use the pack sound.
    unplayable: Vec<PathBuf>,
    /// Warnings for the UI to show.
    pub warnings: Vec<String>,
    /// Alerts the throttle dropped.
    #[cfg(test)]
    pub dropped: u32,
}

impl Default for Sounds {
    fn default() -> Self {
        Self::with_player(
            Settings::default(),
            Limits::default(),
            Output::Bell(BellReason::NoPlayer),
            Box::new(player::Nothing),
            None,
        )
    }
}

impl Sounds {
    pub fn with_player(
        chosen: Settings,
        limits: Limits,
        output: Output,
        player: Box<dyn Player>,
        cache: Option<PathBuf>,
    ) -> Self {
        let (failed_tx, failed_rx) = mpsc::channel();
        Self {
            global_events: chosen.events.clone(),
            chosen,
            limits,
            output,
            player,
            cache,
            pending: None,
            last: None,
            failed_tx,
            failed_rx,
            logged: Vec::new(),
            exit_logged: false,
            warned: Vec::new(),
            unplayable: Vec::new(),
            warnings: Vec::new(),
            #[cfg(test)]
            dropped: 0,
        }
    }

    /// A controller that plays through `recorder` as `afplay` would, rendering pack
    /// sounds under `cache` (tests).
    #[cfg(test)]
    pub fn recording(chosen: Settings, cache: &Path, recorder: &player::Recorder) -> Self {
        Self::with_player(
            chosen,
            Limits::default(),
            Output::Player {
                kind: player::Kind::Afplay,
                program: "afplay".into(),
            },
            Box::new(recorder.clone()),
            Some(cache.to_path_buf()),
        )
    }

    /// The controller for this machine: the player found on `PATH`, or the bell with
    /// its reason. Pack sounds render under `cache`.
    pub fn detect(
        chosen: Settings,
        limits: Limits,
        env: &dyn Fn(&str) -> Option<String>,
        cache: Option<PathBuf>,
    ) -> Self {
        let output = player::output(env, chosen.play_over_ssh, cache.is_some());
        let player: Box<dyn Player> = match &output {
            Output::Player { kind, program } => Box::new(player::Process::new(*kind, program)),
            Output::Bell(_) => Box::new(player::Nothing),
        };
        Self::with_player(chosen, limits, output, player, cache)
    }

    /// The settings in effect.
    pub fn settings(&self) -> Settings {
        self.chosen.restricted(&self.limits)
    }

    /// Why alerts ring the bell instead of playing, if they do.
    pub fn bell_reason(&self) -> Option<BellReason> {
        if self.settings().pack == Pack::Bell {
            return Some(BellReason::Pack);
        }
        self.bell()
    }

    /// `playing through afplay`, or why the bell is used: what `/sounds` shows.
    pub fn status(&self) -> String {
        match (self.bell_reason(), &self.output) {
            (None, Output::Player { kind, .. }) => format!("playing through {}", kind.program()),
            (Some(reason), _) | (None, &Output::Bell(reason)) => {
                format!("bell only: {}", reason.describe())
            }
        }
    }

    /// An alert: whether it sounds, under `tui.sounds` and the focus. A sound waits for
    /// [`Sounds::flush`]. Inside [`THROTTLE`] of the last sound, or while one waits, only
    /// a higher-priority alert gets through, replacing the waiting sound or cutting the
    /// playing one short; any other is dropped, covered by the sound already chosen.
    pub fn fire(&mut self, event: Alert, focused: bool, now: Instant) -> Outcome {
        let settings = self.settings();
        if !settings.enabled || (settings.when == When::Unfocused && focused) {
            return Outcome::Silent;
        }
        match self.source(&settings, event, false) {
            Source::Off => Outcome::Silent,
            Source::Bell => Outcome::Bell,
            Source::Play(job) => self.throttle(event, job, now),
        }
    }

    fn throttle(&mut self, event: Alert, job: Job, now: Instant) -> Outcome {
        let current = match &self.pending {
            Some((waiting, _)) => Some(*waiting),
            None => self
                .last
                .filter(|(_, at)| now < *at + THROTTLE)
                .map(|(playing, _)| playing),
        };
        match current {
            Some(current) if event >= current => self.drop_alert(),
            _ => self.pending = Some((event, job)),
        }
        Outcome::Sound
    }

    fn drop_alert(&mut self) {
        #[cfg(test)]
        {
            self.dropped += 1;
        }
    }

    /// Start the waiting sound, if any.
    pub fn flush(&mut self, now: Instant) {
        let Some((event, job)) = self.pending.take() else {
            return;
        };
        self.last = Some((event, now));
        self.player.play(job, &self.failed_tx);
    }

    /// `/sounds`' preview: the event's sound now, whatever the focus, `when`, the
    /// throttle or the on/off switches say. At volume 0 nothing plays.
    pub fn preview(&mut self, event: Alert) -> Outcome {
        let settings = self.settings();
        match self.source(&settings, event, true) {
            Source::Play(job) => {
                self.player.play(job, &self.failed_tx);
                Outcome::Sound
            }
            Source::Bell => Outcome::Bell,
            Source::Off => Outcome::Silent,
        }
    }

    /// What `event` plays. A custom file is checked again here; a bad one, one this
    /// player cannot play, or one it failed to play gets the pack's sound.
    fn source(&mut self, settings: &Settings, event: Alert, preview: bool) -> Source {
        let pack = match settings.event(event) {
            Choice::Off if !preview => return Source::Off,
            Choice::Pack(pack) => *pack,
            // The bell stands in for every sound.
            Choice::File(_) if self.bell().is_some() => settings.pack,
            Choice::File(path) => match self.playable(path) {
                Ok(()) if settings.volume == 0 => return Source::Off,
                Ok(()) => return Source::Play(Job::File(path.clone())),
                Err(error) => {
                    self.warn_file(path, &error);
                    settings.pack
                }
            },
            _ => settings.pack,
        };
        if pack == Pack::Bell {
            return Source::Bell;
        }
        if let Some(reason) = self.bell() {
            self.log_once(reason, None);
            return Source::Bell;
        }
        if settings.volume == 0 {
            return Source::Off;
        }
        let Some(dir) = self.cache.clone() else {
            return Source::Bell;
        };
        Source::Play(Job::Render {
            dir,
            pack,
            event,
            volume: settings.volume,
        })
    }

    /// Whether the player may be given the custom file `path`.
    fn playable(&self, path: &Path) -> Result<(), String> {
        config::check_file(path)?;
        let Output::Player { kind, .. } = &self.output else {
            return Ok(());
        };
        if !kind.plays(path) || self.unplayable.iter().any(|bad| bad == path) {
            return Err(format!(
                "{}: {} cannot play it",
                path.display(),
                kind.program()
            ));
        }
        Ok(())
    }

    fn bell(&self) -> Option<BellReason> {
        match self.output {
            Output::Bell(reason) => Some(reason),
            Output::Player { .. } => None,
        }
    }

    fn warn_file(&mut self, path: &Path, error: &str) {
        if !self.warned.iter().any(|warned| warned == path) {
            self.warned.push(path.to_path_buf());
            self.warnings
                .push(format!("tui.sounds: {error}; using the pack sound"));
        }
    }

    /// The first bell fallback for each reason is logged, with the player's error.
    fn log_once(&mut self, reason: BellReason, detail: Option<&str>) {
        if self.logged.contains(&reason) {
            return;
        }
        self.logged.push(reason);
        cyber_core::log::info(
            "tui",
            "sound alerts use the terminal bell",
            serde_json::json!({ "reason": reason.describe(), "detail": detail }),
        );
    }

    /// Bell reasons logged so far.
    #[cfg(test)]
    pub fn logged(&self) -> &[BellReason] {
        &self.logged
    }

    /// Whether a player failed since the last call; the bell then stands in for its
    /// sound. A player that cannot start plays no more, and later alerts ring the bell.
    /// A custom file the player could not play gives its event the pack's sound, with
    /// one warning. A pack sound that failed is logged once, and the player is kept.
    pub fn poll(&mut self) -> bool {
        let mut failed = false;
        while let Ok(failure) = self.failed_rx.try_recv() {
            failed = true;
            self.failed(failure);
        }
        failed
    }

    fn failed(&mut self, failure: Failure) {
        match failure {
            Failure::Start(error) => {
                self.output = Output::Bell(BellReason::Failed);
                self.player = Box::new(player::Nothing);
                self.log_once(BellReason::Failed, Some(&error));
            }
            Failure::Exit {
                file: Some(path),
                error,
            } => {
                self.warn_file(&path, &format!("{}: {error}", path.display()));
                self.unplayable.push(path);
            }
            Failure::Exit { file: None, error } if !self.exit_logged => {
                self.exit_logged = true;
                cyber_core::log::info(
                    "tui",
                    "a sound alert failed to play",
                    serde_json::json!({ "detail": error }),
                );
            }
            Failure::Exit { .. } => {}
        }
    }

    /// Render the current pack's six sounds in the background, so the first alert of
    /// each plays at once.
    pub fn prerender(&self) {
        let settings = self.settings();
        let (Some(dir), Output::Player { .. }) = (self.cache.clone(), &self.output) else {
            return;
        };
        if settings.pack == Pack::Bell || settings.volume == 0 {
            return;
        }
        std::thread::spawn(move || {
            for event in Alert::ALL {
                let _ = ensure(&dir, settings.pack, event, settings.volume);
            }
        });
    }

    /// A player process is still running.
    #[cfg(test)]
    pub fn busy(&self) -> bool {
        self.player.busy()
    }

    /// Stop a playing sound: the TUI is exiting.
    pub fn stop(&mut self) {
        self.player.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sound::player::Recorder;

    fn sounds(chosen: Settings) -> (Sounds, Recorder, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let recorder = Recorder::default();
        let sounds = Sounds::recording(chosen, dir.path(), &recorder);
        (sounds, recorder, dir)
    }

    fn names(recorder: &Recorder) -> Vec<String> {
        recorder
            .played()
            .iter()
            .map(|path| path.file_name().unwrap().to_string_lossy().into_owned())
            .collect()
    }

    /// Scenario "Burst coalesces": idle, then a message 200 ms later; only `done`.
    #[test]
    fn a_burst_plays_one_sound() {
        let (mut sounds, recorder, _dir) = sounds(Settings::default());
        let t = Instant::now();
        assert_eq!(sounds.fire(Alert::Done, false, t), Outcome::Sound);
        sounds.flush(t);
        let later = t + Duration::from_millis(200);
        assert_eq!(sounds.fire(Alert::Message, false, later), Outcome::Sound);
        sounds.flush(later);
        assert_eq!(names(&recorder), ["cyber-done-v1-60.wav"]);
        assert_eq!(sounds.dropped, 1);
        // After the window, the next alert plays.
        let after = t + THROTTLE;
        sounds.fire(Alert::Message, false, after);
        sounds.flush(after);
        assert_eq!(names(&recorder).len(), 2);
    }

    #[test]
    fn a_higher_priority_alert_replaces_a_waiting_one() {
        let (mut sounds, recorder, _dir) = sounds(Settings::default());
        let t = Instant::now();
        sounds.fire(Alert::Message, false, t);
        sounds.fire(Alert::Input, false, t);
        sounds.fire(Alert::Done, false, t);
        sounds.flush(t);
        assert_eq!(names(&recorder), ["cyber-input-v1-60.wav"]);
        assert_eq!(sounds.dropped, 1, "done is lower than input");
    }

    /// A higher-priority alert inside the window cuts a playing lower sound short; an
    /// equal or lower one stays dropped.
    #[test]
    fn a_higher_priority_alert_preempts_a_playing_lower_sound() {
        let (mut sounds, recorder, _dir) = sounds(Settings::default());
        let t = Instant::now();
        sounds.fire(Alert::Message, false, t);
        sounds.flush(t);
        let later = t + Duration::from_millis(200);
        assert_eq!(sounds.fire(Alert::Input, false, later), Outcome::Sound);
        sounds.flush(later);
        let again = later + Duration::from_millis(200);
        assert_eq!(sounds.fire(Alert::Error, false, again), Outcome::Sound);
        sounds.flush(again);
        assert_eq!(
            names(&recorder),
            ["cyber-message-v1-60.wav", "cyber-input-v1-60.wav"]
        );
        assert_eq!(sounds.dropped, 1, "error is lower than the playing input");
    }

    #[test]
    fn a_preview_ignores_the_throttle_focus_and_switches() {
        let mut chosen = Settings {
            enabled: false,
            ..Settings::default()
        };
        *chosen.event_mut(Alert::Error) = Choice::Off;
        let (mut sounds, recorder, _dir) = sounds(chosen);
        let t = Instant::now();
        assert_eq!(sounds.fire(Alert::Done, false, t), Outcome::Silent);
        sounds.chosen.enabled = true;
        sounds.fire(Alert::Done, false, t);
        sounds.flush(t);
        assert_eq!(sounds.preview(Alert::Error), Outcome::Sound);
        assert_eq!(sounds.preview(Alert::Error), Outcome::Sound);
        assert_eq!(
            names(&recorder),
            [
                "cyber-done-v1-60.wav",
                "cyber-error-v1-60.wav",
                "cyber-error-v1-60.wav"
            ]
        );
    }

    #[test]
    fn focus_when_and_event_switches_gate_alerts() {
        let (mut sounds, _recorder, _dir) = sounds(Settings::default());
        let t = Instant::now();
        assert_eq!(sounds.fire(Alert::Done, true, t), Outcome::Silent);
        sounds.chosen.when = When::Always;
        assert_eq!(sounds.fire(Alert::Done, true, t), Outcome::Sound);
        *sounds.chosen.event_mut(Alert::Message) = Choice::Off;
        assert_eq!(sounds.fire(Alert::Message, true, t), Outcome::Silent);
        sounds.limits.off = true;
        assert_eq!(sounds.fire(Alert::Input, false, t), Outcome::Silent);
        sounds.limits.off = false;
        sounds.chosen.pack = Pack::Bell;
        assert_eq!(sounds.fire(Alert::Input, false, t), Outcome::Bell);
        *sounds.chosen.event_mut(Alert::Input) = Choice::Pack(Pack::Soft);
        sounds.last = None;
        sounds.pending = None;
        assert_eq!(sounds.fire(Alert::Input, false, t), Outcome::Sound);
    }

    #[test]
    fn the_cache_reuses_files_keys_them_by_volume_and_removes_old_versions() {
        let dir = tempfile::tempdir().unwrap();
        let stale = dir.path().join("cyber-done-v0-60.wav");
        let other = dir.path().join("notes.txt");
        std::fs::write(&stale, b"old").unwrap();
        std::fs::write(&other, b"keep").unwrap();
        let first = ensure(dir.path(), Pack::Cyber, Alert::Done, 60).unwrap();
        assert_eq!(first.file_name().unwrap(), "cyber-done-v1-60.wav");
        assert!(!stale.exists(), "a stale version is removed");
        assert!(other.exists());
        // A second request reuses the file: a marker written into it stays.
        std::fs::write(&first, b"marker").unwrap();
        let again = ensure(dir.path(), Pack::Cyber, Alert::Done, 60).unwrap();
        assert_eq!(again, first);
        assert_eq!(std::fs::read(&again).unwrap(), b"marker");
        // Another volume is another file.
        let quieter = ensure(dir.path(), Pack::Cyber, Alert::Done, 30).unwrap();
        assert_ne!(quieter, first);
        assert_eq!(
            std::fs::read(&quieter).unwrap(),
            synth::wav(&synth::render(Pack::Cyber, Alert::Done, 30))
        );
        let leftovers: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().contains(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");
    }

    /// An alert and the `/sounds` pre-render may write the same file at once.
    #[test]
    fn concurrent_renders_of_one_file_all_succeed() {
        let dir = tempfile::tempdir().unwrap();
        let renders: Vec<_> = (0..4)
            .map(|_| {
                let dir = dir.path().to_path_buf();
                std::thread::spawn(move || ensure(&dir, Pack::Retro, Alert::Input, 60))
            })
            .collect();
        for render in renders {
            assert!(render.join().unwrap().is_ok());
        }
        let names: Vec<String> = std::fs::read_dir(dir.path())
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["retro-input-v1-60.wav"]);
    }

    #[test]
    fn project_limits_only_turn_sounds_down() {
        let chosen = Settings {
            volume: 40,
            ..Settings::default()
        };
        let limits = Limits {
            off: false,
            volume: Some(80),
            events_off: vec![Alert::Message],
        };
        let settings = chosen.restricted(&limits);
        assert_eq!(settings.volume, 40, "a higher project volume is no raise");
        assert_eq!(*settings.event(Alert::Message), Choice::Off);
        let lower = Limits {
            volume: Some(20),
            off: true,
            ..Limits::default()
        };
        let settings = chosen.restricted(&lower);
        assert_eq!((settings.volume, settings.enabled), (20, false));
    }
}
