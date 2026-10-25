//! Appearance preferences (`tui` → Themes and appearance, Color capability and
//! NO_COLOR, Accessibility): `tui.jsonc` in the global config directory and in the
//! project's `.cyber/` directories, with the theme chosen by `/theme` in `kv.json`.

use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::store::LocalStore;
use crate::theme::{Appearance, Catalog, ThemeMode, detect};

/// The global config directory, then `.cyber/` at the project root and at the
/// Location; later entries take precedence.
pub fn config_dirs(config_dir: &Path, directory: &Path) -> Vec<PathBuf> {
    let mut dirs = vec![config_dir.to_path_buf()];
    for dir in [
        cyber_core::config::project_root(directory),
        directory.to_path_buf(),
    ] {
        let dir = dir.join(".cyber");
        if !dirs.contains(&dir) {
            dirs.push(dir);
        }
    }
    dirs
}

/// `tui.jsonc` files larger than this are refused.
const MAX_TUI_JSONC: u64 = 256 * 1024;

/// A `tui.jsonc`, `None` when there is none. A project's may come from an untrusted
/// repository, so the read is bounded and never waits on a FIFO or a device.
fn read_tui_jsonc(path: &Path) -> Result<Option<String>, String> {
    match cyber_core::files::read_small(path, MAX_TUI_JSONC) {
        Ok(text) => Ok(Some(text)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

/// One `tui.jsonc` document: where it sits and when it was last written.
struct Layer {
    origin: Origin,
    modified_ms: u64,
    settings: serde_json::Map<String, Value>,
}

/// The `tui.jsonc` documents that exist, lowest precedence first. The first directory
/// is the global one; the others are the project's. Unreadable or invalid files are
/// reported and skipped.
fn layers(dirs: &[PathBuf], warnings: &mut Vec<String>) -> Vec<Layer> {
    let mut out = Vec::new();
    for (index, dir) in dirs.iter().enumerate() {
        let path = dir.join("tui.jsonc");
        let text = match read_tui_jsonc(&path) {
            Ok(Some(text)) => text,
            Ok(None) => continue,
            Err(warning) => {
                warnings.push(warning);
                continue;
            }
        };
        match cyber_core::config::parse_jsonc(&path.display().to_string(), &text) {
            Ok(Value::Object(settings)) => out.push(Layer {
                origin: if index == 0 {
                    Origin::Global
                } else {
                    Origin::Project
                },
                modified_ms: modified_ms(&path),
                settings,
            }),
            Ok(_) => warnings.push(format!("{}: not an object", path.display())),
            Err(e) => warnings.push(e.to_string()),
        }
    }
    out
}

fn modified_ms(path: &Path) -> u64 {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

/// The layers merged key by key (objects such as `keybinds` key by key too).
fn merged(layers: &[Layer]) -> serde_json::Map<String, Value> {
    let mut merged = serde_json::Map::new();
    for layer in layers {
        merge(&mut merged, layer.settings.clone());
    }
    merged
}

fn merge(into: &mut serde_json::Map<String, Value>, from: serde_json::Map<String, Value>) {
    for (key, value) in from {
        match (into.get_mut(&key), value) {
            (Some(Value::Object(old)), Value::Object(new)) => merge(old, new),
            (_, value) => {
                into.insert(key, value);
            }
        }
    }
}

/// Write `theme` to the `.cyber/tui.jsonc` of the project containing `directory`
/// (`tui` → Theme switcher, `p`). A file with comments is never rewritten, so its
/// comments cannot be lost: it is refused, and the theme must be set there by hand.
pub fn save_project_theme(directory: &Path, name: &str) -> Result<PathBuf, String> {
    if name.is_empty()
        || !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))
    {
        return Err(format!("theme {name:?}: not a theme name"));
    }
    let dir = cyber_core::config::project_root(directory).join(".cyber");
    let path = dir.join("tui.jsonc");
    let mut settings = match read_tui_jsonc(&path)? {
        Some(text) => match serde_json::from_str::<Value>(&text) {
            Ok(Value::Object(settings)) => settings,
            Ok(_) => return Err(format!("{}: not an object", path.display())),
            Err(_) => {
                return Err(format!(
                    "{} has comments or is not plain JSON; set \"theme\": \"{name}\" there",
                    path.display()
                ));
            }
        },
        None => serde_json::Map::new(),
    };
    settings.insert("theme".into(), Value::String(name.to_owned()));
    let text = serde_json::to_string_pretty(&Value::Object(settings)).expect("JSON serializes");
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    std::fs::write(&path, format!("{text}\n")).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(path)
}

/// Where a preference's value comes from (`tui` → Settings Config tab).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    Default,
    /// The global `tui.jsonc`.
    Global,
    /// A project `.cyber/tui.jsonc`.
    Project,
    /// Saved from the TUI to `kv.json`.
    Saved,
    /// Changed for this run only (Ctrl+T, `/timestamps`).
    Session,
}

impl Origin {
    pub fn label(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::Global => "global",
            Self::Project => "project",
            Self::Saved => "saved",
            Self::Session => "session",
        }
    }
}

/// The preferences the Config tab edits and the values each cycles through, default
/// first. `theme` cycles through the theme catalog instead.
pub const EDITABLE: &[(&str, &[&str])] = &[
    ("theme", &["cyber"]),
    ("theme_mode", &["auto", "dark", "light"]),
    ("sidebar", &["auto", "show", "hide"]),
    ("machine_stats", &["on", "off"]),
    ("recap", &["auto", "off"]),
    ("spinner", crate::progress::Spinner::VALUES),
    (
        "workflow_notifications",
        crate::workflow_watch::Scope::VALUES,
    ),
    ("reasoning", &["collapsed", "hidden", "expanded"]),
    ("diff_style", &["auto", "unified", "split"]),
    ("timestamps", &["off", "on"]),
    ("mouse", &["on", "off"]),
    ("color", &["auto", "truecolor", "256", "16", "none"]),
    ("accessibility.reduced_motion", &["off", "on"]),
];

/// Values written as JSON booleans.
fn boolean(key: &str) -> bool {
    matches!(
        key,
        "timestamps" | "accessibility.reduced_motion" | "machine_stats" | "mouse"
    )
}

/// One editable preference.
#[derive(Debug, Clone, PartialEq)]
pub struct Setting {
    pub key: &'static str,
    pub value: String,
    pub origin: Origin,
}

/// The editable preferences with their sources, and what `color: auto` resolves to.
#[derive(Debug, Clone, PartialEq)]
pub struct Preferences {
    pub settings: Vec<Setting>,
    /// `NO_COLOR` is set: it wins over `tui.color`.
    pub no_color: bool,
    /// The depth `COLORTERM` and `TERM` give.
    pub detected_depth: crate::theme::Depth,
}

impl Default for Preferences {
    fn default() -> Self {
        Self {
            settings: EDITABLE
                .iter()
                .map(|(key, values)| Setting {
                    key,
                    value: values[0].to_owned(),
                    origin: Origin::Default,
                })
                .collect(),
            no_color: false,
            detected_depth: crate::theme::Depth::TrueColor,
        }
    }
}

impl Preferences {
    pub fn get(&self, key: &str) -> &str {
        self.settings
            .iter()
            .find(|s| s.key == key)
            .map_or("", |s| s.value.as_str())
    }

    pub fn set(&mut self, key: &str, value: &str, origin: Origin) {
        if let Some(setting) = self.settings.iter_mut().find(|s| s.key == key) {
            setting.value = value.to_owned();
            setting.origin = origin;
        }
    }

    /// The color depth for `tui.color`, with `NO_COLOR` first.
    pub fn depth(&self) -> crate::theme::Depth {
        let no_color = self.no_color;
        let env = move |name: &str| (name == "NO_COLOR" && no_color).then(|| "1".to_owned());
        match self.get("color") {
            "auto" if !no_color => self.detected_depth,
            setting => detect::depth(&env, Some(setting)),
        }
    }

    /// The JSON a value is saved as.
    pub fn json(key: &str, value: &str) -> Value {
        if boolean(key) {
            Value::Bool(value == "on")
        } else {
            Value::String(value.to_owned())
        }
    }
}

/// A `tui.jsonc` value as a Config tab value: booleans as `on`/`off`.
fn text_value(key: &str, value: &Value) -> Option<String> {
    match value {
        Value::Bool(b) if boolean(key) => Some(if *b { "on" } else { "off" }.to_owned()),
        Value::String(s) if !boolean(key) => Some(s.clone()),
        _ => None,
    }
}

/// The value at a dotted key (`accessibility.reduced_motion`).
fn lookup<'a>(settings: &'a serde_json::Map<String, Value>, key: &str) -> Option<&'a Value> {
    let (head, rest) = key.split_once('.').unwrap_or((key, ""));
    let value = settings.get(head)?;
    if rest.is_empty() {
        Some(value)
    } else {
        value.as_object().and_then(|object| lookup(object, rest))
    }
}

/// A value saved from the TUI, and when.
fn saved(store: &LocalStore, key: &str) -> Option<(Value, u64)> {
    let entry = store.get("prefs")?.get(key)?.clone();
    Some((
        entry.get("value")?.clone(),
        entry.get("saved_ms")?.as_u64()?,
    ))
}

/// The nearest `tui.jsonc` value of `key`, its origin and file time.
fn configured(layers: &[Layer], key: &str) -> Option<(Value, Origin, u64)> {
    layers.iter().rev().find_map(|layer| {
        lookup(&layer.settings, key).map(|v| (v.clone(), layer.origin, layer.modified_ms))
    })
}

/// One preference: a saved value wins unless the `tui.jsonc` that sets the key was
/// written after it. A `theme` saved before saving had times (a bare `kv.json`
/// `theme`) always wins, as it did before.
fn resolve(
    store: &LocalStore,
    layers: &[Layer],
    key: &'static str,
    warnings: &mut Vec<String>,
) -> Option<(String, Origin)> {
    let legacy = (key == "theme")
        .then(|| store.get("theme"))
        .flatten()
        .map(|v| (v, u64::MAX));
    let saved = saved(store, key).or(legacy);
    let configured = configured(layers, key);
    let (value, origin) = match (saved, configured) {
        (Some((value, at)), Some((_, _, written))) if written <= at => (value, Origin::Saved),
        (_, Some((value, origin, _))) => (value, origin),
        (Some((value, _)), None) => (value, Origin::Saved),
        (None, None) => return None,
    };
    let Some(text) = text_value(key, &value).filter(|text| allowed(key, text)) else {
        warnings.push(format!("tui.{key}: unknown value {value}"));
        return None;
    };
    Some((text, origin))
}

fn allowed(key: &str, value: &str) -> bool {
    key == "theme" && !value.is_empty()
        || EDITABLE
            .iter()
            .any(|(k, values)| *k == key && values.contains(&value))
}

/// Everything the UI needs to draw its first frame.
pub struct Loaded {
    pub theme: String,
    pub themes: Catalog,
    pub appearance: Appearance,
    pub reduced_motion: bool,
    pub screen_reader: bool,
    pub keymap: crate::keys::Keymap,
    /// `tui.statusline.fields`; `None` keeps the default fields.
    pub statusline_fields: Option<Vec<String>>,
    /// `tui.sidebar`.
    pub sidebar: crate::sidebar::Preference,
    /// `tui.welcome`.
    pub welcome: crate::view::welcome::Welcome,
    /// `tui.explorer.icons`.
    pub explorer_icons: crate::explorer::Icons,
    /// `tui.notifications`.
    pub notifications: crate::osc::Notifications,
    /// `tui.paste_summary`: large pastes become chips.
    pub paste_summary: bool,
    /// `tui.sounds` from the global file and `/sounds`, which `/sounds` edits.
    pub sounds: crate::sound::Settings,
    /// What the project files turn down.
    pub sound_limits: crate::sound::Limits,
    /// The events as the global file alone sets them, which `/sounds` restores.
    pub sound_global_events: crate::sound::Events,
    /// The Config tab's preferences with their sources.
    pub preferences: Preferences,
}

/// A string setting parsed by `parse`; a value it refuses is reported and the default
/// stays.
fn choice<T: Default>(
    settings: &serde_json::Map<String, Value>,
    key: &str,
    parse: fn(Option<&str>) -> Option<T>,
    warnings: &mut Vec<String>,
) -> T {
    let Some(value) = lookup(settings, key) else {
        return T::default();
    };
    parse(value.as_str()).unwrap_or_else(|| {
        warnings.push(format!("tui.{key}: unknown value {value}"));
        T::default()
    })
}

/// `tui.paste_summary`, on unless set to `false`.
fn paste_summary(settings: &serde_json::Map<String, Value>, warnings: &mut Vec<String>) -> bool {
    match settings.get("paste_summary") {
        None | Some(Value::Bool(true)) => true,
        Some(Value::Bool(false)) => false,
        Some(other) => {
            warnings.push(format!("tui.paste_summary: unknown value {other}"));
            true
        }
    }
}

/// `tui.sounds` (`tui` → Sound packs): the global file, then the `/sounds` choices saved
/// in `kv.json` unless the global file was written after them (as `/theme`), then what
/// the project files turn down. Custom file paths are read only from the global file.
fn sounds(
    store: &LocalStore,
    layers: &[Layer],
    home: Option<&Path>,
    warnings: &mut Vec<String>,
) -> (
    crate::sound::Settings,
    crate::sound::Limits,
    crate::sound::Events,
) {
    use crate::sound::config;
    let mut settings = crate::sound::Settings::default();
    let mut limits = crate::sound::Limits::default();
    let mut written = 0;
    for layer in layers {
        let sounds = match layer.settings.get("sounds") {
            None => continue,
            Some(Value::Object(sounds)) => sounds,
            Some(other) => {
                warnings.push(format!("tui.sounds: unknown value {other}"));
                continue;
            }
        };
        if layer.origin == Origin::Global {
            settings = config::global(sounds, home, warnings);
            written = layer.modified_ms;
        } else {
            config::project(sounds, &mut limits, warnings);
        }
    }
    let global_events = settings.events.clone();
    if let Some((value, at)) = saved(store, "sounds")
        && written <= at
    {
        config::apply_saved(&mut settings, &value);
    }
    if limits.volume.is_some_and(|volume| volume > settings.volume) {
        warnings.push(
            "a project tui.jsonc can only turn sounds down; ignored tui.sounds.volume".into(),
        );
    }
    (settings, limits, global_events)
}

/// `tui.statusline.fields`: the known field names it lists, in order.
fn statusline_fields(
    settings: &serde_json::Map<String, Value>,
    warnings: &mut Vec<String>,
) -> Option<Vec<String>> {
    const KNOWN: &[&str] = &[
        "model",
        "variant",
        "agent",
        "mode",
        "context",
        "cost",
        "branch",
        "worktree",
        "goal",
        "loops",
        "remote",
        "cwd",
        "workflows",
    ];
    let list = settings.get("statusline")?.get("fields")?.as_array()?;
    let mut fields = Vec::new();
    for value in list {
        match value.as_str() {
            Some(name) if KNOWN.contains(&name) => fields.push(name.to_owned()),
            _ => warnings.push(format!("tui.statusline.fields: unknown field {value}")),
        }
    }
    Some(fields)
}

/// The editable preferences from `kv.json` and the `tui.jsonc` layers.
fn preferences(
    store: &LocalStore,
    layers: &[Layer],
    env: &dyn Fn(&str) -> Option<String>,
    warnings: &mut Vec<String>,
) -> Preferences {
    let mut preferences = Preferences {
        no_color: env("NO_COLOR").is_some_and(|v| !v.is_empty()),
        detected_depth: detect::depth(
            &|name| (name != "NO_COLOR").then(|| env(name)).flatten(),
            None,
        ),
        ..Preferences::default()
    };
    for (key, _) in EDITABLE {
        if let Some((value, origin)) = resolve(store, layers, key, warnings) {
            preferences.set(key, &value, origin);
        }
    }
    preferences
}

/// Read the preferences. `env` is the process environment.
#[cfg(test)]
pub fn load(store: &LocalStore, dirs: &[PathBuf], env: &dyn Fn(&str) -> Option<String>) -> Loaded {
    load_with_plugins(store, dirs, &[], env)
}

/// [`load`], with loaded plugins' theme folders as `(plugin, dir)` below every config
/// directory's.
pub fn load_with_plugins(
    store: &LocalStore,
    dirs: &[PathBuf],
    plugin_themes: &[(String, PathBuf)],
    env: &dyn Fn(&str) -> Option<String>,
) -> Loaded {
    let theme_dirs: Vec<PathBuf> = dirs.iter().map(|d| d.join("themes")).collect();
    let mut themes = Catalog::load_with_plugins(plugin_themes, &theme_dirs);
    let layers = layers(dirs, &mut themes.warnings);
    let settings = merged(&layers);
    let preferences = preferences(store, &layers, env, &mut themes.warnings);
    let screen_reader = env("CYBER_SCREEN_READER").is_some_and(|v| v == "1")
        || settings
            .get("accessibility")
            .is_some_and(|a| a["screen_reader"] == true);
    let reduced_motion = screen_reader || preferences.get("accessibility.reduced_motion") == "on";
    let appearance = Appearance {
        depth: preferences.depth(),
        mode: ThemeMode::parse(Some(preferences.get("theme_mode"))),
        detected_light: env("COLORFGBG").and_then(|v| detect::colorfgbg_is_light(&v)),
    };
    let keymap = crate::keys::Keymap::load(settings.get("keybinds"), &mut themes.warnings);
    let statusline_fields = statusline_fields(&settings, &mut themes.warnings);
    let welcome = choice(
        &settings,
        "welcome",
        crate::view::welcome::Welcome::parse,
        &mut themes.warnings,
    );
    let explorer_icons = choice(
        &settings,
        "explorer.icons",
        crate::explorer::Icons::parse,
        &mut themes.warnings,
    );
    let notifications = choice(
        &settings,
        "notifications",
        crate::osc::Notifications::parse,
        &mut themes.warnings,
    );
    let paste_summary = paste_summary(&settings, &mut themes.warnings);
    let home = cyber_core::paths::home_dir(&cyber_core::env::FnEnv(env));
    let (sounds, sound_limits, sound_global_events) =
        sounds(store, &layers, home.as_deref(), &mut themes.warnings);
    Loaded {
        theme: preferences.get("theme").to_owned(),
        themes,
        appearance,
        reduced_motion,
        screen_reader,
        keymap,
        statusline_fields,
        sidebar: crate::sidebar::Preference::parse(Some(preferences.get("sidebar")))
            .unwrap_or_default(),
        welcome,
        explorer_icons,
        notifications,
        paste_summary,
        sounds,
        sound_limits,
        sound_global_events,
        preferences,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tui_jsonc_layers_set_color_mode_and_motion_and_kv_keeps_the_theme() {
        let root = tempfile::tempdir().unwrap();
        let (global, project) = (root.path().join("config"), root.path().join("repo"));
        std::fs::create_dir_all(global.join("themes")).unwrap();
        std::fs::create_dir_all(project.join(".cyber")).unwrap();
        std::fs::write(
            global.join("tui.jsonc"),
            "// global\n{\"color\": \"256\", \"theme_mode\": \"light\", \"theme\": \"nord\"}",
        )
        .unwrap();
        std::fs::write(
            project.join(".cyber/tui.jsonc"),
            "{\"color\": \"16\", \"accessibility\": {\"reduced_motion\": true}, \"sidebar\": \"hide\", \"welcome\": \"off\", \"explorer\": {\"icons\": \"nerd\"}}",
        )
        .unwrap();
        std::fs::write(
            global.join("themes/corp.json"),
            r##"{"theme":{"accent":"#123456"}}"##,
        )
        .unwrap();
        let store = LocalStore::new(&root.path().join("state"));
        let env = |name: &str| (name == "COLORFGBG").then(|| "0;15".to_owned());
        let dirs = config_dirs(&global, &project);
        let loaded = load(&store, &dirs, &env);
        assert_eq!(loaded.theme, "nord");
        assert_eq!(loaded.appearance.depth, crate::theme::Depth::Ansi16);
        assert_eq!(loaded.appearance.mode, ThemeMode::Light);
        assert_eq!(loaded.appearance.detected_light, Some(true));
        assert!(loaded.reduced_motion);
        assert!(loaded.themes.names().any(|n| n == "corp"));
        assert_eq!(loaded.sidebar, crate::sidebar::Preference::Hide);
        assert_eq!(loaded.welcome, crate::view::welcome::Welcome::Off);
        assert_eq!(loaded.explorer_icons, crate::explorer::Icons::Nerd);
        assert!(loaded.paste_summary, "on by default");

        store.set("theme", Value::String("dracula".into()));
        let no_color = |name: &str| (name == "NO_COLOR").then(|| "1".to_owned());
        let loaded = load(&store, &dirs, &no_color);
        assert_eq!(loaded.theme, "dracula");
        assert_eq!(loaded.appearance.depth, crate::theme::Depth::None);
    }

    #[test]
    fn sources_are_project_over_global_and_a_save_wins_until_tui_jsonc_is_newer() {
        let root = tempfile::tempdir().unwrap();
        let (global, project) = (root.path().join("config"), root.path().join("repo"));
        std::fs::create_dir_all(&global).unwrap();
        std::fs::create_dir_all(project.join(".cyber")).unwrap();
        std::fs::write(
            global.join("tui.jsonc"),
            "{\"timestamps\": true, \"reasoning\": \"expanded\", \"color\": \"16\"}",
        )
        .unwrap();
        std::fs::write(
            project.join(".cyber/tui.jsonc"),
            "{\"reasoning\": \"hidden\"}",
        )
        .unwrap();
        let store = LocalStore::new(&root.path().join("state"));
        let dirs = config_dirs(&global, &project);
        let loaded = load(&store, &dirs, &|_: &str| None);
        let p = &loaded.preferences;
        let origin = |key: &str| p.settings.iter().find(|s| s.key == key).unwrap().origin;
        assert_eq!(
            (p.get("timestamps"), origin("timestamps")),
            ("on", Origin::Global)
        );
        assert_eq!(
            (p.get("reasoning"), origin("reasoning")),
            ("hidden", Origin::Project)
        );
        assert_eq!((p.get("recap"), origin("recap")), ("auto", Origin::Default));
        assert_eq!(loaded.appearance.depth, crate::theme::Depth::Ansi16);

        // A save made after the files were written wins; booleans stay booleans.
        store.save_preference("timestamps", Preferences::json("timestamps", "off"));
        assert_eq!(store.get("prefs").unwrap()["timestamps"]["value"], false);
        let loaded = load(&store, &dirs, &|_: &str| None);
        assert_eq!(loaded.preferences.get("timestamps"), "off");
        // NO_COLOR wins over `tui.color`.
        let no_color = |name: &str| (name == "NO_COLOR").then(|| "1".to_owned());
        let loaded = load(&store, &dirs, &no_color);
        assert_eq!(loaded.appearance.depth, crate::theme::Depth::None);
        assert_eq!(loaded.preferences.depth(), crate::theme::Depth::None);
    }

    #[test]
    fn an_invalid_tui_jsonc_is_reported_and_ignored() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("tui.jsonc"), "{ nope").unwrap();
        let store = LocalStore::new(&root.path().join("state"));
        let loaded = load(&store, &[root.path().to_path_buf()], &|_: &str| None);
        assert_eq!(loaded.theme, "cyber");
        assert_eq!(loaded.appearance.depth, crate::theme::Depth::Ansi16);
        assert_eq!(loaded.themes.warnings.len(), 1);
    }

    #[test]
    fn paste_summary_turns_off_and_a_non_boolean_is_reported() {
        let root = tempfile::tempdir().unwrap();
        let store = LocalStore::new(&root.path().join("state"));
        let dirs = [root.path().to_path_buf()];
        std::fs::write(root.path().join("tui.jsonc"), "{\"paste_summary\": false}").unwrap();
        assert!(!load(&store, &dirs, &|_: &str| None).paste_summary);
        std::fs::write(root.path().join("tui.jsonc"), "{\"paste_summary\": \"no\"}").unwrap();
        let loaded = load(&store, &dirs, &|_: &str| None);
        assert!(loaded.paste_summary);
        assert_eq!(
            loaded.themes.warnings,
            ["tui.paste_summary: unknown value \"no\""]
        );
    }

    /// Regression: a project `tui.jsonc` (from an untrusted repository) was read whole,
    /// so a huge file or a link to `/dev/zero` exhausted memory and a FIFO blocked
    /// startup for good. Each is now refused with a warning, promptly.
    #[test]
    fn an_oversized_or_special_project_tui_jsonc_is_refused_promptly() {
        let root = tempfile::tempdir().unwrap();
        let (global, project) = (root.path().join("config"), root.path().join("repo"));
        std::fs::create_dir_all(&global).unwrap();
        std::fs::create_dir_all(project.join(".cyber")).unwrap();
        std::fs::write(global.join("tui.jsonc"), "{\"theme\": \"nord\"}").unwrap();
        let file = project.join(".cyber/tui.jsonc");
        let padding = " ".repeat(MAX_TUI_JSONC as usize);
        std::fs::write(&file, format!("{{\"theme\": \"dracula\"}}{padding}")).unwrap();
        let store = LocalStore::new(&root.path().join("state"));
        let dirs = config_dirs(&global, &project);
        let loaded = load(&store, &dirs, &|_: &str| None);
        assert_eq!(loaded.theme, "nord", "the oversized layer is skipped");
        assert!(
            loaded
                .themes
                .warnings
                .iter()
                .any(|w| w.contains("larger than")),
            "{:?}",
            loaded.themes.warnings
        );
        assert!(save_project_theme(&project, "dracula").is_err());
        #[cfg(unix)]
        for special in ["zero", "fifo"] {
            std::fs::remove_file(&file).unwrap();
            if special == "zero" {
                std::os::unix::fs::symlink("/dev/zero", &file).unwrap();
            } else {
                let made = std::process::Command::new("mkfifo").arg(&file).status();
                assert!(made.unwrap().success());
            }
            let (done, finished) = std::sync::mpsc::channel();
            let (state, dirs) = (root.path().join("state"), dirs.clone());
            let saved_in = project.clone();
            std::thread::spawn(move || {
                let loaded = load(&LocalStore::new(&state), &dirs, &|_: &str| None);
                let saved = save_project_theme(&saved_in, "dracula");
                done.send((loaded.theme, loaded.themes.warnings, saved.is_err()))
                    .unwrap();
            });
            let (theme, warnings, refused) = finished
                .recv_timeout(std::time::Duration::from_secs(10))
                .unwrap_or_else(|_| panic!("{special}: loaded promptly"));
            assert_eq!(theme, "nord", "{special}");
            assert!(
                warnings.iter().any(|w| w.contains("not a regular file")),
                "{special}: {warnings:?}"
            );
            assert!(refused, "{special}: never rewritten");
        }
    }

    fn sound_dirs(root: &Path, global: &str, project: &str) -> Vec<PathBuf> {
        let (config, repo) = (root.join("config"), root.join("repo"));
        std::fs::create_dir_all(&config).unwrap();
        std::fs::create_dir_all(repo.join(".cyber")).unwrap();
        std::fs::write(config.join("tui.jsonc"), global).unwrap();
        std::fs::write(repo.join(".cyber/tui.jsonc"), project).unwrap();
        config_dirs(&config, &repo)
    }

    /// Scenario "Project cannot pick a file": the project's file and `enabled: true` are
    /// ignored, sounds stay off, and one warning names the ignored keys.
    #[test]
    fn a_project_cannot_pick_a_sound_file_or_turn_sounds_on() {
        use crate::sound::{Alert, Choice};
        let root = tempfile::tempdir().unwrap();
        let dirs = sound_dirs(
            root.path(),
            r#"{"sounds": {"enabled": false}}"#,
            r#"{"sounds": {"events": {"input": "/tmp/x.wav"}, "enabled": true}}"#,
        );
        let store = LocalStore::new(&root.path().join("state"));
        let loaded = load(&store, &dirs, &|_: &str| None);
        assert!(!loaded.sounds.enabled);
        assert_eq!(loaded.sound_limits, crate::sound::Limits::default());
        assert_eq!(*loaded.sounds.event(Alert::Input), Choice::On);
        assert_eq!(
            loaded.themes.warnings,
            [
                "a project tui.jsonc can only turn sounds down; ignored tui.sounds.events.input, tui.sounds.enabled"
            ]
        );
    }

    /// Scenario "Custom file from global config": `~/sounds/done.wav` plays for `done`,
    /// and the other events keep the pack sounds.
    #[test]
    fn a_custom_file_from_the_global_file_plays_for_its_event() {
        use crate::sound::player::Recorder;
        use crate::sound::{Alert, Choice, Sounds};
        let root = tempfile::tempdir().unwrap();
        let file = root.path().join("home").join("sounds").join("done.wav");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, vec![0u8; 40 * 1024]).unwrap();
        let dirs = sound_dirs(
            root.path(),
            r#"{"sounds": {"events": {"done": "~/sounds/done.wav"}}}"#,
            "{}",
        );
        let store = LocalStore::new(&root.path().join("state"));
        let home = root.path().join("home").display().to_string();
        let env = move |name: &str| (name == "HOME").then(|| home.clone());
        let loaded = load(&store, &dirs, &env);
        assert!(
            loaded.themes.warnings.is_empty(),
            "{:?}",
            loaded.themes.warnings
        );
        assert_eq!(
            *loaded.sounds.event(Alert::Done),
            Choice::File(file.clone())
        );
        let recorder = Recorder::default();
        let cache = root.path().join("cache");
        let mut sounds = Sounds::recording(loaded.sounds, &cache, &recorder);
        sounds.limits = loaded.sound_limits;
        let t = std::time::Instant::now();
        sounds.fire(Alert::Done, false, t);
        sounds.flush(t);
        let later = t + crate::sound::THROTTLE;
        sounds.fire(Alert::Error, false, later);
        sounds.flush(later);
        assert_eq!(
            recorder.played(),
            [file, cache.join("cyber-error-v1-60.wav")]
        );
    }

    #[test]
    fn a_project_lowers_the_volume_but_never_raises_it_and_unknown_keys_warn() {
        let root = tempfile::tempdir().unwrap();
        let store = LocalStore::new(&root.path().join("state"));
        let dirs = sound_dirs(
            root.path(),
            r#"{"sounds": {"volume": 50, "chime": 1}}"#,
            r#"{"sounds": {"volume": 30, "events": {"message": false}}}"#,
        );
        let loaded = load(&store, &dirs, &|_: &str| None);
        let effective = loaded.sounds.restricted(&loaded.sound_limits);
        assert_eq!((loaded.sounds.volume, effective.volume), (50, 30));
        assert!(!effective.event(crate::sound::Alert::Message).on());
        assert_eq!(loaded.themes.warnings, ["tui.sounds.chime: unknown key"]);

        let dirs = sound_dirs(
            root.path(),
            r#"{"sounds": {"volume": 50}}"#,
            r#"{"sounds": {"volume": 90}}"#,
        );
        let loaded = load(&store, &dirs, &|_: &str| None);
        assert_eq!(loaded.sounds.restricted(&loaded.sound_limits).volume, 50);
        assert_eq!(
            loaded.themes.warnings,
            ["a project tui.jsonc can only turn sounds down; ignored tui.sounds.volume"]
        );

        std::fs::write(root.path().join("config/tui.jsonc"), r#"{"sounds": true}"#).unwrap();
        let loaded = load(&store, &dirs, &|_: &str| None);
        assert_eq!(
            loaded.themes.warnings,
            [
                "tui.sounds: unknown value true",
                // The project's 90 is over the default 60.
                "a project tui.jsonc can only turn sounds down; ignored tui.sounds.volume",
            ]
        );
    }

    /// `/sounds` choices in `kv.json` win over the global file, until it is written again.
    #[test]
    fn saved_sound_choices_win_over_the_global_file() {
        use crate::sound::{Pack, Settings, config};
        let root = tempfile::tempdir().unwrap();
        let store = LocalStore::new(&root.path().join("state"));
        let dirs = sound_dirs(root.path(), r#"{"sounds": {"pack": "soft"}}"#, "{}");
        let chosen = Settings {
            pack: Pack::Retro,
            ..Settings::default()
        };
        store.save_preference("sounds", config::saved_value(&chosen));
        assert_eq!(
            load(&store, &dirs, &|_: &str| None).sounds.pack,
            Pack::Retro
        );
        let mut prefs = store.get("prefs").unwrap();
        prefs["sounds"]["saved_ms"] = Value::from(0);
        store.set("prefs", prefs);
        assert_eq!(load(&store, &dirs, &|_: &str| None).sounds.pack, Pack::Soft);
    }
}
