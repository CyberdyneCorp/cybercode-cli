//! Durable disposition of an exact claim, separate from its immutable evidence.
use super::super::{LockedRecord, Managed, UseRecord, invalid};
use super::{CheckoutBinding, Pins, SnapshotClaim, contents, identity, open_file, regular};
use cap_fs_ext::{FollowSymlinks, OpenOptionsFollowExt};
use cap_std::fs::OpenOptions;
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    fs::File,
    io::{self, Write},
    path::PathBuf,
};

const LIMIT: usize = 32 * 1024;
const SUFFIX: &str = ".snapshot-ack";

/// Trusted adapter authority issued only after observing the exact durable receipt.
/// Digests alone never authorize acquisition of a different or historical claim.
pub struct CheckoutAcknowledgement {
    binding: CheckoutBinding,
    resources_digest: String,
    receipt_digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    version: u32,
    binding: CheckoutBinding,
    resources_digest: String,
    receipt_digest: String,
    file_identity: String,
    staging_name: String,
}

pub(super) struct Acknowledged {
    file: LockedRecord,
    record: Record,
}

fn hash(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}
fn stage_prefix(owner: &str) -> String {
    format!("ack_preparing_{}_{owner}_", owner.len())
}
fn stage_owner(name: &str) -> io::Result<&str> {
    let tail = name
        .strip_prefix("ack_preparing_")
        .ok_or_else(|| invalid("Invalid acknowledgement stage"))?;
    let (length, tail) = tail
        .split_once('_')
        .ok_or_else(|| invalid("Invalid acknowledgement stage length"))?;
    let length: usize = length.parse().map_err(invalid)?;
    let owner = tail
        .get(..length)
        .ok_or_else(|| invalid("Invalid acknowledgement stage owner"))?;
    super::super::safe_id(owner, "op")?;
    let nonce = tail
        .get(length..)
        .and_then(|tail| tail.strip_prefix('_'))
        .ok_or_else(|| invalid("Invalid acknowledgement stage nonce"))?;
    if nonce.len() != 26
        || !nonce.bytes().all(|byte| byte.is_ascii_alphanumeric())
        || name != format!("{}{nonce}", stage_prefix(owner))
    {
        return Err(invalid("Invalid acknowledgement construction name"));
    }
    Ok(owner)
}
impl Acknowledged {
    fn open(pins: &Pins, binding: &CheckoutBinding) -> io::Result<Self> {
        let file = open_file(&pins.directory, &format!("{}{SUFFIX}", binding.owner_id))?;
        if !super::super::try_lock(&file)? {
            return Err(invalid("Checkout acknowledgement is still active"));
        }
        let file = LockedRecord(file);
        let record: Record = serde_json::from_slice(&contents(&file, LIMIT)?).map_err(invalid)?;
        let acknowledged = Self { file, record };
        acknowledged.verify(pins, binding)?;
        Ok(acknowledged)
    }
    pub(super) fn verify(&self, pins: &Pins, binding: &CheckoutBinding) -> io::Result<()> {
        if self.record.version != 1
            || self.record.binding != *binding
            || binding.version != 2
            || binding.lifetime.is_none()
            || !hash(&self.record.resources_digest)
            || !hash(&self.record.receipt_digest)
            || stage_owner(&self.record.staging_name)? != binding.owner_id
        {
            return Err(invalid("Checkout acknowledgement ownership changed"));
        }
        regular(&self.file)?;
        let current = if pins.archived {
            Pins::open_for_removal(&binding.common_dir, &binding.worktree_id, &binding.name)?
        } else {
            Pins::open(&binding.common_dir, &binding.worktree_id, &binding.name)?
        };
        pins.matches(binding)?;
        current.matches(binding)?;
        let routed = open_file(&current.directory, &format!("{}{SUFFIX}", binding.owner_id))?;
        let observed: Record =
            serde_json::from_slice(&contents(&self.file, LIMIT)?).map_err(invalid)?;
        if identity(&self.file)? != self.record.file_identity
            || identity(&routed)? != self.record.file_identity
            || observed != self.record
        {
            return Err(invalid(
                "Checkout acknowledgement route or contents changed",
            ));
        }
        Ok(())
    }
}

impl CheckoutAcknowledgement {
    pub fn new(
        binding: &CheckoutBinding,
        resources_digest: &str,
        receipt_digest: &str,
    ) -> io::Result<Self> {
        if binding.version != 2
            || binding.lifetime.is_none()
            || !hash(resources_digest)
            || !hash(receipt_digest)
        {
            return Err(invalid(
                "Checkout acknowledgement lacks exact consuming receipt proof",
            ));
        }
        Ok(Self {
            binding: binding.clone(),
            resources_digest: resources_digest.into(),
            receipt_digest: receipt_digest.into(),
        })
    }
    /// Native metadata only. The caller must retain matching owned or exited lifetime and
    /// complete-resource proof through the observed Store commit and this call.
    pub fn publish(&self, claim: &mut SnapshotClaim) -> io::Result<()> {
        self.publish_before(claim, || Ok(()))
    }
    fn publish_before(
        &self,
        claim: &mut SnapshotClaim,
        before: impl FnOnce() -> io::Result<()>,
    ) -> io::Result<()> {
        if !cfg!(any(target_os = "macos", target_os = "linux")) {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "Durable snapshot checkout acknowledgement is unavailable",
            ));
        }
        self.retain_existing(claim)?;
        if claim.acknowledgement.is_none() {
            let acknowledged = self.create(claim, before)?;
            claim.acknowledgement = Some(acknowledged);
        }
        let acknowledged = claim
            .acknowledgement
            .as_ref()
            .expect("acknowledgement installed");
        let pins = claim
            .lease
            .snapshot_pins
            .as_ref()
            .expect("retained checkout pins");
        super::publication::sync_file(&acknowledged.file)?;
        super::publication::sync_directory(&pins.directory)?;
        claim.verify()
    }
    /// Read-only candidate verification. Retains an existing exact marker through
    /// the consuming transaction; never creates acknowledgement metadata.
    pub fn retain_existing(&self, claim: &mut SnapshotClaim) -> io::Result<()> {
        claim.verify()?;
        if claim.binding() != &self.binding {
            return Err(invalid(
                "Checkout acknowledgement differs from retained original claim",
            ));
        }
        if let Some(acknowledged) = &claim.acknowledgement {
            return self.matches(&acknowledged.record);
        }
        let pins = claim
            .lease
            .snapshot_pins
            .as_ref()
            .ok_or_else(|| invalid("Missing original checkout pins"))?;
        match pins
            .directory
            .symlink_metadata(format!("{}{SUFFIX}", self.binding.owner_id))
        {
            Ok(_) => {
                let acknowledged = Acknowledged::open(pins, &self.binding)?;
                self.matches(&acknowledged.record)?;
                claim.acknowledgement = Some(acknowledged);
                claim.verify()
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error),
        }
    }
    fn matches(&self, record: &Record) -> io::Result<()> {
        if record.binding != self.binding
            || record.resources_digest != self.resources_digest
            || record.receipt_digest != self.receipt_digest
        {
            return Err(invalid(
                "Checkout already has a different consuming receipt",
            ));
        }
        Ok(())
    }
    fn create(
        &self,
        claim: &SnapshotClaim,
        before: impl FnOnce() -> io::Result<()>,
    ) -> io::Result<Acknowledged> {
        let pins = claim
            .lease
            .snapshot_pins
            .as_ref()
            .ok_or_else(|| invalid("Missing checkout pins"))?;
        for (index, entry) in pins.directory.entries()?.enumerate() {
            entry?;
            if index >= 4095 {
                return Err(invalid("Checkout acknowledgement inventory exceeds limit"));
            }
        }
        let stage = format!(
            "{}{}",
            stage_prefix(&self.binding.owner_id),
            ulid::Ulid::new()
        );
        let mut options = OpenOptions::new();
        options
            .read(true)
            .write(true)
            .create_new(true)
            .follow(FollowSymlinks::No);
        #[cfg(unix)]
        {
            use cap_std::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = pins.directory.open_with(&stage, &options)?.into_std();
        if !super::super::try_lock(&file)? {
            return Err(invalid("Checkout acknowledgement construction is busy"));
        }
        let mut file = LockedRecord(file);
        let record = Record {
            version: 1,
            binding: self.binding.clone(),
            resources_digest: self.resources_digest.clone(),
            receipt_digest: self.receipt_digest.clone(),
            file_identity: identity(&file)?,
            staging_name: stage.clone(),
        };
        let bytes = serde_json::to_vec(&record).map_err(invalid)?;
        if bytes.len() > LIMIT {
            return Err(invalid("Checkout acknowledgement exceeds limit"));
        }
        file.write_all(&bytes)?;
        super::publication::sync_file(&file)?;
        super::publication::sync_directory(&pins.directory)?;
        before()?;
        claim.verify()?;
        let routed = open_file(&pins.directory, &stage)?;
        if identity(&routed)? != record.file_identity || contents(&file, LIMIT)? != bytes {
            return Err(invalid("Checkout acknowledgement construction changed"));
        }
        super::publication::publish_name(
            &pins.directory,
            &stage,
            &format!("{}{SUFFIX}", self.binding.owner_id),
        )?;
        super::publication::sync_directory(&pins.directory)?;
        let acknowledged = Acknowledged { file, record };
        acknowledged.verify(pins, &self.binding)?;
        Ok(acknowledged)
    }
}

pub(in crate::worktrees::activity) fn for_removal(
    managed: &Managed,
    record: &UseRecord,
    file: &File,
) -> io::Result<Option<(CheckoutBinding, Box<dyn Send>)>> {
    let Some(binding) = &record.snapshot_binding else {
        return Ok(None);
    };
    let path =
        super::super::use_directory(managed, false)?.join(format!("{}{SUFFIX}", record.owner_id));
    match std::fs::symlink_metadata(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
        Ok(_) => {}
    }
    let pins = Pins::open_for_removal(&managed.common_dir, &managed.id, &managed.name)?;
    let observed: Managed =
        serde_json::from_slice(&super::bytes(&pins.creation)?).map_err(invalid)?;
    if observed != *managed || !observed.ready {
        return Err(invalid("Acknowledged removal creation context differs"));
    }
    pins.verify(file, record)?;
    let acknowledged = Acknowledged::open(&pins, binding)?;
    super::publication::sync_file(&acknowledged.file)?;
    super::publication::sync_directory(&pins.directory)?;
    Ok(Some((binding.clone(), Box::new((acknowledged, pins)))))
}

pub(in crate::worktrees::activity) struct Inventory {
    markers: HashSet<String>,
    stages: Vec<(String, String)>,
}
impl Inventory {
    pub(in crate::worktrees::activity) fn split(
        paths: Vec<PathBuf>,
    ) -> io::Result<(Vec<PathBuf>, Self)> {
        if paths.len() > 4096 {
            return Err(invalid("Checkout activity inventory exceeds limit"));
        }
        let mut inventory = Self {
            markers: HashSet::new(),
            stages: vec![],
        };
        let mut claims = vec![];
        for path in paths {
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or_else(|| invalid("Invalid checkout activity name"))?;
            if let Some(owner) = name.strip_suffix(SUFFIX) {
                super::super::safe_id(owner, "op")?;
                inventory.markers.insert(owner.into());
            } else if name.starts_with("ack_preparing_") {
                inventory
                    .stages
                    .push((stage_owner(name)?.into(), name.into()));
            } else {
                claims.push(path);
            }
        }
        Ok((claims, inventory))
    }
    pub(in crate::worktrees::activity) fn retain(
        self,
        managed: &Managed,
        acknowledged: &HashMap<String, CheckoutBinding>,
    ) -> io::Result<Vec<LockedRecord>> {
        if self
            .markers
            .iter()
            .any(|owner| !acknowledged.contains_key(owner))
        {
            return Err(invalid("Orphan or unverified checkout acknowledgement"));
        }
        let mut files = vec![];
        for (owner, stage) in self.stages {
            let binding = acknowledged
                .get(&owner)
                .ok_or_else(|| invalid("Unacknowledged checkout construction remains"))?;
            let pins = Pins::open_for_removal(&managed.common_dir, &managed.id, &managed.name)?;
            pins.matches(binding)?;
            let file = open_file(&pins.directory, &stage)?;
            if !super::super::try_lock(&file)? {
                return Err(invalid("Checkout acknowledgement construction is active"));
            }
            files.push(LockedRecord(file));
        }
        Ok(files)
    }
}

#[cfg(all(test, any(target_os = "macos", target_os = "linux")))]
mod tests;
