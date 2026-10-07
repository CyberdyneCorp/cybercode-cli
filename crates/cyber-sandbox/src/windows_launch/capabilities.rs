//! Fixed-capability native comparison; compiled only with windows-test-controls.

use std::io;
use std::ptr::null_mut;

use windows_sys::Win32::Foundation::LocalFree;
use windows_sys::Win32::Security::{
    CopySid, DeriveCapabilitySidsFromName, GetLengthSid, IsValidSid, PSID, SID_AND_ATTRIBUTES,
};
use windows_sys::Win32::System::SystemServices::SE_GROUP_ENABLED;

#[derive(Default)]
pub(super) struct CapabilitySet {
    _sid: Vec<u32>,
    pub entries: Vec<SID_AND_ATTRIBUTES>,
}

impl CapabilitySet {
    pub fn new(registry_read: bool) -> io::Result<Self> {
        if !registry_read {
            return Ok(Self::default());
        }
        let name: Vec<u16> = "registryRead".encode_utf16().chain(Some(0)).collect();
        let mut groups = SidArray::default();
        let mut capabilities = SidArray::default();
        // Windows allocates both arrays and their SIDs; each owner frees them
        // even if derivation or later validation fails.
        if unsafe {
            DeriveCapabilitySidsFromName(
                name.as_ptr(),
                &mut groups.pointer,
                &mut groups.count,
                &mut capabilities.pointer,
                &mut capabilities.count,
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        if capabilities.count != 1 || capabilities.pointer.is_null() {
            return Err(io::Error::other("Invalid registryRead capability array"));
        }
        let raw = unsafe { *capabilities.pointer };
        if raw.is_null() || unsafe { IsValidSid(raw) } == 0 {
            return Err(io::Error::other("Invalid registryRead capability SID"));
        }
        let length = unsafe { GetLengthSid(raw) };
        if !(8..=68).contains(&length) {
            return Err(io::Error::other("Invalid registryRead SID length"));
        }
        let mut sid = vec![0u32; (length as usize).div_ceil(4)];
        if unsafe { CopySid(length, sid.as_mut_ptr().cast(), raw) } == 0 {
            return Err(io::Error::last_os_error());
        }
        let entries = vec![SID_AND_ATTRIBUTES {
            Sid: sid.as_mut_ptr().cast(),
            Attributes: SE_GROUP_ENABLED as u32,
        }];
        Ok(Self { _sid: sid, entries })
    }
}

#[derive(Default)]
struct SidArray {
    pointer: *mut PSID,
    count: u32,
}

impl Drop for SidArray {
    fn drop(&mut self) {
        if self.pointer.is_null() {
            return;
        }
        // These pointers and their count come exclusively from the Windows API.
        for index in 0..self.count as usize {
            unsafe { LocalFree(*self.pointer.add(index)) };
        }
        unsafe { LocalFree(self.pointer.cast()) };
        self.pointer = null_mut();
    }
}
