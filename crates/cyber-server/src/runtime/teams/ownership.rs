//! File ownership in shared-Location teams (`agent-teams` → File conflict avoidance;
//! design section 9). In a team with any `shared` teammate, a member working in the
//! shared Location may not mutate a path another member `owns`: permission evaluation
//! denies it with `Path owned by teammate <name>` before any rule or ask. The lead is not
//! exempt (it merges or messages instead). Members in their own worktree are not checked,
//! since their paths are outside the shared Location.

use std::path::{Component, Path, PathBuf};

use cyber_core::teams::{Isolation, owns_path};

use super::{Member, Role};
use crate::runtime::{Runtime, RuntimeError, SessionInfo};

/// The paths other members own, as one member of a shared-Location team sees them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ownership {
    /// The team directory the globs are relative to.
    pub directory: PathBuf,
    /// Other members with ownership globs, in member order.
    pub owners: Vec<(String, Vec<String>)>,
}

impl Ownership {
    /// The member that owns `path` (absolute), if another member does.
    pub fn owner_of(&self, path: &Path) -> Option<&str> {
        self.owner_besides(path, None)
    }

    /// The member other than `except` that owns `path` (absolute), if one does.
    fn owner_besides(&self, path: &Path, except: Option<&str>) -> Option<&str> {
        relative_forms(path, &self.directory)
            .iter()
            .find_map(|relative| {
                self.owners.iter().find(|(name, globs)| {
                    Some(name.as_str()) != except && owns_path(globs, relative)
                })
            })
            .map(|(name, _)| name.as_str())
    }

    /// `Path owned by teammate <name>` for the first of `paths` another member owns.
    /// `merged` names the teammate whose branch `team_merge` lands: its own paths are
    /// its work arriving, while a path any other member owns is still denied.
    pub fn denial(&self, paths: &[PathBuf], merged: Option<&str>) -> Option<String> {
        paths
            .iter()
            .find_map(|path| self.owner_besides(path, merged))
            .map(|name| format!("{OWNED_PREFIX} {name}"))
    }
}

/// The start of every ownership denial.
pub const OWNED_PREFIX: &str = "Path owned by teammate";

impl Runtime {
    /// The ownership guard of `root` (the root Session of the calling tool, so a
    /// teammate's subagents are bound like the teammate): `None` outside a team, in a team
    /// without a `shared` teammate, for a member in its own worktree, or when no other
    /// member owns anything. A teammate whose member row is not recorded yet (its
    /// worktree setup runs before) is checked as a shared member; a team that cannot be
    /// read fails the call (fail closed).
    pub fn team_ownership(&self, root: &SessionInfo) -> Result<Option<Ownership>, RuntimeError> {
        // A lead carries no team tag (it led no team when it was created).
        let (team_id, name) = match &root.team {
            Some(tag) => (tag.team_id.clone(), tag.name.clone()),
            None => match self.team_membership(&root.id)? {
                Some((team, me)) => (team.id, me.name),
                None => return Ok(None),
            },
        };
        let team = self.inner.team(&team_id)?.ok_or_else(|| {
            RuntimeError::Invalid(format!(
                "Team {team_id} of Session {} is not recorded",
                root.id
            ))
        })?;
        let members = self.team_members(&team.id)?;
        let isolation = members
            .iter()
            .find(|member| member.session_id == root.id)
            .map_or(Isolation::Shared, |member| member.isolation);
        Ok(guard(&team.directory, &name, isolation, &members))
    }
}

fn guard(directory: &str, me: &str, isolation: Isolation, members: &[Member]) -> Option<Ownership> {
    let shared_team = members
        .iter()
        .any(|member| member.role == Role::Teammate && member.isolation == Isolation::Shared);
    if !shared_team || isolation == Isolation::Worktree {
        return None;
    }
    let owners: Vec<(String, Vec<String>)> = members
        .iter()
        .filter(|member| member.name != me && !member.owns.is_empty())
        .map(|member| (member.name.clone(), member.owns.clone()))
        .collect();
    (!owners.is_empty()).then(|| Ownership {
        directory: PathBuf::from(directory),
        owners,
    })
}

/// The forms of `path` relative to `directory`, `/`-separated (empty for the directory
/// itself): the lexical one, and the one through the canonical directory and the
/// canonical longest existing ancestor of `path`. Both are checked, so neither a symlink
/// inside the directory (`s2 -> server`), nor `/tmp` against `/private/tmp`, nor a `\\?\`
/// prefix hides an owned path. Empty outside the directory.
fn relative_forms(path: &Path, directory: &Path) -> Vec<String> {
    let lexical_path = normalized(path);
    let lexical = lexical_path
        .strip_prefix(normalized(directory))
        .ok()
        .map(slashed);
    let canonical = std::fs::canonicalize(directory).ok().and_then(|directory| {
        let path = canonical_prefix(path).or_else(|| canonical_prefix(&lexical_path))?;
        path.strip_prefix(&directory).ok().map(slashed)
    });
    lexical.into_iter().chain(canonical).collect()
}

/// Lexically normalized: `.` dropped, `..` applied.
fn normalized(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// The canonical form of the longest existing ancestor of `path`, with the rest of the
/// path appended.
fn canonical_prefix(path: &Path) -> Option<PathBuf> {
    let mut rest = Vec::new();
    let mut current = path;
    loop {
        if let Ok(canonical) = std::fs::canonicalize(current) {
            return Some(
                rest.iter()
                    .rev()
                    .fold(canonical, |acc, part| acc.join(part)),
            );
        }
        rest.push(current.file_name()?.to_owned());
        current = current.parent()?;
    }
}

fn slashed(relative: &Path) -> String {
    relative
        .components()
        .map(|part| part.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::teams::MemberStatus;

    fn member(name: &str, role: Role, isolation: Isolation, owns: &[&str]) -> Member {
        Member {
            team_id: "team_1".into(),
            name: name.into(),
            session_id: format!("ses_{name}"),
            role,
            agent: "build".into(),
            model: None,
            isolation,
            worktree_id: None,
            branch: None,
            owns: owns.iter().map(|glob| (*glob).to_owned()).collect(),
            status: MemberStatus::Idle,
            last_status: None,
        }
    }

    #[test]
    fn shared_members_are_denied_paths_other_members_own() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        let members = vec![
            member("lead", Role::Lead, Isolation::Shared, &[]),
            member("api", Role::Teammate, Isolation::Shared, &["server/**"]),
            member("ui", Role::Teammate, Isolation::Shared, &["web/**"]),
            member("solo", Role::Teammate, Isolation::Worktree, &["solo/**"]),
        ];
        let directory = root.display().to_string();
        let of = |index: usize| {
            let me: &Member = &members[index];
            guard(&directory, &me.name, me.isolation, &members)
        };
        let ui = of(2).unwrap();
        assert_eq!(
            ui.denial(&[root.join("web/app.ts"), root.join("server/api.rs")], None),
            Some("Path owned by teammate api".into())
        );
        assert_eq!(ui.owner_of(&root.join("web/app.ts")), None, "its own");
        assert_eq!(ui.owner_of(&root.join("server")), Some("api"));
        assert_eq!(ui.owner_of(&root.join("web/../server/x.rs")), Some("api"));
        assert_eq!(ui.owner_of(&root.join("solo/x")), Some("solo"));
        assert_eq!(ui.owner_of(Path::new("/elsewhere/server/x.rs")), None);
        // The lead is not exempt.
        let lead = of(0).unwrap();
        assert_eq!(lead.owner_of(&root.join("web/a.ts")), Some("ui"));
        // A merge of `solo`'s branch lands its own paths, never another member's.
        let merged = [root.join("solo/x"), root.join("server/y.rs")];
        assert_eq!(
            lead.denial(&merged[..1], Some("solo")),
            None,
            "the merged teammate's own"
        );
        assert_eq!(
            lead.denial(&merged, Some("solo")),
            Some("Path owned by teammate api".into())
        );
        assert_eq!(
            lead.denial(&merged[..1], None),
            Some("Path owned by teammate solo".into())
        );
        // A member in its own worktree is not checked.
        assert_eq!(of(3), None);
        // A teammate not recorded yet is checked as a shared member.
        let pending = guard(&directory, "new", Isolation::Shared, &members).unwrap();
        assert_eq!(pending.owner_of(&root.join("web/a.ts")), Some("ui"));
        // Without a shared teammate there is no guard.
        let isolated = vec![
            member("lead", Role::Lead, Isolation::Shared, &[]),
            member("api", Role::Teammate, Isolation::Worktree, &["server/**"]),
        ];
        assert_eq!(
            guard(&directory, "lead", Isolation::Shared, &isolated),
            None
        );
    }

    #[test]
    fn owned_paths_are_found_through_the_canonical_directory() {
        let dir = tempfile::tempdir().unwrap();
        let real = std::fs::canonicalize(dir.path()).unwrap();
        std::fs::create_dir_all(real.join("server")).unwrap();
        let link = dir.path().join("link");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&real, &link).unwrap();
        #[cfg(windows)]
        std::os::windows::fs::symlink_dir(&real, &link)
            .unwrap_or_else(|_| std::fs::create_dir_all(&link).unwrap());
        let ownership = Ownership {
            directory: real.clone(),
            owners: vec![("api".into(), vec!["server/**".into()])],
        };
        if std::fs::canonicalize(&link).ok().as_deref() == Some(real.as_path()) {
            assert_eq!(
                ownership.owner_of(&link.join("server/new/file.rs")),
                Some("api")
            );
        }
        assert_eq!(ownership.owner_of(&real.join("server/api.rs")), Some("api"));
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_inside_the_directory_does_not_hide_an_owned_path() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        std::fs::create_dir_all(root.join("server")).unwrap();
        // `ln -s server s2` by a shared teammate.
        std::os::unix::fs::symlink("server", root.join("s2")).unwrap();
        let ownership = Ownership {
            directory: root.clone(),
            owners: vec![("api".into(), vec!["server/**".into()])],
        };
        assert_eq!(ownership.owner_of(&root.join("s2/api.rs")), Some("api"));
        assert_eq!(ownership.owner_of(&root.join("s2/new/x.rs")), Some("api"));
        assert_eq!(ownership.owner_of(&root.join("s2")), Some("api"));
        assert_eq!(ownership.owner_of(&root.join("web/x.rs")), None);
    }
}
