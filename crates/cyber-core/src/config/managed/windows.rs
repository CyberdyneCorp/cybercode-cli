//! Windows: an OS-managed policy document is accepted only when administrators control it
//! (`org-policy` → Policy sources). `C:\ProgramData` lets any user create files and folders,
//! so a document there is managed only when the file and its `cyber` directory are owned by
//! SYSTEM, Administrators or TrustedInstaller and no other principal may change them.

use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::os::windows::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::ptr::null_mut;

use windows_sys::Win32::Foundation::{ERROR_SUCCESS, LocalFree};
use windows_sys::Win32::Security::Authorization::{GetNamedSecurityInfoW, SE_FILE_OBJECT};
use windows_sys::Win32::Security::{
    ACCESS_ALLOWED_ACE, ACE_HEADER, ACL, DACL_SECURITY_INFORMATION, GetAce, GetLengthSid,
    GetSecurityDescriptorDacl, GetSecurityDescriptorOwner, INHERIT_ONLY_ACE, IsValidAcl,
    IsValidSid, IsWellKnownSid, OWNER_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID,
    WinBuiltinAdministratorsSid, WinLocalSystemSid,
};
use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;
use windows_sys::Win32::System::Com::CoTaskMemFree;
use windows_sys::Win32::UI::Shell::{FOLDERID_ProgramData, SHGetKnownFolderPath};

const FILE_WRITE_DATA: u32 = 0x0002;
const FILE_APPEND_DATA: u32 = 0x0004;
const FILE_WRITE_EA: u32 = 0x0010;
const FILE_DELETE_CHILD: u32 = 0x0040;
const DELETE: u32 = 0x0001_0000;
const WRITE_DAC: u32 = 0x0004_0000;
const WRITE_OWNER: u32 = 0x0008_0000;
const GENERIC_ALL: u32 = 0x1000_0000;
const GENERIC_WRITE: u32 = 0x4000_0000;

/// Rights that let a principal change, replace or remove the document itself.
pub(super) const FILE_RIGHTS: u32 = FILE_WRITE_DATA
    | FILE_APPEND_DATA
    | FILE_WRITE_EA
    | DELETE
    | WRITE_DAC
    | WRITE_OWNER
    | GENERIC_ALL
    | GENERIC_WRITE;
/// Rights on the directory that let a principal remove, rename or take over an
/// administrator's document. Adding files stays allowed: an added document is owned by
/// whoever created it and fails the file check.
pub(super) const DIRECTORY_RIGHTS: u32 =
    FILE_DELETE_CHILD | DELETE | WRITE_DAC | WRITE_OWNER | GENERIC_ALL;

/// `NT SERVICE\TrustedInstaller`, S-1-5-80-956008885-3418522649-1831038044-1853292631-2271478464.
const TRUSTED_INSTALLER: [u32; 6] = [
    80,
    956_008_885,
    3_418_522_649,
    1_831_038_044,
    1_853_292_631,
    2_271_478_464,
];

/// `%ProgramData%` from the shell known-folder store, never from the environment, which
/// any process can change.
pub(super) fn program_data() -> PathBuf {
    let mut raw = null_mut();
    // The shell allocates the string with CoTaskMemAlloc; it is freed below either way.
    let result = unsafe { SHGetKnownFolderPath(&FOLDERID_ProgramData, 0, null_mut(), &mut raw) };
    let path = (result >= 0 && !raw.is_null()).then(|| {
        let mut length = 0;
        while unsafe { *raw.add(length) } != 0 {
            length += 1;
        }
        let wide = unsafe { std::slice::from_raw_parts(raw, length) };
        PathBuf::from(std::ffi::OsString::from_wide(wide))
    });
    unsafe { CoTaskMemFree(raw.cast()) };
    path.filter(|path| path.is_absolute())
        .unwrap_or_else(|| PathBuf::from(r"C:\ProgramData"))
}

/// Whether the document at `path` and its directory are administrator-controlled.
pub(super) fn admin_controlled(path: &Path) -> bool {
    let Some(dir) = path.parent() else {
        return false;
    };
    [(path, FILE_RIGHTS), (dir, DIRECTORY_RIGHTS)]
        .into_iter()
        .all(|(object, rights)| {
            !reparse_point(object)
                && Descriptor::named(object).is_some_and(|descriptor| descriptor.admin_only(rights))
        })
}

fn reparse_point(path: &Path) -> bool {
    std::fs::symlink_metadata(path).map_or(true, |metadata| {
        metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
    })
}

/// One security descriptor allocated by Windows and released with `LocalFree`.
pub(super) struct Descriptor(PSECURITY_DESCRIPTOR);

impl Drop for Descriptor {
    fn drop(&mut self) {
        unsafe { LocalFree(self.0) };
    }
}

impl Descriptor {
    fn named(path: &Path) -> Option<Self> {
        let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        let mut allocation = null_mut();
        // Only the self-relative descriptor is requested; owner and DACL are read from it.
        let result = unsafe {
            GetNamedSecurityInfoW(
                wide.as_ptr(),
                SE_FILE_OBJECT,
                OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
                null_mut(),
                null_mut(),
                null_mut(),
                null_mut(),
                &mut allocation,
            )
        };
        let descriptor = (!allocation.is_null()).then_some(Self(allocation));
        (result == ERROR_SUCCESS).then_some(descriptor).flatten()
    }

    /// Wrap a descriptor the caller allocated with `LocalAlloc` semantics.
    #[cfg(test)]
    pub(super) fn from_raw(allocation: PSECURITY_DESCRIPTOR) -> Self {
        Self(allocation)
    }

    /// An administrator owns the object and only administrators hold any of `rights`.
    pub(super) fn admin_only(&self, rights: u32) -> bool {
        self.owner().is_some_and(administrator)
            && self.dacl().is_some_and(|acl| aces_admin_only(acl, rights))
    }

    fn owner(&self) -> Option<PSID> {
        let mut owner = null_mut();
        let mut defaulted = 0;
        let ok = unsafe { GetSecurityDescriptorOwner(self.0, &mut owner, &mut defaulted) };
        (ok != 0 && !owner.is_null() && unsafe { IsValidSid(owner) } != 0).then_some(owner)
    }

    /// The DACL; an absent or NULL DACL grants everyone full access and is refused.
    fn dacl(&self) -> Option<*mut ACL> {
        let mut present = 0;
        let mut acl = null_mut();
        let mut defaulted = 0;
        let ok =
            unsafe { GetSecurityDescriptorDacl(self.0, &mut present, &mut acl, &mut defaulted) };
        (ok != 0 && present != 0 && !acl.is_null() && unsafe { IsValidAcl(acl) } != 0)
            .then_some(acl)
    }
}

fn aces_admin_only(acl: *mut ACL, rights: u32) -> bool {
    let count = unsafe { (*acl).AceCount };
    (0..count).all(|index| ace_admin_only(acl, index, rights))
}

/// Deny entries only narrow access and inherit-only entries do not apply to the object.
/// Allow entries that grant `rights` must name an administrator; any other entry type is
/// refused rather than interpreted.
fn ace_admin_only(acl: *mut ACL, index: u16, rights: u32) -> bool {
    let mut raw = null_mut();
    if unsafe { GetAce(acl, u32::from(index), &mut raw) } == 0 || raw.is_null() {
        return false;
    }
    let header = unsafe { &*raw.cast::<ACE_HEADER>() };
    if u32::from(header.AceFlags) & INHERIT_ONLY_ACE != 0 {
        return true;
    }
    match header.AceType {
        // ACCESS_DENIED_ACE_TYPE
        1 => true,
        // ACCESS_ALLOWED_ACE_TYPE
        0 => {
            let offset = std::mem::offset_of!(ACCESS_ALLOWED_ACE, SidStart);
            if usize::from(header.AceSize) < offset + 8 {
                return false;
            }
            let ace = unsafe { &*raw.cast::<ACCESS_ALLOWED_ACE>() };
            let sid: PSID = unsafe { raw.cast::<u8>().add(offset) }.cast();
            ace.Mask & rights == 0 || (unsafe { IsValidSid(sid) } != 0 && administrator(sid))
        }
        _ => false,
    }
}

fn administrator(sid: PSID) -> bool {
    let system = unsafe { IsWellKnownSid(sid, WinLocalSystemSid) };
    let administrators = unsafe { IsWellKnownSid(sid, WinBuiltinAdministratorsSid) };
    system != 0 || administrators != 0 || trusted_installer(sid)
}

fn trusted_installer(sid: PSID) -> bool {
    let mut expected = vec![1u8, 6, 0, 0, 0, 0, 0, 5];
    expected.extend(TRUSTED_INSTALLER.iter().flat_map(|part| part.to_le_bytes()));
    let length = unsafe { GetLengthSid(sid) } as usize;
    length == expected.len()
        && unsafe { std::slice::from_raw_parts(sid.cast::<u8>(), length) } == expected.as_slice()
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows_sys::Win32::Security::Authorization::ConvertStringSecurityDescriptorToSecurityDescriptorW;

    fn sddl(text: &str) -> Descriptor {
        let wide: Vec<u16> = text.encode_utf16().chain(Some(0)).collect();
        let mut raw = null_mut();
        let ok = unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                wide.as_ptr(),
                1,
                &mut raw,
                null_mut(),
            )
        };
        assert_ne!(ok, 0, "{text}");
        Descriptor::from_raw(raw)
    }

    const TI: &str = "S-1-5-80-956008885-3418522649-1831038044-1853292631-2271478464";

    #[test]
    fn administrator_owned_documents_without_foreign_write_access_are_managed() {
        for text in [
            "O:BAD:P(A;;FA;;;SY)(A;;FA;;;BA)(A;;FR;;;BU)",
            "O:SYD:P(A;;FA;;;SY)(A;OICIIO;FA;;;CO)(A;;0x1200a9;;;BU)",
            "O:BAD:P(A;;FA;;;BA)(D;;FA;;;WD)",
            &format!("O:{TI}D:P(A;;FA;;;{TI})(A;;FR;;;WD)"),
        ] {
            assert!(sddl(text).admin_only(FILE_RIGHTS), "{text}");
        }
    }

    #[test]
    fn user_owned_writable_or_unprotected_documents_are_not_managed() {
        for text in [
            "O:BUD:P(A;;FA;;;SY)",
            "O:WDD:P(A;;FA;;;SY)",
            "O:BAD:P(A;;FA;;;SY)(A;;FW;;;BU)",
            "O:BAD:P(A;;FA;;;SY)(A;;SD;;;AU)",
            "O:BAD:P(A;;FA;;;SY)(A;;WD;;;BU)",
            "O:BAD:P(A;;FA;;;SY)(A;;GA;;;WD)",
            "O:BAD:NO_ACCESS_CONTROL",
            "O:BA",
        ] {
            assert!(!sddl(text).admin_only(FILE_RIGHTS), "{text}");
        }
    }

    #[test]
    fn directory_may_let_users_add_files_but_not_remove_or_take_them_over() {
        let add_files = sddl("O:BAD:P(A;;FA;;;SY)(A;;FA;;;BA)(A;CI;0x1201bf;;;BU)");
        assert!(add_files.admin_only(DIRECTORY_RIGHTS));
        assert!(!add_files.admin_only(FILE_RIGHTS));
        for text in [
            "O:BAD:P(A;;FA;;;SY)(A;;0x40;;;BU)",
            "O:BAD:P(A;;FA;;;SY)(A;;SD;;;BU)",
            "O:BAD:P(A;;FA;;;SY)(A;;WO;;;BU)",
            "O:BUD:P(A;;FA;;;SY)",
        ] {
            assert!(!sddl(text).admin_only(DIRECTORY_RIGHTS), "{text}");
        }
    }

    #[test]
    fn program_data_comes_from_the_known_folder_store_not_the_environment() {
        let path = program_data();
        assert!(path.is_absolute());
        assert!(path.is_dir(), "{}", path.display());
    }

    #[test]
    fn a_missing_document_is_not_admin_controlled() {
        let dir = std::env::temp_dir().join(format!("cyber-managed-{}", std::process::id()));
        assert!(!admin_controlled(&dir.join("cyber-policy.json")));
    }
}
