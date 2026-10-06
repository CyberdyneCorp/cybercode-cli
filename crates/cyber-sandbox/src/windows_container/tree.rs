//! Preflight records for recursive policy preparation; no ACL mutation.

use super::*;
use identity::{FileRecord, ObjectKey};

/// Owns verified records and checked directory pins for an observed tree.
/// Child creation/deletion is not frozen; future children require separate tracking.
pub struct TreeInventory {
    nodes: Vec<Node>,
    pins: Vec<File>,
}

struct Node {
    path: PathBuf,
    directory: bool,
    parent: Option<usize>,
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
            nodes: vec![Node::capture(root, None)?],
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

    pub(super) fn grant_policy(
        mut self,
        profile: &Profile,
        policy: &ExistingTreePolicy,
    ) -> io::Result<ExistingTreeGrant> {
        let policies = self.resolve_rules(policy)?;
        self.verify()?;
        let mut owner = ExistingTreeGrant { leases: Vec::new() };
        for (node, access) in self.nodes.drain(..).zip(policies) {
            match identity::grant_record_policy(profile, node.record, access.policy()) {
                Ok(lease) => owner.leases.push(lease),
                Err(setup) => return Err(owner.rollback_error(setup)),
            }
        }
        if let Err(setup) = owner.verify() {
            return Err(owner.rollback_error(setup));
        }
        Ok(owner)
    }

    fn resolve_rules(&self, policy: &ExistingTreePolicy) -> io::Result<Vec<ObjectRules>> {
        let readonly = self.exclusion_keys(&policy.read_only)?;
        let hidden = self.exclusion_keys(&policy.unreadable)?;
        let base = ObjectRules {
            writable: matches!(policy.access, Access::Write),
            read_only: false,
            unreadable: false,
        };
        let mut rules = Vec::with_capacity(self.nodes.len());
        for node in &self.nodes {
            let mut access = node.parent.map_or(base, |parent| rules[parent]);
            access.read_only |= readonly.contains(&node.record.key());
            access.unreadable |= hidden.contains(&node.record.key());
            rules.push(access);
        }
        Ok(rules)
    }

    fn exclusion_keys(
        &self,
        paths: &[PathBuf],
    ) -> io::Result<std::collections::HashSet<ObjectKey>> {
        let mut keys = std::collections::HashSet::new();
        for path in paths {
            let _ancestors = retain_ancestors(path)?;
            let file = identity::open_object(path)?;
            identity::validate_object(&file)?;
            let record = FileRecord::capture(path, &file)?;
            if !self
                .nodes
                .iter()
                .any(|node| node.record.key() == record.key())
            {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "Exclusion is outside the inventoried tree",
                ));
            }
            keys.insert(record.key());
        }
        Ok(keys)
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
            let node = Node::capture(&path, Some(cursor))?;
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
    fn capture(path: &Path, parent: Option<usize>) -> io::Result<Self> {
        let file = identity::open_object(path)?;
        let directory = identity::validate_object(&file)?;
        Ok(Self {
            path: path.to_owned(),
            directory,
            parent,
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

/// Restrictions on existing objects. Missing and out-of-tree paths are refused.
#[derive(Clone)]
pub struct ExistingTreePolicy {
    pub access: Access,
    pub read_only: Vec<PathBuf>,
    pub unreadable: Vec<PathBuf>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum ObjectPolicy {
    ReadOnly,
    Writable,
    Unreadable,
}

impl ObjectPolicy {
    pub(super) fn entries(self, sid: PSID) -> Vec<EXPLICIT_ACCESS_W> {
        let security = WRITE_DAC | WRITE_OWNER;
        // Generic write includes READ_CONTROL and SYNCHRONIZE, which reads need.
        let writes = (FILE_GENERIC_WRITE & !(READ_CONTROL | SYNCHRONIZE))
            | DELETE
            | FILE_DELETE_CHILD
            | security;
        let denied = match self {
            Self::ReadOnly => writes,
            Self::Writable => security | FILE_DELETE_CHILD,
            Self::Unreadable => FILE_ALL_ACCESS,
        };
        let mut entries = vec![policy_entry(sid, denied, DENY_ACCESS)];
        let allowed = match self {
            Self::ReadOnly => Some(Access::Read.mask()),
            Self::Writable => Some(Access::Write.mask()),
            Self::Unreadable => None,
        };
        if let Some(mask) = allowed {
            entries.push(policy_entry(sid, mask, GRANT_ACCESS));
        }
        entries
    }
}

fn policy_entry(
    sid: PSID,
    mask: u32,
    mode: windows_sys::Win32::Security::Authorization::ACCESS_MODE,
) -> EXPLICIT_ACCESS_W {
    EXPLICIT_ACCESS_W {
        grfAccessPermissions: mask,
        grfAccessMode: mode,
        grfInheritance: 0,
        Trustee: TRUSTEE_W {
            TrusteeForm: TRUSTEE_IS_SID,
            ptstrName: sid.cast(),
            ..Default::default()
        },
    }
}

/// A root's base access and explicit restrictions, resolved before any mutation.
#[derive(Clone)]
pub struct ExistingTreeRoot {
    pub path: PathBuf,
    pub policy: ExistingTreePolicy,
}

#[derive(Clone, Copy)]
struct ObjectRules {
    writable: bool,
    read_only: bool,
    unreadable: bool,
}

impl ObjectRules {
    fn combine(&mut self, other: Self) {
        self.writable |= other.writable;
        self.read_only |= other.read_only;
        self.unreadable |= other.unreadable;
    }

    fn policy(self) -> ObjectPolicy {
        if self.unreadable {
            ObjectPolicy::Unreadable
        } else if self.read_only || !self.writable {
            ObjectPolicy::ReadOnly
        } else {
            ObjectPolicy::Writable
        }
    }
}

struct ForestPreparation {
    nodes: Vec<(Node, ObjectRules)>,
    indexes: std::collections::HashMap<ObjectKey, usize>,
    pins: Vec<File>,
}

impl ForestPreparation {
    fn add(&mut self, root: &ExistingTreeRoot, limit: usize) -> io::Result<()> {
        let mut tree = TreeInventory::capture(&root.path, limit)?;
        let rules = tree.resolve_rules(&root.policy)?;
        for (node, rule) in tree.nodes.drain(..).zip(rules) {
            let key = node.record.key();
            if let Some(&index) = self.indexes.get(&key) {
                self.nodes[index].1.combine(rule);
            } else {
                if self.nodes.len() >= limit {
                    return Err(io::Error::other("Forest inventory object limit exceeded"));
                }
                self.indexes.insert(key, self.nodes.len());
                self.nodes.push((node, rule));
            }
        }
        self.pins.append(&mut tree.pins);
        Ok(())
    }

    fn grant(mut self, profile: &Profile) -> io::Result<ExistingTreeGrant> {
        let mut owner = ExistingTreeGrant { leases: Vec::new() };
        for (node, rule) in self.nodes.drain(..) {
            match identity::grant_record_policy(profile, node.record, rule.policy()) {
                Ok(lease) => owner.leases.push(lease),
                Err(setup) => return Err(owner.rollback_error(setup)),
            }
        }
        if let Err(setup) = owner.verify() {
            return Err(owner.rollback_error(setup));
        }
        Ok(owner)
    }
}

pub(super) fn grant_forest(
    profile: &Profile,
    roots: &[ExistingTreeRoot],
    limit: usize,
) -> io::Result<ExistingTreeGrant> {
    if roots.is_empty() || limit == 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Forest preparation requires roots and a nonzero object limit",
        ));
    }
    let mut forest = ForestPreparation {
        nodes: Vec::new(),
        indexes: std::collections::HashMap::new(),
        pins: Vec::new(),
    };
    for root in roots {
        forest.add(root, limit)?;
    }
    forest.grant(profile)
}
