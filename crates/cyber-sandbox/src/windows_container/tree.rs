//! Preflight records for recursive policy preparation; no ACL mutation.

use super::*;
use identity::FileRecord;

/// Owns verified records and checked directory pins for an observed tree.
/// Child creation/deletion is not frozen; future children require separate tracking.
pub struct TreeInventory {
    nodes: Vec<Node>,
    pins: Vec<File>,
}

struct Node {
    path: PathBuf,
    directory: bool,
    record: FileRecord,
}

impl TreeInventory {
    /// Inventory an ordinary local directory without changing its security.
    /// The limit includes the root. Any failure releases all preparation pins.
    pub fn capture(root: &Path, limit: usize) -> io::Result<Self> {
        if limit == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Tree inventory requires a nonzero object limit",
            ));
        }
        let mut pins = retain_ancestors(root)?;
        pins.push(pin_directory(root)?);
        let mut tree = Self {
            nodes: vec![Node::capture(root)?],
            pins,
        };
        let mut cursor = 0;
        while cursor < tree.nodes.len() {
            if tree.nodes[cursor].directory {
                tree.enumerate(cursor, limit)?;
            }
            cursor += 1;
        }
        tree.verify()?;
        Ok(tree)
    }

    /// Original paths are labels, never authority for later ACL mutation.
    pub fn paths(&self) -> impl Iterator<Item = &Path> {
        self.nodes.iter().map(|node| node.path.as_path())
    }

    /// Reopen the recorded objects, refusing deletion, reparse conversion or
    /// new hard-link aliases. This does not discover later-created children.
    pub fn verify(&self) -> io::Result<()> {
        for node in &self.nodes {
            let file = node.record.open()?;
            if identity::validate_object(&file)? != node.directory {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Recorded tree object type changed",
                ));
            }
        }
        Ok(())
    }

    #[cfg(test)]
    pub(super) fn release_pins_for_test(&mut self) {
        self.pins.clear();
    }

    pub(super) fn grant_existing(
        mut self,
        profile: &Profile,
        access: Access,
    ) -> io::Result<ExistingTreeGrant> {
        self.verify()?;
        let mut owner = ExistingTreeGrant { leases: Vec::new() };
        // Directory pins remain owned by self until every grant is prepared.
        // An error drops owner first, rolling back its earlier successful grants.
        for node in self.nodes.drain(..) {
            match identity::grant_record(profile, node.record, access, 0) {
                Ok(lease) => owner.leases.push(lease),
                Err(setup) => return Err(owner.rollback_error(setup)),
            }
        }
        if let Err(setup) = owner.verify() {
            return Err(owner.rollback_error(setup));
        }
        Ok(owner)
    }

    fn enumerate(&mut self, cursor: usize, limit: usize) -> io::Result<()> {
        for entry in std::fs::read_dir(&self.nodes[cursor].path)? {
            let entry = entry?;
            if self.nodes.len() >= limit {
                return Err(io::Error::other("Tree inventory object limit exceeded"));
            }
            let path = entry.path();
            validate_local_path(&path.components().collect::<Vec<_>>())?;
            // Pin directories before recording/enumerating them. Metadata is a
            // hint only; the opened object receives the authoritative check.
            let directory_hint = entry.file_type()?.is_dir();
            if directory_hint {
                self.pins.push(pin_directory(&path)?);
            }
            let node = Node::capture(&path)?;
            if node.directory && !directory_hint {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Tree entry changed during preparation",
                ));
            }
            self.nodes.push(node);
        }
        Ok(())
    }
}

impl Node {
    fn capture(path: &Path) -> io::Result<Self> {
        let file = identity::open_object(path)?;
        let directory = identity::validate_object(&file)?;
        Ok(Self {
            path: path.to_owned(),
            directory,
            record: FileRecord::capture(path, &file)?,
        })
    }
}

/// Revocable grants for observed existing objects, without child inheritance.
/// It owns no child handles, so ordinary directory moves remain possible.
pub struct ExistingTreeGrant {
    leases: Vec<IdentityGrant>,
}

impl ExistingTreeGrant {
    fn verify(&self) -> io::Result<()> {
        for lease in &self.leases {
            lease.verify()?;
        }
        Ok(())
    }

    fn rollback_error(&mut self, setup: io::Error) -> io::Error {
        match self.close() {
            Ok(()) => setup,
            Err(rollback) => io::Error::other(format!(
                "Tree grant setup failed: {setup}; rollback failed: {rollback}"
            )),
        }
    }

    /// Attempt every revocation; failed leases remain owned for a later retry.
    pub fn close(&mut self) -> io::Result<()> {
        let mut first_error = None;
        self.leases.retain_mut(|lease| match lease.revoke() {
            Ok(()) => false,
            Err(error) => {
                first_error.get_or_insert(error);
                true
            }
        });
        first_error.map_or(Ok(()), Err)
    }
}

impl Drop for ExistingTreeGrant {
    fn drop(&mut self) {
        if let Err(error) = self.close() {
            cleanup_error("existing tree ACL revocation", error);
        }
    }
}
