//! Exact-object deletion. Callers persist a disposal plan before effects.
use super::*;
use std::os::windows::io::IntoRawHandle;
use windows_sys::Wdk::Foundation::NtClose;
use windows_sys::Wdk::Storage::FileSystem::{
    FILE_DISPOSITION_INFORMATION, FileDispositionInformation, NtSetInformationFile,
};
use windows_sys::Win32::Foundation::RtlNtStatusToDosError;
use windows_sys::Win32::System::IO::IO_STATUS_BLOCK;
/// An exclusively admitted source. Normal readonly inspection uses this held handle.
pub struct DisposableChild {
    file: File,
    parent: File,
    id: FileIdentity,
    name: String,
    directory: bool,
}
pub fn dispose_private_file(
    parent: &File,
    name: &str,
    expected: FileIdentity,
) -> Result<DisposableChild, MemoryStorageError> {
    admit(parent, name, expected, false)
}
pub fn dispose_private_directory(
    parent: &File,
    name: &str,
    expected: FileIdentity,
) -> Result<DisposableChild, MemoryStorageError> {
    admit(parent, name, expected, true)
}
fn admit(
    parent: &File,
    name: &str,
    expected: FileIdentity,
    directory: bool,
) -> Result<DisposableChild, MemoryStorageError> {
    verify_private(parent)?;
    let file = creation::child(parent, name, directory, creation::Mode::Dispose)?;
    if identity(&file)? != expected {
        return Err(MemoryStorageError::ReviewConflict);
    }
    Ok(DisposableChild {
        file,
        parent: parent.try_clone()?,
        id: expected,
        name: name.into(),
        directory,
    })
}
impl DisposableChild {
    pub fn file(&self) -> &File {
        &self.file
    }
    /// Consumes the source and acknowledges only flushed named absence.
    /// Failure after disposition/close leaves effects for the caller's recorded recovery plan.
    pub fn remove_durable(self) -> Result<(), MemoryStorageError> {
        self.remove_with(sync_private)
    }
    fn remove_with(
        self,
        mut flush: impl FnMut(&File) -> Result<(), MemoryStorageError>,
    ) -> Result<(), MemoryStorageError> {
        flush(&self.file)?;
        flush(&self.parent)?;
        if verify_private(&self.file)? != self.id {
            return Err(MemoryStorageError::ReviewConflict);
        }
        let information = FILE_DISPOSITION_INFORMATION { DeleteFile: true };
        let mut status = IO_STATUS_BLOCK::default();
        // The private factory owns a synchronous no-sharing handle and this fixed buffer.
        let result = unsafe {
            NtSetInformationFile(
                self.file.as_raw_handle(),
                &mut status,
                (&information as *const FILE_DISPOSITION_INFORMATION).cast(),
                std::mem::size_of::<FILE_DISPOSITION_INFORMATION>() as u32,
                FileDispositionInformation,
            )
        };
        settlement(result)?;
        // Synchronous completion must also agree; no optimistic pending settlement.
        settlement(unsafe { status.Anonymous.Status })?;
        // Disposition permits only close on this source. Consuming File prevents double close.
        let raw = self.file.into_raw_handle();
        settlement(unsafe { NtClose(raw) })?;
        flush(&self.parent)?;
        match creation::child(
            &self.parent,
            &self.name,
            self.directory,
            creation::Mode::Open(Access::Read),
        ) {
            Err(MemoryStorageError::Io(error)) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error),
            Ok(_) => Err(MemoryStorageError::ReviewConflict),
        }
    }
}
fn settlement(status: i32) -> Result<(), MemoryStorageError> {
    if status < 0 {
        return Err(
            io::Error::from_raw_os_error(unsafe { RtlNtStatusToDosError(status) } as i32).into(),
        );
    }
    if status != 0 {
        return Err(refusal("native memory disposal settlement is unknown"));
    }
    Ok(())
}
#[cfg(test)]
#[path = "disposal_tests.rs"]
mod tests;
