//! Exact checkout metadata pins. A binding alone never grants acknowledgement.
use super::{Managed, UseRecord, invalid};
pub(super) mod acknowledgement;
mod publication;
pub use acknowledgement::CheckoutAcknowledgement;
use cap_fs_ext::{DirExt, FollowSymlinks, OpenOptionsFollowExt};
use cap_std::fs::{Dir, OpenOptions};
pub(super) use publication::publish;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CheckoutBinding {
    pub version: u32,
    pub owner_id: String,
    pub worktree_id: String,
    pub name: String,
    pub common_dir: PathBuf,
    pub common_identity: String,
    pub root_identity: String,
    pub directory_identity: String,
    pub file_identity: String,
    pub creation_directory_identity: String,
    pub creation_file_identity: String,
    pub creation_digest: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lifetime: Option<CheckoutOwner>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub staging_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CheckoutOwner {
    pub activity_id: String,
    pub session_id: String,
    pub session_created_id: String,
    pub directory: PathBuf,
    pub lifetime_digest: String,
}

/// Read-only reacquired evidence, deliberately separate from `CheckoutLease`.
/// No generic settlement, creation or source mutation capability is exposed.
pub struct SnapshotClaim {
    lease: super::CheckoutLease,
    acknowledgement: Option<acknowledgement::Acknowledged>,
}
impl SnapshotClaim {
    pub fn binding(&self) -> &CheckoutBinding {
        self.lease
            .record
            .snapshot_binding
            .as_ref()
            .expect("reacquired binding")
    }
    pub fn verify(&self) -> io::Result<()> {
        self.lease.verify_snapshot()?;
        if let Some(acknowledged) = &self.acknowledgement {
            acknowledged.verify(
                self.lease.snapshot_pins.as_ref().expect("retained pins"),
                self.binding(),
            )?;
        }
        Ok(())
    }
}

pub(super) fn reacquire(
    managed: &Managed,
    binding: &CheckoutBinding,
    owner: &CheckoutOwner,
) -> io::Result<SnapshotClaim> {
    super::safe_id(&managed.id, "wt")?;
    super::super::Name::parse(&managed.name).map_err(invalid)?;
    owner.validate(managed)?;
    if binding.version != 2
        || binding.lifetime.as_ref() != Some(owner)
        || binding.owner_id != owner.activity_id
        || binding.worktree_id != managed.id
        || binding.name != managed.name
        || binding.common_dir != managed.common_dir
    {
        return Err(invalid(
            "Checkout reacquisition differs from its exact lifetime or creation",
        ));
    }
    let pins = Pins::open(&binding.common_dir, &binding.worktree_id, &binding.name)?;
    pins.matches(binding)?;
    let observed: Managed = serde_json::from_slice(&bytes(&pins.creation)?).map_err(invalid)?;
    if observed != *managed || !observed.ready {
        return Err(invalid(
            "Snapshot checkout creation differs from reacquisition",
        ));
    }
    let file = open_file(&pins.directory, &format!("{}.lock", binding.owner_id))?;
    if !super::try_lock(&file)? {
        return Err(invalid("Snapshot checkout worker is still active"));
    }
    let mut file = super::LockedRecord(file);
    let record = super::read_record(&mut file)?;
    if record.snapshot_binding.as_ref() != Some(binding) {
        return Err(invalid("Snapshot checkout binding changed"));
    }
    let lease = super::CheckoutLease {
        file,
        record,
        snapshot_pins: Some(pins),
    };
    lease.verify_snapshot()?;
    Ok(SnapshotClaim {
        lease,
        acknowledgement: None,
    })
}
impl CheckoutOwner {
    fn validate_fields(&self) -> io::Result<()> {
        super::safe_id(&self.activity_id, "op")?;
        super::safe_id(&self.session_id, "ses")?;
        if self.session_created_id.is_empty()
            || self.session_created_id.len() > 256
            || !self.directory.is_absolute()
            || self.directory.to_str().is_none()
            || self.lifetime_digest.len() != 64
            || !self
                .lifetime_digest
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(invalid("Invalid snapshot checkout lifetime context"));
        }
        Ok(())
    }
    pub(super) fn validate(&self, managed: &Managed) -> io::Result<()> {
        self.validate_fields()?;
        if self.directory.canonicalize()? != self.directory
            || !self.directory.starts_with(&managed.path)
        {
            return Err(invalid("Snapshot checkout lifetime Location changed"));
        }
        Ok(())
    }
}

pub(super) struct Pins {
    common: Dir,
    root: Dir,
    directory: Dir,
    creations: Dir,
    creation: File,
    archived: bool,
}

fn open_file(directory: &Dir, name: &str) -> io::Result<File> {
    let file = directory
        .open_with(
            name,
            OpenOptions::new().read(true).follow(FollowSymlinks::No),
        )?
        .into_std();
    regular(&file)?;
    Ok(file)
}

fn regular(file: &File) -> io::Result<()> {
    if !file.metadata()?.is_file() {
        return Err(invalid("Snapshot checkout record is not regular"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if file.metadata()?.nlink() != 1 {
            return Err(invalid("Snapshot checkout record has aliases"));
        }
    }
    #[cfg(windows)]
    windows::regular(file)?;
    Ok(())
}

#[cfg(unix)]
fn identity(file: &File) -> io::Result<String> {
    use std::os::unix::fs::MetadataExt;
    let metadata = file.metadata()?;
    let birth = metadata
        .created()?
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(invalid)?;
    Ok(format!(
        "{}:{}:{}:{}:{}:{}:{}",
        metadata.dev(),
        metadata.ino(),
        birth.as_secs(),
        birth.subsec_nanos(),
        metadata.uid(),
        metadata.gid(),
        metadata.mode()
    ))
}
#[cfg(windows)]
fn identity(file: &File) -> io::Result<String> {
    windows::identity(file)
}
#[cfg(not(any(unix, windows)))]
fn identity(_: &File) -> io::Result<String> {
    Err(invalid(
        "Native snapshot checkout identities are unavailable",
    ))
}

fn directory_identity(directory: &Dir) -> io::Result<String> {
    identity(&directory.try_clone()?.into_std_file())
}
fn bytes(file: &File) -> io::Result<Vec<u8>> {
    contents(file, 1024 * 1024)
}
fn contents(file: &File, limit: usize) -> io::Result<Vec<u8>> {
    let mut result = Vec::new();
    let mut chunk = [0; 4096];
    loop {
        let capacity = chunk.len().min(limit + 1 - result.len());
        #[cfg(unix)]
        let read =
            std::os::unix::fs::FileExt::read_at(file, &mut chunk[..capacity], result.len() as u64)?;
        #[cfg(windows)]
        let read = std::os::windows::fs::FileExt::seek_read(
            file,
            &mut chunk[..capacity],
            result.len() as u64,
        )?;
        #[cfg(not(any(unix, windows)))]
        let read = {
            let _ = (file, capacity);
            return Err(invalid("Native checkout reads are unavailable"));
        };
        if read == 0 {
            return Ok(result);
        }
        result.extend_from_slice(&chunk[..read]);
        if result.len() > limit {
            return Err(invalid("Checkout record exceeds limit"));
        }
    }
}
fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

impl Pins {
    fn open(common_dir: &Path, worktree: &str, name: &str) -> io::Result<Self> {
        Self::open_creation(common_dir, worktree, name, false)
    }
    fn open_for_removal(common_dir: &Path, worktree: &str, name: &str) -> io::Result<Self> {
        Self::open_creation(common_dir, worktree, name, true)
    }
    fn open_creation(
        common_dir: &Path,
        worktree: &str,
        name: &str,
        allow_archived: bool,
    ) -> io::Result<Self> {
        if std::fs::symlink_metadata(common_dir)?
            .file_type()
            .is_symlink()
        {
            return Err(invalid("Checkout common directory is a symlink"));
        }
        let common = Dir::open_ambient_dir(common_dir, cap_std::ambient_authority())?;
        let root = common.open_dir_nofollow("cyber-worktree-activity")?;
        let directory = root.open_dir_nofollow(worktree)?;
        let creations = common.open_dir_nofollow("cyber-worktrees")?;
        let (creation, archived) = match open_file(&creations, &format!("{name}.json")) {
            Ok(file) => (file, false),
            Err(error) if allow_archived && error.kind() == io::ErrorKind::NotFound => {
                (open_file(&creations, &format!("{worktree}.removed"))?, true)
            }
            Err(error) => return Err(error),
        };
        Ok(Self {
            common,
            root,
            directory,
            creations,
            creation,
            archived,
        })
    }
    pub(super) fn capture(
        &self,
        managed: &Managed,
        owner: &str,
        file: &File,
    ) -> io::Result<CheckoutBinding> {
        regular(file)?;
        let raw = bytes(&self.creation)?;
        let observed: Managed = serde_json::from_slice(&raw).map_err(invalid)?;
        if observed != *managed || !observed.ready {
            return Err(invalid("Snapshot checkout creation changed"));
        }
        let binding = CheckoutBinding {
            version: 1,
            owner_id: owner.into(),
            worktree_id: managed.id.clone(),
            name: managed.name.clone(),
            common_dir: managed.common_dir.clone(),
            common_identity: directory_identity(&self.common)?,
            root_identity: directory_identity(&self.root)?,
            directory_identity: directory_identity(&self.directory)?,
            file_identity: identity(file)?,
            creation_directory_identity: directory_identity(&self.creations)?,
            creation_file_identity: identity(&self.creation)?,
            creation_digest: digest(&raw),
            lifetime: None,
            staging_name: None,
        };
        Ok(binding)
    }
    fn matches(&self, binding: &CheckoutBinding) -> io::Result<()> {
        if directory_identity(&self.common)? != binding.common_identity
            || directory_identity(&self.root)? != binding.root_identity
            || directory_identity(&self.directory)? != binding.directory_identity
            || directory_identity(&self.creations)? != binding.creation_directory_identity
            || identity(&self.creation)? != binding.creation_file_identity
        {
            return Err(invalid(
                "Snapshot checkout namespace or creation identity changed",
            ));
        }
        Ok(())
    }
    pub(super) fn verify(&self, file: &File, record: &UseRecord) -> io::Result<()> {
        self.verify_at(file, record, &format!("{}.lock", record.owner_id))
    }
    fn verify_at(&self, file: &File, record: &UseRecord, name: &str) -> io::Result<()> {
        let binding = record
            .snapshot_binding
            .as_ref()
            .ok_or_else(|| invalid("Missing checkout snapshot binding"))?;
        super::safe_id(&binding.owner_id, "op")?;
        super::safe_id(&binding.worktree_id, "wt")?;
        super::super::Name::parse(&binding.name).map_err(invalid)?;
        if !matches!(
            (binding.version, binding.lifetime.as_ref()),
            (1, None) | (2, Some(_))
        ) || binding.owner_id != record.owner_id
            || binding.worktree_id != record.worktree_id
        {
            return Err(invalid("Snapshot checkout owner changed"));
        }
        if let Some(owner) = &binding.lifetime {
            owner.validate_fields()?;
            if owner.activity_id != record.owner_id {
                return Err(invalid("Snapshot checkout lifetime owner changed"));
            }
        }
        let prefix = format!("preparing_{}_{}_", binding.owner_id.len(), binding.owner_id);
        if binding.staging_name.as_ref().is_some_and(|stage| {
            !stage.starts_with(&prefix)
                || stage.len() != prefix.len() + 26
                || !stage
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"_-".contains(&byte))
        }) || binding.version == 2 && binding.staging_name.is_none()
        {
            return Err(invalid("Invalid snapshot checkout construction binding"));
        }
        regular(file)?;
        if identity(file)? != binding.file_identity {
            return Err(invalid("Snapshot checkout descriptor changed"));
        }
        self.matches(binding)?;
        let current = if self.archived {
            Self::open_for_removal(&binding.common_dir, &binding.worktree_id, &binding.name)?
        } else {
            Self::open(&binding.common_dir, &binding.worktree_id, &binding.name)?
        };
        current.matches(binding)?;
        if digest(&bytes(&current.creation)?) != binding.creation_digest {
            return Err(invalid("Snapshot checkout creation contents changed"));
        }
        let observed_file = open_file(&current.directory, name)?;
        let raw = contents(file, super::record_limit(record))?;
        let observed: UseRecord = serde_json::from_slice(&raw).map_err(invalid)?;
        let exact: serde_json::Value = serde_json::from_slice(&raw).map_err(invalid)?;
        if identity(&observed_file)? != binding.file_identity
            || observed != *record
            || exact != serde_json::to_value(record).map_err(invalid)?
        {
            return Err(invalid("Snapshot checkout route or record changed"));
        }
        Ok(())
    }
}

#[cfg(windows)]
#[allow(unsafe_code)] // Queries only live owned handles into fixed-size Windows structures.
mod windows {
    use super::*;
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, FILE_ATTRIBUTE_REPARSE_POINT, FILE_ID_INFO, FileIdInfo,
        GetFileInformationByHandle, GetFileInformationByHandleEx,
    };
    fn info(file: &File) -> io::Result<BY_HANDLE_FILE_INFORMATION> {
        let mut value = BY_HANDLE_FILE_INFORMATION::default();
        if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut value) } == 0 {
            return Err(io::Error::last_os_error());
        }
        if value.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err(invalid("Snapshot checkout reparse point"));
        }
        Ok(value)
    }
    pub(super) fn regular(file: &File) -> io::Result<()> {
        if info(file)?.nNumberOfLinks != 1 {
            return Err(invalid("Snapshot checkout record has aliases"));
        }
        Ok(())
    }
    pub(super) fn identity(file: &File) -> io::Result<String> {
        let info = info(file)?;
        let mut id = FILE_ID_INFO::default();
        if unsafe {
            GetFileInformationByHandleEx(
                file.as_raw_handle(),
                FileIdInfo,
                (&mut id as *mut FILE_ID_INFO).cast(),
                std::mem::size_of::<FILE_ID_INFO>() as u32,
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(format!(
            "{}:{:032x}:{}:{}:{}",
            id.VolumeSerialNumber,
            u128::from_le_bytes(id.FileId.Identifier),
            info.ftCreationTime.dwHighDateTime,
            info.ftCreationTime.dwLowDateTime,
            info.dwFileAttributes
        ))
    }
}
