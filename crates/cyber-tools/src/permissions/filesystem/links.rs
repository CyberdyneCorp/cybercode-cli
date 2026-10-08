//! A path-only proof cannot establish all aliases of a multiply linked file.

use std::path::Path;

pub(super) fn unaliased(path: &Path) -> bool {
    match std::fs::metadata(path) {
        Ok(metadata) if metadata.is_dir() => true,
        Ok(metadata) if metadata.is_file() => single_link(path, &metadata),
        Ok(_) => false,
        Err(error) => error.kind() == std::io::ErrorKind::NotFound,
    }
}

#[cfg(unix)]
fn single_link(_path: &Path, metadata: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    metadata.nlink() == 1
}

#[cfg(windows)]
#[allow(unsafe_code)]
fn single_link(path: &Path, _metadata: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, FILE_FLAG_OPEN_REPARSE_POINT, FILE_READ_ATTRIBUTES,
        GetFileInformationByHandle,
    };
    let Ok(file) = std::fs::OpenOptions::new()
        .access_mode(FILE_READ_ATTRIBUTES)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
    else {
        return false;
    };
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    // The original file handle remains owned throughout this metadata query.
    let queried = unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) != 0 };
    queried && info.nNumberOfLinks == 1
}

#[cfg(not(any(unix, windows)))]
fn single_link(_path: &Path, _metadata: &std::fs::Metadata) -> bool {
    false
}
