//! Output styles (`system-context` → Output style definitions).
//!
//! A style is text appended to the system prompt. The built-ins are embedded; custom
//! styles are Markdown files directly in the user's `~/.config/cyber/output-styles/`
//! and the project's `.cyber/output-styles/`, with optional YAML frontmatter (`name`,
//! `description`, and `keep-coding-instructions`, accepted and ignored). Project styles
//! are part of the workspace-trust digest ([`trust_subset`]) and load only while the
//! checkout is trusted; a project style replaces a user style of the same name. A file
//! that does not load is reported with its reason and never blocks the others. A style
//! carries no authority: any other frontmatter key makes the file invalid.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::{Map, Value};

/// The style that adds nothing to the system prompt.
pub const DEFAULT: &str = "default";
/// The issue of an untrusted checkout's project styles.
pub const HELD: &str =
    "project output styles are held until this checkout is trusted (cyber trust inspect)";

const DIRECTORY: &str = "output-styles";
const MAX_FILES: usize = 128;
const MAX_BYTES: u64 = 32 * 1024;
const MAX_NAME: usize = 64;
const FIELDS: [&str; 3] = ["name", "description", "keep-coding-instructions"];
const DEFAULT_DESCRIPTION: &str = "The base prompt alone, with no added instructions";
/// Built-in styles other than [`DEFAULT`], parsed like custom ones.
const BUILTINS: [(&str, &str); 3] = [
    (
        "concise",
        include_str!("../prompts/output-styles/concise.md"),
    ),
    (
        "explanatory",
        include_str!("../prompts/output-styles/explanatory.md"),
    ),
    (
        "learning",
        include_str!("../prompts/output-styles/learning.md"),
    ),
];

/// Where a style came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum StyleOrigin {
    Builtin,
    User,
    Project,
}

/// An available output style.
#[derive(Debug, Clone, PartialEq, Serialize, schemars::JsonSchema)]
pub struct OutputStyle {
    pub name: String,
    pub description: String,
    pub origin: StyleOrigin,
    /// The file of a user or project style.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<PathBuf>,
    /// Styles of the same name this one replaced (a user one under a project one).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub shadows: Vec<PathBuf>,
    /// The trimmed body; empty for [`DEFAULT`].
    #[serde(skip)]
    #[schemars(skip)]
    pub text: String,
}

/// A style file that did not load, and why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, schemars::JsonSchema)]
pub struct StyleIssue {
    pub path: PathBuf,
    pub message: String,
}

/// Every style of a Location: the built-ins first, then custom styles by name.
#[derive(Debug, Clone, Default, PartialEq, Serialize, schemars::JsonSchema)]
pub struct Styles {
    pub styles: Vec<OutputStyle>,
    pub issues: Vec<StyleIssue>,
    /// Names of project style files not read because the checkout is untrusted.
    #[serde(skip)]
    #[schemars(skip)]
    pub held: Vec<String>,
    /// Names of listed style files that could not be read (an I/O error).
    #[serde(skip)]
    #[schemars(skip)]
    pub unreadable: Vec<String>,
}

impl Styles {
    pub fn get(&self, name: &str) -> Option<&OutputStyle> {
        self.styles.iter().find(|style| style.name == name)
    }

    /// The style `name` selects, or why it is unavailable. The name is checked before
    /// any lookup, and a lookup never touches the file system.
    pub fn select(&self, name: &str) -> Result<&OutputStyle, String> {
        if !valid_name(name) {
            return Err(format!("invalid output style name \"{name}\""));
        }
        if let Some(style) = self.get(name) {
            return Ok(style);
        }
        if self.held.iter().any(|held| held == name) {
            return Err(format!(
                "output style \"{name}\" is held until this checkout is trusted (cyber trust inspect)"
            ));
        }
        let invalid = self
            .issues
            .iter()
            .find(|issue| stem(&issue.path) == Some(name));
        Err(match invalid {
            Some(issue) => format!("output style \"{name}\" is invalid: {}", issue.message),
            None => format!("unknown output style \"{name}\""),
        })
    }

    /// The `core/output-style` source text for the effective name: `Ok(None)` when it
    /// adds nothing (`default`, or a name that is invalid, unknown or held, so `default`
    /// applies), and `Err` when the style's listed file could not be read.
    pub fn prompt(&self, name: &str) -> Result<Option<String>, String> {
        match self.select(name) {
            Ok(style) if style.text.is_empty() => Ok(None),
            Ok(style) => Ok(Some(format!(
                "Output style: {}\n\n{}",
                style.name, style.text
            ))),
            Err(reason) if self.unreadable.iter().any(|n| n == name) => Err(reason),
            Err(_) => Ok(None),
        }
    }
}

/// Inputs of [`load`].
pub struct StyleSources<'a> {
    pub location: &'a Path,
    /// `~/.config/cyber`.
    pub global_config_dir: &'a Path,
    /// Workspace trust of the checkout; untrusted project styles are held back.
    pub project_trusted: bool,
    /// `CYBER_DISABLE_PROJECT_CONFIG` off.
    pub project_enabled: bool,
}

/// The configured `output_style` of a Location and the layer that set it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, schemars::JsonSchema)]
pub struct Configured {
    pub name: String,
    pub source: String,
}

/// The styles of a Location with its configured and effective style (`GET
/// /api/v1/config/output-styles`).
#[derive(Debug, Clone, PartialEq, Serialize, schemars::JsonSchema)]
pub struct StyleListing {
    #[serde(flatten)]
    pub styles: Styles,
    pub configured: Option<Configured>,
    /// The style a new Session there uses.
    pub effective: String,
    /// Why the configured style does not apply, when it does not.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unavailable: Option<String>,
}

/// The styles of a Location from its resolved configuration (trust from `resolved`).
pub fn styles(
    location: &Path,
    global_config_dir: &Path,
    resolved: &crate::config::Resolved,
    env: &dyn crate::env::EnvSource,
) -> Styles {
    load(&StyleSources {
        location,
        global_config_dir,
        project_trusted: resolved.trust.trusted,
        project_enabled: !env.flag("CYBER_DISABLE_PROJECT_CONFIG"),
    })
}

/// [`styles`] with the configured `output_style` and the style that applies.
pub fn listing(
    location: &Path,
    global_config_dir: &Path,
    resolved: &crate::config::Resolved,
    env: &dyn crate::env::EnvSource,
) -> StyleListing {
    let styles = styles(location, global_config_dir, resolved, env);
    let configured = resolved
        .value
        .get("output_style")
        .and_then(Value::as_str)
        .map(|name| Configured {
            name: name.to_owned(),
            source: resolved
                .sources
                .get("/output_style")
                .cloned()
                .unwrap_or_default(),
        });
    let selected = configured.as_ref().map(|c| styles.select(&c.name));
    let (effective, unavailable) = match selected {
        None => (DEFAULT.to_owned(), None),
        Some(Ok(style)) => (style.name.clone(), None),
        Some(Err(reason)) => (DEFAULT.to_owned(), Some(reason)),
    };
    StyleListing {
        styles,
        configured,
        effective,
        unavailable,
    }
}

/// Check that `name` may be the global `output_style`: a built-in or user style, since a
/// global default applies in every checkout.
pub fn global_default(global_config_dir: &Path, location: &Path, name: &str) -> Result<(), String> {
    let user = load(&StyleSources {
        location,
        global_config_dir,
        project_trusted: false,
        project_enabled: false,
    });
    match user.select(name) {
        Ok(_) => Ok(()),
        Err(_)
            if valid_name(name)
                && files(&project_dir(location))
                    .iter()
                    .any(|p| stem(p) == Some(name)) =>
        {
            Err(format!(
                "output style \"{name}\" is defined only by a project; the global default must be a built-in or user style"
            ))
        }
        Err(reason) => Err(reason),
    }
}

/// The project's styles directory: `<checkout>/.cyber/output-styles`.
pub fn project_dir(location: &Path) -> PathBuf {
    crate::config::project_root(location)
        .join(".cyber")
        .join(DIRECTORY)
}

/// The user's styles directory: `~/.config/cyber/output-styles`.
pub fn user_dir(global_config_dir: &Path) -> PathBuf {
    global_config_dir.join(DIRECTORY)
}

/// `^[a-z0-9][a-z0-9_-]{0,63}$`: checked on every selection, before any lookup.
pub fn valid_name(name: &str) -> bool {
    let word = |c: char| c.is_ascii_lowercase() || c.is_ascii_digit();
    name.len() <= MAX_NAME
        && name.chars().next().is_some_and(word)
        && name.chars().all(|c| word(c) || c == '_' || c == '-')
}

/// The built-in styles, `default` first.
pub fn builtins() -> Vec<OutputStyle> {
    let default = OutputStyle {
        name: DEFAULT.to_owned(),
        description: DEFAULT_DESCRIPTION.to_owned(),
        origin: StyleOrigin::Builtin,
        path: None,
        shadows: Vec::new(),
        text: String::new(),
    };
    let embedded = BUILTINS.iter().map(|(name, text)| {
        let mut style = parse(text, name).expect("a built-in output style parses");
        style.origin = StyleOrigin::Builtin;
        style
    });
    std::iter::once(default).chain(embedded).collect()
}

/// Load every style: built-ins, then user styles, then trusted project styles.
pub fn load(sources: &StyleSources<'_>) -> Styles {
    let mut out = Styles {
        styles: builtins(),
        ..Styles::default()
    };
    let mut custom: BTreeMap<String, OutputStyle> = BTreeMap::new();
    for style in read_dir(
        &user_dir(sources.global_config_dir),
        StyleOrigin::User,
        &mut out,
    ) {
        custom.insert(style.name.clone(), style);
    }
    let project = project_dir(sources.location);
    if sources.project_enabled && !sources.project_trusted {
        out.held = files(&project)
            .iter()
            .filter_map(|p| stem(p))
            .map(str::to_owned)
            .collect();
        if !out.held.is_empty() {
            out.issues.push(StyleIssue {
                path: project.clone(),
                message: HELD.into(),
            });
        }
    }
    if sources.project_enabled && sources.project_trusted {
        for mut style in read_dir(&project, StyleOrigin::Project, &mut out) {
            if let Some(user) = custom.remove(&style.name) {
                style.shadows.extend(user.path);
            }
            custom.insert(style.name.clone(), style);
        }
    }
    out.styles.extend(custom.into_values());
    out
}

/// The project style files for the trust digest: `(path relative to the checkout,
/// path, contents)`. Only regular files within the size bound are read; anything else
/// is listed with an empty text, so its later replacement still changes the digest.
pub fn trust_subset(location: &Path) -> Vec<(String, PathBuf, String)> {
    let root = crate::config::project_root(location);
    files(&project_dir(location))
        .into_iter()
        .map(|path| {
            let text = read_file(&path).unwrap_or_default();
            let relative = path
                .strip_prefix(&root)
                .unwrap_or(&path)
                .display()
                .to_string();
            (relative, path, text)
        })
        .collect()
}

/// `*.md` entries of `dir`, sorted, at most [`MAX_FILES`]; a linked directory has none.
fn files(dir: &Path) -> Vec<PathBuf> {
    let is_dir = std::fs::symlink_metadata(dir).is_ok_and(|meta| meta.is_dir());
    let Some(entries) = is_dir.then(|| std::fs::read_dir(dir).ok()).flatten() else {
        return Vec::new();
    };
    let mut out: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "md"))
        .collect();
    out.sort();
    out.truncate(MAX_FILES);
    out
}

fn stem(path: &Path) -> Option<&str> {
    path.file_stem().and_then(|stem| stem.to_str())
}

/// The valid styles of one directory; every other file becomes an issue.
fn read_dir(dir: &Path, origin: StyleOrigin, out: &mut Styles) -> Vec<OutputStyle> {
    let mut styles = Vec::new();
    for path in files(dir) {
        match read_style(&path) {
            Ok(_) if out.get(stem(&path).unwrap_or_default()).is_some() => {
                out.issues.push(StyleIssue {
                    path,
                    message: "name: reuses a built-in output style name".into(),
                });
            }
            Ok(mut style) => {
                style.origin = origin;
                style.path = Some(path);
                styles.push(style);
            }
            Err(failure) => {
                if failure.io {
                    out.unreadable.extend(stem(&path).map(str::to_owned));
                }
                out.issues.push(StyleIssue {
                    path,
                    message: failure.message,
                });
            }
        }
    }
    styles
}

/// Why a style file did not load; `io` marks one that could not be read.
struct Failure {
    message: String,
    io: bool,
}

impl From<String> for Failure {
    fn from(message: String) -> Self {
        Self { message, io: false }
    }
}

impl From<std::io::Error> for Failure {
    /// A file that vanished or is not UTF-8 is not an I/O failure of a listed style.
    fn from(error: std::io::Error) -> Self {
        use std::io::ErrorKind;
        let io = !matches!(error.kind(), ErrorKind::NotFound | ErrorKind::InvalidData);
        Self {
            message: error.to_string(),
            io,
        }
    }
}

fn read_style(path: &Path) -> Result<OutputStyle, Failure> {
    let name = stem(path).unwrap_or_default();
    if !valid_name(name) {
        return Err(format!(
            "name: expected lowercase letters, digits, '_' and '-' starting with a letter or digit (at most {MAX_NAME}), got \"{name}\""
        )
        .into());
    }
    Ok(parse(&read_file(path)?, name)?)
}

/// Read a style through one handle opened without following a final link, so a file
/// swapped for a link after listing is never read, and bound what is read, not what
/// was listed.
fn read_file(path: &Path) -> Result<String, Failure> {
    use std::io::Read;
    const NOT_REGULAR: &str = "an output style must be a regular file, not a link";
    let file = match open_no_follow(path) {
        Ok(file) => file,
        // The link itself is refused at open (`ELOOP` on unix): name it as one.
        Err(_) if std::fs::symlink_metadata(path).is_ok_and(|meta| !meta.is_file()) => {
            return Err(String::from(NOT_REGULAR).into());
        }
        Err(e) => return Err(e.into()),
    };
    if !file.metadata()?.is_file() {
        return Err(String::from(NOT_REGULAR).into());
    }
    let mut bytes = Vec::new();
    file.take(MAX_BYTES + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err(format!("an output style is at most {MAX_BYTES} bytes").into());
    }
    String::from_utf8(bytes).map_err(|_| {
        let kind = std::io::ErrorKind::InvalidData;
        std::io::Error::new(kind, "stream did not contain valid UTF-8").into()
    })
}

/// `O_NONBLOCK` keeps a FIFO from blocking the open; it does not affect a regular file.
#[cfg(unix)]
fn open_no_follow(path: &Path) -> std::io::Result<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
}

/// A reparse point opens as itself, which the handle's metadata then refuses.
#[cfg(windows)]
fn open_no_follow(path: &Path) -> std::io::Result<std::fs::File> {
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT;
    std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
}

/// Parse one style named `name` (its file stem).
fn parse(text: &str, name: &str) -> Result<OutputStyle, String> {
    let (header, body) = crate::teams::split_frontmatter(text)?;
    let fields = match header.map(serde_yaml_ng::from_str::<Value>).transpose() {
        Ok(Some(Value::Object(map))) => map,
        Ok(None | Some(Value::Null)) => Map::new(),
        Ok(Some(_)) => return Err("frontmatter: expected a mapping".into()),
        Err(e) => return Err(format!("frontmatter: {e}")),
    };
    if let Some(key) = fields.keys().find(|k| !FIELDS.contains(&k.as_str())) {
        return Err(format!(
            "{key}: unknown field (an output style sets only name and description)"
        ));
    }
    if let Some(value) = fields.get("name")
        && value.as_str() != Some(name)
    {
        return Err(format!(
            "name: must equal the file name \"{name}\", got {value}"
        ));
    }
    let description = match fields.get("description") {
        None => String::new(),
        Some(Value::String(text)) => text.clone(),
        Some(other) => return Err(format!("description: expected a string, got {other}")),
    };
    let text = body.trim();
    if text.is_empty() {
        return Err("an output style needs a body".into());
    }
    Ok(OutputStyle {
        name: name.to_owned(),
        description,
        origin: StyleOrigin::User,
        path: None,
        shadows: Vec::new(),
        text: text.to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixture {
        _dir: tempfile::TempDir,
        repo: PathBuf,
        config: PathBuf,
    }

    impl Fixture {
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let repo = dir.path().join("repo");
            let config = dir.path().join("config");
            std::fs::create_dir_all(repo.join(".git")).unwrap();
            Self {
                _dir: dir,
                repo,
                config,
            }
        }

        fn user(&self, file: &str, text: &str) -> PathBuf {
            write(&user_dir(&self.config).join(file), text)
        }

        fn project(&self, file: &str, text: &str) -> PathBuf {
            write(&project_dir(&self.repo).join(file), text)
        }

        fn load(&self, trusted: bool) -> Styles {
            load(&StyleSources {
                location: &self.repo,
                global_config_dir: &self.config,
                project_trusted: trusted,
                project_enabled: true,
            })
        }
    }

    fn write(path: &Path, text: &str) -> PathBuf {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
        path.to_path_buf()
    }

    fn issue<'a>(styles: &'a Styles, path: &Path) -> &'a str {
        let found = styles.issues.iter().find(|i| i.path == path);
        &found
            .unwrap_or_else(|| panic!("{:?}", styles.issues))
            .message
    }

    fn names(styles: &Styles) -> Vec<&str> {
        styles.styles.iter().map(|s| s.name.as_str()).collect()
    }

    #[test]
    fn builtins_parse_and_default_adds_nothing() {
        let builtins = builtins();
        let names: Vec<&str> = builtins.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["default", "concise", "explanatory", "learning"]);
        assert!(builtins[0].text.is_empty());
        for style in &builtins[1..] {
            assert!(!style.text.is_empty() && !style.description.is_empty());
            assert!(style.text.lines().count() <= 15, "{}", style.name);
        }
    }

    #[test]
    fn names_follow_the_pattern() {
        for good in ["a", "0", "code_review", "x-1", &"a".repeat(64)] {
            assert!(valid_name(good), "{good}");
        }
        for bad in [
            "",
            "../x",
            "Bad Style",
            "-a",
            "_a",
            "a/b",
            "a.b",
            &"a".repeat(65),
        ] {
            assert!(!valid_name(bad), "{bad}");
        }
    }

    #[test]
    fn frontmatter_is_checked() {
        let ok = parse("---\nname: terse\ndescription: Short\nkeep-coding-instructions: true\n---\nBe terse.\n", "terse").unwrap();
        assert_eq!(
            (ok.description.as_str(), ok.text.as_str()),
            ("Short", "Be terse.")
        );
        assert_eq!(parse("Plain body", "terse").unwrap().text, "Plain body");
        let cases = [
            (
                "---\npermissions: {allow: [bash]}\n---\nx",
                "permissions: unknown field",
            ),
            ("---\nmode: bypass\n---\nx", "mode: unknown field"),
            (
                "---\nname: other\n---\nx",
                "name: must equal the file name \"terse\"",
            ),
            (
                "---\ndescription: [1]\n---\nx",
                "description: expected a string",
            ),
            (
                "---\ndescription: d\n---\n \n",
                "an output style needs a body",
            ),
            ("---\n- a\n---\nx", "frontmatter: expected a mapping"),
            ("---\nname: terse\nx", "frontmatter has no closing ---"),
        ];
        for (text, expected) in cases {
            let error = parse(text, "terse").unwrap_err();
            assert!(error.starts_with(expected), "{error} vs {expected}");
        }
    }

    #[test]
    fn invalid_files_are_reported_and_block_nothing() {
        let fx = Fixture::new();
        let fast = fx.user("fast.md", "---\npermissions: {allow: [\"*\"]}\n---\nGo.\n");
        let big = fx.user("big.md", &"x".repeat(MAX_BYTES as usize + 1));
        let odd = fx.user("Odd Name.md", "Body");
        let reused = fx.user("concise.md", "Mine");
        fx.user("terse.md", "Be terse.");
        let styles = fx.load(true);
        assert!(issue(&styles, &fast).starts_with("permissions: unknown field"));
        assert_eq!(
            issue(&styles, &big),
            "an output style is at most 32768 bytes"
        );
        assert!(issue(&styles, &odd).starts_with("name: expected lowercase"));
        assert_eq!(
            issue(&styles, &reused),
            "name: reuses a built-in output style name"
        );
        assert_eq!(styles.issues.len(), 4);
        assert_eq!(
            names(&styles),
            ["default", "concise", "explanatory", "learning", "terse"]
        );
        assert_eq!(styles.get("concise").unwrap().origin, StyleOrigin::Builtin);
        assert!(
            styles
                .select("fast")
                .unwrap_err()
                .contains("is invalid: permissions")
        );
    }

    #[cfg(unix)]
    #[test]
    fn linked_files_are_refused() {
        let fx = Fixture::new();
        let target = write(&fx.repo.join("secret.md"), "Leaked text");
        let link = user_dir(&fx.config).join("linked.md");
        std::fs::create_dir_all(link.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(&target, &link).unwrap();
        let styles = fx.load(true);
        assert_eq!(
            issue(&styles, &link),
            "an output style must be a regular file, not a link"
        );
        assert!(styles.get("linked").is_none());
    }

    /// Regression: the link check and the read were separate path lookups, so a style
    /// replaced by a link between them was read through it. The read itself now refuses
    /// a link, and never blocks on a FIFO.
    #[cfg(unix)]
    #[test]
    fn the_read_itself_refuses_links_and_special_files() {
        let fx = Fixture::new();
        let target = write(&fx.repo.join("secret.md"), "Leaked text");
        let link = fx.repo.join("swapped.md");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        let refused = read_file(&link).unwrap_err().message;
        assert_eq!(
            refused,
            "an output style must be a regular file, not a link"
        );
        let fifo = fx.repo.join("pipe.md");
        let made = std::process::Command::new("mkfifo").arg(&fifo).status();
        assert!(made.unwrap().success());
        let refused = read_file(&fifo).unwrap_err().message;
        assert_eq!(
            refused,
            "an output style must be a regular file, not a link"
        );
    }

    #[test]
    fn the_size_bound_applies_to_the_bytes_read() {
        let fx = Fixture::new();
        let exact = write(&fx.repo.join("exact.md"), &"x".repeat(MAX_BYTES as usize));
        let read = read_file(&exact).map_err(|f| f.message).unwrap();
        assert_eq!(read.len() as u64, MAX_BYTES);
        let over = write(
            &fx.repo.join("over.md"),
            &"x".repeat(MAX_BYTES as usize + 1),
        );
        assert_eq!(
            read_file(&over).unwrap_err().message,
            "an output style is at most 32768 bytes"
        );
    }

    #[test]
    fn project_styles_shadow_user_styles_and_untrusted_ones_are_held() {
        let fx = Fixture::new();
        let user = fx.user("reviewer.md", "User reviewer.");
        fx.user("terse.md", "Be terse.");
        fx.project(
            "reviewer.md",
            "---\ndescription: Project\n---\nProject reviewer.",
        );
        let trusted = fx.load(true);
        assert!(trusted.issues.is_empty(), "{:?}", trusted.issues);
        let reviewer = trusted.get("reviewer").unwrap();
        assert_eq!(
            (reviewer.origin, reviewer.text.as_str()),
            (StyleOrigin::Project, "Project reviewer.")
        );
        assert_eq!(reviewer.shadows, [user]);
        assert_eq!(trusted.get("terse").unwrap().origin, StyleOrigin::User);

        let held = fx.load(false);
        assert_eq!(held.get("reviewer").unwrap().text, "User reviewer.");
        assert_eq!(
            held.issues,
            [StyleIssue {
                path: project_dir(&fx.repo),
                message: HELD.into()
            }]
        );
        fx.project("only.md", "Project only.");
        let held = fx.load(false);
        assert_eq!(
            held.select("only").unwrap_err(),
            "output style \"only\" is held until this checkout is trusted (cyber trust inspect)"
        );
        assert_eq!(
            held.select("../x").unwrap_err(),
            "invalid output style name \"../x\""
        );
        assert_eq!(
            held.select("nope").unwrap_err(),
            "unknown output style \"nope\""
        );
    }

    #[test]
    fn unknown_held_or_invalid_names_add_nothing() {
        let fx = Fixture::new();
        fx.project("only.md", "Project only.");
        fx.user("fast.md", "---\npermissions: {}\n---\nGo.");
        fx.user("terse.md", "Be terse.");
        let styles = fx.load(false);
        for name in ["default", "nope", "only", "fast", "../../etc/passwd"] {
            assert_eq!(styles.prompt(name), Ok(None), "{name}");
        }
        assert_eq!(
            styles.prompt("terse"),
            Ok(Some("Output style: terse\n\nBe terse.".into()))
        );
        let concise = styles.prompt("concise").unwrap().unwrap();
        assert!(
            concise.starts_with("Output style: concise\n\n"),
            "{concise}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn an_unreadable_listed_file_is_unavailable() {
        use std::os::unix::fs::PermissionsExt;
        let fx = Fixture::new();
        let path = fx.user("locked.md", "Locked.");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).unwrap();
        if std::fs::read(&path).is_ok() {
            return; // Running as root: permissions do not apply.
        }
        let styles = fx.load(true);
        assert_eq!(styles.unreadable, ["locked"]);
        let reason = styles.prompt("locked").unwrap_err();
        assert!(
            reason.starts_with("output style \"locked\" is invalid"),
            "{reason}"
        );
        assert_eq!(styles.prompt("nope"), Ok(None));
    }

    #[test]
    fn the_global_default_is_a_builtin_or_user_style() {
        let fx = Fixture::new();
        fx.user("terse.md", "Be terse.");
        fx.project("reviewer.md", "Review.");
        for name in ["default", "concise", "terse"] {
            assert_eq!(global_default(&fx.config, &fx.repo, name), Ok(()), "{name}");
        }
        let project = global_default(&fx.config, &fx.repo, "reviewer").unwrap_err();
        assert!(project.contains("defined only by a project"), "{project}");
        assert_eq!(
            global_default(&fx.config, &fx.repo, "nope").unwrap_err(),
            "unknown output style \"nope\""
        );
        assert_eq!(
            global_default(&fx.config, &fx.repo, "../x").unwrap_err(),
            "invalid output style name \"../x\""
        );
    }

    #[test]
    fn disabled_project_config_skips_project_styles() {
        let fx = Fixture::new();
        fx.project("reviewer.md", "Project reviewer.");
        for trusted in [true, false] {
            let styles = load(&StyleSources {
                location: &fx.repo,
                global_config_dir: &fx.config,
                project_trusted: trusted,
                project_enabled: false,
            });
            assert!(styles.get("reviewer").is_none());
            assert!(styles.issues.is_empty() && styles.held.is_empty());
        }
    }

    #[test]
    fn at_most_the_file_bound_is_read() {
        let fx = Fixture::new();
        for index in 0..=MAX_FILES {
            fx.user(&format!("s{index:03}.md"), "Body");
        }
        assert_eq!(fx.load(true).styles.len(), 4 + MAX_FILES);
        assert!(fx.load(true).get(&format!("s{MAX_FILES:03}")).is_none());
    }

    #[test]
    fn the_trust_subset_lists_project_files_only() {
        let fx = Fixture::new();
        fx.user("terse.md", "Be terse.");
        assert!(trust_subset(&fx.repo).is_empty());
        fx.project("reviewer.md", "Review.");
        let subset = trust_subset(&fx.repo);
        let relative = Path::new(".cyber")
            .join("output-styles")
            .join("reviewer.md");
        assert_eq!(subset.len(), 1);
        assert_eq!(
            (subset[0].0.as_str(), subset[0].2.as_str()),
            (relative.display().to_string().as_str(), "Review.")
        );
    }
}
