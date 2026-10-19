//! npm plugin sources (`plugins-marketplace` → Plugin sources). `npm pack
//! --ignore-scripts` fetches or builds the package tarball without running any lifecycle
//! script and without installing dependencies; the tarball is unpacked here, refusing
//! links and paths outside `package/`.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;

const MAX_BYTES: u64 = 64 * 1024 * 1024;
const MAX_ENTRIES: usize = 4096;

/// Pack `package` into `staging` and unpack it; returns the package directory.
pub(super) fn fetch(staging: &Path, package: &str) -> Result<PathBuf, String> {
    std::fs::create_dir_all(staging).map_err(|e| format!("{}: {e}", staging.display()))?;
    let output = Command::new(if cfg!(windows) { "npm.cmd" } else { "npm" })
        .args(["pack", "--ignore-scripts", "--json", "--pack-destination"])
        .arg(super::install::plain(staging))
        .arg("--")
        .arg(package)
        .current_dir(staging)
        .env("npm_config_ignore_scripts", "true")
        .output()
        .map_err(|e| format!("cannot run npm: {e}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("npm pack {package}: {}", stderr.trim()));
    }
    let packed: serde_json::Value = serde_json::from_slice(&output.stdout)
        .map_err(|e| format!("npm pack {package}: unreadable output: {e}"))?;
    let file = packed[0]["filename"]
        .as_str()
        .filter(|name| !name.contains(['/', '\\']) && name.ends_with(".tgz"))
        .ok_or_else(|| format!("npm pack {package}: no tarball reported"))?;
    let tarball = staging.join(file);
    let target = staging.join("unpacked");
    unpack(&tarball, &target)?;
    Ok(target)
}

/// Unpack a gzip'd tar into `target`, keeping only regular files under `package/`.
pub(super) fn unpack(tarball: &Path, target: &Path) -> Result<(), String> {
    let file = std::fs::File::open(tarball).map_err(|e| format!("{}: {e}", tarball.display()))?;
    let mut bytes = Vec::new();
    flate2::read::GzDecoder::new(file)
        .take(MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("{}: {e}", tarball.display()))?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err("package exceeds 64 MiB".into());
    }
    let mut archive = Archive {
        bytes: &bytes,
        offset: 0,
    };
    let mut long_name = None;
    let mut entries = 0;
    while let Some(entry) = archive.next()? {
        entries += 1;
        if entries > MAX_ENTRIES {
            return Err("package has more than 4096 entries".into());
        }
        match entry.kind {
            b'x' => long_name = pax_path(entry.data).or(long_name),
            b'L' => long_name = Some(c_string(entry.data)),
            b'g' | b'5' => {}
            b'0' | 0 => {
                let name = long_name.take().unwrap_or(entry.name);
                write(target, &name, entry.data)?;
            }
            _ => {
                return Err(format!(
                    "{}: links and special files are not allowed",
                    entry.name
                ));
            }
        }
    }
    Ok(())
}

fn write(target: &Path, name: &str, data: &[u8]) -> Result<(), String> {
    let relative = name
        .strip_prefix("package/")
        .and_then(super::manifest::inside)
        .ok_or_else(|| format!("{name}: outside the package directory"))?;
    let path = target.join(relative);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    std::fs::write(&path, data).map_err(|e| format!("{}: {e}", path.display()))
}

struct Entry<'a> {
    name: String,
    kind: u8,
    data: &'a [u8],
}

struct Archive<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> Archive<'a> {
    fn next(&mut self) -> Result<Option<Entry<'a>>, String> {
        let Some(header) = self.bytes.get(self.offset..self.offset + 512) else {
            return Ok(None);
        };
        if header.iter().all(|byte| *byte == 0) {
            return Ok(None);
        }
        let size = octal(&header[124..136]).ok_or("tar: invalid entry size")?;
        let start = self.offset + 512;
        let end = start
            .checked_add(size)
            .filter(|end| *end <= self.bytes.len())
            .ok_or("tar: truncated entry")?;
        self.offset = start + size.div_ceil(512) * 512;
        let mut name = c_string(&header[0..100]);
        if &header[257..262] == b"ustar" {
            let prefix = c_string(&header[345..500]);
            if !prefix.is_empty() {
                name = format!("{prefix}/{name}");
            }
        }
        Ok(Some(Entry {
            name,
            kind: header[156],
            data: &self.bytes[start..end],
        }))
    }
}

fn c_string(bytes: &[u8]) -> String {
    let end = bytes.iter().position(|b| *b == 0).unwrap_or(bytes.len());
    String::from_utf8_lossy(&bytes[..end]).into_owned()
}

fn octal(field: &[u8]) -> Option<usize> {
    let text = c_string(field);
    let text = text.trim();
    if text.is_empty() {
        return Some(0);
    }
    usize::from_str_radix(text, 8).ok()
}

/// The `path` record of a PAX extended header (`<length> path=<value>\n`).
fn pax_path(data: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(data);
    text.lines()
        .filter_map(|line| line.split_once(' ').map(|(_, record)| record))
        .find_map(|record| record.strip_prefix("path=").map(str::to_owned))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn header(name: &str, size: usize, kind: u8) -> Vec<u8> {
        let mut header = vec![0u8; 512];
        header[..name.len()].copy_from_slice(name.as_bytes());
        let size = format!("{size:011o}\0");
        header[124..136].copy_from_slice(size.as_bytes());
        header[156] = kind;
        header[257..262].copy_from_slice(b"ustar");
        header
    }

    fn tarball(dir: &Path, entries: &[(&str, &[u8], u8)]) -> PathBuf {
        let mut tar = Vec::new();
        for (name, data, kind) in entries {
            tar.extend(header(name, data.len(), *kind));
            tar.extend_from_slice(data);
            tar.resize(tar.len().div_ceil(512) * 512, 0);
        }
        tar.extend([0u8; 1024]);
        let path = dir.join("p.tgz");
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(&tar).unwrap();
        std::fs::write(&path, encoder.finish().unwrap()).unwrap();
        path
    }

    #[test]
    fn package_files_unpack_and_links_or_escapes_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let pax = b"27 path=package/long/name.md\n";
        let path = tarball(
            dir.path(),
            &[
                ("package/cyber-plugin.json", b"{}", b'0'),
                ("pax", pax, b'x'),
                ("short", b"long", b'0'),
            ],
        );
        let target = dir.path().join("out");
        unpack(&path, &target).unwrap();
        assert_eq!(
            std::fs::read(target.join("cyber-plugin.json")).unwrap(),
            b"{}"
        );
        assert_eq!(std::fs::read(target.join("long/name.md")).unwrap(), b"long");

        for (name, kind, message) in [
            ("package/../escape", b'0', "outside"),
            ("other/file", b'0', "outside"),
            ("package/link", b'2', "links"),
        ] {
            let path = tarball(dir.path(), &[(name, b"x", kind)]);
            let error = unpack(&path, &dir.path().join("bad")).unwrap_err();
            assert!(error.contains(message), "{name}: {error}");
        }
    }
}
