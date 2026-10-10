//! Exclusive source ownership and create-only namespace installation.
use super::*;
use std::mem::{offset_of, size_of};
use windows_sys::Wdk::Storage::FileSystem::{
    FILE_RENAME_INFORMATION, FILE_RENAME_INFORMATION_0, FileRenameInformation, NtSetInformationFile,
};
use windows_sys::Win32::Foundation::RtlNtStatusToDosError;
use windows_sys::Win32::System::IO::IO_STATUS_BLOCK;

/// A synchronous source handle denying other writers and deletion until disposal.
/// Callers must review content through this retained handle before installation.
pub struct RetainedChild {
    file: File,
    identity: FileIdentity,
    directory: bool,
}

pub fn retain_private_file(
    parent: &File,
    name: &str,
    expected: FileIdentity,
) -> Result<RetainedChild, MemoryStorageError> {
    retain(parent, name, expected, false)
}
pub fn retain_private_directory(
    parent: &File,
    name: &str,
    expected: FileIdentity,
) -> Result<RetainedChild, MemoryStorageError> {
    retain(parent, name, expected, true)
}
fn retain(
    parent: &File,
    name: &str,
    expected: FileIdentity,
    directory: bool,
) -> Result<RetainedChild, MemoryStorageError> {
    verify_private(parent)?;
    let file = creation::child(parent, name, directory, creation::Mode::Retain)?;
    if identity(&file)? != expected {
        return Err(refusal("native memory source identity changed"));
    }
    Ok(RetainedChild {
        file,
        identity: expected,
        directory,
    })
}
impl RetainedChild {
    pub fn file(&self) -> &File {
        &self.file
    }

    /// Renames this exact retained object without replacing any destination.
    /// This is not a durability acknowledgement; callers retain journal evidence.
    pub fn rename_to(&self, destination: &File, name: &str) -> Result<(), MemoryStorageError> {
        let name = creation::component(name)?;
        let destination_identity = verify_private(destination)?;
        if !destination.metadata()?.is_dir() {
            return Err(refusal("expected a native memory destination directory"));
        }
        if verify_private(&self.file)? != self.identity
            || self.identity.volume != destination_identity.volume
        {
            return Err(refusal("native memory rename identity or volume changed"));
        }
        let buffer = rename_information(destination, &name);
        let mut status = IO_STATUS_BLOCK::default();
        // Only this private constructor can supply the source: NtCreateFile opened it
        // synchronously, so neither native IO nor its buffers can outlive this call.
        let result = unsafe {
            NtSetInformationFile(
                self.file.as_raw_handle(),
                &mut status,
                buffer.as_ptr().cast(),
                (size_of::<FILE_RENAME_INFORMATION>() + size_of_val(name.as_slice())) as u32,
                FileRenameInformation,
            )
        };
        if result < 0 {
            return Err(io::Error::from_raw_os_error(
                unsafe { RtlNtStatusToDosError(result) } as i32
            )
            .into());
        }
        if result != 0 {
            return Err(refusal("native memory rename returned unknown settlement"));
        }
        let name = String::from_utf16(&name)
            .map_err(|_| refusal("invalid native memory destination name"))?;
        let installed = if self.directory {
            open_private_directory(destination, &name, Access::Read)?
        } else {
            open_private_file(destination, &name, Access::Read)?
        };
        if identity(&installed)? != self.identity || verify_private(&self.file)? != self.identity {
            return Err(refusal("native memory rename destination identity changed"));
        }
        Ok(())
    }
}

fn rename_information(destination: &File, name: &[u16]) -> Vec<usize> {
    // Pointer alignment covers the native header. Allocate the full header plus the
    // counted UTF-16 name, including native tail padding, as required by the API.
    let bytes = size_of::<FILE_RENAME_INFORMATION>() + size_of_val(name);
    let mut buffer = vec![0usize; bytes.div_ceil(size_of::<usize>())];
    let header = buffer.as_mut_ptr().cast::<FILE_RENAME_INFORMATION>();
    unsafe {
        header.write(FILE_RENAME_INFORMATION {
            Anonymous: FILE_RENAME_INFORMATION_0 {
                ReplaceIfExists: false,
            },
            RootDirectory: destination.as_raw_handle(),
            FileNameLength: size_of_val(name) as u32,
            FileName: [0],
        });
        std::ptr::copy_nonoverlapping(
            name.as_ptr(),
            buffer
                .as_mut_ptr()
                .cast::<u8>()
                .add(offset_of!(FILE_RENAME_INFORMATION, FileName))
                .cast::<u16>(),
            name.len(),
        );
    }
    buffer
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_ALL_ACCESS, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
    };

    fn parent(path: &std::path::Path) -> File {
        std::fs::OpenOptions::new()
            .access_mode(FILE_ALL_ACCESS)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
            .open(path)
            .unwrap()
    }
    fn staged(directory: &File, name: &str, bytes: &[u8]) -> FileIdentity {
        let mut file = create_private_file(directory, name).unwrap();
        file.write_all(bytes).unwrap();
        file.sync_all().unwrap();
        identity(&file).unwrap()
    }
    #[test]
    fn rename_buffer_preserves_native_alignment_counted_name_and_no_replace_flag() {
        let root = tempfile::tempdir().unwrap();
        let destination = parent(root.path());
        for length in [1, 255] {
            let name = creation::component(&"a".repeat(length)).unwrap();
            let buffer = rename_information(&destination, &name);
            let header = unsafe { &*buffer.as_ptr().cast::<FILE_RENAME_INFORMATION>() };
            assert_eq!(
                (buffer.as_ptr() as usize) % std::mem::align_of::<FILE_RENAME_INFORMATION>(),
                0
            );
            assert!(!unsafe { header.Anonymous.ReplaceIfExists });
            assert_eq!(header.RootDirectory, destination.as_raw_handle());
            assert_eq!(header.FileNameLength as usize, size_of_val(name.as_slice()));
            let stored = unsafe {
                std::slice::from_raw_parts(
                    buffer
                        .as_ptr()
                        .cast::<u8>()
                        .add(offset_of!(FILE_RENAME_INFORMATION, FileName))
                        .cast::<u16>(),
                    name.len(),
                )
            };
            assert_eq!(stored, name);
            assert!(
                size_of_val(buffer.as_slice())
                    >= size_of::<FILE_RENAME_INFORMATION>() + size_of_val(name.as_slice())
            );
        }
    }
    #[test]
    fn native_retained_source_denies_competing_writes_and_moves_without_replacement() {
        let root = tempfile::tempdir().unwrap();
        let directory = create_private_directory(&parent(root.path()), "memory").unwrap();
        let expected = staged(&directory, "staged", b"private bytes");
        let retained = retain_private_file(&directory, "staged", expected).unwrap();
        let source = root.path().join("memory/staged");
        assert!(
            std::fs::OpenOptions::new()
                .write(true)
                .open(&source)
                .is_err()
        );
        assert!(std::fs::remove_file(&source).is_err());
        assert!(std::fs::rename(&source, root.path().join("memory/unexpected")).is_err());
        let mut text = String::new();
        retained.file().read_to_string(&mut text).unwrap();
        assert_eq!(text, "private bytes");
        retained.rename_to(&directory, "installed").unwrap();
        assert!(!source.exists());
        assert_eq!(
            std::fs::read(root.path().join("memory/installed")).unwrap(),
            b"private bytes"
        );
        let installed = open_private_file(&directory, "installed", Access::Read).unwrap();
        assert_eq!(identity(&installed).unwrap(), expected);
        drop(installed);
        drop(retained);
        open_private_file(&directory, "installed", Access::Write).unwrap();
    }
    #[test]
    fn native_rename_collision_and_invalid_names_preserve_all_evidence() {
        let root = tempfile::tempdir().unwrap();
        let directory = create_private_directory(&parent(root.path()), "memory").unwrap();
        let expected = staged(&directory, "staged", b"staged bytes");
        staged(&directory, "existing", b"user edits");
        let retained = retain_private_file(&directory, "staged", expected).unwrap();
        let security = super::super::tests::security(retained.file());
        assert!(retained.rename_to(&directory, "EXISTING").is_err());
        for name in [
            "",
            "..",
            "../escape",
            "sub/escape",
            "note:stream",
            "CON",
            "end.",
        ] {
            assert!(retained.rename_to(&directory, name).is_err(), "{name:?}");
        }
        assert!(retained.rename_to(&directory, &"a".repeat(256)).is_err());
        assert_eq!(super::super::tests::security(retained.file()), security);
        assert_eq!(
            std::fs::read(root.path().join("memory/staged")).unwrap(),
            b"staged bytes"
        );
        assert_eq!(
            std::fs::read(root.path().join("memory/existing")).unwrap(),
            b"user edits"
        );
        assert_eq!(
            std::fs::read_dir(root.path().join("memory"))
                .unwrap()
                .count(),
            2
        );
    }
    #[test]
    fn native_retention_refuses_replaced_or_aliased_sources_before_effects() {
        let root = tempfile::tempdir().unwrap();
        let directory = create_private_directory(&parent(root.path()), "memory").unwrap();
        let expected = staged(&directory, "source", b"original");
        std::fs::rename(
            root.path().join("memory/source"),
            root.path().join("memory/original"),
        )
        .unwrap();
        staged(&directory, "source", b"replacement");
        assert!(retain_private_file(&directory, "source", expected).is_err());
        std::fs::hard_link(
            root.path().join("memory/original"),
            root.path().join("alias"),
        )
        .unwrap();
        assert!(retain_private_file(&directory, "original", expected).is_err());
        std::fs::write(root.path().join("memory/broad"), b"broad bytes").unwrap();
        assert!(retain_private_file(&directory, "broad", expected).is_err());
        assert_eq!(
            std::fs::read(root.path().join("memory/source")).unwrap(),
            b"replacement"
        );
        assert_eq!(
            std::fs::read(root.path().join("alias")).unwrap(),
            b"original"
        );
        assert_eq!(
            std::fs::read(root.path().join("memory/broad")).unwrap(),
            b"broad bytes"
        );
    }
    #[test]
    fn native_rename_refuses_junction_destination_evidence_and_keeps_source() {
        let root = tempfile::tempdir().unwrap();
        let bootstrap = parent(root.path());
        let directory = create_private_directory(&bootstrap, "memory").unwrap();
        let outside = root.path().join("outside");
        std::fs::create_dir(&outside).unwrap();
        std::fs::write(outside.join("user"), b"outside bytes").unwrap();
        let junction = root.path().join("memory").join("junction");
        let output = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(&junction)
            .arg(&outside)
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        let expected = staged(&directory, "source", b"source bytes");
        let retained = retain_private_file(&directory, "source", expected).unwrap();
        assert!(retained.rename_to(&parent(&junction), "escape").is_err());
        assert!(retained.rename_to(&directory, "junction").is_err());
        assert!(retain_private_directory(&directory, "junction", expected).is_err());
        assert_eq!(
            std::fs::read(root.path().join("memory/source")).unwrap(),
            b"source bytes"
        );
        assert_eq!(
            std::fs::read(outside.join("user")).unwrap(),
            b"outside bytes"
        );
        assert_eq!(std::fs::read_dir(&outside).unwrap().count(), 1);
    }
    #[test]
    fn native_directory_rename_stays_under_retained_destination_and_refuses_broad_parents() {
        let root = tempfile::tempdir().unwrap();
        let bootstrap = parent(root.path());
        let source = create_private_directory(&bootstrap, "source").unwrap();
        let child = create_private_directory(&source, "journal").unwrap();
        let expected = identity(&child).unwrap();
        drop(child);
        let destination = create_private_directory(&bootstrap, "destination").unwrap();
        std::fs::rename(root.path().join("destination"), root.path().join("moved")).unwrap();
        std::fs::create_dir(root.path().join("destination")).unwrap();
        let retained = retain_private_directory(&source, "journal", expected).unwrap();
        assert!(retained.rename_to(&bootstrap, "escape").is_err());
        assert!(!root.path().join("escape").exists());
        retained.rename_to(&destination, "history").unwrap();
        assert!(root.path().join("moved/history").exists());
        assert!(!root.path().join("destination/history").exists());
        assert!(!root.path().join("source/journal").exists());
        assert_eq!(identity(retained.file()).unwrap(), expected);
    }
}
