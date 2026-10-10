//! Retain an already-open directory identity without pathname lookup or mutation access.
use std::{
    fs::File,
    io,
    os::windows::io::{AsRawHandle, FromRawHandle},
};
use windows_sys::Win32::{
    Foundation::INVALID_HANDLE_VALUE,
    Storage::FileSystem::{
        FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_READ_ATTRIBUTES,
        FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, ReOpenFile,
    },
};

pub(super) fn retain_directory_identity(original: &File) -> io::Result<File> {
    // The borrowed File owns a live handle. Attribute access adds neither enumeration nor writes.
    // Delete sharing permits user renames after lookup handles are released by the caller.
    let handle = unsafe {
        ReOpenFile(
            original.as_raw_handle(),
            FILE_READ_ATTRIBUTES,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    // Success returns an independent handle. File owns it and closes it on every subsequent path.
    Ok(unsafe { File::from_raw_handle(handle) })
}
