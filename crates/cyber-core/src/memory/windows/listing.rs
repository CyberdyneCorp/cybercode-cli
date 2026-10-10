//! Bounded enumeration through a held synchronous directory, without pathname reopening.
use super::*;
use std::mem::{offset_of, size_of};
use std::ptr::{null, null_mut};
use windows_sys::Wdk::Storage::FileSystem::{
    FILE_NAMES_INFORMATION, FileNamesInformation, NtQueryDirectoryFile,
};
use windows_sys::Win32::Foundation::{RtlNtStatusToDosError, STATUS_NO_MORE_FILES};
use windows_sys::Win32::System::IO::IO_STATUS_BLOCK;
pub(super) fn names(file: &File, limit: usize) -> Result<Vec<String>, MemoryStorageError> {
    if limit > 4096 {
        return Err(MemoryStorageError::TooLarge);
    }
    verify_private(file)?;
    if !file.metadata()?.is_dir() {
        return Err(refusal("expected a private directory for enumeration"));
    }
    let mut names = Vec::new();
    for attempt in 0..limit + 3 {
        let Some(name) = next(file, attempt == 0)? else {
            return Ok(names);
        };
        if matches!(name.as_str(), "." | "..") {
            continue;
        }
        if names.len() >= limit {
            return Err(MemoryStorageError::TooLarge);
        }
        names.push(name);
    }
    Err(MemoryStorageError::TooLarge)
}
fn next(file: &File, restart: bool) -> Result<Option<String>, MemoryStorageError> {
    let mut buffer = vec![0usize; 4096 / size_of::<usize>()];
    let mut status = IO_STATUS_BLOCK::default();
    // Factory handles are synchronous; the aligned fixed buffer and status outlive this call.
    let result = unsafe {
        NtQueryDirectoryFile(
            file.as_raw_handle(),
            null_mut(),
            None,
            null(),
            &mut status,
            buffer.as_mut_ptr().cast(),
            size_of_val(buffer.as_slice()) as u32,
            FileNamesInformation,
            true,
            null(),
            restart,
        )
    };
    let completed = unsafe { status.Anonymous.Status };
    if result == STATUS_NO_MORE_FILES && completed == STATUS_NO_MORE_FILES {
        return Ok(None);
    }
    settled(result)?;
    settled(completed)?;
    decode(&buffer, status.Information).map(Some)
}
fn settled(status: i32) -> Result<(), MemoryStorageError> {
    if status < 0 {
        return Err(
            io::Error::from_raw_os_error(unsafe { RtlNtStatusToDosError(status) } as i32).into(),
        );
    }
    if status != 0 {
        return Err(refusal(
            "native directory enumeration settlement is unknown",
        ));
    }
    Ok(())
}
fn decode(buffer: &[usize], used: usize) -> Result<String, MemoryStorageError> {
    let offset = offset_of!(FILE_NAMES_INFORMATION, FileName);
    if size_of_val(buffer) < size_of::<FILE_NAMES_INFORMATION>()
        || used < offset
        || used > size_of_val(buffer)
    {
        return Err(refusal("invalid native directory entry buffer"));
    }
    let header = unsafe { &*buffer.as_ptr().cast::<FILE_NAMES_INFORMATION>() };
    let length = header.FileNameLength as usize;
    if header.NextEntryOffset != 0
        || length == 0
        || !length.is_multiple_of(2)
        || offset.checked_add(length).is_none_or(|end| end > used)
    {
        return Err(refusal("invalid native directory entry length"));
    }
    // The counted UTF-16 range lies wholly inside the initialized aligned buffer.
    let name = unsafe {
        std::slice::from_raw_parts(
            buffer.as_ptr().cast::<u8>().add(offset).cast::<u16>(),
            length / 2,
        )
    };
    String::from_utf16(name).map_err(|_| refusal("invalid native directory entry name"))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn directory_entry_decoder_refuses_short_oversized_odd_and_chained_entries() {
        let mut buffer = vec![0usize; 16];
        assert!(decode(&buffer, 2).is_err());
        assert!(decode(&buffer, 200).is_err());
        let header = buffer.as_mut_ptr().cast::<FILE_NAMES_INFORMATION>();
        unsafe {
            (*header).FileNameLength = 3;
        }
        assert!(decode(&buffer, 16).is_err());
        unsafe {
            (*header).FileNameLength = 2;
            (*header).NextEntryOffset = 8;
        }
        assert!(decode(&buffer, 16).is_err());
        unsafe {
            (*header).NextEntryOffset = 0;
            (*header).FileName[0] = b'x' as u16;
        }
        assert_eq!(decode(&buffer, 14).unwrap(), "x");
    }
}
