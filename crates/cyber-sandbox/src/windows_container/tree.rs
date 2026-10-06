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
