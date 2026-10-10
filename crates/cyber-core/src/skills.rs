//! Skill discovery and `SKILL.md` parsing (`skills-commands`).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;

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
    pub paths: Vec<String>,
    pub model: Option<String>,
    pub disable_model_invocation: bool,
    pub user_invocable: bool,
    pub argument_hint: Option<String>,
    /// The directory holding `SKILL.md`.
    pub base: PathBuf,
    pub body: String,
}

#[derive(Debug, Clone, Default)]
pub struct Discovery {
    /// Skills by name; later locations replace earlier ones.
    pub skills: BTreeMap<String, Skill>,
    /// Invalid skills, as `path: reason`.
    pub diagnostics: Vec<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "kebab-case")]
struct Frontmatter {
    name: String,
    description: String,
    #[serde(default)]
    allowed_tools: Tools,
    #[serde(default)]
    paths: Vec<String>,
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

/// Folders searched, in precedence order (later wins).
pub fn roots(scope: &SkillScope) -> Vec<PathBuf> {
    let mut roots = vec![scope.global_config_dir.join("skills")];
    if scope.compat {
        roots.extend(
            [".claude/skills", ".agents/skills", ".codex/skills"].map(|d| scope.home.join(d)),
        );
    }
    let project = crate::config::project_root(&scope.location);
    let mut dirs: Vec<&Path> = scope
        .location
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
        roots.extend(names.iter().map(|n| dir.join(n)));
    }
    roots.extend(scope.extra.iter().cloned());
    roots
}

pub fn discover(scope: &SkillScope) -> Discovery {
    let mut out = Discovery::default();
    for root in roots(scope) {
        for dir in skill_dirs(&root) {
            match load(&dir) {
                Ok(skill) => {
                    out.skills.insert(skill.name.clone(), skill);
                }
                Err(reason) => out
                    .diagnostics
                    .push(format!("{}: {reason}", dir.join("SKILL.md").display())),
            }
        }
    }
    out
}

/// A root that is itself a skill directory, or the skill directories directly inside it.
fn skill_dirs(root: &Path) -> Vec<PathBuf> {
    if root.join("SKILL.md").is_file() {
        return vec![root.to_path_buf()];
    }
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut dirs: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.join("SKILL.md").is_file())
        .collect();
    dirs.sort();
    dirs
}

pub fn load(dir: &Path) -> Result<Skill, String> {
    let text = std::fs::read_to_string(dir.join("SKILL.md")).map_err(|e| e.to_string())?;
    let (yaml, body) = split_frontmatter(&text).ok_or("missing YAML frontmatter")?;
    let fm: Frontmatter =
        serde_yaml_ng::from_str(yaml).map_err(|e| format!("invalid frontmatter: {e}"))?;
    validate_name(&fm.name)?;
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
    Ok(Skill {
        name: fm.name,
        description: fm.description.trim().to_string(),
        allowed_tools: fm.allowed_tools.into_vec(),
        paths: fm.paths,
        model: fm.model,
        disable_model_invocation: fm.disable_model_invocation,
        user_invocable: fm.user_invocable.unwrap_or(true),
        argument_hint: fm.argument_hint,
        base: dir.to_path_buf(),
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

/// Up to `limit` files beside `SKILL.md`, as sorted relative paths.
pub fn sibling_files(skill: &Skill, limit: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut stack = vec![skill.base.clone()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        let mut entries: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
        entries.sort();
        for path in entries {
            if path.is_dir() {
                stack.push(path);
            } else if let Ok(rel) = path.strip_prefix(&skill.base)
                && rel != Path::new("SKILL.md")
            {
                out.push(rel.to_string_lossy().replace('\\', "/"));
            }
        }
    }
    out.sort();
    out.truncate(limit);
    out
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

/// Substitute `$ARGUMENTS` and `$1..$N`; the highest placeholder takes the remaining tokens.
/// Without placeholders, non-empty arguments are appended as `ARGUMENTS: <args>`.
pub fn expand(template: &str, args: &str) -> String {
    let numbered = regex_lite_positions(template);
    let has_all = template.contains("$ARGUMENTS");
    if numbered.is_empty() && !has_all {
        return if args.trim().is_empty() {
            template.to_string()
        } else {
            format!("{}\n\nARGUMENTS: {args}", template.trim_end())
        };
    }
    let tokens = tokenize(args);
    let highest = numbered.iter().copied().max().unwrap_or(0);
    let value = |n: usize| -> String {
        if n == highest {
            tokens
                .get(n - 1..)
                .map(|rest| rest.join(" "))
                .unwrap_or_default()
        } else {
            tokens.get(n - 1).cloned().unwrap_or_default()
        }
    };
    let mut out = template.replace("$ARGUMENTS", args);
    // Replace longer numbers first so `$10` is not read as `$1` followed by `0`.
    let mut sorted = numbered;
    sorted.sort_unstable_by(|a, b| b.cmp(a));
    for n in sorted {
        out = out.replace(&format!("${n}"), &value(n));
    }
    out
}

/// Distinct `$<n>` placeholders (n ≥ 1) in a template.
fn regex_lite_positions(template: &str) -> Vec<usize> {
    let mut found = Vec::new();
    let bytes = template.as_bytes();
    for (i, _) in template.match_indices('$') {
        let digits: String = bytes[i + 1..]
            .iter()
            .take_while(|b| b.is_ascii_digit())
            .map(|&b| b as char)
            .collect();
        if let Ok(n) = digits.parse::<usize>()
            && n >= 1
            && !found.contains(&n)
        {
            found.push(n);
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert!(found.skills.is_empty());
        assert_eq!(found.diagnostics.len(), 2, "{:?}", found.diagnostics);
    }

    #[test]
    fn arguments_substitute_positionally_and_append_without_placeholders() {
        assert_eq!(
            tokenize(r#"one "two words" 'three'"#),
            vec!["one", "two words", "three"]
        );
        assert_eq!(
            expand("Fix $1 in $2", "bug src/a.rs and more"),
            "Fix bug in src/a.rs and more"
        );
        assert_eq!(expand("All: $ARGUMENTS", "a b"), "All: a b");
        assert_eq!(expand("Do it.\n", "now"), "Do it.\n\nARGUMENTS: now");
        assert_eq!(expand("Do it.", ""), "Do it.");
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
