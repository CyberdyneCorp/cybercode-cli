//! `cyber-sandbox-exec --appcontainer <plan> -- program args` (`sandbox` → Windows
//! enforcement): run one command in a fresh less-privileged AppContainer.
//!
//! The helper creates a single-use identity, grants the plan's roots recursively
//! (read-write under `workspace-write`, read-only otherwise) with the protected and
//! unreadable paths excluded, and launches the command with the helper's own standard
//! streams. Objects the command creates inside a granted directory inherit its grant. Every
//! grant and the identity are revoked when the command exits; a helper terminated with its
//! command leaves a grant journal that the next launch's sweep revokes. Any setup failure
//! refuses the launch: nothing runs outside the container.

use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::fs::OpenOptions;
use std::io;
use std::os::windows::io::{AsHandle, AsRawHandle, BorrowedHandle, OwnedHandle};
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::container_plan::{ContainerPlan, within};
use crate::windows_container::{
    Access, ExistingTreeGrant, ExistingTreePolicy, ExistingTreeRoot, GrantJournal, IdentityGrant,
    Profile,
};
use crate::windows_launch::{StandardStreams, spawn_confined};

/// The most file-system objects one launch grants. Larger trees refuse the launch.
pub const OBJECT_LIMIT: usize = 100_000;

/// The exit status of a refused launch (`EX_UNAVAILABLE`).
pub const UNAVAILABLE: i32 = 69;

/// Run the command after `--appcontainer`; returns its exit status, or [`UNAVAILABLE`] with
/// a `SandboxUnavailableError` line on stderr when confinement cannot be prepared.
pub fn main(args: &[OsString]) -> i32 {
    match run(args) {
        Ok(code) => code as i32,
        Err(error) => {
            eprintln!("cyber-sandbox-exec: SandboxUnavailableError: {error}");
            UNAVAILABLE
        }
    }
}

fn run(args: &[OsString]) -> io::Result<u32> {
    let (plan, program, rest) = parse(args)?;
    let directory = std::env::current_dir()?;
    let program = resolve_program(&program, std::env::var_os("PATH").as_deref())?;
    // Grants of helpers that were killed with their command are revoked first.
    crate::windows_container::sweep(&plan.journal, OBJECT_LIMIT);
    let mut profile = Profile::new()?;
    let (roots, files) = grants(&plan, &program);
    let recorded: Vec<PathBuf> = roots
        .iter()
        .map(|root| root.path.clone())
        .chain(files.iter().cloned())
        .collect();
    // Nothing is granted that a killed helper could not leave on record.
    let journal = GrantJournal::create(&plan.journal, &profile, &recorded)?;
    let mut tree = grant_roots(&profile, &roots)?;
    let mut leases = files
        .iter()
        .map(|file| profile.grant_relocatable(file, Access::Read))
        .collect::<io::Result<Vec<IdentityGrant>>>()?;
    let environment: BTreeMap<String, String> = std::env::vars_os()
        .filter_map(|(key, value)| Some((key.into_string().ok()?, value.into_string().ok()?)))
        .collect();
    let streams = Streams::current()?;
    let mut child = spawn_confined(
        &profile,
        &program,
        &rest,
        &environment,
        &directory,
        streams.borrow(),
        plan.network,
    )?;
    let code = loop {
        match child.wait(Duration::from_secs(3600)) {
            Err(error) if error.kind() == io::ErrorKind::TimedOut => continue,
            result => break result?,
        }
    };
    drop(child);
    // The command already ran: a cleanup failure is reported, never mistaken for a
    // refused launch, and a lease that could not be revoked is retried on drop.
    let cleanup = tree
        .close()
        .and_then(|()| leases.drain(..).try_for_each(IdentityGrant::close))
        .and_then(|()| profile.close());
    // A failed cleanup keeps its journal, so a later launch retries it.
    match cleanup {
        Ok(()) => {
            let _ = journal.finish();
        }
        Err(error) => eprintln!("cyber-sandbox-exec: AppContainer grant cleanup failed: {error}"),
    }
    Ok(code)
}

fn grant_roots(profile: &Profile, roots: &[ExistingTreeRoot]) -> io::Result<ExistingTreeGrant> {
    if roots.is_empty() {
        return Err(io::Error::other(
            "the Windows sandbox has no existing root to grant",
        ));
    }
    profile
        .grant_existing_forest_inheritable(roots, OBJECT_LIMIT)
        .map_err(|error| {
            io::Error::new(
                error.kind(),
                format!(
                    "could not grant the sandbox roots (at most {OBJECT_LIMIT} ordinary files and directories, no links or junctions): {error}"
                ),
            )
        })
}

/// `<plan> -- program args...`.
fn parse(args: &[OsString]) -> io::Result<(ContainerPlan, OsString, Vec<OsString>)> {
    let invalid = |message: &str| io::Error::new(io::ErrorKind::InvalidInput, message.to_owned());
    let [plan, separator, program, rest @ ..] = args else {
        return Err(invalid("expected <plan> -- <program> [args...]"));
    };
    if separator != "--" {
        return Err(invalid("expected -- before the program"));
    }
    let plan = plan
        .to_str()
        .ok_or_else(|| invalid("the plan is not Unicode"))?;
    let plan = ContainerPlan::decode(plan).map_err(|error| invalid(&error))?;
    Ok((plan, program.clone(), rest.to_vec()))
}

/// Directory roots and single files to grant: the plan's roots and readable paths, plus
/// the program's own directory unless the system already lets AppContainers read it.
fn grants(plan: &ContainerPlan, program: &Path) -> (Vec<ExistingTreeRoot>, Vec<PathBuf>) {
    let access = if plan.writable {
        Access::Write
    } else {
        Access::Read
    };
    let mut entries: Vec<(PathBuf, Access)> =
        plan.roots.iter().map(|p| (p.clone(), access)).collect();
    entries.extend(plan.readable.iter().map(|p| (p.clone(), Access::Read)));
    if let Some(parent) = program.parent()
        && !system_directory(parent)
        && !entries.iter().any(|(root, _)| within(program, root))
    {
        entries.push((parent.to_path_buf(), Access::Read));
    }
    let mut roots = Vec::new();
    let mut files = Vec::new();
    for (path, access) in entries {
        let Ok(path) = std::fs::canonicalize(&path) else {
            continue;
        };
        if path.is_file() {
            files.push(path);
        } else if path.is_dir() {
            roots.push(ExistingTreeRoot {
                policy: ExistingTreePolicy {
                    access,
                    read_only: inside(&plan.read_only, &path),
                    unreadable: inside(&plan.unreadable, &path),
                },
                path,
            });
        }
    }
    (roots, files)
}

/// The existing paths of `list` inside `root`, canonical like the root.
fn inside(list: &[PathBuf], root: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = list
        .iter()
        .filter_map(|path| std::fs::canonicalize(path).ok())
        .filter(|path| path.starts_with(root) && path != root)
        .collect();
    out.sort();
    out.dedup();
    out
}

/// Windows and program directories readable by every AppContainer through their own ACLs.
fn system_directory(path: &Path) -> bool {
    [
        "SystemRoot",
        "ProgramFiles",
        "ProgramFiles(x86)",
        "ProgramW6432",
    ]
    .iter()
    .filter_map(std::env::var_os)
    .any(|root| within(path, Path::new(&root)))
}

/// The executable to launch: an absolute path as given, else the first `PATH` match with
/// `.exe` or `.com` appended when missing. Batch files need `cmd.exe`, which would parse
/// their arguments again, so they are refused.
fn resolve_program(program: &OsStr, path: Option<&OsStr>) -> io::Result<PathBuf> {
    let given = Path::new(program);
    let candidates: Vec<PathBuf> = if given.is_absolute() {
        vec![given.to_path_buf()]
    } else if given.components().count() == 1 {
        let dirs: Vec<PathBuf> = path
            .map(|p| std::env::split_paths(p).collect())
            .unwrap_or_default();
        dirs.iter()
            .filter(|dir| dir.is_absolute())
            .flat_map(|dir| {
                executable_names(given)
                    .into_iter()
                    .map(move |name| dir.join(name))
            })
            .collect()
    } else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "the program {} must be absolute or a bare name",
                given.display()
            ),
        ));
    };
    let found = candidates
        .into_iter()
        .find(|candidate| candidate.is_file())
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                format!("program not found: {}", given.display()),
            )
        })?;
    let extension = found
        .extension()
        .and_then(OsStr::to_str)
        .unwrap_or_default()
        .to_ascii_lowercase();
    if matches!(extension.as_str(), "cmd" | "bat") {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            format!(
                "batch files cannot run inside the Windows sandbox: {}",
                found.display()
            ),
        ));
    }
    Ok(found)
}

fn executable_names(name: &Path) -> Vec<PathBuf> {
    if name.extension().is_some() {
        return vec![name.to_path_buf()];
    }
    ["exe", "com"]
        .iter()
        .map(|extension| name.with_extension(extension))
        .collect()
}

/// Owned copies of the helper's standard streams, with the null device standing in for a
/// missing one.
struct Streams([OwnedHandle; 3]);

impl Streams {
    fn current() -> io::Result<Self> {
        Ok(Self([
            owned(io::stdin().as_handle(), false)?,
            owned(io::stdout().as_handle(), true)?,
            owned(io::stderr().as_handle(), true)?,
        ]))
    }

    fn borrow(&self) -> StandardStreams<'_> {
        StandardStreams {
            stdin: self.0[0].as_handle(),
            stdout: self.0[1].as_handle(),
            stderr: self.0[2].as_handle(),
        }
    }
}

fn owned(handle: BorrowedHandle<'_>, write: bool) -> io::Result<OwnedHandle> {
    let raw = handle.as_raw_handle();
    if raw.is_null() || raw as isize == -1 {
        let null = OpenOptions::new().read(!write).write(write).open("NUL")?;
        return Ok(OwnedHandle::from(null));
    }
    handle.try_clone_to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn programs_resolve_on_path_and_batch_files_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let bin = std::fs::canonicalize(dir.path()).unwrap();
        std::fs::write(bin.join("tool.exe"), "").unwrap();
        std::fs::write(bin.join("script.cmd"), "").unwrap();
        let path = std::env::join_paths([bin.clone()]).unwrap();
        assert_eq!(
            resolve_program(OsStr::new("tool"), Some(&path)).unwrap(),
            bin.join("tool.exe")
        );
        let error = resolve_program(OsStr::new("script.cmd"), Some(&path)).unwrap_err();
        assert!(error.to_string().contains("batch files"), "{error}");
        assert!(resolve_program(OsStr::new("missing"), Some(&path)).is_err());
        assert!(resolve_program(OsStr::new("sub\\tool.exe"), Some(&path)).is_err());
    }

    #[test]
    fn malformed_invocations_are_refused_before_any_identity_exists() {
        let plan = r#"{"roots":[],"writable":true,"readable":[],"read_only":[],"unreadable":[],"network":false,"journal":"C:\\journal"}"#;
        for args in [
            vec![],
            vec![plan, "program"],
            vec![plan, "-", "program"],
            vec!["{}", "--", "program"],
        ] {
            let args: Vec<OsString> = args.into_iter().map(OsString::from).collect();
            assert!(parse(&args).is_err(), "{args:?}");
        }
    }

    #[test]
    fn exclusions_outside_a_root_or_missing_are_dropped() {
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        let repo = root.join("repo");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        std::fs::create_dir_all(root.join("other")).unwrap();
        let plan = ContainerPlan {
            roots: vec![repo.clone(), root.join("missing")],
            writable: true,
            readable: vec![],
            read_only: vec![repo.join(".git"), repo.join("cyber.jsonc")],
            unreadable: vec![root.join("other")],
            network: false,
            journal: root.join("journal"),
        };
        let program = PathBuf::from(std::env::var_os("SystemRoot").unwrap())
            .join("System32")
            .join("cmd.exe");
        let (roots, files) = grants(&plan, &program);
        assert!(files.is_empty());
        assert_eq!(roots.len(), 1);
        assert_eq!(roots[0].policy.read_only, vec![repo.join(".git")]);
        assert!(roots[0].policy.unreadable.is_empty());
    }
}
