//! Small owned wrappers for macOS ACL evidence and metadata-only copying.
//! No caller pointer is accepted; all native allocations are freed on every path.
use super::*;
use std::ffi::{c_char, c_int, c_uint, c_void};
use std::os::fd::AsRawFd;

unsafe extern "C" {
    fn acl_get_fd_np(fd: c_int, kind: c_uint) -> *mut c_void;
    fn acl_to_text(acl: *mut c_void, length: *mut isize) -> *mut c_char;
    fn acl_free(object: *mut c_void) -> c_int;
}
struct Allocation(*mut c_void);
impl Drop for Allocation {
    fn drop(&mut self) {
        unsafe {
            acl_free(self.0);
        }
    }
}

pub(super) fn acl(file: &File) -> Result<Vec<u8>, Error> {
    // ACL_TYPE_EXTENDED from the platform sys/acl.h. The File owns a live descriptor.
    let raw = unsafe { acl_get_fd_np(file.as_raw_fd(), 0x100) };
    if raw.is_null() {
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::ENOENT) {
            return Ok(vec![]);
        }
        return Err(Error::Io);
    }
    let _acl = Allocation(raw);
    let mut length = 0isize;
    // The ACL remains owned and valid until after this bounded text copy.
    let text = unsafe { acl_to_text(raw, &mut length) };
    if text.is_null() {
        return Err(Error::Io);
    }
    let _text = Allocation(text.cast());
    if !(0..=65536).contains(&length) {
        return Err(Error::Limit);
    }
    // acl_to_text allocated at least the reported length; no unbounded C-string scan.
    Ok(unsafe { std::slice::from_raw_parts(text.cast::<u8>(), length as usize) }.to_vec())
}

pub(super) fn copy_metadata(source: &File, destination: &File) -> Result<(), Error> {
    struct State(rustix::fs::copyfile_state_t);
    impl Drop for State {
        fn drop(&mut self) {
            let _ = unsafe { rustix::fs::copyfile_state_free(self.0) };
        }
    }
    let state = State(rustix::fs::copyfile_state_alloc().map_err(|_| Error::Io)?);
    // This state was allocated above, is exclusively owned, and is freed only after
    // this synchronous call. METADATA copies no file data or caller buffers.
    unsafe {
        rustix::fs::fcopyfile(
            source,
            destination,
            state.0,
            rustix::fs::CopyfileFlags::METADATA,
        )
    }
    .map_err(|_| Error::Io)
}
