//! Themes resolved to semantic color tokens (`tui` → Theme tokens, Themes and
//! appearance, Color capability and NO_COLOR).
//!
//! Built-in themes are the same JSON documents as custom `themes/*.json` files, so both
//! take one path: theme value → `defs` reference → `{dark, light}` pick → derivation
//! rule → color depth.

pub mod color;
pub mod detect;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use ratatui::style::{Color, Modifier, Style};
use serde_json::Value;

pub use color::Depth;
pub use detect::ThemeMode;

macro_rules! tokens {
    ($($field:ident),* $(,)?) => {
        /// Every color a view may use, resolved for one theme, variant and depth.
        #[derive(Debug, Clone, PartialEq)]
        pub struct Tokens {
            pub name: String,
            /// The light variant (or a light-background theme) was resolved.
            pub light: bool,
            /// No colors at all: views convey state with attributes and glyphs.
            pub monochrome: bool,
            $(pub $field: Color,)*
        }

        /// Token names, in the order of [`Tokens`]' fields.
        pub const TOKEN_NAMES: &[&str] = &[$(stringify!($field)),*];

        impl Tokens {
            fn from_slots(name: String, light: bool, monochrome: bool, slots: &[Color]) -> Self {
                let mut slots = slots.iter().copied();
                Self {
                    name,
                    light,
                    monochrome,
                    $($field: slots.next().unwrap_or(Color::Reset),)*
                }
            }
        }
    };
}

tokens! {
    background, surface, surface_alt, border, border_active, selection,
    text, text_muted, text_subtle,
    accent, secondary, user, success, warning, error, info,
    diff_added, diff_removed, diff_added_bg, diff_removed_bg, diff_context,
    md_heading, md_link, md_code, md_quote, md_rule,
    syntax_comment, syntax_keyword, syntax_function, syntax_variable, syntax_string,
    syntax_number, syntax_type, syntax_operator, syntax_punctuation,
    heat_0, heat_1, heat_2, heat_3, heat_4, meter_fill, meter_empty,
    file_dir, file_doc, file_config, file_code, file_dim,
}

impl Tokens {
    /// A selected row or tab: `selection` behind `accent`, or reverse video without color.
    pub fn selected(&self) -> Style {
        if self.monochrome {
            Style::default().add_modifier(Modifier::REVERSED | Modifier::BOLD)
        } else {
            Style::default()
                .fg(self.accent)
                .bg(self.selection)
                .add_modifier(Modifier::BOLD)
        }
    }
}

/// How a token missing from a theme is derived.
enum Rule {
    /// The first token moved this far toward the second.
    Blend(&'static str, &'static str, f32),
    Same(&'static str),
    /// Fixed `(dark, light)` colors.
    Fixed(u32, u32),
}

/// `text`, `background` and `accent` when a theme leaves them out: the built-in
/// `cyber` values for the variant.
const BASE: [(&str, u32, u32); 3] = [
    ("background", 0x0b0f14, 0xffffff),
    ("text", 0xd6e4f0, 0x1f2933),
    ("accent", 0x00e5ff, 0x00799e),
];

/// Derivation rules, in dependency order.
const RULES: &[(&str, Rule)] = &[
    ("surface", Rule::Blend("background", "text", 0.06)),
    ("surface_alt", Rule::Blend("background", "text", 0.04)),
    ("border", Rule::Blend("background", "text", 0.18)),
    ("border_active", Rule::Same("accent")),
    ("selection", Rule::Blend("background", "accent", 0.25)),
    ("text_muted", Rule::Blend("text", "background", 0.40)),
    ("text_subtle", Rule::Blend("text", "background", 0.55)),
    ("secondary", Rule::Blend("accent", "text", 0.40)),
    ("user", Rule::Same("accent")),
    ("success", Rule::Fixed(0x7ee787, 0x1a7f37)),
    ("warning", Rule::Fixed(0xffcc66, 0x9a6700)),
    ("error", Rule::Fixed(0xff6b6b, 0xcf222e)),
    ("info", Rule::Same("accent")),
    ("diff_added", Rule::Same("success")),
    ("diff_removed", Rule::Same("error")),
    ("diff_added_bg", Rule::Blend("background", "success", 0.15)),
    ("diff_removed_bg", Rule::Blend("background", "error", 0.15)),
    ("diff_context", Rule::Same("text_muted")),
    ("md_heading", Rule::Same("accent")),
    ("md_link", Rule::Same("info")),
    ("md_code", Rule::Same("secondary")),
    ("md_quote", Rule::Same("text_muted")),
    ("md_rule", Rule::Same("border")),
    ("syntax_comment", Rule::Same("text_muted")),
    ("syntax_keyword", Rule::Same("secondary")),
    ("syntax_function", Rule::Same("accent")),
    ("syntax_variable", Rule::Same("text")),
    ("syntax_string", Rule::Same("success")),
    ("syntax_number", Rule::Same("warning")),
    ("syntax_type", Rule::Same("info")),
    ("syntax_operator", Rule::Same("text")),
    ("syntax_punctuation", Rule::Same("text_muted")),
    ("heat_0", Rule::Same("surface")),
    ("heat_1", Rule::Blend("background", "accent", 0.25)),
    ("heat_2", Rule::Blend("background", "accent", 0.50)),
    ("heat_3", Rule::Blend("background", "accent", 0.75)),
    ("heat_4", Rule::Same("accent")),
    ("meter_fill", Rule::Same("accent")),
    ("meter_empty", Rule::Same("border")),
    ("file_dir", Rule::Blend("user", "text", 0.25)),
    ("file_doc", Rule::Blend("warning", "text", 0.25)),
    ("file_config", Rule::Blend("success", "text", 0.25)),
    ("file_code", Rule::Blend("secondary", "text", 0.25)),
    ("file_dim", Rule::Same("text_subtle")),
];

fn index(name: &str) -> usize {
    TOKEN_NAMES
        .iter()
        .position(|n| *n == name)
        .expect("token name")
}

const fn rgb(hex: u32) -> Color {
    Color::Rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

/// A parsed theme document: each token's `(dark, light)` value.
#[derive(Debug, Clone, PartialEq)]
pub struct ThemeDoc {
    pub name: String,
    values: BTreeMap<String, (Color, Color)>,
    /// Some value differs between `dark` and `light`.
    variants: bool,
    /// Loaded from a `themes/*.json` file rather than built in.
    pub custom: bool,
}

impl ThemeDoc {
    /// Parse `{ "defs"?, "theme": { token: value } }`. Unknown tokens and invalid values
    /// are skipped with a warning, so the token is derived instead.
    pub fn parse(name: &str, text: &str, warnings: &mut Vec<String>) -> Result<Self, String> {
        let doc: Value = serde_json::from_str(text).map_err(|e| format!("theme {name}: {e}"))?;
        let defs = doc["defs"].as_object().cloned().unwrap_or_default();
        let theme = doc["theme"]
            .as_object()
            .ok_or_else(|| format!("theme {name}: missing \"theme\" object"))?;
        let mut values = BTreeMap::new();
        let mut variants = false;
        for (token, value) in theme {
            if !TOKEN_NAMES.contains(&token.as_str()) {
                warnings.push(format!("theme {name}: unknown token {token}"));
                continue;
            }
            let pair = match value {
                Value::Object(v) => (
                    reference(&defs, &v.get("dark").cloned().unwrap_or_default()),
                    reference(&defs, &v.get("light").cloned().unwrap_or_default()),
                ),
                other => (reference(&defs, other), reference(&defs, other)),
            };
            let (Some(dark), Some(light)) = pair else {
                warnings.push(format!("theme {name}: invalid value for {token}"));
                continue;
            };
            variants |= dark != light;
            values.insert(token.clone(), (dark, light));
        }
        Ok(Self {
            name: name.to_owned(),
            values,
            variants,
            custom: false,
        })
    }

    /// Whether the theme has distinct dark and light variants.
    #[cfg(test)]
    pub fn has_variants(&self) -> bool {
        self.variants
    }

    fn get(&self, token: &str, light: bool) -> Option<Color> {
        self.values
            .get(token)
            .map(|(dark, l)| if light { *l } else { *dark })
    }

    /// The variant to render: the terminal's for a two-variant theme, else the one
    /// matching the theme's own background.
    fn light(&self, light_background: bool) -> bool {
        if self.variants {
            return light_background;
        }
        self.get("background", false)
            .and_then(color::luminance)
            .map_or(light_background, |l| l > 0.5)
    }

    /// The tokens for a terminal background and color depth.
    pub fn resolve(&self, light_background: bool, depth: Depth) -> Tokens {
        let light = self.light(light_background);
        let mut slots: Vec<Option<Color>> =
            TOKEN_NAMES.iter().map(|t| self.get(t, light)).collect();
        for (token, dark, l) in BASE {
            slots[index(token)].get_or_insert(rgb(if light { l } else { dark }));
        }
        for (token, rule) in RULES {
            if slots[index(token)].is_some() {
                continue;
            }
            let get = |name: &str| slots[index(name)].unwrap_or(Color::Reset);
            let value = match rule {
                Rule::Blend(from, toward, amount) => color::blend(get(from), get(toward), *amount),
                Rule::Same(other) => get(other),
                Rule::Fixed(dark, l) => rgb(if light { *l } else { *dark }),
            };
            slots[index(token)] = Some(value);
        }
        let colors: Vec<Color> = slots
            .into_iter()
            .map(|c| color::reduce(c.unwrap_or(Color::Reset), depth))
            .collect();
        Tokens::from_slots(self.name.clone(), light, depth == Depth::None, &colors)
    }
}

/// A theme value: a `defs` name or a literal color.
fn reference(defs: &serde_json::Map<String, Value>, value: &Value) -> Option<Color> {
    let text = value.as_str()?;
    let literal = defs.get(text).and_then(Value::as_str).unwrap_or(text);
    color::parse(literal)
}

/// The built-in themes, compiled in as JSON.
pub const BUILTIN: [(&str, &str); 18] = [
    ("cyber", include_str!("../../themes/cyber.json")),
    ("system", include_str!("../../themes/system.json")),
    ("dracula", include_str!("../../themes/dracula.json")),
    ("tokyonight", include_str!("../../themes/tokyonight.json")),
    ("catppuccin", include_str!("../../themes/catppuccin.json")),
    ("gruvbox", include_str!("../../themes/gruvbox.json")),
    ("nord", include_str!("../../themes/nord.json")),
    ("one-dark", include_str!("../../themes/one-dark.json")),
    (
        "github-light",
        include_str!("../../themes/github-light.json"),
    ),
    (
        "solarized-light",
        include_str!("../../themes/solarized-light.json"),
    ),
    ("monokai", include_str!("../../themes/monokai.json")),
    ("everforest", include_str!("../../themes/everforest.json")),
    ("cyber-light", include_str!("../../themes/cyber-light.json")),
    (
        "synthwave-84",
        include_str!("../../themes/synthwave-84.json"),
    ),
    ("rose-pine", include_str!("../../themes/rose-pine.json")),
    ("kanagawa", include_str!("../../themes/kanagawa.json")),
    ("ayu-mirage", include_str!("../../themes/ayu-mirage.json")),
    ("matrix", include_str!("../../themes/matrix.json")),
];

/// Custom theme files larger than this are refused.
const MAX_THEME_BYTES: u64 = 64 * 1024;

/// Every theme by name: built-ins first, then custom ones; a later directory's
/// `<name>.json` replaces an earlier one or a built-in of that name.
#[derive(Debug, Clone)]
pub struct Catalog {
    docs: Vec<ThemeDoc>,
    pub warnings: Vec<String>,
}

impl Default for Catalog {
    fn default() -> Self {
        Self::load(&[])
    }
}

impl Catalog {
    /// Built-ins plus `<dir>/*.json` from each directory, lowest precedence first.
    pub fn load(dirs: &[PathBuf]) -> Self {
        let mut catalog = Self {
            docs: Vec::new(),
            warnings: Vec::new(),
        };
        for (name, text) in BUILTIN {
            let doc =
                ThemeDoc::parse(name, text, &mut catalog.warnings).expect("built-in themes parse");
            catalog.insert(doc);
        }
        for dir in dirs {
            catalog.load_dir(dir);
        }
        catalog
    }

    fn load_dir(&mut self, dir: &Path) {
        let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
            .collect();
        files.sort();
        for path in files {
            match read_theme(&path, &mut self.warnings) {
                Ok(doc) => self.insert(ThemeDoc {
                    custom: true,
                    ..doc
                }),
                Err(error) => self.warnings.push(error),
            }
        }
    }

    fn insert(&mut self, doc: ThemeDoc) {
        match self.docs.iter_mut().find(|d| d.name == doc.name) {
            Some(existing) => *existing = doc,
            None => self.docs.push(doc),
        }
    }

    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.docs.iter().map(|d| d.name.as_str())
    }

    /// The named theme, or `cyber` when there is none.
    pub fn get(&self, name: &str) -> &ThemeDoc {
        self.docs
            .iter()
            .find(|d| d.name == name)
            .unwrap_or(&self.docs[0])
    }
}

fn read_theme(path: &Path, warnings: &mut Vec<String>) -> Result<ThemeDoc, String> {
    let shown = path.display();
    let name = path
        .file_stem()
        .and_then(|s| s.to_str())
        .filter(|s| {
            !s.is_empty()
                && s.chars()
                    .all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))
        })
        .ok_or_else(|| format!("theme {shown}: the file name is not a theme name"))?;
    // A project's theme may be a FIFO or a link to `/dev/zero`: the read is bounded.
    let text = cyber_core::files::read_small(path, MAX_THEME_BYTES)
        .map_err(|e| format!("theme {shown}: {e}"))?;
    ThemeDoc::parse(name, &text, warnings)
}

/// How themes are rendered on this terminal.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Appearance {
    pub depth: Depth,
    pub mode: ThemeMode,
    /// The detected terminal background (`None` until something answers).
    pub detected_light: Option<bool>,
}

impl Default for Appearance {
    fn default() -> Self {
        Self {
            depth: Depth::TrueColor,
            mode: ThemeMode::Auto,
            detected_light: None,
        }
    }
}

impl Appearance {
    /// Whether to render light variants: `tui.theme_mode`, else the detected
    /// background, else dark.
    pub fn light(&self) -> bool {
        match self.mode {
            ThemeMode::Dark => false,
            ThemeMode::Light => true,
            ThemeMode::Auto => self.detected_light.unwrap_or(false),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn builtin(name: &str) -> ThemeDoc {
        Catalog::default().get(name).clone()
    }

    /// Each variant a built-in renders: both for a two-variant theme, else its own.
    fn variants() -> Vec<Tokens> {
        let catalog = Catalog::default();
        let mut out = Vec::new();
        for name in catalog.names() {
            let doc = catalog.get(name);
            out.push(doc.resolve(false, Depth::TrueColor));
            if doc.has_variants() {
                out.push(doc.resolve(true, Depth::TrueColor));
            }
        }
        out
    }

    #[test]
    fn every_builtin_theme_and_variant_meets_the_contrast_minimums() {
        let mut checked = 0;
        let mut low = Vec::new();
        for t in variants() {
            if color::luminance(t.background).is_none() {
                // `system` uses the terminal's own palette, which cannot be measured.
                assert_eq!(t.name, "system");
                continue;
            }
            let ratio = |c| color::contrast(c, t.background).unwrap();
            let variant = if t.light { "light" } else { "dark" };
            assert!(
                ratio(t.text) >= 4.5,
                "{} {variant} text {}",
                t.name,
                ratio(t.text)
            );
            for (token, c) in [
                ("text_muted", t.text_muted),
                ("border_active", t.border_active),
            ] {
                assert!(ratio(c) >= 3.0, "{} {variant} {token} {}", t.name, ratio(c));
            }
            // File kinds sit on the explorer's `surface_alt` tree pane.
            let pane = |c| color::contrast(c, t.surface_alt).unwrap();
            for (token, c) in [
                ("file_dir", t.file_dir),
                ("file_doc", t.file_doc),
                ("file_config", t.file_config),
                ("file_code", t.file_code),
            ] {
                if pane(c) < 3.0 {
                    low.push(format!("{} {variant} {token} {:.2}", t.name, pane(c)));
                }
            }
            checked += 1;
        }
        assert!(
            low.is_empty(),
            "file kinds below 3:1 on the tree pane: {low:?}"
        );
        assert_eq!(checked, 18, "17 RGB themes, cyber in two variants");
    }

    #[test]
    fn every_derived_token_has_a_rule_and_builtins_keep_their_names() {
        for name in TOKEN_NAMES {
            let based = BASE.iter().any(|(t, ..)| t == name);
            let ruled = RULES.iter().filter(|(t, _)| t == name).count();
            assert_eq!(usize::from(based) + ruled, 1, "{name}");
        }
        let names: Vec<String> = Catalog::default().names().map(str::to_owned).collect();
        assert_eq!(names.len(), 18);
        for name in [
            "cyber",
            "system",
            "dracula",
            "github-light",
            "solarized-light",
            "cyber-light",
            "synthwave-84",
            "rose-pine",
            "kanagawa",
            "ayu-mirage",
            "matrix",
        ] {
            assert!(names.iter().any(|n| n == name), "{name}");
        }
        assert!(builtin("cyber").has_variants());
        assert_eq!(Catalog::default().get("missing").name, "cyber");
    }

    #[test]
    fn cyber_light_is_the_cyber_light_variant_as_its_own_theme() {
        let own = builtin("cyber-light").resolve(false, Depth::TrueColor);
        let variant = builtin("cyber").resolve(true, Depth::TrueColor);
        assert!(own.light);
        for (a, b) in [
            (own.background, variant.background),
            (own.text, variant.text),
            (own.text_muted, variant.text_muted),
            (own.accent, variant.accent),
            (own.secondary, variant.secondary),
            (own.surface, variant.surface),
            (own.diff_added_bg, variant.diff_added_bg),
            (own.heat_4, variant.heat_4),
        ] {
            assert_eq!(a, b);
        }
    }

    #[test]
    fn a_partial_theme_derives_surfaces_muted_text_and_heat_levels() {
        let mut warnings = Vec::new();
        let doc = ThemeDoc::parse(
            "corp",
            r##"{"defs":{"ink":"#e0e0e0"},"theme":{"text":"ink","background":"#000000","accent":"#ff0000","bogus":"#123456","error":"red-ish"}}"##,
            &mut warnings,
        )
        .unwrap();
        assert_eq!(warnings.len(), 2, "{warnings:?}");
        let t = doc.resolve(true, Depth::TrueColor);
        assert!(!t.light, "a dark background renders dark fixed colors");
        assert_eq!(t.text, Color::Rgb(0xe0, 0xe0, 0xe0));
        assert_eq!(t.surface, Color::Rgb(13, 13, 13));
        assert_eq!(t.text_muted, Color::Rgb(134, 134, 134));
        assert_eq!(
            [t.heat_1, t.heat_2, t.heat_3, t.heat_4],
            [
                Color::Rgb(64, 0, 0),
                Color::Rgb(128, 0, 0),
                Color::Rgb(191, 0, 0),
                Color::Rgb(255, 0, 0)
            ]
        );
        assert_eq!(t.error, Color::Rgb(0xff, 0x6b, 0x6b));
        assert_eq!(t.border_active, t.accent);
    }

    #[test]
    fn no_color_resolves_every_token_to_the_terminal_default() {
        let t = builtin("dracula").resolve(false, Depth::None);
        assert!(t.monochrome);
        assert_eq!(t.background, Color::Reset);
        assert_eq!(t.accent, Color::Reset);
        assert!(t.selected().add_modifier.contains(Modifier::REVERSED));
        let t = builtin("dracula").resolve(false, Depth::Ansi256);
        assert!(matches!(t.text, Color::Indexed(_)));
    }

    #[test]
    fn the_light_variant_follows_the_mode_and_detected_background() {
        let mut appearance = Appearance::default();
        assert!(!appearance.light());
        appearance.detected_light = Some(true);
        assert!(appearance.light());
        appearance.mode = ThemeMode::Dark;
        assert!(!appearance.light());
        let cyber = builtin("cyber");
        assert_eq!(
            cyber.resolve(true, Depth::TrueColor).background,
            rgb(0xffffff)
        );
        let github = builtin("github-light").resolve(false, Depth::TrueColor);
        assert!(
            github.light,
            "a light-only theme stays light on a dark terminal"
        );
    }

    #[test]
    fn custom_themes_load_with_project_over_global_and_bad_files_skipped() {
        let root = tempfile::tempdir().unwrap();
        let (global, project) = (root.path().join("g"), root.path().join("p"));
        std::fs::create_dir_all(&global).unwrap();
        std::fs::create_dir_all(&project).unwrap();
        std::fs::write(
            global.join("corp.json"),
            r##"{"theme":{"accent":"#111111"}}"##,
        )
        .unwrap();
        std::fs::write(
            project.join("corp.json"),
            r##"{"theme":{"accent":"#222222"}}"##,
        )
        .unwrap();
        std::fs::write(project.join("broken.json"), "{").unwrap();
        std::fs::write(
            project.join("nord.json"),
            r##"{"theme":{"accent":"#333333"}}"##,
        )
        .unwrap();
        let catalog = Catalog::load(&[global, project]);
        assert_eq!(catalog.names().filter(|n| *n == "corp").count(), 1);
        assert_eq!(catalog.names().count(), 19);
        let corp = catalog.get("corp").resolve(false, Depth::TrueColor);
        assert_eq!(corp.accent, rgb(0x222222));
        assert_eq!(
            catalog.get("nord").resolve(false, Depth::TrueColor).accent,
            rgb(0x333333)
        );
        assert!(catalog.names().all(|n| n != "broken"));
        assert!(
            catalog.warnings.iter().any(|w| w.contains("broken")),
            "{:?}",
            catalog.warnings
        );
    }

    /// Regression: the size check used `metadata`, which reports 0 bytes for a device
    /// or a FIFO, and then read the whole file, so a project theme linked to `/dev/zero`
    /// exhausted memory and a FIFO blocked startup. The read itself is bounded now.
    #[test]
    fn oversized_and_special_theme_files_are_refused_promptly() {
        let dir = tempfile::tempdir().unwrap();
        let body = r##"{"theme":{"accent":"#123456"}}"##;
        let padding = " ".repeat(MAX_THEME_BYTES as usize);
        std::fs::write(dir.path().join("big.json"), format!("{body}{padding}")).unwrap();
        std::fs::write(dir.path().join("fine.json"), body).unwrap();
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink("/dev/zero", dir.path().join("zero.json")).unwrap();
            let made = std::process::Command::new("mkfifo")
                .arg(dir.path().join("fifo.json"))
                .status();
            assert!(made.unwrap().success());
        }
        let (done, finished) = std::sync::mpsc::channel();
        let at = dir.path().to_path_buf();
        std::thread::spawn(move || done.send(Catalog::load(&[at])).unwrap());
        let catalog = finished
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("loaded promptly");
        let names: Vec<&str> = catalog.names().collect();
        assert!(names.contains(&"fine"), "{names:?}");
        for refused in ["big", "zero", "fifo"] {
            assert!(!names.contains(&refused), "{refused}: {names:?}");
        }
        let warned = |text: &str| catalog.warnings.iter().any(|w| w.contains(text));
        assert!(warned("larger than"), "{:?}", catalog.warnings);
        #[cfg(unix)]
        assert!(warned("not a regular file"), "{:?}", catalog.warnings);
    }
}
