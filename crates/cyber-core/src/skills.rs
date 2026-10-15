//! Skill discovery and `SKILL.md` parsing (`skills-commands`).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

mod bundled;
mod discovery;
mod template;

pub use template::{
    Append, Expanded, Injection, MAX_INJECTIONS, derived_argument_hint, expand, expand_template,
};

/// Captured declarations from an authorized load; instruction text is not authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillActivation {
    pub name: String,
    pub allowed_tools: Vec<String>,
    pub disallowed_tools: Vec<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SkillContext {
    #[default]
    Inline,
    Fork,
}

/// Embedded packages have no local directory or filesystem authority.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkillSource {
    Directory(PathBuf),
    Bundled,
}

/// The scope a skill was discovered in.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "lowercase")]
pub enum SkillOrigin {
    /// Embedded in the binary.
    #[default]
    Builtin,
    /// `~/.config/cyber/skills` and the home compatibility folders.
    User,
    /// A directory from the project root down to the Location.
    Project,
    /// A local path from the `skills` configuration.
    Config,
}

impl SkillOrigin {
    pub fn as_str(self) -> &'static str {
        match self {
            SkillOrigin::Builtin => "builtin",
            SkillOrigin::User => "user",
            SkillOrigin::Project => "project",
            SkillOrigin::Config => "config",
        }
    }
}

/// A skipped skill or a duplicate notice. `path` is the `SKILL.md`, the skill
/// directory, or `builtin:<name>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillDiagnostic {
    pub path: String,
    pub origin: SkillOrigin,
    /// The winning skill's name when this is a duplicate notice.
    pub replaces: Option<String>,
    pub message: String,
}

impl std::fmt::Display for SkillDiagnostic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.path, self.message)
    }
}

/// Where to look for skills.
#[derive(Debug, Clone)]
pub struct SkillScope {
    pub location: PathBuf,
    pub home: PathBuf,
    /// `~/.config/cyber`.
    pub global_config_dir: PathBuf,
    /// `skills.compat`: include `.claude`, `.agents` and `.codex` folders.
    pub compat: bool,
    /// `skills` config entries that are local paths: a skill directory or a folder of them.
    pub extra: Vec<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skill {
    pub name: String,
    pub description: String,
    pub allowed_tools: Vec<String>,
    pub disallowed_tools: Vec<String>,
    pub paths: Vec<String>,
    pub context: SkillContext,
    pub agent: Option<String>,
    pub model: Option<String>,
    pub disable_model_invocation: bool,
    pub user_invocable: bool,
    pub argument_hint: Option<String>,
    pub source: SkillSource,
    pub origin: SkillOrigin,
    pub body: String,
}

impl Skill {
    pub fn directory(&self) -> Option<&Path> {
        match &self.source {
            SkillSource::Directory(path) => Some(path),
            SkillSource::Bundled => None,
        }
    }

    pub fn is_bundled(&self) -> bool {
        self.source == SkillSource::Bundled
    }

    pub fn source_label(&self) -> String {
        self.directory().map_or_else(
            || format!("builtin:{}", self.name),
            |path| path.display().to_string(),
        )
    }
}

#[derive(Debug, Clone, Default)]
pub struct Discovery {
    /// Skills by name; later locations replace earlier ones.
    pub skills: BTreeMap<String, Skill>,
    /// Skipped skills and duplicate notices, in discovery order.
    pub diagnostics: Vec<SkillDiagnostic>,
}

#[derive(Deserialize)]
#[serde(rename_all = "kebab-case")]
struct Frontmatter {
    name: String,
    description: String,
    #[serde(default)]
    allowed_tools: Tools,
    #[serde(default)]
    disallowed_tools: Tools,
    #[serde(default)]
    paths: Vec<String>,
    #[serde(default)]
    context: SkillContext,
    agent: Option<String>,
    model: Option<String>,
    #[serde(default)]
    disable_model_invocation: bool,
    user_invocable: Option<bool>,
    argument_hint: Option<String>,
}

/// `allowed-tools` as a YAML list or a comma-separated string.
#[derive(Deserialize, Default)]
#[serde(untagged)]
enum Tools {
    #[default]
    None,
    List(Vec<String>),
    Text(String),
}

impl Tools {
    fn into_vec(self) -> Vec<String> {
        match self {
            Tools::None => Vec::new(),
            Tools::List(items) => items,
            Tools::Text(text) => text
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect(),
        }
    }
}

/// Folders searched, in precedence order (later wins). Bases are canonical so the fenced
/// reads below can refuse any link inside a skill root.
pub fn roots(scope: &SkillScope) -> Vec<(PathBuf, SkillOrigin)> {
    let global = discovery::canonical(&scope.global_config_dir);
    let home = discovery::canonical(&scope.home);
    let mut roots = vec![(global.join("skills"), SkillOrigin::User)];
    if scope.compat {
        roots.extend(
            [".claude/skills", ".agents/skills", ".codex/skills"]
                .map(|d| (home.join(d), SkillOrigin::User)),
        );
    }
    let location = discovery::canonical(&scope.location);
    let project = crate::config::project_root(&location);
    let mut dirs: Vec<&Path> = location
        .ancestors()
        .take_while(|d| d.starts_with(&project))
        .collect();
    dirs.reverse();
    let names: &[&str] = if scope.compat {
        &[
            ".claude/skills",
            ".agents/skills",
            ".codex/skills",
            ".cyber/skills",
        ]
    } else {
        &[".cyber/skills"]
    };
    for dir in dirs {
        roots.extend(names.iter().map(|n| (dir.join(n), SkillOrigin::Project)));
    }
    roots.extend(
        scope
            .extra
            .iter()
            .map(|path| (discovery::canonical_parent(path), SkillOrigin::Config)),
    );
    roots
}

pub fn discover(scope: &SkillScope) -> Discovery {
    let mut scanner = discovery::Scanner::new(bundled::discover());
    for (root, origin) in roots(scope) {
        scanner.root(&root, origin);
    }
    scanner.out
}

/// Load one skill directory through the fenced reader; only the directory itself is resolved.
pub fn load(dir: &Path) -> Result<Skill, String> {
    discovery::read(&discovery::canonical(dir))
}

fn parse(text: &str, source: SkillSource) -> Result<Skill, String> {
    let (yaml, body) = split_frontmatter(text).ok_or("missing YAML frontmatter")?;
    let fm: Frontmatter =
        serde_yaml_ng::from_str(yaml).map_err(|e| format!("invalid frontmatter: {e}"))?;
    validate_name(&fm.name)?;
    if let Some(agent) = &fm.agent
        && (fm.context != SkillContext::Fork
            || agent.trim().is_empty()
            || agent.len() > 128
            || agent.chars().any(char::is_control))
    {
        return Err("agent must be a bounded name on a forked skill".into());
    }
    if fm.description.trim().is_empty() || fm.description.chars().count() > 1024 {
        return Err("description must be 1-1024 characters".into());
    }
    if fm.paths.len() > 64
        || fm.paths.iter().any(|path| {
            path.is_empty()
                || path.len() > 256
                || path.contains('\0')
                || path.as_bytes().get(1) == Some(&b':')
                || path.starts_with('/')
                || path.contains('\\')
                || path.split('/').any(|part| part == "..")
                || globset::GlobBuilder::new(path)
                    .literal_separator(true)
                    .build()
                    .is_err()
        })
    {
        return Err("paths must contain at most 64 bounded project-relative globs".into());
    }
    if let Some(model) = &fm.model
        && !valid_model_ref(model)
    {
        return Err(format!(
            "invalid model {model:?}: use provider/model[#variant]"
        ));
    }
    let allowed_tools = fm.allowed_tools.into_vec();
    let disallowed_tools = fm.disallowed_tools.into_vec();
    validate_tool_patterns(&allowed_tools)?;
    validate_tool_patterns(&disallowed_tools)?;
    Ok(Skill {
        name: fm.name,
        description: fm.description.trim().to_string(),
        allowed_tools,
        disallowed_tools,
        paths: fm.paths,
        context: fm.context,
        agent: fm.agent,
        model: fm.model,
        disable_model_invocation: fm.disable_model_invocation,
        user_invocable: fm.user_invocable.unwrap_or(true),
        argument_hint: fm.argument_hint,
        source,
        origin: SkillOrigin::Builtin,
        body: body.trim_start_matches(['\r', '\n']).to_string(),
    })
}

fn split_frontmatter(text: &str) -> Option<(&str, &str)> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let rest = text
        .strip_prefix("---\n")
        .or_else(|| text.strip_prefix("---\r\n"))?;
    let end = rest.find("\n---")?;
    let after = &rest[end + 4..];
    Some((
        &rest[..end],
        after.split_once('\n').map_or("", |(_, body)| body),
    ))
}

/// The `provider/model[#variant]` grammar of `cyber_llm::catalog::ModelRef::parse`, bounded.
pub(crate) fn valid_model_ref(model: &str) -> bool {
    let Some((provider, rest)) = model.split_once('/') else {
        return false;
    };
    let (name, variant) = rest
        .rsplit_once('#')
        .map_or((rest, None), |(m, v)| (m, Some(v)));
    model.len() <= 256
        && !model.chars().any(char::is_control)
        && !provider.is_empty()
        && !name.is_empty()
        && variant != Some("")
}

fn validate_tool_patterns(patterns: &[String]) -> Result<(), String> {
    if patterns.len() > 64
        || patterns.iter().any(|pattern| {
            let (tool, resource) = pattern.split_once(':').unwrap_or((pattern, "*"));
            pattern.len() > 256
                || pattern.chars().any(char::is_control)
                || tool.is_empty()
                || resource.is_empty()
        })
    {
        return Err(
            "tool declarations must contain at most 64 bounded tool[:resource] wildcard patterns"
                .into(),
        );
    }
    Ok(())
}

pub fn validate_name(name: &str) -> Result<(), String> {
    let mut chars = name.chars();
    let first_ok = chars
        .next()
        .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit());
    let rest_ok = chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    if first_ok && rest_ok && name.len() <= 64 {
        Ok(())
    } else {
        Err(format!(
            "invalid name {name:?}: use lowercase letters, digits and hyphens"
        ))
    }
}

/// Captured instructions and bounded supporting-file context for either execution route.
pub fn instruction_frame(skill: &Skill, arguments: &str) -> String {
    instruction_frame_template(skill, arguments).text
}

/// [`instruction_frame`] with the body's shell injections located in the frame.
pub fn instruction_frame_template(skill: &Skill, arguments: &str) -> Expanded {
    let mut body = expand_template(&skill.body, arguments, Append::Labeled);
    body.text.truncate(body.text.trim_end().len());
    let files = sibling_files(skill, 20);
    let listing = if files.is_empty() {
        String::new()
    } else {
        format!("\n\nFiles in this skill:\n{}", files.join("\n"))
    };
    let prefix = format!(
        "<skill name=\"{}\" base=\"{}\">\n",
        skill.name,
        skill.source_label()
    );
    body.framed(&prefix, &format!("{listing}\n</skill>"))
}

/// Up to `limit` files beside `SKILL.md`, as sorted relative paths. The walk stops once
/// `limit` files are found and never follows directory links.
pub fn sibling_files(skill: &Skill, limit: usize) -> Vec<String> {
    skill
        .directory()
        .map(|base| discovery::sibling_files(base, limit))
        .unwrap_or_default()
}

/// Split arguments: double- or single-quoted, or whitespace-separated, quotes stripped.
pub fn tokenize(args: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    let mut started = false;
    for c in args.chars() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), c) => current.push(c),
            (None, '"' | '\'') => {
                quote = Some(c);
                started = true;
            }
            (None, c) if c.is_whitespace() => {
                if started || !current.is_empty() {
                    out.push(std::mem::take(&mut current));
                }
                started = false;
            }
            (None, c) => current.push(c),
        }
    }
    if started || !current.is_empty() {
        out.push(current);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fork_agent_selection_is_bounded_and_inline_selectors_are_refused() {
        let tmp = tempfile::tempdir().unwrap();
        for (header, valid) in [
            ("context: fork\nagent: reviewer\n", true),
            ("context: fork\nagent: 'audit reviewer'\n", true),
            ("agent: reviewer\n", false),
            ("context: fork\nagent: ''\n", false),
            ("context: fork\nagent: \"\\n\"\n", false),
        ] {
            std::fs::write(
                tmp.path().join("SKILL.md"),
                format!("---\nname: audit\ndescription: Audit\n{header}---\nInstructions\n"),
            )
            .unwrap();
            assert_eq!(load(tmp.path()).is_ok(), valid, "{header}");
        }
        std::fs::write(
            tmp.path().join("SKILL.md"),
            format!(
                "---\nname: audit\ndescription: Audit\ncontext: fork\nagent: '{}'\n---\nBody\n",
                "x".repeat(129)
            ),
        )
        .unwrap();
        assert!(load(tmp.path()).unwrap_err().contains("agent must"));
    }

    #[test]
    fn context_defaults_to_inline_and_refuses_unknown_execution_modes() {
        let tmp = tempfile::tempdir().unwrap();
        for (declaration, expected) in [
            ("", SkillContext::Inline),
            ("context: inline\n", SkillContext::Inline),
            ("context: fork\n", SkillContext::Fork),
        ] {
            std::fs::write(
                tmp.path().join("SKILL.md"),
                format!("---\nname: release\ndescription: Release\n{declaration}---\nBody\n"),
            )
            .unwrap();
            assert_eq!(load(tmp.path()).unwrap().context, expected);
        }
        std::fs::write(
            tmp.path().join("SKILL.md"),
            "---\nname: release\ndescription: Release\ncontext: detached\n---\nBody\n",
        )
        .unwrap();
        assert!(load(tmp.path()).unwrap_err().contains("unknown variant"));
    }

    #[test]
    fn skill_tool_scopes_capture_lists_and_refuse_unbounded_declarations() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("SKILL.md"), "---\nname: release\ndescription: Release\nallowed-tools: [read, 'bash:git tag *']\ndisallowed-tools: 'write:blocked*, mcp__*'\n---\nBody\n").unwrap();
        let skill = load(tmp.path()).unwrap();
        assert_eq!(skill.allowed_tools, ["read", "bash:git tag *"]);
        assert_eq!(skill.disallowed_tools, ["write:blocked*", "mcp__*"]);
        for pattern in ["", ":resource", "tool:", "tool:bad\0", "tool:bad\n"] {
            assert!(
                validate_tool_patterns(&[pattern.into()]).is_err(),
                "{pattern:?}"
            );
        }
        assert!(validate_tool_patterns(&vec!["read".into(); 65]).is_err());
        assert!(validate_tool_patterns(&["x".repeat(257)]).is_err());
    }

    fn write_skill(dir: &Path, name: &str, description: &str) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(
            dir.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: {description}\n---\n\nBody of {name}.\n"),
        )
        .unwrap();
    }

    #[test]
    fn project_skills_override_user_skills_and_compat_can_be_disabled() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        write_skill(
            &home.join(".config/cyber/skills/notes"),
            "notes",
            "user notes",
        );
        write_skill(&repo.join(".cyber/skills/notes"), "notes", "project notes");
        write_skill(
            &repo.join(".claude/skills/legacy"),
            "legacy",
            "compat skill",
        );
        std::fs::write(repo.join(".cyber/skills/notes/template.md"), "x").unwrap();
        let scope = SkillScope {
            location: repo.clone(),
            home: home.clone(),
            global_config_dir: home.join(".config/cyber"),
            compat: true,
            extra: Vec::new(),
        };
        let found = discover(&scope);
        assert_eq!(found.skills["notes"].description, "project notes");
        assert_eq!(found.skills["notes"].body, "Body of notes.\n");
        assert!(found.skills.contains_key("legacy"));
        assert_eq!(
            sibling_files(&found.skills["notes"], 20),
            vec!["template.md".to_string()]
        );
        let strict = discover(&SkillScope {
            compat: false,
            ..scope
        });
        assert!(!strict.skills.contains_key("legacy"));
    }

    #[test]
    fn invalid_skills_are_reported_not_loaded() {
        let tmp = tempfile::tempdir().unwrap();
        write_skill(&tmp.path().join("Bad"), "Bad_Name", "x");
        std::fs::create_dir_all(tmp.path().join("nofm")).unwrap();
        std::fs::write(tmp.path().join("nofm/SKILL.md"), "just text").unwrap();
        let scope = SkillScope {
            location: tmp.path().join("nowhere"),
            home: tmp.path().join("h"),
            global_config_dir: tmp.path().join("g"),
            compat: false,
            extra: vec![tmp.path().to_path_buf()],
        };
        let found = discover(&scope);
        assert_eq!(found.skills.len(), 5);
        assert!(found.skills.values().all(Skill::is_bundled));
        assert_eq!(found.diagnostics.len(), 2, "{:?}", found.diagnostics);
    }

    fn scope_for(root: &Path) -> SkillScope {
        SkillScope {
            location: root.join("repo"),
            home: root.join("home"),
            global_config_dir: root.join("global"),
            compat: false,
            extra: Vec::new(),
        }
    }

    fn reported(found: &Discovery, needle: &str) -> bool {
        found
            .diagnostics
            .iter()
            .any(|d| d.to_string().replace('\\', "/").contains(needle))
    }

    #[cfg(unix)]
    #[test]
    fn linked_skill_files_and_directories_are_refused_with_diagnostics() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let outside = root.join("outside");
        write_skill(&outside, "outside", "Outside the skill root");
        let skills = root.join("repo/.cyber/skills");
        std::fs::create_dir_all(root.join("repo/.git")).unwrap();
        std::fs::create_dir_all(skills.join("file-link")).unwrap();
        std::os::unix::fs::symlink(outside.join("SKILL.md"), skills.join("file-link/SKILL.md"))
            .unwrap();
        std::os::unix::fs::symlink(&outside, skills.join("dir-link")).unwrap();
        write_skill(&skills.join("real"), "real", "A real skill");
        let found = discover(&scope_for(root));
        assert!(found.skills.contains_key("real"));
        assert!(!found.skills.contains_key("outside"), "{:?}", found.skills);
        assert!(
            reported(&found, "file-link/SKILL.md"),
            "{:?}",
            found.diagnostics
        );
        assert!(
            reported(&found, "linked SKILL.md"),
            "{:?}",
            found.diagnostics
        );
        assert!(reported(&found, "dir-link"), "{:?}", found.diagnostics);
        assert!(
            reported(&found, "linked skill directory"),
            "{:?}",
            found.diagnostics
        );
        // A linked root is refused as a whole.
        let linked_root = root.join("linked-root");
        std::fs::create_dir_all(linked_root.join("repo/.git")).unwrap();
        std::fs::create_dir_all(linked_root.join("repo/.cyber")).unwrap();
        std::os::unix::fs::symlink(&skills, linked_root.join("repo/.cyber/skills")).unwrap();
        let found = discover(&scope_for(&linked_root));
        assert!(!found.skills.contains_key("real"));
        assert!(
            reported(&found, "unavailable or linked"),
            "{:?}",
            found.diagnostics
        );
    }

    #[test]
    fn oversized_skill_md_is_refused_with_a_diagnostic() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("repo/.cyber/skills/huge");
        std::fs::create_dir_all(tmp.path().join("repo/.git")).unwrap();
        std::fs::create_dir_all(&dir).unwrap();
        let mut text = "---\nname: huge\ndescription: Huge\n---\n".to_string();
        text.push_str(&"x".repeat(1024 * 1024));
        std::fs::write(dir.join("SKILL.md"), text).unwrap();
        let found = discover(&scope_for(tmp.path()));
        assert!(!found.skills.contains_key("huge"));
        assert!(reported(&found, "huge/SKILL.md"), "{:?}", found.diagnostics);
        assert!(reported(&found, "limit"), "{:?}", found.diagnostics);
    }

    #[test]
    fn invalid_model_is_reported_at_discovery() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("repo/.git")).unwrap();
        let skills = tmp.path().join("repo/.cyber/skills");
        for (name, model) in [
            ("bare", "gpt"),
            ("variant", "openai/gpt#"),
            ("ok", "openai/gpt-5#high"),
        ] {
            std::fs::create_dir_all(skills.join(name)).unwrap();
            std::fs::write(
                skills.join(name).join("SKILL.md"),
                format!("---\nname: {name}\ndescription: d\nmodel: '{model}'\n---\nBody\n"),
            )
            .unwrap();
        }
        let found = discover(&scope_for(tmp.path()));
        assert_eq!(
            found.skills["ok"].model.as_deref(),
            Some("openai/gpt-5#high")
        );
        for name in ["bare", "variant"] {
            assert!(!found.skills.contains_key(name));
            assert!(
                reported(&found, &format!("{name}/SKILL.md: invalid model")),
                "{:?}",
                found.diagnostics
            );
        }
    }

    #[test]
    fn sibling_walk_is_bounded_and_skips_directory_links() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("skill");
        write_skill(&dir, "many", "Many files");
        for i in 0..50 {
            std::fs::create_dir_all(dir.join(format!("d{i:02}"))).unwrap();
            std::fs::write(dir.join(format!("d{i:02}/f.md")), "x").unwrap();
        }
        let skill = load(&dir).unwrap();
        let files = sibling_files(&skill, 20);
        assert_eq!(files.len(), 20, "{files:?}");
        assert!(!files.iter().any(|f| f == "SKILL.md"));
        #[cfg(unix)]
        {
            let outside = tmp.path().join("outside");
            std::fs::create_dir_all(&outside).unwrap();
            std::fs::write(outside.join("secret.txt"), "x").unwrap();
            let small = tmp.path().join("small");
            write_skill(&small, "small", "Small");
            std::os::unix::fs::symlink(&outside, small.join("linked")).unwrap();
            let files = sibling_files(&load(&small).unwrap(), 20);
            assert_eq!(files, ["linked"], "{files:?}");
        }
    }

    #[test]
    fn duplicate_notice_names_both_paths_and_scope() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        std::fs::create_dir_all(root.join("repo/.git")).unwrap();
        let global = root.join("global/skills/deploy");
        let project = root.join("repo/.cyber/skills/deploy");
        write_skill(&global, "deploy", "global deploy");
        write_skill(&project, "deploy", "project deploy");
        let found = discover(&scope_for(&root));
        assert_eq!(found.skills["deploy"].origin, SkillOrigin::Project);
        assert_eq!(found.skills["deploy"].description, "project deploy");
        let notice = found
            .diagnostics
            .iter()
            .find(|d| d.replaces.as_deref() == Some("deploy"))
            .unwrap();
        assert_eq!(notice.origin, SkillOrigin::Project);
        assert!(notice.to_string().contains(&project.display().to_string()));
        assert!(notice.to_string().contains(&global.display().to_string()));
    }

    #[test]
    fn allowed_tools_accepts_lists_and_strings() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("SKILL.md"),
            "---\nname: a\ndescription: d\nallowed-tools: \"bash(git *), read\"\n---\nb",
        )
        .unwrap();
        assert_eq!(
            load(tmp.path()).unwrap().allowed_tools,
            vec!["bash(git *)", "read"]
        );
        std::fs::write(
            tmp.path().join("SKILL.md"),
            "---\nname: a\ndescription: d\nallowed-tools:\n  - read\n---\nb",
        )
        .unwrap();
        assert_eq!(load(tmp.path()).unwrap().allowed_tools, vec!["read"]);
    }
}

#[cfg(test)]
mod path_tests {
    #[test]
    fn paths_load_as_bounded_relative_globs_and_refuse_escaping_or_invalid_patterns() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("SKILL.md");
        let document = |paths: serde_json::Value| {
            format!("---\nname: migrations\ndescription: Migrations\npaths: {paths}\n---\nBody")
        };
        std::fs::write(
            &file,
            document(serde_json::json!(["db/migrations/**", "src/*.{rs,sql}"])),
        )
        .unwrap();
        assert_eq!(
            super::load(temp.path()).unwrap().paths,
            ["db/migrations/**", "src/*.{rs,sql}"]
        );
        for pattern in [
            "",
            "../outside/**",
            "/absolute/**",
            "C:/outside/**",
            "bad[",
            "db\\migrations\\*",
        ] {
            std::fs::write(&file, document(serde_json::json!([pattern]))).unwrap();
            assert!(super::load(temp.path()).is_err(), "{pattern}");
        }
        std::fs::write(&file, document(serde_json::json!(vec!["db/**"; 65]))).unwrap();
        assert!(super::load(temp.path()).is_err());
    }
}
