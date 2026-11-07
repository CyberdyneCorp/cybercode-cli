//! Exact checkout metadata pins. A binding alone never grants acknowledgement.
use super::{Managed, UseRecord, invalid};
use cap_fs_ext::{DirExt, FollowSymlinks, OpenOptionsFollowExt};
use cap_std::fs::{Dir, OpenOptions};
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
}

pub(super) struct Pins {
    common: Dir,
    root: Dir,
    directory: Dir,
    creations: Dir,
    creation: File,
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
        let creation = open_file(&creations, &format!("{name}.json"))?;
        Ok(Self {
            common,
            root,
            directory,
            creations,
            creation,
        })
    }
    pub(super) fn capture(
        managed: &Managed,
        owner: &str,
        file: &File,
    ) -> io::Result<(CheckoutBinding, Self)> {
        regular(file)?;
        let pins = Self::open(&managed.common_dir, &managed.id, &managed.name)?;
        let raw = bytes(&pins.creation)?;
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
            common_identity: directory_identity(&pins.common)?,
            root_identity: directory_identity(&pins.root)?,
            directory_identity: directory_identity(&pins.directory)?,
            file_identity: identity(file)?,
            creation_directory_identity: directory_identity(&pins.creations)?,
            creation_file_identity: identity(&pins.creation)?,
            creation_digest: digest(&raw),
        };
        Ok((binding, pins))
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
        let binding = record
            .snapshot_binding
            .as_ref()
            .ok_or_else(|| invalid("Missing checkout snapshot binding"))?;
        super::safe_id(&binding.owner_id, "op")?;
        super::safe_id(&binding.worktree_id, "wt")?;
        super::super::Name::parse(&binding.name).map_err(invalid)?;
        if binding.version != 1
            || binding.owner_id != record.owner_id
            || binding.worktree_id != record.worktree_id
        {
            return Err(invalid("Snapshot checkout owner changed"));
        }
        regular(file)?;
        if identity(file)? != binding.file_identity {
            return Err(invalid("Snapshot checkout descriptor changed"));
        }
        self.matches(binding)?;
        let current = Self::open(&binding.common_dir, &binding.worktree_id, &binding.name)?;
        current.matches(binding)?;
        if digest(&bytes(&current.creation)?) != binding.creation_digest {
            return Err(invalid("Snapshot checkout creation contents changed"));
        }
        let observed_file = open_file(&current.directory, &format!("{}.lock", binding.owner_id))?;
        let raw = contents(file, 4096)?;
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
