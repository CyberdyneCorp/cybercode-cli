use super::*;
use cap_fs_ext::{DirExt, FollowSymlinks, OpenOptionsFollowExt};
use cap_std::fs::{Dir, OpenOptions};
use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, Instant};

/// One live source with a pinned descriptor. No page reopens a caller-supplied path.
pub struct Document {
    root: PathBuf,
    relative: PathBuf,
    requested: String,
    directories: Vec<same_file::Handle>,
    identity: same_file::Handle,
    observed: String,
    length: u64,
}

fn handle(dir: Dir) -> Result<same_file::Handle, Error> {
    same_file::Handle::from_file(dir.into_std_file()).map_err(|_| Error::Io)
}

fn open(root: &Path, relative: &Path) -> Result<(Vec<same_file::Handle>, File), Error> {
    let mut dir =
        Dir::open_ambient_dir(root, cap_std::ambient_authority()).map_err(|_| Error::File)?;
    let mut directories = vec![handle(dir.try_clone().map_err(|_| Error::Io)?)?];
    let mut parts = relative.components().peekable();
    while let Some(part) = parts.next() {
        let Component::Normal(name) = part else {
            return Err(Error::Invalid);
        };
        if parts.peek().is_some() {
            dir = dir.open_dir_nofollow(name).map_err(|_| Error::File)?;
            directories.push(handle(dir.try_clone().map_err(|_| Error::Io)?)?);
        } else {
            let metadata = dir.symlink_metadata(name).map_err(|_| Error::File)?;
            if !metadata.is_file() || metadata.file_type().is_symlink() {
                return Err(Error::File);
            }
            let mut options = OpenOptions::new();
            options.read(true).follow(FollowSymlinks::No);
            #[cfg(unix)]
            {
                use cap_std::fs::OpenOptionsExt;
                options.custom_flags(libc::O_NONBLOCK);
            }
            let file = dir
                .open_with(name, &options)
                .map_err(|_| Error::File)?
                .into_std();
            if !file.metadata().map_err(|_| Error::Io)?.is_file() {
                return Err(Error::File);
            }
            return Ok((directories, file));
        }
    }
    Err(Error::Invalid)
}

fn generation(file: &File) -> Result<(String, u64), Error> {
    let m = file.metadata().map_err(|_| Error::Io)?;
    let mut hash = Sha256::new();
    hash.update(m.len().to_le_bytes());
    hash.update(format!("{:?}", m.modified().map_err(|_| Error::Io)?));
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        hash.update(m.ctime().to_le_bytes());
        hash.update(m.ctime_nsec().to_le_bytes());
    }
    Ok((format!("{:x}", hash.finalize()), m.len()))
}

impl Document {
    /// `root` must be the caller's canonical authorized project root. Resolve aliases
    /// inside that root, then pin every canonical binding without following links.
    /// Call `validate` after authorization; authorization must use `path()`.
    pub fn open(root: &Path, relative: &str) -> Result<Self, Error> {
        if !root.is_absolute() || relative.len() > 4096 {
            return Err(Error::Invalid);
        }
        let requested = relative.to_owned();
        let canonical = crate::files::resolve(root, relative).map_err(|_| Error::File)?;
        let relative = canonical
            .strip_prefix(root)
            .map_err(|_| Error::File)?
            .to_path_buf();
        let (directories, file) = open(root, &relative)?;
        let (observed, length) = generation(&file)?;
        let doc = Self {
            root: root.into(),
            relative,
            requested,
            directories,
            identity: same_file::Handle::from_file(file).map_err(|_| Error::Io)?,
            observed,
            length,
        };
        doc.validate()?;
        Ok(doc)
    }

    pub fn path(&self) -> PathBuf {
        self.root.join(&self.relative)
    }

    pub fn metadata(&self) -> Metadata {
        Metadata {
            path: crate::vcs::slash_path(&self.relative),
            length: Offset(self.length),
            revision: self.observed.clone(),
            source: Source::Live,
            within_edit_size_limit: self.length <= MAX_EDIT_SIZE,
        }
    }

    /// Reopen only for identity comparison, never to serve bytes from a replacement.
    pub fn validate(&self) -> Result<(), Error> {
        let canonical =
            crate::files::resolve(&self.root, &self.requested).map_err(|_| Error::Changed)?;
        if canonical != self.path() {
            return Err(Error::Changed);
        }
        let (dirs, file) = open(&self.root, &self.relative).map_err(|_| Error::Changed)?;
        let identity = same_file::Handle::from_file(file).map_err(|_| Error::Io)?;
        if dirs != self.directories
            || identity != self.identity
            || generation(self.identity.as_file())? != (self.observed.clone(), self.length)
        {
            return Err(Error::Changed);
        }
        Ok(())
    }

    fn check_revision(&self, revision: &str) -> Result<(), Error> {
        if revision != self.observed {
            return Err(Error::Changed);
        }
        self.validate()
    }

    fn read_at(&self, offset: u64, limit: usize) -> Result<Vec<u8>, Error> {
        // A borrow of the same pinned descriptor, not a path reopen or shared seek
        // position with another document. The document owner serializes operations.
        let mut file = self.identity.as_file();
        file.seek(SeekFrom::Start(offset)).map_err(|_| Error::Io)?;
        let mut bytes = Vec::with_capacity(limit);
        file.take(limit as u64)
            .read_to_end(&mut bytes)
            .map_err(|_| Error::Io)?;
        Ok(bytes)
    }

    pub fn page(&mut self, revision: &str, offset: Offset, limit: usize) -> Result<Page, Error> {
        if limit == 0 || limit > MAX_PAGE || offset.0 > self.length {
            return Err(Error::Limit);
        }
        self.check_revision(revision)?;
        let expected = limit.min(usize::try_from(self.length - offset.0).unwrap_or(usize::MAX));
        let bytes = self.read_at(offset.0, expected)?;
        self.validate()?;
        if bytes.len() != expected {
            return Err(Error::Changed);
        }
        let end = offset
            .0
            .checked_add(bytes.len() as u64)
            .ok_or(Error::Invalid)?;
        Ok(Page {
            sha256: format!("{:x}", Sha256::digest(&bytes)),
            offset,
            bytes,
            eof: end == self.length,
        })
    }

    /// Live/best-effort scanning. A continuation restarts the final pattern overlap,
    /// so matches crossing an IO chunk or request boundary are neither lost nor repeated.
    pub fn search(
        &mut self,
        request: &SearchRequest,
        cancelled: impl Fn() -> bool,
    ) -> Result<SearchResult, Error> {
        let pattern = decode_hex(&request.hex, MAX_PATTERN)?;
        if pattern.is_empty() || request.offset.0 > self.length {
            return Err(Error::Invalid);
        }
        self.check_revision(&request.revision)?;
        let matcher = aho_corasick::AhoCorasick::new([&pattern]).map_err(|_| Error::Invalid)?;
        let deadline = Instant::now() + Duration::from_millis(250);
        let mut at = request.offset.0;
        let mut carry = Vec::new();
        let mut offsets = Vec::new();
        let mut scanned = 0usize;
        let mut next = None;
        while at < self.length && scanned < MAX_SEARCH_BYTES {
            if cancelled() {
                return Err(Error::Cancelled);
            }
            let remaining = usize::try_from(self.length - at).unwrap_or(usize::MAX);
            let chunk = self.read_at(
                at,
                DEFAULT_PAGE.min(MAX_SEARCH_BYTES - scanned).min(remaining),
            )?;
            if chunk.is_empty() {
                return Err(Error::Changed);
            }
            let start = at - carry.len() as u64;
            scanned += chunk.len();
            at += chunk.len() as u64;
            carry.extend_from_slice(&chunk);
            for found in matcher.find_overlapping_iter(&carry) {
                offsets.push(Offset(start + found.start() as u64));
                if offsets.len() == MAX_MATCHES {
                    next = Some(Offset(start + found.start() as u64 + 1));
                    break;
                }
            }
            if next.is_some() {
                break;
            }
            let keep = (pattern.len() - 1).min(carry.len());
            carry.drain(..carry.len() - keep);
            if Instant::now() >= deadline {
                break;
            }
        }
        self.validate()?;
        if next.is_none() && at < self.length {
            next = Some(Offset(at.saturating_sub(carry.len() as u64)));
        }
        Ok(SearchResult {
            revision: self.observed.clone(),
            source: Source::Live,
            offsets,
            next_offset: next,
            scanned: Offset(scanned as u64),
        })
    }
}
