//! Dedicated helper-only macOS policy-before-target entry, not execution admission.
//! This private binary module must never be called in the owner process.
//! Callers must first prove profile/image ownership, signature-identity separation,
//! generation authority, environment/namespace isolation and runtime compatibility.
use std::ffi::{CString, OsString};
use std::os::unix::ffi::OsStrExt;
use std::process::ExitCode;

const MAX_ARGS: usize = 256;
const MAX_BYTES: usize = 64 * 1024;

/// Arguments include the native mode flag as argv[0], then absolute profile and
/// target paths and the target's arguments. A successful call replaces this PID.
pub fn run(arguments: impl IntoIterator<Item = OsString>) -> ExitCode {
    let Some(arguments) = bounded(arguments) else {
        eprintln!("cyber-sandbox-exec: invalid native plugin exec arguments");
        return ExitCode::from(64);
    };
    ExitCode::from(match execute(&arguments) {
        13 => 13,
        _ => 2,
    })
}

fn bounded(arguments: impl IntoIterator<Item = OsString>) -> Option<Vec<CString>> {
    let mut strings = Vec::new();
    let mut bytes = 0usize;
    for argument in arguments {
        let value = argument.as_os_str().as_bytes();
        bytes = bytes.checked_add(value.len().checked_add(1)?)?;
        if strings.len() >= MAX_ARGS || bytes > MAX_BYTES {
            return None;
        }
        strings.push(CString::new(value).ok()?);
    }
    if strings.len() < 3 || strings[0].as_bytes() != b"--plugin-native-exec" {
        return None;
    }
    for argument in &strings[1..3] {
        if argument.as_bytes().len() > 4096 || !argument.as_bytes().starts_with(b"/") {
            return None;
        }
    }
    Some(strings)
}

/// The C entry reads these owned NUL-terminated strings without modifying them.
/// They and the null-terminated pointer array remain live for the complete call.
/// The count is bounded to 256, so its conversion to C int cannot truncate.
#[allow(unsafe_code)]
fn execute(arguments: &[CString]) -> i32 {
    #[link(name = "cyber_plugin_exec", kind = "static")]
    unsafe extern "C" {
        fn cyber_plugin_exec(argc: i32, argv: *const *const std::ffi::c_char) -> i32;
    }
    let mut pointers: Vec<_> = arguments.iter().map(|value| value.as_ptr()).collect();
    pointers.push(std::ptr::null());
    // SAFETY: pointer/count/lifetime preconditions are established above; C takes
    // no ownership and either returns an error or performs the replacing exec.
    unsafe { cyber_plugin_exec(arguments.len() as i32, pointers.as_ptr()) }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn argument_bounds_refuse_relative_paths_bypass_and_embedded_nul() {
        for arguments in [
            vec!["--plugin-native-exec", "-", "/bin/sh"],
            vec!["--plugin-native-exec", "/profile", "relative"],
            vec!["wrong-mode", "/profile", "/bin/sh"],
            vec![
                "--plugin-native-exec",
                "/profile",
                "/bin/sh",
                "bad\0argument",
            ],
        ] {
            assert!(bounded(arguments.into_iter().map(OsString::from)).is_none());
        }
        let mut arguments = vec![
            OsString::from("--plugin-native-exec"),
            "/profile".into(),
            "/target".into(),
        ];
        arguments.extend((0..MAX_ARGS).map(|_| OsString::from("arg")));
        assert!(bounded(arguments).is_none());
        let large = vec![
            OsString::from("--plugin-native-exec"),
            "/profile".into(),
            "/target".into(),
            "x".repeat(MAX_BYTES).into(),
        ];
        assert!(bounded(large).is_none());
    }
}
