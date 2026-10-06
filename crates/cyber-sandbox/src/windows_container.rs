//! Invocation-owned AppContainer profiles and direct-object ACL leases.
//! This foundation does not launch or enable the Windows sandbox.
#![allow(unsafe_code)]

mod identity;
mod tree;
pub use identity::IdentityGrant;
pub use tree::{ExistingTreeGrant, TreeInventory};

use std::fs::{File, OpenOptions};
use std::io;
use std::os::windows::ffi::OsStringExt;
use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
use std::os::windows::io::{AsRawHandle, BorrowedHandle};
use std::path::{Component, Path, PathBuf, Prefix};
use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicBool, Ordering};
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
    ACCESS_ALLOWED_ACE, ACE_HEADER, ACL, AddAce, CONTAINER_INHERIT_ACE, CopySid,
    DACL_SECURITY_INFORMATION, EqualSid, FreeSid, GetAce, GetLengthSid, InitializeAcl,
    OBJECT_INHERIT_ACE, PROTECTED_DACL_SECURITY_INFORMATION, PSID,
};
use windows_sys::Win32::Storage::FileSystem::{
    DELETE, FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
    FILE_GENERIC_EXECUTE, FILE_GENERIC_READ, FILE_GENERIC_WRITE, FILE_LIST_DIRECTORY,
    FILE_READ_ATTRIBUTES, FILE_SHARE_READ, READ_CONTROL, WRITE_DAC,
};
use windows_sys::Win32::System::Com::CoTaskMemFree;
use windows_sys::Win32::System::Threading::ResumeThread;

// Serialize our own read/merge/write operations on shared objects.
static ACL_UPDATE: Mutex<()> = Mutex::new(());

#[derive(Clone)]
pub struct Profile(Arc<ProfileInner>);

struct ProfileInner {
    name: String,
    sid: Vec<u32>,
    active: bool,
    launching: AtomicBool,
    used: AtomicBool,
    preparation: Mutex<()>,
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
            launching: AtomicBool::new(false),
            used: AtomicBool::new(false),
            preparation: Mutex::new(()),
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
        self.grant_object(path, access, 0)
    }

    /// Record a single object's identity without retaining its child handle.
    /// This does not recursively grant a directory or enable confinement.
    pub fn grant_relocatable(&self, path: &Path, access: Access) -> io::Result<IdentityGrant> {
        self.grant_relocatable_object(path, access, 0)
    }

    /// Grant observed existing objects only, with no future-child inheritance.
    /// Recursive exclusions and runtime policy require separate preparation.
    pub fn grant_existing_tree(
        &self,
        root: &Path,
        access: Access,
        limit: usize,
    ) -> io::Result<ExistingTreeGrant> {
        let _preparation = self
            .0
            .preparation
            .lock()
            .map_err(|_| io::Error::other("Profile preparation lock poisoned"))?;
        self.ensure_unstarted()?;
        TreeInventory::capture(root, limit)?.grant_existing(self, access)
    }

    fn grant_relocatable_object(
        &self,
        path: &Path,
        access: Access,
        inheritance: u32,
    ) -> io::Result<IdentityGrant> {
        let _preparation = self
            .0
            .preparation
            .lock()
            .map_err(|_| io::Error::other("Profile preparation lock poisoned"))?;
        self.ensure_unstarted()?;
        identity::grant(self, path, access, inheritance)
    }

    pub(crate) fn grant_private_directory(&self, path: &Path) -> io::Result<AclGrant> {
        self.grant_object(
            path,
            Access::Write,
            CONTAINER_INHERIT_ACE | OBJECT_INHERIT_ACE,
        )
    }

    fn grant_object(&self, path: &Path, access: Access, inheritance: u32) -> io::Result<AclGrant> {
        let _preparation = self
            .0
            .preparation
            .lock()
            .map_err(|_| io::Error::other("Profile preparation lock poisoned"))?;
        self.ensure_unstarted()?;
        let _ancestors = retain_ancestors(path)?;
        let file = OpenOptions::new()
            .access_mode(READ_CONTROL | WRITE_DAC)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
            .open(path)?;
        if file.metadata()?.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err(io::Error::other("ACL leases refuse reparse points"));
        }
        if inheritance != 0 && !file.metadata()?.is_dir() {
            return Err(io::Error::other(
                "Inheritable grants require a private directory",
            ));
        }
        if inheritance != 0 {
            prepare_private_acl(&file, self.sid())?;
        }
        update_acl(&file, self.sid(), Some(access.mask()), inheritance)?;
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

    pub(crate) fn reserve_launch(&self) -> io::Result<LaunchReservation> {
        self.ensure_active()?;
        self.0
            .launching
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| {
                io::Error::new(
                    io::ErrorKind::WouldBlock,
                    "AppContainer profile already has a command owner",
                )
            })?;
        let reservation = LaunchReservation(self.clone());
        self.ensure_unstarted()?;
        Ok(reservation)
    }

    fn ensure_unstarted(&self) -> io::Result<()> {
        self.ensure_active()?;
        if self.0.used.load(Ordering::Acquire) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "AppContainer profiles are single-use after command start",
            ));
        }
        Ok(())
    }
}

/// Pin each checked directory while opening and granting the leaf. Sharing only
/// reads refuses conflicting directory writers and deletion/rename handles.
fn retain_ancestors(path: &Path) -> io::Result<Vec<File>> {
    let components: Vec<_> = path.components().collect();
    validate_local_path(&components)?;
    let mut current = PathBuf::new();
    let mut ancestors = Vec::new();
    for component in &components[..components.len() - 1] {
        current.push(component.as_os_str());
        if matches!(component, Component::Prefix(_)) {
            continue;
        }
        let directory = pin_directory(&current)?;
        ancestors.push(directory);
    }
    Ok(ancestors)
}

fn pin_directory(path: &Path) -> io::Result<File> {
    let directory = OpenOptions::new()
        .access_mode(FILE_READ_ATTRIBUTES | FILE_LIST_DIRECTORY)
        .share_mode(FILE_SHARE_READ)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)?;
    let metadata = directory.metadata()?;
    if !metadata.is_dir() || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "ACL preparation requires ordinary directories",
        ));
    }
    Ok(directory)
}

fn validate_local_path(components: &[Component<'_>]) -> io::Result<()> {
    let local = matches!(
        components.first(),
        Some(Component::Prefix(prefix))
            if matches!(prefix.kind(), Prefix::Disk(_) | Prefix::VerbatimDisk(_))
    );
    let rooted = matches!(components.get(1), Some(Component::RootDir));
    let ordinary = components
        .get(2..)
        .is_some_and(|tail| !tail.is_empty() && tail.iter().all(ordinary_component));
    if local && rooted && ordinary {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "ACL grants require an absolute local path without parent traversal",
        ))
    }
}

fn ordinary_component(component: &Component<'_>) -> bool {
    use std::os::windows::ffi::OsStrExt;
    let Component::Normal(name) = component else {
        return false;
    };
    let units: Vec<_> = name.encode_wide().collect();
    if matches!(units.last(), Some(32 | 46))
        || units
            .iter()
            .any(|unit| matches!(unit, 0..=31 | 47 | 58 | 92))
    {
        return false;
    }
    let name = String::from_utf16_lossy(&units).to_ascii_uppercase();
    let stem = name.split('.').next().unwrap_or("").trim_end();
    let reserved = matches!(stem, "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$")
        || ((stem.starts_with("COM") || stem.starts_with("LPT"))
            && matches!(
                stem.get(3..),
                Some("1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³")
            ));
    !reserved
}

pub(crate) struct LaunchReservation(Profile);

impl LaunchReservation {
    pub(crate) fn resume(&self, thread: BorrowedHandle<'_>) -> io::Result<()> {
        // Serialize final scope preparation and execution start. No further
        // grants can race successful resume; setup failure keeps the identity unused.
        let _preparation = self
            .0
            .0
            .preparation
            .lock()
            .map_err(|_| io::Error::other("Profile preparation lock poisoned"))?;
        self.0.ensure_unstarted()?;
        if unsafe { ResumeThread(thread.as_raw_handle()) } == u32::MAX {
            return Err(io::Error::last_os_error());
        }
        self.0.0.used.store(true, Ordering::Release);
        Ok(())
    }
}

impl Drop for LaunchReservation {
    fn drop(&mut self) {
        self.0.0.launching.store(false, Ordering::Release);
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
    // Close the object handle before the last profile owner attempts deletion.
    file: File,
    profile: Profile,
    active: bool,
}

impl AclGrant {
    pub fn close(mut self) -> io::Result<()> {
        self.revoke()
    }

    fn revoke(&mut self) -> io::Result<()> {
        if self.active {
            update_acl(&self.file, self.profile.sid(), None, 0)?;
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

enum SecurityAllocation {
    Local(LocalAllocation),
    Native(Vec<u32>),
}

impl SecurityAllocation {
    fn pointer(&self) -> *mut core::ffi::c_void {
        match self {
            Self::Local(allocation) => allocation.0,
            Self::Native(storage) => storage.as_ptr().cast_mut().cast(),
        }
    }
}

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

fn read_acl(file: &File) -> io::Result<(SecurityAllocation, *mut ACL)> {
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
    Ok((SecurityAllocation::Local(allocation), acl))
}

fn update_acl(file: &File, sid: PSID, mask: Option<u32>, inheritance: u32) -> io::Result<()> {
    update_acl_using(file, sid, mask, inheritance, set_acl, read_acl)
}

type AclSetter = fn(&File, *mut core::ffi::c_void, *mut ACL) -> io::Result<()>;
type AclReader = fn(&File) -> io::Result<(SecurityAllocation, *mut ACL)>;

fn update_acl_using(
    file: &File,
    sid: PSID,
    mask: Option<u32>,
    inheritance: u32,
    setter: AclSetter,
    reader: AclReader,
) -> io::Result<()> {
    let _lock = ACL_UPDATE
        .lock()
        .map_err(|_| io::Error::other("ACL update lock poisoned"))?;
    let (descriptor, old) = reader(file)?;
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
        grfInheritance: inheritance,
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
    setter(file, descriptor.pointer(), merged)
}

fn set_acl(file: &File, _descriptor: *mut core::ffi::c_void, acl: *mut ACL) -> io::Result<()> {
    win32(unsafe {
        SetSecurityInfo(
            file.as_raw_handle(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION,
            null_mut(),
            null_mut(),
            acl,
            null(),
        )
    })
}

/// Only for a freshly created, invocation-owned directory, never a shared root.
/// Replace the profile's inherited entries with a revocable explicit lease.
fn prepare_private_acl(file: &File, sid: PSID) -> io::Result<()> {
    let _lock = ACL_UPDATE
        .lock()
        .map_err(|_| io::Error::other("ACL update lock poisoned"))?;
    let (_descriptor, old) = read_acl(file)?;
    if old.is_null() {
        return Err(io::Error::other("Private directories refuse a null DACL"));
    }
    let mut words = acl_without_sid(old, sid)?;
    let replacement = words.as_mut_ptr().cast::<ACL>();
    // Prevent the parent from reintroducing the inherited profile grant.
    win32(unsafe {
        SetSecurityInfo(
            file.as_raw_handle(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
            null_mut(),
            null_mut(),
            replacement,
            null(),
        )
    })
}

fn remove_profile_entries(file: &File, sid: PSID) -> io::Result<()> {
    let _lock = ACL_UPDATE
        .lock()
        .map_err(|_| io::Error::other("ACL update lock poisoned"))?;
    let (descriptor, old) = identity::read_acl(file)?;
    if old.is_null() || sid_entries(old, sid)?.is_empty() {
        return Ok(());
    }
    let mut words = acl_without_sid(old, sid)?;
    identity::set_acl(file, descriptor.pointer(), words.as_mut_ptr().cast())
}

fn acl_without_sid(old: *const ACL, sid: PSID) -> io::Result<Vec<u32>> {
    let own = sid_entries(old, sid)?;
    // The descriptor owns this valid ACL; u32 storage preserves its alignment.
    let size = u32::from(unsafe { (*old).AclSize });
    let revision = u32::from(unsafe { (*old).AclRevision });
    let mut words = vec![0u32; (size as usize).div_ceil(4)];
    let replacement = words.as_mut_ptr().cast::<ACL>();
    if unsafe { InitializeAcl(replacement, size, revision) } == 0 {
        return Err(io::Error::last_os_error());
    }
    for index in 0..unsafe { (*old).AceCount } {
        let mut raw = null_mut();
        if unsafe { GetAce(old, u32::from(index), &mut raw) } == 0 {
            return Err(io::Error::last_os_error());
        }
        let header = unsafe { &*raw.cast::<ACE_HEADER>() };
        let bytes =
            unsafe { std::slice::from_raw_parts(raw.cast::<u8>(), usize::from(header.AceSize)) };
        if own.iter().any(|entry| entry.as_slice() == bytes) {
            continue;
        }
        if unsafe {
            AddAce(
                replacement,
                revision,
                u32::MAX,
                raw,
                u32::from(header.AceSize),
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(words)
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

    fn directory_file(path: &Path) -> File {
        OpenOptions::new()
            .access_mode(READ_CONTROL | WRITE_DAC)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
            .open(path)
            .unwrap()
    }

    fn all_entries(file: &File) -> Vec<Vec<u8>> {
        let (_descriptor, acl) = read_acl(file).unwrap();
        assert!(!acl.is_null());
        acl_entries(acl)
    }

    fn acl_entries(acl: *const ACL) -> Vec<Vec<u8>> {
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

    fn stored_entries(file: &File) -> Vec<Vec<u8>> {
        use windows_sys::Wdk::Storage::FileSystem::NtQuerySecurityObject;
        use windows_sys::Win32::Security::GetSecurityDescriptorDacl;
        let mut needed = 0;
        unsafe {
            NtQuerySecurityObject(
                file.as_raw_handle(),
                DACL_SECURITY_INFORMATION,
                null_mut(),
                0,
                &mut needed,
            );
        }
        assert!(needed > 0 && needed <= 1024 * 1024);
        let mut storage = vec![0u32; (needed as usize).div_ceil(4)];
        let descriptor = storage.as_mut_ptr().cast();
        let status = unsafe {
            NtQuerySecurityObject(
                file.as_raw_handle(),
                DACL_SECURITY_INFORMATION,
                descriptor,
                needed,
                &mut needed,
            )
        };
        assert!(status >= 0, "security query failed: {status:#x}");
        let mut present = 0;
        let mut defaulted = 0;
        let mut acl = null_mut();
        assert_ne!(
            unsafe {
                GetSecurityDescriptorDacl(descriptor, &mut present, &mut acl, &mut defaulted)
            },
            0,
        );
        assert_ne!(present, 0);
        assert!(!acl.is_null());
        acl_entries(acl)
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
        assert_ne!(
            first.storage_path().unwrap().canonicalize().unwrap(),
            second.storage_path().unwrap().canonicalize().unwrap()
        );
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
    fn private_directory_replaces_inherited_identity_with_revocable_lease() {
        let mut profile = Profile::new().unwrap();
        let mut other = Profile::new().unwrap();
        let path = profile.storage_path().unwrap().join("private-acl-test");
        std::fs::create_dir(&path).unwrap();
        let object = OpenOptions::new()
            .access_mode(READ_CONTROL | WRITE_DAC)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
            .open(&path)
            .unwrap();
        assert!(!entries(&object, &profile).is_empty());
        let unrelated = other.grant(&path, Access::Read).unwrap();
        let preserved = entries(&object, &other);
        let lease = profile.grant_private_directory(&path).unwrap();
        assert_mask(&object, &profile, Access::Write);
        assert_eq!(entries(&object, &other), preserved);
        let nested = path.join("nested.txt");
        std::fs::write(&nested, "private").unwrap();
        let nested_object = file(&nested);
        assert_mask(&nested_object, &profile, Access::Write);
        lease.close().unwrap();
        assert!(entries(&object, &profile).is_empty());
        assert!(entries(&nested_object, &profile).is_empty());
        assert_eq!(entries(&object, &other), preserved);
        unrelated.close().unwrap();
        drop(nested_object);
        drop(object);
        std::fs::remove_dir_all(&path).unwrap();
        profile.close().unwrap();
        other.close().unwrap();
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
    fn identity_leases_allow_directory_moves_and_preserve_other_profiles() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("tree");
        let moved = directory.path().join("moved");
        std::fs::create_dir(&root).unwrap();
        let path = root.join("original.txt");
        std::fs::write(&path, "original").unwrap();
        let mut first = Profile::new().unwrap();
        let mut other = Profile::new().unwrap();
        let lease = first.grant_relocatable(&path, Access::Write).unwrap();
        let unrelated = other.grant_relocatable(&path, Access::Read).unwrap();
        assert!(first.close().is_err());
        std::fs::rename(&root, &moved).unwrap();
        std::fs::create_dir(&root).unwrap();
        std::fs::write(&path, "replacement").unwrap();
        let original = file(&moved.join("original.txt"));
        let replacement = file(&path);
        assert_mask(&original, &first, Access::Write);
        let preserved = entries(&original, &other);
        assert!(entries(&replacement, &first).is_empty());
        let replacement_acl = all_entries(&replacement);
        lease.close().unwrap();
        assert!(entries(&original, &first).is_empty());
        assert_eq!(entries(&original, &other), preserved);
        assert_eq!(all_entries(&replacement), replacement_acl);
        unrelated.close().unwrap();
        assert!(entries(&original, &other).is_empty());
        first.close().unwrap();
        other.close().unwrap();
    }

    #[test]
    fn identity_setup_refuses_hardlinks_without_acl_changes() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("original.txt");
        let alias = directory.path().join("alias.txt");
        std::fs::write(&path, "original").unwrap();
        std::fs::hard_link(&path, &alias).unwrap();
        let object = file(&path);
        let original = all_entries(&object);
        let mut profile = Profile::new().unwrap();
        assert!(profile.grant_relocatable(&path, Access::Write).is_err());
        assert_eq!(all_entries(&object), original);
        assert_eq!(all_entries(&file(&alias)), original);
        profile.close().unwrap();
    }

    struct DeniedDacl {
        file: File,
        descriptor: SecurityAllocation,
        acl: *mut ACL,
    }

    impl Drop for DeniedDacl {
        fn drop(&mut self) {
            if let Err(error) = identity::set_acl(&self.file, self.descriptor.pointer(), self.acl) {
                cleanup_error("test DACL restoration", error);
            }
        }
    }

    fn deny_new_dacl_handles(file: File) -> DeniedDacl {
        use windows_sys::Win32::Security::Authorization::{ConvertStringSidToSidW, DENY_ACCESS};
        let (descriptor, old) = identity::read_acl(&file).unwrap();
        // Owner Rights suppresses implicit WRITE_DAC; World covers explicit grants.
        let sids = ["S-1-1-0", "S-1-3-4"].map(|name| {
            let mut sid = null_mut();
            assert_ne!(
                unsafe { ConvertStringSidToSidW(wide(name).as_ptr(), &mut sid) },
                0
            );
            LocalAllocation(sid)
        });
        let entries = sids.each_ref().map(|sid| EXPLICIT_ACCESS_W {
            grfAccessPermissions: WRITE_DAC,
            grfAccessMode: DENY_ACCESS,
            grfInheritance: 0,
            Trustee: TRUSTEE_W {
                TrusteeForm: TRUSTEE_IS_SID,
                ptstrName: sid.0.cast(),
                ..Default::default()
            },
        });
        let mut acl = null_mut();
        win32(unsafe { SetEntriesInAclW(2, entries.as_ptr(), old, &mut acl) }).unwrap();
        let _allocation = LocalAllocation(acl.cast());
        identity::set_acl(&file, descriptor.pointer(), acl).unwrap();
        DeniedDacl {
            file,
            descriptor,
            acl: old,
        }
    }

    #[test]
    fn existing_tree_revocation_failure_preserves_retry_and_cleans_other_objects() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("tree");
        let nested = root.join("nested");
        std::fs::create_dir_all(&nested).unwrap();
        let leaf = nested.join("leaf.txt");
        std::fs::write(&leaf, "original").unwrap();
        let mut profile = Profile::new().unwrap();
        let mut owner = profile
            .grant_existing_tree(&root, Access::Write, 3)
            .unwrap();
        let denied = deny_new_dacl_handles(directory_file(&root));
        assert_eq!(
            identity::open_object(&root).unwrap_err().raw_os_error(),
            Some(5)
        );
        assert_eq!(owner.close().unwrap_err().raw_os_error(), Some(5));
        assert_mask(&denied.file, &profile, Access::Write);
        assert!(entries(&directory_file(&nested), &profile).is_empty());
        assert!(entries(&file(&leaf), &profile).is_empty());
        assert!(profile.close().is_err());
        drop(denied);
        owner.close().unwrap();
        assert!(entries(&directory_file(&root), &profile).is_empty());
        profile.close().unwrap();
    }

    #[test]
    fn existing_tree_grants_revoke_moved_objects_and_preserve_other_profiles() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("tree");
        let nested = root.join("nested");
        let leaf = nested.join("leaf.txt");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(&leaf, "original").unwrap();
        let mut profile = Profile::new().unwrap();
        let mut other = Profile::new().unwrap();
        let mut owner = profile
            .grant_existing_tree(&root, Access::Write, 3)
            .unwrap();
        let other_grant = other.grant_relocatable(&leaf, Access::Read).unwrap();
        assert!(profile.close().is_err());
        let moved = directory.path().join("moved");
        std::fs::rename(&root, &moved).unwrap();
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(&leaf, "replacement").unwrap();
        let replacement = stored_entries(&file(&leaf));
        let original = file(&moved.join("nested/leaf.txt"));
        assert_mask(&original, &profile, Access::Write);
        assert_mask(&original, &other, Access::Read);
        owner.close().unwrap();
        owner.close().unwrap();
        assert!(entries(&original, &profile).is_empty());
        assert!(entries(&directory_file(&moved), &profile).is_empty());
        assert!(entries(&directory_file(&moved.join("nested")), &profile).is_empty());
        assert_mask(&original, &other, Access::Read);
        assert_eq!(stored_entries(&file(&leaf)), replacement);
        profile.close().unwrap();
        other_grant.close().unwrap();
        other.close().unwrap();
    }

    #[test]
    fn existing_tree_partial_setup_rolls_back_prior_grants() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("tree");
        let nested = root.join("nested");
        let leaf = nested.join("leaf.txt");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(&leaf, "original").unwrap();
        let mut profile = Profile::new().unwrap();
        let prior = profile.grant_relocatable(&leaf, Access::Read).unwrap();
        let directories = [&root, &nested].map(|path| stored_entries(&directory_file(path)));
        let leaf_acl = stored_entries(&file(&leaf));
        assert!(
            profile
                .grant_existing_tree(&root, Access::Write, 3)
                .is_err()
        );
        assert_eq!(
            [&root, &nested].map(|path| stored_entries(&directory_file(path))),
            directories
        );
        assert_eq!(stored_entries(&file(&leaf)), leaf_acl);
        assert_mask(&file(&leaf), &profile, Access::Read);
        std::fs::rename(&root, directory.path().join("moved")).unwrap();
        prior.close().unwrap();
        profile.close().unwrap();
    }

    #[test]
    fn tree_inventory_pins_directories_without_acl_changes() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("tree");
        let nested = root.join("nested");
        std::fs::create_dir_all(&nested).unwrap();
        let leaf = nested.join("leaf.txt");
        std::fs::write(&leaf, "original").unwrap();
        let before = stored_entries(&file(&leaf));
        let directory_acls = [&root, &nested].map(|path| stored_entries(&directory_file(path)));
        let inventory = TreeInventory::capture(&root, 3).unwrap();
        assert_eq!(inventory.paths().count(), 3);
        assert!(inventory.paths().any(|path| path == leaf));
        inventory.verify().unwrap();
        assert_eq!(stored_entries(&file(&leaf)), before);
        assert_eq!(
            [&root, &nested].map(|path| stored_entries(&directory_file(path))),
            directory_acls
        );
        assert!(std::fs::rename(&nested, root.join("moved-nested")).is_err());
        assert!(std::fs::rename(&root, directory.path().join("moved")).is_err());
        drop(inventory);
        std::fs::rename(&nested, root.join("moved-nested")).unwrap();
        std::fs::rename(&root, directory.path().join("moved")).unwrap();
    }

    #[test]
    fn tree_inventory_limits_and_hardlinks_release_all_pins() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("tree");
        std::fs::create_dir(&root).unwrap();
        let leaf = root.join("leaf.txt");
        std::fs::write(&leaf, "original").unwrap();
        let before = stored_entries(&file(&leaf));
        assert!(TreeInventory::capture(&root, 0).is_err());
        assert!(TreeInventory::capture(&root, 1).is_err());
        let mut inventory = TreeInventory::capture(&root, 2).unwrap();
        let alias = root.join("alias.txt");
        assert_eq!(
            std::fs::hard_link(&leaf, &alias)
                .unwrap_err()
                .raw_os_error(),
            Some(32)
        );
        inventory.release_pins_for_test();
        std::fs::hard_link(&leaf, &alias).unwrap();
        assert!(inventory.verify().is_err());
        drop(inventory);
        assert!(TreeInventory::capture(&root, 3).is_err());
        assert_eq!(stored_entries(&file(&leaf)), before);
        std::fs::rename(&root, directory.path().join("moved")).unwrap();
    }

    #[test]
    fn tree_inventory_revalidation_refuses_replacement_paths() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("tree");
        std::fs::create_dir(&root).unwrap();
        let leaf = root.join("leaf.txt");
        std::fs::write(&leaf, "original").unwrap();
        let inventory = TreeInventory::capture(&root, 2).unwrap();
        std::fs::remove_file(&leaf).unwrap();
        std::fs::write(&leaf, "replacement").unwrap();
        let before = stored_entries(&file(&leaf));
        assert!(inventory.verify().is_err());
        assert_eq!(stored_entries(&file(&leaf)), before);
        drop(inventory);
        std::fs::rename(&root, directory.path().join("moved")).unwrap();
    }

    #[test]
    fn tree_inventory_refuses_junctions_without_changing_target_acls() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("tree");
        let target = directory.path().join("outside");
        std::fs::create_dir(&root).unwrap();
        std::fs::create_dir(&target).unwrap();
        let leaf = target.join("leaf.txt");
        std::fs::write(&leaf, "outside").unwrap();
        let before = stored_entries(&file(&leaf));
        let link = root.join("junction");
        let output = std::process::Command::new("cmd.exe")
            .args(["/d", "/c", "mklink", "/j"])
            .arg(&link)
            .arg(&target)
            .output()
            .unwrap();
        assert!(output.status.success(), "junction setup failed: {output:?}");
        assert!(TreeInventory::capture(&root, 10).is_err());
        assert_eq!(stored_entries(&file(&leaf)), before);
        std::fs::remove_dir(&link).unwrap();
        std::fs::rename(&root, directory.path().join("moved")).unwrap();
    }

    #[test]
    fn identity_directory_grants_coexist_with_preparation_pins() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("tree");
        let moved = directory.path().join("moved");
        std::fs::create_dir(&root).unwrap();
        let guards = retain_ancestors(&root.join("future.txt")).unwrap();
        let mut profile = Profile::new().unwrap();
        let lease = profile.grant_relocatable(&root, Access::Read).unwrap();
        assert!(std::fs::rename(&root, &moved).is_err());
        lease.close().unwrap();
        assert!(std::fs::rename(&root, &moved).is_err());
        drop(guards);
        std::fs::rename(&root, &moved).unwrap();
        profile.close().unwrap();
    }

    #[test]
    fn deleted_identity_cleanup_does_not_touch_replacement_file() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("original.txt");
        std::fs::write(&path, "original").unwrap();
        let mut profile = Profile::new().unwrap();
        let lease = profile.grant_relocatable(&path, Access::Write).unwrap();
        std::fs::remove_file(&path).unwrap();
        std::fs::write(&path, "replacement").unwrap();
        let replacement = file(&path);
        let original_acl = all_entries(&replacement);
        lease.close().unwrap();
        assert_eq!(all_entries(&replacement), original_acl);
        assert!(entries(&replacement, &profile).is_empty());
        profile.close().unwrap();
    }

    #[test]
    fn identity_updates_do_not_propagate_and_cleanup_removes_inherited_grants() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("tree");
        std::fs::create_dir(&root).unwrap();
        let existing = root.join("existing.txt");
        std::fs::write(&existing, "existing").unwrap();
        let existing_stored = stored_entries(&file(&existing));
        let root_object = directory_file(&root);
        let root_stored = stored_entries(&root_object);
        let mut profile = Profile::new().unwrap();
        let mut other = Profile::new().unwrap();
        let lease = profile
            .grant_relocatable_object(
                &root,
                Access::Write,
                CONTAINER_INHERIT_ACE | OBJECT_INHERIT_ACE,
            )
            .unwrap();
        let stored_after = stored_entries(&file(&existing));
        assert_eq!(
            stored_after, existing_stored,
            "stored child ACL must be unchanged"
        );
        let own_root = entries(&root_object, &profile);
        let unrelated_root: Vec<_> = stored_entries(&root_object)
            .into_iter()
            .filter(|entry| !own_root.contains(entry))
            .collect();
        assert_eq!(unrelated_root, root_stored);
        let created = root.join("created.txt");
        std::fs::write(&created, "created").unwrap();
        let object = file(&created);
        assert_mask(&object, &profile, Access::Write);
        let inherited = entries(&object, &profile);
        assert_ne!(
            inherited[0][1] & windows_sys::Win32::Security::INHERITED_ACE as u8,
            0,
        );
        let unrelated = other.grant_relocatable(&created, Access::Read).unwrap();
        let preserved = entries(&object, &other);
        let child_stored = stored_entries(&object);
        let record = identity::FileRecord::capture(&created, &object).unwrap();
        lease.close().unwrap();
        // Suppressed propagation requires explicit cleanup of newly inherited entries.
        assert_eq!(stored_entries(&object), child_stored);
        assert_eq!(stored_entries(&root_object), root_stored);
        remove_profile_entries(&record.open().unwrap(), profile.sid()).unwrap();
        assert!(entries(&object, &profile).is_empty());
        assert_eq!(entries(&object, &other), preserved);
        unrelated.close().unwrap();
        profile.close().unwrap();
        other.close().unwrap();
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
        let nested = target.join("nested.txt");
        std::fs::write(&nested, "outside").unwrap();
        let nested_object = file(&nested);
        let original = all_entries(&nested_object);
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
        assert!(
            profile
                .grant(&link.join("nested.txt"), Access::Write)
                .is_err()
        );
        assert_eq!(all_entries(&nested_object), original);
        let ordinary = profile.grant(&nested, Access::Write).unwrap();
        assert_mask(&nested_object, &profile, Access::Write);
        ordinary.close().unwrap();
        assert_eq!(all_entries(&nested_object), original);
        let object = OpenOptions::new()
            .access_mode(READ_CONTROL)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
            .open(&target)
            .unwrap();
        assert!(entries(&object, &profile).is_empty());
        std::fs::remove_dir(&link).unwrap();
        profile.close().unwrap();
    }

    #[test]
    fn ancestor_handles_pin_paths_until_grant_preparation_finishes() {
        let directory = tempfile::tempdir().unwrap();
        let parent = directory.path().join("parent");
        let moved = directory.path().join("moved");
        std::fs::create_dir(&parent).unwrap();
        let path = parent.join("file.txt");
        std::fs::write(&path, "original").unwrap();
        let ancestors = retain_ancestors(&path).unwrap();
        assert!(std::fs::rename(&parent, &moved).is_err());
        assert!(
            OpenOptions::new()
                .access_mode(FILE_GENERIC_WRITE)
                .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
                .open(&parent)
                .is_err()
        );
        drop(ancestors);
        // The same requested directory rights succeed after the guards release.
        drop(
            OpenOptions::new()
                .access_mode(FILE_GENERIC_WRITE)
                .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
                .open(&parent)
                .unwrap(),
        );
        std::fs::rename(&parent, &moved).unwrap();
        let mut profile = Profile::new().unwrap();
        let lease = profile
            .grant(&moved.join("file.txt"), Access::Read)
            .unwrap();
        // Preparation guards must not restrict ordinary later directory writes.
        std::fs::write(moved.join("another.txt"), "later").unwrap();
        std::fs::create_dir(&parent).unwrap();
        std::fs::rename(moved.join("file.txt"), &path).unwrap();
        lease.close().unwrap();
        assert!(entries(&file(&path), &profile).is_empty());
        profile.close().unwrap();
    }

    #[test]
    fn unsafe_path_shapes_are_refused_before_acl_changes() {
        for path in [
            "relative.txt",
            r"C:relative.txt",
            r"C:\parent\..\file.txt",
            r"\\server\share\file.txt",
            r"\\.\PhysicalDrive0",
            r"C:\parent\.. \file.txt",
            r"C:\parent\file.txt:stream",
            r"C:\parent\NUL.txt",
            r"C:\parent\COM¹",
        ] {
            let components: Vec<_> = Path::new(path).components().collect();
            assert_eq!(
                validate_local_path(&components).unwrap_err().kind(),
                io::ErrorKind::InvalidInput,
                "{path}"
            );
        }
        for path in [r"C:\parent\file.txt", r"\\?\C:\parent\file.txt"] {
            let components: Vec<_> = Path::new(path).components().collect();
            validate_local_path(&components).unwrap();
        }
    }
}
