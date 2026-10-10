//! Create-only native objects with explicit security before any content is written.
use super::*;
use std::ptr::null;
use windows_sys::Wdk::Foundation::OBJECT_ATTRIBUTES;
use windows_sys::Wdk::Storage::FileSystem::{
    FILE_CREATE, FILE_DIRECTORY_FILE, FILE_NON_DIRECTORY_FILE, FILE_OPEN_REPARSE_POINT,
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
    FILE_ALL_ACCESS, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
};
use windows_sys::Win32::System::IO::IO_STATUS_BLOCK;

/// Creates a private child of a retained caller-selected directory. Parent routing/binding
/// remains the caller's responsibility; this never opens or repairs an existing child.
pub fn create_private_directory(parent: &File, name: &str) -> Result<File, MemoryStorageError> {
    create(parent, name, true)
}
/// Creates a private empty file under an already verified private directory.
pub fn create_private_file(parent: &File, name: &str) -> Result<File, MemoryStorageError> {
    verify_private(parent)?;
    create(parent, name, false)
}
fn component(name: &str) -> Result<Vec<u16>, MemoryStorageError> {
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
fn create(parent: &File, name: &str, directory: bool) -> Result<File, MemoryStorageError> {
    let mut name = component(name)?;
    identity(parent)?;
    if !parent.metadata()?.is_dir() {
        return Err(refusal("expected a native memory parent directory"));
    }
    let descriptor = private_descriptor()?;
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
        SecurityDescriptor: descriptor.allocation.cast(),
        SecurityQualityOfService: null(),
    };
    let mut status = IO_STATUS_BLOCK::default();
    let mut handle = null_mut();
    // One bounded component is resolved under the retained handle; FILE_CREATE cannot
    // open/truncate an existing child. The protected descriptor is installed atomically.
    let result = unsafe {
        NtCreateFile(
            &mut handle,
            FILE_ALL_ACCESS,
            &attributes,
            &mut status,
            null(),
            FILE_ATTRIBUTE_NORMAL,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            FILE_CREATE,
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
        return Err(refusal("native memory creation returned no handle"));
    }
    let file = unsafe { File::from_raw_handle(handle) };
    if result != 0 || status.Information != 2 {
        return Err(refusal(
            "native memory creation did not report a fresh object",
        ));
    }
    verify_private(&file)?;
    if file.metadata()?.is_dir() != directory {
        return Err(refusal(
            "native memory creation returned the wrong object type",
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
        }
        assert!(create_private_directory(&parent, &"a".repeat(256)).is_err());
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
    }
}
