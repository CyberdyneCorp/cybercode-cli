//! Sound alerts and `/sounds` (`tui` → Sound alerts, Sound settings command,
//! Needs-input attention) against a production App: requests and Turns from a real
//! server reach the App through the runner's own `apply` and `server_event`, the
//! desktop notifications land in `attention_log`, and a recording player stands in for
//! the platform's one, so no test plays audio. One Unix test runs the production player
//! path against a stub `afplay`/`pw-play` on `PATH`.

use std::path::{Path, PathBuf};
use std::time::Duration;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use futures::StreamExt;
use serde_json::json;

use crate::app::{Action, App, Overlay};
use crate::screens::{Opts, Server, render};
use crate::sound::player::Recorder;
use crate::sound::{Alert, Choice, Pack, Sounds};

fn press(app: &mut App, code: KeyCode) -> Vec<Action> {
    app.on_key(KeyEvent::new(code, KeyModifiers::NONE))
}

fn typed(app: &mut App, text: &str) -> Vec<Action> {
    for c in text.chars() {
        press(app, KeyCode::Char(c));
    }
    press(app, KeyCode::Enter)
}

/// Replace the App's player with a recorder; pack sounds render under `cache`.
fn record(app: &mut App, cache: &Path) -> Recorder {
    let recorder = Recorder::default();
    let limits = app.sounds.limits.clone();
    let global_events = app.sounds.global_events.clone();
    app.sounds = Sounds::recording(app.sounds.chosen.clone(), cache, &recorder);
    app.sounds.limits = limits;
    app.sounds.global_events = global_events;
    recorder
}

fn file_names(recorder: &Recorder) -> Vec<String> {
    recorder
        .played()
        .iter()
        .map(|path| path.file_name().unwrap().to_string_lossy().into_owned())
        .collect()
}

/// Wait until `id` has a pending permission request.
async fn asked(server: &Server, id: &str) {
    for _ in 0..1000 {
        let pending = server
            .client
            .get(&format!("/permissions/requests?session_id={id}"))
            .await
            .unwrap();
        if pending["data"].as_array().is_some_and(|a| !a.is_empty()) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("the write did not ask for permission");
}

/// A fresh snapshot of the App's Session through the runner's `apply`.
async fn refresh(server: &Server, app: &mut App) {
    let msg = crate::perform::snapshot(&server.client, &app.session.id).await;
    crate::runner::apply(app, msg, &mut Default::default());
}

/// Scenario "Approval needed while away", and the regression for "Permission request
/// notifies once": the `input` sound plays once, the OSC 9 notification is written
/// without a BEL, and the request shown again after a reconnect alerts nothing more.
#[tokio::test]
async fn an_approval_while_away_sounds_and_notifies_once_across_a_reconnect() {
    let server = Server::start().await;
    let id = server.session().await;
    let mut app = server.app(&id).await;
    let cache = tempfile::tempdir().unwrap();
    let recorder = record(&mut app, cache.path());
    app.focused = false;
    // In `default` mode the model's `write` asks.
    server.prompt(&id, "WRITE the notes").await;
    asked(&server, &id).await;
    refresh(&server, &mut app).await;
    app.flush_sounds();
    assert!(matches!(app.overlay, Overlay::Permission(_)));
    let name = if app.session.title.is_empty() {
        "the session".to_owned()
    } else {
        app.session.title.clone()
    };
    let notice = format!("\x1b]9;cyber: {name} needs input\x07");
    assert_eq!(app.attention_log, std::slice::from_ref(&notice));
    assert_eq!(file_names(&recorder), ["cyber-input-v1-60.wav"]);

    // The event stream reconnects: `server.connected` refreshes, and the snapshot
    // still holds the request.
    let connected = cyber_client::Event {
        kind: "server.connected".into(),
        data: json!({}),
        seq: None,
        session_id: None,
    };
    let actions = crate::runner::server_event(&mut app, &connected);
    assert!(actions.contains(&Action::Refresh), "{actions:?}");
    refresh(&server, &mut app).await;
    app.clock = Some(app.now() + Duration::from_secs(5));
    app.flush_sounds();
    assert!(!app.requests.is_empty(), "still pending");
    assert_eq!(app.attention_log, [notice]);
    assert_eq!(recorder.played().len(), 1);
}

/// Feed the instance events of a prompt until `id` is idle again.
async fn turn(server: &Server, app: &mut App, id: &str, text: &str) {
    let mut events = Box::pin(server.client.events(true).await.unwrap());
    server.prompt(id, text).await;
    loop {
        let event = tokio::time::timeout(Duration::from_secs(20), events.next())
            .await
            .expect("the Turn ends")
            .unwrap();
        crate::runner::server_event(app, &event);
        app.flush_sounds();
        if event.kind == "session.idle" && event.session_id.as_deref() == Some(id) {
            return;
        }
    }
}

/// Scenario "Focused terminal stays quiet"; the same Turn while unfocused plays `done`.
#[tokio::test]
async fn a_focused_terminal_stays_quiet_when_a_turn_finishes() {
    let server = Server::start().await;
    let id = server.answered_session().await;
    let mut app = server.app(&id).await;
    let cache = tempfile::tempdir().unwrap();
    let recorder = record(&mut app, cache.path());
    turn(&server, &mut app, &id, "again").await;
    assert!(recorder.played().is_empty());
    assert!(app.attention_log.is_empty(), "{:?}", app.attention_log);

    app.focused = false;
    turn(&server, &mut app, &id, "once more").await;
    assert_eq!(file_names(&recorder), ["cyber-done-v1-60.wav"]);
    assert_eq!(app.attention_log, ["\x1b]9;cyber: the session is idle\x07"]);
}

/// An executable stub named `name` in `dir` that records its arguments to `<name>.log`.
fn stub(dir: &Path, name: &str) -> PathBuf {
    let path = dir.join(name);
    let log = dir.join(format!("{name}.log"));
    std::fs::write(
        &path,
        format!("#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\n", log.display()),
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    log
}

/// The players of every OS, as stubs, in one directory.
fn players(dir: &Path) -> Vec<PathBuf> {
    ["afplay", "pw-play", "paplay", "aplay", "powershell.exe"]
        .iter()
        .map(|name| stub(dir, name))
        .collect()
}

fn env_of(vars: Vec<(&'static str, String)>) -> impl Fn(&str) -> Option<String> {
    move |name| {
        vars.iter()
            .find(|(k, _)| *k == name)
            .map(|(_, v)| v.clone())
    }
}

/// Scenario "SSH session uses the bell" and "Fallback shown": no player is started, a
/// BEL is written, and `/sounds` says why.
#[test]
fn over_ssh_the_bell_rings_and_no_player_starts() {
    let bin = tempfile::tempdir().unwrap();
    let logs = players(bin.path());
    let cache = tempfile::tempdir().unwrap();
    let env = env_of(vec![
        ("SSH_CONNECTION", "10.0.0.2 51000 10.0.0.1 22".into()),
        ("PATH", bin.path().display().to_string()),
    ]);
    let mut app = App::new(Default::default(), Vec::new(), "cyber");
    app.sounds = Sounds::detect(
        Default::default(),
        Default::default(),
        &env,
        Some(cache.path().to_path_buf()),
    );
    app.notifications = crate::osc::Notifications::Osc9;
    app.focused = false;
    app.on_event("session.error", &json!({ "message": "boom" }));
    app.flush_sounds();
    assert_eq!(app.attention_log, ["\x07\x1b]9;cyber: error: boom\x07"]);
    std::thread::sleep(Duration::from_millis(100));
    assert!(logs.iter().all(|log| !log.exists()), "no player ran");
    assert_eq!(std::fs::read_dir(cache.path()).unwrap().count(), 0);

    app.open_sounds();
    let shown = render(&mut app, 100, 30, &Opts::default());
    assert!(shown.contains("bell only: SSH session"), "{shown}");
}

/// Scenario "No player available": a BEL, and one log line naming the reason.
#[test]
fn without_a_player_the_bell_rings_and_the_reason_is_logged_once() {
    let empty = tempfile::tempdir().unwrap();
    let cache = tempfile::tempdir().unwrap();
    let env = env_of(vec![("PATH", empty.path().display().to_string())]);
    let mut app = App::new(Default::default(), Vec::new(), "cyber");
    app.sounds = Sounds::detect(
        Default::default(),
        Default::default(),
        &env,
        Some(cache.path().to_path_buf()),
    );
    app.focused = false;
    app.on_event("session.idle", &json!({}));
    app.on_event("session.idle", &json!({}));
    assert_eq!(
        app.attention_log,
        ["\x07\x1b]9;cyber: the session is idle\x07"; 2]
    );
    assert_eq!(app.sounds.logged(), [crate::sound::BellReason::NoPlayer]);
    assert_eq!(app.sounds.status(), "bell only: no player found");
}

/// `/sounds` drawn dark, light and under NO_COLOR: every state is a word.
#[test]
fn sounds_overlay_snapshots() {
    let cache = tempfile::tempdir().unwrap();
    let mut app = App::new(Default::default(), Vec::new(), "cyber");
    record(&mut app, cache.path());
    *app.sounds.chosen.event_mut(Alert::Message) = Choice::Off;
    app.sounds.limits.volume = Some(40);
    typed(&mut app, "/sounds");
    assert!(matches!(app.overlay, Overlay::Sounds(_)));
    for (name, opts) in [
        (
            "sounds_dark_100x30",
            Opts {
                legend: &["› Sounds"],
                ..Opts::default()
            },
        ),
        (
            "sounds_light_100x30",
            Opts {
                light: true,
                legend: &["› Sounds"],
                ..Opts::default()
            },
        ),
        (
            "sounds_no_color_100x30",
            Opts {
                no_color: true,
                legend: &["› Sounds"],
                ..Opts::default()
            },
        ),
    ] {
        let shown = render(&mut app, 100, 30, &opts);
        for needle in [
            "Sounds",
            "[cyber]",
            "60   ■■■■■■···· · 40 in this project",
            "[unfocused]",
            "message           off",
            "playing through afplay",
        ] {
            assert!(shown.contains(needle), "{needle:?}:\n{shown}");
        }
        if cfg!(not(windows)) {
            insta::assert_snapshot!(name, shown);
        }
    }
}

fn options(server: &Server, root: &Path, id: &str) -> crate::TuiOptions {
    crate::TuiOptions {
        client: server.client.clone(),
        directory: root.display().to_string(),
        state_dir: root.join("state"),
        config_dir: root.join("config"),
        start: crate::Start::Resume(id.to_owned()),
        fork: false,
        model: None,
        agent: None,
        mode: None,
        prompt: None,
    }
}

/// Scenario "Pick a pack and preview": `retro` is selected, `p` on `input` plays the
/// retro `input` sound, and `retro` is still selected after the TUI restarts.
#[tokio::test]
async fn a_picked_pack_previews_and_survives_a_restart() {
    let server = Server::start().await;
    let id = server.session().await;
    let root = server.root();
    let opts = options(&server, &root, &id);
    let store = crate::store::LocalStore::new(&opts.state_dir);
    let env = |_: &str| -> Option<String> { None };
    let (mut app, _) = crate::runner::start(&server.client, &store, &opts, &env)
        .await
        .unwrap();
    let cache = tempfile::tempdir().unwrap();
    let recorder = record(&mut app, cache.path());
    let mut saved = typed(&mut app, "/sounds");
    press(&mut app, KeyCode::Down);
    saved.extend(press(&mut app, KeyCode::Right));
    saved.extend(press(&mut app, KeyCode::Right));
    assert_eq!(app.sounds.chosen.pack, Pack::Retro);
    for _ in 0..3 {
        press(&mut app, KeyCode::Down);
    }
    let shown = render(&mut app, 100, 30, &Opts::default());
    assert!(shown.contains("› input"), "{shown}");
    assert!(press(&mut app, KeyCode::Char('p')).is_empty());
    assert_eq!(file_names(&recorder), ["retro-input-v1-60.wav"]);
    assert_eq!(saved.len(), 2, "{saved:?}");
    for action in saved {
        crate::runner::persist(&store, action);
    }
    press(&mut app, KeyCode::Esc);
    assert!(matches!(app.overlay, Overlay::None));

    let (app, _) = crate::runner::start(&server.client, &store, &opts, &env)
        .await
        .unwrap();
    assert_eq!(app.sounds.chosen.pack, Pack::Retro);
}

/// Space on an event row turns it off: its alert then plays nothing, while the
/// notification keeps its bell.
#[test]
fn an_event_toggled_off_is_silent() {
    let cache = tempfile::tempdir().unwrap();
    let mut app = App::new(Default::default(), Vec::new(), "cyber");
    let recorder = record(&mut app, cache.path());
    app.open_sounds();
    for _ in 0..9 {
        press(&mut app, KeyCode::Down);
    }
    let saved = press(&mut app, KeyCode::Char(' '));
    let [Action::SavePreference { key, value }] = saved.as_slice() else {
        panic!("{saved:?}");
    };
    assert_eq!(
        (*key, &value["events"]["message"]),
        ("sounds", &json!(false))
    );
    press(&mut app, KeyCode::Esc);
    app.focused = false;
    app.on_event(
        "messaging.message.delivered.1",
        &json!({ "direction": "in", "from": "review", "message_id": "m1" }),
    );
    app.flush_sounds();
    assert!(recorder.played().is_empty());
    assert_eq!(
        app.attention_log,
        ["\x07\x1b]9;cyber: message from review\x07"]
    );
}

/// Regression: turning an event with a custom file off and on again in `/sounds` used
/// to replace the file with the pack sound until a restart brought it back.
#[test]
fn an_event_toggled_off_and_on_keeps_its_custom_file() {
    let cache = tempfile::tempdir().unwrap();
    let file = cache.path().join("ping.wav");
    std::fs::write(&file, b"RIFF").unwrap();
    let mut app = App::new(Default::default(), Vec::new(), "cyber");
    *app.sounds.chosen.event_mut(Alert::Message) = Choice::File(file.clone());
    *app.sounds.chosen.event_mut(Alert::Error) = Choice::Pack(Pack::Retro);
    app.sounds.global_events = app.sounds.chosen.events.clone();
    let recorder = record(&mut app, cache.path());
    app.open_sounds();
    for _ in 0..5 {
        press(&mut app, KeyCode::Down);
    }
    press(&mut app, KeyCode::Char(' '));
    assert_eq!(*app.sounds.chosen.event(Alert::Error), Choice::Off);
    press(&mut app, KeyCode::Char(' '));
    assert_eq!(
        *app.sounds.chosen.event(Alert::Error),
        Choice::Pack(Pack::Retro)
    );
    for _ in 0..4 {
        press(&mut app, KeyCode::Down);
    }
    press(&mut app, KeyCode::Char(' '));
    press(&mut app, KeyCode::Char(' '));
    press(&mut app, KeyCode::Esc);
    assert_eq!(
        *app.sounds.chosen.event(Alert::Message),
        Choice::File(file.clone())
    );
    app.focused = false;
    app.on_event(
        "messaging.message.delivered.1",
        &json!({ "direction": "in", "from": "review", "message_id": "m1" }),
    );
    app.flush_sounds();
    assert_eq!(recorder.played(), [file]);
}

/// `/sounds` is in the palette and help, and `sounds_view` is unbound by default.
#[test]
fn sounds_is_offered_in_the_palette_and_help_without_a_key() {
    let mut app = App::new(Default::default(), Vec::new(), "cyber");
    assert_eq!(app.keymap.label("global", "sounds_view"), None);
    let entries = crate::palette::entries(&app.keymap, &[]);
    let entry = entries.iter().find(|e| e.name == "/sounds").unwrap();
    assert_eq!(entry.key, None);
    app.overlay = Overlay::Help;
    let screen = render(&mut app, 120, 60, &Opts::default());
    assert!(screen.contains("/sounds"), "{screen}");
}

/// The production player path (task 2.2): a stub player on `PATH` gets the cached WAV
/// of the `done` sound when a Turn ends while unfocused, and exits.
#[cfg(unix)]
#[tokio::test]
async fn a_player_on_path_plays_the_cached_sound_and_exits() {
    let server = Server::start().await;
    let id = server.answered_session().await;
    let root = server.root();
    let opts = options(&server, &root, &id);
    let store = crate::store::LocalStore::new(&opts.state_dir);
    let bin = tempfile::tempdir().unwrap();
    let player = if cfg!(target_os = "macos") {
        "afplay"
    } else {
        "pw-play"
    };
    let log = stub(bin.path(), player);
    let cache = root.join("cache");
    let env = env_of(vec![
        ("PATH", bin.path().display().to_string()),
        ("HOME", root.display().to_string()),
        ("XDG_CACHE_HOME", cache.display().to_string()),
    ]);
    let (mut app, _) = crate::runner::start(&server.client, &store, &opts, &env)
        .await
        .unwrap();
    assert_eq!(app.sounds.status(), format!("playing through {player}"));
    app.focused = false;
    turn(&server, &mut app, &id, "again").await;
    let wav = cache.join("cyber/sounds/cyber-done-v1-60.wav");
    for _ in 0..500 {
        if std::fs::read_to_string(&log).is_ok_and(|text| !text.is_empty()) && !app.sounds.busy() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(
        std::fs::read_to_string(&log).unwrap(),
        format!("{}\n", wav.display())
    );
    assert!(!app.sounds.busy(), "the player exited");
    let bytes = std::fs::read(&wav).unwrap();
    assert_eq!(&bytes[..4], b"RIFF");
    assert_eq!(
        bytes,
        crate::sound::synth::wav(&crate::sound::synth::render(Pack::Cyber, Alert::Done, 60))
    );
    assert_eq!(app.attention_log, ["\x1b]9;cyber: the session is idle\x07"]);
    app.poll_sounds();
    assert_eq!(app.attention_log.len(), 1, "the player succeeded");
    app.sounds.stop();
}
