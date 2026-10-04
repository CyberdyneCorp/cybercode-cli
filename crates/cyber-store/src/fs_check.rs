//! Detection of network filesystems, which are rejected for the live database.

use std::path::Path;

/// The filesystem name when `path` is on a known network filesystem.
pub fn network_filesystem(path: &Path) -> Option<String> {
    detect(path)
}

#[cfg(target_os = "macos")]
fn detect(path: &Path) -> Option<String> {
    let name = sys::fs_type_name(path)?;
    is_network_fs_name(&name).then_some(name)
}

#[cfg(target_os = "linux")]
fn detect(path: &Path) -> Option<String> {
    network_magic_name(sys::fs_magic(path)?).map(str::to_string)
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn detect(_path: &Path) -> Option<String> {
    None
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn is_network_fs_name(name: &str) -> bool {
    matches!(name, "nfs" | "smbfs" | "afpfs" | "webdav" | "cifs" | "ftp")
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn network_magic_name(magic: i64) -> Option<&'static str> {
    match magic {
        0x6969 => Some("nfs"),
        0x517B => Some("smb"),
        0xFE53_4D42 => Some("smb2"),
        0xFF53_4D42 => Some("cifs"),
        0x5346_414F => Some("afs"),
        0x0102_1997 => Some("9p"),
        _ => None,
    }
}

#[cfg(unix)]
#[allow(unsafe_code)]
mod sys {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    use std::path::Path;

    fn statfs(path: &Path) -> Option<libc::statfs> {
        let c_path = CString::new(path.as_os_str().as_bytes()).ok()?;
        // SAFETY: an all-zero statfs is a valid out-parameter for statfs(2).
        let mut st: libc::statfs = unsafe { std::mem::zeroed() };
        // SAFETY: `c_path` is NUL-terminated and `st` is a valid, writable statfs.
        let rc = unsafe { libc::statfs(c_path.as_ptr(), &mut st) };
        (rc == 0).then_some(st)
    }

    #[cfg(target_os = "macos")]
    pub fn fs_type_name(path: &Path) -> Option<String> {
        let st = statfs(path)?;
        // SAFETY: the kernel fills f_fstypename with a NUL-terminated C string.
        let name = unsafe { std::ffi::CStr::from_ptr(st.f_fstypename.as_ptr()) };
        Some(name.to_string_lossy().into_owned())
    }

    #[cfg(target_os = "linux")]
    #[allow(clippy::unnecessary_cast)]
    pub fn fs_magic(path: &Path) -> Option<i64> {
        statfs(path).map(|st| st.f_type as i64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_network_filesystems() {
        assert!(is_network_fs_name("nfs"));
        assert!(is_network_fs_name("smbfs"));
        assert!(!is_network_fs_name("apfs"));
        assert_eq!(network_magic_name(0x6969), Some("nfs"));
        assert_eq!(network_magic_name(0xEF53), None);
    }

    #[test]
    fn local_temp_dir_is_not_network() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(network_filesystem(dir.path()), None);
    }
}
