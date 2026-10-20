//! Team definitions and settings (`agent-teams` → Team definitions, Team composition).
//!
//! A definition is a Markdown file with YAML frontmatter (`name`, `description`,
//! `teammates`, `budget`) whose body is the lead's instructions. Definitions load from
//! the project's `.cyber/teams/*.md`, only while the checkout is trusted (the files are
//! part of the workspace-trust digest, [`trust_subset`]), and from the user's
//! `~/.config/cyber/teams/*.md`; a project definition wins over a user one of the same
//! name. A definition that does not validate is reported with the field it names and
//! never blocks the others.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::{Map, Value};

use crate::budget::Budget;

/// `teams.max_teammates` when unset.
pub const DEFAULT_MAX_TEAMMATES: u32 = 6;
/// The largest `teams.max_teammates`.
const MAX_TEAMMATES_LIMIT: u32 = 32;
/// Names a teammate may not take: the lead's member name and the broadcast address.
pub const RESERVED_NAMES: [&str; 2] = ["lead", "all"];
/// The message of a definition that names a `runner` (P3 distributed teammates).
pub const RUNNER_UNAVAILABLE: &str = "Distributed teammates are not available yet (runner)";

/// What every team surface says while the `teams` feature is off.
pub const DISABLED: &str =
    "Agent teams are disabled. Enable them with \"cyber features enable teams\".";

const DIRECTORY: &str = "teams";
const MAX_FILES: usize = 128;
const MAX_BYTES: u64 = 64 * 1024;
const FIELDS: [&str; 4] = ["name", "description", "teammates", "budget"];
const TEAMMATE_FIELDS: [&str; 6] = ["name", "agent", "model", "isolation", "owns", "runner"];

/// `teams`: the resolved settings of one Location.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TeamSettings {
    pub max_teammates: u32,
    /// `budgets.team`: the budget of a team whose definition sets none.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub budget: Option<Budget>,
}

impl Default for TeamSettings {
    fn default() -> Self {
        Self {
            max_teammates: DEFAULT_MAX_TEAMMATES,
            budget: None,
        }
    }
}

impl TeamSettings {
    pub fn from_config(config: &Value) -> Result<Self, String> {
        let mut settings = Self::default();
        let Some(teams) = config.get("teams") else {
            return Ok(settings);
        };
        let teams = teams.as_object().ok_or("teams: expected an object")?;
        for (key, value) in teams {
            match key.as_str() {
                "max_teammates" => settings.max_teammates = max_teammates(value)?,
                // Teammate panes (P4) are not available: only the inline view.
                "display" if value.as_str() == Some("inline") => {}
                "display" => {
                    return Err(format!("teams.display: expected \"inline\", got {value}"));
                }
                _ => return Err(format!("teams.{key}: unknown key")),
            }
        }
        Ok(settings)
    }
}

fn max_teammates(value: &Value) -> Result<u32, String> {
    value
        .as_u64()
        .and_then(|n| u32::try_from(n).ok())
        .filter(|n| (1..=MAX_TEAMMATES_LIMIT).contains(n))
        .ok_or_else(|| {
            format!("teams.max_teammates: expected an integer from 1 to {MAX_TEAMMATES_LIMIT}, got {value}")
        })
}

/// Where a teammate works.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, serde::Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "lowercase")]
pub enum Isolation {
    /// The lead's Location.
    Shared,
    /// Its own managed worktree.
    Worktree,
}

/// One teammate of a definition.
#[derive(Debug, Clone, PartialEq, Serialize, schemars::JsonSchema)]
pub struct TeammateSpec {
    pub name: String,
    pub agent: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// `None`: chosen at spawn (worktree when two or more teammates can edit).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub isolation: Option<Isolation>,
    /// Globs, relative to the team directory, only this teammate may edit.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub owns: Vec<String>,
}

/// Which directory a definition came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum DefinitionScope {
    Project,
    User,
}

/// A valid team definition.
#[derive(Debug, Clone, PartialEq, Serialize, schemars::JsonSchema)]
pub struct TeamDefinition {
    pub name: String,
    pub description: String,
    pub teammates: Vec<TeammateSpec>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub budget: Option<Budget>,
    /// The body: the lead's instructions.
    pub instructions: String,
    pub scope: DefinitionScope,
    pub path: PathBuf,
    /// Definitions of the same name this one replaced (a user one under a project one).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub shadows: Vec<PathBuf>,
}

/// A definition file that did not load, and why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, schemars::JsonSchema)]
pub struct DefinitionIssue {
    pub path: PathBuf,
    /// `<field>: <problem>`, such as `teammates[1].name: "lead" is reserved`.
    pub message: String,
}

/// Every definition of a Location, valid ones by name.
#[derive(Debug, Clone, Default, PartialEq, Serialize, schemars::JsonSchema)]
pub struct Definitions {
    pub definitions: Vec<TeamDefinition>,
    pub issues: Vec<DefinitionIssue>,
}

impl Definitions {
    pub fn get(&self, name: &str) -> Option<&TeamDefinition> {
        self.definitions.iter().find(|d| d.name == name)
    }
}

/// Inputs of [`load`].
pub struct DefinitionSources<'a> {
    pub location: &'a Path,
    /// `~/.config/cyber`.
    pub global_config_dir: &'a Path,
    /// Workspace trust of the checkout; untrusted project definitions are held back.
    pub project_trusted: bool,
    /// `CYBER_DISABLE_PROJECT_CONFIG` off.
    pub project_enabled: bool,
    pub max_teammates: u32,
}

/// The definitions of a Location from its resolved configuration: trust and
/// `teams.max_teammates` come from `resolved`.
pub fn definitions(
    location: &Path,
    global_config_dir: &Path,
    resolved: &crate::config::Resolved,
    env: &dyn crate::env::EnvSource,
) -> Definitions {
    let max_teammates = TeamSettings::from_config(&resolved.value)
        .map(|settings| settings.max_teammates)
        .unwrap_or(DEFAULT_MAX_TEAMMATES);
    load(&DefinitionSources {
        location,
        global_config_dir,
        project_trusted: resolved.trust.trusted,
        project_enabled: !env.flag("CYBER_DISABLE_PROJECT_CONFIG"),
        max_teammates,
    })
}

/// The project's definitions directory: `<checkout>/.cyber/teams`.
pub fn project_dir(location: &Path) -> PathBuf {
    crate::config::project_root(location)
        .join(".cyber")
        .join(DIRECTORY)
}

/// The user's definitions directory: `~/.config/cyber/teams`.
pub fn user_dir(global_config_dir: &Path) -> PathBuf {
    global_config_dir.join(DIRECTORY)
}

/// Load and validate every definition; project definitions win on a name clash.
pub fn load(sources: &DefinitionSources<'_>) -> Definitions {
    let mut issues = Vec::new();
    let mut by_name: BTreeMap<String, TeamDefinition> = BTreeMap::new();
    let user = user_dir(sources.global_config_dir);
    for definition in read_dir(&user, DefinitionScope::User, sources, &mut issues) {
        by_name.insert(definition.name.clone(), definition);
    }
    let project = project_dir(sources.location);
    if sources.project_enabled && !sources.project_trusted && !files(&project).is_empty() {
        issues.push(DefinitionIssue {
            path: project.clone(),
            message: "project team definitions are held until this checkout is trusted (cyber trust inspect)".into(),
        });
    }
    if sources.project_enabled && sources.project_trusted {
        for mut definition in read_dir(&project, DefinitionScope::Project, sources, &mut issues) {
            if let Some(user) = by_name.remove(&definition.name) {
                definition.shadows.push(user.path);
            }
            by_name.insert(definition.name.clone(), definition);
        }
    }
    Definitions {
        definitions: by_name.into_values().collect(),
        issues,
    }
}

/// The project definition files for the trust digest: `(path relative to the checkout,
/// contents)`. A file that cannot be read is listed with an empty text, so its later
/// appearance still changes the digest.
pub fn trust_subset(location: &Path) -> Vec<(String, PathBuf, String)> {
    let root = crate::config::project_root(location);
    files(&project_dir(location))
        .into_iter()
        .map(|path| {
            let text = std::fs::read_to_string(&path).unwrap_or_default();
            let relative = path
                .strip_prefix(&root)
                .unwrap_or(&path)
                .display()
                .to_string();
            (relative, path, text)
        })
        .collect()
}

/// `*.md` files of `dir`, sorted; links and entries beyond the bound are left out.
fn files(dir: &Path) -> Vec<PathBuf> {
    let Ok(meta) = std::fs::symlink_metadata(dir) else {
        return Vec::new();
    };
    if !meta.is_dir() {
        return Vec::new();
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
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

fn read_dir(
    dir: &Path,
    scope: DefinitionScope,
    sources: &DefinitionSources<'_>,
    issues: &mut Vec<DefinitionIssue>,
) -> Vec<TeamDefinition> {
    let mut seen: BTreeMap<String, PathBuf> = BTreeMap::new();
    let mut out = Vec::new();
    for path in files(dir) {
        let parsed = read_file(&path).and_then(|text| parse(&text, &path, sources.max_teammates));
        let mut definition = match parsed {
            Ok(definition) => definition,
            Err(message) => {
                issues.push(DefinitionIssue { path, message });
                continue;
            }
        };
        if let Some(first) = seen.get(&definition.name) {
            issues.push(DefinitionIssue {
                message: format!(
                    "name: \"{}\" is already defined by {}",
                    definition.name,
                    first.display()
                ),
                path,
            });
            continue;
        }
        seen.insert(definition.name.clone(), path.clone());
        definition.scope = scope;
        definition.path = path;
        out.push(definition);
    }
    out
}

fn read_file(path: &Path) -> Result<String, String> {
    let meta = std::fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    if !meta.is_file() {
        return Err("a team definition must be a regular file, not a link".into());
    }
    if meta.len() > MAX_BYTES {
        return Err(format!("a team definition is at most {MAX_BYTES} bytes"));
    }
    std::fs::read_to_string(path).map_err(|e| e.to_string())
}

/// Parse one definition. `path` supplies the default name (the file stem).
pub fn parse(text: &str, path: &Path, max_teammates: u32) -> Result<TeamDefinition, String> {
    let (header, body) = split_frontmatter(text)?;
    let header = header.ok_or("a team definition needs YAML frontmatter")?;
    let fields: Value =
        serde_yaml_ng::from_str::<Value>(header).map_err(|e| format!("frontmatter: {e}"))?;
    let fields = match fields {
        Value::Object(map) => map,
        Value::Null => Map::new(),
        _ => return Err("frontmatter: expected a mapping".into()),
    };
    if let Some(key) = fields.keys().find(|k| !FIELDS.contains(&k.as_str())) {
        return Err(format!("{key}: unknown field"));
    }
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or_default();
    let name = match fields.get("name") {
        None => stem.to_owned(),
        Some(value) => text_field(value, "name")?,
    };
    check_name(&name, "name")?;
    let description = match fields.get("description") {
        None => String::new(),
        Some(value) => text_field(value, "description")?,
    };
    Ok(TeamDefinition {
        name,
        description,
        teammates: teammates(fields.get("teammates"), max_teammates)?,
        budget: budget(fields.get("budget"))?,
        instructions: body.trim().to_owned(),
        scope: DefinitionScope::User,
        path: path.to_path_buf(),
        shadows: Vec::new(),
    })
}

/// `(frontmatter, body)`; the frontmatter is `None` without an opening `---` line.
fn split_frontmatter(text: &str) -> Result<(Option<&str>, &str), String> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let Some(rest) = text
        .strip_prefix("---\n")
        .or_else(|| text.strip_prefix("---\r\n"))
    else {
        return Ok((None, text));
    };
    let mut offset = 0;
    for line in rest.split_inclusive('\n') {
        if line.trim_end_matches(['\r', '\n']) == "---" {
            return Ok((Some(&rest[..offset]), &rest[offset + line.len()..]));
        }
        offset += line.len();
    }
    Err("frontmatter has no closing ---".into())
}

fn text_field(value: &Value, field: &str) -> Result<String, String> {
    value
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| format!("{field}: expected a string, got {value}"))
}

/// `[a-z][a-z0-9-]{0,31}`.
pub fn valid_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && name.len() <= 32
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

fn check_name(name: &str, field: &str) -> Result<(), String> {
    if valid_name(name) {
        Ok(())
    } else {
        Err(format!(
            "{field}: expected lowercase letters, digits and '-' starting with a letter (at most 32), got \"{name}\""
        ))
    }
}

fn teammates(value: Option<&Value>, max: u32) -> Result<Vec<TeammateSpec>, String> {
    let list = match value {
        None => return Err("teammates: required".into()),
        Some(Value::Array(list)) => list,
        Some(other) => return Err(format!("teammates: expected a list, got {other}")),
    };
    if list.is_empty() {
        return Err("teammates: expected at least one teammate".into());
    }
    if list.len() > max as usize {
        return Err(format!(
            "teammates: at most {max} teammates (teams.max_teammates), got {}",
            list.len()
        ));
    }
    let mut out: Vec<TeammateSpec> = Vec::new();
    for (index, item) in list.iter().enumerate() {
        let field = format!("teammates[{index}]");
        let spec = teammate(item, &field)?;
        if out.iter().any(|other| other.name == spec.name) {
            return Err(format!(
                "{field}.name: duplicate teammate name \"{}\"",
                spec.name
            ));
        }
        out.push(spec);
    }
    Ok(out)
}

fn teammate(value: &Value, field: &str) -> Result<TeammateSpec, String> {
    let map = value
        .as_object()
        .ok_or_else(|| format!("{field}: expected a mapping, got {value}"))?;
    if let Some(key) = map.keys().find(|k| !TEAMMATE_FIELDS.contains(&k.as_str())) {
        return Err(format!("{field}.{key}: unknown field"));
    }
    if map.contains_key("runner") {
        return Err(format!("{field}.runner: {RUNNER_UNAVAILABLE}"));
    }
    let required = |key: &str| -> Result<String, String> {
        let path = format!("{field}.{key}");
        map.get(key)
            .ok_or_else(|| format!("{path}: required"))
            .and_then(|value| text_field(value, &path))
    };
    let name = required("name")?;
    check_name(&name, &format!("{field}.name"))?;
    if RESERVED_NAMES.contains(&name.as_str()) {
        return Err(format!("{field}.name: \"{name}\" is reserved"));
    }
    let agent = required("agent")?;
    if agent.trim().is_empty() {
        return Err(format!("{field}.agent: expected an agent name"));
    }
    Ok(TeammateSpec {
        name,
        agent,
        model: model(map.get("model"), field)?,
        isolation: isolation(map.get("isolation"), field)?,
        owns: owns(map.get("owns"), field)?,
    })
}

fn model(value: Option<&Value>, field: &str) -> Result<Option<String>, String> {
    let Some(value) = value else { return Ok(None) };
    let model = text_field(value, &format!("{field}.model"))?;
    match model.split_once('/') {
        Some((provider, id)) if !provider.is_empty() && !id.is_empty() => Ok(Some(model)),
        _ => Err(format!(
            "{field}.model: expected provider/model[#variant], got \"{model}\""
        )),
    }
}

fn isolation(value: Option<&Value>, field: &str) -> Result<Option<Isolation>, String> {
    match value.map(|v| (v, v.as_str())) {
        None => Ok(None),
        Some((_, Some("shared"))) => Ok(Some(Isolation::Shared)),
        Some((_, Some("worktree"))) => Ok(Some(Isolation::Worktree)),
        Some((value, _)) => Err(format!(
            "{field}.isolation: expected shared or worktree, got {value}"
        )),
    }
}

fn owns(value: Option<&Value>, field: &str) -> Result<Vec<String>, String> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let list = value
        .as_array()
        .ok_or_else(|| format!("{field}.owns: expected a list of globs, got {value}"))?;
    list.iter()
        .enumerate()
        .map(|(index, glob)| owned_glob(glob, &format!("{field}.owns[{index}]")))
        .collect()
}

/// A relative glob that stays inside the team directory.
fn owned_glob(value: &Value, field: &str) -> Result<String, String> {
    let glob = text_field(value, field)?;
    check_owned_glob(&glob).map_err(|error| format!("{field}: {error}"))?;
    Ok(glob)
}

/// An `owns` glob: relative to the team directory, never leaving it, and valid.
pub fn check_owned_glob(glob: &str) -> Result<(), String> {
    let escapes = glob.starts_with(['/', '\\'])
        || glob.split(['/', '\\']).any(|part| part == "..")
        || glob.get(1..2) == Some(":");
    if glob.trim().is_empty() || escapes {
        return Err(format!(
            "expected a glob relative to the team directory, got \"{glob}\""
        ));
    }
    owned_matcher(glob).map(drop)
}

/// The matcher of an `owns` glob over `/`-separated paths relative to the team
/// directory. `*` stays within one path segment; matching ignores case where the file
/// system usually does (macOS, Windows), so a differently cased path is still owned.
fn owned_matcher(glob: &str) -> Result<globset::GlobMatcher, String> {
    globset::GlobBuilder::new(glob)
        .literal_separator(true)
        .case_insensitive(cfg!(any(windows, target_os = "macos")))
        .build()
        .map(|glob| glob.compile_matcher())
        .map_err(|error| error.to_string())
}

/// Whether `relative` (`/`-separated, relative to the team directory, empty for the
/// directory itself) is owned under `globs`: it matches one, or it is a directory that
/// holds a glob's fixed leading directories (removing `server` removes `server/**`). A
/// glob that does not compile owns everything (fail closed).
pub fn owns_path(globs: &[String], relative: &str) -> bool {
    let fold = |text: &str| {
        if cfg!(any(windows, target_os = "macos")) {
            text.to_lowercase()
        } else {
            text.to_owned()
        }
    };
    let directory = fold(&format!("{relative}/"));
    globs.iter().any(|glob| {
        relative.is_empty()
            || fold(literal_directories(glob)).starts_with(&directory)
            || owned_matcher(glob).map_or(true, |matcher| matcher.is_match(relative))
    })
}

/// The leading directories of a glob before its first wildcard, with the trailing `/`
/// (`server/api/` of `server/api/*.rs`; empty for `**/*.rs`).
fn literal_directories(glob: &str) -> &str {
    let fixed = glob
        .find(['*', '?', '[', '{', '\\'])
        .map_or(glob, |end| &glob[..end]);
    fixed.rfind('/').map_or("", |slash| &fixed[..=slash])
}

fn budget(value: Option<&Value>) -> Result<Option<Budget>, String> {
    let Some(value) = value.filter(|v| !v.is_null()) else {
        return Ok(None);
    };
    let budget: Budget =
        serde_json::from_value(value.clone()).map_err(|e| format!("budget: {e}"))?;
    budget.validate().map_err(|e| format!("budget.{e}"))?;
    Ok(Some(budget))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn owned_paths_match_globs_and_the_directories_holding_them() {
        let globs = vec!["server/**".to_owned(), "docs/*.md".to_owned()];
        for owned in ["server/api.rs", "server/a/b.rs", "docs/x.md", "server", ""] {
            assert!(owns_path(&globs, owned), "{owned}");
        }
        for free in [
            "web/app.ts",
            "docs/a/x.md",
            "serverless/x",
            "docs.md",
            "docs/x.txt",
        ] {
            assert!(!owns_path(&globs, free), "{free}");
        }
        // `docs` holds `docs/*.md`.
        assert!(owns_path(&globs, "docs"));
        assert!(owns_path(&["**/*.rs".to_owned()], "a/b/c.rs"));
        assert!(!owns_path(&["**/*.rs".to_owned()], "a/b"));
        assert!(owns_path(&["[".to_owned()], "anything"), "fails closed");
        assert!(check_owned_glob("server/**").is_ok());
        for escaping in ["/etc/**", "../x", "C:/x", " ", "a/../b"] {
            assert!(check_owned_glob(escaping).is_err(), "{escaping}");
        }
        assert!(check_owned_glob("[").is_err());
    }

    const FEATURE: &str = "---\nname: feature\ndescription: Ship a feature\nteammates:\n  - name: api\n    agent: build\n    owns: [\"src/api/**\"]\n  - name: ui\n    agent: build\n    model: test/small\n    isolation: worktree\nbudget:\n  max_cost_usd: 20\n---\nPlan the work, then assign it.\n";

    fn parsed(text: &str) -> Result<TeamDefinition, String> {
        parse(text, Path::new("/t/feature.md"), DEFAULT_MAX_TEAMMATES)
    }

    #[test]
    fn a_definition_parses_with_its_instructions() {
        let d = parsed(FEATURE).unwrap();
        assert_eq!(d.name, "feature");
        assert_eq!(d.description, "Ship a feature");
        assert_eq!(d.instructions, "Plan the work, then assign it.");
        assert_eq!(d.teammates.len(), 2);
        assert_eq!(d.teammates[0].owns, ["src/api/**"]);
        assert_eq!(d.teammates[1].isolation, Some(Isolation::Worktree));
        assert_eq!(d.teammates[1].model.as_deref(), Some("test/small"));
        assert_eq!(d.budget.unwrap().max_cost_usd, Some(20.0));
    }

    #[test]
    fn invalid_fields_are_named() {
        let cases = [
            (
                FEATURE.replace("name: ui", "name: lead"),
                "teammates[1].name: \"lead\" is reserved",
            ),
            (
                FEATURE.replace("name: ui", "name: all"),
                "teammates[1].name: \"all\" is reserved",
            ),
            (
                FEATURE.replace("name: ui", "name: api"),
                "teammates[1].name: duplicate teammate name \"api\"",
            ),
            (
                FEATURE.replace("    isolation: worktree", "    runner: desk"),
                "teammates[1].runner: Distributed teammates are not available yet (runner)",
            ),
            (
                FEATURE.replace("isolation: worktree", "isolation: remote"),
                "teammates[1].isolation: expected shared or worktree",
            ),
            (
                FEATURE.replace("    agent: build\n    owns", "    owns"),
                "teammates[0].agent: required",
            ),
            (
                FEATURE.replace("src/api/**", "../x"),
                "teammates[0].owns[0]: expected a glob relative",
            ),
            (
                FEATURE.replace("model: test/small", "model: small"),
                "teammates[1].model: expected provider/model",
            ),
            (
                FEATURE.replace("name: feature", "name: Feature"),
                "name: expected lowercase",
            ),
            (
                FEATURE.replace("max_cost_usd: 20", "max_dollars: 20"),
                "budget: unknown field `max_dollars`",
            ),
            (
                FEATURE.replace("description:", "colour: red\ndescription:"),
                "colour: unknown field",
            ),
            (
                FEATURE.replace("    model: test/small", "    color: red"),
                "teammates[1].color: unknown field",
            ),
            (
                "no frontmatter".to_owned(),
                "a team definition needs YAML frontmatter",
            ),
        ];
        for (text, expected) in cases {
            let error = parsed(&text).unwrap_err();
            assert!(error.starts_with(expected), "{error} vs {expected}");
        }
    }

    #[test]
    fn the_team_size_limit_applies_to_definitions() {
        let error = parse(FEATURE, Path::new("/t/f.md"), 1).unwrap_err();
        assert_eq!(
            error,
            "teammates: at most 1 teammates (teams.max_teammates), got 2"
        );
    }

    #[test]
    fn the_file_stem_names_a_definition_without_a_name() {
        let text = FEATURE.replace("name: feature\n", "");
        let d = parse(&text, Path::new("/t/review.md"), 6).unwrap();
        assert_eq!(d.name, "review");
    }

    #[test]
    fn settings_validate_the_limit_and_display() {
        let settings = |v: Value| TeamSettings::from_config(&serde_json::json!({ "teams": v }));
        assert_eq!(settings(serde_json::json!({})).unwrap().max_teammates, 6);
        assert_eq!(
            settings(serde_json::json!({"max_teammates": 3, "display": "inline"}))
                .unwrap()
                .max_teammates,
            3
        );
        assert!(settings(serde_json::json!({"max_teammates": 0})).is_err());
        assert!(
            settings(serde_json::json!({"display": "tmux"}))
                .unwrap_err()
                .contains("teams.display")
        );
        assert!(settings(serde_json::json!({"size": 1})).is_err());
    }

    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    #[test]
    fn project_definitions_win_and_untrusted_ones_are_held() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        let config = dir.path().join("config");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        write(
            &config.join("teams/feature.md"),
            &FEATURE.replace("Ship a feature", "user"),
        );
        write(
            &config.join("teams/solo.md"),
            "---\nteammates: [{name: a, agent: build}]\n---\n",
        );
        write(
            &repo.join(".cyber/teams/feature.md"),
            &FEATURE.replace("Ship a feature", "project"),
        );
        let sources = |trusted| DefinitionSources {
            location: &repo,
            global_config_dir: &config,
            project_trusted: trusted,
            project_enabled: true,
            max_teammates: 6,
        };
        let loaded = load(&sources(true));
        assert!(loaded.issues.is_empty(), "{:?}", loaded.issues);
        let feature = loaded.get("feature").unwrap();
        assert_eq!(
            (feature.description.as_str(), feature.scope),
            ("project", DefinitionScope::Project)
        );
        assert_eq!(feature.shadows, [config.join("teams/feature.md")]);
        assert_eq!(loaded.get("solo").unwrap().scope, DefinitionScope::User);
        let held = load(&sources(false));
        assert_eq!(held.get("feature").unwrap().description, "user");
        assert!(
            held.issues[0]
                .message
                .contains("held until this checkout is trusted")
        );
        assert_eq!(trust_subset(&repo).len(), 1);
    }
}
