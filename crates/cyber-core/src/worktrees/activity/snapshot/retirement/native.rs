//! Fixed native claim routes. The caller retains the consumed snapshot lifetime
//! and invokes each unlink only inside its durably planned Store writer callback.
use super::super::super::{LockedRecord, UseRecord};
use super::super::{
    Pins, SnapshotClaim, acknowledgement, bytes, contents, digest, directory_identity, open_file,
};
use super::*;
use cap_std::fs::Dir;
use std::os::unix::fs::MetadataExt;
use std::{
    collections::BTreeMap,
    fs::File,
    path::{Path, PathBuf},
};

struct RepositoryPin {
    path: PathBuf,
    common_identity: String,
    identity: String,
    file: LockedRecord,
}
impl RepositoryPin {
    fn open(binding: &CheckoutBinding, expected: Option<&str>) -> io::Result<Self> {
        let common = common(&binding.common_dir, &binding.common_identity)?;
        let file = open_file(&common, "cyber-worktree.lock")?;
        if !super::super::super::try_lock(&file)? {
            return Err(invalid("Checkout repository maintenance is busy"));
        }
        let file = LockedRecord(file);
        let identity = super::super::identity(&file)?;
        if expected.is_some_and(|expected| expected != identity) {
            return Err(invalid("Checkout retirement repository lock changed"));
        }
        super::super::publication::sync_file(&file)?;
        super::super::publication::sync_directory(&common)?;
        let pin = Self {
            path: binding.common_dir.clone(),
            common_identity: binding.common_identity.clone(),
            identity,
            file,
        };
        pin.verify()?;
        Ok(pin)
    }
    fn verify(&self) -> io::Result<()> {
        let common = common(&self.path, &self.common_identity)?;
        let current = open_file(&common, "cyber-worktree.lock")?;
        if super::super::identity(&current)? != self.identity
            || super::super::identity(&self.file)? != self.identity
        {
            return Err(invalid("Checkout retirement repository lock route changed"));
        }
        Ok(())
    }
}
fn common(path: &Path, expected: &str) -> io::Result<Dir> {
    if !path.is_absolute() {
        return Err(invalid(
            "Checkout retirement native path is unavailable on this host",
        ));
    }
    if std::fs::symlink_metadata(path)?.file_type().is_symlink() {
        return Err(invalid("Checkout retirement common route is a symlink"));
    }
    let dir = Dir::open_ambient_dir(path, cap_std::ambient_authority())?;
    if directory_identity(&dir)? != expected {
        return Err(invalid("Checkout retirement common route changed"));
    }
    Ok(dir)
}
fn generation(file: &File) -> io::Result<String> {
    let m = file.metadata()?;
    Ok(digest(
        &serde_json::to_vec(&(
            m.dev(),
            m.ino(),
            m.mode(),
            m.uid(),
            m.gid(),
            m.nlink(),
            m.len(),
            m.mtime(),
            m.mtime_nsec(),
            m.ctime(),
            m.ctime_nsec(),
        ))
        .map_err(invalid)?,
    ))
}
impl CheckoutRetirementFile {
    fn observe(file: &File) -> io::Result<Self> {
        super::super::regular(file)?;
        let generation = generation(file)?;
        let bytes = contents(file, 1024 * 1024)?;
        let observed = Self {
            identity: super::super::identity(file)?,
            generation: generation.clone(),
            hash: blake3::hash(&bytes).to_hex().to_string(),
            bytes: bytes.len() as u64,
        };
        observed.validate()?;
        if generation != self::generation(file)? {
            return Err(invalid("Checkout retirement file changed while hashing"));
        }
        Ok(observed)
    }
}
struct Leaf {
    file: LockedRecord,
    present: bool,
}
impl Leaf {
    fn open(
        pins: &Pins,
        name: &str,
        expected: &CheckoutRetirementFile,
    ) -> io::Result<Option<Self>> {
        let file = match open_file(&pins.directory, name) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            result => result?,
        };
        if !super::super::super::try_lock(&file)? {
            return Err(invalid("Checkout retirement leaf is still active"));
        }
        let file = LockedRecord(file);
        if CheckoutRetirementFile::observe(&file)? != *expected {
            return Err(invalid(
                "Checkout retirement leaf differs from the frozen plan",
            ));
        }
        Ok(Some(Self {
            file,
            present: true,
        }))
    }
    fn verify(&self, pins: &Pins, name: &str, expected: &CheckoutRetirementFile) -> io::Result<()> {
        if !self.present {
            return absent(&pins.directory, name);
        }
        let current = open_file(&pins.directory, name)?;
        super::super::regular(&self.file)?;
        if super::super::identity(&current)? != expected.identity
            || super::super::identity(&self.file)? != expected.identity
            || generation(&current)? != expected.generation
            || generation(&self.file)? != expected.generation
        {
            return Err(invalid(
                "Checkout retirement leaf route or generation changed",
            ));
        }
        Ok(())
    }
}
struct Claim {
    plan: CheckoutRetirementPlan,
    pins: Pins,
    claim: Option<Leaf>,
    acknowledgement: Option<Leaf>,
}
impl Claim {
    fn capture(claim: SnapshotClaim, repository_lock_identity: String) -> io::Result<Self> {
        let acknowledgement = claim
            .acknowledgement
            .as_ref()
            .ok_or_else(|| invalid("Checkout retirement requires an existing acknowledgement"))?;
        let (resources_digest, receipt_digest) = acknowledgement.retirement_context();
        let claim_file = CheckoutRetirementFile::observe(&claim.lease.file)?;
        let acknowledgement_file =
            CheckoutRetirementFile::observe(acknowledgement.file_for_retirement())?;
        // Verify typed original context AFTER observing bytes/generations. The
        // final guard check then rejects any mutation since those observations.
        claim.verify()?;
        let pins = claim
            .lease
            .snapshot_pins
            .as_ref()
            .ok_or_else(|| invalid("Missing checkout retirement pins"))?;
        super::super::publication::sync_file(&claim.lease.file)?;
        super::super::publication::sync_file(acknowledgement.file_for_retirement())?;
        super::super::publication::sync_file(&pins.creation)?;
        for directory in [&pins.common, &pins.root, &pins.directory, &pins.creations] {
            super::super::publication::sync_directory(directory)?;
        }
        let SnapshotClaim {
            lease,
            acknowledgement,
        } = claim;
        let acknowledgement =
            acknowledgement.ok_or_else(|| invalid("Missing captured checkout acknowledgement"))?;
        let plan = CheckoutRetirementPlan {
            version: 1,
            binding: lease
                .record
                .snapshot_binding
                .clone()
                .ok_or_else(|| invalid("Missing checkout retirement binding"))?,
            resources_digest,
            receipt_digest,
            repository_lock_identity,
            claim: claim_file,
            acknowledgement: acknowledgement_file,
        };
        plan.validate()?;
        Ok(Self {
            plan,
            pins: lease
                .snapshot_pins
                .ok_or_else(|| invalid("Missing checkout retirement pins"))?,
            claim: Some(Leaf {
                file: lease.file,
                present: true,
            }),
            acknowledgement: Some(Leaf {
                file: acknowledgement.into_retirement_file(),
                present: true,
            }),
        })
    }
    fn resume(plan: CheckoutRetirementPlan) -> io::Result<Self> {
        plan.validate()?;
        let pins = Pins::open(
            &plan.binding.common_dir,
            &plan.binding.worktree_id,
            &plan.binding.name,
        )?;
        verify_creation(&pins, &plan.binding)?;
        let mut claim = Leaf::open(&pins, &claim_name(&plan), &plan.claim)?;
        if let Some(leaf) = &mut claim {
            let record: UseRecord = super::super::super::read_record(&mut leaf.file)?;
            if record.snapshot_binding.as_ref() != Some(&plan.binding) || record.settled {
                return Err(invalid(
                    "Checkout retirement original claim context differs",
                ));
            }
            pins.verify(&leaf.file, &record)?;
        }
        let acknowledgement = Leaf::open(&pins, &ack_name(&plan), &plan.acknowledgement)?;
        if let Some(leaf) = &acknowledgement {
            acknowledgement::verify_retirement_context(&leaf.file, &plan)?;
        }
        Ok(Self {
            plan,
            pins,
            claim,
            acknowledgement,
        })
    }
    fn verify(&self) -> io::Result<()> {
        verify_creation(&self.pins, &self.plan.binding)?;
        verify_leaf(
            self.claim.as_ref(),
            &self.pins,
            &claim_name(&self.plan),
            &self.plan.claim,
        )?;
        verify_leaf(
            self.acknowledgement.as_ref(),
            &self.pins,
            &ack_name(&self.plan),
            &self.plan.acknowledgement,
        )?;
        let owner = &self.plan.binding.owner_id;
        let prefixes = [
            format!("preparing_{}_{owner}_", owner.len()),
            format!("ack_preparing_{}_{owner}_", owner.len()),
        ];
        for (index, entry) in self.pins.directory.entries()?.enumerate() {
            let name = entry?.file_name();
            if index >= 4096 {
                return Err(invalid("Checkout retirement inventory exceeds limit"));
            }
            if prefixes
                .iter()
                .any(|prefix| name.to_string_lossy().starts_with(prefix))
            {
                return Err(invalid(
                    "Checkout retirement retains unknown same-owner construction evidence",
                ));
            }
        }
        Ok(())
    }
}
fn verify_creation(pins: &Pins, binding: &CheckoutBinding) -> io::Result<()> {
    pins.matches(binding)?;
    let current = Pins::open(&binding.common_dir, &binding.worktree_id, &binding.name)?;
    current.matches(binding)?;
    if digest(&bytes(&pins.creation)?) != binding.creation_digest
        || digest(&bytes(&current.creation)?) != binding.creation_digest
    {
        return Err(invalid("Checkout retirement creation bytes changed"));
    }
    Ok(())
}
fn claim_name(plan: &CheckoutRetirementPlan) -> String {
    format!("{}.lock", plan.binding.owner_id)
}
fn ack_name(plan: &CheckoutRetirementPlan) -> String {
    format!("{}.snapshot-ack", plan.binding.owner_id)
}
fn absent(dir: &Dir, name: &str) -> io::Result<()> {
    match dir.symlink_metadata(name) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
        Ok(_) => Err(invalid("Retired checkout metadata route was replaced")),
    }
}
fn verify_leaf(
    leaf: Option<&Leaf>,
    pins: &Pins,
    name: &str,
    expected: &CheckoutRetirementFile,
) -> io::Result<()> {
    match leaf {
        Some(leaf) => leaf.verify(pins, name, expected),
        None => absent(&pins.directory, name),
    }
}

#[cfg(test)]
mod tests;

/// Retains native evidence through a trusted adapter's last filesystem call.
/// It supplies no Store receipt, reference, retention or source-mutation authority.
pub struct CheckoutRetirementGuard {
    claims: Vec<Claim>,
    repositories: BTreeMap<PathBuf, RepositoryPin>,
}
impl CheckoutRetirementGuard {
    /// Every claim must already carry its exact consuming acknowledgement. The
    /// caller additionally retains the original snapshot lifetime and Store proof.
    pub fn prepare(claims: Vec<SnapshotClaim>) -> io::Result<Self> {
        if claims.len() > LIMIT {
            return Err(invalid("Checkout retirement resource limit exceeded"));
        }
        let mut total = 0u64;
        for claim in &claims {
            let ack = claim.acknowledgement.as_ref().ok_or_else(|| {
                invalid("Checkout retirement requires an existing acknowledgement")
            })?;
            for file in [&*claim.lease.file, ack.file_for_retirement()] {
                total = total
                    .checked_add(file.metadata()?.len())
                    .filter(|bytes| *bytes <= TOTAL_BYTES)
                    .ok_or_else(|| invalid("Checkout retirement total bytes exceed limit"))?;
            }
        }
        let mut repositories = BTreeMap::<PathBuf, RepositoryPin>::new();
        for claim in &claims {
            let binding = claim.binding();
            if !repositories.contains_key(&binding.common_dir) {
                repositories.insert(
                    binding.common_dir.clone(),
                    RepositoryPin::open(binding, None)?,
                );
            }
        }
        let claims = claims
            .into_iter()
            .map(|claim| {
                let repository = &repositories[&claim.binding().common_dir];
                Claim::capture(claim, repository.identity.clone())
            })
            .collect::<io::Result<Vec<_>>>()?;
        let guard = Self {
            claims,
            repositories,
        };
        guard.validate()?;
        guard.verify()?;
        Ok(guard)
    }
    /// Missing leaves are authorized progress ONLY by an exact already committed
    /// Store metadata plan. The trusted adapter must attest that before calling.
    pub fn resume_committed(plans: Vec<CheckoutRetirementPlan>) -> io::Result<Self> {
        CheckoutRetirementPlan::validate_set(&plans)?;
        let mut repositories = BTreeMap::<PathBuf, RepositoryPin>::new();
        for plan in &plans {
            match repositories.get(&plan.binding.common_dir) {
                Some(repository) if repository.identity != plan.repository_lock_identity => {
                    return Err(invalid("Checkout retirement repository plans differ"));
                }
                Some(_) => {}
                None => {
                    repositories.insert(
                        plan.binding.common_dir.clone(),
                        RepositoryPin::open(&plan.binding, Some(&plan.repository_lock_identity))?,
                    );
                }
            }
        }
        let guard = Self {
            claims: plans
                .into_iter()
                .map(Claim::resume)
                .collect::<io::Result<_>>()?,
            repositories,
        };
        guard.verify()?;
        Ok(guard)
    }
    pub fn plans(&self) -> Vec<CheckoutRetirementPlan> {
        self.claims.iter().map(|claim| claim.plan.clone()).collect()
    }
    fn validate(&self) -> io::Result<()> {
        CheckoutRetirementPlan::validate_set(&self.plans())
    }
    pub fn verify(&self) -> io::Result<()> {
        for repository in self.repositories.values() {
            repository.verify()?;
        }
        for claim in &self.claims {
            claim.verify()?;
        }
        Ok(())
    }
    /// One exact unlink per call, acknowledgement before original claim. Callers
    /// re-attest their retained Store/native/retention scope before EVERY call.
    pub fn remove_next(&mut self) -> io::Result<bool> {
        self.verify()?;
        for acknowledgement in [true, false] {
            for claim in &mut self.claims {
                let (leaf, name) = if acknowledgement {
                    (&mut claim.acknowledgement, ack_name(&claim.plan))
                } else {
                    (&mut claim.claim, claim_name(&claim.plan))
                };
                if let Some(leaf) = leaf.as_mut().filter(|leaf| leaf.present) {
                    claim.pins.directory.remove_file(name)?;
                    super::super::publication::sync_directory(&claim.pins.directory)?;
                    leaf.present = false;
                    self.verify()?;
                    return Ok(true);
                }
            }
        }
        // Missing planned names may have been observed after an interrupted
        // unlink whose first directory sync failed. Bind their absence now.
        for claim in &self.claims {
            super::super::publication::sync_directory(&claim.pins.directory)?;
        }
        self.verify()?;
        Ok(false)
    }
}
