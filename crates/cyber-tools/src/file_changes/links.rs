//! A multiply-linked file is observable only when every reported link is
//! accounted for among permitted entries in this bounded scan. This allows
//! ordinary in-Location aliases without exposing an alias of private/outside state.
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use cap_std::fs::Metadata;

pub(super) fn identity(metadata: &Metadata) -> Option<(u64, u64)> {
    #[cfg(any(unix, windows))]
    {
        use cap_fs_ext::MetadataExt;
        Some((metadata.dev(), metadata.ino()))
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = metadata;
        None
    }
}
fn link_count(metadata: &Metadata) -> u64 {
    #[cfg(any(unix, windows))]
    {
        use cap_fs_ext::MetadataExt;
        metadata.nlink()
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = metadata;
        0
    }
}

struct Group {
    links: u64,
    consistent: bool,
    paths: Vec<PathBuf>,
}
#[derive(Default)]
pub(super) struct Aliases {
    groups: BTreeMap<(u64, u64), Group>,
    unknown: BTreeSet<PathBuf>,
}
impl Aliases {
    pub fn observe(&mut self, path: &Path, metadata: &Metadata) {
        let Some(identity) = identity(metadata) else {
            self.unknown.insert(path.into());
            return;
        };
        let links = link_count(metadata);
        let group = self.groups.entry(identity).or_insert_with(|| Group {
            links,
            consistent: links != 0,
            paths: Vec::new(),
        });
        group.consistent &= group.links == links;
        group.paths.push(path.into());
    }
    pub fn unproven(mut self) -> BTreeSet<PathBuf> {
        for group in self.groups.into_values() {
            if !group.consistent || group.paths.len() as u64 != group.links {
                self.unknown.extend(group.paths);
            }
        }
        self.unknown
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn changed_link_counts_during_inventory_leave_the_whole_group_unproven() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join("first"), "public").unwrap();
        let root =
            cap_std::fs::Dir::open_ambient_dir(temp.path(), cap_std::ambient_authority()).unwrap();
        let mut aliases = Aliases::default();
        aliases.observe(Path::new("first"), &root.symlink_metadata("first").unwrap());
        std::fs::hard_link(temp.path().join("first"), temp.path().join("second")).unwrap();
        aliases.observe(
            Path::new("second"),
            &root.symlink_metadata("second").unwrap(),
        );
        assert_eq!(
            aliases.unproven(),
            BTreeSet::from([PathBuf::from("first"), PathBuf::from("second")])
        );
    }
}
