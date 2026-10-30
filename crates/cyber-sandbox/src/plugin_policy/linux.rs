//! Linux-specific mount ordering and Landlock input; no native IO or execution.
use super::{Error, LinuxPlan, LinuxPolicy, MaskPath};
use std::collections::BTreeSet;
use std::fmt::Write;
use std::path::{Component, Path};

const MAX_ROOTS: usize = 64;
const MAX_PROFILE: usize = 64 * 1024;
const MAX_ARGUMENTS: usize = 1024;
const MAX_ARGUMENT_BYTES: usize = 128 * 1024;

fn mask_path<'a>(mask: &MaskPath<'a>) -> &'a Path {
    match mask {
        MaskPath::File(path) | MaskPath::Directory(path) => path,
    }
}
fn path(path: &Path) -> Result<&str, Error> {
    let text = path.to_str().ok_or(Error::Input)?;
    if !path.is_absolute()
        || path.parent().is_none()
        || text.len() > 4096
        || text.chars().any(char::is_control)
        || path.components().count() > 64
        || path
            .components()
            .any(|part| !matches!(part, Component::RootDir | Component::Normal(_)))
    {
        return Err(Error::Input);
    }
    Ok(text)
}
fn overlaps(a: &Path, b: &Path) -> bool {
    a.starts_with(b) || b.starts_with(a)
}
fn pseudo(path: &Path) -> bool {
    path.starts_with("/proc") || path.starts_with("/dev")
}
fn startup_resources<'a>(p: &'a LinuxPolicy<'a>) -> impl Iterator<Item = &'a Path> {
    [p.helper, p.target, p.profile, p.package, p.mask_file]
        .into_iter()
        .chain(p.bootstrap.iter().copied())
}
fn immutable<'a>(p: &'a LinuxPolicy<'a>) -> impl Iterator<Item = &'a Path> {
    startup_resources(p).chain(p.read_only.iter().copied())
}
fn validate_masks(masks: &[MaskPath<'_>]) -> Result<(), Error> {
    for (index, a) in masks.iter().enumerate() {
        for b in masks.iter().skip(index + 1) {
            let (pa, pb) = (mask_path(a), mask_path(b));
            if pa == pb && std::mem::discriminant(a) != std::mem::discriminant(b) {
                return Err(Error::Input);
            }
            if (matches!(a, MaskPath::File(_)) && pa != pb && pb.starts_with(pa))
                || (matches!(b, MaskPath::File(_)) && pa != pb && pa.starts_with(pb))
            {
                return Err(Error::Overlap);
            }
        }
    }
    Ok(())
}
fn validate(p: &LinuxPolicy<'_>) -> Result<(), Error> {
    if p.bootstrap
        .len()
        .saturating_add(p.private.len())
        .saturating_add(p.read_only.len())
        .saturating_add(7)
        > MAX_ROOTS
    {
        return Err(Error::Limit);
    }
    let roots = immutable(p)
        .chain([p.location, p.scratch])
        .chain(p.private.iter().map(mask_path));
    let mut bytes = 0;
    for root in roots {
        bytes += path(root)?.len();
        if bytes > MAX_PROFILE {
            return Err(Error::Limit);
        }
        if pseudo(root) {
            return Err(Error::Input);
        }
    }
    validate_masks(p.private)?;
    if p.location.starts_with(p.scratch) {
        return Err(Error::Overlap);
    }
    for root in immutable(p) {
        if overlaps(root, p.scratch) || p.location.starts_with(root) {
            return Err(Error::Overlap);
        }
    }
    if startup_resources(p).any(|root| p.private.iter().any(|mask| overlaps(root, mask_path(mask))))
    {
        return Err(Error::Overlap);
    }
    if p.private
        .iter()
        .any(|mask| overlaps(mask_path(mask), p.scratch) || p.location.starts_with(mask_path(mask)))
    {
        return Err(Error::Overlap);
    }
    let prefix = [
        "--plugin-native-exec",
        path(p.profile)?,
        "--initial-shell=0",
        path(p.target)?,
    ];
    let mut count = prefix.len();
    let mut bytes: usize = prefix.iter().map(|arg| arg.len() + 1).sum();
    if p.target_args.len() > 252 {
        return Err(Error::Limit);
    }
    for arg in p.target_args {
        if arg.len() > MAX_PROFILE {
            return Err(Error::Limit);
        }
        if arg.contains('\0') {
            return Err(Error::Input);
        }
        count += 1;
        bytes = bytes.saturating_add(arg.len() + 1);
        if count > 256 || bytes > MAX_PROFILE {
            return Err(Error::Limit);
        }
    }
    Ok(())
}
#[derive(Default)]
struct Arguments {
    values: Vec<String>,
    bytes: usize,
}
impl Arguments {
    fn add<'a>(&mut self, parts: impl IntoIterator<Item = &'a str>) -> Result<(), Error> {
        for part in parts {
            if self.values.len() >= MAX_ARGUMENTS
                || self.bytes + part.len() + 1 > MAX_ARGUMENT_BYTES
            {
                return Err(Error::Limit);
            }
            self.bytes += part.len() + 1;
            self.values.push(part.into());
        }
        Ok(())
    }
    fn bind(&mut self, mode: &str, root: &Path) -> Result<(), Error> {
        self.add([mode, path(root)?, path(root)?])
    }
    fn hide_directory(&mut self, root: &Path) -> Result<(), Error> {
        self.add([
            "--perms",
            "0000",
            "--tmpfs",
            path(root)?,
            "--remount-ro",
            path(root)?,
        ])
    }
}
fn anchors<'a>(p: &'a LinuxPolicy<'a>) -> Vec<&'a Path> {
    let mut set = BTreeSet::new();
    for root in immutable(p).chain(p.private.iter().map(mask_path)) {
        for parent in root.ancestors().skip(1) {
            if parent != p.location && parent.starts_with(p.location) {
                set.insert(parent);
            }
        }
    }
    let mut paths: Vec<_> = set.into_iter().collect();
    paths.sort_by(|a, b| {
        a.components()
            .count()
            .cmp(&b.components().count())
            .then_with(|| a.cmp(b))
    });
    paths
}
fn masks<'a>(p: &'a LinuxPolicy<'a>) -> Vec<MaskPath<'a>> {
    let mut selected = Vec::new();
    for mask in p.private.iter().copied() {
        let root = mask_path(&mask);
        if p.private.iter().any(|other| matches!(other, MaskPath::Directory(parent) if root != *parent && root.starts_with(parent))) {
            continue;
        }
        if !selected.iter().any(|other| mask_path(other) == root) {
            selected.push(mask);
        }
    }
    selected
}
fn mounts(p: &LinuxPolicy<'_>) -> Result<Vec<String>, Error> {
    let mut args = Arguments::default();
    args.add([
        "--die-with-parent",
        "--unshare-user",
        "--unshare-pid",
        "--unshare-net",
        "--unshare-ipc",
        "--unshare-uts",
        "--disable-userns",
        "--cap-drop",
        "ALL",
        "--ro-bind",
        "/",
        "/",
        "--proc",
        "/proc",
        "--dev",
        "/dev",
    ])?;
    let mode = if p.fs_write { "--bind" } else { "--ro-bind" };
    args.bind(mode, p.location)?;
    for parent in anchors(p) {
        args.bind(mode, parent)?;
    }
    let readonly: BTreeSet<_> = immutable(p).collect();
    for root in readonly {
        args.bind("--ro-bind", root)?;
    }
    for mask in masks(p) {
        match mask {
            MaskPath::Directory(root) => args.hide_directory(root)?,
            MaskPath::File(root) => args.add(["--ro-bind", path(p.mask_file)?, path(root)?])?,
        }
    }
    if p.namespace_proc {
        for root in [Path::new("/proc/1"), Path::new("/proc/sys")] {
            args.hide_directory(root)?;
        }
    }
    args.bind("--bind", p.scratch)?;
    args.add([
        "--chdir",
        path(p.package)?,
        "--",
        path(p.helper)?,
        "--plugin-native-exec",
        path(p.profile)?,
        if p.shell {
            "--initial-shell=1"
        } else {
            "--initial-shell=0"
        },
        path(p.target)?,
    ])?;
    args.add(p.target_args.iter().map(String::as_str))?;
    Ok(args.values)
}
fn grant(text: &mut String, right: char, root: &Path) -> Result<(), Error> {
    let root = path(root)?;
    if text.len() + root.len() + 3 > MAX_PROFILE {
        return Err(Error::Limit);
    }
    writeln!(text, "{right}\t{root}").map_err(|_| Error::Limit)
}
fn landlock(p: &LinuxPolicy<'_>) -> Result<String, Error> {
    let mut text = String::new();
    for root in [p.target, p.package, p.scratch]
        .into_iter()
        .chain(p.bootstrap.iter().copied())
    {
        grant(&mut text, 'R', root)?;
    }
    for root in ["/dev/null", "/dev/urandom", "/dev/random"] {
        grant(&mut text, 'R', Path::new(root))?;
    }
    grant(&mut text, 'W', Path::new("/dev/null"))?;
    grant(&mut text, 'W', p.scratch)?;
    if p.fs_read {
        grant(&mut text, 'R', p.location)?;
    }
    if p.fs_write {
        grant(&mut text, 'W', p.location)?;
    }
    if p.namespace_proc {
        grant(&mut text, 'R', Path::new("/proc"))?;
    }
    Ok(text)
}
pub(super) fn compose(p: &LinuxPolicy<'_>) -> Result<LinuxPlan, Error> {
    validate(p)?;
    Ok(LinuxPlan {
        arguments: mounts(p)?,
        landlock: landlock(p)?,
    })
}

#[cfg(test)]
mod tests;
