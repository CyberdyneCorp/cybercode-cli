//! Handle-bound native evidence. Verification never changes ACLs or grants mutation authority.
#[path = "windows/creation.rs"]
mod creation;
use super::MemoryStorageError;
pub use creation::{create_private_directory, create_private_file};
use std::fs::File;
use std::io;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::ptr::null_mut;
use windows_sys::Win32::Foundation::{ERROR_INSUFFICIENT_BUFFER, ERROR_SUCCESS, LocalFree};
use windows_sys::Win32::Security::Authorization::{GetSecurityInfo, SE_FILE_OBJECT};
use windows_sys::Win32::Security::{
    ACCESS_ALLOWED_ACE, ACCESS_DENIED_ACE, ACE_HEADER, ACL, DACL_SECURITY_INFORMATION, EqualSid,
    GetAce, GetSecurityDescriptorControl, GetTokenInformation, IsValidAcl,
    IsValidSecurityDescriptor, IsValidSid, IsWellKnownSid, OWNER_SECURITY_INFORMATION, PSID,
    SE_DACL_PROTECTED, TOKEN_QUERY, TOKEN_USER, TokenUser, WinLocalSystemSid,
};
use windows_sys::Win32::Storage::FileSystem::{
    BY_HANDLE_FILE_INFORMATION, FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_REPARSE_POINT,
    FILE_ID_INFO, FileIdInfo, GetFileInformationByHandle, GetFileInformationByHandleEx,
};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

/// Full volume/file identity, including the 128-bit identifier used by ReFS.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct FileIdentity {
    pub volume: u64,
    pub file: [u8; 16],
}

fn refusal(reason: &'static str) -> MemoryStorageError {
    MemoryStorageError::Unsafe(reason)
}
fn information(file: &File) -> Result<BY_HANDLE_FILE_INFORMATION, MemoryStorageError> {
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    // The retained File owns a live handle; Windows writes exactly this structure.
    if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) } == 0 {
        return Err(io::Error::last_os_error().into());
    }
    if info.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(refusal("memory reparse points are not allowed"));
    }
    Ok(info)
}
/// Refuse directory/device handles, every reparse type and multiple file links.
pub fn verify_regular(file: &File) -> Result<(), MemoryStorageError> {
    let info = information(file)?;
    if !file.metadata()?.is_file() || info.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0 {
        return Err(refusal("expected a regular memory file"));
    }
    if info.nNumberOfLinks != 1 {
        return Err(refusal("hard-linked memory file"));
    }
    Ok(())
}
/// Query identity without reopening by path or falling back to truncated identifiers.
pub fn identity(file: &File) -> Result<FileIdentity, MemoryStorageError> {
    let info = information(file)?;
    if info.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY == 0 {
        verify_regular(file)?;
    }
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
        return Err(io::Error::last_os_error().into());
    }
    Ok(FileIdentity {
        volume: id.VolumeSerialNumber,
        file: id.FileId.Identifier,
    })
}

/// Require the process user as owner and a protected DACL granting only that user or SYSTEM.
/// Unsafe existing evidence is refused, never repaired. Native creation/durability remain separate.
pub fn verify_private(file: &File) -> Result<FileIdentity, MemoryStorageError> {
    let identity = identity(file)?;
    let user = process_user()?;
    let descriptor = Descriptor::read(file)?;
    descriptor.verify_owner(user.as_ptr().cast_mut().cast())?;
    descriptor.verify_acl(user.as_ptr().cast_mut().cast())?;
    Ok(identity)
}

struct Descriptor {
    allocation: *mut std::ffi::c_void,
    owner: PSID,
    acl: *mut ACL,
}
impl Drop for Descriptor {
    fn drop(&mut self) {
        unsafe { LocalFree(self.allocation) };
    }
}
impl Descriptor {
    fn verify_owner(&self, user: PSID) -> Result<(), MemoryStorageError> {
        if self.owner.is_null()
            || unsafe { IsValidSid(self.owner) } == 0
            || unsafe { EqualSid(self.owner, user) } == 0
        {
            return Err(refusal("memory object is not owned by the current user"));
        }
        Ok(())
    }
    fn read(file: &File) -> Result<Self, MemoryStorageError> {
        let mut descriptor = Self {
            allocation: null_mut(),
            owner: null_mut(),
            acl: null_mut(),
        };
        // GetSecurityInfo owns one allocation; owner/ACL pointers borrow it until Drop.
        let result = unsafe {
            GetSecurityInfo(
                file.as_raw_handle(),
                SE_FILE_OBJECT,
                OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
                &mut descriptor.owner,
                null_mut(),
                &mut descriptor.acl,
                null_mut(),
                &mut descriptor.allocation,
            )
        };
        if result != ERROR_SUCCESS {
            return Err(io::Error::from_raw_os_error(result as i32).into());
        }
        if descriptor.allocation.is_null()
            || unsafe { IsValidSecurityDescriptor(descriptor.allocation) } == 0
        {
            return Err(refusal("invalid memory security descriptor"));
        }
        Ok(descriptor)
    }
    fn verify_acl(&self, user: PSID) -> Result<(), MemoryStorageError> {
        let mut control = 0u16;
        let mut revision = 0u32;
        if unsafe { GetSecurityDescriptorControl(self.allocation, &mut control, &mut revision) }
            == 0
        {
            return Err(io::Error::last_os_error().into());
        }
        if control & SE_DACL_PROTECTED == 0
            || self.acl.is_null()
            || unsafe { IsValidAcl(self.acl) } == 0
        {
            return Err(refusal("memory requires a protected non-null DACL"));
        }
        let count = unsafe { (*self.acl).AceCount };
        if count > 4096 {
            return Err(refusal("memory ACL exceeds its bound"));
        }
        for index in 0..count {
            self.verify_ace(index, user)?;
        }
        Ok(())
    }
    fn verify_ace(&self, index: u16, user: PSID) -> Result<(), MemoryStorageError> {
        let mut raw = null_mut();
        if unsafe { GetAce(self.acl, u32::from(index), &mut raw) } == 0 {
            return Err(io::Error::last_os_error().into());
        }
        let header = unsafe { &*raw.cast::<ACE_HEADER>() };
        let offset = match header.AceType {
            // Standard ACCESS_ALLOWED_ACE_TYPE and ACCESS_DENIED_ACE_TYPE.
            0 => std::mem::offset_of!(ACCESS_ALLOWED_ACE, SidStart),
            1 => std::mem::offset_of!(ACCESS_DENIED_ACE, SidStart),
            _ => return Err(refusal("unsupported memory ACL entry")),
        };
        let bytes =
            unsafe { std::slice::from_raw_parts(raw.cast::<u8>(), usize::from(header.AceSize)) };
        let sid = bounded_sid(
            bytes
                .get(offset..)
                .ok_or_else(|| refusal("truncated memory ACL entry"))?,
        )?;
        if header.AceType == 0
            && unsafe { EqualSid(sid, user) } == 0
            && unsafe { IsWellKnownSid(sid, WinLocalSystemSid) } == 0
        {
            return Err(refusal("memory ACL grants another principal access"));
        }
        Ok(())
    }
}
fn bounded_sid(bytes: &[u8]) -> Result<PSID, MemoryStorageError> {
    if bytes.len() < 8 || bytes[1] > 15 || 8 + 4 * usize::from(bytes[1]) > bytes.len() {
        return Err(refusal("invalid memory SID bounds"));
    }
    let sid = bytes.as_ptr().cast_mut().cast();
    if unsafe { IsValidSid(sid) } == 0 {
        return Err(refusal("invalid memory SID"));
    }
    Ok(sid)
}
fn process_user() -> Result<Vec<u32>, MemoryStorageError> {
    let mut token = null_mut();
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
        return Err(io::Error::last_os_error().into());
    }
    let token = unsafe { OwnedHandle::from_raw_handle(token) };
    let mut length = 0;
    let result = unsafe {
        GetTokenInformation(token.as_raw_handle(), TokenUser, null_mut(), 0, &mut length)
    };
    if result != 0
        || io::Error::last_os_error().raw_os_error() != Some(ERROR_INSUFFICIENT_BUFFER as i32)
        || length as usize > 1024 * 1024
        || (length as usize) < std::mem::size_of::<TOKEN_USER>()
    {
        return Err(refusal("invalid current-user token bounds"));
    }
    let mut buffer = vec![0usize; (length as usize).div_ceil(std::mem::size_of::<usize>())];
    let capacity = length;
    if unsafe {
        GetTokenInformation(
            token.as_raw_handle(),
            TokenUser,
            buffer.as_mut_ptr().cast(),
            capacity,
            &mut length,
        )
    } == 0
    {
        return Err(io::Error::last_os_error().into());
    }
    copy_user(&buffer, length as usize)
}
fn copy_user(buffer: &[usize], length: usize) -> Result<Vec<u32>, MemoryStorageError> {
    let capacity = std::mem::size_of_val(buffer);
    if length > capacity || length < std::mem::size_of::<TOKEN_USER>() {
        return Err(refusal("truncated current-user token"));
    }
    let user = unsafe { &*buffer.as_ptr().cast::<TOKEN_USER>() };
    let offset = (user.User.Sid as usize)
        .checked_sub(buffer.as_ptr() as usize)
        .filter(|offset| *offset <= length)
        .ok_or_else(|| refusal("foreign current-user SID pointer"))?;
    let bytes = unsafe { std::slice::from_raw_parts(buffer.as_ptr().cast::<u8>(), length) };
    let sid_bytes = &bytes[offset..];
    bounded_sid(sid_bytes)?;
    let length = 8 + 4 * usize::from(sid_bytes[1]);
    let mut sid = vec![0u32; length.div_ceil(4)];
    // The verified SID is DWORD-aligned and bounded by its original token allocation.
    unsafe {
        std::ptr::copy_nonoverlapping(sid_bytes.as_ptr(), sid.as_mut_ptr().cast::<u8>(), length)
    };
    Ok(sid)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Security::Authorization::{
        ConvertSecurityDescriptorToStringSecurityDescriptorW, ConvertSidToStringSidW,
        ConvertStringSecurityDescriptorToSecurityDescriptorW, SetSecurityInfo,
    };
    use windows_sys::Win32::Security::{
        GetSecurityDescriptorDacl, GetSecurityDescriptorOwner, PROTECTED_DACL_SECURITY_INFORMATION,
    };
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_ALL_ACCESS, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
    };

    fn file(path: &std::path::Path) -> File {
        std::fs::OpenOptions::new()
            .write(true)
            .access_mode(FILE_ALL_ACCESS)
            .create_new(true)
            .open(path)
            .unwrap()
    }
    fn user_string() -> String {
        let user = process_user().unwrap();
        let mut raw = null_mut();
        assert_ne!(
            unsafe { ConvertSidToStringSidW(user.as_ptr().cast_mut().cast(), &mut raw) },
            0
        );
        let mut length = 0;
        while unsafe { *raw.add(length) } != 0 {
            length += 1;
            assert!(length < 256);
        }
        let text = String::from_utf16(unsafe { std::slice::from_raw_parts(raw, length) }).unwrap();
        unsafe { LocalFree(raw.cast()) };
        text
    }
    fn set_acl(file: &File, extra: &str, protected: bool) {
        let user = user_string();
        let text = format!(
            "O:{user}D:{}(A;OICI;FA;;;{user})(A;OICI;FA;;;SY){extra}",
            if protected { "P" } else { "" }
        );
        let text: Vec<u16> = text.encode_utf16().chain(Some(0)).collect();
        let mut raw = null_mut();
        assert_ne!(
            unsafe {
                ConvertStringSecurityDescriptorToSecurityDescriptorW(
                    text.as_ptr(),
                    1,
                    &mut raw,
                    null_mut(),
                )
            },
            0
        );
        let allocation = Descriptor {
            allocation: raw,
            owner: null_mut(),
            acl: null_mut(),
        };
        let mut owner = null_mut();
        let mut defaulted = 0;
        let mut present = 0;
        let mut acl = null_mut();
        assert_ne!(
            unsafe { GetSecurityDescriptorOwner(raw, &mut owner, &mut defaulted) },
            0
        );
        assert_ne!(
            unsafe { GetSecurityDescriptorDacl(raw, &mut present, &mut acl, &mut defaulted) },
            0
        );
        let protection = if protected {
            PROTECTED_DACL_SECURITY_INFORMATION
        } else {
            windows_sys::Win32::Security::UNPROTECTED_DACL_SECURITY_INFORMATION
        };
        assert_eq!(
            unsafe {
                SetSecurityInfo(
                    file.as_raw_handle(),
                    SE_FILE_OBJECT,
                    OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION | protection,
                    owner,
                    null_mut(),
                    acl,
                    null_mut(),
                )
            },
            ERROR_SUCCESS
        );
        drop(allocation);
    }
    fn security(file: &File) -> String {
        let descriptor = Descriptor::read(file).unwrap();
        let mut raw = null_mut();
        let mut length = 0;
        assert_ne!(
            unsafe {
                ConvertSecurityDescriptorToStringSecurityDescriptorW(
                    descriptor.allocation,
                    1,
                    OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
                    &mut raw,
                    &mut length,
                )
            },
            0
        );
        assert!((1..=1024 * 1024).contains(&length));
        let text = String::from_utf16(unsafe { std::slice::from_raw_parts(raw, length as usize) })
            .unwrap();
        unsafe { LocalFree(raw.cast()) };
        text
    }
    #[test]
    fn native_private_acl_accepts_user_and_system_without_repair() {
        let root = tempfile::tempdir().unwrap();
        let file = file(&root.path().join("private"));
        set_acl(&file, "", true);
        let before = security(&file);
        assert_eq!(verify_private(&file).unwrap(), identity(&file).unwrap());
        assert_eq!(security(&file), before);
    }
    #[test]
    fn native_private_directory_acl_is_checked_without_regular_file_admission() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("private");
        std::fs::create_dir(&directory).unwrap();
        let file = std::fs::OpenOptions::new()
            .access_mode(FILE_ALL_ACCESS)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
            .open(&directory)
            .unwrap();
        set_acl(&file, "", true);
        let before = security(&file);
        assert_eq!(verify_private(&file).unwrap(), identity(&file).unwrap());
        assert!(verify_regular(&file).is_err());
        assert_eq!(security(&file), before);
    }
    #[test]
    fn native_sid_and_token_bounds_refuse_before_pointer_reads() {
        for bytes in [
            vec![],
            vec![0; 7],
            vec![1, 16, 0, 0, 0, 0, 0, 0],
            vec![1, 2, 0, 0, 0, 0, 0, 0],
        ] {
            assert!(bounded_sid(&bytes).is_err());
        }
        let buffer =
            vec![0usize; std::mem::size_of::<TOKEN_USER>().div_ceil(std::mem::size_of::<usize>())];
        assert!(copy_user(&buffer, std::mem::size_of_val(buffer.as_slice()) + 1).is_err());
        assert!(copy_user(&buffer, std::mem::size_of::<TOKEN_USER>() - 1).is_err());
        assert!(copy_user(&buffer, std::mem::size_of::<TOKEN_USER>()).is_err());
    }
    #[test]
    fn native_foreign_owner_is_refused_even_with_a_user_only_acl() {
        let user = process_user().unwrap();
        let text = format!("O:BUD:P(A;;FA;;;{})", user_string());
        let text: Vec<u16> = text.encode_utf16().chain(Some(0)).collect();
        let mut raw = null_mut();
        assert_ne!(
            unsafe {
                ConvertStringSecurityDescriptorToSecurityDescriptorW(
                    text.as_ptr(),
                    1,
                    &mut raw,
                    null_mut(),
                )
            },
            0
        );
        let mut descriptor = Descriptor {
            allocation: raw,
            owner: null_mut(),
            acl: null_mut(),
        };
        let mut defaulted = 0;
        assert_ne!(
            unsafe { GetSecurityDescriptorOwner(raw, &mut descriptor.owner, &mut defaulted) },
            0
        );
        assert!(
            descriptor
                .verify_owner(user.as_ptr().cast_mut().cast())
                .is_err()
        );
    }
    #[test]
    fn native_acl_refuses_broad_unprotected_and_null_acl_without_repair() {
        let root = tempfile::tempdir().unwrap();
        let file = file(&root.path().join("unsafe"));
        for (extra, protected) in [("(A;;FR;;;WD)", true), ("(A;;FA;;;BA)", true), ("", false)] {
            set_acl(&file, extra, protected);
            let before = security(&file);
            assert!(verify_private(&file).is_err());
            assert_eq!(security(&file), before);
        }
        assert_eq!(
            unsafe {
                SetSecurityInfo(
                    file.as_raw_handle(),
                    SE_FILE_OBJECT,
                    DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                    null_mut(),
                    null_mut(),
                    null_mut(),
                    null_mut(),
                )
            },
            ERROR_SUCCESS
        );
        let before = security(&file);
        assert!(verify_private(&file).is_err());
        assert_eq!(security(&file), before);
    }
    #[test]
    fn native_identity_distinguishes_replacement_and_refuses_hard_links() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("original");
        let original = file(&path);
        let id = identity(&original).unwrap();
        std::fs::rename(&path, root.path().join("moved")).unwrap();
        let replacement = file(&path);
        assert_ne!(identity(&replacement).unwrap(), id);
        assert_eq!(identity(&original).unwrap(), id);
        std::fs::hard_link(&path, root.path().join("alias")).unwrap();
        assert!(verify_regular(&replacement).is_err());
        assert!(identity(&replacement).is_err());
    }
    #[test]
    fn native_junction_handles_are_refused_before_security_or_identity_admission() {
        let root = tempfile::tempdir().unwrap();
        let target = root.path().join("target");
        std::fs::create_dir(&target).unwrap();
        let junction = root.path().join("junction");
        assert!(
            std::process::Command::new("cmd")
                .args(["/C", "mklink", "/J"])
                .arg(&junction)
                .arg(&target)
                .output()
                .unwrap()
                .status
                .success()
        );
        let file = std::fs::OpenOptions::new()
            .access_mode(FILE_ALL_ACCESS)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
            .open(&junction)
            .unwrap();
        assert!(identity(&file).is_err());
        assert!(verify_private(&file).is_err());
    }
}
