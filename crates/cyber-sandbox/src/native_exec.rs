//! Dedicated helper-only native policy-before-target entry, not execution admission.
//! This private binary module must never be called in the owner process.
//! Callers must first prove profile/image ownership, platform identity separation,
//! generation authority, environment/namespace isolation and runtime compatibility.
use std::ffi::{CString, OsString};
use std::os::unix::ffi::OsStrExt;
use std::process::ExitCode;

const MAX_ARGS: usize = 256;
const MAX_BYTES: usize = 64 * 1024;

/// Arguments include the native mode flag as argv[0], then absolute profile and
/// target paths and the target's arguments. Linux requires --initial-shell=0/1
/// between the profile and target. A successful call replaces this PID.
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
    let target = if cfg!(target_os = "linux") { 3 } else { 2 };
    if strings.len() <= target || strings[0].as_bytes() != b"--plugin-native-exec" {
        return None;
    }
    #[cfg(target_os = "linux")]
    if !matches!(
        strings[2].as_bytes(),
        b"--initial-shell=0" | b"--initial-shell=1"
    ) {
        return None;
    }
    for argument in [&strings[1], &strings[target]] {
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
    fn valid_native_arguments_require_platform_policy_shape() {
        let mut arguments = vec!["--plugin-native-exec", "/profile"];
        if cfg!(target_os = "linux") {
            arguments.push("--initial-shell=0");
        }
        arguments.extend(["/target", "argument"]);
        assert!(bounded(arguments.into_iter().map(OsString::from)).is_some());
        #[cfg(target_os = "linux")]
        for flag in ["--initial-shell=2", "--initial-shell=", "-"] {
            assert!(
                bounded(
                    ["--plugin-native-exec", "/profile", flag, "/target"]
                        .into_iter()
                        .map(OsString::from)
                )
                .is_none()
            );
        }
    }
    #[test]
    fn argument_bounds_refuse_relative_paths_bypass_and_embedded_nul() {
        for mut arguments in [
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
            if cfg!(target_os = "linux") {
                arguments.insert(2, "--initial-shell=0");
            }
            assert!(bounded(arguments.into_iter().map(OsString::from)).is_none());
        }
    }
    #[test]
    fn argument_count_and_byte_limits_accept_boundary_and_refuse_overflow() {
        let mut prefix = vec![OsString::from("--plugin-native-exec"), "/profile".into()];
        if cfg!(target_os = "linux") {
            prefix.push("--initial-shell=0".into());
        }
        prefix.push("/target".into());
        let mut count = prefix.clone();
        count.resize(MAX_ARGS, "argument".into());
        assert!(bounded(count.clone()).is_some());
        count.push("one-too-many".into());
        assert!(bounded(count).is_none());
        let prefix_bytes: usize = prefix
            .iter()
            .map(|arg| arg.as_os_str().as_bytes().len() + 1)
            .sum();
        let mut bytes = prefix.clone();
        bytes.push("x".repeat(MAX_BYTES - prefix_bytes - 1).into());
        assert!(bounded(bytes).is_some());
        prefix.push("x".repeat(MAX_BYTES - prefix_bytes).into());
        assert!(bounded(prefix).is_none());
    }
}
