use super::storage::{self, Result, fail};
use super::{ENTRY_LIMIT, Entry, Store, Tree};
use cap_fs_ext::DirExt;
use cap_std::fs::Dir;
use ignore::gitignore::{Gitignore, GitignoreBuilder};
use std::collections::BTreeMap;
use std::io::Read;
use std::os::unix::fs::MetadataExt;
use std::path::{Component, Path};

pub(super) fn safe_path(path: &str) -> Result<()> {
    if path.is_empty()
        || path.len() > 4096
        || path.contains('\0')
        || path
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
        || Path::new(path)
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
    {
        return Err("Invalid snapshot path".into());
    }
    Ok(())
}
pub(super) fn protected(path: &Path) -> bool {
    path.components()
        .any(|c| matches!(c,Component::Normal(n) if n==".git" || n==".cyber" || n==".claude"))
}
impl Store {
    pub(super) fn capture(&self) -> Result<Tree> {
        self.verify_root()?;
        let mut tree = Tree {
            version: 1,
            location: self.root.clone(),
            root: self.root_id.clone(),
            files: BTreeMap::new(),
            skipped: vec![],
        };
        let mut matcher = GitignoreBuilder::new(&self.root);
        for line in &self.options.ignore {
            matcher.add_line(None, line).map_err(fail)?;
        }
        let mut matchers = vec![matcher.build().map_err(fail)?];
        self.walk(&self.work, Path::new(""), &mut matchers, &mut tree, 0)?;
        self.verify_root()?;
        tree.skipped.sort();
        Ok(tree)
    }
    fn rules(&self, dir: &Dir, relative: &Path) -> Result<Gitignore> {
        let mut rules = GitignoreBuilder::new(&self.root);
        for name in [".gitignore", ".cyberignore"] {
            if dir.symlink_metadata(name).is_err() {
                continue;
            }
            let bytes = storage::read(dir, name, 64 * 1024)?;
            let text = std::str::from_utf8(&bytes).map_err(fail)?;
            for line in text.lines() {
                rules
                    .add_line(Some(self.root.join(relative).join(name)), line)
                    .map_err(fail)?;
            }
        }
        rules.build().map_err(fail)
    }
    fn walk(
        &self,
        dir: &Dir,
        relative: &Path,
        matchers: &mut Vec<Gitignore>,
        tree: &mut Tree,
        depth: usize,
    ) -> Result<()> {
        if depth > 64 {
            return Err("Snapshot directory depth limit exceeded".into());
        }
        matchers.push(self.rules(dir, relative)?);
        for item in dir.entries().map_err(fail)? {
            let item = item.map_err(fail)?;
            let path = relative.join(item.file_name());
            let name = path
                .to_str()
                .ok_or("Non-UTF-8 snapshot path is unsupported")?
                .to_owned();
            safe_path(&name)?;
            if tree.files.len() + tree.skipped.len() >= ENTRY_LIMIT {
                return Err("Snapshot path limit exceeded".into());
            }
            let metadata = dir.symlink_metadata(item.file_name()).map_err(fail)?;
            let excluded = self.private_data_path(&self.root.join(&path))
                || protected(&path)
                || self
                    .options
                    .private_roots
                    .iter()
                    .any(|p| self.root.join(&path).starts_with(p));
            let configured_ignore = matchers
                .first()
                .is_some_and(|r| r.matched(&path, metadata.is_dir()).is_ignore());
            let ignored = configured_ignore
                || matchers
                    .iter()
                    .skip(1)
                    .rev()
                    .map(|r| r.matched(&path, metadata.is_dir()))
                    .find(|m| !m.is_none())
                    .is_some_and(|m| m.is_ignore());
            if excluded || ignored {
                tree.skipped.push(format!(
                    "{}: {name}",
                    if excluded { "private" } else { "ignore" }
                ));
            } else if metadata.is_dir() {
                let child = dir.open_dir_nofollow(item.file_name()).map_err(fail)?;
                self.walk(&child, &path, matchers, tree, depth + 1)?;
            } else if !metadata.is_file()
                || metadata.file_type().is_symlink()
                || cap_fs_ext::MetadataExt::nlink(&metadata) != 1
            {
                tree.skipped.push(format!("unsupported: {name}"));
            } else if metadata.len() > self.options.max_file {
                tree.skipped.push(format!("size: {name}"));
            } else {
                match self.read_entry(dir, &item.file_name()) {
                    Ok(entry) => {
                        tree.files.insert(name, entry);
                    }
                    Err(error) if error == "Unsupported snapshot metadata" => {
                        tree.skipped.push(format!("metadata: {name}"))
                    }
                    Err(error) => return Err(error),
                }
            }
        }
        matchers.pop();
        Ok(())
    }
    pub(super) fn read_entry(&self, dir: &Dir, name: &std::ffi::OsStr) -> Result<Entry> {
        self.entry(dir, name, true)
    }
    pub(super) fn inspect_entry(&self, dir: &Dir, name: &std::ffi::OsStr) -> Result<Entry> {
        self.entry(dir, name, false)
    }
    fn entry(&self, dir: &Dir, name: &std::ffi::OsStr, save: bool) -> Result<Entry> {
        if dir
            .symlink_metadata(name)
            .map_err(fail)?
            .file_type()
            .is_symlink()
        {
            return self.link_entry(dir, name, save);
        }
        let mut file = storage::open(dir, Path::new(name))?;
        let before = file.metadata().map_err(fail)?;
        let metadata = cyber_core::binary::native::plain_snapshot_metadata(&file)
            .map_err(|_| "Unsupported snapshot metadata")?;
        if before.len() > self.options.max_file {
            return Err("Snapshot file exceeds configured limit".into());
        }
        let mut bytes = vec![];
        Read::by_ref(&mut file)
            .take(self.options.max_file.saturating_add(1))
            .read_to_end(&mut bytes)
            .map_err(fail)?;
        let after = file.metadata().map_err(fail)?;
        let current = dir.symlink_metadata(name).map_err(fail)?;
        if before.len() != bytes.len() as u64
            || before.len() != after.len()
            || before.mtime() != after.mtime()
            || before.mtime_nsec() != after.mtime_nsec()
            || before.ctime() != after.ctime()
            || before.ctime_nsec() != after.ctime_nsec()
            || before.dev() != cap_fs_ext::MetadataExt::dev(&current)
            || before.ino() != cap_fs_ext::MetadataExt::ino(&current)
            || current.file_type().is_symlink()
            || cap_fs_ext::MetadataExt::nlink(&current) != 1
        {
            return Err("Snapshot source changed while hashing".into());
        }
        let mut entry = if save {
            self.object(&bytes)?
        } else {
            Entry {
                kind: super::ImageKind::File,
                object: storage::digest(&bytes),
                length: bytes.len() as u64,
                mode: 0,
                uid: 0,
                gid: 0,
            }
        };
        entry.mode = metadata.mode;
        entry.uid = metadata.uid;
        entry.gid = metadata.gid;
        Ok(entry)
    }
    pub(super) fn parent(&self, path: &str) -> Result<Dir> {
        self.check_source_path(path)?;
        let parent = Path::new(path).parent().ok_or("Invalid snapshot parent")?;
        self.verify_root()?;
        let mut dir = self.work.try_clone().map_err(fail)?;
        for component in parent.components() {
            let Component::Normal(name) = component else {
                return Err("Invalid snapshot parent".into());
            };
            dir = dir.open_dir_nofollow(name).map_err(fail)?;
        }
        Ok(dir)
    }
    pub(super) fn check_source_path(&self, path: &str) -> Result<()> {
        safe_path(path)?;
        if protected(Path::new(path))
            || self.private_data_path(&self.root.join(path))
            || self
                .options
                .private_roots
                .iter()
                .any(|p| self.root.join(path).starts_with(p))
        {
            return Err("Protected snapshot path".into());
        }
        Ok(())
    }
    pub(super) fn current(&self, path: &str) -> Result<Option<Entry>> {
        let parent = match self.parent(path) {
            Ok(parent) => parent,
            Err(error) => {
                // Only a genuinely absent ancestor is absence; aliases and errors refuse.
                let mut cursor = self.work.try_clone().map_err(fail)?;
                for component in Path::new(path)
                    .parent()
                    .ok_or("Invalid snapshot path")?
                    .components()
                {
                    let Component::Normal(name) = component else {
                        return Err(error);
                    };
                    match cursor.symlink_metadata(name) {
                        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
                        _ => cursor = cursor.open_dir_nofollow(name).map_err(fail)?,
                    }
                }
                return Err(error);
            }
        };
        let leaf = Path::new(path).file_name().ok_or("Invalid snapshot leaf")?;
        match parent.symlink_metadata(leaf) {
            Ok(_) => self.read_entry(&parent, leaf).map(Some),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(fail(e)),
        }
    }
}
