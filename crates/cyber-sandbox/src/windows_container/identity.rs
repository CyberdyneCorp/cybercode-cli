//! Object-identity leases for recursive-root preparation without open child handles.

use super::*;
use std::os::windows::io::FromRawHandle;
use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
use windows_sys::Win32::Storage::FileSystem::{
    BY_HANDLE_FILE_INFORMATION, ExtendedFileIdType, FILE_ID_128, FILE_ID_DESCRIPTOR,
    FILE_ID_DESCRIPTOR_0, FILE_ID_INFO, FILE_SHARE_DELETE, FILE_SHARE_WRITE, FileIdInfo,
    FileIdType, GetFileInformationByHandle, GetFileInformationByHandleEx, OpenFileById,
};
use windows_sys::Win32::System::SystemServices::MAXIMUM_ALLOWED;

/// A direct-object grant reopened by verified file ID during cleanup.
/// It owns no child handle and grants no recursive tree by itself.
pub struct IdentityGrant {
    record: FileRecord,
    profile: Profile,
    active: bool,
}

impl IdentityGrant {
    pub fn close(mut self) -> io::Result<()> {
        self.revoke()
    }

    fn revoke(&mut self) -> io::Result<()> {
        if !self.active {
            return Ok(());
        }
        match self.record.open() {
            Ok(file) => remove_profile_entries(&file, self.profile.sid())?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        self.active = false;
        Ok(())
    }
}

impl Drop for IdentityGrant {
    fn drop(&mut self) {
        if let Err(error) = self.revoke() {
            cleanup_error("identity ACL revocation", error);
        }
    }
}

pub(super) fn grant(
    profile: &Profile,
    path: &Path,
    access: Access,
    inheritance: u32,
) -> io::Result<IdentityGrant> {
    let _ancestors = retain_ancestors(path)?;
    // MAXIMUM_ALLOWED suppresses SetSecurityInfo's automatic child traversal.
    // Every recursive object must be prepared and cleaned explicitly.
    let file = OpenOptions::new()
        .access_mode(MAXIMUM_ALLOWED)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)?;
    let metadata = file.metadata()?;
    if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Identity leases refuse reparse points",
        ));
    }
    if !metadata.is_dir() && link_count(&file)? != 1 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Identity grants refuse multiply linked files",
        ));
    }
    if inheritance != 0 && !metadata.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Inheritable identity grants require a directory",
        ));
    }
    let record = FileRecord::capture(path, &file)?;
    update_acl(&file, profile.sid(), Some(access.mask()), inheritance)?;
    Ok(IdentityGrant {
        record,
        profile: profile.clone(),
        active: true,
    })
}

pub(super) struct FileRecord {
    volume: File,
    serial: u64,
    id: [u8; 16],
    descriptor: FILE_ID_DESCRIPTOR,
}

impl FileRecord {
    pub(super) fn capture(path: &Path, file: &File) -> io::Result<Self> {
        let root: PathBuf = path.components().take(2).collect();
        // A volume-root hint does not pin any movable child directory.
        let volume = OpenOptions::new()
            .access_mode(FILE_READ_ATTRIBUTES)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
            .open(root)?;
        let info = information(file)?;
        if information(&volume)?.VolumeSerialNumber != info.VolumeSerialNumber {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Object and volume hint disagree",
            ));
        }
        let mut record = Self {
            volume,
            serial: info.VolumeSerialNumber,
            id: info.FileId.Identifier,
            descriptor: descriptor(info.FileId),
        };
        record.verify_reopening(file)?;
        Ok(record)
    }

    fn verify_reopening(&mut self, original: &File) -> io::Result<()> {
        match self.open() {
            Ok(file) => drop(file),
            // NTFS may require its classic 64-bit ID rather than the ReFS form.
            // This alternative must still match the full captured identity.
            Err(error) if error.raw_os_error() == Some(87) => {
                let info = standard_information(original)?;
                self.descriptor.Type = FileIdType;
                self.descriptor.Anonymous = FILE_ID_DESCRIPTOR_0 {
                    FileId: ((u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow))
                        as i64,
                };
                drop(self.open()?);
            }
            Err(error) => return Err(error),
        }
        Ok(())
    }

    pub(super) fn open(&self) -> io::Result<File> {
        let raw = unsafe {
            OpenFileById(
                self.volume.as_raw_handle(),
                &self.descriptor,
                MAXIMUM_ALLOWED,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                null(),
                FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
            )
        };
        if raw == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }
        // The API transfers this handle; every later error closes it through File.
        let file = unsafe { File::from_raw_handle(raw) };
        let info = information(&file)?;
        if info.VolumeSerialNumber != self.serial || info.FileId.Identifier != self.id {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Reopened object identity changed",
            ));
        }
        Ok(file)
    }
}

fn descriptor(id: FILE_ID_128) -> FILE_ID_DESCRIPTOR {
    FILE_ID_DESCRIPTOR {
        dwSize: std::mem::size_of::<FILE_ID_DESCRIPTOR>() as u32,
        Type: ExtendedFileIdType,
        Anonymous: FILE_ID_DESCRIPTOR_0 { ExtendedFileId: id },
    }
}

fn information(file: &File) -> io::Result<FILE_ID_INFO> {
    let mut info = FILE_ID_INFO::default();
    if unsafe {
        GetFileInformationByHandleEx(
            file.as_raw_handle(),
            FileIdInfo,
            (&mut info as *mut FILE_ID_INFO).cast(),
            std::mem::size_of::<FILE_ID_INFO>() as u32,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(info)
}

fn link_count(file: &File) -> io::Result<u32> {
    Ok(standard_information(file)?.nNumberOfLinks)
}

fn standard_information(file: &File) -> io::Result<BY_HANDLE_FILE_INFORMATION> {
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(info)
}
