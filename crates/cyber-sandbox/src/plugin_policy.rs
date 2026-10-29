//! Plugin-specific native policy composition. This is not execution admission:
//! callers own immutable runtime/helper/package provenance, complete private-root
//! inventory, namespace protection and generation-bound process lifetime.
use std::path::Path;

#[derive(Debug, Clone, Copy)]
pub enum ReadPath<'a> {
    File(&'a Path),
    Directory(&'a Path),
}
#[cfg(target_os = "macos")]
impl<'a> ReadPath<'a> {
    fn path(&self) -> &'a Path {
        match self {
            Self::File(path) | Self::Directory(path) => path,
        }
    }
}

/// Explicit owner-selected canonical bound paths; none are discovered from the
/// project or PATH. Validation is syntactic and cannot establish native bindings.
/// Network defaults to denied. Network/proxy/secret integration is separate work.
pub struct MacProfile<'a> {
    pub target: &'a Path,
    pub helper_identifier: &'a str,
    pub package: &'a Path,
    pub scratch: &'a Path,
    pub location: &'a Path,
    pub bootstrap: &'a [ReadPath<'a>],
    pub private: &'a [&'a Path],
    pub read_only: &'a [&'a Path],
    pub fs_read: bool,
    pub fs_write: bool,
    pub shell: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    #[error("native plugin policy composition is unavailable on this platform")]
    Unavailable,
    #[error("invalid native plugin policy input")]
    Input,
    #[error("native plugin policy roots overlap unsafely")]
    Overlap,
    #[error("native plugin policy exceeds its size limit")]
    Limit,
}

impl MacProfile<'_> {
    /// A complete deny-default filesystem/process profile with no network grant.
    /// The identifier must name a freshly signed admitted helper and differ from
    /// every architecture slice of the admitted target. This builder cannot attest
    /// those facts and does not open paths or execute verification tools.
    pub fn render(&self) -> Result<String, Error> {
        #[cfg(target_os = "macos")]
        return macos::render(self);
        #[cfg(not(target_os = "macos"))]
        Err(Error::Unavailable)
    }
}

#[cfg(target_os = "macos")]
mod macos {
    use super::*;
    use std::collections::BTreeSet;
    use std::fmt::Write;
    use std::path::Component;

    const MAX_PROFILE: usize = 64 * 1024;
    const MAX_ROOTS: usize = 64;
    const MAX_PATH_BYTES: usize = 64 * 1024;

    struct Text(String);
    impl Write for Text {
        fn write_str(&mut self, text: &str) -> std::fmt::Result {
            if self.0.len() + text.len() > MAX_PROFILE {
                return Err(std::fmt::Error);
            }
            self.0.push_str(text);
            Ok(())
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
    fn quote(text: &str) -> String {
        format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\""))
    }
    fn overlaps(a: &Path, b: &Path) -> bool {
        a.starts_with(b) || b.starts_with(a)
    }
    fn validate(profile: &MacProfile<'_>) -> Result<(), Error> {
        let identifier = profile.helper_identifier;
        if identifier.is_empty()
            || identifier.len() > 256
            || !identifier
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
        {
            return Err(Error::Input);
        }
        if profile
            .bootstrap
            .len()
            .saturating_add(profile.private.len())
            .saturating_add(profile.read_only.len())
            .saturating_add(4)
            > MAX_ROOTS
        {
            return Err(Error::Limit);
        }
        let mut paths = vec![
            profile.target,
            profile.package,
            profile.scratch,
            profile.location,
        ];
        paths.extend(profile.bootstrap.iter().map(ReadPath::path));
        paths.extend(profile.private.iter().copied());
        paths.extend(profile.read_only.iter().copied());
        if paths.len() > MAX_ROOTS {
            return Err(Error::Limit);
        }
        let mut bytes = 0;
        for value in paths {
            bytes += path(value)?.len();
            if bytes > MAX_PATH_BYTES {
                return Err(Error::Limit);
            }
        }
        let immutable = std::iter::once(profile.package)
            .chain(std::iter::once(profile.target))
            .chain(profile.bootstrap.iter().map(ReadPath::path));
        for root in immutable {
            if overlaps(root, profile.scratch)
                || profile
                    .private
                    .iter()
                    .any(|private| overlaps(root, private))
            {
                return Err(Error::Overlap);
            }
        }
        if profile
            .private
            .iter()
            .chain(profile.read_only)
            .any(|root| overlaps(root, profile.scratch))
        {
            return Err(Error::Overlap);
        }
        Ok(())
    }
    fn rule(
        out: &mut Text,
        action: &str,
        operation: &str,
        kind: &str,
        path: &Path,
    ) -> Result<(), Error> {
        writeln!(
            out,
            "({action} {operation} ({kind} {}))",
            quote(path.to_str().ok_or(Error::Input)?)
        )
        .map_err(|_| Error::Limit)
    }
    fn immutable<'a>(profile: &'a MacProfile<'a>) -> impl Iterator<Item = ReadPath<'a>> {
        [
            ReadPath::Directory(profile.package),
            ReadPath::File(profile.target),
        ]
        .into_iter()
        .chain(profile.bootstrap.iter().copied())
    }
    pub(super) fn render(profile: &MacProfile<'_>) -> Result<String, Error> {
        validate(profile)?;
        let mut out = Text(String::new());
        out.write_str("(version 1)\n(deny default)\n(allow file-read-metadata)\n(allow sysctl-read)\n(allow signal (target same-sandbox))\n(allow process-info* (target same-sandbox))\n(allow file-read-data (literal \"/\"))\n(allow file-read* file-write* (literal \"/dev/null\"))\n")
            .map_err(|_| Error::Limit)?;
        for root in immutable(profile).chain(std::iter::once(ReadPath::Directory(profile.scratch)))
        {
            let kind = if matches!(root, ReadPath::File(_)) {
                "literal"
            } else {
                "subpath"
            };
            rule(
                &mut out,
                "allow",
                "file-read* file-map-executable",
                kind,
                root.path(),
            )?;
        }
        rule(&mut out, "allow", "file-write*", "subpath", profile.scratch)?;
        if profile.fs_read {
            rule(&mut out, "allow", "file-read*", "subpath", profile.location)?;
        }
        if profile.fs_write {
            rule(
                &mut out,
                "allow",
                "file-write*",
                "subpath",
                profile.location,
            )?;
        }
        for root in immutable(profile) {
            let kind = if matches!(root, ReadPath::File(_)) {
                "literal"
            } else {
                "subpath"
            };
            rule(&mut out, "deny", "file-write*", kind, root.path())?;
        }
        for root in profile.read_only {
            rule(&mut out, "deny", "file-write*", "subpath", root)?;
        }
        for root in profile.private {
            rule(&mut out, "deny", "file-read* file-write*", "subpath", root)?;
        }
        let roots = immutable(profile)
            .map(|root| root.path())
            .chain(profile.private.iter().copied())
            .chain(profile.read_only.iter().copied());
        let mut ancestors = BTreeSet::new();
        for root in roots {
            for ancestor in root
                .ancestors()
                .skip(1)
                .filter(|path| path.parent().is_some())
            {
                ancestors.insert(ancestor);
            }
        }
        ancestors.insert(profile.location);
        for ancestor in ancestors {
            rule(&mut out, "deny", "file-write-unlink", "literal", ancestor)?;
        }
        if profile.shell {
            out.write_str("(allow process-fork)\n(allow process-exec)\n")
                .map_err(|_| Error::Limit)?;
        } else {
            writeln!(
                out,
                "(allow process-exec (require-all (signing-identifier {}) (literal {})))",
                quote(profile.helper_identifier),
                quote(profile.target.to_str().ok_or(Error::Input)?)
            )
            .map_err(|_| Error::Limit)?;
        }
        Ok(out.0)
    }
}

#[cfg(test)]
mod tests;
