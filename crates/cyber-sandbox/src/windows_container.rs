//! Invocation-owned AppContainer profiles and direct-object ACL leases.
//! This foundation does not launch or enable the Windows sandbox.
#![allow(unsafe_code)]

use std::fs::{File, OpenOptions};
use std::io;
use std::os::windows::ffi::OsStringExt;
use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
use std::os::windows::io::AsRawHandle;
use std::path::{Path, PathBuf};
use std::ptr::{null, null_mut};
use std::sync::{Arc, Mutex};

use windows_sys::Win32::Foundation::LocalFree;
use windows_sys::Win32::Security::Authorization::{
    ConvertSidToStringSidW, EXPLICIT_ACCESS_W, GRANT_ACCESS, GetSecurityInfo, REVOKE_ACCESS,
    SE_FILE_OBJECT, SetEntriesInAclW, SetSecurityInfo, TRUSTEE_IS_SID, TRUSTEE_W,
};
use windows_sys::Win32::Security::Isolation::{
    CreateAppContainerProfile, DeleteAppContainerProfile, GetAppContainerFolderPath,
};
use windows_sys::Win32::Security::{
    ACCESS_ALLOWED_ACE, ACE_HEADER, ACL, CopySid, DACL_SECURITY_INFORMATION, EqualSid, FreeSid,
    GetAce, GetLengthSid, PSID,
};
use windows_sys::Win32::Storage::FileSystem::{
    DELETE, FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
    FILE_GENERIC_EXECUTE, FILE_GENERIC_READ, FILE_GENERIC_WRITE, READ_CONTROL, WRITE_DAC,
};
use windows_sys::Win32::System::Com::CoTaskMemFree;

// Serialize our own read/merge/write operations on shared objects.
static ACL_UPDATE: Mutex<()> = Mutex::new(());

#[derive(Clone)]
pub struct Profile(Arc<ProfileInner>);

struct ProfileInner {
    name: String,
    sid: Vec<u32>,
    active: bool,
}

impl Profile {
    /// Create a fresh identity; an existing profile is never reused.
    pub fn new() -> io::Result<Self> {
        Self::named(format!(
            "cyberdyne.cyber.{}",
            cyber_core::ids::new_id("box")
        ))
    }

    fn named(name: String) -> io::Result<Self> {
        let encoded = wide(&name);
        let mut raw = null_mut();
        // All strings and the SID output remain valid throughout the call.
        let status = unsafe {
            CreateAppContainerProfile(
                encoded.as_ptr(),
                encoded.as_ptr(),
                encoded.as_ptr(),
                null(),
                0,
                &mut raw,
            )
        };
        let sid = AllocatedSid(raw);
        hresult(status)?;
        let mut inner = ProfileInner {
            name,
            sid: Vec::new(),
            active: true,
        };
        inner.sid = sid.copy()?;
        Ok(Self(Arc::new(inner)))
    }

    pub fn name(&self) -> &str {
        &self.0.name
    }

    /// Ask Windows for the storage belonging to this exact profile identity.
    pub fn storage_path(&self) -> io::Result<PathBuf> {
        self.ensure_active()?;
        let mut sid = null_mut();
        let status = unsafe { ConvertSidToStringSidW(self.sid(), &mut sid) };
        let _sid = LocalAllocation(sid.cast());
        if status == 0 {
            return Err(io::Error::last_os_error());
        }
        let mut path = null_mut();
        let status = unsafe { GetAppContainerFolderPath(sid, &mut path) };
        let allocation = TaskAllocation(path);
        hresult(status)?;
        allocation.path()
    }

    /// Refuse deletion while clones or ACL leases still own the identity.
    pub fn close(&mut self) -> io::Result<()> {
        Arc::get_mut(&mut self.0)
            .ok_or_else(|| io::Error::other("AppContainer profile still has owners"))?
            .close()
    }

    /// Grant only this object, with no inheritance or recursive traversal.
    pub fn grant(&self, path: &Path, access: Access) -> io::Result<AclGrant> {
        if !self.0.active {
            return Err(io::Error::other("AppContainer profile is closed"));
        }
        let file = OpenOptions::new()
            .access_mode(READ_CONTROL | WRITE_DAC)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
            .open(path)?;
        if file.metadata()?.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err(io::Error::other("ACL leases refuse reparse points"));
        }
        update_acl(&file, self.sid(), Some(access.mask()))?;
        Ok(AclGrant {
            profile: self.clone(),
            file,
            active: true,
        })
    }

    pub(crate) fn sid(&self) -> PSID {
        self.0.sid.as_ptr().cast_mut().cast()
    }

    pub(crate) fn ensure_active(&self) -> io::Result<()> {
        if self.0.active {
            Ok(())
        } else {
            Err(io::Error::other("AppContainer profile is closed"))
        }
    }
}

impl ProfileInner {
    fn close(&mut self) -> io::Result<()> {
        if self.active {
            let name = wide(&self.name);
            // The profile name remains valid and all leases have released ownership.
            hresult(unsafe { DeleteAppContainerProfile(name.as_ptr()) })?;
            self.active = false;
        }
        Ok(())
    }
}

impl Drop for ProfileInner {
    fn drop(&mut self) {
        if let Err(error) = self.close() {
            cleanup_error("profile deletion", error);
        }
    }
}

#[derive(Clone, Copy)]
pub enum Access {
    Read,
    Write,
}

impl Access {
    fn mask(self) -> u32 {
        let read = FILE_GENERIC_READ | FILE_GENERIC_EXECUTE;
        match self {
            Self::Read => read,
            Self::Write => read | FILE_GENERIC_WRITE | DELETE,
        }
    }
}

/// Keeps the original object handle and profile alive through revocation.
pub struct AclGrant {
    profile: Profile,
    file: File,
    active: bool,
}

impl AclGrant {
    pub fn close(mut self) -> io::Result<()> {
        self.revoke()
    }

    fn revoke(&mut self) -> io::Result<()> {
        if self.active {
            update_acl(&self.file, self.profile.sid(), None)?;
            self.active = false;
        }
        Ok(())
    }
}

impl Drop for AclGrant {
    fn drop(&mut self) {
        if let Err(error) = self.revoke() {
            cleanup_error("ACL revocation", error);
        }
    }
}

struct AllocatedSid(PSID);

impl AllocatedSid {
    fn copy(&self) -> io::Result<Vec<u32>> {
        if self.0.is_null() {
            return Err(io::Error::other("AppContainer creation returned no SID"));
        }
        // The profile API returned a valid SID; u32 storage preserves SID alignment.
        let length = unsafe { GetLengthSid(self.0) };
        let mut words = vec![0u32; (length as usize).div_ceil(4)];
        if unsafe { CopySid(length, words.as_mut_ptr().cast(), self.0) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(words)
    }
}

impl Drop for AllocatedSid {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // CreateAppContainerProfile transfers a SID allocation released by FreeSid.
            unsafe { FreeSid(self.0) };
        }
    }
}

struct LocalAllocation(*mut core::ffi::c_void);

struct TaskAllocation(*mut u16);

impl TaskAllocation {
    fn path(&self) -> io::Result<PathBuf> {
        if self.0.is_null() {
            return Err(io::Error::other("AppContainer storage path is missing"));
        }
        // Windows returns a valid NUL-terminated UTF-16 allocation.
        let mut length = 0;
        while unsafe { *self.0.add(length) } != 0 {
            length += 1;
        }
        let units = unsafe { std::slice::from_raw_parts(self.0, length) };
        Ok(std::ffi::OsString::from_wide(units).into())
    }
}

impl Drop for TaskAllocation {
    fn drop(&mut self) {
        // GetAppContainerFolderPath transfers a CoTaskMem allocation to the caller.
        unsafe { CoTaskMemFree(self.0.cast()) };
    }
}

impl Drop for LocalAllocation {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // Security descriptors and merged ACLs use LocalAlloc ownership.
            unsafe { LocalFree(self.0) };
        }
    }
}

fn read_acl(file: &File) -> io::Result<(LocalAllocation, *mut ACL)> {
    let mut descriptor = null_mut();
    let mut acl = null_mut();
    // The retained file handle is valid; the ACL borrows the returned descriptor.
    let status = unsafe {
        GetSecurityInfo(
            file.as_raw_handle(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION,
            null_mut(),
            null_mut(),
            &mut acl,
            null_mut(),
            &mut descriptor,
        )
    };
    let allocation = LocalAllocation(descriptor);
    win32(status)?;
    Ok((allocation, acl))
}

fn update_acl(file: &File, sid: PSID, mask: Option<u32>) -> io::Result<()> {
    let _lock = ACL_UPDATE
        .lock()
        .map_err(|_| io::Error::other("ACL update lock poisoned"))?;
    let (_descriptor, old) = read_acl(file)?;
    if old.is_null() {
        return if mask.is_some() {
            Err(io::Error::other("ACL leases refuse a null DACL"))
        } else {
            Ok(())
        };
    }
    if mask.is_some() && !sid_entries(old, sid)?.is_empty() {
        return Err(io::Error::other(
            "This profile already has a grant on the object",
        ));
    }
    let entry = EXPLICIT_ACCESS_W {
        grfAccessPermissions: mask.unwrap_or_default(),
        grfAccessMode: if mask.is_some() {
            GRANT_ACCESS
        } else {
            REVOKE_ACCESS
        },
        grfInheritance: 0,
        Trustee: TRUSTEE_W {
            TrusteeForm: TRUSTEE_IS_SID,
            ptstrName: sid.cast(),
            ..Default::default()
        },
    };
    let mut merged = null_mut();
    // The descriptor, entry and aligned SID stay alive through merging and installation.
    let status = unsafe { SetEntriesInAclW(1, &entry, old, &mut merged) };
    let _merged = LocalAllocation(merged.cast());
    win32(status)?;
    win32(unsafe {
        SetSecurityInfo(
            file.as_raw_handle(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION,
            null_mut(),
            null_mut(),
            merged,
            null(),
        )
    })
}

fn sid_entries(acl: *const ACL, sid: PSID) -> io::Result<Vec<Vec<u8>>> {
    let mut entries = Vec::new();
    // The ACL is borrowed from the live security descriptor returned by Windows.
    for index in 0..unsafe { (*acl).AceCount } {
        let mut raw = null_mut();
        if unsafe { GetAce(acl, u32::from(index), &mut raw) } == 0 {
            return Err(io::Error::last_os_error());
        }
        let header = unsafe { &*raw.cast::<ACE_HEADER>() };
        // Ordinary allowed/denied ACEs share the same SID offset. Our grants are allowed ACEs.
        if header.AceType > 1 {
            continue;
        }
        let entry = raw.cast::<ACCESS_ALLOWED_ACE>();
        let entry_sid = unsafe { std::ptr::addr_of!((*entry).SidStart).cast_mut().cast() };
        if unsafe { EqualSid(entry_sid, sid) } != 0 {
            entries.push(unsafe {
                std::slice::from_raw_parts(raw.cast::<u8>(), usize::from(header.AceSize)).to_vec()
            });
        }
    }
    Ok(entries)
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(Some(0)).collect()
}

fn hresult(status: i32) -> io::Result<()> {
    if status < 0 {
        Err(io::Error::other(format!(
            "AppContainer HRESULT {status:#010x}"
        )))
    } else {
        Ok(())
    }
}

fn win32(status: u32) -> io::Result<()> {
    if status == 0 {
        Ok(())
    } else {
        Err(io::Error::from_raw_os_error(status as i32))
    }
}

fn cleanup_error(operation: &str, error: io::Error) {
    cyber_core::log::error(
        "sandbox",
        "Windows AppContainer cleanup failed",
        serde_json::json!({"operation": operation, "error": error.to_string()}),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entries(file: &File, profile: &Profile) -> Vec<Vec<u8>> {
        let (_descriptor, acl) = read_acl(file).unwrap();
        assert!(!acl.is_null());
        sid_entries(acl, profile.sid()).unwrap()
    }

    fn file(path: &Path) -> File {
        OpenOptions::new()
            .access_mode(READ_CONTROL | WRITE_DAC)
            .open(path)
            .unwrap()
    }

    fn all_entries(file: &File) -> Vec<Vec<u8>> {
        let (_descriptor, acl) = read_acl(file).unwrap();
        assert!(!acl.is_null());
        let mut entries = Vec::new();
        for index in 0..unsafe { (*acl).AceCount } {
            let mut raw = null_mut();
            assert_ne!(unsafe { GetAce(acl, u32::from(index), &mut raw) }, 0);
            let header = unsafe { &*raw.cast::<ACE_HEADER>() };
            entries.push(unsafe {
                std::slice::from_raw_parts(raw.cast::<u8>(), usize::from(header.AceSize)).to_vec()
            });
        }
        entries
    }

    fn assert_mask(file: &File, profile: &Profile, access: Access) {
        let own = entries(file, profile);
        assert_eq!(own.len(), 1);
        let bytes: [u8; 4] = own[0][4..8].try_into().unwrap();
        assert_eq!(u32::from_ne_bytes(bytes), access.mask());
    }

    #[test]
    fn unique_profiles_refuse_reuse_and_delete_after_last_owner() {
        let mut first = Profile::new().unwrap();
        let mut second = Profile::new().unwrap();
        assert_ne!(first.name(), second.name());
        assert_ne!(first.0.sid, second.0.sid);
        let name = first.name().to_owned();
        assert!(Profile::named(name.clone()).is_err());
        let clone = first.clone();
        assert!(first.close().is_err());
        drop(clone);
        first.close().unwrap();
        let mut recreated = Profile::named(name).unwrap();
        recreated.close().unwrap();
        second.close().unwrap();
    }

    #[test]
    fn leases_preserve_other_profiles_and_refuse_overlapping_grants() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("leased.txt");
        std::fs::write(&path, "owned").unwrap();
        let object = file(&path);
        let original = all_entries(&object);
        let mut first = Profile::new().unwrap();
        let mut second = Profile::new().unwrap();
        let read = first.grant(&path, Access::Read).unwrap();
        assert_mask(&object, &first, Access::Read);
        assert!(first.grant(&path, Access::Write).is_err());
        assert!(first.close().is_err());
        let write = second.grant(&path, Access::Write).unwrap();
        assert_mask(&object, &second, Access::Write);
        let preserved = entries(&object, &second);
        assert_eq!(preserved.len(), 1);
        read.close().unwrap();
        assert!(entries(&object, &first).is_empty());
        assert_eq!(entries(&object, &second), preserved);
        write.close().unwrap();
        assert!(entries(&object, &second).is_empty());
        assert_eq!(all_entries(&object), original);
        first.close().unwrap();
        second.close().unwrap();
    }

    #[test]
    fn revocation_uses_original_handle_after_path_replacement() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("original.txt");
        let moved = directory.path().join("moved.txt");
        std::fs::write(&path, "original").unwrap();
        let mut profile = Profile::new().unwrap();
        let lease = profile.grant(&path, Access::Write).unwrap();
        std::fs::rename(&path, &moved).unwrap();
        std::fs::write(&path, "replacement").unwrap();
        let original = file(&moved);
        let replacement = file(&path);
        assert_eq!(entries(&original, &profile).len(), 1);
        assert!(entries(&replacement, &profile).is_empty());
        lease.close().unwrap();
        assert!(entries(&original, &profile).is_empty());
        assert!(entries(&replacement, &profile).is_empty());
        profile.close().unwrap();
    }

    #[test]
    fn setup_failure_and_drop_release_profile_ownership() {
        let directory = tempfile::tempdir().unwrap();
        let mut profile = Profile::new().unwrap();
        assert!(
            profile
                .grant(&directory.path().join("missing"), Access::Read)
                .is_err()
        );
        let path = directory.path().join("file.txt");
        std::fs::write(&path, "owned").unwrap();
        let object = file(&path);
        let lease = profile.grant(&path, Access::Read).unwrap();
        drop(lease);
        assert!(entries(&object, &profile).is_empty());
        profile.close().unwrap();
        assert!(profile.grant(&path, Access::Read).is_err());
    }

    #[test]
    fn directory_junction_is_refused_without_changing_target_acl() {
        let directory = tempfile::tempdir().unwrap();
        let target = directory.path().join("target");
        let link = directory.path().join("junction");
        std::fs::create_dir(&target).unwrap();
        let status = std::process::Command::new("cmd.exe")
            .args(["/d", "/c", "mklink", "/j"])
            .arg(&link)
            .arg(&target)
            .output()
            .unwrap();
        assert!(
            status.status.success(),
            "junction creation failed: {status:?}"
        );
        let mut profile = Profile::new().unwrap();
        assert!(profile.grant(&link, Access::Write).is_err());
        let object = OpenOptions::new()
            .access_mode(READ_CONTROL)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
            .open(&target)
            .unwrap();
        assert!(entries(&object, &profile).is_empty());
        std::fs::remove_dir(&link).unwrap();
        profile.close().unwrap();
    }
}
