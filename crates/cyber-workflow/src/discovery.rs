//! Saved Workflow discovery (`workflows` → Saved and bundled workflows).
//!
//! Roots are scanned lowest precedence first: the bundled Workflows (when offered), the
//! global `~/.config/cyber/workflows`, then each project `.cyber/workflows` from the
//! outermost ancestor to the Location. A later root replaces an earlier Workflow of the
//! same `meta.name`, so project overrides global and both override bundled.
//! Traversal is bounded and never follows links; unreadable or invalid modules are listed
//! as issues with their diagnostic instead of hiding the rest.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::diagnostic::{Diagnostic, DiagnosticKind, Position};
use crate::module::{MAX_SOURCE_BYTES, Origin, Scope, WorkflowModule};

const MAX_ENTRIES: usize = 4096;
const MAX_DEPTH: usize = 16;

/// One discovery root.
#[derive(Debug, Clone)]
pub struct Root {
    pub scope: Scope,
    pub path: PathBuf,
}

/// The discovery roots for a Location, lowest precedence first.
pub fn roots(global_config_dir: &Path, project: Option<(&Path, &Path)>) -> Vec<Root> {
    let mut roots = vec![Root {
        scope: Scope::Global,
        path: global_config_dir.join("workflows"),
    }];
    if let Some((location, project_root)) = project {
        let mut directories: Vec<&Path> = location
            .ancestors()
            .take_while(|p| p.starts_with(project_root))
            .collect();
        directories.reverse();
        roots.extend(directories.into_iter().map(|dir| Root {
            scope: Scope::Project,
            path: dir.join(".cyber").join("workflows"),
        }));
    }
    roots
}

/// A discovered Workflow and the definitions it replaced.
#[derive(Debug, Clone, Serialize)]
pub struct Entry {
    #[serde(flatten)]
    pub module: WorkflowModule,
    pub shadowed: Vec<PathBuf>,
}

/// A module that could not be loaded.
#[derive(Debug, Clone, Serialize)]
pub struct Issue {
    pub path: PathBuf,
    pub diagnostic: Diagnostic,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Listing {
    pub workflows: Vec<Entry>,
    pub issues: Vec<Issue>,
}

impl Listing {
    pub fn find(&self, name: &str) -> Option<&WorkflowModule> {
        self.workflows
            .iter()
            .find(|e| e.module.meta.name == name)
            .map(|e| &e.module)
    }
}

/// Discover saved Workflows under `roots` (lowest precedence first).
pub fn discover(roots: &[Root]) -> Listing {
    discover_library(roots, false)
}

/// The Workflow library: the bundled Workflows when `bundled`, overridden by the saved
/// ones under `roots` (lowest precedence first).
pub fn discover_library(roots: &[Root], bundled: bool) -> Listing {
    let mut scan = Scan::default();
    if bundled {
        for module in crate::bundled::modules() {
            scan.insert(module.clone());
        }
    }
    for root in roots {
        if let Some(files) = scan.files(&root.path) {
            for path in files {
                scan.load(root.scope, path);
            }
        }
    }
    Listing {
        workflows: scan.found.into_values().collect(),
        issues: scan.issues,
    }
}

#[derive(Default)]
struct Scan {
    found: BTreeMap<String, Entry>,
    issues: Vec<Issue>,
    entries: usize,
}

impl Scan {
    fn issue(&mut self, path: &Path, kind: DiagnosticKind, message: &str) {
        self.issues.push(Issue {
            path: path.into(),
            diagnostic: Diagnostic::new(kind, message),
        });
    }

    /// Module files under one root in sorted order; `None` when the root is absent.
    fn files(&mut self, root: &Path) -> Option<Vec<PathBuf>> {
        let metadata = std::fs::symlink_metadata(root).ok()?;
        if !metadata.is_dir() {
            self.issue(
                root,
                DiagnosticKind::ExecutionFailure,
                "workflow directory is not a plain directory",
            );
            return None;
        }
        let mut files = Vec::new();
        self.walk(root, 0, &mut files);
        Some(files)
    }

    fn walk(&mut self, dir: &Path, depth: usize, files: &mut Vec<PathBuf>) {
        let Some(mut children) = self.children(dir) else {
            return;
        };
        children.sort();
        for child in children {
            let Ok(metadata) = std::fs::symlink_metadata(&child) else {
                continue;
            };
            if metadata.file_type().is_symlink() {
                self.issue(
                    &child,
                    DiagnosticKind::ExecutionFailure,
                    "linked workflow entries are not followed",
                );
            } else if metadata.is_dir() && depth + 1 < MAX_DEPTH {
                self.walk(&child, depth + 1, files);
            } else if metadata.is_file() && has_extension(&child, &["js", "ts"]) {
                files.push(child);
            }
        }
    }

    fn children(&mut self, dir: &Path) -> Option<Vec<PathBuf>> {
        let Ok(read) = std::fs::read_dir(dir) else {
            self.issue(
                dir,
                DiagnosticKind::ExecutionFailure,
                "workflow directory is unreadable",
            );
            return None;
        };
        let mut children = Vec::new();
        for entry in read.flatten() {
            self.entries += 1;
            if self.entries > MAX_ENTRIES {
                self.issue(
                    dir,
                    DiagnosticKind::ExecutionFailure,
                    "workflow directory entry bound exceeded",
                );
                return Some(children);
            }
            children.push(entry.path());
        }
        Some(children)
    }

    fn load(&mut self, scope: Scope, path: PathBuf) {
        match read_module(scope, &path) {
            Ok(module) => self.insert(module),
            Err(diagnostic) => self.issues.push(Issue { path, diagnostic }),
        }
    }

    fn insert(&mut self, module: WorkflowModule) {
        let name = module.meta.name.clone();
        match self.found.get_mut(&name) {
            Some(entry) => {
                let old = std::mem::replace(&mut entry.module, module);
                entry.shadowed.push(definition(&old));
            }
            None => {
                self.found.insert(
                    name,
                    Entry {
                        module,
                        shadowed: Vec::new(),
                    },
                );
            }
        }
    }
}

/// How a shadowed definition is named: its path, or `bundled:<name>`.
fn definition(module: &WorkflowModule) -> PathBuf {
    match &module.origin {
        Origin::Saved { path, .. } => path.clone(),
        Origin::Bundled { name } => PathBuf::from(format!("bundled:{name}")),
        Origin::Inline => PathBuf::new(),
    }
}

fn has_extension(path: &Path, extensions: &[&str]) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| extensions.contains(&e))
}

/// Read and parse one saved module.
pub fn read_module(scope: Scope, path: &Path) -> Result<WorkflowModule, Diagnostic> {
    let start = Position { line: 1, column: 1 };
    if has_extension(path, &["ts"]) {
        return Err(Diagnostic::new(
            DiagnosticKind::UnsupportedSyntax,
            "TypeScript Workflows are not supported yet; save the script as .js",
        )
        .at(Some(start)));
    }
    let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(u64::MAX);
    if size > MAX_SOURCE_BYTES as u64 {
        return Err(Diagnostic::parse(
            format!("script exceeds {MAX_SOURCE_BYTES} bytes"),
            start,
        ));
    }
    let source = std::fs::read_to_string(path)
        .map_err(|e| Diagnostic::parse(format!("cannot read script: {e}"), start))?;
    WorkflowModule::parse(
        source,
        Origin::Saved {
            scope,
            path: path.into(),
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, name: &str, description: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            path,
            format!("export const meta = {{ name: {name:?}, description: {description:?} }};\nexport default async () => 1;\n"),
        )
        .unwrap();
    }

    #[test]
    fn project_overrides_global_and_nearer_project_overrides_outer() {
        let tmp = tempfile::tempdir().unwrap();
        let global = tmp.path().join("config");
        let project = tmp.path().join("repo");
        let location = project.join("sub");
        write(&global.join("workflows/audit.js"), "audit", "global audit");
        write(&global.join("workflows/plan.js"), "plan", "global plan");
        write(
            &project.join(".cyber/workflows/x/audit.js"),
            "audit",
            "project audit",
        );
        write(
            &project.join(".cyber/workflows/migrate.js"),
            "migrate",
            "outer",
        );
        write(
            &location.join(".cyber/workflows/migrate.js"),
            "migrate",
            "inner",
        );
        std::fs::write(
            project.join(".cyber/workflows/bad.js"),
            "export const meta = { name: `${x}` };",
        )
        .unwrap();
        std::fs::write(
            project.join(".cyber/workflows/typed.ts"),
            "export const meta = {};",
        )
        .unwrap();
        std::fs::write(project.join(".cyber/workflows/notes.md"), "ignored").unwrap();

        let listing = discover(&roots(&global, Some((&location, &project))));
        let described: Vec<(&str, &str)> = listing
            .workflows
            .iter()
            .map(|e| {
                (
                    e.module.meta.name.as_str(),
                    e.module.meta.description.as_str(),
                )
            })
            .collect();
        assert_eq!(
            described,
            [
                ("audit", "project audit"),
                ("migrate", "inner"),
                ("plan", "global plan")
            ]
        );
        let audit = &listing.workflows[0];
        assert_eq!(audit.shadowed, [global.join("workflows/audit.js")]);
        assert!(matches!(
            audit.module.origin,
            Origin::Saved {
                scope: Scope::Project,
                ..
            }
        ));
        assert_eq!(
            listing.find("plan").unwrap().meta.description,
            "global plan"
        );

        let kinds: Vec<(String, DiagnosticKind)> = listing
            .issues
            .iter()
            .map(|i| {
                (
                    i.path.file_name().unwrap().to_string_lossy().into_owned(),
                    i.diagnostic.kind,
                )
            })
            .collect();
        assert_eq!(
            kinds,
            [
                ("bad.js".to_string(), DiagnosticKind::ParseError),
                ("typed.ts".to_string(), DiagnosticKind::UnsupportedSyntax)
            ]
        );
    }

    #[test]
    fn bundled_workflows_have_the_lowest_precedence() {
        let tmp = tempfile::tempdir().unwrap();
        let global = tmp.path().join("config");
        let project = tmp.path().join("repo");
        write(&global.join("workflows/plan.js"), "plan", "global plan");
        write(
            &project.join(".cyber/workflows/audit.js"),
            "audit",
            "project audit",
        );
        write(&global.join("workflows/audit.js"), "audit", "global audit");
        let roots = roots(&global, Some((&project, &project)));

        let listing = discover_library(&roots, true);
        let origins: Vec<(&str, String)> = listing
            .workflows
            .iter()
            .map(|e| {
                let origin = match &e.module.origin {
                    Origin::Bundled { .. } => "bundled".to_owned(),
                    Origin::Saved { scope, .. } => format!("{scope:?}").to_lowercase(),
                    Origin::Inline => "inline".to_owned(),
                };
                (e.module.meta.name.as_str(), origin)
            })
            .collect();
        assert_eq!(
            origins,
            [
                ("audit", "project".to_owned()),
                ("migrate", "bundled".to_owned()),
                ("plan", "global".to_owned()),
                ("research", "bundled".to_owned()),
                ("review", "bundled".to_owned()),
                ("solve", "bundled".to_owned()),
            ]
        );
        let audit = &listing.workflows[0];
        assert_eq!(
            audit.shadowed,
            [
                PathBuf::from("bundled:audit"),
                global.join("workflows/audit.js")
            ]
        );
        assert_eq!(
            listing.workflows[2].shadowed,
            [PathBuf::from("bundled:plan")]
        );

        let without = discover_library(&roots, false);
        let names: Vec<&str> = without
            .workflows
            .iter()
            .map(|e| e.module.meta.name.as_str())
            .collect();
        assert_eq!(names, ["audit", "plan"]);
    }

    #[test]
    fn absent_roots_are_empty_and_inline_scripts_load() {
        let tmp = tempfile::tempdir().unwrap();
        let listing = discover(&roots(tmp.path(), None));
        assert!(listing.workflows.is_empty() && listing.issues.is_empty());
        let inline = WorkflowModule::inline(
            "export const meta = { name: 'inline', description: 'one-off' };\nlog(1);",
        )
        .unwrap();
        assert_eq!(inline.origin, Origin::Inline);
    }

    #[test]
    fn deeply_nested_meta_is_an_issue_not_a_crash() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("config/workflows");
        std::fs::create_dir_all(&root).unwrap();
        let levels = 300_000;
        std::fs::write(
            root.join("deep.js"),
            format!(
                "export const meta = {{ name: 'deep', description: 'd', x: {}{} }};",
                "[".repeat(levels),
                "]".repeat(levels)
            ),
        )
        .unwrap();
        write(&root.join("ok.js"), "ok", "fine");
        let listing = discover(&roots(&tmp.path().join("config"), None));
        assert_eq!(listing.workflows.len(), 1);
        assert_eq!(listing.issues.len(), 1);
        assert_eq!(listing.issues[0].diagnostic.message, crate::meta::TOO_DEEP);
    }

    #[cfg(unix)]
    #[test]
    fn linked_entries_are_reported_not_followed() {
        let tmp = tempfile::tempdir().unwrap();
        let outside = tmp.path().join("outside.js");
        write(&outside, "outside", "o");
        let root = tmp.path().join("config/workflows");
        std::fs::create_dir_all(&root).unwrap();
        std::os::unix::fs::symlink(&outside, root.join("link.js")).unwrap();
        let listing = discover(&roots(&tmp.path().join("config"), None));
        assert!(listing.workflows.is_empty());
        assert_eq!(listing.issues.len(), 1);
    }
}
