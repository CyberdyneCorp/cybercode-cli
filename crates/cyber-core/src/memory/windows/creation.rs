//! Handle-relative private child creation and opening, verified before caller access.
use super::*;
use std::ptr::null;
use windows_sys::Wdk::Foundation::OBJECT_ATTRIBUTES;
use windows_sys::Wdk::Storage::FileSystem::{
    FILE_CREATE, FILE_DIRECTORY_FILE, FILE_NON_DIRECTORY_FILE, FILE_OPEN, FILE_OPEN_REPARSE_POINT,
    FILE_SYNCHRONOUS_IO_NONALERT, NtCreateFile,
};
use windows_sys::Win32::Foundation::{
    INVALID_HANDLE_VALUE, OBJ_CASE_INSENSITIVE, OBJ_DONT_REPARSE, RtlNtStatusToDosError,
    UNICODE_STRING,
};
use windows_sys::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
};
use windows_sys::Win32::Storage::FileSystem::{
    FILE_ALL_ACCESS, FILE_ATTRIBUTE_NORMAL, FILE_GENERIC_READ, FILE_GENERIC_WRITE,
    FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
};
use windows_sys::Win32::System::IO::IO_STATUS_BLOCK;

/// Creates a private child of a retained caller-selected directory. Parent routing/binding
/// remains the caller's responsibility; this never opens or repairs an existing child.
pub fn create_private_directory(parent: &File, name: &str) -> Result<File, MemoryStorageError> {
    child(parent, name, true, Mode::Create)
}
/// Creates a private empty file under an already verified private directory.
pub fn create_private_file(parent: &File, name: &str) -> Result<File, MemoryStorageError> {
    verify_private(parent)?;
    child(parent, name, false, Mode::Create)
}
/// Access requested for an existing private child. Read mode grants no data writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Access {
    Read,
    Write,
    /// Read/write content without delete or ACL mutation authority.
    DataWrite,
}

/// An existing private directory whose name cannot be deleted/renamed while held.
pub fn open_pinned_private_directory(
    parent: &File,
    name: &str,
    writable: bool,
) -> Result<File, MemoryStorageError> {
    child(
        parent,
        name,
        true,
        Mode::Pinned(if writable {
            Access::DataWrite
        } else {
            Access::Read
        }),
    )
}
/// An existing private lock pinned to its name; caller still acquires the OS file lock.
pub fn open_pinned_private_file(parent: &File, name: &str) -> Result<File, MemoryStorageError> {
    verify_private(parent)?;
    child(parent, name, false, Mode::Pinned(Access::DataWrite))
}
/// Read-only exact-object ownership denying other content writes and namespace changes.
pub fn freeze_private_file(
    parent: &File,
    name: &str,
    expected: FileIdentity,
) -> Result<File, MemoryStorageError> {
    verify_private(parent)?;
    let file = child(parent, name, false, Mode::Frozen)?;
    if identity(&file)? != expected {
        return Err(MemoryStorageError::ReviewConflict);
    }
    Ok(file)
}

/// Open an existing private directory under its retained parent, without creation or ACL repair.
pub fn open_private_directory(
    parent: &File,
    name: &str,
    access: Access,
) -> Result<File, MemoryStorageError> {
    child(parent, name, true, Mode::Open(access))
}
/// Open an existing private regular file under a private parent, without truncation or repair.
pub fn open_private_file(
    parent: &File,
    name: &str,
    access: Access,
) -> Result<File, MemoryStorageError> {
    verify_private(parent)?;
    child(parent, name, false, Mode::Open(access))
}
#[derive(Clone, Copy)]
pub(super) enum Mode {
    Create,
    Open(Access),
    Pinned(Access),
    Frozen,
    Retain,
}
impl Mode {
    fn disposition(self) -> u32 {
        match self {
            Self::Create => FILE_CREATE,
            Self::Open(_) | Self::Pinned(_) | Self::Frozen | Self::Retain => FILE_OPEN,
        }
    }
    fn access(self) -> u32 {
        match self {
            Self::Open(Access::Read) | Self::Pinned(Access::Read) | Self::Frozen => {
                FILE_GENERIC_READ
            }
            Self::Open(Access::DataWrite) | Self::Pinned(Access::DataWrite) => {
                FILE_GENERIC_READ | FILE_GENERIC_WRITE
            }
            _ => FILE_ALL_ACCESS,
        }
    }
    fn information(self) -> usize {
        match self {
            Self::Create => 2,
            Self::Open(_) | Self::Pinned(_) | Self::Frozen | Self::Retain => 1,
        }
    }
}
pub(super) fn component(name: &str) -> Result<Vec<u16>, MemoryStorageError> {
    if name.is_empty()
        || name.len() > 255
        || name == "."
        || name == ".."
        || name.ends_with('.')
        || !name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
    {
        return Err(refusal("invalid native memory child name"));
    }
    let base = name
        .split('.')
        .next()
        .unwrap_or_default()
        .to_ascii_uppercase();
    let numbered = base
        .strip_prefix("COM")
        .or_else(|| base.strip_prefix("LPT"));
    if matches!(base.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || numbered.is_some_and(|s| s.len() == 1 && matches!(s.as_bytes()[0], b'1'..=b'9'))
    {
        return Err(refusal("reserved native memory child name"));
    }
    Ok(name.encode_utf16().collect())
}
fn private_descriptor() -> Result<Descriptor, MemoryStorageError> {
    let user = process_user()?;
    let mut raw = null_mut();
    if unsafe { ConvertSidToStringSidW(user.as_ptr().cast_mut().cast(), &mut raw) } == 0 {
        return Err(io::Error::last_os_error().into());
    }
    let allocation = Descriptor {
        allocation: raw.cast(),
        owner: null_mut(),
        acl: null_mut(),
    };
    // A valid SID string has at most 184 characters; the API returns a terminated allocation.
    let mut length = 0;
    while unsafe { *raw.add(length) } != 0 {
        length += 1;
        if length > 184 {
            return Err(refusal("current-user SID string exceeds its bound"));
        }
    }
    let sid = String::from_utf16(unsafe { std::slice::from_raw_parts(raw, length) })
        .map_err(|_| refusal("invalid current-user SID string"))?;
    drop(allocation);
    let sddl = format!("O:{sid}D:P(A;OICI;FA;;;{sid})(A;OICI;FA;;;SY)");
    let sddl: Vec<u16> = sddl.encode_utf16().chain(Some(0)).collect();
    let mut descriptor = Descriptor {
        allocation: null_mut(),
        owner: null_mut(),
        acl: null_mut(),
    };
    if unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(),
            1,
            &mut descriptor.allocation,
            null_mut(),
        )
    } == 0
    {
        return Err(io::Error::last_os_error().into());
    }
    Ok(descriptor)
}
pub(super) fn child(
    parent: &File,
    name: &str,
    directory: bool,
    mode: Mode,
) -> Result<File, MemoryStorageError> {
    let mut name = component(name)?;
    identity(parent)?;
    if !parent.metadata()?.is_dir() {
        return Err(refusal("expected a native memory parent directory"));
    }
    let descriptor = match mode {
        Mode::Create => Some(private_descriptor()?),
        Mode::Open(_) | Mode::Pinned(_) | Mode::Frozen | Mode::Retain => None,
    };
    let unicode = UNICODE_STRING {
        Length: (name.len() * 2) as u16,
        MaximumLength: (name.len() * 2) as u16,
        Buffer: name.as_mut_ptr(),
    };
    let attributes = OBJECT_ATTRIBUTES {
        Length: std::mem::size_of::<OBJECT_ATTRIBUTES>() as u32,
        RootDirectory: parent.as_raw_handle(),
        ObjectName: &unicode,
        Attributes: OBJ_CASE_INSENSITIVE | OBJ_DONT_REPARSE,
        SecurityDescriptor: descriptor
            .as_ref()
            .map_or(null(), |sd| sd.allocation.cast()),
        SecurityQualityOfService: null(),
    };
    let mut status = IO_STATUS_BLOCK::default();
    let mut handle = null_mut();
    // One bounded component is resolved under the retained handle. FILE_CREATE
    // installs explicit security; FILE_OPEN neither creates nor repairs existing objects.
    let result = unsafe {
        NtCreateFile(
            &mut handle,
            mode.access(),
            &attributes,
            &mut status,
            null(),
            FILE_ATTRIBUTE_NORMAL,
            if matches!(mode, Mode::Retain | Mode::Frozen) {
                FILE_SHARE_READ
            } else if matches!(mode, Mode::Pinned(_)) {
                FILE_SHARE_READ | FILE_SHARE_WRITE
            } else {
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE
            },
            mode.disposition(),
            FILE_OPEN_REPARSE_POINT
                | FILE_SYNCHRONOUS_IO_NONALERT
                | if directory {
                    FILE_DIRECTORY_FILE
                } else {
                    FILE_NON_DIRECTORY_FILE
                },
            null(),
            0,
        )
    };
    if result < 0 {
        return Err(
            io::Error::from_raw_os_error(unsafe { RtlNtStatusToDosError(result) } as i32).into(),
        );
    }
    if handle.is_null() || handle == INVALID_HANDLE_VALUE {
        return Err(refusal("native memory acquisition returned no handle"));
    }
    let file = unsafe { File::from_raw_handle(handle) };
    if result != 0 || status.Information != mode.information() {
        return Err(refusal(
            "native memory acquisition returned unexpected disposition",
        ));
    }
    verify_private(&file)?;
    if file.metadata()?.is_dir() != directory {
        return Err(refusal(
            "native memory acquisition returned the wrong object type",
        ));
    }
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
    };
    fn parent(path: &std::path::Path) -> File {
        std::fs::OpenOptions::new()
            .access_mode(FILE_ALL_ACCESS)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
            .open(path)
            .unwrap()
    }
    #[test]
    fn native_creation_installs_private_security_before_bytes_and_never_repairs_existing_children()
    {
        use std::io::{Read, Seek, Write};
        let root = tempfile::tempdir().unwrap();
        let parent = parent(root.path());
        let directory = create_private_directory(&parent, "memory").unwrap();
        verify_private(&directory).unwrap();
        let mut file = create_private_file(&directory, "policy.md").unwrap();
        verify_private(&file).unwrap();
        file.write_all(b"private data").unwrap();
        file.sync_all().unwrap();
        assert!(create_private_file(&directory, "POLICY.md").is_err());
        assert!(create_private_directory(&parent, "memory").is_err());
        file.rewind().unwrap();
        let mut text = String::new();
        file.read_to_string(&mut text).unwrap();
        assert_eq!(text, "private data");
        std::fs::write(root.path().join("existing"), b"user edits").unwrap();
        assert!(create_private_directory(&parent, "existing").is_err());
        assert_eq!(
            std::fs::read(root.path().join("existing")).unwrap(),
            b"user edits"
        );
    }
    #[test]
    fn native_creation_remains_bound_to_moved_parent_and_refuses_foreign_or_reparse_parents() {
        let root = tempfile::tempdir().unwrap();
        let original = root.path().join("original");
        std::fs::create_dir(&original).unwrap();
        let retained = parent(&original);
        let moved = root.path().join("moved");
        std::fs::rename(&original, &moved).unwrap();
        std::fs::create_dir(&original).unwrap();
        let directory = create_private_directory(&retained, "memory").unwrap();
        assert!(moved.join("memory").exists());
        assert!(!original.join("memory").exists());
        let broad = parent(&original);
        assert!(create_private_file(&broad, "private.md").is_err());
        assert!(!original.join("private.md").exists());
        let file = create_private_file(&directory, "regular").unwrap();
        assert!(create_private_directory(&file, "child").is_err());
        let junction = root.path().join("junction");
        assert!(
            std::process::Command::new("cmd")
                .args(["/C", "mklink", "/J"])
                .arg(&junction)
                .arg(&original)
                .output()
                .unwrap()
                .status
                .success()
        );
        assert!(create_private_directory(&parent(&junction), "escape").is_err());
        assert!(!original.join("escape").exists());
    }
    #[test]
    fn native_open_preserves_existing_bytes_and_enforces_read_only_access() {
        use std::io::{Read, Write};
        let root = tempfile::tempdir().unwrap();
        let directory = create_private_directory(&parent(root.path()), "memory").unwrap();
        let mut created = create_private_file(&directory, "note.md").unwrap();
        created.write_all(b"original").unwrap();
        created.sync_all().unwrap();
        let opened = open_private_directory(&parent(root.path()), "MEMORY", Access::Read).unwrap();
        assert_eq!(identity(&opened).unwrap(), identity(&directory).unwrap());
        let mut read = open_private_file(&opened, "NOTE.md", Access::Read).unwrap();
        let mut text = String::new();
        read.read_to_string(&mut text).unwrap();
        assert_eq!(text, "original");
        assert!(read.write_all(b"overwrite").is_err());
        assert_eq!(
            std::fs::read(root.path().join("memory/note.md")).unwrap(),
            b"original"
        );
        let mut write = open_private_file(&directory, "note.md", Access::Write).unwrap();
        assert_eq!(identity(&write).unwrap(), identity(&created).unwrap());
        write.write_all(b"changed!").unwrap();
        write.sync_all().unwrap();
        assert_eq!(
            std::fs::read(root.path().join("memory/note.md")).unwrap(),
            b"changed!"
        );
    }
    #[test]
    fn native_open_uses_retained_moved_parent_and_does_not_create_missing_children() {
        use std::io::Write;
        let root = tempfile::tempdir().unwrap();
        let bootstrap = parent(root.path());
        let directory = create_private_directory(&bootstrap, "original").unwrap();
        let mut note = create_private_file(&directory, "note.md").unwrap();
        note.write_all(b"retained").unwrap();
        let expected = identity(&note).unwrap();
        // Windows refuses parent renames with an open descendant file.
        drop(note);
        std::fs::rename(root.path().join("original"), root.path().join("moved")).unwrap();
        let replacement = create_private_directory(&bootstrap, "original").unwrap();
        create_private_file(&replacement, "note.md")
            .unwrap()
            .write_all(b"replacement")
            .unwrap();
        let opened = open_private_file(&directory, "note.md", Access::Read).unwrap();
        assert_eq!(identity(&opened).unwrap(), expected);
        for access in [Access::Read, Access::Write] {
            assert!(open_private_file(&directory, "missing", access).is_err());
            assert!(open_private_directory(&directory, "missing", access).is_err());
        }
        assert!(!root.path().join("moved/missing").exists());
        assert_eq!(
            std::fs::read(root.path().join("original/note.md")).unwrap(),
            b"replacement"
        );
    }
    #[test]
    fn native_open_refuses_broad_children_wrong_types_and_hard_links_without_repair() {
        let root = tempfile::tempdir().unwrap();
        let bootstrap = parent(root.path());
        let directory = create_private_directory(&bootstrap, "memory").unwrap();
        create_private_directory(&directory, "nested").unwrap();
        create_private_file(&directory, "regular").unwrap();
        // Default inherited ACLs are intentionally unprotected and must not be repaired.
        std::fs::write(root.path().join("memory/broad"), b"user bytes").unwrap();
        std::fs::create_dir(root.path().join("broad-directory")).unwrap();
        let broad = parent(&root.path().join("memory/broad"));
        let security_before = super::super::tests::security(&broad);
        std::fs::hard_link(
            root.path().join("memory/regular"),
            root.path().join("alias"),
        )
        .unwrap();
        for access in [Access::Read, Access::Write] {
            assert!(open_private_file(&directory, "nested", access).is_err());
            assert!(open_private_directory(&directory, "regular", access).is_err());
            assert!(open_private_file(&directory, "regular", access).is_err());
            assert!(open_private_file(&directory, "broad", access).is_err());
            assert!(open_private_directory(&bootstrap, "broad-directory", access).is_err());
            assert!(open_private_file(&bootstrap, "alias", access).is_err());
        }
        assert_eq!(
            std::fs::read(root.path().join("memory/broad")).unwrap(),
            b"user bytes"
        );
        assert_eq!(std::fs::read(root.path().join("alias")).unwrap(), b"");
        assert_eq!(super::super::tests::security(&broad), security_before);
    }
    #[test]
    fn native_open_refuses_junction_children_without_following_them() {
        let root = tempfile::tempdir().unwrap();
        let directory = create_private_directory(&parent(root.path()), "memory").unwrap();
        let outside = root.path().join("outside");
        std::fs::create_dir(&outside).unwrap();
        std::fs::write(outside.join("user.md"), b"external edits").unwrap();
        let output = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(root.path().join("memory").join("junction"))
            .arg(&outside)
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        for access in [Access::Read, Access::Write] {
            assert!(open_private_directory(&directory, "junction", access).is_err());
            assert!(open_private_file(&directory, "junction", access).is_err());
        }
        assert_eq!(
            std::fs::read(outside.join("user.md")).unwrap(),
            b"external edits"
        );
        assert_eq!(std::fs::read_dir(&outside).unwrap().count(), 1);
    }
    #[test]
    fn invalid_components_never_reach_native_creation() {
        let root = tempfile::tempdir().unwrap();
        let parent = parent(root.path());
        for name in [
            "",
            ".",
            "..",
            "../escape",
            "sub/escape",
            "sub\\escape",
            "note:stream",
            "CON",
            "nul.md",
            "COM1",
            "lpt9.txt",
            "trailing.",
            "trailing ",
            "\0",
            "é",
        ] {
            assert!(create_private_directory(&parent, name).is_err(), "{name:?}");
            assert!(
                open_private_directory(&parent, name, Access::Read).is_err(),
                "{name:?}"
            );
        }
        assert!(create_private_directory(&parent, &"a".repeat(256)).is_err());
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
    }
}
