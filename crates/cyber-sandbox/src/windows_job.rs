//! Windows process-tree ownership. This does not implement sandbox confinement.
#![allow(unsafe_code)]

use std::io;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::process::{Command, ExitCode};
use std::ptr::null;

use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
    SetInformationJobObject,
};
use windows_sys::Win32::System::Threading::GetCurrentProcess;

fn own_process_tree() -> io::Result<OwnedHandle> {
    // Null security attributes create a non-inheritable handle; no name exposes the job.
    let raw = unsafe { CreateJobObjectW(null(), null()) };
    if raw.is_null() {
        return Err(io::Error::last_os_error());
    }
    // CreateJobObjectW transferred this valid handle to us; OwnedHandle closes failures.
    let job = unsafe { OwnedHandle::from_raw_handle(raw) };
    let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
    limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
    // The initialized structure remains alive for this synchronous API call.
    let configured = unsafe {
        SetInformationJobObject(
            job.as_raw_handle(),
            JobObjectExtendedLimitInformation,
            (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
            std::mem::size_of_val(&limits) as u32,
        )
    };
    if configured == 0 {
        return Err(io::Error::last_os_error());
    }
    // Assign before spawning so even immediately-created descendants belong to the job.
    if unsafe { AssignProcessToJobObject(job.as_raw_handle(), GetCurrentProcess()) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(job)
}

pub(super) fn run() -> ExitCode {
    let mut args = std::env::args_os().skip(2);
    let delimiter = args.next();
    let program = args.next();
    if delimiter.as_deref() != Some(std::ffi::OsStr::new("--")) || program.is_none() {
        eprintln!("usage: cyber-sandbox-exec --job -- program [args...]");
        return ExitCode::from(2);
    }
    let _job = match own_process_tree() {
        Ok(job) => job,
        Err(error) => {
            eprintln!("cyber-sandbox-exec: cannot own Windows process tree: {error}");
            return ExitCode::from(69);
        }
    };
    let code = match Command::new(program.unwrap()).args(args).status() {
        Ok(status) => status.code().unwrap_or(1),
        Err(error) => {
            eprintln!("cyber-sandbox-exec: cannot launch command: {error}");
            69
        }
    };
    // Do not drop our own kill-on-close job before setting the helper's exit status.
    // Process exit closes its non-inherited handle and terminates remaining descendants.
    std::process::exit(code)
}
