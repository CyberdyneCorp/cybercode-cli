//! The keybinding table (`tui` → Keybindings): defaults merged with `tui.keybinds`.
//!
//! A binding is a key (`ctrl+p`) or a leader chord (`<leader>b`), scoped to a context.
//! `"none"` disables a binding, and a disabled binding is never shown in a hint, the
//! help screen or the palette. A value that does not parse keeps the default and is
//! reported, so a typo never leaves an action without its key.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use serde_json::Value;

/// Contexts `tui.keybinds` may scope bindings to. `global` bindings sit at the top level.
pub const CONTEXTS: &[&str] = &[
    "global", "composer", "palette", "sidebar", "settings", "explorer",
];

/// `(context, action, default, what it does)`.
const DEFAULTS: &[(&str, &str, &str, &str)] = &[
    ("global", "leader", "ctrl+x", "leader key"),
    ("global", "command_palette", "ctrl+p", "command palette"),
    (
        "global",
        "sidebar_toggle",
        "<leader>b",
        "show or hide the sidebar",
    ),
    ("global", "sidebar_focus", "<leader>e", "focus the sidebar"),
    ("global", "settings", "<leader>,", "settings"),
    (
        "global",
        "file_explorer",
        "<leader>o",
        "browse project files",
    ),
    ("global", "model_list", "<leader>m", "switch model"),
    ("global", "session_list", "<leader>l", "sessions"),
    ("global", "session_new", "<leader>n", "new session"),
    ("global", "theme_list", "<leader>s", "switch theme"),
    (
        "global",
        "theme_cycle",
        "<leader>t",
        "next theme in the switcher's filter",
    ),
    ("global", "session_fork", "<leader>f", "fork session"),
    (
        "global",
        "copy_code",
        "<leader>y",
        "select a code block to copy",
    ),
    ("global", "help", "<leader>h", "keys"),
    (
        "global",
        "reasoning_toggle",
        "ctrl+t",
        "show or hide reasoning",
    ),
    (
        "global",
        "tool_details",
        "ctrl+o",
        "expand or collapse tool output",
    ),
    (
        "composer",
        "history_previous",
        "up",
        "line up, then older history",
    ),
    (
        "composer",
        "history_next",
        "down",
        "line down, then newer history",
    ),
    ("composer", "newline", "ctrl+j", "insert a newline"),
    ("composer", "editor", "ctrl+g", "edit in $EDITOR"),
    ("palette", "palette_close", "esc", "close the palette"),
    ("palette", "palette_run", "enter", "run the selected entry"),
    ("palette", "palette_next", "down", "next entry"),
    ("palette", "palette_previous", "up", "previous entry"),
    ("settings", "settings_close", "esc", "close the settings"),
    ("settings", "settings_next", "tab", "next tab"),
    ("settings", "settings_previous", "shift+tab", "previous tab"),
    ("settings", "settings_refresh", "r", "load the tab again"),
    ("explorer", "explorer_close", "esc", "close"),
    ("explorer", "explorer_open", "enter", "open"),
    ("explorer", "explorer_next", "down", "next row"),
    ("explorer", "explorer_previous", "up", "previous row"),
    ("explorer", "explorer_filter", "/", "filter"),
    ("explorer", "explorer_changed", "g", "changed only"),
    ("explorer", "explorer_hidden", ".", "hidden files"),
    ("explorer", "explorer_mention", "@", "mention"),
    ("explorer", "explorer_copy", "y", "copy path"),
    ("explorer", "explorer_refresh", "r", "refresh"),
    ("sidebar", "sidebar_close", "esc", "return to the composer"),
    ("sidebar", "sidebar_next", "down", "next row"),
    ("sidebar", "sidebar_previous", "up", "previous row"),
    (
        "sidebar",
        "sidebar_select",
        "enter",
        "collapse a section or open a file's diff",
    ),
    (
        "sidebar",
        "sidebar_reconnect",
        "r",
        "reconnect a failed MCP server",
    ),
];

/// One key with its modifiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Key {
    pub code: KeyCode,
    pub modifiers: KeyModifiers,
}

impl Key {
    fn matches(&self, key: &KeyEvent) -> bool {
        let wanted = self.modifiers & !KeyModifiers::SHIFT;
        let got = key.modifiers & !KeyModifiers::SHIFT;
        let code = match (self.code, key.code) {
            (KeyCode::Char(a), KeyCode::Char(b)) => a.eq_ignore_ascii_case(&b),
            (a, b) => a == b,
        };
        code && wanted == got
    }

    fn label(&self) -> String {
        let mut out = String::new();
        for (flag, name) in [
            (KeyModifiers::CONTROL, "ctrl+"),
            (KeyModifiers::ALT, "alt+"),
            (KeyModifiers::SHIFT, "shift+"),
        ] {
            if self.modifiers.contains(flag) {
                out.push_str(name);
            }
        }
        out.push_str(&match self.code {
            KeyCode::Char(' ') => "space".into(),
            KeyCode::Char(c) => c.to_string(),
            KeyCode::F(n) => format!("f{n}"),
            KeyCode::Enter => "enter".into(),
            KeyCode::Esc => "esc".into(),
            KeyCode::Tab => "tab".into(),
            KeyCode::BackTab => "shift+tab".into(),
            KeyCode::Up => "up".into(),
            KeyCode::Down => "down".into(),
            KeyCode::Left => "left".into(),
            KeyCode::Right => "right".into(),
            KeyCode::Home => "home".into(),
            KeyCode::End => "end".into(),
            KeyCode::PageUp => "pageup".into(),
            KeyCode::PageDown => "pagedown".into(),
            KeyCode::Backspace => "backspace".into(),
            KeyCode::Delete => "delete".into(),
            _ => "?".into(),
        });
        out
    }
}

/// What a binding is bound to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Chord {
    Key(Key),
    /// The leader key, then this key.
    Leader(Key),
    /// `"none"`.
    Disabled,
}

/// Parse `ctrl+p`, `alt+enter`, `f2`, `<leader>b` or `none`.
pub fn parse(value: &str) -> Option<Chord> {
    let value = value.trim().to_ascii_lowercase();
    if value == "none" {
        return Some(Chord::Disabled);
    }
    if let Some(rest) = value.strip_prefix("<leader>") {
        return parse_key(rest.trim()).map(Chord::Leader);
    }
    parse_key(&value).map(Chord::Key)
}

fn parse_key(value: &str) -> Option<Key> {
    let mut modifiers = KeyModifiers::NONE;
    let mut parts: Vec<&str> = value.split('+').collect();
    // `ctrl++` binds the plus key.
    if value.ends_with("++") {
        parts.pop();
        parts.pop();
        parts.push("+");
    }
    let name = parts.pop()?;
    for part in parts {
        modifiers |= match part {
            "ctrl" | "control" => KeyModifiers::CONTROL,
            "alt" | "option" | "meta" => KeyModifiers::ALT,
            "shift" => KeyModifiers::SHIFT,
            _ => return None,
        };
    }
    let code = match name {
        "enter" | "return" => KeyCode::Enter,
        "esc" | "escape" => KeyCode::Esc,
        "tab" if modifiers.contains(KeyModifiers::SHIFT) => KeyCode::BackTab,
        "tab" => KeyCode::Tab,
        "up" => KeyCode::Up,
        "down" => KeyCode::Down,
        "left" => KeyCode::Left,
        "right" => KeyCode::Right,
        "home" => KeyCode::Home,
        "end" => KeyCode::End,
        "pageup" => KeyCode::PageUp,
        "pagedown" => KeyCode::PageDown,
        "backspace" => KeyCode::Backspace,
        "delete" => KeyCode::Delete,
        "space" => KeyCode::Char(' '),
        f if f.len() > 1 && f.starts_with('f') => KeyCode::F(f[1..].parse().ok()?),
        c if c.chars().count() == 1 => KeyCode::Char(c.chars().next()?),
        _ => return None,
    };
    Some(Key { code, modifiers })
}

#[derive(Debug, Clone, PartialEq)]
struct Binding {
    context: &'static str,
    action: &'static str,
    description: &'static str,
    chord: Chord,
}

/// The merged table.
#[derive(Debug, Clone, PartialEq)]
pub struct Keymap {
    bindings: Vec<Binding>,
}

impl Default for Keymap {
    fn default() -> Self {
        Self {
            bindings: DEFAULTS
                .iter()
                .map(|&(context, action, default, description)| Binding {
                    context,
                    action,
                    description,
                    chord: parse(default).expect("default bindings parse"),
                })
                .collect(),
        }
    }
}

impl Keymap {
    /// The defaults with `tui.keybinds` merged over them. Top-level string values bind
    /// global actions; object values bind the actions of the context they are named
    /// after. Problems are pushed to `warnings`.
    pub fn load(config: Option<&Value>, warnings: &mut Vec<String>) -> Self {
        let mut map = Self::default();
        let Some(config) = config else {
            return map;
        };
        let Some(entries) = config.as_object() else {
            warnings.push("tui.keybinds: not an object".into());
            return map;
        };
        for (name, value) in entries {
            match value {
                Value::Object(scoped) if CONTEXTS.contains(&name.as_str()) => {
                    for (action, value) in scoped {
                        map.set(name, action, value, warnings);
                    }
                }
                Value::Object(_) => warnings.push(format!("tui.keybinds: unknown context {name}")),
                value => map.set("global", name, value, warnings),
            }
        }
        map
    }

    fn set(&mut self, context: &str, action: &str, value: &Value, warnings: &mut Vec<String>) {
        let Some(binding) = self
            .bindings
            .iter_mut()
            .find(|b| b.context == context && b.action == action)
        else {
            warnings.push(format!("tui.keybinds: unknown binding {context}.{action}"));
            return;
        };
        let leader_itself = binding.action == "leader";
        match value.as_str().and_then(parse) {
            Some(Chord::Leader(_)) if leader_itself => {
                warnings.push("tui.keybinds.leader: the leader cannot use itself".into())
            }
            Some(chord) => binding.chord = chord,
            None => warnings.push(format!(
                "tui.keybinds: {context}.{action} has an invalid key; the default stays"
            )),
        }
    }

    fn chord(&self, context: &str, action: &str) -> Chord {
        self.bindings
            .iter()
            .find(|b| b.context == context && b.action == action)
            .map_or(Chord::Disabled, |b| b.chord)
    }

    fn leader(&self) -> Option<Key> {
        match self.chord("global", "leader") {
            Chord::Key(key) => Some(key),
            _ => None,
        }
    }

    /// The key starts a leader chord.
    pub fn is_leader(&self, key: &KeyEvent) -> bool {
        self.leader().is_some_and(|leader| leader.matches(key))
    }

    /// The `context` action `key` triggers; `after_leader` when the leader came first.
    pub fn action(
        &self,
        context: &str,
        key: &KeyEvent,
        after_leader: bool,
    ) -> Option<&'static str> {
        self.bindings
            .iter()
            .filter(|b| b.context == context && b.action != "leader")
            .find(|b| match b.chord {
                Chord::Key(k) => !after_leader && k.matches(key),
                Chord::Leader(k) => after_leader && k.matches(key),
                Chord::Disabled => false,
            })
            .map(|b| b.action)
    }

    /// The key a hint shows for an action (`ctrl+p`, `ctrl+x b`); `None` when disabled.
    pub fn label(&self, context: &str, action: &str) -> Option<String> {
        match self.chord(context, action) {
            Chord::Key(key) => Some(key.label()),
            Chord::Leader(key) => Some(format!("{} {}", self.leader()?.label(), key.label())),
            Chord::Disabled => None,
        }
    }

    /// `(key, description)` of one enabled binding, for hints; `None` when disabled.
    pub fn hint(&self, context: &str, action: &str) -> Option<(String, &'static str)> {
        let binding = self
            .bindings
            .iter()
            .find(|b| b.context == context && b.action == action)?;
        Some((self.label(context, action)?, binding.description))
    }

    /// Enabled global bindings except `hidden` actions, for the help screen:
    /// `(key, description)`.
    pub fn help_rows(&self, hidden: &[&str]) -> Vec<(String, &'static str)> {
        self.bindings
            .iter()
            .filter(|b| b.context == "global" && !hidden.contains(&b.action))
            .filter_map(|b| Some((self.label(b.context, b.action)?, b.description)))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ctrl(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    #[test]
    fn defaults_cover_palette_sidebar_and_settings() {
        let map = Keymap::default();
        assert_eq!(
            map.label("global", "command_palette").as_deref(),
            Some("ctrl+p")
        );
        assert_eq!(
            map.label("global", "sidebar_toggle").as_deref(),
            Some("ctrl+x b")
        );
        assert_eq!(
            map.label("global", "sidebar_focus").as_deref(),
            Some("ctrl+x e")
        );
        assert_eq!(map.label("global", "settings").as_deref(), Some("ctrl+x ,"));
        assert_eq!(
            map.action("global", &ctrl('p'), false),
            Some("command_palette")
        );
        let b = KeyEvent::new(KeyCode::Char('b'), KeyModifiers::NONE);
        assert_eq!(map.action("global", &b, true), Some("sidebar_toggle"));
        assert_eq!(map.action("global", &b, false), None);
        assert!(map.is_leader(&ctrl('x')));
    }

    #[test]
    fn config_rebinds_disables_and_reports_mistakes() {
        let mut warnings = Vec::new();
        let config = json!({
            "command_palette": "ctrl+k",
            "leader": "ctrl+space",
            "help": "none",
            "palette": { "palette_close": "ctrl+g" },
            "sidebar": {},
            "settings": "F2",
            "model_list": "hyper+m",
            "bogus": "ctrl+b",
            "nowhere": { "x": "y" },
        });
        let map = Keymap::load(Some(&config), &mut warnings);
        assert_eq!(
            map.label("global", "command_palette").as_deref(),
            Some("ctrl+k")
        );
        assert_eq!(
            map.action("global", &ctrl('k'), false),
            Some("command_palette")
        );
        assert_eq!(map.action("global", &ctrl('p'), false), None);
        assert_eq!(
            map.label("global", "help"),
            None,
            "disabled bindings have no hint"
        );
        assert!(map.help_rows(&[]).iter().all(|(_, d)| *d != "keys"));
        assert_eq!(
            map.label("global", "session_new").as_deref(),
            Some("ctrl+space n")
        );
        assert_eq!(map.label("global", "settings").as_deref(), Some("f2"));
        assert_eq!(
            map.label("global", "model_list").as_deref(),
            Some("ctrl+space m")
        );
        assert_eq!(
            map.action("palette", &ctrl('g'), false),
            Some("palette_close")
        );
        assert_eq!(warnings.len(), 3, "{warnings:?}");
    }

    /// Regression: contexts with no bindings were accepted silently.
    #[test]
    fn composer_bindings_load_and_contexts_without_bindings_are_reported() {
        let mut warnings = Vec::new();
        let config = json!({
            "composer": { "history_previous": "none" },
            "explorer": { "explorer_copy": "c" },
            "messages": { "scroll": "k" },
        });
        let map = Keymap::load(Some(&config), &mut warnings);
        let up = KeyEvent::new(KeyCode::Up, KeyModifiers::NONE);
        assert_eq!(map.action("composer", &up, false), None);
        assert_eq!(
            Keymap::default().action("composer", &up, false),
            Some("history_previous")
        );
        assert_eq!(map.label("explorer", "explorer_copy").as_deref(), Some("c"));
        assert_eq!(warnings, ["tui.keybinds: unknown context messages"]);
    }

    #[test]
    fn a_leader_bound_to_a_chord_is_refused() {
        let mut warnings = Vec::new();
        let map = Keymap::load(Some(&json!({ "leader": "<leader>x" })), &mut warnings);
        assert!(map.is_leader(&ctrl('x')));
        assert_eq!(warnings.len(), 1);
    }
}
