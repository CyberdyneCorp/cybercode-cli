//! macOS Seatbelt profile (`sandbox` → macOS enforcement). Later rules take precedence.

use std::fmt::Write as _;
use std::path::Path;

use crate::{Launch, NetworkMode, Policy};

pub(crate) const SANDBOX_EXEC: &str = "/usr/bin/sandbox-exec";

/// Mach services ordinary command-line tools need: user lookup, TLS trust, logging, prefs.
const MACH_SERVICES: &[&str] = &[
    "com.apple.system.opendirectoryd.libinfo",
    "com.apple.system.opendirectoryd.membership",
    "com.apple.trustd",
    "com.apple.trustd.agent",
    "com.apple.SecurityServer",
    "com.apple.system.logger",
    "com.apple.logd",
    "com.apple.diagnosticd",
    "com.apple.system.notification_center",
    "com.apple.cfprefsd.daemon",
    "com.apple.cfprefsd.agent",
    "com.apple.PowerManagement.control",
];

const BASE: &str = r#"(version 1)
(deny default)
(allow process-exec)
(allow process-fork)
(allow signal (target same-sandbox))
(allow process-info* (target same-sandbox))
(allow user-preference-read)
(allow sysctl-read)
(allow iokit-open (iokit-registry-entry-class "RootDomainUserClient"))
(allow pseudo-tty)
(allow file-read* file-write* file-ioctl (literal "/dev/ptmx"))
(allow file-read* file-write* file-ioctl (regex #"^/dev/ttys[0-9]+$"))
(allow ipc-posix-sem)
(allow ipc-posix-shm)
(allow file-read*)
(allow file-write* (literal "/dev/null") (literal "/dev/zero") (literal "/dev/dtracehelper") (subpath "/dev/fd"))
"#;

pub(crate) fn profile(launch: &Launch) -> String {
    let mut out = String::from(BASE);
    let names: Vec<String> = MACH_SERVICES
        .iter()
        .map(|s| format!("(global-name {})", quote(s)))
        .collect();
    let _ = writeln!(out, "(allow mach-lookup {})", names.join(" "));
    if !launch.unreadable.is_empty() {
        let _ = writeln!(out, "(deny file-read* {})", filters(&launch.unreadable));
    }
    for regex in launch.deny_read.regexes() {
        let _ = writeln!(out, "(deny file-read* (regex #\"{regex}\"))");
    }
    if launch.config.policy == Policy::WorkspaceWrite && !launch.writable.is_empty() {
        let _ = writeln!(out, "(allow file-write* {})", filters(&launch.writable));
        if !launch.read_only.is_empty() {
            let _ = writeln!(out, "(deny file-write* {})", filters(&launch.read_only));
        }
    }
    // Private credentials/captures can live below a writable project or an
    // explicit writable root. These later denials must override that broad
    // write allowance, including renaming an ancestor out of the hidden path.
    if !launch.unreadable.is_empty() {
        let _ = writeln!(out, "(deny file-write* {})", filters(&launch.unreadable));
        let mut ancestors = std::collections::BTreeSet::new();
        for path in &launch.unreadable {
            let canonical = std::path::PathBuf::from(real(path));
            for ancestor in canonical
                .ancestors()
                .skip(1)
                .filter(|path| path.parent().is_some())
            {
                ancestors.insert(ancestor.to_path_buf());
            }
        }
        for ancestor in ancestors {
            let _ = writeln!(
                out,
                "(deny file-write-unlink (literal {}))",
                quote(&ancestor.to_string_lossy())
            );
        }
    }
    // A hidden file must not be renamed, linked or moved out of its pattern and then read
    // under the new name: no writes to a match, no removal of the directories the
    // pattern names.
    for regex in launch.deny_read.regexes() {
        let _ = writeln!(out, "(deny file-write* (regex #\"{regex}\"))");
    }
    for regex in launch.deny_read.ancestor_regexes() {
        let _ = writeln!(out, "(deny file-write-unlink (regex #\"{regex}\"))");
    }
    let proxy_port = match &launch.proxy {
        Some(crate::proxy::Endpoint::Tcp(port)) => Some(*port),
        _ => None,
    };
    match (launch.config.network, proxy_port) {
        (NetworkMode::On, _) => {
            out.push_str("(allow network*)\n(allow system-socket)\n");
            out.push_str("(allow mach-lookup (global-name \"com.apple.dnssd.service\"))\n");
        }
        (NetworkMode::Proxy, Some(port)) => {
            let _ = writeln!(
                out,
                "(allow network-outbound (remote ip \"localhost:{port}\"))"
            );
        }
        _ => {}
    }
    out
}

fn filters(paths: &[std::path::PathBuf]) -> String {
    paths
        .iter()
        .map(|p| format!("(subpath {})", quote(&real(p))))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Seatbelt matches real paths (`/private/var`, not `/var`): canonicalize the longest
/// existing ancestor.
fn real(path: &Path) -> String {
    let mut existing = path.to_path_buf();
    let mut rest = Vec::new();
    while !existing.exists() {
        match (
            existing.file_name().map(|n| n.to_os_string()),
            existing.parent(),
        ) {
            (Some(name), Some(parent)) => {
                rest.push(name);
                existing = parent.to_path_buf();
            }
            _ => break,
        }
    }
    let mut out = std::fs::canonicalize(&existing).unwrap_or(existing);
    for name in rest.into_iter().rev() {
        out.push(name);
    }
    out.to_string_lossy().into_owned()
}

fn quote(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

#[cfg(test)]
mod private_write_tests {
    use super::*;
    #[test]
    fn unreadable_write_and_ancestor_denials_follow_broad_workspace_allowance() {
        let root = tempfile::tempdir().unwrap();
        let state = root.path().join("project/owner-state");
        std::fs::create_dir_all(&state).unwrap();
        let project = state.parent().unwrap().to_path_buf();
        let config = crate::SandboxConfig::resolve(
            &serde_json::json!({}),
            &Default::default(),
            None,
            root.path(),
        );
        let launch = Launch {
            config,
            writable: vec![project.clone()],
            read_only: vec![],
            unreadable: vec![state.clone()],
            deny_read: Default::default(),
            proxy: None,
            helper: None,
        };
        let output = profile(&launch);
        let allowed = output
            .find(&format!(
                "(allow file-write* {})",
                filters(std::slice::from_ref(&project))
            ))
            .unwrap();
        let private = output
            .find(&format!("(deny file-write* {})", filters(&[state])))
            .unwrap();
        let ancestor = output
            .find(&format!(
                "(deny file-write-unlink (literal {}))",
                quote(&real(&project))
            ))
            .unwrap();
        assert!(private > allowed);
        assert!(ancestor > allowed);
        assert!(!output.contains("(deny file-write-unlink (literal \"/\"))"));
    }
}
