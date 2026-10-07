//! Capability-free less-privileged AppContainer process launch. Tool dispatch remains disabled.
#![allow(unsafe_code)]

#[cfg(feature = "windows-test-controls")]
mod capabilities;

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::{OsStr, OsString};
use std::io;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::{AsHandle, AsRawHandle, BorrowedHandle, FromRawHandle, OwnedHandle};
use std::path::{Path, PathBuf};
use std::ptr::{null, null_mut};
use std::time::Duration;

use windows_sys::Win32::Foundation::{
    DUPLICATE_SAME_ACCESS, DuplicateHandle, HANDLE, WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows_sys::Win32::Security::{
    EqualSid, GetTokenInformation, SECURITY_CAPABILITIES, TOKEN_APPCONTAINER_INFORMATION,
    TOKEN_QUERY, TokenAppContainerSid, TokenIsAppContainer,
};
use windows_sys::Win32::System::Threading::{
    CREATE_SUSPENDED, CREATE_UNICODE_ENVIRONMENT, CreateProcessW, DeleteProcThreadAttributeList,
    EXTENDED_STARTUPINFO_PRESENT, GetCurrentProcess, GetExitCodeProcess,
    InitializeProcThreadAttributeList, OpenProcessToken,
    PROC_THREAD_ATTRIBUTE_ALL_APPLICATION_PACKAGES_POLICY, PROC_THREAD_ATTRIBUTE_HANDLE_LIST,
    PROC_THREAD_ATTRIBUTE_JOB_LIST, PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES,
    PROCESS_INFORMATION, STARTF_USESTDHANDLES, STARTUPINFOEXW, TerminateProcess,
    UpdateProcThreadAttribute, WaitForSingleObject,
};

use windows_sys::Win32::System::WindowsProgramming::PROCESS_CREATION_ALL_APPLICATION_PACKAGES_OPT_OUT;

use crate::windows_container::{AclGrant, LaunchReservation, Profile};
use crate::windows_process::Job;

/// Launch only after job assignment and exact AppContainer identity verification.
/// The environment is explicit and no parent handles are inherited.
/// This low-level path does not implement root policy or credential filtering.
pub fn spawn(
    profile: &Profile,
    program: &Path,
    args: &[OsString],
    environment: &BTreeMap<String, String>,
    directory: &Path,
) -> io::Result<ContainerChild> {
    spawn_inner(
        profile,
        program,
        args,
        environment,
        directory,
        None,
        LaunchPolicy::Isolated,
    )
}

/// Explicit standard streams. Supplied handles must grant only intended stream access.
pub struct StandardStreams<'a> {
    pub stdin: BorrowedHandle<'a>,
    pub stdout: BorrowedHandle<'a>,
    pub stderr: BorrowedHandle<'a>,
}

/// Launch with exactly the three supplied stream handles, duplicated for inheritance.
pub fn spawn_with_stdio(
    profile: &Profile,
    program: &Path,
    args: &[OsString],
    environment: &BTreeMap<String, String>,
    directory: &Path,
    streams: StandardStreams<'_>,
) -> io::Result<ContainerChild> {
    spawn_inner(
        profile,
        program,
        args,
        environment,
        directory,
        Some(streams),
        LaunchPolicy::Isolated,
    )
}

/// Ordinary AppContainer positive control; never used by tool dispatch.
#[cfg(feature = "windows-test-controls")]
pub fn spawn_with_package_allowances_for_test(
    profile: &Profile,
    program: &Path,
    args: &[OsString],
    environment: &BTreeMap<String, String>,
    directory: &Path,
) -> io::Result<ContainerChild> {
    spawn_inner(
        profile,
        program,
        args,
        environment,
        directory,
        None,
        LaunchPolicy::PackageAllowance,
    )
}

/// Fixed registry-read LPAC comparison; unavailable to production tool dispatch.
#[cfg(feature = "windows-test-controls")]
pub fn spawn_with_registry_read_for_test(
    profile: &Profile,
    program: &Path,
    args: &[OsString],
    environment: &BTreeMap<String, String>,
    directory: &Path,
    streams: Option<StandardStreams<'_>>,
) -> io::Result<ContainerChild> {
    spawn_inner(
        profile,
        program,
        args,
        environment,
        directory,
        streams,
        LaunchPolicy::RegistryRead,
    )
}

#[derive(Clone, Copy)]
enum LaunchPolicy {
    Isolated,
    #[cfg(feature = "windows-test-controls")]
    PackageAllowance,
    #[cfg(feature = "windows-test-controls")]
    RegistryRead,
}

fn spawn_inner(
    profile: &Profile,
    program: &Path,
    args: &[OsString],
    environment: &BTreeMap<String, String>,
    directory: &Path,
    streams: Option<StandardStreams<'_>>,
    policy: LaunchPolicy,
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
    environment_block(environment)?;
    let temp = PrivateTemp::new(profile)?;
    let environment = temp.environment(environment)?;
    #[cfg(feature = "windows-test-controls")]
    let capability_owner =
        capabilities::CapabilitySet::new(matches!(policy, LaunchPolicy::RegistryRead))?;
    let capabilities = SECURITY_CAPABILITIES {
        AppContainerSid: profile.sid(),
        #[cfg(feature = "windows-test-controls")]
        Capabilities: if capability_owner.entries.is_empty() {
            null_mut()
        } else {
            capability_owner.entries.as_ptr().cast_mut()
        },
        #[cfg(feature = "windows-test-controls")]
        CapabilityCount: capability_owner.entries.len() as u32,
        ..Default::default()
    };
    let job = Job::new()?;
    let jobs = [job.handle()];
    let streams = streams.map(InheritedStreams::new).transpose()?;
    let handles = streams.as_ref().map(InheritedStreams::handles);
    #[cfg(feature = "windows-test-controls")]
    let opt_out = !matches!(policy, LaunchPolicy::PackageAllowance);
    #[cfg(not(feature = "windows-test-controls"))]
    let opt_out = matches!(policy, LaunchPolicy::Isolated);
    let package_policy = opt_out.then_some(PROCESS_CREATION_ALL_APPLICATION_PACKAGES_OPT_OUT);
    let mut attributes = Attributes::new(
        &capabilities,
        &jobs,
        handles.as_ref(),
        package_policy.as_ref(),
    )?;
    let mut startup = STARTUPINFOEXW::default();
    startup.StartupInfo.cb = std::mem::size_of::<STARTUPINFOEXW>() as u32;
    startup.lpAttributeList = attributes.pointer();
    if let Some(handles) = handles {
        startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
        startup.StartupInfo.hStdInput = handles[0];
        startup.StartupInfo.hStdOutput = handles[1];
        startup.StartupInfo.hStdError = handles[2];
    }
    let mut information = PROCESS_INFORMATION::default();
    // All buffers and security capabilities stay alive until the synchronous API returns.
    let created = unsafe {
        CreateProcessW(
            application.as_ptr(),
            command.as_mut_ptr(),
            null(),
            null(),
            i32::from(streams.is_some()),
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
        _temp: temp,
        _profile: profile.clone(),
    };
    child
        .job
        .as_ref()
        .unwrap()
        .verify_handle(child.process.as_raw_handle())?;
    verify_identity(child.process.as_raw_handle(), profile)?;
    // User code cannot run until both ownership and identity checks have succeeded.
    child._temp._reservation.resume(thread.as_handle())?;
    Ok(child)
}

struct InheritedStreams([OwnedHandle; 3]);

impl InheritedStreams {
    fn new(streams: StandardStreams<'_>) -> io::Result<Self> {
        Ok(Self([
            inheritable_duplicate(streams.stdin)?,
            inheritable_duplicate(streams.stdout)?,
            inheritable_duplicate(streams.stderr)?,
        ]))
    }

    fn handles(&self) -> [HANDLE; 3] {
        self.0.each_ref().map(AsRawHandle::as_raw_handle)
    }
}

fn inheritable_duplicate(source: BorrowedHandle<'_>) -> io::Result<OwnedHandle> {
    let owner = unsafe { GetCurrentProcess() };
    let mut duplicate = null_mut();
    if unsafe {
        DuplicateHandle(
            owner,
            source.as_raw_handle(),
            owner,
            &mut duplicate,
            0,
            1,
            DUPLICATE_SAME_ACCESS,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    // DuplicateHandle transferred a real handle to this owner.
    Ok(unsafe { OwnedHandle::from_raw_handle(duplicate) })
}

/// Dropping this owner terminates the process and closes the descendant-owning job.
pub struct ContainerChild {
    job: Option<Job>,
    process: OwnedHandle,
    _temp: PrivateTemp,
    _profile: Profile,
}

struct PrivateTemp {
    path: PathBuf,
    grant: Option<AclGrant>,
    _reservation: LaunchReservation,
}

impl PrivateTemp {
    fn new(profile: &Profile) -> io::Result<Self> {
        let reservation = profile.reserve_launch()?;
        // Windows rewrites TEMP/TMP to AC\Temp even with an explicit environment.
        // The invocation's fresh identity owns this directory, not a shared host temp.
        let path = profile.storage_path()?.join("Temp");
        prepare_temp_directory(&path)?;
        let mut temp = Self {
            path,
            grant: None,
            _reservation: reservation,
        };
        temp.grant = Some(profile.grant_private_directory(&temp.path)?);
        Ok(temp)
    }

    fn environment(&self, input: &BTreeMap<String, String>) -> io::Result<Vec<u16>> {
        let path = self.path.to_str().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "Temporary path is not Unicode")
        })?;
        let mut variables = input.clone();
        variables.retain(|name, _| {
            !name.eq_ignore_ascii_case("TEMP") && !name.eq_ignore_ascii_case("TMP")
        });
        for name in ["TEMP", "TMP"] {
            variables.insert(name.into(), path.into());
        }
        environment_block(&variables)
    }
}

fn prepare_temp_directory(path: &Path) -> io::Result<()> {
    use std::os::windows::fs::MetadataExt;
    use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;
    match std::fs::create_dir(path) {
        Ok(()) => return Ok(()),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error),
    }
    let metadata = std::fs::symlink_metadata(path)?;
    if !metadata.is_dir() || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(io::Error::other(
            "Private temporary storage refuses reparse points",
        ));
    }
    if std::fs::read_dir(path)?.next().is_some() {
        return Err(io::Error::other(
            "Private temporary storage must be empty before launch",
        ));
    }
    Ok(())
}

impl Drop for PrivateTemp {
    fn drop(&mut self) {
        if let Some(grant) = self.grant.take()
            && let Err(error) = grant.close()
        {
            temp_cleanup_error("revoke", error);
        }
        if let Err(error) = std::fs::remove_dir_all(&self.path)
            && error.kind() != io::ErrorKind::NotFound
        {
            temp_cleanup_error("remove", error);
        }
    }
}

fn temp_cleanup_error(operation: &str, error: io::Error) {
    cyber_core::log::error(
        "sandbox",
        "Windows private temp cleanup failed",
        serde_json::json!({"operation": operation, "error": error.to_string()}),
    );
}

impl AsHandle for ContainerChild {
    fn as_handle(&self) -> BorrowedHandle<'_> {
        self.process.as_handle()
    }
}

impl ContainerChild {
    /// Invocation-owned scratch space, removed when this process owner is dropped.
    pub fn temporary_directory(&self) -> &Path {
        &self._temp.path
    }

    /// Own the process throughout asynchronous waiting, including cancellation.
    /// Discarding this future also discards its process/job owner before first poll.
    pub async fn wait_owned(mut self) -> io::Result<u32> {
        loop {
            match self.wait(Duration::ZERO) {
                Err(error) if error.kind() == io::ErrorKind::TimedOut => {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
                result => return result,
            }
        }
    }

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

struct Attributes<'a> {
    storage: Vec<usize>,
    initialized: bool,
    _capabilities: &'a SECURITY_CAPABILITIES,
    _jobs: &'a [HANDLE; 1],
    _handles: Option<&'a [HANDLE; 3]>,
    _package_policy: Option<&'a u32>,
}

impl<'a> Attributes<'a> {
    fn new(
        capabilities: &'a SECURITY_CAPABILITIES,
        jobs: &'a [HANDLE; 1],
        handles: Option<&'a [HANDLE; 3]>,
        package_policy: Option<&'a u32>,
    ) -> io::Result<Self> {
        let count = 2 + u32::from(handles.is_some()) + u32::from(package_policy.is_some());
        let mut bytes = 0;
        unsafe { InitializeProcThreadAttributeList(null_mut(), count, 0, &mut bytes) };
        if bytes == 0 {
            return Err(io::Error::last_os_error());
        }
        let mut attributes = Self {
            storage: vec![0; bytes.div_ceil(std::mem::size_of::<usize>())],
            initialized: false,
            _capabilities: capabilities,
            _jobs: jobs,
            _handles: handles,
            _package_policy: package_policy,
        };
        // The storage is pointer-aligned and Windows supplied its required size.
        if unsafe { InitializeProcThreadAttributeList(attributes.pointer(), count, 0, &mut bytes) }
            == 0
        {
            return Err(io::Error::last_os_error());
        }
        attributes.initialized = true;
        if let Some(policy) = package_policy {
            attributes.package_policy(policy)?;
        }
        if let Some(handles) = handles {
            attributes.streams(handles)?;
        }
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

    fn package_policy(&mut self, policy: &u32) -> io::Result<()> {
        if unsafe {
            UpdateProcThreadAttribute(
                self.pointer(),
                0,
                PROC_THREAD_ATTRIBUTE_ALL_APPLICATION_PACKAGES_POLICY as usize,
                (policy as *const u32).cast(),
                std::mem::size_of_val(policy),
                null_mut(),
                null(),
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    fn streams(&mut self, handles: &[HANDLE; 3]) -> io::Result<()> {
        if unsafe {
            UpdateProcThreadAttribute(
                self.pointer(),
                0,
                PROC_THREAD_ATTRIBUTE_HANDLE_LIST as usize,
                handles.as_ptr().cast(),
                std::mem::size_of_val(handles),
                null_mut(),
                null(),
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    fn pointer(&mut self) -> windows_sys::Win32::System::Threading::LPPROC_THREAD_ATTRIBUTE_LIST {
        self.storage.as_mut_ptr().cast()
    }
}

impl Drop for Attributes<'_> {
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
