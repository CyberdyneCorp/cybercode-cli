//! Object-identity leases for recursive-root preparation without open child handles.

use super::*;
use std::os::windows::io::FromRawHandle;
use windows_sys::Wdk::Storage::FileSystem::{NtQuerySecurityObject, NtSetSecurityObject};
use windows_sys::Win32::Foundation::{INVALID_HANDLE_VALUE, RtlNtStatusToDosError};
use windows_sys::Win32::Security::{
    GetSecurityDescriptorControl, GetSecurityDescriptorDacl, InitializeSecurityDescriptor,
    SE_DACL_AUTO_INHERITED, SE_DACL_DEFAULTED, SE_DACL_PROTECTED, SECURITY_DESCRIPTOR,
    SetSecurityDescriptorControl, SetSecurityDescriptorDacl,
};
use windows_sys::Win32::Storage::FileSystem::{
    BY_HANDLE_FILE_INFORMATION, ExtendedFileIdType, FILE_ID_128, FILE_ID_DESCRIPTOR,
    FILE_ID_DESCRIPTOR_0, FILE_ID_INFO, FILE_SHARE_DELETE, FILE_SHARE_WRITE, FileIdInfo,
    FileIdType, GetFileInformationByHandle, GetFileInformationByHandleEx, OpenFileById,
};
use windows_sys::Win32::System::SystemServices::SECURITY_DESCRIPTOR_REVISION;

const METADATA_ACCESS: u32 = READ_CONTROL | WRITE_DAC | FILE_READ_ATTRIBUTES;

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

    pub(super) fn verify(&self) -> io::Result<()> {
        validate_object(&self.record.open()?).map(|_| ())
    }

    pub(super) fn revoke(&mut self) -> io::Result<()> {
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
    let file = open_object(path)?;
    let directory = validate_object(&file)?;
    if inheritance != 0 && !directory {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Inheritable identity grants require a directory",
        ));
    }
    let record = FileRecord::capture(path, &file)?;
    grant_record(profile, record, access, inheritance)
}

pub(super) fn grant_record(
    profile: &Profile,
    record: FileRecord,
    access: Access,
    inheritance: u32,
) -> io::Result<IdentityGrant> {
    let file = record.open()?;
    let directory = validate_object(&file)?;
    if inheritance != 0 && !directory {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Inheritable identity grants require a directory",
        ));
    }
    update_acl_using(
        &file,
        profile.sid(),
        Some(access.mask()),
        inheritance,
        set_acl,
        read_acl,
    )?;
    Ok(IdentityGrant {
        record,
        profile: profile.clone(),
        active: true,
    })
}

pub(super) fn open_object(path: &Path) -> io::Result<File> {
    OpenOptions::new()
        .access_mode(METADATA_ACCESS)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
}

pub(super) fn validate_object(file: &File) -> io::Result<bool> {
    let metadata = file.metadata()?;
    if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Identity preparation refuses reparse points",
        ));
    }
    if !metadata.is_dir() && (!metadata.is_file() || link_count(file)? != 1) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Identity preparation requires ordinary singly linked files",
        ));
    }
    Ok(metadata.is_dir())
}

pub(super) fn read_acl(file: &File) -> io::Result<(SecurityAllocation, *mut ACL)> {
    let mut needed = 0;
    let status = unsafe {
        NtQuerySecurityObject(
            file.as_raw_handle(),
            DACL_SECURITY_INFORMATION,
            null_mut(),
            0,
            &mut needed,
        )
    };
    if needed == 0 {
        nt_result(status)?;
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Empty security descriptor",
        ));
    }
    if needed > 1024 * 1024 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Security descriptor exceeds limit",
        ));
    }
    let mut storage = vec![0u32; (needed as usize).div_ceil(4)];
    let descriptor = storage.as_mut_ptr().cast();
    nt_result(unsafe {
        NtQuerySecurityObject(
            file.as_raw_handle(),
            DACL_SECURITY_INFORMATION,
            descriptor,
            needed,
            &mut needed,
        )
    })?;
    let mut present = 0;
    let mut defaulted = 0;
    let mut acl = null_mut();
    checked(unsafe {
        GetSecurityDescriptorDacl(descriptor, &mut present, &mut acl, &mut defaulted)
    })?;
    Ok((SecurityAllocation::Native(storage), acl))
}

pub(super) fn set_acl(
    file: &File,
    original: *mut core::ffi::c_void,
    acl: *mut ACL,
) -> io::Result<()> {
    let mut control = 0;
    let mut revision = 0;
    checked(unsafe { GetSecurityDescriptorControl(original, &mut control, &mut revision) })?;
    let mut descriptor = SECURITY_DESCRIPTOR::default();
    let raw = (&mut descriptor as *mut SECURITY_DESCRIPTOR).cast();
    checked(unsafe { InitializeSecurityDescriptor(raw, SECURITY_DESCRIPTOR_REVISION) })?;
    checked(unsafe {
        SetSecurityDescriptorDacl(raw, 1, acl, i32::from(control & SE_DACL_DEFAULTED != 0))
    })?;
    let preserve = SE_DACL_AUTO_INHERITED | SE_DACL_PROTECTED;
    checked(unsafe { SetSecurityDescriptorControl(raw, preserve, control & preserve) })?;
    // NtSetSecurityObject addresses this verified handle only. Preserve the
    // descriptor's inheritance/protection state without traversing its children.
    let status =
        unsafe { NtSetSecurityObject(file.as_raw_handle(), DACL_SECURITY_INFORMATION, raw) };
    if status < 0 {
        return Err(io::Error::from_raw_os_error(
            unsafe { RtlNtStatusToDosError(status) } as i32,
        ));
    }
    Ok(())
}

fn nt_result(status: i32) -> io::Result<()> {
    if status < 0 {
        Err(io::Error::from_raw_os_error(
            unsafe { RtlNtStatusToDosError(status) } as i32,
        ))
    } else {
        Ok(())
    }
}

fn checked(result: i32) -> io::Result<()> {
    if result == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
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
        match self.open_raw() {
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
                drop(self.open_raw()?);
            }
            Err(error) => return Err(error),
        }
        Ok(())
    }

    pub(super) fn open(&self) -> io::Result<File> {
        match self.open_raw() {
            Err(error) if error.raw_os_error() == Some(87) => {
                // The descriptor form was successfully verified during capture.
                // NTFS reports a stale/deleted file reference as invalid parameter.
                if information(&self.volume)?.VolumeSerialNumber != self.serial {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "Recorded volume identity changed",
                    ));
                }
                Err(io::Error::new(
                    io::ErrorKind::NotFound,
                    "Verified Windows file identity no longer exists",
                ))
            }
            result => result,
        }
    }

    fn open_raw(&self) -> io::Result<File> {
        let raw = unsafe {
            OpenFileById(
                self.volume.as_raw_handle(),
                &self.descriptor,
                METADATA_ACCESS,
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
