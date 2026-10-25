//! `tui.sounds` from the `tui.jsonc` layers and `kv.json` (`tui` → Sound packs, Sound
//! settings command): the global file sets everything, `/sounds` choices saved in
//! `kv.json` win over it, and a project file, which may come from an untrusted
//! repository, can only turn sounds down. Bad values warn and keep the default.

use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};

use super::{Alert, Choice, Limits, Pack, Settings, When};

/// Custom sound files larger than this are refused.
pub const MAX_FILE: u64 = 5 * 1024 * 1024;
/// The extensions a custom sound file may have; whether it plays is up to the player.
pub const EXTENSIONS: &[&str] = &["wav", "aiff", "aif", "mp3", "ogg", "oga"];

const KEYS: &[&str] = &[
    "enabled",
    "pack",
    "volume",
    "when",
    "play_over_ssh",
    "events",
];

/// Whether `path` may be played: a regular file of at most [`MAX_FILE`] with one of
/// [`EXTENSIONS`].
pub fn check_file(path: &Path) -> Result<(), String> {
    let extension = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    if !EXTENSIONS.contains(&extension.as_str()) {
        return Err(format!("{}: not a sound file", path.display()));
    }
    let meta = std::fs::metadata(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if !meta.is_file() {
        return Err(format!("{}: not a regular file", path.display()));
    }
    if meta.len() > MAX_FILE {
        return Err(format!("{}: larger than 5 MiB", path.display()));
    }
    Ok(())
}

/// The global `sounds` object applied to the defaults.
pub fn global(
    sounds: &Map<String, Value>,
    home: Option<&Path>,
    warnings: &mut Vec<String>,
) -> Settings {
    let mut settings = Settings::default();
    for (key, value) in sounds {
        let applied = match key.as_str() {
            "events" => {
                events(&mut settings, value, home, warnings);
                true
            }
            "enabled" => set(&mut settings.enabled, value.as_bool()),
            "play_over_ssh" => set(&mut settings.play_over_ssh, value.as_bool()),
            "pack" => set(&mut settings.pack, value.as_str().and_then(Pack::parse)),
            "when" => set(&mut settings.when, value.as_str().and_then(When::parse)),
            "volume" => set(&mut settings.volume, volume(value)),
            _ => {
                warnings.push(format!("tui.sounds.{key}: unknown key"));
                true
            }
        };
        if !applied {
            warnings.push(format!("tui.sounds.{key}: unknown value {value}"));
        }
    }
    settings
}

fn set<T>(slot: &mut T, value: Option<T>) -> bool {
    value.map(|value| *slot = value).is_some()
}

fn volume(value: &Value) -> Option<u8> {
    value
        .as_u64()
        .filter(|v| *v <= 100)
        .and_then(|v| u8::try_from(v).ok())
}

/// `events`: `true`, `false`, a pack name or a sound file path per event.
fn events(settings: &mut Settings, value: &Value, home: Option<&Path>, warnings: &mut Vec<String>) {
    let Some(events) = value.as_object() else {
        warnings.push(format!("tui.sounds.events: unknown value {value}"));
        return;
    };
    for (name, value) in events {
        let Some(alert) = Alert::parse(name) else {
            warnings.push(format!("tui.sounds.events.{name}: unknown event"));
            continue;
        };
        match choice(value, home) {
            Ok(choice) => *settings.event_mut(alert) = choice,
            Err(error) => warnings.push(format!("tui.sounds.events.{name}: {error}")),
        }
    }
}

/// One event's value. A path must be absolute or start with `~/`, and the file must
/// pass [`check_file`]; one that does not keeps the pack sound.
fn choice(value: &Value, home: Option<&Path>) -> Result<Choice, String> {
    match value {
        Value::Bool(true) => Ok(Choice::On),
        Value::Bool(false) => Ok(Choice::Off),
        Value::String(name) => match Pack::parse(name) {
            Some(pack) => Ok(Choice::Pack(pack)),
            None => {
                let path = expand(name, home)
                    .ok_or_else(|| format!("{name}: not a pack, an absolute path or ~/ path"))?;
                check_file(&path)?;
                Ok(Choice::File(path))
            }
        },
        other => Err(format!("unknown value {other}")),
    }
}

fn expand(path: &str, home: Option<&Path>) -> Option<PathBuf> {
    if let Some(rest) = path.strip_prefix("~/") {
        return home.map(|home| {
            rest.split('/')
                .fold(home.to_path_buf(), |dir, part| dir.join(part))
        });
    }
    let path = PathBuf::from(path);
    path.is_absolute().then_some(path)
}

/// `/sounds` choices as saved in `kv.json`.
pub fn saved_value(settings: &Settings) -> Value {
    let events: Map<String, Value> = Alert::ALL
        .iter()
        .map(|&alert| {
            (
                alert.name().to_owned(),
                Value::Bool(settings.event(alert).on()),
            )
        })
        .collect();
    json!({
        "enabled": settings.enabled,
        "pack": settings.pack.name(),
        "volume": settings.volume,
        "when": settings.when.name(),
        "events": events,
    })
}

/// Saved `/sounds` choices over `settings`. An event saved on keeps a pack or file the
/// global file gave it.
pub fn apply_saved(settings: &mut Settings, saved: &Value) {
    if let Some(enabled) = saved["enabled"].as_bool() {
        settings.enabled = enabled;
    }
    if let Some(pack) = saved["pack"].as_str().and_then(Pack::parse) {
        settings.pack = pack;
    }
    if let Some(volume) = volume(&saved["volume"]) {
        settings.volume = volume;
    }
    if let Some(when) = saved["when"].as_str().and_then(When::parse) {
        settings.when = when;
    }
    for alert in Alert::ALL {
        match saved["events"][alert.name()].as_bool() {
            Some(false) => *settings.event_mut(alert) = Choice::Off,
            Some(true) if !settings.event(alert).on() => *settings.event_mut(alert) = Choice::On,
            _ => {}
        }
    }
}

/// What one project `sounds` object may do: `enabled: false`, a volume, events set to
/// `false`. Every other value is named in one warning. A volume above the global one
/// is a no-op, which [`crate::prefs`] warns about once both layers are read.
pub fn project(sounds: &Map<String, Value>, limits: &mut Limits, warnings: &mut Vec<String>) {
    let mut ignored = Vec::new();
    for (key, value) in sounds {
        match (key.as_str(), value) {
            ("enabled", Value::Bool(false)) => limits.off = true,
            ("volume", value) => match volume(value) {
                Some(v) => limits.volume = Some(limits.volume.map_or(v, |low| low.min(v))),
                None => ignored.push("tui.sounds.volume".to_owned()),
            },
            ("events", Value::Object(events)) => {
                for (name, value) in events {
                    match (Alert::parse(name), value) {
                        (Some(alert), Value::Bool(false)) => limits.events_off.push(alert),
                        _ => ignored.push(format!("tui.sounds.events.{name}")),
                    }
                }
            }
            (key, _) if KEYS.contains(&key) => ignored.push(format!("tui.sounds.{key}")),
            (key, _) => ignored.push(format!("tui.sounds.{key} (unknown key)")),
        }
    }
    if !ignored.is_empty() {
        warnings.push(format!(
            "a project tui.jsonc can only turn sounds down; ignored {}",
            ignored.join(", ")
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn object(text: &str) -> Map<String, Value> {
        serde_json::from_str(text).unwrap()
    }

    #[test]
    fn global_values_parse_and_bad_ones_warn_and_keep_the_default() {
        let mut warnings = Vec::new();
        let settings = global(
            &object(
                r#"{"enabled": false, "pack": "retro", "volume": 30, "when": "always",
                    "play_over_ssh": true, "events": {"message": false, "error": "soft"}}"#,
            ),
            None,
            &mut warnings,
        );
        assert!(warnings.is_empty(), "{warnings:?}");
        assert!(!settings.enabled && settings.play_over_ssh);
        assert_eq!(
            (settings.pack, settings.volume, settings.when),
            (Pack::Retro, 30, When::Always)
        );
        assert_eq!(*settings.event(Alert::Message), Choice::Off);
        assert_eq!(*settings.event(Alert::Error), Choice::Pack(Pack::Soft));

        let settings = global(
            &object(
                r#"{"pack": "jazz", "volume": 101, "loud": true,
                    "events": {"ping": true, "done": 3, "input": "sounds/x.wav"}}"#,
            ),
            None,
            &mut warnings,
        );
        assert_eq!(settings, Settings::default());
        for needle in [
            "tui.sounds.pack: unknown value \"jazz\"",
            "tui.sounds.volume: unknown value 101",
            "tui.sounds.loud: unknown key",
            "tui.sounds.events.ping: unknown event",
            "tui.sounds.events.done: unknown value 3",
            "tui.sounds.events.input: sounds/x.wav: not a pack",
        ] {
            assert!(
                warnings.iter().any(|w| w.starts_with(needle)),
                "{needle}: {warnings:?}"
            );
        }
    }

    #[test]
    fn a_custom_file_must_be_a_small_regular_sound_file() {
        let home = tempfile::tempdir().unwrap();
        let sounds = home.path().join("sounds");
        std::fs::create_dir_all(sounds.join("dir.wav")).unwrap();
        std::fs::write(sounds.join("done.wav"), vec![0; 40 * 1024]).unwrap();
        std::fs::write(sounds.join("notes.txt"), "x").unwrap();
        let big = std::fs::File::create(sounds.join("big.mp3")).unwrap();
        big.set_len(MAX_FILE + 1).unwrap();
        let home = Some(home.path());
        assert_eq!(
            choice(&json!("~/sounds/done.wav"), home),
            Ok(Choice::File(sounds.join("done.wav")))
        );
        for (value, error) in [
            ("~/sounds/notes.txt", "not a sound file"),
            ("~/sounds/big.mp3", "larger than 5 MiB"),
            ("~/sounds/dir.wav", "not a regular file"),
        ] {
            let got = choice(&json!(value), home).unwrap_err();
            assert!(got.ends_with(error), "{value}: {got}");
        }
        assert!(choice(&json!("~/sounds/missing.wav"), home).is_err());
        assert!(
            choice(&json!("~/sounds/done.wav"), None).is_err(),
            "no home"
        );
    }

    #[test]
    fn saved_choices_round_trip_and_keep_global_files() {
        let mut chosen = Settings {
            pack: Pack::Soft,
            volume: 30,
            ..Settings::default()
        };
        *chosen.event_mut(Alert::Message) = Choice::Off;
        let saved = saved_value(&chosen);
        let mut settings = Settings::default();
        *settings.event_mut(Alert::Done) = Choice::File("/s/done.wav".into());
        apply_saved(&mut settings, &saved);
        assert_eq!((settings.pack, settings.volume), (Pack::Soft, 30));
        assert_eq!(*settings.event(Alert::Message), Choice::Off);
        assert_eq!(
            *settings.event(Alert::Done),
            Choice::File("/s/done.wav".into())
        );
    }

    #[test]
    fn a_project_may_only_turn_sounds_down() {
        let mut limits = Limits::default();
        let mut warnings = Vec::new();
        project(
            &object(
                r#"{"enabled": false, "volume": 40,
                    "events": {"message": false, "input": "/tmp/x.wav", "done": true}}"#,
            ),
            &mut limits,
            &mut warnings,
        );
        project(
            &object(r#"{"volume": 70, "enabled": true, "pack": "retro"}"#),
            &mut limits,
            &mut warnings,
        );
        assert_eq!(
            limits,
            Limits {
                off: true,
                volume: Some(40),
                events_off: vec![Alert::Message],
            }
        );
        assert_eq!(
            warnings,
            [
                "a project tui.jsonc can only turn sounds down; ignored tui.sounds.events.input, tui.sounds.events.done",
                "a project tui.jsonc can only turn sounds down; ignored tui.sounds.enabled, tui.sounds.pack",
            ]
        );
    }
}
