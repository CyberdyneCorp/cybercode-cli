//! Shared key-chord policy (design F8). Runs during action validation, before
//! controller admission and before any journal intent. Native adapters keep their
//! own check as a second line. Layout safety stays in the adapters.

use crate::{Error, KeyChord, Modifier, ReasonCode, backend::SourceKind};

/// Keys a modified chord may use under app or window scope. This is exactly the
/// list the macOS adapter shipped with (F8a).
pub const EDIT_KEYS: &[&str] = &[
    "a",
    "z",
    "y",
    "s",
    "left",
    "right",
    "up",
    "down",
    "home",
    "end",
    "backspace",
    "delete",
];

/// Chords refused in every scope on platforms added after macOS: secure-attention,
/// session, task switching, launcher and quit chords. A chord matches when it uses
/// at least these modifiers with this key. Chords with the system modifier are
/// refused separately.
const GLOBAL_DENY: &[(&[Modifier], &str)] = &[
    (&[Modifier::Control, Modifier::Alt], "delete"),
    (&[Modifier::Control, Modifier::Alt], "backspace"),
    (&[Modifier::Control, Modifier::Shift], "escape"),
    (&[Modifier::Control], "escape"),
    (&[Modifier::Control, Modifier::Alt], "l"),
    (&[Modifier::Control, Modifier::Alt], "t"),
    (&[Modifier::Alt], "tab"),
    (&[Modifier::Alt], "escape"),
    (&[Modifier::Alt], "space"),
    (&[Modifier::Control], "q"),
];

/// Logical modifier roles of a platform.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    /// The foundation platform: rules reproduce the shipped native adapter.
    Macos,
    Windows,
    Linux,
}

impl Platform {
    /// Unknown platforms get the stricter rules of platforms added after macOS.
    pub fn from_name(name: &str) -> Self {
        match name {
            "macos" => Self::Macos,
            "windows" => Self::Windows,
            _ => Self::Linux,
        }
    }

    /// The primary editing modifier (Command on macOS, Control elsewhere).
    pub fn primary(self) -> Modifier {
        match self {
            Self::Macos => Modifier::Command,
            Self::Windows | Self::Linux => Modifier::Control,
        }
    }

    /// The shell-global modifier (Control on macOS, Windows/Super key elsewhere).
    pub fn system(self) -> Modifier {
        match self {
            Self::Macos => Modifier::Control,
            Self::Windows | Self::Linux => Modifier::Command,
        }
    }

    fn later(self) -> bool {
        self != Self::Macos
    }
}

/// App and window grants are scoped; display and output grants cover a whole screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    Scoped,
    Whole,
}

impl Scope {
    pub fn of(kind: SourceKind) -> Self {
        match kind {
            SourceKind::App | SourceKind::Window => Self::Scoped,
            SourceKind::Display | SourceKind::Output => Self::Whole,
        }
    }
}

fn refused() -> Error {
    Error::Refused(
        ReasonCode::KeyChordRefused,
        "this key chord is not allowed for the armed target",
    )
}

/// Evaluates the rules in order; the first refusal wins. Grammar errors keep their
/// existing input error; policy refusals return `key_chord_refused`.
pub fn evaluate(platform: Platform, scope: Scope, chord: &KeyChord) -> Result<(), Error> {
    chord.validate()?;
    let key = chord.key.to_ascii_lowercase();
    if platform.later() && globally_denied(platform, chord, &key) {
        return Err(refused());
    }
    if scope == Scope::Scoped && !scoped_allowed(platform, chord, &key) {
        return Err(refused());
    }
    Ok(())
}

fn globally_denied(platform: Platform, chord: &KeyChord, key: &str) -> bool {
    chord.modifiers.contains(&platform.system())
        || GLOBAL_DENY.iter().any(|(modifiers, denied)| {
            *denied == key
                && modifiers
                    .iter()
                    .all(|modifier| chord.modifiers.contains(modifier))
        })
}

fn scoped_allowed(platform: Platform, chord: &KeyChord, key: &str) -> bool {
    if chord.modifiers.contains(&platform.system()) {
        return false;
    }
    if platform.later() && chord.modifiers.contains(&Modifier::Alt) {
        return false;
    }
    chord.modifiers.is_empty() || EDIT_KEYS.contains(&key)
}

/// Every chord the grammar accepts, for table-driven tests: each ordered-free
/// modifier subset with every single letter/digit (both cases) and named key.
pub fn grammar_chords() -> Vec<KeyChord> {
    const MODIFIERS: [Modifier; 4] = [
        Modifier::Shift,
        Modifier::Control,
        Modifier::Alt,
        Modifier::Command,
    ];
    const NAMED: [&str; 14] = [
        "enter",
        "escape",
        "tab",
        "space",
        "backspace",
        "delete",
        "left",
        "right",
        "up",
        "down",
        "home",
        "end",
        "page_up",
        "page_down",
    ];
    let singles = (b'a'..=b'z')
        .chain(b'A'..=b'Z')
        .chain(b'0'..=b'9')
        .map(|byte| char::from(byte).to_string());
    let keys: Vec<String> = singles
        .chain(NAMED.iter().map(|name| (*name).to_owned()))
        .collect();
    let mut chords = Vec::new();
    for mask in 0u8..16 {
        let modifiers: Vec<Modifier> = MODIFIERS
            .iter()
            .enumerate()
            .filter(|(bit, _)| mask & (1 << bit) != 0)
            .map(|(_, modifier)| *modifier)
            .collect();
        for key in &keys {
            chords.push(KeyChord {
                modifiers: modifiers.clone(),
                key: key.clone(),
            });
        }
    }
    chords
}

/// The macOS adapter's rule as shipped, written independently of `evaluate`:
/// display scope adds nothing; app scope refuses Control and allows any modifier
/// only with an edit key.
pub fn shipped_macos_rule(app_scope: bool, chord: &KeyChord) -> bool {
    if !app_scope {
        return true;
    }
    const SHIPPED: [&str; 12] = [
        "a",
        "z",
        "y",
        "s",
        "left",
        "right",
        "up",
        "down",
        "home",
        "end",
        "backspace",
        "delete",
    ];
    let key = chord.key.to_ascii_lowercase();
    !chord.modifiers.contains(&Modifier::Control)
        && (chord.modifiers.is_empty() || SHIPPED.contains(&key.as_str()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chord(modifiers: &[Modifier], key: &str) -> KeyChord {
        KeyChord {
            modifiers: modifiers.to_vec(),
            key: key.into(),
        }
    }
    fn allowed(platform: Platform, scope: Scope, modifiers: &[Modifier], key: &str) -> bool {
        evaluate(platform, scope, &chord(modifiers, key)).is_ok()
    }

    #[test]
    fn macos_rows_match_the_shipped_rule_for_every_grammar_chord() {
        let chords = grammar_chords();
        assert_eq!(chords.len(), 16 * (26 + 26 + 10 + 14));
        let mut refused = 0;
        for chord in &chords {
            for (scope, app) in [(Scope::Scoped, true), (Scope::Whole, false)] {
                let rust = evaluate(Platform::Macos, scope, chord);
                assert_eq!(
                    rust.is_ok(),
                    shipped_macos_rule(app, chord),
                    "{chord:?} app={app}"
                );
                if let Err(error) = rust {
                    refused += 1;
                    assert_eq!(error.reason(), Some(ReasonCode::KeyChordRefused));
                }
            }
        }
        assert!(refused > 0);
    }

    #[test]
    fn clipboard_chords_are_refused_under_app_or_window_scope_on_every_platform() {
        for platform in [Platform::Macos, Platform::Windows, Platform::Linux] {
            for key in ["c", "x", "v", "C"] {
                assert!(!allowed(
                    platform,
                    Scope::Scoped,
                    &[platform.primary()],
                    key
                ));
            }
            assert!(allowed(platform, Scope::Scoped, &[platform.primary()], "z"));
            assert!(allowed(platform, Scope::Scoped, &[platform.primary()], "s"));
            assert!(allowed(platform, Scope::Scoped, &[], "enter"));
            assert!(allowed(platform, Scope::Scoped, &[], "q"));
            assert!(!allowed(platform, Scope::Scoped, &[platform.system()], "a"));
        }
    }

    #[test]
    fn macos_undo_and_display_scope_are_unchanged() {
        assert!(allowed(
            Platform::Macos,
            Scope::Scoped,
            &[Modifier::Command],
            "z"
        ));
        assert!(allowed(
            Platform::Macos,
            Scope::Scoped,
            &[Modifier::Alt],
            "left"
        ));
        assert!(allowed(
            Platform::Macos,
            Scope::Whole,
            &[Modifier::Command],
            "tab"
        ));
        assert!(allowed(
            Platform::Macos,
            Scope::Whole,
            &[Modifier::Control],
            "q"
        ));
        assert!(allowed(
            Platform::Macos,
            Scope::Whole,
            &[Modifier::Command],
            "space"
        ));
    }

    #[test]
    fn later_platforms_refuse_global_chords_in_every_scope_and_alt_under_scoped_grants() {
        for platform in [Platform::Windows, Platform::Linux] {
            for scope in [Scope::Scoped, Scope::Whole] {
                for (modifiers, key) in [
                    (&[Modifier::Control, Modifier::Alt][..], "delete"),
                    (
                        &[Modifier::Control, Modifier::Shift, Modifier::Alt],
                        "delete",
                    ),
                    (&[Modifier::Control, Modifier::Shift], "escape"),
                    (&[Modifier::Control], "escape"),
                    (&[Modifier::Alt], "tab"),
                    (&[Modifier::Alt, Modifier::Shift], "tab"),
                    (&[Modifier::Command], "tab"),
                    (&[Modifier::Command], "space"),
                    (&[Modifier::Command], "l"),
                    (&[Modifier::Control], "q"),
                ] {
                    assert!(
                        !allowed(platform, scope, modifiers, key),
                        "{platform:?} {scope:?} {modifiers:?}+{key}"
                    );
                }
                assert!(allowed(platform, scope, &[], "tab"));
                assert!(allowed(platform, scope, &[Modifier::Control], "a"));
            }
            assert!(!allowed(platform, Scope::Scoped, &[Modifier::Alt], "left"));
            assert!(allowed(platform, Scope::Whole, &[Modifier::Alt], "left"));
            assert!(allowed(platform, Scope::Whole, &[Modifier::Control], "c"));
        }
    }

    #[test]
    fn grammar_errors_keep_their_input_error_and_unknown_platforms_are_strict() {
        let error = evaluate(Platform::Macos, Scope::Whole, &chord(&[], "f4")).unwrap_err();
        assert!(matches!(error, Error::Input(_)));
        assert_eq!(Platform::from_name("macos"), Platform::Macos);
        assert_eq!(Platform::from_name("windows"), Platform::Windows);
        assert_eq!(Platform::from_name("haiku"), Platform::Linux);
        assert_eq!(Scope::of(SourceKind::Window), Scope::Scoped);
        assert_eq!(Scope::of(SourceKind::Output), Scope::Whole);
    }
}
