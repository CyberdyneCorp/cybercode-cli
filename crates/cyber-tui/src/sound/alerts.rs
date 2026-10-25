//! Alert dispatch (`tui` → Sound alerts, Needs-input attention): every source of an
//! alert goes through [`App::alert`], which sends the attention notification of
//! `tui.notifications` and plays the event's sound, ringing the bell only when no sound
//! covers the alert.

use std::collections::VecDeque;

use super::{Alert, When};
use crate::app::App;
use crate::osc::BEL;

/// How many request IDs are remembered as alerted.
const SEEN: usize = 512;

/// Requests already alerted for in this TUI process, newest last.
#[derive(Debug, Default)]
pub struct Seen(VecDeque<String>);

impl Seen {
    /// Whether `id` is new; it is remembered either way.
    pub fn first(&mut self, id: &str) -> bool {
        if self.0.iter().any(|seen| seen == id) {
            return false;
        }
        if self.0.len() == SEEN {
            self.0.pop_front();
        }
        self.0.push_back(id.to_owned());
        true
    }
}

impl App {
    /// An alert: the attention notification while the terminal is unfocused (for
    /// `input`, also whenever `tui.sounds.when` is `always`), and the event's sound.
    pub(crate) fn alert(&mut self, event: Alert, text: &str) {
        let always = self.sounds.settings().when == When::Always;
        let notify = !self.focused || (event == Alert::Input && always);
        self.alert_with(event, text, notify);
    }

    /// [`App::alert`] with the caller deciding whether to notify. When a sound covers
    /// the alert the notification leaves out its bell; when the bell stands in for the
    /// sound it rings once ([`crate::osc::Notifications::alert_sequence`]).
    pub(crate) fn alert_with(&mut self, event: Alert, text: &str, notify: bool) {
        let now = self.now();
        let outcome = self.sounds.fire(event, self.focused, now);
        let text = notify.then_some(text);
        if let Some(sequence) = self.notifications.alert_sequence(text, outcome) {
            self.write_terminal(&sequence);
        }
        self.sound_warnings();
    }

    /// A permission or question request this TUI has not alerted for: `cyber: <session
    /// name> needs input`, once per request ID, also when a reconnect shows it again.
    pub(crate) fn alert_requests(&mut self) {
        let fresh: Vec<String> = self
            .requests
            .iter()
            .filter(|request| request.visible_in(&self.session.id))
            .filter(|request| self.input_alerts.first(request.id()))
            .map(|request| {
                let name = request.origin_title().unwrap_or(&self.session.title);
                crate::sidebar::clean(name)
            })
            .collect();
        for name in fresh {
            let name = if name.is_empty() {
                "the session".to_owned()
            } else {
                name
            };
            self.alert(Alert::Input, &format!("cyber: {name} needs input"));
        }
    }

    /// Start the sound the last events chose; the runner calls this after each batch,
    /// so a burst in one batch plays its most important sound.
    pub(crate) fn flush_sounds(&mut self) {
        let now = self.now();
        self.sounds.flush(now);
    }

    /// A player that failed since the last tick rings the bell for its alert
    /// ([`crate::sound::Sounds::poll`] decides what later alerts do).
    pub(crate) fn poll_sounds(&mut self) {
        if self.sounds.poll() {
            self.write_terminal(BEL);
        }
        self.sound_warnings();
    }

    fn sound_warnings(&mut self) {
        if let Some(warning) = self.sounds.warnings.pop() {
            self.toast(warning);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use serde_json::json;

    use super::*;
    use crate::model::Request;
    use crate::osc::Notifications;
    use crate::sound::player::Recorder;
    use crate::sound::{BellReason, Settings, Sounds, THROTTLE};

    fn with_recorder(app: &mut App, cache: &std::path::Path) -> Recorder {
        let recorder = Recorder::default();
        app.sounds = Sounds::recording(Settings::default(), cache, &recorder);
        recorder
    }

    fn app() -> App {
        let mut app = App::new(Default::default(), Vec::new(), "cyber");
        app.session.id = "ses_a".into();
        app.session.title = "fix tests".into();
        app.clock = Some(app.started());
        app
    }

    fn question(id: &str, session: &str) -> Request {
        Request::Question {
            id: id.into(),
            session_id: session.into(),
            origin: None,
            member: None,
            routed_to: Vec::new(),
            questions: Vec::new(),
        }
    }

    #[test]
    fn a_question_request_alerts_once_and_a_focused_terminal_stays_quiet() {
        let cache = tempfile::tempdir().unwrap();
        let mut app = app();
        let recorder = with_recorder(&mut app, cache.path());
        app.focused = false;
        app.requests = vec![question("q1", "ses_a"), question("q2", "ses_other")];
        app.alert_requests();
        app.flush_sounds();
        app.alert_requests();
        app.flush_sounds();
        assert_eq!(
            app.attention_log,
            ["\x1b]9;cyber: fix tests needs input\x07"]
        );
        assert_eq!(recorder.played().len(), 1);
        assert!(recorder.played()[0].ends_with("cyber-input-v1-60.wav"));

        // Focused: remembered, but no notification and no sound.
        app.focused = true;
        app.clock = Some(app.started() + Duration::from_secs(5));
        app.requests.push(question("q3", "ses_a"));
        app.alert_requests();
        app.flush_sounds();
        assert_eq!(app.attention_log.len(), 1);
        assert_eq!(recorder.played().len(), 1);
        app.focused = false;
        app.alert_requests();
        assert_eq!(app.attention_log.len(), 1, "q3 was seen while focused");
    }

    #[test]
    fn input_follows_when_always_even_while_focused() {
        let cache = tempfile::tempdir().unwrap();
        let mut app = app();
        let recorder = with_recorder(&mut app, cache.path());
        app.sounds.chosen.when = When::Always;
        app.requests = vec![question("q1", "ses_a")];
        app.alert_requests();
        app.flush_sounds();
        assert_eq!(
            app.attention_log,
            ["\x1b]9;cyber: fix tests needs input\x07"]
        );
        assert_eq!(recorder.played().len(), 1);
        // Other events keep the focus rule for the notification but still sound.
        app.clock = Some(app.started() + Duration::from_secs(5));
        app.alert(Alert::Done, "cyber: the session is idle");
        app.flush_sounds();
        assert_eq!(app.attention_log.len(), 1);
        assert_eq!(recorder.played().len(), 2);
    }

    #[test]
    fn the_bell_stands_in_once_whatever_the_notification_setting() {
        let mut app = app();
        app.focused = false;
        // The default controller has no player: the bell.
        app.alert(Alert::Done, "idle");
        assert_eq!(app.attention_log, ["\x07\x1b]9;idle\x07"], "one BEL");
        app.notifications = Notifications::Osc9;
        app.attention_log.clear();
        app.alert(Alert::Done, "idle");
        assert_eq!(app.attention_log, ["\x07\x1b]9;idle\x07"]);
        app.notifications = Notifications::Off;
        app.attention_log.clear();
        app.alert(Alert::Done, "idle");
        assert_eq!(app.attention_log, ["\x07"], "sounds are their own channel");
        app.sounds.chosen.enabled = false;
        app.attention_log.clear();
        app.alert(Alert::Done, "idle");
        assert!(app.attention_log.is_empty());
    }

    /// A player that cannot start: the bell rings for that alert, the failure is logged
    /// once, and later alerts ring the bell without trying the player again.
    #[test]
    fn a_spawn_error_chooses_the_bell_and_is_logged_once() {
        let cache = tempfile::tempdir().unwrap();
        let mut app = app();
        app.sounds = Sounds::recording(Settings::default(), cache.path(), &Recorder::failing());
        app.focused = false;
        app.alert(Alert::Error, "cyber: error: boom");
        app.flush_sounds();
        assert_eq!(app.attention_log, ["\x1b]9;cyber: error: boom\x07"]);
        app.poll_sounds();
        assert_eq!(app.attention_log.last().unwrap(), "\x07");
        assert_eq!(app.sounds.logged(), [BellReason::Failed]);
        assert_eq!(app.sounds.status(), "bell only: the player failed");
        app.attention_log.clear();
        app.clock = Some(app.started() + Duration::from_secs(5));
        app.alert(Alert::Error, "cyber: error: again");
        app.poll_sounds();
        assert_eq!(app.attention_log, ["\x07\x1b]9;cyber: error: again\x07"]);
        assert_eq!(app.sounds.logged(), [BellReason::Failed]);
    }

    /// A custom file that went missing after the configuration loaded plays the pack's
    /// sound, with one warning.
    #[test]
    fn a_missing_custom_file_falls_back_with_one_warning() {
        let cache = tempfile::tempdir().unwrap();
        let mut app = app();
        let recorder = with_recorder(&mut app, cache.path());
        let gone = cache.path().join("gone.wav");
        *app.sounds.chosen.event_mut(Alert::Message) = super::super::Choice::File(gone);
        app.focused = false;
        app.on_event(
            "messaging.message.held.1",
            &json!({ "direction": "in", "from": "review", "message_id": "m1" }),
        );
        app.flush_sounds();
        assert!(recorder.played()[0].ends_with("cyber-message-v1-60.wav"));
        assert!(app.toast.as_ref().unwrap().0.contains("gone.wav"));
        assert_eq!(
            app.attention_log,
            ["\x1b]9;cyber: message held from review\x07"]
        );
        app.toast = None;
        app.clock = Some(app.started() + Duration::from_secs(5));
        app.on_event(
            "messaging.message.delivered.1",
            &json!({ "direction": "in", "from": "review", "message_id": "m2" }),
        );
        app.flush_sounds();
        assert!(app.toast.is_none(), "warned once");
        assert_eq!(recorder.played().len(), 2);
        // A sent message is not an alert.
        app.on_event(
            "messaging.message.delivered.1",
            &json!({ "direction": "out", "to": "review", "message_id": "m3" }),
        );
        assert_eq!(app.attention_log.len(), 2);
    }

    #[test]
    fn a_session_error_plays_the_error_sound() {
        let cache = tempfile::tempdir().unwrap();
        let mut app = app();
        let recorder = with_recorder(&mut app, cache.path());
        app.focused = false;
        app.on_event("session.error", &json!({ "message": "rate limited" }));
        app.flush_sounds();
        assert_eq!(app.attention_log, ["\x1b]9;cyber: error: rate limited\x07"]);
        assert!(recorder.played()[0].ends_with("cyber-error-v1-60.wav"));
    }

    /// Regression: a needs-input alert soon after a lower sound started used to be
    /// dropped by the throttle while its notification still left the BEL out, so with
    /// `tui.notifications: bell` nothing at all was heard. It now cuts the lower sound
    /// short and plays.
    #[test]
    fn input_soon_after_a_message_sound_still_sounds() {
        let cache = tempfile::tempdir().unwrap();
        let mut app = app();
        let recorder = with_recorder(&mut app, cache.path());
        app.notifications = Notifications::Bell;
        app.focused = false;
        app.alert(Alert::Message, "cyber: message from review");
        app.flush_sounds();
        app.clock = Some(app.started() + Duration::from_millis(200));
        app.requests = vec![question("q1", "ses_a")];
        app.alert_requests();
        app.flush_sounds();
        let played: Vec<_> = recorder
            .played()
            .iter()
            .map(|path| path.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(played, ["cyber-message-v1-60.wav", "cyber-input-v1-60.wav"]);
        assert!(app.attention_log.is_empty(), "the sounds cover both");
    }

    /// Regression: one failing exit of the player (a busy device) used to switch every
    /// later alert to the bell until restart. The bell rings for that alert only.
    #[test]
    fn a_failing_player_exit_keeps_the_player() {
        let cache = tempfile::tempdir().unwrap();
        let mut app = app();
        let recorder = Recorder::exiting_with_error();
        app.sounds = Sounds::recording(Settings::default(), cache.path(), &recorder);
        app.focused = false;
        app.alert(Alert::Error, "cyber: error: boom");
        app.flush_sounds();
        app.poll_sounds();
        assert_eq!(app.attention_log, ["\x1b]9;cyber: error: boom\x07", "\x07"]);
        assert_eq!(app.sounds.status(), "playing through afplay");
        app.clock = Some(app.started() + Duration::from_secs(5));
        app.alert(Alert::Done, "cyber: the session is idle");
        app.flush_sounds();
        assert_eq!(recorder.played().len(), 2, "the next alert still plays");
        assert!(app.sounds.logged().is_empty());
    }

    /// Regression: a custom file the player cannot decode used to disable every pack
    /// sound. Its event falls back to the pack sound with one warning; the others play.
    #[test]
    fn a_custom_file_the_player_fails_on_falls_back_to_the_pack_sound() {
        let cache = tempfile::tempdir().unwrap();
        let file = cache.path().join("done.mp3");
        std::fs::write(&file, b"ID3").unwrap();
        let mut app = app();
        let recorder = Recorder::exiting_with_error();
        let mut chosen = Settings::default();
        *chosen.event_mut(Alert::Done) = super::super::Choice::File(file.clone());
        app.sounds = Sounds::recording(chosen, cache.path(), &recorder);
        app.focused = false;
        app.alert(Alert::Done, "idle");
        app.flush_sounds();
        app.poll_sounds();
        assert!(app.toast.as_ref().unwrap().0.contains("done.mp3"));
        app.toast = None;
        for (i, event) in [Alert::Done, Alert::Done, Alert::Error]
            .into_iter()
            .enumerate()
        {
            app.clock = Some(app.started() + THROTTLE * (i as u32 + 1));
            app.alert(event, "again");
            app.flush_sounds();
            app.poll_sounds();
        }
        assert!(app.toast.is_none(), "warned once");
        let played = recorder.played();
        assert_eq!(played[0], file);
        assert!(played[1].ends_with("cyber-done-v1-60.wav"));
        assert!(played[2].ends_with("cyber-done-v1-60.wav"));
        assert!(played[3].ends_with("cyber-error-v1-60.wav"));
    }

    /// Regression: volume 0 played a silent file and still left the BEL out, so the
    /// alert made no sound at all. Nothing plays, and the notification keeps its bell.
    #[test]
    fn volume_zero_plays_nothing_and_the_notification_keeps_its_bell() {
        let cache = tempfile::tempdir().unwrap();
        let mut app = app();
        let recorder = with_recorder(&mut app, cache.path());
        app.sounds.chosen.volume = 0;
        app.focused = false;
        app.alert(Alert::Done, "idle");
        app.flush_sounds();
        assert!(recorder.played().is_empty());
        assert_eq!(app.attention_log, ["\x07\x1b]9;idle\x07"]);
    }

    /// `aplay` and PowerShell play WAV only: an mp3 for them is never handed over.
    #[test]
    fn a_custom_file_the_player_cannot_decode_is_not_tried() {
        let cache = tempfile::tempdir().unwrap();
        let file = cache.path().join("done.mp3");
        std::fs::write(&file, b"ID3").unwrap();
        let recorder = Recorder::default();
        let mut chosen = Settings::default();
        *chosen.event_mut(Alert::Done) = super::super::Choice::File(file);
        let mut app = app();
        app.sounds = Sounds::with_player(
            chosen,
            Default::default(),
            crate::sound::Output::Player {
                kind: crate::sound::player::Kind::Aplay,
                program: "aplay".into(),
            },
            Box::new(recorder.clone()),
            Some(cache.path().to_path_buf()),
        );
        app.focused = false;
        app.alert(Alert::Done, "idle");
        app.flush_sounds();
        assert!(recorder.played()[0].ends_with("cyber-done-v1-60.wav"));
        let toast = &app.toast.as_ref().unwrap().0;
        assert!(toast.contains("aplay cannot play it"), "{toast}");
    }

    #[test]
    fn seen_ids_are_bounded() {
        let mut seen = Seen::default();
        for i in 0..SEEN + 1 {
            assert!(seen.first(&i.to_string()));
        }
        assert!(!seen.first(&SEEN.to_string()));
        assert!(seen.first("0"), "the oldest was forgotten");
    }
}
