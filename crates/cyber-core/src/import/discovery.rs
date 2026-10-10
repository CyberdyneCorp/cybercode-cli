use serde::Serialize;
use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SourceTool {
    OpenCode,
    Codex,
    Claude,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum SourceLayer {
    Global,
    CodexHome,
    Project,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum SourceKind {
    Config,
    Profile,
    Instruction,
    Mcp,
    Agent,
    Command,
    Skill,
    SkillAsset,
    Rule,
    Hook,
    Policy,
    Mode,
    Plugin,
    CustomTool,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SourceFile {
    pub tool: SourceTool,
    pub layer: SourceLayer,
    pub kind: SourceKind,
    pub path: PathBuf,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DiscoveryIssue {
    pub path: PathBuf,
    pub reason: &'static str,
}
#[derive(Debug, Default, Serialize)]
pub struct SourceInventory {
    pub files: Vec<SourceFile>,
    pub issues: Vec<DiscoveryIssue>,
}
#[derive(Debug, thiserror::Error)]
#[error("{reason}")]
pub struct DiscoveryError {
    pub path: PathBuf,
    pub reason: &'static str,
}

/// All search authority comes from these inputs, not process-global environment.
pub struct SourceRoots {
    pub project_root: PathBuf,
    pub directory: PathBuf,
    pub home: PathBuf,
    pub codex_home: Option<PathBuf>,
}

/// Inventory static source locations only. It reads metadata and directory names, never contents.
pub fn discover_sources(roots: &SourceRoots) -> Result<SourceInventory, DiscoveryError> {
    let project = canonical(&roots.project_root)?;
    let directory = canonical(&roots.directory)?;
    let home = canonical(&roots.home)?;
    if !directory.starts_with(&project) {
        return Err(error(
            &directory,
            "source Location is outside the project root",
        ));
    }
    let mut scanner = Scanner {
        inventory: SourceInventory::default(),
        seen: BTreeSet::new(),
        entries: 0,
        issues_seen: BTreeSet::new(),
    };
    // Enum ordering retains the canonical auto-source order.
    for tool in [SourceTool::OpenCode, SourceTool::Codex, SourceTool::Claude] {
        scanner.global(tool, &home, roots.codex_home.as_deref())?;
        let mut layers: Vec<_> = directory
            .ancestors()
            .take_while(|path| path.starts_with(&project))
            .collect();
        layers.reverse();
        for layer in layers {
            scanner.project(tool, layer)?;
        }
    }
    Ok(scanner.inventory)
}

fn error(path: &Path, reason: &'static str) -> DiscoveryError {
    DiscoveryError {
        path: path.into(),
        reason,
    }
}
fn canonical(path: &Path) -> Result<PathBuf, DiscoveryError> {
    let path = path
        .canonicalize()
        .map_err(|_| error(path, "migration root is unavailable"))?;
    if !path.is_dir() {
        return Err(error(&path, "migration root is not a directory"));
    }
    Ok(path)
}

pub(super) fn is_link(metadata: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        metadata.file_attributes()
            & windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT
            != 0
    }
    #[cfg(not(windows))]
    {
        metadata.file_type().is_symlink()
    }
}

struct Scanner {
    inventory: SourceInventory,
    seen: BTreeSet<(SourceTool, PathBuf)>,
    entries: usize,
    issues_seen: BTreeSet<(PathBuf, &'static str)>,
}
#[derive(Clone, Copy)]
enum Filter {
    Markdown,
    Toml,
    Profile,
    Skills,
    Script,
    Rule,
}
impl Filter {
    fn kind(self, path: &Path, kind: SourceKind) -> Option<SourceKind> {
        let name = path.file_name()?.to_str()?;
        let extension = path
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or_default();
        match self {
            Self::Rule if extension == "rules" => Some(SourceKind::Rule),
            Self::Markdown if extension == "md" => Some(kind),
            Self::Toml if extension == "toml" => Some(kind),
            Self::Profile if name.ends_with(".config.toml") => Some(SourceKind::Profile),
            Self::Skills => Some(if name == "SKILL.md" {
                SourceKind::Skill
            } else {
                SourceKind::SkillAsset
            }),
            Self::Script if ["js", "ts", "mjs", "cjs", "mts", "cts"].contains(&extension) => {
                Some(kind)
            }
            _ => None,
        }
    }
}
#[derive(Clone, Copy)]
struct TreeSource {
    tool: SourceTool,
    layer: SourceLayer,
    kind: SourceKind,
    filter: Filter,
    recursive: bool,
}

impl Scanner {
    fn count(&mut self, path: &Path) -> Result<(), DiscoveryError> {
        self.entries += 1;
        if self.entries > 4096 {
            return Err(error(
                path,
                "migration source inventory entry limit exceeded",
            ));
        }
        Ok(())
    }
    fn metadata(&mut self, path: &Path) -> Result<Option<fs::Metadata>, DiscoveryError> {
        self.count(path)?;
        if self.blocked_ancestor(path)? {
            return Ok(None);
        }
        match fs::symlink_metadata(path) {
            Ok(metadata) if is_link(&metadata) => {
                self.issue(
                    path,
                    "source symbolic link or reparse point was not followed",
                );
                Ok(None)
            }
            Ok(metadata) => Ok(Some(metadata)),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(_) => {
                self.issue(path, "source metadata is unavailable");
                Ok(None)
            }
        }
    }
    fn blocked_ancestor(&mut self, path: &Path) -> Result<bool, DiscoveryError> {
        for ancestor in path.ancestors().skip(1) {
            self.count(ancestor)?;
            match fs::symlink_metadata(ancestor) {
                Ok(metadata) if is_link(&metadata) => {
                    self.issue(
                        ancestor,
                        "source ancestor symbolic link or reparse point was not followed",
                    );
                    return Ok(true);
                }
                Ok(metadata) if !metadata.is_dir() => {
                    self.issue(ancestor, "source ancestor is not a directory");
                    return Ok(true);
                }
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(true),
                Err(_) => {
                    self.issue(ancestor, "source parent metadata is unavailable");
                    return Ok(true);
                }
                _ => {}
            }
        }
        Ok(false)
    }
    fn issue(&mut self, path: &Path, reason: &'static str) {
        if self.issues_seen.insert((path.into(), reason)) {
            self.inventory.issues.push(DiscoveryIssue {
                path: path.into(),
                reason,
            });
        }
    }
    fn add(&mut self, tool: SourceTool, layer: SourceLayer, kind: SourceKind, path: &Path) {
        if self.seen.insert((tool, path.into())) {
            self.inventory.files.push(SourceFile {
                tool,
                layer,
                kind,
                path: path.into(),
            });
        }
    }
    fn file(
        &mut self,
        tool: SourceTool,
        layer: SourceLayer,
        kind: SourceKind,
        path: &Path,
    ) -> Result<(), DiscoveryError> {
        if let Some(metadata) = self.metadata(path)? {
            if metadata.is_file() {
                self.add(tool, layer, kind, path);
            } else {
                self.issue(path, "source file is not a regular file");
            }
        }
        Ok(())
    }
    fn tree(
        &mut self,
        tool: SourceTool,
        layer: SourceLayer,
        kind: SourceKind,
        root: &Path,
        filter: Filter,
        recursive: bool,
    ) -> Result<(), DiscoveryError> {
        self.walk(
            root,
            TreeSource {
                tool,
                layer,
                kind,
                filter,
                recursive,
            },
            0,
        )
    }
    fn walk(
        &mut self,
        root: &Path,
        source: TreeSource,
        depth: usize,
    ) -> Result<(), DiscoveryError> {
        let TreeSource {
            tool,
            layer,
            kind,
            filter,
            recursive,
        } = source;
        if depth > 32 {
            return Err(error(root, "migration source tree depth limit exceeded"));
        }
        let Some(metadata) = self.metadata(root)? else {
            return Ok(());
        };
        if !metadata.is_dir() {
            self.issue(root, "source tree is not a directory");
            return Ok(());
        }
        let entries = match fs::read_dir(root) {
            Ok(entries) => entries,
            Err(_) => {
                self.issue(root, "source directory is unavailable");
                return Ok(());
            }
        };
        let mut children = Vec::new();
        for entry in entries {
            self.count(root)?;
            match entry {
                Ok(entry) => {
                    children.push(entry.path());
                }
                Err(_) => self.issue(root, "source directory entry is unavailable"),
            }
        }
        children.sort();
        for child in children {
            let Some(metadata) = self.metadata(&child)? else {
                continue;
            };
            if metadata.is_dir() && recursive {
                self.walk(&child, source, depth + 1)?;
            } else if metadata.is_file()
                && let Some(kind) = filter.kind(&child, kind)
            {
                self.add(tool, layer, kind, &child);
            }
        }
        Ok(())
    }
    fn global(
        &mut self,
        tool: SourceTool,
        home: &Path,
        codex_home: Option<&Path>,
    ) -> Result<(), DiscoveryError> {
        match tool {
            SourceTool::Claude => {
                self.claude(&home.join(".claude"), SourceLayer::Global)?;
                self.file(
                    tool,
                    SourceLayer::Global,
                    SourceKind::Mcp,
                    &home.join(".claude.json"),
                )?;
            }
            SourceTool::Codex => {
                let default = home.join(".codex");
                self.codex(&default, SourceLayer::Global)?;
                if let Some(custom) = codex_home {
                    self.custom_codex(custom, &default)?;
                }
                self.tree(
                    tool,
                    SourceLayer::Global,
                    SourceKind::Skill,
                    &home.join(".agents/skills"),
                    Filter::Skills,
                    true,
                )?;
            }
            SourceTool::OpenCode => {
                self.opencode(&home.join(".config/opencode"), SourceLayer::Global)?
            }
        }
        Ok(())
    }
    fn custom_codex(&mut self, custom: &Path, default: &Path) -> Result<(), DiscoveryError> {
        if !custom.is_absolute()
            || custom.components().any(|part| {
                matches!(
                    part,
                    std::path::Component::ParentDir | std::path::Component::CurDir
                )
            })
        {
            return Err(error(
                custom,
                "explicit CODEX_HOME must be absolute without traversal",
            ));
        }
        let Some(metadata) = self.metadata(custom)? else {
            return Ok(());
        };
        if !metadata.is_dir() {
            self.issue(custom, "custom Codex home is not a directory");
            return Ok(());
        }
        let custom = canonical(custom)?;
        if custom != default {
            self.codex(&custom, SourceLayer::CodexHome)?;
        }
        Ok(())
    }
    fn project(&mut self, tool: SourceTool, root: &Path) -> Result<(), DiscoveryError> {
        let layer = SourceLayer::Project;
        match tool {
            SourceTool::Claude => {
                self.claude(&root.join(".claude"), layer)?;
                self.file(tool, layer, SourceKind::Mcp, &root.join(".mcp.json"))?;
                self.file(
                    tool,
                    layer,
                    SourceKind::Instruction,
                    &root.join("CLAUDE.md"),
                )?;
            }
            SourceTool::Codex => {
                self.codex(&root.join(".codex"), layer)?;
                for name in ["AGENTS.md", "AGENTS.override.md"] {
                    self.file(tool, layer, SourceKind::Instruction, &root.join(name))?;
                }
                self.tree(
                    tool,
                    layer,
                    SourceKind::Skill,
                    &root.join(".agents/skills"),
                    Filter::Skills,
                    true,
                )?;
            }
            SourceTool::OpenCode => {
                self.opencode(&root.join(".opencode"), layer)?;
                for name in ["opencode.json", "opencode.jsonc"] {
                    self.file(tool, layer, SourceKind::Config, &root.join(name))?;
                }
            }
        }
        Ok(())
    }
    fn claude(&mut self, root: &Path, layer: SourceLayer) -> Result<(), DiscoveryError> {
        let tool = SourceTool::Claude;
        for name in ["settings.json", "settings.local.json"] {
            self.file(tool, layer, SourceKind::Config, &root.join(name))?;
        }
        self.file(
            tool,
            layer,
            SourceKind::Instruction,
            &root.join("CLAUDE.md"),
        )?;
        for (name, kind, recursive) in [
            ("agents", SourceKind::Agent, false),
            ("commands", SourceKind::Command, true),
            ("rules", SourceKind::Rule, false),
        ] {
            self.tree(
                tool,
                layer,
                kind,
                &root.join(name),
                Filter::Markdown,
                recursive,
            )?;
        }
        self.tree(
            tool,
            layer,
            SourceKind::Skill,
            &root.join("skills"),
            Filter::Skills,
            true,
        )
    }
    fn codex(&mut self, root: &Path, layer: SourceLayer) -> Result<(), DiscoveryError> {
        let tool = SourceTool::Codex;
        for (name, kind) in [
            ("config.toml", SourceKind::Config),
            ("hooks.json", SourceKind::Hook),
            ("requirements.toml", SourceKind::Policy),
            ("AGENTS.md", SourceKind::Instruction),
            ("AGENTS.override.md", SourceKind::Instruction),
        ] {
            self.file(tool, layer, kind, &root.join(name))?;
        }
        self.tree(
            tool,
            layer,
            SourceKind::Profile,
            root,
            Filter::Profile,
            false,
        )?;
        self.tree(
            tool,
            layer,
            SourceKind::Agent,
            &root.join("agents"),
            Filter::Toml,
            true,
        )?;
        self.tree(
            tool,
            layer,
            SourceKind::Rule,
            &root.join("rules"),
            Filter::Rule,
            false,
        )?;
        self.tree(
            tool,
            layer,
            SourceKind::Skill,
            &root.join("skills"),
            Filter::Skills,
            true,
        )
    }
    fn opencode(&mut self, root: &Path, layer: SourceLayer) -> Result<(), DiscoveryError> {
        let tool = SourceTool::OpenCode;
        for name in [
            "opencode.json",
            "opencode.jsonc",
            "config.json",
            "config.jsonc",
        ] {
            self.file(tool, layer, SourceKind::Config, &root.join(name))?;
        }
        for (name, kind) in [
            ("agents", SourceKind::Agent),
            ("commands", SourceKind::Command),
            ("modes", SourceKind::Mode),
        ] {
            self.tree(tool, layer, kind, &root.join(name), Filter::Markdown, true)?;
        }
        for (name, kind) in [
            ("plugins", SourceKind::Plugin),
            ("tools", SourceKind::CustomTool),
        ] {
            self.tree(tool, layer, kind, &root.join(name), Filter::Script, true)?;
        }
        self.tree(
            tool,
            layer,
            SourceKind::Skill,
            &root.join("skills"),
            Filter::Skills,
            true,
        )
    }
}
