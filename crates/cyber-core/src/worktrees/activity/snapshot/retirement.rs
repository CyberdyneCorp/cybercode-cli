//! Frozen observations are data; deletion additionally needs the native guard
//! inside a consuming Store writer's reference/retention authorization callback.
use super::{CheckoutBinding, invalid};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, io};
const LIMIT: usize = 8192;
const TOTAL_BYTES: u64 = 64 * 1024 * 1024;

#[cfg(any(target_os = "linux", target_os = "macos"))]
mod native;
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub use native::CheckoutRetirementGuard;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CheckoutRetirementFile {
    pub identity: String,
    pub generation: String,
    pub hash: String,
    pub bytes: u64,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CheckoutRetirementPlan {
    pub version: u32,
    pub binding: CheckoutBinding,
    pub resources_digest: String,
    pub receipt_digest: String,
    pub repository_lock_identity: String,
    pub claim: CheckoutRetirementFile,
    pub acknowledgement: CheckoutRetirementFile,
}
pub(super) fn hash(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}
fn identity(value: &str) -> bool {
    let mut parts = value.split(':');
    if value.len() > 256 {
        return false;
    }
    for index in 0..7 {
        let Some(part) = parts.next() else {
            return false;
        };
        let Ok(value) = part.parse::<u64>() else {
            return false;
        };
        if part.is_empty()
            || !part.bytes().all(|byte| byte.is_ascii_digit())
            || index == 3 && value >= 1_000_000_000
        {
            return false;
        }
    }
    parts.next().is_none()
}
fn portable_absolute(path: &std::path::Path) -> bool {
    let Some(path) = path.to_str().filter(|path| path.len() <= 4096) else {
        return false;
    };
    path.starts_with('/')
        || path.starts_with("\\\\")
        || path.as_bytes().get(1..3).is_some_and(|tail| {
            tail[0] == b':'
                && matches!(tail[1], b'/' | b'\\')
                && path.as_bytes()[0].is_ascii_alphabetic()
        })
}
fn validate_binding(binding: &CheckoutBinding) -> io::Result<()> {
    super::super::safe_id(&binding.owner_id, "op")?;
    super::super::safe_id(&binding.worktree_id, "wt")?;
    super::super::super::Name::parse(&binding.name).map_err(invalid)?;
    let owner = binding
        .lifetime
        .as_ref()
        .ok_or_else(|| invalid("Missing retirement lifetime"))?;
    super::super::safe_id(&owner.session_id, "ses")?;
    if binding.version != 2
        || owner.activity_id != binding.owner_id
        || !hash(&owner.lifetime_digest)
        || owner.session_created_id.is_empty()
        || owner.session_created_id.len() > 256
        || !portable_absolute(&owner.directory)
        || !portable_absolute(&binding.common_dir)
        || !hash(&binding.creation_digest)
    {
        return Err(invalid("Invalid checkout retirement binding"));
    }
    validate_namespace(binding)
}
fn validate_namespace(binding: &CheckoutBinding) -> io::Result<()> {
    for value in [
        &binding.common_identity,
        &binding.root_identity,
        &binding.directory_identity,
        &binding.creation_directory_identity,
        &binding.creation_file_identity,
    ] {
        if !identity(value) {
            return Err(invalid("Invalid retirement namespace identity"));
        }
    }
    let prefix = format!("preparing_{}_{}_", binding.owner_id.len(), binding.owner_id);
    if binding.staging_name.as_deref().is_none_or(|stage| {
        !stage.starts_with(&prefix)
            || stage.len() != prefix.len() + 26
            || stage[prefix.len()..].parse::<ulid::Ulid>().is_err()
    }) {
        return Err(invalid("Invalid retirement construction binding"));
    }
    Ok(())
}

impl CheckoutRetirementFile {
    pub fn validate(&self) -> io::Result<()> {
        if !identity(&self.identity)
            || !hash(&self.generation)
            || !hash(&self.hash)
            || self.bytes > 1024 * 1024
        {
            return Err(invalid("Invalid checkout retirement file bounds"));
        }
        Ok(())
    }
}
impl CheckoutRetirementPlan {
    /// Data validation only; never native acquisition, acknowledgement or deletion.
    pub fn validate(&self) -> io::Result<()> {
        let binding = &self.binding;
        validate_binding(binding)?;
        if self.version != 1
            || !hash(&self.resources_digest)
            || !hash(&self.receipt_digest)
            || !identity(&self.repository_lock_identity)
        {
            return Err(invalid("Invalid checkout retirement ownership"));
        }
        self.claim.validate()?;
        self.acknowledgement.validate()?;
        if self.claim.identity != binding.file_identity
            || self.claim.identity == self.acknowledgement.identity
            || self.repository_lock_identity == self.claim.identity
            || self.repository_lock_identity == self.acknowledgement.identity
        {
            return Err(invalid("Aliased checkout retirement roles"));
        }
        Ok(())
    }
    /// Bounds and uniqueness for a complete metadata frame; still no native authority.
    pub fn validate_set(plans: &[Self]) -> io::Result<()> {
        if plans.len() > LIMIT {
            return Err(invalid("Checkout retirement resource limit exceeded"));
        }
        let mut owners = BTreeSet::new();
        let mut identities = BTreeSet::new();
        let mut bytes = 0u64;
        for plan in plans {
            plan.validate()?;
            if !owners.insert((
                &plan.binding.common_dir,
                &plan.binding.worktree_id,
                &plan.binding.owner_id,
            )) || !identities.insert(&plan.claim.identity)
                || !identities.insert(&plan.acknowledgement.identity)
            {
                return Err(invalid(
                    "Duplicate checkout retirement resource or native file",
                ));
            }
            bytes = bytes
                .checked_add(plan.total_bytes()?)
                .filter(|bytes| *bytes <= TOTAL_BYTES)
                .ok_or_else(|| invalid("Checkout retirement total bytes exceed limit"))?;
        }
        Ok(())
    }
    pub fn total_bytes(&self) -> io::Result<u64> {
        self.claim
            .bytes
            .checked_add(self.acknowledgement.bytes)
            .ok_or_else(|| invalid("Checkout retirement size overflow"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn historical_absolute_paths_validate_without_accessing_the_host_filesystem() {
        for path in ["/old/deleted", r"C:\old\deleted", r"\\server\share\deleted"] {
            assert!(portable_absolute(std::path::Path::new(path)), "{path}");
        }
        for path in ["relative/path", "C:relative", "\\single-root"] {
            assert!(!portable_absolute(std::path::Path::new(path)), "{path}");
        }
    }
}
