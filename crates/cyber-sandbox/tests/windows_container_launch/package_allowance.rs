//! Native positive controls for unrelated application-package allowances.

use super::*;
use std::fs::{File, OpenOptions};
use std::os::windows::fs::OpenOptionsExt;
use std::ptr::null_mut;
use windows_sys::Wdk::Storage::FileSystem::{NtQuerySecurityObject, NtSetSecurityObject};
use windows_sys::Win32::Foundation::{LocalFree, RtlNtStatusToDosError};
use windows_sys::Win32::Security::Authorization::{
    ConvertStringSidToSidW, EXPLICIT_ACCESS_W, GRANT_ACCESS, SetEntriesInAclW, TRUSTEE_IS_SID,
    TRUSTEE_W,
};
use windows_sys::Win32::Security::{
    ACL, DACL_SECURITY_INFORMATION, GetSecurityDescriptorControl, GetSecurityDescriptorDacl,
    InitializeSecurityDescriptor, SE_DACL_AUTO_INHERITED, SE_DACL_DEFAULTED, SE_DACL_PROTECTED,
    SECURITY_DESCRIPTOR, SetSecurityDescriptorControl, SetSecurityDescriptorDacl,
};
use windows_sys::Win32::Storage::FileSystem::{
    FILE_ALL_ACCESS, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, READ_CONTROL,
    WRITE_DAC,
};
use windows_sys::Win32::System::SystemServices::SECURITY_DESCRIPTOR_REVISION;

struct Allocation(*mut core::ffi::c_void);

impl Drop for Allocation {
    fn drop(&mut self) {
        unsafe { LocalFree(self.0) };
    }
}

struct PackageAllowance {
    file: File,
    original: Vec<u32>,
    active: bool,
}

impl PackageAllowance {
    fn new(path: &Path) -> Self {
        let file = OpenOptions::new()
            .access_mode(READ_CONTROL | WRITE_DAC)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
            .open(path)
            .unwrap();
        let original = descriptor(&file);
        let mut sid = null_mut();
        let encoded: Vec<u16> = "S-1-15-2-1\0".encode_utf16().collect();
        assert_ne!(
            unsafe { ConvertStringSidToSidW(encoded.as_ptr(), &mut sid) },
            0
        );
        let _sid = Allocation(sid);
        let entry = EXPLICIT_ACCESS_W {
            grfAccessPermissions: FILE_ALL_ACCESS,
            grfAccessMode: GRANT_ACCESS,
            grfInheritance: 0,
            Trustee: TRUSTEE_W {
                TrusteeForm: TRUSTEE_IS_SID,
                ptstrName: sid.cast(),
                ..Default::default()
            },
        };
        let mut merged = null_mut();
        assert_eq!(
            unsafe { SetEntriesInAclW(1, &entry, dacl(&original), &mut merged) },
            0
        );
        let _merged = Allocation(merged.cast());
        write_dacl(&file, &original, merged).unwrap();
        Self {
            file,
            original,
            active: true,
        }
    }

    fn close(&mut self) -> std::io::Result<()> {
        if self.active {
            write_dacl(&self.file, &self.original, dacl(&self.original))?;
            self.active = false;
        }
        Ok(())
    }
}

impl Drop for PackageAllowance {
    fn drop(&mut self) {
        if let Err(error) = self.close() {
            eprintln!("fixture package-allowance restoration failed: {error}");
        }
    }
}

fn descriptor(file: &File) -> Vec<u32> {
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
    let mut words = vec![0u32; (needed as usize).div_ceil(4)];
    let status = unsafe {
        NtQuerySecurityObject(
            file.as_raw_handle(),
            DACL_SECURITY_INFORMATION,
            words.as_mut_ptr().cast(),
            needed,
            &mut needed,
        )
    };
    assert!(status >= 0, "fixture descriptor query failed: {status:#x}");
    words
}

fn dacl(words: &[u32]) -> *mut ACL {
    let mut present = 0;
    let mut defaulted = 0;
    let mut acl = null_mut();
    assert_ne!(
        unsafe {
            GetSecurityDescriptorDacl(
                words.as_ptr().cast_mut().cast(),
                &mut present,
                &mut acl,
                &mut defaulted,
            )
        },
        0
    );
    assert_ne!(present, 0);
    assert!(!acl.is_null());
    acl
}

fn write_dacl(file: &File, original: &[u32], acl: *mut ACL) -> std::io::Result<()> {
    let mut control = 0;
    let mut revision = 0;
    assert_ne!(
        unsafe {
            GetSecurityDescriptorControl(
                original.as_ptr().cast_mut().cast(),
                &mut control,
                &mut revision,
            )
        },
        0
    );
    let mut descriptor = SECURITY_DESCRIPTOR::default();
    let raw = (&mut descriptor as *mut SECURITY_DESCRIPTOR).cast();
    assert_ne!(
        unsafe { InitializeSecurityDescriptor(raw, SECURITY_DESCRIPTOR_REVISION) },
        0
    );
    assert_ne!(
        unsafe {
            SetSecurityDescriptorDacl(raw, 1, acl, i32::from(control & SE_DACL_DEFAULTED != 0))
        },
        0
    );
    let preserve = SE_DACL_PROTECTED | SE_DACL_AUTO_INHERITED;
    assert_ne!(
        unsafe { SetSecurityDescriptorControl(raw, preserve, control & preserve) },
        0
    );
    let status =
        unsafe { NtSetSecurityObject(file.as_raw_handle(), DACL_SECURITY_INFORMATION, raw) };
    if status < 0 {
        Err(std::io::Error::from_raw_os_error(
            unsafe { RtlNtStatusToDosError(status) } as i32,
        ))
    } else {
        Ok(())
    }
}

pub(super) fn baseline_worker(root: &Path) -> std::io::Result<()> {
    let scope = root.join("scope");
    if std::fs::read_to_string(scope.join("secret/leaf.txt"))? != "original" {
        return Err(std::io::Error::other("Baseline hidden read changed"));
    }
    std::fs::write(scope.join("protected/leaf.txt"), "baseline")?;
    std::fs::write(scope.join("writable/leaf.txt"), "baseline")?;
    std::fs::write(scope.join("protected/new.txt"), "baseline")?;
    std::fs::remove_file(scope.join("protected/new.txt"))?;
    // These controls have no package ACE; deletion needs the parent's allowance.
    std::fs::remove_file(scope.join("protected/delete.txt"))?;
    std::fs::remove_file(scope.join("writable/delete.txt"))?;
    Ok(())
}

fn reset_files(scope: &Path) {
    for name in ["writable", "protected", "secret"] {
        std::fs::write(scope.join(name).join("leaf.txt"), "original").unwrap();
    }
    for name in ["writable", "protected"] {
        std::fs::write(scope.join(name).join("delete.txt"), "delete control").unwrap();
    }
}

async fn run_probe(root: &Path, role: &str, with_policy: bool) {
    let mut profile = Profile::new().unwrap();
    let (program, grants) = setup(root, &profile);
    let scope = root.join("scope");
    let policy = ExistingTreePolicy {
        access: Access::Write,
        read_only: vec![scope.join("protected"), scope.join("secret")],
        unreadable: vec![scope.join("secret")],
    };
    let mut tree = with_policy.then(|| {
        profile
            .grant_existing_tree_policy(&scope, &policy, 9)
            .unwrap()
    });
    let env = environment(root, role, &profile);
    let child = spawn(&profile, &program, &arguments(), &env, root).unwrap();
    let code = tokio::time::timeout(Duration::from_secs(10), child.wait_owned())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        code,
        0,
        "{}",
        std::fs::read_to_string(root.join("write.txt")).unwrap()
    );
    if let Some(tree) = &mut tree {
        tree.close().unwrap();
    }
    for grant in grants {
        grant.close().unwrap();
    }
    profile.close().unwrap();
}

#[tokio::test]
async fn exclusion_denies_override_broad_package_allowances() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let scope = root.join("scope");
    std::fs::create_dir(&scope).unwrap();
    let mut allowances = vec![PackageAllowance::new(&scope)];
    for name in ["writable", "protected", "secret"] {
        let folder = scope.join(name);
        std::fs::create_dir(&folder).unwrap();
        std::fs::write(folder.join("leaf.txt"), "original").unwrap();
        allowances.push(PackageAllowance::new(&folder));
        allowances.push(PackageAllowance::new(&folder.join("leaf.txt")));
    }
    reset_files(&scope);
    run_probe(root, "package-baseline", false).await;
    assert_eq!(
        std::fs::read_to_string(scope.join("protected/leaf.txt")).unwrap(),
        "baseline"
    );
    assert!(!scope.join("protected/delete.txt").exists());
    assert!(!scope.join("writable/delete.txt").exists());
    reset_files(&scope);
    run_probe(root, "existing-policy", true).await;
    assert_eq!(
        std::fs::read_to_string(scope.join("writable/leaf.txt")).unwrap(),
        "policy allowed"
    );
    assert_eq!(
        std::fs::read_to_string(scope.join("protected/leaf.txt")).unwrap(),
        "original"
    );
    assert_eq!(
        std::fs::read_to_string(scope.join("secret/leaf.txt")).unwrap(),
        "original"
    );
    assert!(scope.join("protected/delete.txt").exists());
    assert!(!scope.join("writable/delete.txt").exists());
    assert!(!scope.join("protected/new.txt").exists());
    // Successful cleanup must leave the unrelated package allowance usable.
    reset_files(&scope);
    run_probe(root, "package-baseline", false).await;
    assert_eq!(
        std::fs::read_to_string(scope.join("protected/leaf.txt")).unwrap(),
        "baseline"
    );
    for allowance in allowances.iter_mut().rev() {
        allowance.close().unwrap();
    }
}
