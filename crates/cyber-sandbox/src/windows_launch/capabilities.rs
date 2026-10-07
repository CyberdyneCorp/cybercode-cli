//! Owned fixed runtime capability and suspended-token verification.

use std::io;
use std::os::windows::io::{AsRawHandle, OwnedHandle};
use std::ptr::null_mut;

use windows_sys::Win32::Foundation::LocalFree;
use windows_sys::Win32::Security::{
    CopySid, DeriveCapabilitySidsFromName, EqualSid, GetLengthSid, GetTokenInformation, IsValidSid,
    PSID, SID_AND_ATTRIBUTES, TOKEN_GROUPS, TokenCapabilities,
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
        Self::derive("registryRead")
    }

    #[cfg(feature = "windows-test-controls")]
    pub fn network_control() -> io::Result<Self> {
        Self::derive("internetClient")
    }

    fn derive(name: &str) -> io::Result<Self> {
        let name: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
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

    pub fn verify(&self, token: &OwnedHandle) -> io::Result<()> {
        let (buffer, length) = token_capabilities(token)?;
        // The Windows API populated an aligned buffer with at least a count.
        let count = unsafe { *buffer.as_ptr().cast::<u32>() } as usize;
        if count != self.entries.len() {
            return Err(io::Error::other("Unexpected runtime capability count"));
        }
        if count == 0 {
            return Ok(());
        }
        let offset = std::mem::offset_of!(TOKEN_GROUPS, Groups);
        let bytes = count * std::mem::size_of::<SID_AND_ATTRIBUTES>();
        if offset + bytes > length {
            return Err(io::Error::other("Truncated runtime capability information"));
        }
        // Count is bounded by our expected set and the verified API buffer size.
        let actual = unsafe {
            std::slice::from_raw_parts(
                buffer
                    .as_ptr()
                    .cast::<u8>()
                    .add(offset)
                    .cast::<SID_AND_ATTRIBUTES>(),
                count,
            )
        };
        self.verify_entries(actual)
    }

    fn verify_entries(&self, actual: &[SID_AND_ATTRIBUTES]) -> io::Result<()> {
        if actual.len() != self.entries.len() {
            return Err(io::Error::other("Unexpected runtime capability count"));
        }
        for (actual, expected) in actual.iter().zip(&self.entries) {
            if actual.Sid.is_null()
                || actual.Attributes & SE_GROUP_ENABLED as u32 == 0
                || unsafe { IsValidSid(actual.Sid) } == 0
                || unsafe { EqualSid(actual.Sid, expected.Sid) } == 0
            {
                return Err(io::Error::other(
                    "Unexpected or disabled runtime capability SID",
                ));
            }
        }
        Ok(())
    }
}

fn token_capabilities(token: &OwnedHandle) -> io::Result<(Vec<usize>, usize)> {
    let mut length = 0;
    unsafe {
        GetTokenInformation(
            token.as_raw_handle(),
            TokenCapabilities,
            null_mut(),
            0,
            &mut length,
        )
    };
    if !(4..=4096).contains(&length) {
        return Err(io::Error::other(
            "Invalid runtime capability information size",
        ));
    }
    let capacity = length;
    let mut buffer = vec![0usize; (capacity as usize).div_ceil(std::mem::size_of::<usize>())];
    if unsafe {
        GetTokenInformation(
            token.as_raw_handle(),
            TokenCapabilities,
            buffer.as_mut_ptr().cast(),
            capacity,
            &mut length,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    if length < 4 || length > capacity {
        return Err(io::Error::other(
            "Runtime capability information changed size",
        ));
    }
    Ok((buffer, length as usize))
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::windows::io::FromRawHandle;
    use windows_sys::Win32::Security::TOKEN_QUERY;
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    #[test]
    fn same_count_unexpected_capability_sid_is_refused() {
        let expected = CapabilitySet::new(true).unwrap();
        expected.verify_entries(&expected.entries).unwrap();
        // Both sets contain one valid, enabled, Windows-derived capability.
        let other = CapabilitySet::derive("internetClient").unwrap();
        let error = expected.verify_entries(&other.entries).unwrap_err();
        assert!(error.to_string().contains("capability SID"));
    }

    #[test]
    fn expected_capability_must_be_enabled() {
        let expected = CapabilitySet::new(true).unwrap();
        expected.verify_entries(&expected.entries).unwrap();
        let disabled = SID_AND_ATTRIBUTES {
            Sid: expected.entries[0].Sid,
            Attributes: 0,
        };
        let error = expected.verify_entries(&[disabled]).unwrap_err();
        assert!(error.to_string().contains("disabled runtime capability"));
    }

    #[test]
    fn host_token_does_not_satisfy_runtime_capability_policy() {
        let mut raw = null_mut();
        assert_ne!(
            unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut raw) },
            0
        );
        let token = unsafe { OwnedHandle::from_raw_handle(raw) };
        CapabilitySet::default().verify(&token).unwrap();
        let error = CapabilitySet::new(true)
            .unwrap()
            .verify(&token)
            .unwrap_err();
        assert!(error.to_string().contains("capability count"));
    }
}
