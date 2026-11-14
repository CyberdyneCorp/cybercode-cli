//! Exact readonly working-tree images; never native mutation/acknowledgement authority.
use super::{links, ownership, storage};
use cap_std::fs::Dir;
use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::io::Read;
use std::os::unix::fs::MetadataExt;
use std::path::Path;

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Capture {
    pub policy: String,
    pub files: BTreeMap<String, RawImage>,
}
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct RawImage {
    pub mode: &'static str,
    pub bytes: Vec<u8>,
}
pub(super) fn read(dir: &Dir, name: &OsStr) -> storage::Result<RawImage> {
    const LIMIT: u64 = 64 * 1024 * 1024;
    if dir
        .symlink_metadata(name)
        .map_err(storage::fail)?
        .file_type()
        .is_symlink()
    {
        return links::read(dir, name, LIMIT).map(|(_, bytes)| RawImage {
            mode: "120000",
            bytes,
        });
    }
    let mut file = storage::open(dir, Path::new(name))?;
    cyber_core::binary::native::plain_snapshot_metadata(&file)
        .map_err(|_| "Unsupported Git raw capture metadata")?;
    let before = ownership::generation(&file)?;
    let metadata = file.metadata().map_err(storage::fail)?;
    if metadata.len() > LIMIT {
        return Err("Git raw capture file exceeds limit".into());
    }
    let mut bytes = Vec::new();
    Read::by_ref(&mut file)
        .take(LIMIT + 1)
        .read_to_end(&mut bytes)
        .map_err(storage::fail)?;
    let current = storage::open(dir, Path::new(name))?;
    if bytes.len() as u64 != metadata.len()
        || ownership::generation(&file)? != before
        || ownership::generation(&current)? != before
        || storage::identity(&current)? != storage::identity(&file)?
    {
        return Err("Git raw capture source changed while reading".into());
    }
    Ok(RawImage {
        mode: if metadata.mode() & 0o111 == 0 {
            "100644"
        } else {
            "100755"
        },
        bytes,
    })
}
