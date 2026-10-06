//! Capability-free AppContainer process launch. Tool dispatch remains disabled.
#![allow(unsafe_code)]

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::{OsStr, OsString};
use std::io;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::{AsHandle, AsRawHandle, BorrowedHandle, FromRawHandle, OwnedHandle};
use std::path::Path;
use std::ptr::{null, null_mut};
use std::time::Duration;

use windows_sys::Win32::Foundation::{HANDLE, WAIT_OBJECT_0, WAIT_TIMEOUT};
use windows_sys::Win32::Security::{
    EqualSid, GetTokenInformation, SECURITY_CAPABILITIES, TOKEN_APPCONTAINER_INFORMATION,
    TOKEN_QUERY, TokenAppContainerSid, TokenIsAppContainer,
};
use windows_sys::Win32::System::Threading::{
    CREATE_SUSPENDED, CREATE_UNICODE_ENVIRONMENT, CreateProcessW, DeleteProcThreadAttributeList,
    EXTENDED_STARTUPINFO_PRESENT, GetExitCodeProcess, InitializeProcThreadAttributeList,
    OpenProcessToken, PROC_THREAD_ATTRIBUTE_JOB_LIST, PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES,
    PROCESS_INFORMATION, ResumeThread, STARTUPINFOEXW, TerminateProcess, UpdateProcThreadAttribute,
    WaitForSingleObject,
};

use crate::windows_container::Profile;
use crate::windows_process::Job;

/// Launch only after job assignment and exact AppContainer identity verification.
/// The environment is explicit; no parent handles or credential variables are inherited.
/// This low-level path has no redirected stdio and does not implement root policy.
pub fn spawn(
    profile: &Profile,
    program: &Path,
    args: &[OsString],
    environment: &BTreeMap<String, String>,
    directory: &Path,
) -> io::Result<ContainerChild> {
    profile.ensure_active()?;
    if !program.is_absolute() || !directory.is_absolute() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Windows launch paths must be absolute",
        ));
    }
    let application = terminated(program.as_os_str())?;
    let directory = terminated(directory.as_os_str())?;
    let mut command = command_line(program.as_os_str(), args)?;
    let environment = environment_block(environment)?;
    let capabilities = SECURITY_CAPABILITIES {
        AppContainerSid: profile.sid(),
        ..Default::default()
    };
    let job = Job::new()?;
    let jobs = [job.handle()];
    let mut attributes = Attributes::new(&capabilities, &jobs)?;
    let mut startup = STARTUPINFOEXW::default();
    startup.StartupInfo.cb = std::mem::size_of::<STARTUPINFOEXW>() as u32;
    startup.lpAttributeList = attributes.pointer();
    let mut information = PROCESS_INFORMATION::default();
    // All buffers and security capabilities stay alive until the synchronous API returns.
    let created = unsafe {
        CreateProcessW(
            application.as_ptr(),
            command.as_mut_ptr(),
            null(),
            null(),
            0,
            CREATE_SUSPENDED | CREATE_UNICODE_ENVIRONMENT | EXTENDED_STARTUPINFO_PRESENT,
            environment.as_ptr().cast(),
            directory.as_ptr(),
            &startup.StartupInfo,
            &mut information,
        )
    };
    if created == 0 {
        return Err(io::Error::last_os_error());
    }
    // Success transfers two valid non-inheritable handles to these owners.
    let process = unsafe { OwnedHandle::from_raw_handle(information.hProcess) };
    let thread = unsafe { OwnedHandle::from_raw_handle(information.hThread) };
    let child = ContainerChild {
        job: Some(job),
        process,
        _profile: profile.clone(),
    };
    child
        .job
        .as_ref()
        .unwrap()
        .verify_handle(child.process.as_raw_handle())?;
    verify_identity(child.process.as_raw_handle(), profile)?;
    // User code cannot run until both ownership and identity checks have succeeded.
    if unsafe { ResumeThread(thread.as_raw_handle()) } == u32::MAX {
        return Err(io::Error::last_os_error());
    }
    Ok(child)
}

/// Dropping this owner terminates the process and closes the descendant-owning job.
pub struct ContainerChild {
    job: Option<Job>,
    process: OwnedHandle,
    _profile: Profile,
}

impl AsHandle for ContainerChild {
    fn as_handle(&self) -> BorrowedHandle<'_> {
        self.process.as_handle()
    }
}

impl ContainerChild {
    /// A bounded synchronous wait, intended for launch helpers or blocking workers.
    pub fn wait(&mut self, timeout: Duration) -> io::Result<u32> {
        let milliseconds = timeout.as_millis().min(u128::from(u32::MAX - 1)) as u32;
        match unsafe { WaitForSingleObject(self.process.as_raw_handle(), milliseconds) } {
            WAIT_OBJECT_0 => {}
            WAIT_TIMEOUT => {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "AppContainer wait timed out",
                ));
            }
            _ => return Err(io::Error::last_os_error()),
        }
        let mut code = 0;
        if unsafe { GetExitCodeProcess(self.process.as_raw_handle(), &mut code) } == 0 {
            return Err(io::Error::last_os_error());
        }
        drop(self.job.take());
        Ok(code)
    }
}

impl Drop for ContainerChild {
    fn drop(&mut self) {
        // Also handles setup failure before successful job assignment.
        unsafe { TerminateProcess(self.process.as_raw_handle(), 1) };
        drop(self.job.take());
    }
}

struct Attributes {
    storage: Vec<usize>,
    initialized: bool,
}

impl Attributes {
    fn new(capabilities: &SECURITY_CAPABILITIES, jobs: &[HANDLE; 1]) -> io::Result<Self> {
        let mut bytes = 0;
        unsafe { InitializeProcThreadAttributeList(null_mut(), 2, 0, &mut bytes) };
        if bytes == 0 {
            return Err(io::Error::last_os_error());
        }
        let mut attributes = Self {
            storage: vec![0; bytes.div_ceil(std::mem::size_of::<usize>())],
            initialized: false,
        };
        // The storage is pointer-aligned and Windows supplied its required size.
        if unsafe { InitializeProcThreadAttributeList(attributes.pointer(), 2, 0, &mut bytes) } == 0
        {
            return Err(io::Error::last_os_error());
        }
        attributes.initialized = true;
        // Atomic assignment closes the parent-death-before-assignment gap.
        if unsafe {
            UpdateProcThreadAttribute(
                attributes.pointer(),
                0,
                PROC_THREAD_ATTRIBUTE_JOB_LIST as usize,
                jobs.as_ptr().cast(),
                std::mem::size_of_val(jobs),
                null_mut(),
                null(),
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        if unsafe {
            UpdateProcThreadAttribute(
                attributes.pointer(),
                0,
                PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES as usize,
                (capabilities as *const SECURITY_CAPABILITIES).cast(),
                std::mem::size_of::<SECURITY_CAPABILITIES>(),
                null_mut(),
                null(),
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(attributes)
    }

    fn pointer(&mut self) -> windows_sys::Win32::System::Threading::LPPROC_THREAD_ATTRIBUTE_LIST {
        self.storage.as_mut_ptr().cast()
    }
}

impl Drop for Attributes {
    fn drop(&mut self) {
        if self.initialized {
            unsafe { DeleteProcThreadAttributeList(self.pointer()) };
        }
    }
}

fn verify_identity(process: HANDLE, profile: &Profile) -> io::Result<()> {
    let mut raw = null_mut();
    if unsafe { OpenProcessToken(process, TOKEN_QUERY, &mut raw) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let token = unsafe { OwnedHandle::from_raw_handle(raw) };
    let mut flag = 0u32;
    let mut length = 0;
    if unsafe {
        GetTokenInformation(
            token.as_raw_handle(),
            TokenIsAppContainer,
            (&mut flag as *mut u32).cast(),
            4,
            &mut length,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    if flag != 1 {
        return Err(io::Error::other("Created process is not an AppContainer"));
    }
    let sid = token_sid(&token)?;
    let information = unsafe { &*sid.as_ptr().cast::<TOKEN_APPCONTAINER_INFORMATION>() };
    if information.TokenAppContainer.is_null()
        || unsafe { EqualSid(information.TokenAppContainer, profile.sid()) } == 0
    {
        return Err(io::Error::other(
            "Created process has the wrong AppContainer identity",
        ));
    }
    Ok(())
}

fn token_sid(token: &OwnedHandle) -> io::Result<Vec<usize>> {
    let mut length = 0;
    unsafe {
        GetTokenInformation(
            token.as_raw_handle(),
            TokenAppContainerSid,
            null_mut(),
            0,
            &mut length,
        )
    };
    if length == 0 || length > 4096 {
        return Err(io::Error::other("Invalid AppContainer token SID size"));
    }
    let mut buffer = vec![0usize; (length as usize).div_ceil(std::mem::size_of::<usize>())];
    if unsafe {
        GetTokenInformation(
            token.as_raw_handle(),
            TokenAppContainerSid,
            buffer.as_mut_ptr().cast(),
            length,
            &mut length,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(buffer)
}

fn terminated(value: &OsStr) -> io::Result<Vec<u16>> {
    let mut units: Vec<u16> = value.encode_wide().collect();
    if units.contains(&0) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "NUL in launch input",
        ));
    }
    units.push(0);
    Ok(units)
}

fn command_line(program: &OsStr, args: &[OsString]) -> io::Result<Vec<u16>> {
    let mut command = Vec::new();
    for argument in std::iter::once(program).chain(args.iter().map(OsString::as_os_str)) {
        if !command.is_empty() {
            command.push(32);
        }
        quote(argument, &mut command)?;
    }
    command.push(0);
    if command.len() > 32767 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Windows command line too long",
        ));
    }
    Ok(command)
}

fn quote(value: &OsStr, output: &mut Vec<u16>) -> io::Result<()> {
    let units = terminated(value)?;
    output.push(34);
    let mut slashes = 0;
    for unit in units.into_iter().take_while(|unit| *unit != 0) {
        if unit == 92 {
            slashes += 1;
            continue;
        }
        let count = if unit == 34 { slashes * 2 + 1 } else { slashes };
        output.extend(std::iter::repeat_n(92, count));
        output.push(unit);
        slashes = 0;
    }
    output.extend(std::iter::repeat_n(92, slashes * 2));
    output.push(34);
    Ok(())
}

fn environment_block(environment: &BTreeMap<String, String>) -> io::Result<Vec<u16>> {
    if !environment
        .iter()
        .any(|(name, value)| name.eq_ignore_ascii_case("LOCALAPPDATA") && !value.is_empty())
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "AppContainer environment requires nonempty LOCALAPPDATA",
        ));
    }
    let mut names = BTreeSet::new();
    let mut entries = Vec::new();
    for (name, value) in environment {
        if name.is_empty() || name.contains('=') || !names.insert(name.to_uppercase()) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Invalid or duplicate environment name",
            ));
        }
        entries.push((
            name.to_uppercase(),
            terminated(OsStr::new(&format!("{name}={value}")))?,
        ));
    }
    entries.sort_by(|left, right| left.0.cmp(&right.0));
    let mut block: Vec<u16> = entries.into_iter().flat_map(|(_, units)| units).collect();
    if block.is_empty() {
        block.push(0);
    }
    block.push(0);
    Ok(block)
}
