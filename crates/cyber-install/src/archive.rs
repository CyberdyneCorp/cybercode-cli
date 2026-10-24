//! Release archive extraction (`installation-upgrade` → Self-update integrity): only
//! `cyber` and `cyber-sandbox-exec` at the archive root are written, each to a hidden
//! temporary file beside the binary it replaces. An archive with an absolute or `..`
//! member, a link, a device or a duplicate binary is refused whole.

use std::io::{Read, Write};
use std::path::{Component, Path};

/// The binaries a release archive carries; every other regular member is skipped.
pub const CYBER: &str = "cyber";
pub const HELPER: &str = "cyber-sandbox-exec";

/// One extracted binary is never larger than a whole archive.
const MAX_MEMBER_BYTES: u64 = crate::fetch::MAX_ARCHIVE_BYTES;

#[derive(Debug, thiserror::Error)]
pub enum ArchiveError {
    #[error("invalid release archive: {0}")]
    Invalid(String),
    #[error("release archive: {0}")]
    Io(#[from] std::io::Error),
}

fn invalid(reason: impl Into<String>) -> ArchiveError {
    ArchiveError::Invalid(reason.into())
}

/// The extracted binaries; each temporary file is deleted when it drops unextracted.
#[derive(Debug)]
pub struct Extracted {
    pub cyber: tempfile::TempPath,
    pub helper: Option<tempfile::TempPath>,
}

/// Which binaries to look for: `cyber.exe` and `cyber-sandbox-exec.exe` for Windows
/// targets.
#[derive(Debug, Clone, Copy)]
pub struct Members {
    pub exe_suffix: &'static str,
}

impl Members {
    pub fn for_target(target: &str) -> Self {
        let exe_suffix = if target.contains("windows") {
            ".exe"
        } else {
            ""
        };
        Self { exe_suffix }
    }

    /// `CYBER` or `HELPER` when `name` is one of them (with the suffix).
    fn wanted(self, name: &str) -> Option<&'static str> {
        let stem = name.strip_suffix(self.exe_suffix)?;
        [CYBER, HELPER].into_iter().find(|wanted| *wanted == stem)
    }
}

/// The member name with `.` components dropped, or an error for an absolute path, a
/// drive prefix or `..`.
fn member_path(raw: &Path) -> Result<String, ArchiveError> {
    let mut parts = Vec::new();
    for component in raw.components() {
        match component {
            Component::Normal(part) => parts.push(part.to_string_lossy().into_owned()),
            Component::CurDir => {}
            _ => return Err(invalid(format!("unsafe member path {}", raw.display()))),
        }
    }
    Ok(parts.join("/"))
}

/// The kind of one archive member, as far as extraction cares.
enum Kind {
    File,
    Dir,
}

struct Sink<'a> {
    dir: &'a Path,
    members: Members,
    cyber: Option<tempfile::TempPath>,
    helper: Option<tempfile::TempPath>,
}

impl Sink<'_> {
    /// Write `reader` when `name` is a wanted binary at the archive root.
    fn member(
        &mut self,
        name: &str,
        kind: Kind,
        reader: &mut dyn Read,
    ) -> Result<(), ArchiveError> {
        let Kind::File = kind else {
            return Ok(());
        };
        let Some(wanted) = (!name.contains('/'))
            .then(|| self.members.wanted(name))
            .flatten()
        else {
            return Ok(());
        };
        let slot = if wanted == CYBER {
            &mut self.cyber
        } else {
            &mut self.helper
        };
        if slot.is_some() {
            return Err(invalid(format!("duplicate member {name}")));
        }
        *slot = Some(write_member(self.dir, reader)?);
        Ok(())
    }

    fn finish(self) -> Result<Extracted, ArchiveError> {
        let cyber = self
            .cyber
            .ok_or_else(|| invalid(format!("no {CYBER}{} member", self.members.exe_suffix)))?;
        Ok(Extracted {
            cyber,
            helper: self.helper,
        })
    }
}

fn write_member(dir: &Path, reader: &mut dyn Read) -> Result<tempfile::TempPath, ArchiveError> {
    let mut file = tempfile::Builder::new()
        .prefix(".cyber-upgrade-")
        .suffix(".new")
        .tempfile_in(dir)?;
    let copied = std::io::copy(&mut reader.take(MAX_MEMBER_BYTES + 1), file.as_file_mut())?;
    if copied > MAX_MEMBER_BYTES {
        return Err(invalid("member larger than the archive limit"));
    }
    file.as_file_mut().flush()?;
    file.as_file().sync_all()?;
    Ok(file.into_temp_path())
}

/// Extract the binaries of `archive` into `dir`; its release `name` gives the format
/// (`.zip`, else gzip-compressed tar).
pub fn extract(
    archive: &Path,
    name: &str,
    members: Members,
    dir: &Path,
) -> Result<Extracted, ArchiveError> {
    let mut sink = Sink {
        dir,
        members,
        cyber: None,
        helper: None,
    };
    if name.ends_with(".zip") {
        extract_zip(archive, &mut sink)?;
    } else {
        extract_tar_gz(archive, &mut sink)?;
    }
    sink.finish()
}

fn extract_tar_gz(archive: &Path, sink: &mut Sink<'_>) -> Result<(), ArchiveError> {
    let file = std::fs::File::open(archive)?;
    let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(file));
    for entry in tar.entries()? {
        let mut entry = entry?;
        let path = entry.path()?.into_owned();
        let name = member_path(&path)?;
        let kind = match entry.header().entry_type() {
            tar::EntryType::Regular | tar::EntryType::Continuous => Kind::File,
            tar::EntryType::Directory => Kind::Dir,
            other => return Err(invalid(format!("{other:?} member {name} refused"))),
        };
        sink.member(&name, kind, &mut entry)?;
    }
    Ok(())
}

fn extract_zip(archive: &Path, sink: &mut Sink<'_>) -> Result<(), ArchiveError> {
    let file = std::fs::File::open(archive)?;
    let mut zip = zip::ZipArchive::new(file).map_err(|e| invalid(e.to_string()))?;
    for index in 0..zip.len() {
        let mut entry = zip.by_index(index).map_err(|e| invalid(e.to_string()))?;
        // Zip names use `/`; a `\` is a Windows separator in disguise.
        let raw = entry.name().replace('\\', "/");
        let name = member_path(Path::new(&raw))?;
        if raw.starts_with('/') || raw.contains(':') {
            return Err(invalid(format!("unsafe member path {raw}")));
        }
        if entry.is_symlink() {
            return Err(invalid(format!("link member {name} refused")));
        }
        let kind = if entry.is_dir() {
            Kind::Dir
        } else {
            Kind::File
        };
        sink.member(&name, kind, &mut entry)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const UNIX: Members = Members { exe_suffix: "" };

    fn header(kind: tar::EntryType, size: u64) -> tar::Header {
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(kind);
        header.set_size(size);
        header.set_mode(0o755);
        header
    }

    /// A tar.gz of `(name, kind, body)` members, names written raw so `..` survives.
    fn tar_gz(dir: &Path, members: &[(&str, tar::EntryType, &[u8])]) -> std::path::PathBuf {
        let path = dir.join("release.tar.gz");
        let gz = flate2::write::GzEncoder::new(
            std::fs::File::create(&path).unwrap(),
            flate2::Compression::fast(),
        );
        let mut builder = tar::Builder::new(gz);
        for (name, kind, body) in members {
            let mut header = header(*kind, body.len() as u64);
            if *kind == tar::EntryType::Symlink {
                header.set_link_name("/etc/passwd").unwrap();
            }
            let raw = &mut header.as_old_mut().name;
            raw[..name.len()].copy_from_slice(name.as_bytes());
            header.set_cksum();
            builder.append(&header, *body).unwrap();
        }
        builder.into_inner().unwrap().finish().unwrap();
        path
    }

    fn zip_file(dir: &Path, members: &[(&str, &[u8])]) -> std::path::PathBuf {
        let path = dir.join("release.zip");
        let mut zip = zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
        for (name, body) in members {
            zip.start_file(*name, zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(body).unwrap();
        }
        zip.finish().unwrap();
        path
    }

    fn leftovers(dir: &Path) -> Vec<String> {
        std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with(".cyber-upgrade-"))
            .collect()
    }

    #[test]
    fn extracts_only_the_binaries() {
        let dir = tempfile::tempdir().unwrap();
        let regular = tar::EntryType::Regular;
        let archive = tar_gz(
            dir.path(),
            &[
                ("./", tar::EntryType::Directory, b""),
                ("./cyber", regular, b"new cyber"),
                ("cyber-sandbox-exec", regular, b"new helper"),
                ("LICENSE", regular, b"license"),
                ("docs/cyber", regular, b"not the binary"),
            ],
        );
        let out = extract(&archive, "release.tar.gz", UNIX, dir.path()).unwrap();
        assert_eq!(std::fs::read(&out.cyber).unwrap(), b"new cyber");
        assert_eq!(
            std::fs::read(out.helper.as_ref().unwrap()).unwrap(),
            b"new helper"
        );
        assert!(!dir.path().join("LICENSE").exists());
        drop(out);
        assert!(leftovers(dir.path()).is_empty());
    }

    #[test]
    fn traversal_links_and_duplicates_are_refused() {
        let regular = tar::EntryType::Regular;
        let cases: &[&[(&str, tar::EntryType, &[u8])]] = &[
            &[("../cyber", regular, b"x")],
            &[("/cyber", regular, b"x")],
            &[("bin/../../cyber", regular, b"x")],
            &[("cyber", tar::EntryType::Symlink, b"")],
            &[("cyber", tar::EntryType::Link, b"")],
            &[
                ("other", tar::EntryType::Symlink, b""),
                ("cyber", regular, b"x"),
            ],
            &[("cyber", tar::EntryType::Char, b"")],
            &[("cyber", regular, b"x"), ("./cyber", regular, b"y")],
        ];
        for members in cases {
            let dir = tempfile::tempdir().unwrap();
            let archive = tar_gz(dir.path(), members);
            let err = extract(&archive, "release.tar.gz", UNIX, dir.path()).unwrap_err();
            assert!(
                matches!(err, ArchiveError::Invalid(_)),
                "{members:?}: {err}"
            );
            let names: Vec<_> = std::fs::read_dir(dir.path())
                .unwrap()
                .map(|e| e.unwrap().file_name())
                .collect();
            assert_eq!(names, ["release.tar.gz"], "{members:?}");
        }
    }

    #[test]
    fn an_archive_without_cyber_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let archive = tar_gz(dir.path(), &[("README.md", tar::EntryType::Regular, b"x")]);
        let err = extract(&archive, "release.tar.gz", UNIX, dir.path()).unwrap_err();
        assert_eq!(err.to_string(), "invalid release archive: no cyber member");
    }

    #[test]
    fn a_truncated_archive_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let archive = tar_gz(
            dir.path(),
            &[("cyber", tar::EntryType::Regular, &[7; 4096])],
        );
        let bytes = std::fs::read(&archive).unwrap();
        std::fs::write(&archive, &bytes[..bytes.len() / 2]).unwrap();
        assert!(extract(&archive, "release.tar.gz", UNIX, dir.path()).is_err());
        assert!(leftovers(dir.path()).is_empty());
    }

    #[test]
    fn zip_archives_for_windows_targets() {
        let dir = tempfile::tempdir().unwrap();
        let members = Members::for_target("x86_64-pc-windows-msvc");
        let archive = zip_file(
            dir.path(),
            &[
                ("cyber.exe", b"exe"),
                ("cyber-sandbox-exec.exe", b"helper"),
                ("cyber", b"no"),
            ],
        );
        let out = extract(&archive, "release.zip", members, dir.path()).unwrap();
        assert_eq!(std::fs::read(&out.cyber).unwrap(), b"exe");
        assert_eq!(
            std::fs::read(out.helper.as_ref().unwrap()).unwrap(),
            b"helper"
        );

        for name in [
            "../cyber.exe",
            "..\\cyber.exe",
            "C:/cyber.exe",
            "/cyber.exe",
        ] {
            let dir = tempfile::tempdir().unwrap();
            let archive = zip_file(dir.path(), &[(name, b"x")]);
            let err = extract(&archive, "release.zip", members, dir.path()).unwrap_err();
            assert!(matches!(err, ArchiveError::Invalid(_)), "{name}: {err}");
        }
    }

    #[test]
    fn zip_symlinks_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("release.zip");
        let mut zip = zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
        zip.add_symlink(
            "cyber.exe",
            "C:/Windows/notepad.exe",
            zip::write::SimpleFileOptions::default(),
        )
        .unwrap();
        zip.finish().unwrap();
        let members = Members::for_target("x86_64-pc-windows-msvc");
        let err = extract(&path, "release.zip", members, dir.path()).unwrap_err();
        assert!(err.to_string().contains("link member"), "{err}");
    }
}
